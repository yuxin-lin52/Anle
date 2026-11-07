//! 扁平 IR → LLVM IR 发射器
//!
//! 消费 `anle_ir::flat::Flat`，逐指令解释发射。
//! 语义全部复用树后端 `Lower` 的臂：读走 load_place，
//! 写走 store_place，运算走 binop，内置走 builtin。
//! 扁平侧只补三样：值迹（指令号 → LLVM 值）、
//! 块参数 φ 的前区承接、赋值收口（Nop）的存与回填。

use crate::{slot_idx_of, 单选局部, Dbg, Lower};
use anle_ir::flat::{Flat, FlatFn, Op, Tag};
use anle_ir::{Exit, Function, MapKey, Place, Program, Ty, Value};
use inkwell::context::Context;
use inkwell::debug_info::{AsDIScope, DIFlags, DIFlagsConstants};
use inkwell::module::Module;
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};
use inkwell::values::BasicValueEnum;
use inkwell::AddressSpace;
use inkwell::OptimizationLevel;
use std::path::Path;

//nt: 序列元素类型
fn seq_elem_ty(t: &str) -> Option<String> {
    let (head, inner) = t.split_once('(')?;
    let tail = inner.strip_suffix(')')?;
    match head {
        "list" | "que" | "stack" => Some(tail.trim().to_string()),
        _ => None,
    }
}

//nt: 槽宽与符号
fn elem_esz_sgn(t: &str) -> (i64, i64) {
    match t {
        "int_8" => (1, 1),
        "uint_8" => (1, 0),
        "int_16" => (2, 1),
        "uint_16" => (2, 0),
        "int_32" => (4, 1),
        "uint_32" => (4, 0),
        "int_64" => (8, 1),
        "uint_64" => (8, 0),
        _ => (8, 1),
    }
}

//nt: map 值类型
fn map_value_ty(t: &str) -> Option<String> {
    let inner = t.strip_prefix("map(")?.strip_suffix(')')?;
    let mut depth = 0i32;
    for (i, c) in inner.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => return Some(inner[i + 1..].trim().to_string()),
            _ => {}
        }
    }
    None
}

//nt: 指令值迹与定义表
struct 迹<'ctx> {
    vals: Vec<Option<BasicValueEnum<'ctx>>>,
    //nt: 局部号 → 定义指令号
    def: Vec<Option<u32>>,
}

impl<'ctx> 迹<'ctx> {
    fn new(n: usize) -> Self {
        Self { vals: vec![None; n], def: Vec::new() }
    }

    fn 取(&self, id: u32) -> Result<BasicValueEnum<'ctx>, String> {
        self.vals
            .get(id as usize)
            .and_then(|v| *v)
            .ok_or_else(|| format!("指令 {} 没有值", id))
    }

    fn 存(&mut self, id: u32, v: BasicValueEnum<'ctx>) {
        let i = id as usize;
        if i >= self.vals.len() {
            self.vals.resize(i + 1, None);
        }
        self.vals[i] = Some(v);
    }

    //nt: 记定义
    fn 记(&mut self, lid: usize, id: u32) {
        let i = lid;
        if i >= self.def.len() {
            self.def.resize(i + 1, None);
        }
        self.def[i] = Some(id);
    }

    //nt: 定义指令反查局部
    fn 反查(&self, id: u32) -> Option<usize> {
        self.def.iter().position(|x| *x == Some(id))
    }

    //nt: 取定义
    fn 定义(&self, lid: usize) -> Option<u32> {
        self.def.get(lid).and_then(|x| *x)
    }
}

//nt: 扁平发射入口
pub fn emit_flat_object(
    p: &Program,
    flat: &Flat,
    path: &Path,
    opt: u32,
    debug: bool,
    file: &str,
) -> Result<(), String> {
    let opt = match opt {
        0 => OptimizationLevel::None,
        1 => OptimizationLevel::Less,
        2 => OptimizationLevel::Default,
        _ => OptimizationLevel::Aggressive,
    };
    let ctx = Context::create();
    let module = ctx.create_module("anle");
    Target::initialize_all(&InitializationConfig::default());
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| e.to_string())?;
    let tm = target
        .create_target_machine(&triple, "generic", "", opt, RelocMode::PIC, CodeModel::Default)
        .ok_or_else(|| "建不了 LLVM 目标机".to_string())?;
    module.set_triple(&triple);
    module.set_data_layout(&tm.get_target_data().get_data_layout());
    let td = tm.get_target_data();
    let builder = ctx.create_builder();
    let dbg = if debug {
        use inkwell::debug_info::{DWARFEmissionKind, DWARFSourceLanguage};
        use inkwell::module::FlagBehavior;
        let (dib, unit) = module.create_debug_info_builder(
            false,
            DWARFSourceLanguage::C,
            file,
            "",
            "anle",
            opt != OptimizationLevel::None,
            "",
            0,
            "",
            DWARFEmissionKind::Full,
            0,
            false,
            false,
            "",
            "",
        );
        let df = dib.create_file(file, "");
        module.add_basic_value_flag(
            "Debug Info Version",
            FlagBehavior::Error,
            ctx.i32_type().const_int(inkwell::debug_info::debug_metadata_version() as u64, false),
        );
        Some(Dbg { dib, file: df, unit, scope: None })
    } else {
        None
    };
    //nt: 各文件源码行
    let mut src_lines = std::collections::BTreeMap::new();
    if let Ok(t) = std::fs::read_to_string(file) {
        src_lines.insert(file.to_string(), t.lines().map(|s| s.to_string()).collect());
    }
    for f in &p.functions {
        if !f.file.is_empty() && !src_lines.contains_key(&f.file) {
            if let Ok(t) = std::fs::read_to_string(&f.file) {
                src_lines.insert(f.file.clone(), t.lines().map(|s| s.to_string()).collect());
            }
        }
    }
    let mut lo = Lower {
        ctx: &ctx,
        module,
        builder,
        release: !debug,
        funcs: std::collections::BTreeMap::new(),
        strings: std::collections::BTreeMap::new(),
        want: None,
        td,
        structs: std::collections::BTreeMap::new(),
        type_fields: std::collections::BTreeMap::new(),
        值槽: std::cell::RefCell::new(Vec::new()),
        块参数phi: std::cell::RefCell::new(std::collections::BTreeMap::new()),
        列表型: None,
        cur_line: 0,
        entry_file: file.to_string(),
        src_lines,
        dbg,
        owned: p.owned_elem.clone(),
        循环环: std::cell::RefCell::new(Vec::new()),
        值入列: p.值入列.clone(),
    };
    lo.declare_types(p)?;
    lo.declare(p)?;
    for (i, f) in p.functions.iter().enumerate() {
        let ffn = flat.fns.get(i).ok_or_else(|| format!("扁平体缺函数 {}", f.name))?;
        lo.lower_fn_flat(f, ffn, p)?;
    }
    //nt: 先收尾调试信息
    lo.dbg = None;
    lo.module.verify().map_err(|e| format!("LLVM 校验失败：{}", e.to_string()))?;
    //nt: IR优化管线
    if opt != OptimizationLevel::None {
        use inkwell::passes::PassBuilderOptions;
        let 级 = match opt {
            OptimizationLevel::Less => "default<O1>",
            OptimizationLevel::Default => "default<O2>",
            _ => "default<O3>",
        };
        lo.module
            .run_passes(级, &tm, PassBuilderOptions::create())
            .map_err(|e| e.to_string())?;
    }
    //nt: 循环元数据在优化后挂
    lo.挂循环元数据()?;
    //nt: 优化后快照
    let _ = std::fs::write(path.with_extension("ll"), lo.module.print_to_string().to_string());
    tm.write_to_file(&lo.module, FileType::Object, path).map_err(|e| e.to_string())
}

impl<'ctx> Lower<'ctx> {
    //nt: 优化后挂循环元数据
    //nt: 回边 = 指回头的分支
    fn 挂循环元数据(&self) -> Result<(), String> {
        let 环 = self.循环环.borrow();
        for (fv, head, n) in 环.iter() {
            let trip = self
                .ctx
                .metadata_node(&[self.ctx.i32_type().const_int(*n, false).into()]);
            let inner = self.ctx.metadata_node(&[
                self.ctx
                    .metadata_string("llvm.loop.estimated_trip_count")
                    .into(),
                trip.into(),
            ]);
            let loopmd = self.ctx.metadata_node(&[inner.into()]);
            let kind = self.ctx.get_kind_id("llvm.loop");
            let 块表 = fv.get_basic_blocks();
            let mut first = true;
            for bb in 块表 {
                //nt: 入口块与头块不挂
                if first || bb.get_name().to_string_lossy() == *head {
                    first = false;
                    continue;
                }
                first = false;
                let 回头 = bb
                    .get_terminator()
                    .map(|t| {
                        (0..t.get_num_operands()).any(|i| {
                            t.get_operand(i)
                                .and_then(|o| o.block())
                                .map(|s| s.get_name().to_string_lossy() == *head)
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);
                if 回头 {
                    bb.get_terminator()
                        .unwrap()
                        .set_metadata(loopmd, kind)
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }

    //nt: 扁平解释一函数
    pub fn lower_fn_flat(
        &mut self,
        f: &Function,
        ffn: &FlatFn,
        p: &Program,
    ) -> Result<(), String> {
        //nt: 逃逸集按函数清
        crate::ESCAPED.with(|s| s.borrow_mut().clear());
        let fv = *self.funcs.get(&f.name).ok_or_else(|| format!("缺函数 {}", f.name))?;
        if let Some(d) = self.dbg.as_mut() {
            let ft = d.dib.create_subroutine_type(d.file, None, &[], DIFlags::PUBLIC);
            let sp = d.dib.create_function(
                d.unit.as_debug_info_scope(),
                &f.name,
                Some(&f.name),
                d.file,
                1,
                ft,
                true,
                true,
                0,
                DIFlags::PUBLIC,
                true,
            );
            fv.set_subprogram(sp);
            d.scope = Some(sp);
        }
        let entry = self.ctx.append_basic_block(fv, "entry");
        self.builder.position_at_end(entry);
        self.builder.unset_current_debug_location();
        //nt: 当前源文件
        let fg = self.ptr_global("anle_cur_file");
        let 文件名 = f.file.rsplit(['/', '\\']).next().unwrap_or("");
        let fval = if 文件名.is_empty() {
            self.ctx.ptr_type(AddressSpace::default()).const_null()
        } else {
            self.fmt(文件名)
        };
        self.builder.build_store(fg.as_pointer_value(), fval).map_err(|e| e.to_string())?;
        let mut allocas: Vec<Option<inkwell::values::PointerValue<'ctx>>> = vec![None; f.locals.len()];
        //nt: 塊參數集合
        let mut 塊參數集: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        for b in &f.blocks {
            for (pid, _) in &b.params {
                塊參數集.insert(*pid);
            }
        }
        self.块参数phi.borrow_mut().clear();
        //nt: 单赋值局部走值槽
        let 值槽集 = 单选局部(f);
        self.值槽.borrow_mut().clear();
        self.值槽.borrow_mut().resize(f.locals.len(), None);
        //nt: 槽键索引与后端一致
        let slot_idx = slot_idx_of(p);
        for (i, l) in f.locals.iter().enumerate() {
            if l.is_param {
                continue;
            }
            //nt: 塊參數走φ不建棧位
            if 塊參數集.contains(&i) {
                continue;
            }
            //nt: 跨块槽用全局
            if let Some(n) = &l.name {
                if let Some(idx) = slot_idx.get(n) {
                    let g = self.slot_global(n, &l.ty, *idx)?;
                    allocas[i] = Some(g);
                    continue;
                }
            }
            if 值槽集.contains(&i) {
                continue; //nt: 值槽不建栈位
            }
            let t = self.ty(&l.ty);
            let a = self.alloca(t, &format!("v{}", i)).map_err(|e| e.to_string())?;
            //nt: 指针栈位先置空
            if t.is_pointer_type() {
                self.builder
                    .build_store(a, t.into_pointer_type().const_null())
                    .map_err(|e| e.to_string())?;
            }
            allocas[i] = Some(a);
        }
        let mut pi = 0u32;
        for (i, l) in f.locals.iter().enumerate() {
            if !l.is_param {
                continue;
            }
            let t = self.ty(&l.ty);
            let a = self.alloca(t, &format!("p{}", i)).map_err(|e| e.to_string())?;
            let pv = fv.get_nth_param(pi).ok_or_else(|| "缺 LLVM 形参".to_string())?;
            self.builder.build_store(a, pv).map_err(|e| e.to_string())?;
            allocas[i] = Some(a);
            pi += 1;
        }
        //nt: 变量级调试
        self.发变量调试(f, &allocas)?;
        let mut bbs = Vec::new();
        for b in &f.blocks {
            bbs.push(self.ctx.append_basic_block(fv, &format!("bb{}", b.id)));
        }
        if let Some(first) = bbs.first() {
            self.builder.build_unconditional_branch(*first).map_err(|e| e.to_string())?;
        }
        //nt: 先建全部φ 再发射
        for (bi, b) in f.blocks.iter().enumerate() {
            self.builder.position_at_end(bbs[bi]);
            for (pid, pty) in &b.params {
                let t = self.ty(pty);
                let phi = self
                    .builder
                    .build_phi(t, &format!("p{}", pid))
                    .map_err(|e| e.to_string())?;
                self.块参数phi.borrow_mut().insert(*pid, phi);
                let bv: BasicValueEnum<'ctx> = phi.as_basic_value();
                let n = self.值槽.borrow().len();
                if *pid < n {
                    self.值槽.borrow_mut()[*pid] = Some(bv);
                }
            }
        }
        let mut 迹 = 迹::new(ffn.insts.len());
        //nt: 前区读局部接φ
        let pre_end = ffn.blocks.first().map(|b| b.start as usize).unwrap_or(ffn.insts.len());
        let mut pids: Vec<usize> = Vec::new();
        for b in &f.blocks {
            for (pid, _) in &b.params {
                pids.push(*pid);
            }
        }
        let mut pk = 0usize;
        for iid in 0..pre_end {
            if ffn.insts[iid].tag != Tag::ReadLocal {
                continue;
            }
            let pid = *pids.get(pk).ok_or_else(|| "块参数与扁平前区不齐".to_string())?;
            pk += 1;
            let v = self
                .值槽
                .borrow()
                .get(pid)
                .and_then(|x| *x)
                .ok_or_else(|| "块参数缺φ".to_string())?;
            迹.存(iid as u32, v);
        }
        //nt: 赋值目标预扫
        let mut want_of: Vec<Option<Ty>> = vec![None; ffn.insts.len()];
        for (iid, inst) in ffn.insts.iter().enumerate() {
            if inst.tag != Tag::Nop {
                continue;
            }
            if let Some(pl) = ffn.places.get(iid).and_then(|x| x.clone()) {
                let vi = inst.a as usize;
                if vi < want_of.len() {
                    want_of[vi] = Some(self.place_ty(f, &pl));
                }
            }
        }
        for (bi, b) in f.blocks.iter().enumerate() {
            self.builder.position_at_end(bbs[bi]);
            let 起 = ffn
                .blocks
                .get(bi)
                .map(|x| x.start as usize)
                .unwrap_or(ffn.insts.len());
            let 止 = ffn
                .blocks
                .get(bi + 1)
                .map(|x| x.start as usize)
                .unwrap_or(ffn.insts.len());
            for iid in 起..止 {
                let inst = ffn.insts[iid];
                self.cur_line = ffn.lines.get(iid).copied().unwrap_or(0) as usize;
                self.want = want_of[iid].clone();
                //nt: 调试位置随指令
                if let Some(d) = self.dbg.as_ref() {
                    if let Some(sco) = d.scope {
                        if self.cur_line > 0 {
                            let loc = d.dib.create_debug_location(
                                self.ctx,
                                self.cur_line as u32,
                                ffn.cols.get(iid).copied().unwrap_or(0),
                                sco.as_debug_info_scope(),
                                None,
                            );
                            self.builder.set_current_debug_location(loc);
                        }
                    }
                    //nt: 溯源全局随指令
                    let ln = self.cur_line as u64;
                    let g = self.line_global();
                    self.builder
                        .build_store(g.as_pointer_value(), self.ctx.i64_type().const_int(ln, false))
                        .map_err(|e| e.to_string())?;
                    let gc = self.i64_global("anle_col");
                    self.builder
                        .build_store(
                            gc.as_pointer_value(),
                            self.ctx
                                .i64_type()
                                .const_int(ffn.cols.get(iid).copied().unwrap_or(0) as u64, false),
                        )
                        .map_err(|e| e.to_string())?;
                    let gl = self.i64_global("anle_len");
                    self.builder
                        .build_store(
                            gl.as_pointer_value(),
                            self.ctx
                                .i64_type()
                                .const_int(ffn.lens.get(iid).copied().unwrap_or(0) as u64, false),
                        )
                        .map_err(|e| e.to_string())?;
                    //nt: 当前源码行
                    let key = if f.file.is_empty() { self.entry_file.clone() } else { f.file.clone() };
                    let 行文本 = self
                        .src_lines
                        .get(&key)
                        .and_then(|ls| ls.get(self.cur_line.saturating_sub(1)))
                        .cloned()
                        .unwrap_or_default();
                    let sv = if 行文本.is_empty() {
                        self.ctx.ptr_type(AddressSpace::default()).const_null()
                    } else {
                        self.fmt(&行文本)
                    };
                    let gs = self.ptr_global("anle_src");
                    self.builder
                        .build_store(gs.as_pointer_value(), sv)
                        .map_err(|e| e.to_string())?;
                }
                self.解释一条(f, ffn, p, iid, &allocas, &mut 迹)?;
                self.want = None;
            }
            match &b.exit {
                Exit::Return(v) => {
                    if f.name == "main" {
                        self.builder
                            .build_return(Some(&self.ctx.i32_type().const_int(0, false)))
                            .map_err(|e| e.to_string())?;
                    } else if let Some(v) = v {
                        let mut x = self.value(f, &allocas, v)?;
                        //nt: fn自有对象复印重绑
                        //nt: 复印目标 = 调用方地皮
                        if let Value::Local(lid) = v {
                            if f.locals.get(*lid).map(|l| l.fn_owned).unwrap_or(false) {
                                if let BasicValueEnum::PointerValue(pv) = x {
                                    let base = f.locals[*lid].ty.0.trim_end_matches('*').to_string();
                                    if let Some(st) = self.structs.get(&base).cloned() {
                                        let 已逃 =
                                            crate::ESCAPED.with(|s| s.borrow().contains(lid));
                                        if !已逃 {
                                            let size = self.td.get_store_size(&st) as i64;
                                            let c = self.call_rt(
                                                "anle_copy_to_parent",
                                                &[self.pty(), self.ity()],
                                                Some(self.pty()),
                                                &[pv.into(), self.ci(size)],
                                            )?;
                                            if let Some(a) = allocas[*lid] {
                                                self.builder.build_store(a, c).map_err(|e| e.to_string())?;
                                            } else if let Some(x) = self.值槽.borrow_mut().get_mut(*lid) {
                                                //nt: 值槽也回写
                                                *x = Some(c.into());
                                            }
                                            crate::ESCAPED.with(|s| s.borrow_mut().insert(*lid));
                                            x = c;
                                        }
                                    }
                                }
                            }
                        }
                        self.builder.build_return(Some(&x)).map_err(|e| e.to_string())?;
                    } else {
                        self.builder.build_return(None).map_err(|e| e.to_string())?;
                    }
                }
                Exit::Unreachable => {
                    //nt: 不可达先 abort
                    self.call_rt("abort", &[], None, &[])?;
                    self.builder.build_unreachable().map_err(|e| e.to_string())?;
                }
                Exit::Branch { cond, targets } => {
                    if targets.is_empty() {
                        return Err("分支没有目标块".into());
                    }
                    let c = self.value(f, &allocas, cond)?;
                    let i1 = self.to_bool(c)?;
                    if targets.len() == 1 {
                        let cur = self
                            .builder
                            .get_insert_block()
                            .ok_or_else(|| "分支不在基本块里".to_string())?;
                        self.邊φ(f, &allocas, &targets[0], cur)?;
                        self.builder
                            .build_unconditional_branch(bbs[targets[0].target])
                            .map_err(|e| e.to_string())?;
                    } else {
                        let dest = [bbs[targets[0].target], bbs[targets[1].target]];
                        for k in 0..2 {
                            let cur = self
                                .builder
                                .get_insert_block()
                                .ok_or_else(|| "分支不在基本块里".to_string())?;
                            self.邊φ(f, &allocas, &targets[k], cur)?;
                        }
                        self.builder
                            .build_conditional_branch(i1, dest[0], dest[1])
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
            //nt: 头块上限登记
            if let Some(n) = b.max {
                self.循环环
                    .borrow_mut()
                    .push((fv, format!("bb{}", b.id), n));
            }
        }
        Ok(())
    }

    //nt: 解释一条指令
    fn 解释一条(
        &mut self,
        f: &Function,
        ffn: &FlatFn,
        p: &Program,
        iid: usize,
        allocas: &[Option<inkwell::values::PointerValue<'ctx>>],
        迹: &mut 迹<'ctx>,
    ) -> Result<(), String> {
        let inst = ffn.insts[iid];
        let 记 = |迹: &mut 迹<'ctx>, v: Option<BasicValueEnum<'ctx>>| {
            if iid < 迹.vals.len() {
                迹.vals[iid] = v;
            }
        };
        match inst.tag {
            Tag::ConstInt => {
                let v = ffn.ints.get(inst.a as usize).copied().unwrap_or(0);
                let x = self.value(f, allocas, &Value::Int(v))?;
                记(迹, Some(x));
            }
            Tag::ConstFloat => {
                let v = ffn.floats.get(inst.a as usize).copied().unwrap_or(0.0);
                let x = self.value(f, allocas, &Value::Float(v))?;
                记(迹, Some(x));
            }
            Tag::ConstStr => {
                let v = ffn.strings.get(inst.a as usize).cloned().unwrap_or_default();
                let x = self.value(f, allocas, &Value::Str(v))?;
                记(迹, Some(x));
            }
            Tag::ConstBool => {
                let x = self.value(f, allocas, &Value::Bool(inst.a != 0))?;
                记(迹, Some(x));
            }
            Tag::ConstNull => {
                let x = self.value(f, allocas, &Value::Null)?;
                记(迹, Some(x));
            }
            Tag::ReadLocal => {
                let x = self.value(f, allocas, &Value::Local(inst.b as usize))?;
                记(迹, Some(x));
            }
            Tag::BinOp => {
                let op = Op::from_u32(inst.a).ok_or_else(|| "未知运算".to_string())?;
                let li = ffn.extra.get(inst.b as usize).copied().ok_or_else(|| "缺左目".to_string())?;
                let ri = ffn
                    .extra
                    .get(inst.b as usize + 1)
                    .copied()
                    .ok_or_else(|| "缺右目".to_string())?;
                let lv = self.flat值2(迹, ffn,li).ok_or_else(|| "运算左目不是局部或常量".to_string())?;
                let rv = self.flat值2(迹, ffn,ri).ok_or_else(|| "运算右目不是局部或常量".to_string())?;
                let a = 迹.取(li)?;
                let b = 迹.取(ri)?;
                let r = self.binop(f, op.to_binop(), &lv, &rv, a, b)?;
                记(迹, Some(r));
            }
            Tag::Call => {
                let name = ffn.strings.get(inst.a as usize).cloned().unwrap_or_default();
                let ids: Vec<u32> = (0..inst.c)
                    .filter_map(|k| ffn.extra.get(inst.b as usize + k as usize).copied())
                    .collect();
                let vs: Vec<Value> = ids
                    .iter()
                    .map(|&id| {
                        self.flat值2(迹, ffn,id)
                            .ok_or_else(|| format!("调用 {} 的实参不是局部或常量", name))
                    })
                    .collect::<Result<_, _>>()?;
                if name == "print" {
                    self.print(f, allocas, &vs)?;
                } else if let Some(v) = self.builtin(f, allocas, name.as_str(), &vs)? {
                    记(迹, Some(v));
                } else {
                    let mut vals = Vec::new();
                    for &id in &ids {
                        vals.push(迹.取(id)?);
                    }
                    let r = self.call(f, name.as_str(), &vals)?;
                    记(迹, Some(r));
                }
            }
            Tag::FnRef => {
                let name = ffn.strings.get(inst.a as usize).cloned().unwrap_or_default();
                let fv = *self
                    .funcs
                    .get(&name)
                    .ok_or_else(|| format!("LLVM 后端不认识函数 {}", name))?;
                记(迹, Some(fv.as_global_value().as_pointer_value().into()));
            }
            Tag::CallIndirect => {
                let cid = inst.a;
                let cv = self.flat值2(迹, ffn,cid).ok_or_else(|| "被调者不是局部".to_string())?;
                let cty = self.arg_ty(f, &cv);
                let raw = 迹.取(cid)?;
                let pv = match raw {
                    BasicValueEnum::PointerValue(pp) => pp,
                    BasicValueEnum::IntValue(i) => self
                        .builder
                        .build_int_to_ptr(i, self.ctx.ptr_type(AddressSpace::default()), "fp")
                        .map_err(|e| e.to_string())?,
                    _ => return Err("间接调用的被调者不是函数值".into()),
                };
                let (ps, ret) =
                    fn_sig(&cty).ok_or_else(|| format!("认不出函数类型 {}", cty))?;
                let mps: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
                    ps.iter().map(|t| self.ty(t).into()).collect();
                let fty = match ret.as_ref().map(|t| self.ty(t)) {
                    None => self.ctx.void_type().fn_type(&mps, false),
                    Some(inkwell::types::BasicTypeEnum::IntType(t)) => t.fn_type(&mps, false),
                    Some(inkwell::types::BasicTypeEnum::FloatType(t)) => t.fn_type(&mps, false),
                    Some(inkwell::types::BasicTypeEnum::PointerType(t)) => t.fn_type(&mps, false),
                    Some(inkwell::types::BasicTypeEnum::StructType(t)) => t.fn_type(&mps, false),
                    Some(_) => return Err("不支持的 LLVM 返回类型".into()),
                };
                let ids: Vec<u32> = (0..inst.c)
                    .filter_map(|k| ffn.extra.get(inst.b as usize + k as usize).copied())
                    .collect();
                let mut av: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = Vec::new();
                for &id in &ids {
                    av.push(迹.取(id)?.into());
                }
                let c = self
                    .builder
                    .build_indirect_call(fty, pv, &av, "icall")
                    .map_err(|e| e.to_string())?;
                记(迹, c.try_as_basic_value().basic());
            }
            Tag::ListLit => {
                let ids: Vec<u32> = (0..inst.b)
                    .filter_map(|k| ffn.extra.get(inst.a as usize + k as usize).copied())
                    .collect();
                let elems: Vec<Value> = ids
                    .iter()
                    .map(|&id| self.flat值2(迹, ffn,id).ok_or_else(|| "列表元素不是局部或常量".to_string()))
                    .collect::<Result<_, _>>()?;
                let r = self.list_lit(f, allocas, &elems)?;
                记(迹, Some(r));
            }
            Tag::MapLit => {
                let mut pairs: Vec<(MapKey, Value)> = Vec::new();
                let mut k = 0usize;
                while k + 1 < inst.b as usize {
                    let kid = ffn
                        .extra
                        .get(inst.a as usize + k)
                        .copied()
                        .ok_or_else(|| "缺映射键".to_string())?;
                    let vid = ffn
                        .extra
                        .get(inst.a as usize + k + 1)
                        .copied()
                        .ok_or_else(|| "缺映射值".to_string())?;
                    let key = match ffn.insts.get(kid as usize).map(|x| x.tag) {
                        Some(Tag::ConstStr) => {
                            MapKey::Str(ffn.strings.get(ffn.insts[kid as usize].a as usize).cloned().unwrap_or_default())
                        }
                        Some(Tag::ConstInt) => {
                            MapKey::Int(ffn.ints.get(ffn.insts[kid as usize].a as usize).copied().unwrap_or(0))
                        }
                        _ => return Err("映射键不是常量".into()),
                    };
                    let v = self
                        .flat值2(迹, ffn,vid)
                        .ok_or_else(|| "映射值不是局部或常量".to_string())?;
                    pairs.push((key, v));
                    k += 2;
                }
                let r = self.map_lit(f, allocas, &pairs)?;
                记(迹, Some(r));
            }
            Tag::ListGet => {
                let lv = self
                    .flat值2(迹, ffn,inst.a)
                    .ok_or_else(|| "列表不是局部或常量".to_string())?;
                let iv = self
                    .flat值2(迹, ffn,inst.b)
                    .ok_or_else(|| "下标不是局部或常量".to_string())?;
                let r = self.list_get(f, allocas, &lv, &iv)?;
                记(迹, Some(r));
            }
            Tag::MapGet => {
                let mv = self
                    .flat值2(迹, ffn,inst.a)
                    .ok_or_else(|| "映射不是局部或常量".to_string())?;
                let kv = self
                    .flat值2(迹, ffn,inst.b)
                    .ok_or_else(|| "键不是局部或常量".to_string())?;
                let mt = self.arg_ty(f, &mv);
                let vt = map_value_ty(&mt).unwrap_or_else(|| "int".into());
                let raw = self.value(f, allocas, &mv)?;
                let kraw = self.value(f, allocas, &kv)?;
                let kvv = self.key_ll(kraw)?;
                let out = self.call_rt(
                    "anle_map_at",
                    &[self.pty(), self.ity()],
                    Some(self.ity()),
                    &[raw, kvv],
                )?;
                let r = self.from_slot(out, &vt)?;
                记(迹, Some(r));
            }
            //nt: 链尾读取按记位走
            Tag::GetField | Tag::GetIndex | Tag::Deref => {
                if let Some(pl) = ffn.places.get(iid).and_then(|x| x.clone()) {
                    let x = self.load_place(f, allocas, &pl)?;
                    记(迹, Some(x));
                }
            }
            //nt: 链中步走收口
            Tag::GetFieldAddr | Tag::GetIndexAddr => {}
            Tag::Ref => {
                let pl = ffn
                    .places
                    .get(iid)
                    .and_then(|x| x.clone())
                    .ok_or_else(|| "取址没有位置".to_string())?;
                let a = self.place_ptr(allocas, &pl, f)?;
                记(迹, Some(a.into()));
            }
            Tag::VariantLit => {
                let tyname = ffn
                    .strings
                    .get((inst.a >> 8) as usize)
                    .cloned()
                    .unwrap_or_default();
                let tag = (inst.a & 0xFF) as usize;
                let ids: Vec<u32> = (0..inst.c)
                    .filter_map(|k| ffn.extra.get(inst.b as usize + k as usize).copied())
                    .collect();
                let args: Vec<Value> = ids
                    .iter()
                    .map(|&id| self.flat值2(迹, ffn,id).ok_or_else(|| "变体载荷不是局部或常量".to_string()))
                    .collect::<Result<_, _>>()?;
                let tys: Vec<Ty> = p
                    .types
                    .iter()
                    .find(|st| st.name == tyname)
                    .and_then(|st| st.variants.get(tag))
                    .map(|v| v.fields.iter().map(|(_, t)| t.clone()).collect())
                    .ok_or_else(|| format!("缺和类型变体 {}", tyname))?;
                let r = self.variant_lit(f, allocas, &tyname, tag, &args, &tys)?;
                记(迹, Some(r));
            }
            Tag::VariantTag => {
                let v = self
                    .flat值2(迹, ffn,inst.a)
                    .ok_or_else(|| "变体不是局部或常量".to_string())?;
                let sv = self.struct_value(f, allocas, &v)?;
                let x = self
                    .builder
                    .build_extract_value(sv, 0, "tag")
                    .map_err(|e| e.to_string())?;
                记(迹, Some(x));
            }
            Tag::VariantGet => {
                let v = self
                    .flat值2(迹, ffn,inst.a)
                    .ok_or_else(|| "变体不是局部或常量".to_string())?;
                let tyid = ffn
                    .extra
                    .get(inst.b as usize)
                    .copied()
                    .ok_or_else(|| "缺载荷类型".to_string())?;
                let idx = ffn
                    .extra
                    .get(inst.b as usize + 1)
                    .copied()
                    .ok_or_else(|| "缺载荷下标".to_string())?;
                let tyname = ffn.strings.get(tyid as usize).cloned().unwrap_or_default();
                let sv = self.struct_value(f, allocas, &v)?;
                let raw = self
                    .builder
                    .build_extract_value(sv, idx + 1, "pay")
                    .map_err(|e| e.to_string())?;
                let r = self.from_slot(raw, &tyname)?;
                记(迹, Some(r));
            }
            Tag::Nop => {
                let lhs = ffn
                    .places
                    .get(iid)
                    .and_then(|x| x.clone())
                    .ok_or_else(|| "赋值收口没有位置".to_string())?;
                let vi = inst.a;
                let mut v = 迹.取(vi)?;
                //nt: 串赋值即堆副本
                if self.place_ty(f, &lhs).0 == "str" && inst.c != 0 {
                    v = self.call_rt("anle_str_copy", &[self.pty()], Some(self.pty()), &[v])?;
                }
                self.store_place(f, allocas, &lhs, v)?;
                self.容器移交flat(f, allocas, &lhs, ffn, iid, 迹)?;
                if lhs.proj.is_empty() {
                    //nt: 回填值迹
                    let cur = self.load_place(f, allocas, &Place::local(lhs.base))?;
                    迹.存(vi, cur);
                    记(迹, Some(cur));
                    迹.记(lhs.base, vi);
                } else if let Some(dv) = 迹.定义(lhs.base) {
                    //nt: 改写后同步值迹
                    let cur = self.load_place(f, allocas, &Place::local(lhs.base))?;
                    迹.存(dv, cur);
                }
            }
            Tag::ListSetStmt => {
                let lid = ffn.extra.get(inst.a as usize).copied().ok_or_else(|| "缺列表".to_string())?;
                let iid2 = ffn.extra.get(inst.a as usize + 1).copied().ok_or_else(|| "缺下标".to_string())?;
                let vid = ffn.extra.get(inst.a as usize + 2).copied().ok_or_else(|| "缺值".to_string())?;
                let lv = self
                    .flat值2(迹, ffn,lid)
                    .ok_or_else(|| "列表不是局部或常量".to_string())?;
                let iv = self
                    .flat值2(迹, ffn,iid2)
                    .ok_or_else(|| "下标不是局部或常量".to_string())?;
                let vv = self
                    .flat值2(迹, ffn,vid)
                    .ok_or_else(|| "值不是局部或常量".to_string())?;
                let lt = self.arg_ty(f, &lv);
                let et = seq_elem_ty(&lt).unwrap_or_else(|| "int".into());
                let lraw = self.value(f, allocas, &lv)?;
                let iraw = self.value(f, allocas, &iv)?;
                let vraw = self.value(f, allocas, &vv)?;
                if let Some(st) = self.structs.get(&et).copied() {
                    //nt: 聚合按地址
                    let a = self.alloca(st, "as").map_err(|e| e.to_string())?;
                    self.builder.build_store(a, vraw).map_err(|e| e.to_string())?;
                    self.call_rt(
                        "anle_list_setat_from",
                        &[self.pty(), self.ity(), self.pty()],
                        None,
                        &[lraw, iraw, a.into()],
                    )?;
                } else {
                    let slot = if et == "str" {
                        let c = self.call_rt("anle_str_copy", &[self.pty()], Some(self.pty()), &[vraw])?;
                        self.to_slot(c)?
                    } else {
                        self.to_slot(vraw)?
                    };
                    self.call_rt(
                        "anle_list_setat",
                        &[self.pty(), self.ity(), self.ity()],
                        None,
                        &[lraw, iraw, slot.into()],
                    )?;
                }
            }
            Tag::MapSetStmt => {
                let mid = ffn.extra.get(inst.a as usize).copied().ok_or_else(|| "缺映射".to_string())?;
                let kid = ffn.extra.get(inst.a as usize + 1).copied().ok_or_else(|| "缺键".to_string())?;
                let vid = ffn.extra.get(inst.a as usize + 2).copied().ok_or_else(|| "缺值".to_string())?;
                let mv = self
                    .flat值2(迹, ffn,mid)
                    .ok_or_else(|| "映射不是局部或常量".to_string())?;
                let kv = self
                    .flat值2(迹, ffn,kid)
                    .ok_or_else(|| "键不是局部或常量".to_string())?;
                let vv = self
                    .flat值2(迹, ffn,vid)
                    .ok_or_else(|| "值不是局部或常量".to_string())?;
                let mt = self.arg_ty(f, &mv);
                let vt = map_value_ty(&mt).unwrap_or_else(|| "int".into());
                let mraw = self.value(f, allocas, &mv)?;
                let kraw = self.value(f, allocas, &kv)?;
                let kraw = self.key_ll(kraw)?;
                let vraw = self.value(f, allocas, &vv)?;
                let slot = if vt == "str" {
                    let c = self.call_rt("anle_str_copy", &[self.pty()], Some(self.pty()), &[vraw])?;
                    self.to_slot(c)?
                } else {
                    self.to_slot(vraw)?
                };
                self.call_rt(
                    "anle_map_set",
                    &[self.pty(), self.ity(), self.ity()],
                    None,
                    &[mraw, kraw, slot.into()],
                )?;
            }
            Tag::Drop => {
                let obj = inst.a;
                let lids: Vec<usize> = f
                    .locals
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.obj == Some(obj))
                    .map(|(i, _)| i)
                    .collect();
                for lid in lids {
                    self.释放局部(f, allocas, lid)?;
                }
            }
            //nt: 释放局部当前值
            Tag::DropVar => {
                self.释放局部(f, allocas, inst.a as usize)?;
            }
            Tag::Assert => {
                if self.dbg.is_some() {
                    let c = 迹.取(inst.a)?;
                    let i1 = self.to_bool(c)?;
                    let mid = ffn.extra.get(inst.b as usize).copied().unwrap_or(0);
                    let fid = ffn.extra.get(inst.b as usize + 1).copied().unwrap_or(0);
                    let msg = ffn.strings.get(mid as usize).cloned().unwrap_or_default();
                    let fix = ffn.strings.get(fid as usize).cloned().unwrap_or_default();
                    let fnv = self
                        .builder
                        .get_insert_block()
                        .and_then(|b| b.get_parent())
                        .ok_or_else(|| "assert 不在函数里".to_string())?;
                    let ok = self.ctx.append_basic_block(fnv, "assert.ok");
                    let bad = self.ctx.append_basic_block(fnv, "assert.bad");
                    self.builder.build_conditional_branch(i1, ok, bad).map_err(|e| e.to_string())?;
                    self.builder.position_at_end(bad);
                    let code = if msg.contains("空引用") { 1 } else { 7 };
                    let m = self.fmt(&msg);
                    let fx = self.fmt(&fix);
                    self.call_rt(
                        "anle_die",
                        &[self.ctx.i32_type().into(), self.ity(), self.pty(), self.pty()],
                        None,
                        &[
                            self.ctx.i32_type().const_int(code, false).into(),
                            self.ci(self.cur_line as i64),
                            m.into(),
                            fx.into(),
                        ],
                    )?;
                    self.builder.build_unreachable().map_err(|e| e.to_string())?;
                    self.builder.position_at_end(ok);
                }
            }
            //nt: 对象三件套本步空臂
            Tag::Alloc | Tag::Bind | Tag::Kill => {}
            Tag::AccessBegin | Tag::AccessEnd => {}
            //nt: 转换器只出语句式写
            Tag::ListSet | Tag::MapSet => return Err("扁平载体不出现表达式式容器写".into()),
            Tag::Asm => {
                let blob = ffn
                    .strings
                    .get(inst.a as usize)
                    .cloned()
                    .unwrap_or_default();
                let form = anle_ir::flat::解汇编(&blob).ok_or_else(|| "汇编形串坏".to_string())?;
                self.asm_stmt(f, allocas, &form)?;
            }
        }
        Ok(())
    }

    //nt: 指令转值迹前置
    fn flat值2(&self, 迹: &迹<'ctx>, ffn: &FlatFn, id: u32) -> Option<Value> {
        self.flat值(ffn, id, 迹)
    }

    //nt: 指令转值
    fn flat值(&self, ffn: &FlatFn, id: u32, 迹: &迹<'ctx>) -> Option<Value> {
        let inst = ffn.insts.get(id as usize).copied()?;
        //nt: 定义指令按局部取型
        if let Some(lid) = 迹.反查(id) {
            return Some(Value::Local(lid));
        }
        match inst.tag {
            Tag::ConstInt => Some(Value::Int(ffn.ints.get(inst.a as usize).copied().unwrap_or(0))),
            Tag::ConstFloat => Some(Value::Float(ffn.floats.get(inst.a as usize).copied().unwrap_or(0.0))),
            Tag::ConstStr => Some(Value::Str(ffn.strings.get(inst.a as usize).cloned().unwrap_or_default())),
            Tag::ConstBool => Some(Value::Bool(inst.a != 0)),
            Tag::ConstNull => Some(Value::Null),
            Tag::ReadLocal => Some(Value::Local(inst.b as usize)),
            //nt: 定义指令反查所属局部
            _ => 迹.反查(id).map(Value::Local),
        }
    }

    //nt: 容器赋值移交
    fn 容器移交flat(
        &mut self,
        f: &Function,
        allocas: &[Option<inkwell::values::PointerValue<'ctx>>],
        lhs: &Place,
        ffn: &FlatFn,
        iid: usize,
        迹: &mut 迹<'ctx>,
    ) -> Result<(), String> {
        let ty = self.local_ty(f, lhs.base).0;
        if !anle_ir::is_container_ty(&ty) {
            return Ok(());
        }
        //nt: 收口b记移交源
        let nb = ffn.insts[iid].b;
        if nb == 0 {
            return Ok(());
        }
        let u = (nb - 1) as usize;
        let 匿名 = f.locals.get(u).map(|l| l.name.is_none()).unwrap_or(false);
        if !匿名 {
            return Ok(());
        }
        let pt = self.ctx.ptr_type(AddressSpace::default());
        if let Some(a) = allocas.get(u).and_then(|x| *x) {
            self.builder.build_store(a, pt.const_null()).map_err(|e| e.to_string())?;
        } else {
            let mut 槽 = self.值槽.borrow_mut();
            if let Some(x) = 槽.get_mut(u) {
                *x = Some(pt.const_null().into());
            }
        }
        if let Some(dv) = 迹.定义(u) {
            迹.存(dv, pt.const_null().into());
        }
        Ok(())
    }

    //nt: 列表字面量
    fn list_lit(
        &mut self,
        f: &Function,
        allocas: &[Option<inkwell::values::PointerValue<'ctx>>],
        elems: &[Value],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let wty = self.want.clone().map(|t| t.0).unwrap_or_default();
        let et = seq_elem_ty(&wty).unwrap_or_else(|| "int".into());
        let (esz, sgn) = elem_esz_sgn(&et);
        let 宽 = self.聚合宽(&et);
        if elems.is_empty() {
            return self.call_rt(
                "anle_list_new",
                &[self.ity(), self.ity()],
                Some(self.pty()),
                &[self.ci(宽.unwrap_or(esz)), self.ci(sgn)],
            );
        }
        if let Some(宽) = 宽 {
            //nt: 聚合字面量
            let pt = self.ctx.ptr_type(AddressSpace::default());
            let at = pt.array_type(elems.len() as u32);
            let a = self.alloca(at, "aarr").map_err(|e| e.to_string())?;
            for (i, e) in elems.iter().enumerate() {
                let v = self.value(f, allocas, e)?;
                let z = self.ctx.i64_type().const_int(0, false);
                let ix = self.ctx.i64_type().const_int(i as u64, false);
                let ep = unsafe { self.builder.build_gep(at, a, &[z, ix], "ae") }
                    .map_err(|e| e.to_string())?;
                self.builder.build_store(ep, v).map_err(|e| e.to_string())?;
            }
            return self.call_rt(
                "anle_list_lit_from",
                &[self.ity(), self.pty(), self.ity()],
                Some(self.pty()),
                &[
                    self.ci(elems.len() as i64),
                    a.into(),
                    self.ctx.i64_type().const_int(宽 as u64, false).into(),
                ],
            );
        }
        if et == "str" {
            let mut ptrs = Vec::new();
            for e in elems {
                match self.value(f, allocas, e)? {
                    BasicValueEnum::PointerValue(pp) => ptrs.push(pp),
                    _ => return Err("str 列表元素不是指针".into()),
                }
            }
            let arr = self.stack_ptr_arr(&ptrs)?;
            return self.call_rt(
                "anle_list_lit_s",
                &[self.ity(), self.pty()],
                Some(self.pty()),
                &[self.ci(elems.len() as i64), arr.into()],
            );
        }
        let mut vals = Vec::new();
        for e in elems {
            let v = self.value(f, allocas, e)?;
            vals.push(self.to_slot(v)?);
        }
        let arr = self.stack_i64_arr(&vals)?;
        self.call_rt(
            "anle_list_lit",
            &[self.ity(), self.pty(), self.ity(), self.ity()],
            Some(self.pty()),
            &[self.ci(elems.len() as i64), arr.into(), self.ci(esz), self.ci(sgn)],
        )
    }

    //nt: 列表取元素
    fn list_get(
        &mut self,
        f: &Function,
        allocas: &[Option<inkwell::values::PointerValue<'ctx>>],
        list: &Value,
        index: &Value,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let lt = self.arg_ty(f, list);
        let et = seq_elem_ty(&lt).unwrap_or_else(|| "int".into());
        let lv = self.value(f, allocas, list)?;
        let iv = self.value(f, allocas, index)?;
        if let Some(st) = self.structs.get(&et).copied() {
            //nt: 聚合整块拷
            let a = self.alloca(st, "ae").map_err(|e| e.to_string())?;
            self.call_rt(
                "anle_list_copy_at",
                &[self.pty(), self.ity(), self.pty()],
                None,
                &[lv, iv, a.into()],
            )?;
            return self.builder.build_load(st, a, "av").map_err(|e| e.to_string());
        }
        //nt: 调试态走检查函数
        if self.dbg.is_some() {
            let c = self.call_rt(
                "anle_list_at",
                &[self.pty(), self.ity()],
                Some(self.ity()),
                &[lv.into(), iv.into()],
            )?;
            return self.from_slot(c, &et);
        }
        //nt: 八字节元素直读
        let 位 = self
            .builder
            .build_int_sub(iv.into_int_value(), self.ity().into_int_type().const_int(1, false), "idx")
            .map_err(|e| e.to_string())?;
        let 数据 = match self.列表字段(lv.into_pointer_value(), 4)? {
            BasicValueEnum::PointerValue(pp) => pp,
            _ => return Err("列表数据指针类型不对".into()),
        };
        let 八 = self.ity().into_int_type().const_int(8, false);
        let 偏 = self.builder.build_int_mul(位, 八, "off").map_err(|e| e.to_string())?;
        let 偏64 = self
            .builder
            .build_int_s_extend(偏, self.ctx.i64_type(), "sext")
            .map_err(|e| e.to_string())?;
        let 址 = unsafe { self.builder.build_gep(self.ctx.i8_type(), 数据, &[偏64], "at") }
            .map_err(|e| e.to_string())?;
        let raw = self.builder.build_load(self.ity(), 址, "cell").map_err(|e| e.to_string())?;
        self.from_slot(raw, &et)
    }

    //nt: 映射字面量
    fn map_lit(
        &mut self,
        f: &Function,
        allocas: &[Option<inkwell::values::PointerValue<'ctx>>],
        pairs: &[(MapKey, Value)],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let wty = self.want.clone().map(|t| t.0).unwrap_or_default();
        let vt = map_value_ty(&wty).unwrap_or_else(|| "int".into());
        //nt: 键型按声明
        let kty = if wty
            .strip_prefix("map(")
            .map(|r| r.split(',').next().unwrap_or("").trim() == "int")
            .unwrap_or(false)
        {
            1
        } else {
            0
        };
        let (esz, sgn) = elem_esz_sgn(&vt);
        let mut ks = Vec::new();
        let mut vs = Vec::new();
        for (k, v) in pairs {
            let g: inkwell::values::IntValue<'ctx> = match k {
                MapKey::Int(n) => self.ctx.i64_type().const_int(*n as u64, false),
                MapKey::Str(s) => {
                    let g = self.builder.build_global_string_ptr(&s, "mk").map_err(|e| e.to_string())?;
                    self.builder
                        .build_ptr_to_int(g.as_pointer_value(), self.ctx.i64_type(), "k2i")
                        .map_err(|e| e.to_string())?
                }
            };
            ks.push(g);
            let vv = self.value(f, allocas, v)?;
            let vv = if vt == "str" {
                self.call_rt("anle_str_copy", &[self.pty()], Some(self.pty()), &[vv])?
            } else {
                vv
            };
            vs.push(self.to_slot(vv)?);
        }
        let karr = self.stack_i64_arr(&ks)?;
        let varr = self.stack_i64_arr(&vs)?;
        self.call_rt(
            "anle_map_lit",
            &[self.ity(), self.pty(), self.pty(), self.ity(), self.ity(), self.ity()],
            Some(self.pty()),
            &[
                self.ci(pairs.len() as i64),
                karr.into(),
                varr.into(),
                self.ci(esz),
                self.ci(sgn),
                self.ci(kty),
            ],
        )
    }

    //nt: 变体字面量
    fn variant_lit(
        &mut self,
        f: &Function,
        allocas: &[Option<inkwell::values::PointerValue<'ctx>>],
        ty: &str,
        tag: usize,
        args: &[Value],
        tys: &[Ty],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let st = *self.structs.get(ty).ok_or_else(|| format!("缺和类型 {}", ty))?;
        let slot = self.alloca(st, "var").map_err(|e| e.to_string())?;
        let tag_p = self
            .builder
            .build_struct_gep(st, slot, 0, "tagp")
            .map_err(|e| e.to_string())?;
        self.builder
            .build_store(tag_p, self.ctx.i64_type().const_int(tag as u64, false))
            .map_err(|e| e.to_string())?;
        for (i, a) in args.iter().enumerate() {
            let v = self.value(f, allocas, a)?;
            let want = tys.get(i).map(|t| t.0.clone()).unwrap_or_default();
            let iv = if want == "str" {
                self.to_slot(v)?
            } else if want.ends_with('*') {
                match v {
                    BasicValueEnum::PointerValue(pp) => self
                        .builder
                        .build_ptr_to_int(pp, self.ctx.i64_type(), "ph")
                        .map_err(|e| e.to_string())?,
                    _ => match a {
                        Value::Local(id) => {
                            let ap = allocas
                                .get(*id)
                                .and_then(|x| *x)
                                .ok_or_else(|| "变体载荷没有栈位".to_string())?;
                            self.builder
                                .build_ptr_to_int(ap, self.ctx.i64_type(), "ph")
                                .map_err(|e| e.to_string())?
                        }
                        _ => return Err("把手载荷只能来自局部".into()),
                    },
                }
            } else {
                self.to_slot(v)?
            };
            let fp = self
                .builder
                .build_struct_gep(st, slot, (i + 1) as u32, "pay")
                .map_err(|e| e.to_string())?;
            self.builder.build_store(fp, iv).map_err(|e| e.to_string())?;
        }
        self.builder.build_load(st, slot, "vlit").map_err(|e| e.to_string())
    }
}

//nt: 解析函数签名
fn fn_sig(s: &str) -> Option<(Vec<Ty>, Option<Ty>)> {
    let rest = s.strip_prefix("fn(")?;
    let mut depth = 1i32;
    let mut end = None;
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    let inner = &rest[..end];
    let ret = rest[end + 1..].trim();
    let mut ps = Vec::new();
    let mut cur = String::new();
    let mut d = 0i32;
    for c in inner.chars() {
        match c {
            '(' => {
                d += 1;
                cur.push(c);
            }
            ')' => {
                d -= 1;
                cur.push(c);
            }
            ',' if d == 0 => {
                ps.push(Ty(cur.trim().to_string()));
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        ps.push(Ty(cur.trim().to_string()));
    }
    let ret = if ret.is_empty() { None } else { Some(Ty(ret.to_string())) };
    Some((ps, ret))
}
