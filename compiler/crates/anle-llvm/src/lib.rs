//! anle-llvm —— 后端之一：中间表示 → LLVM IR → 目标文件
//!
//! 另一条 lower 在 `anle-codegen`。
//!
//! 覆盖：与 C 后端同一套 IR 输入；产出 `.ll` 与 `.o`，由外部 clang 链接。
//! 运行期依赖 LLVM-C 动态库。

use anle_ir::{AsmForm, BinOp, Exit, Function, LocalId, Place, Proj, Program, Rvalue, Stmt, Ty, Value};
use inkwell::AddressSpace;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::debug_info::{
    AsDIScope, DICompileUnit, DIFile, DIFlags, DIFlagsConstants, DISubprogram, DIType, DWARFEmissionKind,
    DWARFSourceLanguage, DebugInfoBuilder,
};
use inkwell::module::{FlagBehavior, Module};
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetData, TargetMachine};
use inkwell::types::{BasicMetadataTypeEnum, BasicTypeEnum};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FunctionValue, GlobalValue, PointerValue, StructValue};
use inkwell::{IntPredicate, OptimizationLevel};
use std::collections::BTreeMap;
use std::path::Path;

//nt: 槽键索引全局名
//nt: 顺序与 C 后端一致
fn slot_idx_of(p: &Program) -> BTreeMap<String, usize> {
    let mut keys: BTreeMap<String, Ty> = BTreeMap::new();
    for f in &p.functions {
        for l in &f.locals {
            if let Some(n) = &l.name {
                if n.contains('#') {
                    keys.entry(n.clone()).or_insert_with(|| l.ty.clone());
                }
            }
        }
    }
    keys.keys().enumerate().map(|(i, k)| (k.clone(), i)).collect()
}

//nt: 已复印逃逸的局部
thread_local! {
    static ESCAPED: std::cell::RefCell<std::collections::BTreeSet<usize>> =
        std::cell::RefCell::new(std::collections::BTreeSet::new());
}

pub fn emit_object(p: &Program, path: &Path, opt: u32, debug: bool, file: &str) -> Result<(), String> {
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
        .create_target_machine(
            &triple,
            "generic",
            "",
            opt,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| "建不了 LLVM 目标机".to_string())?;
    module.set_triple(&triple);
    module.set_data_layout(&tm.get_target_data().get_data_layout());
    let td = tm.get_target_data();
    let builder = ctx.create_builder();
    let dbg = if debug {
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
    let mut src_lines: BTreeMap<String, Vec<String>> = BTreeMap::new();
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
        funcs: BTreeMap::new(),
        strings: BTreeMap::new(),
        want: None,
        td,
        structs: BTreeMap::new(),
        type_fields: BTreeMap::new(),
        cur_line: 0,
        entry_file: file.to_string(),
        src_lines,
        dbg,
    };
    lo.declare_types(p)?;
    lo.declare(p)?;
    for f in &p.functions {
        lo.lower_fn(f, p)?;
    }
    //nt: 先收尾调试信息
    lo.dbg = None;
    let _ = std::fs::write(path.with_extension("ll"), lo.module.print_to_string().to_string());
    lo.module.verify().map_err(|e| format!("LLVM 校验失败：{}", e.to_string()))?;
    tm.write_to_file(&lo.module, FileType::Object, path)
        .map_err(|e| e.to_string())
}

struct Lower<'ctx> {
    ctx: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    funcs: BTreeMap<String, FunctionValue<'ctx>>,
    strings: BTreeMap<String, PointerValue<'ctx>>,
    ///nt: 赋值目标类型
    want: Option<Ty>,
    td: TargetData,
    ///nt: 类型到结构体
    structs: BTreeMap<String, inkwell::types::StructType<'ctx>>,
    ///nt: 类型字段表
    type_fields: BTreeMap<String, Vec<(String, Ty)>>,
    ///nt: 当前语句行号
    cur_line: usize,
    ///nt: 入口文件
    entry_file: String,
    ///nt: 各文件源码行
    src_lines: BTreeMap<String, Vec<String>>,
    ///nt: 调试信息
    dbg: Option<Dbg<'ctx>>,
}

///nt: 调试信息上下文
struct Dbg<'ctx> {
    dib: DebugInfoBuilder<'ctx>,
    file: DIFile<'ctx>,
    unit: DICompileUnit<'ctx>,
    scope: Option<DISubprogram<'ctx>>,
}

impl<'ctx> Lower<'ctx> {
    ///nt: 先占名再填体
    fn declare_types(&mut self, p: &Program) -> Result<(), String> {
        for (i, st) in p.types.iter().enumerate() {
            let t = self.ctx.opaque_struct_type(&format!("anle_st_{}", i));
            self.structs.insert(st.name.clone(), t);
            self.type_fields.insert(st.name.clone(), st.fields.clone());
        }
        for st in &p.types {
            let t = *self.structs.get(&st.name).ok_or_else(|| format!("缺类型 {}", st.name))?;
            let fields: Vec<BasicTypeEnum<'ctx>> = if !st.variants.is_empty() {
                let maxn = st.variants.iter().map(|v| v.fields.len()).max().unwrap_or(0);
                let mut v: Vec<BasicTypeEnum<'ctx>> = vec![self.ctx.i64_type().into()];
                for _ in 0..maxn {
                    v.push(self.ctx.i64_type().into());
                }
                v
            } else {
                st.fields.iter().map(|(_, ty)| self.field_ty(ty)).collect()
            };
            t.set_body(&fields, false);
        }
        Ok(())
    }

    fn declare(&mut self, p: &Program) -> Result<(), String> {
        for (i, f) in p.functions.iter().enumerate() {
            let name = if f.name == "main" {
                "main".to_string()
            } else {
                format!("f{}", i)
            };
            let ret = if f.name == "main" {
                Some(self.ctx.i32_type().into())
            } else {
                f.ret.as_ref().map(|t| self.ty(t))
            };
            let ps: Vec<BasicMetadataTypeEnum<'ctx>> = f
                .locals
                .iter()
                .filter(|l| l.is_param)
                .map(|l| self.ty(&l.ty).into())
                .collect();
            let fty = self.fn_type(ret, &ps, false)?;
            let fv = self.module.add_function(&name, fty, None);
            self.funcs.insert(f.name.clone(), fv);
        }
        for e in &p.externs {
            if self.funcs.contains_key(&e.name) {
                continue;
            }
            let ret: Option<BasicTypeEnum<'ctx>> = Some(self.ty(&e.ret));
            let ps: Vec<BasicMetadataTypeEnum<'ctx>> =
                e.params.iter().map(|t| self.ty(t).into()).collect();
            let fty = self.fn_type(ret, &ps, false)?;
            let fv = self.module.add_function(&e.name, fty, None);
            self.funcs.insert(e.name.clone(), fv);
        }
        let roots = Self::root_names(p);
        if !self.funcs.contains_key("main") {
            let pty = self.ctx.ptr_type(AddressSpace::default());
            let mps = [
                BasicMetadataTypeEnum::from(self.ctx.i32_type()),
                BasicMetadataTypeEnum::from(pty),
            ];
            let fty = self.ctx.i32_type().fn_type(&mps, false);
            let fv = self.module.add_function("main", fty, None);
            let bb = self.ctx.append_basic_block(fv, "entry");
            self.builder.position_at_end(bb);
            //nt: 转交命令行参数
            let argc = fv.get_nth_param(0).ok_or_else(|| "main 缺 argc".to_string())?;
            let argv = fv.get_nth_param(1).ok_or_else(|| "main 缺 argv".to_string())?;
            let sps = [
                BasicMetadataTypeEnum::from(self.ctx.i32_type()),
                BasicMetadataTypeEnum::from(pty),
            ];
            let sfty = self.ctx.void_type().fn_type(&sps, false);
            let sfv = self.module.add_function("anle_args_set", sfty, None);
            let _ = self
                .builder
                .build_call(sfv, &[argc.into(), argv.into()], "")
                .map_err(|e| e.to_string())?;
            for r in &roots {
                if let Some(rf) = self.funcs.get(r).copied() {
                    //nt: 槽参在调用方备存储
                    //nt: 与 C 后端 main 一致
                    let mut args: Vec<BasicMetadataValueEnum> = Vec::new();
                    if let Some(fdef) = p.functions.iter().find(|f| &f.name == r) {
                        for l in fdef.locals.iter().filter(|l| l.is_param) {
                            let et = l.ty.0.trim_end_matches('*').to_string();
                            let t = self.ty(&Ty(et));
                            let a = self.alloca(t, "slot").map_err(|e| e.to_string())?;
                            args.push(a.into());
                        }
                    }
                    let _ = self.builder.build_call(rf, &args, "");
                }
            }
            let _ = self.builder.build_return(Some(&self.ctx.i32_type().const_int(0, false)));
        }
        Ok(())
    }

    ///nt: 链根块表
    fn root_names(p: &Program) -> Vec<String> {
        let mut called = std::collections::BTreeSet::new();
        for f in &p.functions {
            for b in &f.blocks {
                for s in &b.stmts {
                    match s {
                        Stmt::Call { name, .. } => {
                            called.insert(name.clone());
                        }
                        Stmt::Assign { rhs: Rvalue::Call { name, .. }, .. } => {
                            called.insert(name.clone());
                        }
                        _ => {}
                    }
                }
            }
        }
        p.functions
            .iter()
            .filter(|f| f.is_block && !f.is_sle && !called.contains(&f.name))
            .map(|f| f.name.clone())
            .collect()
    }

    fn fn_type(
        &self,
        ret: Option<BasicTypeEnum<'ctx>>,
        ps: &[BasicMetadataTypeEnum<'ctx>],
        varargs: bool,
    ) -> Result<inkwell::types::FunctionType<'ctx>, String> {
        match ret {
            None => Ok(self.ctx.void_type().fn_type(ps, varargs)),
            Some(BasicTypeEnum::IntType(t)) => Ok(t.fn_type(ps, varargs)),
            Some(BasicTypeEnum::FloatType(t)) => Ok(t.fn_type(ps, varargs)),
            Some(BasicTypeEnum::PointerType(t)) => Ok(t.fn_type(ps, varargs)),
            Some(BasicTypeEnum::StructType(t)) => Ok(t.fn_type(ps, varargs)),
            _ => Err("不支持的 LLVM 返回类型".into()),
        }
    }

    fn ty(&self, t: &Ty) -> BasicTypeEnum<'ctx> {
        let n = t.0.as_str();
        if n.starts_with("fn(") {
            return self.ctx.ptr_type(AddressSpace::default()).into();
        }
        if let Some(st) = self.structs.get(n) {
            return (*st).into();
        }
        if let Some(base) = n.strip_suffix('*') {
            if self.structs.contains_key(base) {
                return self.ctx.ptr_type(AddressSpace::default()).into();
            }
        }
        if n == "float" {
            return self.ctx.f64_type().into();
        }
        if n == "str" || n.ends_with('*') || n == "null" {
            return self.ctx.ptr_type(AddressSpace::default()).into();
        }
        if n.starts_with("list") || n.starts_with("que") || n.starts_with("stack") || n.starts_with("map") {
            return self.ctx.ptr_type(AddressSpace::default()).into();
        }
        if n.starts_with("int_") || n.starts_with("uint_") {
            return self.ctx.i64_type().into();
        }
        if n == "bool" || n == "int" {
            return self.ctx.i64_type().into();
        }
        self.ctx.i64_type().into()
    }

    ///nt: 字段按宽度
    fn field_ty(&self, t: &Ty) -> BasicTypeEnum<'ctx> {
        match t.0.as_str() {
            "int_8" | "uint_8" => self.ctx.i8_type().into(),
            "int_16" | "uint_16" => self.ctx.i16_type().into(),
            "int_32" | "uint_32" => self.ctx.i32_type().into(),
            _ => self.ty(t),
        }
    }

    ///nt: 位置有效类型
    fn place_ty(&self, f: &Function, p: &Place) -> Ty {
        let mut t = self.local_ty(f, p.base);
        for pr in &p.proj {
            match pr {
                Proj::Field(n) => {
                    let owner = t.0.trim_end_matches('*').to_string();
                    t = self
                        .type_fields
                        .get(&owner)
                        .and_then(|fs| fs.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone()))
                        .unwrap_or_else(|| Ty("int".into()));
                }
                Proj::Deref => {
                    t = Ty(t.0.trim_end_matches('*').to_string());
                }
                Proj::Index(_) => {
                    if let Some(e) = Self::seq_elem_ty(&t.0) {
                        t = Ty(e);
                    }
                }
            }
        }
        t
    }

    ///nt: 类型转调试类型
    fn di_ty(&self, ty: &Ty) -> Option<DIType<'ctx>> {
        let d = self.dbg.as_ref()?;
        let 名 = ty.0.trim();
        // nt: 指针递归
        if let Some(基) = 名.strip_suffix('*') {
            let 内 = self.di_ty(&Ty(基.trim().to_string()))?;
            return Some(
                d.dib.create_pointer_type(&ty.0, 内, 64, 64, AddressSpace::default()).as_type(),
            );
        }
        // nt: 位宽与编码
        let (位, 码) = match 名 {
            "bool" => (8u64, 0x02u32),
            "float" => (64, 0x04),
            "str" => (64, 0x01),
            "int" | "int_64" => (64, 0x05),
            "int_32" => (32, 0x05),
            "int_16" => (16, 0x05),
            "int_8" => (8, 0x05),
            "uint_64" => (64, 0x07),
            "uint_32" => (32, 0x07),
            "uint_16" => (16, 0x07),
            "uint_8" => (8, 0x07),
            // nt: 其余不透明
            _ => (64, 0x07),
        };
        d.dib.create_basic_type(名, 位, 码, DIFlags::PUBLIC).ok().map(|t| t.as_type())
    }

    ///nt: 发变量调试信息
    fn 发变量调试(&self, f: &Function, allocas: &[Option<PointerValue<'ctx>>]) -> Result<(), String> {
        let Some(d) = self.dbg.as_ref() else { return Ok(()) };
        let Some(scope) = d.scope else { return Ok(()) };
        let Some(块) = self.builder.get_insert_block() else { return Ok(()) };
        let mut 形参序 = 0u32;
        for (i, l) in f.locals.iter().enumerate() {
            let 参 = l.is_param;
            if 参 {
                形参序 += 1;
            }
            let Some(Some(a)) = allocas.get(i) else { continue };
            let Some(名) = l.name.as_deref().filter(|n| !n.is_empty()) else { continue };
            let Some(dt) = self.di_ty(&l.ty) else { continue };
            let 行 = (l.line.max(1)) as u32;
            let 列 = l.col as u32;
            let 位 = d.dib.create_debug_location(
                self.ctx,
                行,
                列,
                scope.as_debug_info_scope(),
                None,
            );
            let 变 = if 参 {
                d.dib.create_parameter_variable(
                    scope.as_debug_info_scope(),
                    名,
                    形参序,
                    d.file,
                    行,
                    dt,
                    true,
                    DIFlags::PUBLIC,
                )
            } else {
                d.dib.create_auto_variable(
                    scope.as_debug_info_scope(),
                    名,
                    d.file,
                    行,
                    dt,
                    true,
                    DIFlags::PUBLIC,
                    0,
                )
            };
            d.dib.insert_declare_at_end(*a, Some(变), None, 位, 块);
        }
        Ok(())
    }

    fn lower_fn(&mut self, f: &Function, p: &Program) -> Result<(), String> {
        //nt: 逃逸集按函数清
        ESCAPED.with(|s| s.borrow_mut().clear());
        //nt: 出口收段
        let free_own = f.kills_own || Self::root_names(p).iter().any(|n| n == &f.name);
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
        //nt: 清调试位置
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
        let mut allocas: Vec<Option<PointerValue<'ctx>>> = vec![None; f.locals.len()];
        //nt: 槽键索引与后端一致
        let slot_idx = slot_idx_of(p);
        for (i, l) in f.locals.iter().enumerate() {
            if l.is_param {
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
            let t = self.ty(&l.ty);
            let a = self.alloca(t, &format!("v{}", i)).map_err(|e| e.to_string())?;
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
        let (parent, region) = self.region_enter(&f.name)?;
        //nt: 圈头取标记
        let mut heads: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        for b in &f.blocks {
            if let Exit::Branch { targets, .. } = &b.exit {
                for t in targets {
                    if *t < b.id {
                        heads.insert(*t);
                    }
                }
            }
        }
        let mut marks: BTreeMap<usize, PointerValue<'ctx>> = BTreeMap::new();
        for h in &heads {
            let a = self.alloca(self.pty(), &format!("m{}", h)).map_err(|e| e.to_string())?;
            marks.insert(*h, a);
        }
        let mut bbs = Vec::new();
        for b in &f.blocks {
            bbs.push(self.ctx.append_basic_block(fv, &format!("bb{}", b.id)));
        }
        if let Some(first) = bbs.first() {
            self.builder.build_unconditional_branch(*first).map_err(|e| e.to_string())?;
        }
        for (bi, b) in f.blocks.iter().enumerate() {
            self.builder.position_at_end(bbs[bi]);
            if let Some(m) = marks.get(&b.id).copied() {
                let mv = self.call_rt("anle_region_scope_mark", &[], Some(self.pty()), &[])?;
                self.builder.build_store(m, mv).map_err(|e| e.to_string())?;
            }
            for (si, s) in b.stmts.iter().enumerate() {
                let sp = b.spans.get(si).copied().unwrap_or_default();
                let ln = sp.line;
                self.cur_line = ln;
                if let Some(d) = self.dbg.as_ref() {
                    if let Some(sco) = d.scope {
                        if ln > 0 {
                            let loc = d.dib.create_debug_location(
                                self.ctx,
                                ln as u32,
                                sp.col as u32,
                                sco.as_debug_info_scope(),
                                None,
                            );
                            self.builder.set_current_debug_location(loc);
                        }
                    }
                }
                if ln > 0 {
                    let g = self.line_global();
                    self.builder
                        .build_store(g.as_pointer_value(), self.ctx.i64_type().const_int(ln as u64, false))
                        .map_err(|e| e.to_string())?;
                    let gc = self.i64_global("anle_col");
                    self.builder
                        .build_store(gc.as_pointer_value(), self.ctx.i64_type().const_int(sp.col as u64, false))
                        .map_err(|e| e.to_string())?;
                    let gl = self.i64_global("anle_len");
                    self.builder
                        .build_store(gl.as_pointer_value(), self.ctx.i64_type().const_int(sp.len as u64, false))
                        .map_err(|e| e.to_string())?;
                    //nt: 当前源码行
                    let key = if f.file.is_empty() { self.entry_file.clone() } else { f.file.clone() };
                    let 行文本 = self
                        .src_lines
                        .get(&key)
                        .and_then(|ls| ls.get(sp.line - 1))
                        .cloned()
                        .unwrap_or_default();
                    let sv = if 行文本.is_empty() {
                        self.ctx.ptr_type(AddressSpace::default()).const_null()
                    } else {
                        self.fmt(&行文本)
                    };
                    let gs = self.ptr_global("anle_src");
                    self.builder.build_store(gs.as_pointer_value(), sv).map_err(|e| e.to_string())?;
                }
                self.stmt(f, &allocas, s)?;
            }
            match &b.exit {
                Exit::Return(v) => {
                    if f.name == "main" {
                        self.builder.build_return(Some(&self.ctx.i32_type().const_int(0, false))).map_err(|e| e.to_string())?;
                    } else if let Some(v) = v {
                        let mut x = self.value(f, &allocas, v)?;
                        //nt: fn自有对象复印重绑
                        //nt: 复印目标 = 调用方地皮
                        if let Value::Local(lid) = v {
                            if f.locals.get(*lid).map(|l| l.fn_owned).unwrap_or(false) {
                                if let BasicValueEnum::PointerValue(pv) = x {
                                    let base = f.locals[*lid].ty.0.trim_end_matches('*').to_string();
                                    if let Some(st) = self.structs.get(&base).cloned() {
                                        let 已逃 = ESCAPED.with(|s| s.borrow().contains(lid));
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
                                            }
                                            ESCAPED.with(|s| s.borrow_mut().insert(*lid));
                                            x = c;
                                        }
                                    }
                                }
                            }
                        }
                        self.region_exit(parent, region, free_own)?;
                        self.builder.build_return(Some(&x)).map_err(|e| e.to_string())?;
                    } else {
                        self.region_exit(parent, region, free_own)?;
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
                    let pt = self.ctx.ptr_type(AddressSpace::default());
                    //nt: 回边先还池
                    let release = |this: &mut Self, t: usize| -> Result<(), String> {
                        if let Some(m) = marks.get(&t).copied() {
                            if t < b.id && !Self::loop_call_assign(f, t, b.id) {
                                let mv = this.builder.build_load(pt, m, "mk").map_err(|e| e.to_string())?;
                                this.call_rt("anle_region_scope_release", &[pt.into()], None, &[mv])?;
                            }
                        }
                        Ok(())
                    };
                    if targets.len() == 1 {
                        release(self, targets[0])?;
                        self.builder.build_unconditional_branch(bbs[targets[0]]).map_err(|e| e.to_string())?;
                    } else {
                        let fnv = self.builder.get_insert_block().and_then(|x| x.get_parent()).ok_or_else(|| "分支不在函数里".to_string())?;
                        let mut dest = [bbs[targets[0]], bbs[targets[1]]];
                        for k in 0..2 {
                            let t = targets[k];
                            if marks.contains_key(&t) && t < b.id && !Self::loop_call_assign(f, t, b.id) {
                                let mid = self.ctx.append_basic_block(fnv, "loop.release");
                                self.builder.position_at_end(mid);
                                release(self, t)?;
                                self.builder.build_unconditional_branch(bbs[t]).map_err(|e| e.to_string())?;
                                dest[k] = mid;
                            }
                        }
                        //nt: br发当前位置
                        self.builder
                            .build_conditional_branch(i1, dest[0], dest[1])
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
        }
        Ok(())
    }

    fn to_bool(&self, v: BasicValueEnum<'ctx>) -> Result<inkwell::values::IntValue<'ctx>, String> {
        match v {
            BasicValueEnum::IntValue(i) => {
                let zero = self.ctx.i64_type().const_int(0, false);
                self.builder
                    .build_int_compare(IntPredicate::NE, i, zero, "cond")
                    .map_err(|e| e.to_string())
            }
            _ => Err("条件不是布尔值".into()),
        }
    }

    fn local_ty(&self, f: &Function, id: LocalId) -> Ty {
        f.locals.get(id).map(|l| l.ty.clone()).unwrap_or(Ty("int".into()))
    }

    //nt: 小工具

    fn is_container_ty(t: &str) -> bool {
        t == "list" || t == "map"
            || t.starts_with("list(") || t.starts_with("que") || t.starts_with("stack")
            || t.starts_with("map(")
    }

    ///nt: 取序列元素类型
    fn seq_elem_ty(t: &str) -> Option<String> {
        let (head, inner) = t.split_once('(')?;
        let tail = inner.strip_suffix(')')?;
        match head {
            "list" | "que" | "stack" => Some(tail.trim().to_string()),
            _ => None,
        }
    }

    ///nt: 取 map 值类型
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

    ///nt: 槽宽与符号
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

    ///nt: 指针进槽
    fn boxed_ty(t: &str) -> bool {
        t == "str" || t.ends_with('*') || Self::is_container_ty(t)
    }

    fn arg_ty(&self, f: &Function, v: &Value) -> String {
        match v {
            Value::Local(id) => self.local_ty(f, *id).0,
            Value::Str(_) => "str".into(),
            Value::Float(_) => "float".into(),
            Value::Bool(_) => "bool".into(),
            Value::Null => "null".into(),
            _ => "int".into(),
        }
    }

    fn ci(&self, v: i64) -> BasicValueEnum<'ctx> {
        self.ctx.i64_type().const_int(v as u64, false).into()
    }

    fn pty(&self) -> BasicTypeEnum<'ctx> {
        self.ctx.ptr_type(AddressSpace::default()).into()
    }

    fn ity(&self) -> BasicTypeEnum<'ctx> {
        self.ctx.i64_type().into()
    }

    ///nt: 调运行时函数
    fn call_rt(
        &mut self,
        name: &str,
        ps: &[BasicTypeEnum<'ctx>],
        ret: Option<BasicTypeEnum<'ctx>>,
        args: &[BasicValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let fv = if let Some(f) = self.module.get_function(name) {
            f
        } else {
            let mps: Vec<BasicMetadataTypeEnum<'ctx>> = ps.iter().map(|t| (*t).into()).collect();
            let fty = match ret {
                None => self.ctx.void_type().fn_type(&mps, false),
                Some(BasicTypeEnum::IntType(t)) => t.fn_type(&mps, false),
                Some(BasicTypeEnum::FloatType(t)) => t.fn_type(&mps, false),
                Some(BasicTypeEnum::PointerType(t)) => t.fn_type(&mps, false),
                Some(BasicTypeEnum::StructType(t)) => t.fn_type(&mps, false),
                Some(_) => return Err(format!("运行时 {} 返回类型不支持", name)),
            };
            self.module.add_function(name, fty, None)
        };
        let av: Vec<BasicMetadataValueEnum<'ctx>> = args.iter().map(|a| (*a).into()).collect();
        let c = self.builder.build_call(fv, &av, name).map_err(|e| e.to_string())?;
        Ok(c.try_as_basic_value().basic().unwrap_or_else(|| self.ci(0)))
    }

    //nt: 键统一转i64
    fn key_ll(&self, v: BasicValueEnum<'ctx>) -> Result<BasicValueEnum<'ctx>, String> {
        match v {
            BasicValueEnum::PointerValue(p) => {
                let i = self
                    .builder
                    .build_ptr_to_int(p, self.ctx.i64_type(), "k2i")
                    .map_err(|e| e.to_string())?;
                Ok(i.into())
            }
            other => Ok(other),
        }
    }

    fn stack_i64_arr(&mut self, vals: &[inkwell::values::IntValue<'ctx>]) -> Result<PointerValue<'ctx>, String> {
        let at = self.ctx.i64_type().array_type(vals.len() as u32);
        let a = self.alloca(at, "arr").map_err(|e| e.to_string())?;
        let z = self.ctx.i64_type().const_int(0, false);
        for (i, v) in vals.iter().enumerate() {
            let idx = self.ctx.i64_type().const_int(i as u64, false);
            //nt: GEP 用数组类型
            let p = unsafe { self.builder.build_gep(at, a, &[z, idx], "ae") }.map_err(|e| e.to_string())?;
            self.builder.build_store(p, *v).map_err(|e| e.to_string())?;
        }
        Ok(a)
    }

    fn stack_ptr_arr(&mut self, vals: &[PointerValue<'ctx>]) -> Result<PointerValue<'ctx>, String> {
        let pt = self.ctx.ptr_type(AddressSpace::default());
        let at = pt.array_type(vals.len() as u32);
        let a = self.alloca(at, "parr").map_err(|e| e.to_string())?;
        let z = self.ctx.i64_type().const_int(0, false);
        for (i, v) in vals.iter().enumerate() {
            let idx = self.ctx.i64_type().const_int(i as u64, false);
            //nt: GEP 用数组类型
            let p = unsafe { self.builder.build_gep(at, a, &[z, idx], "pe") }.map_err(|e| e.to_string())?;
            self.builder.build_store(p, *v).map_err(|e| e.to_string())?;
        }
        Ok(a)
    }

    ///nt: 聚合字节宽
    fn 聚合宽(&self, t: &str) -> Option<i64> {
        self.structs.get(t).map(|s| {
            s.size_of()
                .and_then(|i| i.get_zero_extended_constant())
                .map(|v| v as i64)
                .unwrap_or_else(|| s.count_fields() as i64 * 8)
        })
    }

    ///nt: alloca入首块
    fn alloca<'a, T: inkwell::types::BasicType<'ctx>>(
        &mut self,
        ty: T,
        name: &str,
    ) -> Result<PointerValue<'ctx>, String> {
        let 原块 = self.builder.get_insert_block();
        let 首块 = 原块
            .and_then(|b| b.get_parent())
            .and_then(|f| f.get_first_basic_block());
        if let (Some(b), Some(首)) = (原块, 首块) {
            if b != 首 {
                if let Some(inst) = 首.get_first_instruction() {
                    self.builder.position_before(&inst);
                    let a = self.builder.build_alloca(ty, name).map_err(|e| e.to_string())?;
                    self.builder.position_at_end(b);
                    return Ok(a);
                }
            }
        }
        self.builder.build_alloca(ty, name).map_err(|e| e.to_string())
    }

    ///nt: 值进槽
    fn to_slot(&self, v: BasicValueEnum<'ctx>) -> Result<inkwell::values::IntValue<'ctx>, String> {
        match v {
            BasicValueEnum::IntValue(i) => Ok(i),
            BasicValueEnum::PointerValue(p) => {
                self.builder.build_ptr_to_int(p, self.ctx.i64_type(), "slot").map_err(|e| e.to_string())
            }
            BasicValueEnum::FloatValue(f) => {
                let b = self.builder.build_bit_cast(f, self.ctx.i64_type(), "fbits").map_err(|e| e.to_string())?;
                Ok(b.into_int_value())
            }
            _ => Err("槽只能装标量/指针".into()),
        }
    }

    ///nt: 槽按类型拆箱
    fn from_slot(&self, v: BasicValueEnum<'ctx>, ty: &str) -> Result<BasicValueEnum<'ctx>, String> {
        let i = match v {
            BasicValueEnum::IntValue(i) => i,
            _ => return Err("槽值不是整数".into()),
        };
        if Self::boxed_ty(ty) {
            return Ok(self
                .builder
                .build_int_to_ptr(i, self.ctx.ptr_type(AddressSpace::default()), "unbox")
                .map_err(|e| e.to_string())?
                .into());
        }
        if ty == "float" {
            let b = self.builder.build_bit_cast(i, self.ctx.f64_type(), "f64").map_err(|e| e.to_string())?;
            return Ok(b.into_float_value().into());
        }
        Ok(i.into())
    }

    fn value(
        &self,
        f: &Function,
        allocas: &[Option<PointerValue<'ctx>>],
        v: &Value,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match v {
            Value::Local(id) => self.load_place(f, allocas, &Place::local(*id)),
            Value::Int(i) => Ok(self.ctx.i64_type().const_int(*i as u64, false).into()),
            Value::Float(x) => Ok(self.ctx.f64_type().const_float(*x).into()),
            Value::Bool(b) => Ok(self.ctx.i64_type().const_int(if *b { 1 } else { 0 }, false).into()),
            Value::Null => Ok(self.ctx.ptr_type(AddressSpace::default()).const_null().into()),
            Value::Str(s) => {
                if let Some(p) = self.strings.get(s) {
                    return Ok((*p).into());
                }
                let g = self.builder.build_global_string_ptr(s, "str").map_err(|e| e.to_string())?;
                Ok(g.as_pointer_value().into())
            }
        }
    }

    ///nt: 位置取指针
    fn place_ptr(
        &self,
        allocas: &[Option<PointerValue<'ctx>>],
        p: &Place,
        f: &Function,
    ) -> Result<PointerValue<'ctx>, String> {
        let mut a = allocas
            .get(p.base)
            .and_then(|x| *x)
            .ok_or_else(|| "位置没有栈位".to_string())?;
        let mut cur = self.local_ty(f, p.base).0;
        let pt = self.ctx.ptr_type(AddressSpace::default());
        for pr in &p.proj {
            match pr {
                Proj::Field(name) => {
                    if cur.ends_with('*') {
                        a = self
                            .builder
                            .build_load(pt, a, "handle")
                            .map_err(|e| e.to_string())?
                            .into_pointer_value();
                        cur = cur.trim_end_matches('*').to_string();
                    }
                    let owner = cur.clone();
                    let idx = self
                        .type_fields
                        .get(&owner)
                        .and_then(|fs| fs.iter().position(|(k, _)| k == name))
                        .ok_or_else(|| format!("类型 {} 没有字段 {}", owner, name))?;
                    let st = *self.structs.get(&owner).ok_or_else(|| format!("缺类型 {}", owner))?;
                    a = self
                        .builder
                        .build_struct_gep(st, a, idx as u32, "fld")
                        .map_err(|e| e.to_string())?;
                    cur = self.type_fields[&owner][idx].1 .0.clone();
                }
                Proj::Deref => {
                    a = self
                        .builder
                        .build_load(pt, a, "deref")
                        .map_err(|e| e.to_string())?
                        .into_pointer_value();
                    cur = cur.trim_end_matches('*').to_string();
                }
                Proj::Index(_) => return Err("LLVM 后端暂不支持下标投影".into()),
            }
        }
        Ok(a)
    }

    fn load_place(
        &self,
        f: &Function,
        allocas: &[Option<PointerValue<'ctx>>],
        p: &Place,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let a = self.place_ptr(allocas, p, f)?;
        let src_ty = self.place_ty(f, p);
        let lty = self.field_ty(&src_ty);
        let v = self.builder.build_load(lty, a, "load").map_err(|e| e.to_string())?;
        if let BasicValueEnum::IntValue(i) = v {
            let w = i.get_type().get_bit_width();
            if w < 64 {
                let ext = if src_ty.0.starts_with("uint_") {
                    self.builder.build_int_z_extend(i, self.ctx.i64_type(), "zext")
                } else {
                    self.builder.build_int_s_extend(i, self.ctx.i64_type(), "sext")
                }
                .map_err(|e| e.to_string())?;
                return Ok(ext.into());
            }
        }
        Ok(v)
    }

    ///nt: 积类型一层复印
    fn deep_copy_struct(&mut self, name: &str, val: StructValue<'ctx>) -> Result<BasicValueEnum<'ctx>, String> {
        let st = *self.structs.get(name).ok_or_else(|| format!("缺类型 {}", name))?;
        //nt: alloca提到入口
        let 原块 = self.builder.get_insert_block();
        let tmp = {
            let 入口 = 原块
                .and_then(|b| b.get_parent())
                .and_then(|f| f.get_first_basic_block());
            match 入口.and_then(|e| e.get_first_instruction()) {
                Some(首) if Some(首.get_parent().unwrap()) != 原块 => {
                    self.builder.position_before(&首);
                    let a = self.alloca(st, "dc").map_err(|e| e.to_string())?;
                    if let Some(b) = 原块 {
                        self.builder.position_at_end(b);
                    }
                    a
                }
                _ => self.alloca(st, "dc").map_err(|e| e.to_string())?,
            }
        };
        self.builder.build_store(tmp, val).map_err(|e| e.to_string())?;
        let fields = self.type_fields.get(name).cloned().unwrap_or_default();
        for (i, (_, ft)) in fields.iter().enumerate() {
            let base = ft.0.trim_end_matches('*').to_string();
            if ft.0 == "str" {
                let fp = self
                    .builder
                    .build_struct_gep(st, tmp, i as u32, "dcf")
                    .map_err(|e| e.to_string())?;
                let pv = self
                    .builder
                    .build_load(self.ctx.ptr_type(AddressSpace::default()), fp, "s")
                    .map_err(|e| e.to_string())?
                    .into_pointer_value();
                let isnull = self.builder.build_is_null(pv, "null").map_err(|e| e.to_string())?;
                let copied = self.call_rt("anle_str_copy", &[self.pty()], Some(self.pty()), &[pv.into()])?;
                let nullv = self.ctx.ptr_type(AddressSpace::default()).const_null().into();
                let picked = self
                    .builder
                    .build_select(isnull, nullv, copied, "dcs")
                    .map_err(|e| e.to_string())?;
                self.builder.build_store(fp, picked).map_err(|e| e.to_string())?;
            } else if base == "list" || base == "que" || base == "stack" || base == "map" {
                //nt: 容器字段按元素深拷
                let 内 = ft
                    .0
                    .split_once('(')
                    .and_then(|(_, r)| r.strip_suffix(')'))
                    .unwrap_or("")
                    .to_string();
                let 旗: i64 = if base == "map" {
                    let v = 内.rsplit(',').next().unwrap_or("").trim();
                    if v == "str" { 1 } else { 0 }
                } else if 内.trim() == "str" {
                    1
                } else {
                    0
                };
                let 函数 = if base == "map" { "anle_dc_map" } else { "anle_dc_list" };
                let fp = self
                    .builder
                    .build_struct_gep(st, tmp, i as u32, "dcc")
                    .map_err(|e| e.to_string())?;
                let pv = self.builder.build_load(self.pty(), fp, "cv").map_err(|e| e.to_string())?;
                let cp = self.call_rt(函数, &[self.pty(), self.ity()], Some(self.pty()), &[pv, self.ci(旗).into()])?;
                self.builder.build_store(fp, cp).map_err(|e| e.to_string())?;
            } else if !ft.0.ends_with('*') && self.structs.contains_key(&base) && !self.type_fields[&base].is_empty() {
                let nested = *self.structs.get(&base).unwrap();
                let fp = self
                    .builder
                    .build_struct_gep(st, tmp, i as u32, "dcn")
                    .map_err(|e| e.to_string())?;
                let nv = self
                    .builder
                    .build_load(nested, fp, "n")
                    .map_err(|e| e.to_string())?
                    .into_struct_value();
                let cp = self.deep_copy_struct(&base, nv)?;
                self.builder.build_store(fp, cp).map_err(|e| e.to_string())?;
            }
        }
        self.builder.build_load(st, tmp, "dcv").map_err(|e| e.to_string())
    }

    fn store_place(
        &mut self,
        f: &Function,
        allocas: &[Option<PointerValue<'ctx>>],
        p: &Place,
        v: BasicValueEnum<'ctx>,
    ) -> Result<(), String> {
        let a = self.place_ptr(allocas, p, f)?;
        let dst = self.place_ty(f, p);
        //nt: 积类型一层复印
        let mut v = v;
        let base = dst.0.trim_end_matches('*').to_string();
        if let BasicValueEnum::StructValue(sv) = v {
            if self.structs.contains_key(&base) && !self.type_fields.get(&base).map(|x| x.is_empty()).unwrap_or(true) {
                v = self.deep_copy_struct(&base, sv)?;
            }
        }
        //nt: 开段存结构
        if dst.0.ends_with('*') && self.structs.contains_key(&base) {
            if let BasicValueEnum::StructValue(_) = v {
                let st = *self.structs.get(&base).unwrap();
                let size = self.td.get_store_size(&st) as i64;
                let pt = self.ctx.ptr_type(AddressSpace::default());
                let g = self.region_global();
                let cur = self.builder.build_load(pt, g.as_pointer_value(), "cur").map_err(|e| e.to_string())?;
                let mem = self.call_rt(
                    "anle_region_alloc",
                    &[self.pty(), self.ity()],
                    Some(self.pty()),
                    &[cur, self.ci(size)],
                )?;
                let mp = match mem {
                    BasicValueEnum::PointerValue(x) => x,
                    _ => return Err("region_alloc 返回类型不对".into()),
                };
                self.builder.build_store(mp, v).map_err(|e| e.to_string())?;
                self.builder.build_store(a, mem).map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
        let wv = if let BasicValueEnum::IntValue(i) = v {
            let want = match self.field_ty(&dst) {
                BasicTypeEnum::IntType(t) => t.get_bit_width(),
                _ => 64,
            };
            if i.get_type().get_bit_width() > want {
                let narrow: BasicTypeEnum<'ctx> = match want {
                    8 => self.ctx.i8_type().into(),
                    16 => self.ctx.i16_type().into(),
                    32 => self.ctx.i32_type().into(),
                    _ => self.ctx.i64_type().into(),
                };
                let nt = match narrow {
                    BasicTypeEnum::IntType(t) => t,
                    _ => return Err("截断目标不是整数".into()),
                };
                self.builder.build_int_truncate(i, nt, "trunc")
                    .map_err(|e| e.to_string())?
                    .into()
            } else if i.get_type().get_bit_width() < want {
                let e = if dst.0.starts_with("uint_") {
                    self.builder.build_int_z_extend(i, self.ctx.i64_type(), "zext")
                } else {
                    self.builder.build_int_s_extend(i, self.ctx.i64_type(), "sext")
                }
                .map_err(|e| e.to_string())?;
                e.into()
            } else {
                v
            }
        } else {
            v
        };
        self.builder.build_store(a, wv).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn stmt(
        &mut self,
        f: &Function,
        allocas: &[Option<PointerValue<'ctx>>],
        s: &Stmt,
    ) -> Result<(), String> {
        match s {
            Stmt::Assign { lhs, rhs } => {
                self.want = Some(self.local_ty(f, lhs.base));
                let v = self.rvalue(f, allocas, rhs)?;
                self.want = None;
                self.store_place(f, allocas, lhs, v)?;
            }
            Stmt::Call { name, args } => {
                if name == "print" {
                    self.print(f, allocas, args)?;
                } else if let Some(v) = self.builtin(f, allocas, name, args)? {
                    let _ = v;
                } else {
                    let mut vals = Vec::new();
                    for a in args {
                        vals.push(self.value(f, allocas, a)?);
                    }
                    self.call(f, name, &vals)?;
                }
            }
            Stmt::Live(_) | Stmt::Dead(_) | Stmt::AccessBegin(_) | Stmt::AccessEnd(_)
            | Stmt::Copy { .. } | Stmt::Destroy(_) | Stmt::Release(_) => {}
            Stmt::KillRegion { block } => {
                let n = self.fmt(block);
                self.call_rt("anle_region_kill", &[self.pty()], None, &[n.into()])?;
            }
            Stmt::Assert { cond, msg, fix } => {
                let c = self.value(f, allocas, cond)?;
                let i1 = self.to_bool(c)?;
                let fnv = self.builder.get_insert_block().and_then(|b| b.get_parent()).ok_or_else(|| "assert 不在函数里".to_string())?;
                let ok = self.ctx.append_basic_block(fnv, "assert.ok");
                let bad = self.ctx.append_basic_block(fnv, "assert.bad");
                self.builder
                    .build_conditional_branch(i1, ok, bad)
                    .map_err(|e| e.to_string())?;
                self.builder.position_at_end(bad);
                let code = if msg.contains("空引用") { 1 } else { 7 };
                let m = self.fmt(msg);
                let fx = self.fmt(fix);
                self.call_rt(
                    "anle_die",
                    &[self.ctx.i32_type().into(), self.ity(), self.pty(), self.pty()],
                    None,
                    &[self.ctx.i32_type().const_int(code, false).into(), self.ci(self.cur_line as i64), m.into(), fx.into()],
                )?;
                self.builder.build_unreachable().map_err(|e| e.to_string())?;
                self.builder.position_at_end(ok);
            }
            Stmt::ListSet { list, index, value } => {
                let lt = self.arg_ty(f, list);
                let et = Self::seq_elem_ty(&lt).unwrap_or_else(|| "int".into());
                let lv = self.value(f, allocas, list)?;
                let iv = self.value(f, allocas, index)?;
                let vv = self.value(f, allocas, value)?;
                if let Some(st) = self.structs.get(&et).copied() {
                    //nt: 聚合按地址
                    let a = self.alloca(st, "as").map_err(|e| e.to_string())?;
                    self.builder.build_store(a, vv).map_err(|e| e.to_string())?;
                    self.call_rt(
                        "anle_list_setat_from",
                        &[self.pty(), self.ity(), self.pty()],
                        None,
                        &[lv, iv, a.into()],
                    )?
                } else {
                    let slot = if et == "str" {
                        let c = self.call_rt("anle_str_copy", &[self.pty()], Some(self.pty()), &[vv])?;
                        self.to_slot(c)?
                    } else {
                        self.to_slot(vv)?
                    };
                    self.call_rt("anle_list_setat", &[self.pty(), self.ity(), self.ity()], None, &[lv, iv, slot.into()])?
                };
            }
            Stmt::MapSet { map, key, value } => {
                let mt = self.arg_ty(f, map);
                let vt = Self::map_value_ty(&mt).unwrap_or_else(|| "int".into());
                let mv = self.value(f, allocas, map)?;
                let kv = self.value(f, allocas, key)?;
                let kv = self.key_ll(kv)?;
                let vv = self.value(f, allocas, value)?;
                let slot = if vt == "str" {
                    let c = self.call_rt("anle_str_copy", &[self.pty()], Some(self.pty()), &[vv])?;
                    self.to_slot(c)?
                } else {
                    self.to_slot(vv)?
                };
                self.call_rt("anle_map_set", &[self.pty(), self.ity(), self.ity()], None, &[mv, kv, slot.into()])?;
            }
            Stmt::Asm(form) => self.asm_stmt(f, allocas, form)?,
        }
        Ok(())
    }

    ///nt: 内联汇编
    fn asm_stmt(
        &mut self,
        f: &Function,
        allocas: &[Option<PointerValue<'ctx>>],
        form: &AsmForm,
    ) -> Result<(), String> {
        let mut in_vals = Vec::new();
        let mut in_ty = Vec::new();
        for (_, v) in &form.ins {
            let x = self.value(f, allocas, v)?;
            //nt: 实参按宽度
            let want = self.field_ty(&Ty(self.arg_ty(f, v)));
            let x = match (x, want) {
                (BasicValueEnum::IntValue(i), BasicTypeEnum::IntType(t))
                    if i.get_type().get_bit_width() > t.get_bit_width() =>
                {
                    self.builder.build_int_truncate(i, t, "ai").map_err(|e| e.to_string())?.into()
                }
                _ => x,
            };
            in_ty.push(self.basic_ty_of(x));
            in_vals.push(x);
        }
        let mut out_ty: Vec<BasicTypeEnum<'ctx>> = Vec::new();
        for (_, id, _) in &form.outs {
            out_ty.push(self.field_ty(&self.local_ty(f, *id)));
        }
        let ret_ty: Option<BasicTypeEnum<'ctx>> = match out_ty.len() {
            0 => None,
            1 => Some(out_ty[0]),
            _ => Some(self.ctx.struct_type(&out_ty, false).into()),
        };
        let mps: Vec<BasicMetadataTypeEnum<'ctx>> = in_ty.iter().map(|t| (*t).into()).collect();
        let fty = self.fn_type(ret_ty, &mps, false)?;
        let mut cs: Vec<String> = Vec::new();
        for (i, (c, _, rw)) in form.outs.iter().enumerate() {
            cs.push(format!(
                "{}{}",
                if *rw { "+" } else { "=" },
                Self::llvm_constraint(c, &out_ty[i])
            ));
        }
        for (k, (c, _)) in form.ins.iter().enumerate() {
            cs.push(Self::llvm_constraint(c, &in_ty[k]));
        }
        for cl in &form.clobbers {
            cs.push(match cl.as_str() {
                "memory" => "~{memory}".to_string(),
                "cc" => "~{dirflag},~{fpsr},~{flags}".to_string(),
                x => format!("~{{{}}}", x),
            });
        }
        let mut names: BTreeMap<String, usize> = BTreeMap::new();
        for (i, (_, id, _)) in form.outs.iter().enumerate() {
            let n = f.locals.get(*id).and_then(|l| l.name.clone()).unwrap_or_else(|| format!("o{}", id));
            names.insert(n, i);
        }
        for (k, _) in form.ins.iter().enumerate() {
            names.insert(format!("i{}", k), form.outs.len() + k);
        }
        let tmpl = Self::asm_template(&form.tmpl, &names);
        let asm = self.ctx.create_inline_asm(
            fty,
            tmpl,
            cs.join(","),
            true,
            false,
            None,
            false,
        );
        let av: Vec<BasicMetadataValueEnum<'ctx>> = in_vals.iter().map(|v| (*v).into()).collect();
        let c = self
            .builder
            .build_indirect_call(fty, asm, &av, "asm")
            .map_err(|e| e.to_string())?;
        if let Some(r) = c.try_as_basic_value().basic() {
            if form.outs.len() == 1 {
                self.store_place(f, allocas, &Place::local(form.outs[0].1), r)?;
            } else if form.outs.len() > 1 {
                for (i, (_, id, _)) in form.outs.iter().enumerate() {
                    let v = self.builder.build_extract_value(r.into_struct_value(), i as u32, "o")
                        .map_err(|e| e.to_string())?;
                    self.store_place(f, allocas, &Place::local(*id), v)?;
                }
            }
        }
        Ok(())
    }

    fn basic_ty_of(&self, v: BasicValueEnum<'ctx>) -> BasicTypeEnum<'ctx> {
        match v {
            BasicValueEnum::IntValue(i) => i.get_type().into(),
            BasicValueEnum::FloatValue(f) => f.get_type().into(),
            BasicValueEnum::PointerValue(p) => p.get_type().into(),
            BasicValueEnum::StructValue(s) => s.get_type().into(),
            BasicValueEnum::ArrayValue(a) => a.get_type().into(),
            _ => self.ity(),
        }
    }

    ///nt: 约束转寄存器类
    fn llvm_constraint(c: &str, ty: &BasicTypeEnum<'ctx>) -> String {
        let w = match ty {
            BasicTypeEnum::IntType(t) => t.get_bit_width(),
            BasicTypeEnum::PointerType(_) => 64,
            _ => 64,
        };
        let reg = |q8: &str, q16: &str, q32: &str, q64: &str| {
            if w <= 8 {
                q8.to_string()
            } else if w <= 16 {
                q16.to_string()
            } else if w <= 32 {
                q32.to_string()
            } else {
                q64.to_string()
            }
        };
        match c {
            "a" => format!("{{{}}}", reg("al", "ax", "eax", "rax")),
            "b" => format!("{{{}}}", reg("bl", "bx", "ebx", "rbx")),
            "c" => format!("{{{}}}", reg("cl", "cx", "ecx", "rcx")),
            "d" => format!("{{{}}}", reg("dl", "dx", "edx", "rdx")),
            "S" => format!("{{{}}}", reg("sil", "si", "esi", "rsi")),
            "D" => format!("{{{}}}", reg("dil", "di", "edi", "rdi")),
            x if x.starts_with('r') && x[1..].chars().all(|ch| ch.is_ascii_digit()) => {
                format!("{{{}}}", x)
            }
            x => x.to_string(),
        }
    }

    ///nt: 占位符改写
    fn asm_template(tmpl: &str, names: &BTreeMap<String, usize>) -> String {
        let mut out = String::new();
        let bytes = tmpl.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] == b'%' && bytes.get(i + 1) == Some(&b'[') {
                if let Some(rel) = tmpl[i + 2..].find(']') {
                    let name = &tmpl[i + 2..i + 2 + rel];
                    if let Some(idx) = names.get(name) {
                        out.push('$');
                        out.push_str(&idx.to_string());
                    } else {
                        out.push_str(&tmpl[i..i + 3 + rel]);
                    }
                    i += 3 + rel;
                    continue;
                }
            }
            if bytes[i] == b'$' {
                out.push_str("$$");
                i += 1;
                continue;
            }
            out.push(bytes[i] as char);
            i += 1;
        }
        out
    }

    fn rvalue(
        &mut self,
        f: &Function,
        allocas: &[Option<PointerValue<'ctx>>],
        r: &Rvalue,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match r {
            Rvalue::Use(p) => self.load_place(f, allocas, p),
            Rvalue::Const(v) => self.value(f, allocas, v),
            Rvalue::BinOp { op, lhs, rhs } => {
                let a = self.value(f, allocas, lhs)?;
                let b = self.value(f, allocas, rhs)?;
                self.binop(f, *op, lhs, rhs, a, b)
            }
            Rvalue::Call { name, args } => {
                if name == "print" {
                    return Err("print 没有返回值".into());
                }
                if let Some(v) = self.builtin(f, allocas, name, args)? {
                    return Ok(v);
                }
                let mut vals = Vec::new();
                for a in args {
                    vals.push(self.value(f, allocas, a)?);
                }
                self.call(f, name, &vals)
            }
            Rvalue::FnRef { name } => {
                let fv = *self.funcs.get(name).ok_or_else(|| format!("LLVM 后端不认识函数 {}", name))?;
                Ok(fv.as_global_value().as_pointer_value().into())
            }
            Rvalue::CallIndirect { callee, args } => {
                let cty = self.arg_ty(f, callee);
                let pv = match self.value(f, allocas, callee)? {
                    BasicValueEnum::PointerValue(p) => p,
                    BasicValueEnum::IntValue(i) => self
                        .builder
                        .build_int_to_ptr(i, self.ctx.ptr_type(AddressSpace::default()), "fp")
                        .map_err(|e| e.to_string())?,
                    _ => return Err("间接调用的被调者不是函数值".into()),
                };
                let (ps, ret) = Self::fn_sig(&cty).ok_or_else(|| format!("认不出函数类型 {}", cty))?;
                let mps: Vec<BasicMetadataTypeEnum<'ctx>> = ps.iter().map(|t| self.ty(t).into()).collect();
                let fty = self.fn_type(ret.as_ref().map(|t| self.ty(t)), &mps, false)?;
                let mut av: Vec<BasicMetadataValueEnum<'ctx>> = Vec::new();
                for a in args {
                    av.push(self.value(f, allocas, a)?.into());
                }
                let c = self
                    .builder
                    .build_indirect_call(fty, pv, &av, "icall")
                    .map_err(|e| e.to_string())?;
                Ok(c.try_as_basic_value().basic().unwrap_or_else(|| self.ci(0)))
            }
            Rvalue::Alloc { .. } => Ok(self.ctx.i64_type().const_int(0, false).into()),
            Rvalue::VariantLit { ty, tag, args, tys } => {
                let st = *self.structs.get(ty).ok_or_else(|| format!("缺和类型 {}", ty))?;
                let slot = self.alloca(st, "var").map_err(|e| e.to_string())?;
                let tag_p = self
                    .builder
                    .build_struct_gep(st, slot, 0, "tagp")
                    .map_err(|e| e.to_string())?;
                self.builder
                    .build_store(tag_p, self.ctx.i64_type().const_int(*tag as u64, false))
                    .map_err(|e| e.to_string())?;
                for (i, a) in args.iter().enumerate() {
                    let v = self.value(f, allocas, a)?;
                    let want = tys.get(i).map(|t| t.0.clone()).unwrap_or_default();
                    let iv = if want == "str" {
                        self.to_slot(v)?
                    } else if want.ends_with('*') {
                        match v {
                            BasicValueEnum::PointerValue(p) => self.builder.build_ptr_to_int(p, self.ctx.i64_type(), "ph").map_err(|e| e.to_string())?,
                            _ => match a {
                                Value::Local(id) => {
                                    let ap = allocas.get(*id).and_then(|x| *x).ok_or_else(|| "变体载荷没有栈位".to_string())?;
                                    self.builder.build_ptr_to_int(ap, self.ctx.i64_type(), "ph").map_err(|e| e.to_string())?
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
            Rvalue::VariantTag(v) => {
                let sv = self.struct_value(f, allocas, v)?;
                let x = self.builder.build_extract_value(sv, 0, "tag").map_err(|e| e.to_string())?;
                Ok(x)
            }
            Rvalue::VariantGet { value, idx, ty } => {
                let sv = self.struct_value(f, allocas, value)?;
                let raw = self
                    .builder
                    .build_extract_value(sv, (*idx + 1) as u32, "pay")
                    .map_err(|e| e.to_string())?;
                self.from_slot(raw, &ty.0)
            }
            Rvalue::Ref(p) => Ok(self.place_ptr(allocas, p, f)?.into()),
            Rvalue::ListLit { elems } => {
                let wty = self.want.clone().map(|t| t.0).unwrap_or_default();
                let et = Self::seq_elem_ty(&wty).unwrap_or_else(|| "int".into());
                let (esz, sgn) = Self::elem_esz_sgn(&et);
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
                            BasicValueEnum::PointerValue(p) => ptrs.push(p),
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
            Rvalue::ListGet { list, index } => {
                let lt = self.arg_ty(f, list);
                let et = Self::seq_elem_ty(&lt).unwrap_or_else(|| "int".into());
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
                    let v = self.builder.build_load(st, a, "av").map_err(|e| e.to_string())?;
                    return Ok(v);
                }
                let raw = self.call_rt("anle_list_at", &[self.pty(), self.ity()], Some(self.ity()), &[lv, iv])?;
                self.from_slot(raw, &et)
            }
            Rvalue::MapLit { pairs } => {
                let wty = self.want.clone().map(|t| t.0).unwrap_or_default();
                let vt = Self::map_value_ty(&wty).unwrap_or_else(|| "int".into());
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
                let (esz, sgn) = Self::elem_esz_sgn(&vt);
                let mut ks = Vec::new();
                let mut vs = Vec::new();
                for (k, v) in pairs {
                    let g: inkwell::values::IntValue<'ctx> = match k {
                        anle_ir::MapKey::Int(n) => self.ctx.i64_type().const_int(*n as u64, false),
                        anle_ir::MapKey::Str(s) => {
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
                    &[self.ci(pairs.len() as i64), karr.into(), varr.into(), self.ci(esz), self.ci(sgn), self.ci(kty)],
                )
            }
            Rvalue::MapGet { map, key } => {
                let mt = self.arg_ty(f, map);
                let vt = Self::map_value_ty(&mt).unwrap_or_else(|| "int".into());
                let mv = self.value(f, allocas, map)?;
                let kv = self.value(f, allocas, key)?;
                let kv = self.key_ll(kv)?;
                let raw = self.call_rt("anle_map_at", &[self.pty(), self.ity()], Some(self.ity()), &[mv, kv])?;
                self.from_slot(raw, &vt)
            }
        }
    }

    ///nt: 圈内调用判定
    fn loop_call_assign(f: &Function, head: usize, cur: usize) -> bool {
        for b in f.blocks.iter().filter(|b| b.id >= head && b.id <= cur) {
            for (i, s) in b.stmts.iter().enumerate() {
                let Stmt::Assign { lhs, rhs: Rvalue::Call { name, .. } } = s else {
                    continue;
                };
                //nt: 复印语句不建段
                if matches!(name.as_str(), "#str_copy" | "#dc_list" | "#dc_map") {
                    continue;
                }
                //nt: 调用落临时则看下一句
                let 是临时 = f.locals.get(lhs.base).map(|l| l.name.is_none()).unwrap_or(true);
                let (dst, 位) = if 是临时 {
                    match b.stmts.get(i + 1) {
                        Some(Stmt::Assign { lhs: d, rhs }) => {
                            let src = match rhs {
                                Rvalue::Const(Value::Local(u)) => *u,
                                Rvalue::Use(p) => p.base,
                                _ => continue,
                            };
                            if src != lhs.base {
                                continue;
                            }
                            (d.base, i + 2)
                        }
                        _ => continue,
                    }
                } else {
                    (lhs.base, i + 1)
                };
                let Some(l) = f.locals.get(dst) else { continue };
                //nt: 未标跨圈无需护
                if !l.loop_escape {
                    continue;
                }
                //nt: 已复印逃逸值
                if matches!(
                    b.stmts.get(位),
                    Some(Stmt::Assign { rhs: Rvalue::Call { name, .. }, .. })
                        if matches!(name.as_str(), "#str_copy" | "#dc_list" | "#dc_map")
                ) {
                    continue;
                }
                return true;
            }
        }
        false
    }

    ///nt: 变体解引用
    fn struct_value(
        &self,
        f: &Function,
        allocas: &[Option<PointerValue<'ctx>>],
        v: &Value,
    ) -> Result<StructValue<'ctx>, String> {
        let ty = self.arg_ty(f, v);
        let base = ty.trim_end_matches('*').to_string();
        let st = *self.structs.get(&base).ok_or_else(|| format!("缺类型 {}", base))?;
        let raw = self.value(f, allocas, v)?;
        let sv = if ty.ends_with('*') {
            let p = match raw {
                BasicValueEnum::PointerValue(p) => p,
                _ => return Err("把手不是指针".into()),
            };
            self.builder.build_load(st, p, "hval").map_err(|e| e.to_string())?
        } else {
            raw
        };
        match sv {
            BasicValueEnum::StructValue(s) => Ok(s),
            _ => Err(format!("{} 不是结构值", base)),
        }
    }

    ///nt: 解析函数签名
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

    fn rt_ptr2(&mut self, name: &str) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function(name) {
            return f;
        }
        let ptr = self.ctx.ptr_type(AddressSpace::default());
        let ps = [BasicMetadataTypeEnum::from(ptr), BasicMetadataTypeEnum::from(ptr)];
        let fty = ptr.fn_type(&ps, false);
        self.module.add_function(name, fty, None)
    }

    fn rt_str_eq(&mut self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("anle_str_eq") {
            return f;
        }
        let ptr = self.ctx.ptr_type(AddressSpace::default());
        let ps = [BasicMetadataTypeEnum::from(ptr), BasicMetadataTypeEnum::from(ptr)];
        let fty = self.ctx.i64_type().fn_type(&ps, false);
        self.module.add_function("anle_str_eq", fty, None)
    }

    fn line_global(&mut self) -> GlobalValue<'ctx> {
        self.i64_global("anle_line")
    }

    fn i64_global(&mut self, name: &str) -> GlobalValue<'ctx> {
        if let Some(g) = self.module.get_global(name) {
            return g;
        }
        let g = self.module.add_global(self.ctx.i64_type(), None, name);
        g.set_linkage(inkwell::module::Linkage::External);
        g
    }

    fn ptr_global(&mut self, name: &str) -> GlobalValue<'ctx> {
        if let Some(g) = self.module.get_global(name) {
            return g;
        }
        let g = self.module.add_global(self.ctx.ptr_type(AddressSpace::default()), None, name);
        g.set_linkage(inkwell::module::Linkage::External);
        g
    }

    //nt: 跨块槽全局
    //nt: 同键共用一份存储
    fn slot_global(&mut self, key: &str, ty: &Ty, idx: usize) -> Result<PointerValue<'ctx>, String> {
        let name = format!("anle_slot_{}", idx);
        if let Some(g) = self.module.get_global(&name) {
            return Ok(g.as_pointer_value());
        }
        let t = self.ty(ty);
        let g = self.module.add_global(t, None, &name);
        g.set_linkage(inkwell::module::Linkage::Internal);
        g.set_initializer(&t.const_zero());
        Ok(g.as_pointer_value())
    }

    fn region_global(&mut self) -> GlobalValue<'ctx> {
        if let Some(g) = self.module.get_global("anle_region_cur") {
            return g;
        }
        let g = self.module.add_global(self.ctx.ptr_type(AddressSpace::default()), None, "anle_region_cur");
        g.set_linkage(inkwell::module::Linkage::External);
        g
    }

    fn region_enter(&mut self, name: &str) -> Result<(PointerValue<'ctx>, PointerValue<'ctx>), String> {
        let g = self.region_global();
        let parent = self.builder.build_load(self.ctx.ptr_type(AddressSpace::default()), g.as_pointer_value(), "parent").map_err(|e| e.to_string())?;
        let fv = self.rt_region_open();
        let blk = self.builder.build_global_string_ptr(name, "blk").map_err(|e| e.to_string())?.as_pointer_value();
        let c = self.builder.build_call(fv, &[parent.into(), blk.into()], "region").map_err(|e| e.to_string())?;
        let r = c.try_as_basic_value().basic().ok_or_else(|| "region_open 无返回值".to_string())?;
        self.builder.build_store(g.as_pointer_value(), r).map_err(|e| e.to_string())?;
        let parent = match parent {
            BasicValueEnum::PointerValue(p) => p,
            _ => return Err("region 父指针类型不对".into()),
        };
        let r = match r {
            BasicValueEnum::PointerValue(p) => p,
            _ => return Err("region_open 返回类型不对".into()),
        };
        Ok((parent, r))
    }

    fn region_leave(&mut self, parent: PointerValue<'ctx>, region: PointerValue<'ctx>) -> Result<(), String> {
        let g = self.region_global();
        self.builder.build_store(g.as_pointer_value(), parent).map_err(|e| e.to_string())?;
        let fv = self.rt_region_free();
        self.builder.build_call(fv, &[region.into()], "").map_err(|e| e.to_string())?;
        Ok(())
    }

    ///nt: 出口收段规则
    fn region_exit(
        &mut self,
        parent: PointerValue<'ctx>,
        region: PointerValue<'ctx>,
        free_own: bool,
    ) -> Result<(), String> {
        if free_own {
            self.region_leave(parent, region)
        } else {
            let g = self.region_global();
            self.builder
                .build_store(g.as_pointer_value(), parent)
                .map_err(|e| e.to_string())?;
            Ok(())
        }
    }

    fn rt_region_open(&mut self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("anle_region_open") {
            return f;
        }
        let ptr = self.ctx.ptr_type(AddressSpace::default());
        let ps = [BasicMetadataTypeEnum::from(ptr), BasicMetadataTypeEnum::from(ptr)];
        let fty = ptr.fn_type(&ps, false);
        self.module.add_function("anle_region_open", fty, None)
    }

    fn rt_region_free(&mut self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("anle_region_free") {
            return f;
        }
        let ptr = self.ctx.ptr_type(AddressSpace::default());
        let ps = [BasicMetadataTypeEnum::from(ptr)];
        let fty = self.ctx.void_type().fn_type(&ps, false);
        self.module.add_function("anle_region_free", fty, None)
    }

    fn binop(
        &mut self,
        f: &Function,
        op: BinOp,
        lhs: &Value,
        rhs: &Value,
        a: BasicValueEnum<'ctx>,
        b: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        //nt: 整浮混合提升
        //nt: 对齐C后端隐式转换
        if matches!(a, BasicValueEnum::FloatValue(_)) {
            if let BasicValueEnum::IntValue(bi) = b {
                let fb = self
                    .builder
                    .build_signed_int_to_float(bi, self.ctx.f64_type(), "i2f")
                    .map_err(|e| e.to_string())?;
                return self.binop(f, op, lhs, rhs, a, fb.into());
            }
        }
        if let BasicValueEnum::IntValue(ai) = a {
            if matches!(b, BasicValueEnum::FloatValue(_)) {
                let fa = self
                    .builder
                    .build_signed_int_to_float(ai, self.ctx.f64_type(), "i2f")
                    .map_err(|e| e.to_string())?;
                return self.binop(f, op, lhs, rhs, fa.into(), b);
            }
        }
        if let (BasicValueEnum::FloatValue(fa), BasicValueEnum::FloatValue(fb)) = (a, b) {
            let r = match op {
                BinOp::Add => self.builder.build_float_add(fa, fb, "fadd"),
                BinOp::Sub => self.builder.build_float_sub(fa, fb, "fsub"),
                BinOp::Mul => self.builder.build_float_mul(fa, fb, "fmul"),
                BinOp::Div => self.builder.build_float_div(fa, fb, "fdiv"),
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                    let p = match op {
                        BinOp::Eq => inkwell::FloatPredicate::OEQ,
                        BinOp::Ne => inkwell::FloatPredicate::ONE,
                        BinOp::Lt => inkwell::FloatPredicate::OLT,
                        BinOp::Le => inkwell::FloatPredicate::OLE,
                        BinOp::Gt => inkwell::FloatPredicate::OGT,
                        _ => inkwell::FloatPredicate::OGE,
                    };
                    let c = self.builder.build_float_compare(p, fa, fb, "fcmp").map_err(|e| e.to_string())?;
                    return Ok(self.builder.build_int_z_extend(c, self.ctx.i64_type(), "bool").map_err(|e| e.to_string())?.into());
                }
                _ => return Err("LLVM 后端不支持该浮点运算".into()),
            }.map_err(|e| e.to_string())?;
            return Ok(r.into());
        }
        if let (BasicValueEnum::PointerValue(pa), BasicValueEnum::PointerValue(pb)) = (a, b) {
            return match op {
                BinOp::Add => {
                    let fv = self.rt_ptr2("anle_concat");
                    let c = self.builder.build_call(fv, &[pa.into(), pb.into()], "concat").map_err(|e| e.to_string())?;
                    Ok(c.try_as_basic_value().basic().ok_or_else(|| "concat 无返回值".to_string())?)
                }
                BinOp::Eq | BinOp::Ne => {
                    let fv = self.rt_str_eq();
                    let c = self.builder.build_call(fv, &[pa.into(), pb.into()], "streq").map_err(|e| e.to_string())?;
                    let n = c.try_as_basic_value().basic().ok_or_else(|| "str_eq 无返回值".to_string())?;
                    let ni = match n {
                        BasicValueEnum::IntValue(i) => i,
                        _ => return Err("str_eq 返回类型不对".into()),
                    };
                    let zero = self.ctx.i64_type().const_int(0, false);
                    let pred = if op == BinOp::Eq { IntPredicate::NE } else { IntPredicate::EQ };
                    let c = self.builder.build_int_compare(pred, ni, zero, "streqcmp").map_err(|e| e.to_string())?;
                    Ok(self.builder.build_int_z_extend(c, self.ctx.i64_type(), "bool").map_err(|e| e.to_string())?.into())
                }
                _ => Err("LLVM 后端不支持该字符串运算".into()),
            };
        }
        let ai = match a {
            BasicValueEnum::IntValue(i) => i,
            _ => return Err("LLVM 后端算术只支持整数/浮点/字符串".into()),
        };
        let bi = match b {
            BasicValueEnum::IntValue(i) => i,
            _ => return Err("LLVM 后端算术只支持整数/浮点/字符串".into()),
        };
        let r = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                let n = match op {
                    BinOp::Add => "anle_sadd",
                    BinOp::Sub => "anle_ssub",
                    BinOp::Mul => "anle_smul",
                    BinOp::Div => "anle_div",
                    _ => "anle_mod",
                };
                let fv = self.runtime_i64_i64(n)?;
                let c = self.builder
                    .build_call(fv, &[ai.into(), bi.into()], n)
                    .map_err(|e| e.to_string())?;
                c.try_as_basic_value().basic().ok_or_else(|| "运行时调用没有返回值".to_string())?
            }
            BinOp::And | BinOp::Or => {
                //nt: 逻辑与或归一
                let ca = self.to_bool(ai.into())?;
                let cb = self.to_bool(bi.into())?;
                let c = if op == BinOp::And {
                    self.builder.build_and(ca, cb, "land")
                } else {
                    self.builder.build_or(ca, cb, "lor")
                }
                .map_err(|e| e.to_string())?;
                self.builder.build_int_z_extend(c, self.ctx.i64_type(), "bool").map_err(|e| e.to_string())?.into()
            }
            BinOp::BitAnd => self.builder.build_and(ai, bi, "band").map_err(|e| e.to_string())?.into(),
            BinOp::BitOr => self.builder.build_or(ai, bi, "bor").map_err(|e| e.to_string())?.into(),
            BinOp::BitXor => self.builder.build_xor(ai, bi, "bxor").map_err(|e| e.to_string())?.into(),
            BinOp::Shl => self.builder.build_left_shift(ai, bi, "shl").map_err(|e| e.to_string())?.into(),
            BinOp::Shr => {
                //nt: 按符号选右移
                let arith = !Self::fixed_width(f, lhs).map(|(_, s)| !s).unwrap_or(false);
                self.builder.build_right_shift(ai, bi, arith, "shr").map_err(|e| e.to_string())?.into()
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let p = match op {
                    BinOp::Eq => IntPredicate::EQ,
                    BinOp::Ne => IntPredicate::NE,
                    BinOp::Lt => IntPredicate::SLT,
                    BinOp::Le => IntPredicate::SLE,
                    BinOp::Gt => IntPredicate::SGT,
                    _ => IntPredicate::SGE,
                };
                let c = self.builder.build_int_compare(p, ai, bi, "cmp").map_err(|e| e.to_string())?;
                    self.builder.build_int_z_extend(c, self.ctx.i64_type(), "bool").map_err(|e| e.to_string())?.into()
            }
        };
        //nt: 结果按宽度截
        if matches!(
            op,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod
                | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr
        ) {
            if let Some((w, signed)) = Self::fixed_width(f, lhs).or_else(|| Self::fixed_width(f, rhs)) {
                if w < 64 {
                    if let BasicValueEnum::IntValue(i) = r {
                        let nt = match w {
                            8 => self.ctx.i8_type(),
                            16 => self.ctx.i16_type(),
                            _ => self.ctx.i32_type(),
                        };
                        let t = self.builder.build_int_truncate(i, nt, "wrap").map_err(|e| e.to_string())?;
                        let e = if signed {
                            self.builder.build_int_s_extend(t, self.ctx.i64_type(), "sext")
                        } else {
                            self.builder.build_int_z_extend(t, self.ctx.i64_type(), "zext")
                        }
                        .map_err(|e| e.to_string())?;
                        return Ok(e.into());
                    }
                }
            }
        }
        Ok(r)
    }

    ///nt: 定长族宽度
    fn fixed_width(f: &Function, v: &Value) -> Option<(u32, bool)> {
        let t = match v {
            Value::Local(id) => f.locals.get(*id).map(|l| l.ty.0.as_str())?,
            _ => return None,
        };
        match t {
            "int_8" => Some((8, true)),
            "uint_8" => Some((8, false)),
            "int_16" => Some((16, true)),
            "uint_16" => Some((16, false)),
            "int_32" => Some((32, true)),
            "uint_32" => Some((32, false)),
            "int_64" => Some((64, true)),
            "uint_64" => Some((64, false)),
            _ => None,
        }
    }

    fn runtime_i64_i64(&mut self, name: &str) -> Result<FunctionValue<'ctx>, String> {
        if let Some(f) = self.module.get_function(name) {
            return Ok(f);
        }
        let ps = [
            BasicMetadataTypeEnum::from(self.ctx.i64_type()),
            BasicMetadataTypeEnum::from(self.ctx.i64_type()),
        ];
        let fty = self.ctx.i64_type().fn_type(&ps, false);
        Ok(self.module.add_function(name, fty, None))
    }

    fn call(
        &mut self,
        _f: &Function,
        name: &str,
        args: &[BasicValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let fv = *self.funcs.get(name).ok_or_else(|| format!("LLVM 后端不认识调用 {}", name))?;
        let mut av: Vec<BasicMetadataValueEnum<'ctx>> = Vec::new();
        for a in args {
            av.push((*a).into());
        }
        let c = self.builder.build_call(fv, &av, "call").map_err(|e| e.to_string())?;
        Ok(c.try_as_basic_value().basic().unwrap_or_else(|| self.ctx.i64_type().const_int(0, false).into()))
    }

    fn print(
        &mut self,
        f: &Function,
        allocas: &[Option<PointerValue<'ctx>>],
        args: &[Value],
    ) -> Result<(), String> {
        let printf = self.printf();
        if args.is_empty() {
            let fmt = self.fmt("\n");
            let _ = self.builder.build_call(printf, &[fmt.into()], "").map_err(|e| e.to_string())?;
            return Ok(());
        }
        for (i, a) in args.iter().enumerate() {
            let ty = self.arg_ty(f, a);
            //nt: 积类型不可印
            if self.structs.contains_key(ty.trim_end_matches('*')) {
                return Err("print 不收积类型——先取字段（结构可能成环）".into());
            }
            let ck = if ty.starts_with("list(str") {
                Some("list_s")
            } else if ty == "list" || ty.starts_with("list(") {
                if Self::seq_elem_ty(&ty).as_deref() == Some("float") {
                    Some("list_f")
                } else {
                    Some("list")
                }
            } else if ty.starts_with("que") {
                match Self::seq_elem_ty(&ty).as_deref() {
                    Some("str") => Some("que_s"),
                    Some("float") => Some("que_f"),
                    _ => Some("que"),
                }
            } else if ty.starts_with("stack") {
                match Self::seq_elem_ty(&ty).as_deref() {
                    Some("str") => Some("stack_s"),
                    Some("float") => Some("stack_f"),
                    _ => Some("stack"),
                }
            } else if ty == "map" || ty.starts_with("map(") {
                match Self::map_value_ty(&ty).as_deref() {
                    Some("str") => Some("map_s"),
                    Some("float") => Some("map_f"),
                    _ => Some("map"),
                }
            } else {
                None
            };
            if let Some(k) = ck {
                if i > 0 {
                    let sp = self.fmt(" ");
                    let _ = self.builder.build_call(printf, &[sp.into()], "").map_err(|e| e.to_string())?;
                }
                let v = self.value(f, allocas, a)?;
                match k {
                    "list_s" => {
                        self.call_rt("anle_list_print_s", &[self.pty()], None, &[v])?;
                    }
                    "list_f" => {
                        self.call_rt("anle_list_print_f", &[self.pty()], None, &[v])?;
                    }
                    "list" => {
                        self.call_rt("anle_list_print", &[self.pty()], None, &[v])?;
                    }
                    "que" | "stack" | "que_f" | "stack_f" | "que_s" | "stack_s" => {
                        let tag = if k.starts_with("que") { "que" } else { "stack" };
                        let tv = self.fmt(tag);
                        let name = if k.ends_with("_f") {
                            "anle_list_print_tag_f"
                        } else if k.ends_with("_s") {
                            "anle_list_print_tag_s"
                        } else {
                            "anle_list_print_tag"
                        };
                        self.call_rt(name, &[self.pty(), self.pty()], None, &[v, tv.into()])?;
                    }
                    "map_f" => {
                        self.call_rt("anle_map_print_f", &[self.pty()], None, &[v])?;
                    }
                    "map_s" => {
                        self.call_rt("anle_map_print_s", &[self.pty()], None, &[v])?;
                    }
                    _ => {
                        self.call_rt("anle_map_print", &[self.pty()], None, &[v])?;
                    }
                }
                continue;
            }
            let v = self.value(f, allocas, a)?;
            let (spec, val) = self.print_arg(f, a, v)?;
            let fmt = if i == 0 { spec } else { format!(" {}", spec) };
            let fmtv = self.fmt(&fmt);
            let _ = self.builder
                .build_call(printf, &[fmtv.into(), val.into()], "")
                .map_err(|e| e.to_string())?;
        }
        let nl = self.fmt("\n");
        let _ = self.builder.build_call(printf, &[nl.into()], "").map_err(|e| e.to_string())?;
        Ok(())
    }

    fn print_arg(
        &mut self,
        f: &Function,
        a: &Value,
        v: BasicValueEnum<'ctx>,
    ) -> Result<(String, BasicValueEnum<'ctx>), String> {
        let ty = match a {
            Value::Local(id) => self.local_ty(f, *id).0,
            Value::Float(_) => "float".to_string(),
            Value::Str(_) => "str".to_string(),
            Value::Bool(_) => "bool".to_string(),
            _ => "int".to_string(),
        };
        if ty == "bool" {
            let fv = self.runtime_bool_str();
            let c = self.builder.build_call(fv, &[v.into()], "bstr").map_err(|e| e.to_string())?;
            let s = c.try_as_basic_value().basic().ok_or_else(|| "bool_str 无返回值".to_string())?;
            return Ok(("%s".to_string(), s));
        }
        if ty == "float" {
            return Ok(("%g".to_string(), v));
        }
        if ty == "str" {
            return Ok(("%s".to_string(), v));
        }
        //nt: 指针转整数
        if let BasicValueEnum::PointerValue(p) = v {
            let i = self.builder.build_ptr_to_int(p, self.ctx.i64_type(), "pv").map_err(|e| e.to_string())?;
            return Ok(("%lld".to_string(), i.into()));
        }
        Ok(("%lld".to_string(), v))
    }

    fn runtime_bool_str(&mut self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("anle_bool_str") {
            return f;
        }
        let ps = [BasicMetadataTypeEnum::from(self.ctx.i64_type())];
        let fty = self.ctx.ptr_type(AddressSpace::default()).fn_type(&ps, false);
        self.module.add_function("anle_bool_str", fty, None)
    }

    ///nt: 内置函数分派
    fn builtin(
        &mut self,
        f: &Function,
        allocas: &[Option<PointerValue<'ctx>>],
        name: &str,
        args: &[Value],
    ) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        let known = matches!(
            name,
            "len" | "len_utf8" | "empty" | "push" | "pop" | "insert" | "remove" | "sort"
                | "clear" | "has" | "index" | "slice" | "keys" | "vals" | "put" | "que" | "stack"
                | "str" | "int" | "float" | "bool" | "code" | "char" | "concat" | "sub_str"
                | "nth_char" | "#str_copy" | "#dc_list" | "#dc_map"
                | "#str_iter_start" | "#str_iter_next"
                | "args" | "read_file" | "save_file" | "run" | "rnd" | "sle_ms"
                | "str_to_c" | "str_from_c"
                | "read_8" | "read_16" | "read_32" | "read_64"
                | "write_8" | "write_16" | "write_32" | "write_64"
        );
        let known = known || name.starts_with("#dcdeep_");
        if !known {
            return Ok(None);
        }
        let mut vals = Vec::new();
        for a in args {
            vals.push(self.value(f, allocas, a)?);
        }
        let t0 = args.first().map(|a| self.arg_ty(f, a)).unwrap_or_default();
        let elem = Self::seq_elem_ty(&t0).unwrap_or_default();
        let v = match name {
            "len" => {
                if Self::is_container_ty(&t0) && !t0.starts_with("map") {
                    self.call_rt("anle_list_len", &[self.pty()], Some(self.ity()), &vals)?
                } else if t0.starts_with("map") {
                    self.call_rt("anle_map_len", &[self.pty()], Some(self.ity()), &vals)?
                } else {
                    self.call_rt("anle_str_char_len", &[self.pty()], Some(self.ity()), &vals)?
                }
            }
            "len_utf8" => self.call_rt("anle_str_byte_len", &[self.pty()], Some(self.ity()), &vals)?,
            "empty" => {
                let n = if t0.starts_with("map") {
                    self.call_rt("anle_map_len", &[self.pty()], Some(self.ity()), &vals)?
                } else {
                    self.call_rt("anle_list_len", &[self.pty()], Some(self.ity()), &vals)?
                };
                let ni = match n {
                    BasicValueEnum::IntValue(i) => i,
                    _ => return Err("len 返回类型不对".into()),
                };
                let z = self.ctx.i64_type().const_int(0, false);
                let c = self.builder.build_int_compare(IntPredicate::EQ, ni, z, "isempty").map_err(|e| e.to_string())?;
                self.builder.build_int_z_extend(c, self.ctx.i64_type(), "bool").map_err(|e| e.to_string())?.into()
            }
            "push" => {
                let mut a1 = *vals.get(1).ok_or_else(|| "push 要 2 个参数".to_string())?;
                //nt: fn自有对象复印重绑
                if let (Value::Local(lid), BasicValueEnum::PointerValue(pv)) = (&args[1], a1) {
                    if f.locals.get(*lid).map(|l| l.fn_owned).unwrap_or(false) {
                        let base = f.locals[*lid].ty.0.trim_end_matches('*').to_string();
                        if let Some(st) = self.structs.get(&base).cloned() {
                            let 已逃 = ESCAPED.with(|s| s.borrow().contains(lid));
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
                                }
                                ESCAPED.with(|s| s.borrow_mut().insert(*lid));
                                a1 = c;
                            }
                        }
                    }
                }
                if let BasicValueEnum::StructValue(sv) = a1 {
                    //nt: 聚合按地址
                    let st = sv.get_type();
                    let a = self.alloca(st, "ap").map_err(|e| e.to_string())?;
                    self.builder.build_store(a, sv).map_err(|e| e.to_string())?;
                    return Ok(Some(self.call_rt(
                        "anle_list_push_from",
                        &[self.pty(), self.pty()],
                        Some(self.ity()),
                        &[vals[0], a.into()],
                    )?));
                }
                let slot = if elem == "str" {
                    let c = self.call_rt("anle_str_copy", &[self.pty()], Some(self.pty()), &[a1])?;
                    self.to_slot(c)?
                } else {
                    self.to_slot(a1)?
                };
                self.call_rt("anle_list_push", &[self.pty(), self.ity()], Some(self.ity()), &[vals[0], slot.into()])?
            }
            "pop" => {
                let raw = if t0.starts_with("que") {
                    self.call_rt("anle_list_remove", &[self.pty(), self.ity()], Some(self.ity()), &[vals[0], self.ci(1)])?
                } else {
                    self.call_rt("anle_list_pop", &[self.pty()], Some(self.ity()), &[vals[0]])?
                };
                self.from_slot(raw, &elem)?
            }
            "insert" => {
                let a2 = *vals.get(2).ok_or_else(|| "insert 要 3 个参数".to_string())?;
                if let BasicValueEnum::StructValue(sv) = a2 {
                    //nt: 聚合按地址
                    let st = sv.get_type();
                    let a = self.alloca(st, "ai").map_err(|e| e.to_string())?;
                    self.builder.build_store(a, sv).map_err(|e| e.to_string())?;
                    return Ok(Some(self.call_rt(
                        "anle_list_insert_from",
                        &[self.pty(), self.ity(), self.pty()],
                        Some(self.ity()),
                        &[vals[0], vals[1], a.into()],
                    )?));
                }
                let slot = self.to_slot(a2)?;
                self.call_rt(
                    "anle_list_insert",
                    &[self.pty(), self.ity(), self.ity()],
                    Some(self.ity()),
                    &[vals[0], vals[1], slot.into()],
                )?
            }
            "remove" => {
                if t0.starts_with("map") {
                    let k = self.key_ll(vals[1])?;
                    self.call_rt("anle_map_remove", &[self.pty(), self.ity()], Some(self.ity()), &[vals[0], k])?
                } else {
                    let raw = self.call_rt(
                        "anle_list_remove",
                        &[self.pty(), self.ity()],
                        Some(self.ity()),
                        &[vals[0], vals[1]],
                    )?;
                    self.from_slot(raw, &elem)?
                }
            }
            "sort" => {
                let n = if elem == "str" { "anle_list_sort_s" } else { "anle_list_sort" };
                self.call_rt(n, &[self.pty()], None, &vals)?;
                self.ci(0)
            }
            "clear" => {
                let n = if t0.starts_with("map") { "anle_map_clear" } else { "anle_list_clear" };
                self.call_rt(n, &[self.pty()], None, &vals)?;
                self.ci(0)
            }
            "has" | "index" => {
                if t0.starts_with("map") {
                    let k = self.key_ll(vals[1])?;
                    if name == "has" {
                        self.call_rt("anle_map_contains", &[self.pty(), self.ity()], Some(self.ity()), &[vals[0], k])?
                    } else {
                        let vt = Self::map_value_ty(&t0).unwrap_or_else(|| "int".into());
                        let raw = self.call_rt("anle_map_at", &[self.pty(), self.ity()], Some(self.ity()), &[vals[0], k])?;
                        self.from_slot(raw, &vt)?
                    }
                } else if elem == "str" {
                    let n = if name == "has" { "anle_list_contains_s" } else { "anle_list_index_s" };
                    self.call_rt(n, &[self.pty(), self.pty()], Some(self.ity()), &vals[0..2])?
                } else {
                    let n = if name == "has" { "anle_list_contains" } else { "anle_list_index" };
                    let slot = self.to_slot(vals[1])?;
                    self.call_rt(n, &[self.pty(), self.ity()], Some(self.ity()), &[vals[0], slot.into()])?
                }
            }
            "slice" => {
                if t0 == "str" {
                    self.call_rt("anle_sub_str", &[self.pty(), self.ity(), self.ity()], Some(self.pty()), &vals[0..3])?
                } else {
                    self.call_rt("anle_list_slice", &[self.pty(), self.ity(), self.ity()], Some(self.pty()), &vals[0..3])?
                }
            }
            "keys" => self.call_rt("anle_map_keys", &[self.pty()], Some(self.pty()), &vals)?,
            "vals" => self.call_rt("anle_map_values", &[self.pty()], Some(self.pty()), &vals)?,
            "put" => {
                let vt = Self::map_value_ty(&t0).unwrap_or_else(|| "int".into());
                let a2 = *vals.get(2).ok_or_else(|| "put 要 3 个参数".to_string())?;
                let slot = if vt == "str" {
                    let c = self.call_rt("anle_str_copy", &[self.pty()], Some(self.pty()), &[a2])?;
                    self.to_slot(c)?
                } else {
                    self.to_slot(a2)?
                };
                let k = self.key_ll(vals[1])?;
                self.call_rt("anle_map_put", &[self.pty(), self.ity(), self.ity()], None, &[vals[0], k, slot.into()])?;
                self.ci(0)
            }
            "que" | "stack" => {
                if vals.is_empty() {
                    let wty = self.want.clone().map(|t| t.0).unwrap_or_default();
                    let et = Self::seq_elem_ty(&wty).unwrap_or_else(|| "int".into());
                    let (esz, sgn) = Self::elem_esz_sgn(&et);
                    self.call_rt(
                        "anle_list_new",
                        &[self.ity(), self.ity()],
                        Some(self.pty()),
                        &[self.ci(esz), self.ci(sgn)],
                    )?
                } else {
                    vals[0]
                }
            }
            "str" => {
                let is_float = matches!(args.first(), Some(Value::Float(_))) || t0 == "float";
                if is_float {
                    let ps = [BasicTypeEnum::from(self.ctx.f64_type())];
                    self.call_rt("anle_str_of_float", &ps, Some(self.pty()), &vals)?
                } else {
                    self.call_rt("anle_str_of_int", &[self.ity()], Some(self.pty()), &vals)?
                }
            }
            "int" => {
                //nt: 按型转int
                match vals[0] {
                    BasicValueEnum::IntValue(i) => self
                        .builder
                        .build_int_cast(i, self.ctx.i64_type(), "i")
                        .map_err(|e| e.to_string())?
                        .into(),
                    BasicValueEnum::FloatValue(f) => self
                        .builder
                        .build_float_to_signed_int(f, self.ctx.i64_type(), "i")
                        .map_err(|e| e.to_string())?
                        .into(),
                    _ => self.call_rt("anle_int_of_str", &[self.pty()], Some(self.ity()), &vals)?,
                }
            }
            "float" => {
                //nt: 按型转float
                match vals[0] {
                    BasicValueEnum::PointerValue(_) => {
                        self.call_rt("anle_float_of_str", &[self.pty()], Some(BasicTypeEnum::from(self.ctx.f64_type())), &vals)?
                    }
                    BasicValueEnum::IntValue(i) => self
                        .builder
                        .build_signed_int_to_float(i, self.ctx.f64_type(), "f")
                        .map_err(|e| e.to_string())?
                        .into(),
                    _ => vals[0],
                }
            }
            "bool" => {
                let i = match vals[0] {
                    BasicValueEnum::IntValue(i) => i,
                    _ => return Err("bool 只收整数".into()),
                };
                let z = self.ctx.i64_type().const_int(0, false);
                let c = self.builder.build_int_compare(IntPredicate::NE, i, z, "b").map_err(|e| e.to_string())?;
                self.builder.build_int_z_extend(c, self.ctx.i64_type(), "bool").map_err(|e| e.to_string())?.into()
            }
            "code" => self.call_rt("anle_char_code", &[self.pty()], Some(self.ity()), &vals)?,
            "char" => self.call_rt("anle_code_char", &[self.ity()], Some(self.pty()), &vals)?,
            "concat" => {
                self.call_rt("anle_concat", &[self.pty(), self.pty()], Some(self.pty()), &vals[0..2])?
            }
            "sub_str" => self.call_rt("anle_sub_str", &[self.pty(), self.ity(), self.ity()], Some(self.pty()), &vals[0..3])?,
            "nth_char" => self.call_rt("anle_nth_char", &[self.pty(), self.ity()], Some(self.pty()), &vals[0..2])?,
            //nt: 串顺序取字符
            "#str_iter_start" => {
                self.call_rt("anle_str_iter_start", &[self.pty()], None, &vals[0..1])?;
                self.ctx.i64_type().const_int(0, false).into()
            }
            "#str_iter_next" => {
                self.call_rt("anle_str_iter_next", &[self.pty(), self.ity()], Some(self.pty()), &vals[0..2])?
            }
            "#str_copy" => self.call_rt("anle_str_copy", &[self.pty()], Some(self.pty()), &vals)?,
            "#dc_list" => self.call_rt("anle_dc_list", &[self.pty(), self.ity()], Some(self.pty()), &vals[0..2])?,
            "#dc_map" => self.call_rt("anle_dc_map", &[self.pty(), self.ity()], Some(self.pty()), &vals[0..2])?,
            //nt: 结构体深拷
            n if n.starts_with("#dcdeep_") => {
                let 型 = n.trim_start_matches("#dcdeep_").to_string();
                match vals[0] {
                    BasicValueEnum::StructValue(sv) => self.deep_copy_struct(&型, sv)?,
                    _ => return Err("深拷目标不是结构体".into()),
                }
            }
            "args" => self.call_rt("anle_args", &[], Some(self.pty()), &[])?,
            "read_file" => self.call_rt("anle_read_file", &[self.pty()], Some(self.pty()), &vals)?,
            "save_file" => self.call_rt("anle_save_file", &[self.pty(), self.pty()], Some(self.ity()), &vals[0..2])?,
            "run" => {
                //nt: 缺省为空
                let p0 = *vals.first().ok_or_else(|| "run 要 1 个参数".to_string())?;
                let a1 = vals.get(1).copied().unwrap_or_else(|| self.ctx.ptr_type(AddressSpace::default()).const_null().into());
                let a2 = match vals.get(2).copied() {
                    Some(v) => v,
                    None => self.fmt("").into(),
                };
                let a3 = match vals.get(3).copied() {
                    Some(v) => v,
                    None => self.fmt("").into(),
                };
                self.call_rt(
                    "anle_run",
                    &[self.pty(), self.pty(), self.pty(), self.pty()],
                    Some(self.ity()),
                    &[p0, a1, a2, a3],
                )?
            }
            "rnd" => self.call_rt("anle_random", &[self.ity()], Some(self.ity()), &vals)?,
            "sle_ms" => {
                self.call_rt("anle_sleep", &[self.ity()], None, &vals)?;
                self.ci(0)
            }
            "str_to_c" => {
                let p = self.call_rt("anle_to_c_str", &[self.pty()], Some(self.pty()), &vals)?;
                let pv = match p {
                    BasicValueEnum::PointerValue(p) => p,
                    _ => return Err("to_c_str 返回类型不对".into()),
                };
                self.builder
                    .build_ptr_to_int(pv, self.ctx.i64_type(), "caddr")
                    .map_err(|e| e.to_string())?
                    .into()
            }
            "str_from_c" => {
                let i = match vals[0] {
                    BasicValueEnum::IntValue(i) => i,
                    _ => return Err("str_from_c 只收整数地址".into()),
                };
                let p = self
                    .builder
                    .build_int_to_ptr(i, self.ctx.ptr_type(AddressSpace::default()), "cptr")
                    .map_err(|e| e.to_string())?;
                self.call_rt("anle_from_c_str", &[self.pty()], Some(self.pty()), &[p.into()])?
            }
            "read_8" | "read_16" | "read_32" | "read_64" => {
                let i = match vals[0] {
                    BasicValueEnum::IntValue(i) => i,
                    _ => return Err(format!("{} 只收整数地址", name)),
                };
                let p = self
                    .builder
                    .build_int_to_ptr(i, self.ctx.ptr_type(AddressSpace::default()), "mmio")
                    .map_err(|e| e.to_string())?;
                //nt: 符号扩展
                let w = match name {
                    "read_8" => 8,
                    "read_16" => 16,
                    "read_32" => 32,
                    _ => 64,
                };
                if w == 64 {
                    self.builder.build_load(self.ctx.i64_type(), p, "rd").map_err(|e| e.to_string())?
                } else {
                    let t = match w {
                        8 => self.ctx.i8_type(),
                        _ => self.ctx.i16_type(),
                    };
                    let raw = self.builder.build_load(t, p, "rd").map_err(|e| e.to_string())?;
                    let iv = match raw {
                        BasicValueEnum::IntValue(i) => i,
                        _ => return Err(format!("{} 读数类型不对", name)),
                    };
                    self.builder.build_int_s_extend(iv, self.ctx.i64_type(), "sext").map_err(|e| e.to_string())?.into()
                }
            }
            "write_8" | "write_16" | "write_32" | "write_64" => {
                let i = match vals[0] {
                    BasicValueEnum::IntValue(i) => i,
                    _ => return Err(format!("{} 只收整数地址", name)),
                };
                let p = self
                    .builder
                    .build_int_to_ptr(i, self.ctx.ptr_type(AddressSpace::default()), "mmio")
                    .map_err(|e| e.to_string())?;
                let v = match vals.get(1) {
                    Some(BasicValueEnum::IntValue(v)) => *v,
                    _ => return Err(format!("{} 要地址与值两个参数", name)),
                };
                let w = match name {
                    "write_8" => 8,
                    "write_16" => 16,
                    "write_32" => 32,
                    _ => 64,
                };
                if w == 64 {
                    self.builder.build_store(p, v).map_err(|e| e.to_string())?;
                } else {
                    let t = match w {
                        8 => self.ctx.i8_type(),
                        16 => self.ctx.i16_type(),
                        _ => self.ctx.i32_type(),
                    };
                    let tv = self.builder.build_int_truncate(v, t, "wt").map_err(|e| e.to_string())?;
                    self.builder.build_store(p, tv).map_err(|e| e.to_string())?;
                }
                self.ci(0)
            }
            _ => return Ok(None),
        };
        Ok(Some(v))
    }

    fn printf(&mut self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("printf") {
            return f;
        }
        let ps = [BasicMetadataTypeEnum::from(self.ctx.ptr_type(AddressSpace::default()))];
        let fty = self.ctx.i32_type().fn_type(&ps, true);
        self.module.add_function("printf", fty, None)
    }

    fn fmt(&mut self, s: &str) -> PointerValue<'ctx> {
        self.builder.build_global_string_ptr(s, "fmt").map_err(|e| e.to_string()).unwrap().as_pointer_value()
    }
}
