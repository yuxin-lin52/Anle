//! 扁平索引载体：指令以数组下标为身份（`flat`）
//!
//! 指令存扁平数组，值 = 指令索引，变长载荷进 extra 区。
//! 每条指令产一个值；语句类指令不产值。块参数 = φ 承接。
//! 与树形 IR 并存：由 from_program 转换，dump 对拍后两个后端改吃本载体。

use crate::{BinOp, Function, LocalId, MapKey, Place, Program, Proj, Rvalue, Stmt, Ty, Value};
use anle_error::Span;

//nt: 指令号
pub type InstId = u32;

//nt: 二元运算
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

//nt: 树形转
impl Op {
    pub fn from_binop(op: &BinOp) -> Op {
        match op {
            BinOp::Add => Op::Add,
            BinOp::Sub => Op::Sub,
            BinOp::Mul => Op::Mul,
            BinOp::Div => Op::Div,
            BinOp::Mod => Op::Mod,
            BinOp::Eq => Op::Eq,
            BinOp::Ne => Op::Ne,
            BinOp::Lt => Op::Lt,
            BinOp::Le => Op::Le,
            BinOp::Gt => Op::Gt,
            BinOp::Ge => Op::Ge,
            BinOp::And => Op::And,
            BinOp::Or => Op::Or,
            BinOp::BitAnd => Op::BitAnd,
            BinOp::BitOr => Op::BitOr,
            BinOp::BitXor => Op::BitXor,
            BinOp::Shl => Op::Shl,
            BinOp::Shr => Op::Shr,
        }
    }

    //nt: 转回树形运算
    pub fn to_binop(&self) -> BinOp {
        match self {
            Op::Add => BinOp::Add,
            Op::Sub => BinOp::Sub,
            Op::Mul => BinOp::Mul,
            Op::Div => BinOp::Div,
            Op::Mod => BinOp::Mod,
            Op::Eq => BinOp::Eq,
            Op::Ne => BinOp::Ne,
            Op::Lt => BinOp::Lt,
            Op::Le => BinOp::Le,
            Op::Gt => BinOp::Gt,
            Op::Ge => BinOp::Ge,
            Op::And => BinOp::And,
            Op::Or => BinOp::Or,
            Op::BitAnd => BinOp::BitAnd,
            Op::BitOr => BinOp::BitOr,
            Op::BitXor => BinOp::BitXor,
            Op::Shl => BinOp::Shl,
            Op::Shr => BinOp::Shr,
        }
    }

    //nt: 从编号转
    pub fn from_u32(v: u32) -> Option<Op> {
        match v {
            0 => Some(Op::Add),
            1 => Some(Op::Sub),
            2 => Some(Op::Mul),
            3 => Some(Op::Div),
            4 => Some(Op::Mod),
            5 => Some(Op::Eq),
            6 => Some(Op::Ne),
            7 => Some(Op::Lt),
            8 => Some(Op::Le),
            9 => Some(Op::Gt),
            10 => Some(Op::Ge),
            11 => Some(Op::And),
            12 => Some(Op::Or),
            13 => Some(Op::BitAnd),
            14 => Some(Op::BitOr),
            15 => Some(Op::BitXor),
            16 => Some(Op::Shl),
            17 => Some(Op::Shr),
            _ => None,
        }
    }
}

//nt: 指令种类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    ConstInt,
    ConstFloat,
    ConstStr,
    ConstBool,
    ConstNull,
    ReadLocal,
    GetField,
    GetFieldAddr,
    GetIndex,
    GetIndexAddr,
    Deref,
    BinOp,
    Call,
    CallIndirect,
    FnRef,
    ListLit,
    MapLit,
    ListGet,
    MapGet,
    ListSet,
    MapSet,
    VariantLit,
    VariantTag,
    VariantGet,
    Ref,
    Alloc,
    Bind,
    Drop,
    Kill,
    DropVar,
    ListSetStmt,
    MapSetStmt,
    AccessBegin,
    AccessEnd,
    Assert,
    Asm,
    Nop,
}

//nt: 指令
#[derive(Debug, Clone, Copy)]
pub struct Inst {
    pub tag: Tag,
    pub a: u32,
    pub b: u32,
    //nt: 变长载荷长度
    pub c: u32,
}

//nt: 块出口
#[derive(Debug, Clone)]
pub enum FExit {
    Return(Option<InstId>),
    Branch {
        cond: InstId,
        targets: Vec<(String, Vec<InstId>)>,
    },
    Unreachable,
}

//nt: 块
#[derive(Debug, Clone)]
pub struct FlatBlock {
    //nt: 块名 bbN
    pub name: String,
    pub start: InstId,
    //nt: 块参数值
    pub params: Vec<InstId>,
    pub exit: FExit,
}

//nt: 函数级扁平体
#[derive(Debug, Clone, Default)]
pub struct FlatFn {
    pub name: String,
    pub ret: Option<String>,
    //nt: 形参名
    pub params: Vec<String>,
    pub insts: Vec<Inst>,
    //nt: 逐指令类型号
    pub itys: Vec<u32>,
    //nt: 逐指令源行
    pub lines: Vec<u32>,
    //nt: 逐指令源列
    pub cols: Vec<u32>,
    //nt: 逐指令源宽
    pub lens: Vec<u32>,
    pub extra: Vec<u32>,
    pub ints: Vec<i64>,
    pub floats: Vec<f64>,
    pub strings: Vec<String>,
    pub blocks: Vec<FlatBlock>,
    //nt: 投影写目标位置
    pub places: Vec<Option<crate::Place>>,
}

//nt: 全程序扁平体
#[derive(Debug, Clone, Default)]
pub struct Flat {
    pub fns: Vec<FlatFn>,
}

//nt: 转换中的一个函数
struct 转中<'a> {
    f: &'a Function,
    //nt: 局部号 → 当前定义指令
    定义: Vec<Option<InstId>>,
    //nt: 多变局部走存储
    多变: Vec<bool>,
    //nt: 块号 → 块参数值
    块参: Vec<Vec<InstId>>,
    insts: Vec<Inst>,
    //nt: 逐指令类型号
    itys: Vec<u32>,
    //nt: 逐指令源行
    lines: Vec<u32>,
    //nt: 逐指令源列
    cols: Vec<u32>,
    //nt: 逐指令源宽
    lens: Vec<u32>,
    //nt: 投影写目标位置
    places: Vec<Option<crate::Place>>,
    //nt: 当前语句行列宽
    行: u32,
    列: u32,
    宽: u32,
    extra: Vec<u32>,
    ints: Vec<i64>,
    floats: Vec<f64>,
    strings: Vec<String>,
    名池: std::collections::BTreeMap<String, InstId>,
}

impl 转中<'_> {
    //nt: 入名池
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(i) = self.名池.get(s) {
            return *i;
        }
        self.strings.push(s.to_string());
        let i = (self.strings.len() - 1) as u32;
        self.名池.insert(s.to_string(), i);
        i
    }

    //nt: 追加指令
    fn push(&mut self, tag: Tag, a: u32, b: u32, c: u32) -> InstId {
        let id = self.insts.len() as InstId;
        self.insts.push(Inst { tag, a, b, c });
        self.itys.push(0);
        self.lines.push(self.行);
        self.cols.push(self.列);
        self.lens.push(self.宽);
        self.places.push(None);
        id
    }

    //nt: 带类型追加
    fn push_ty(&mut self, tag: Tag, a: u32, b: u32, c: u32, ty: &str) -> u32 {
        let id = self.push(tag, a, b, c);
        self.itys[id as usize] = self.intern(ty);
        id
    }

    //nt: 变长载荷
    fn push_extra(&mut self, items: &[InstId]) -> (u32, u32) {
        let start = self.extra.len() as u32;
        self.extra.extend_from_slice(items);
        (start, items.len() as u32)
    }

    //nt: 常量整数
    fn int_c(&mut self, v: i64) -> InstId {
        self.ints.push(v);
        self.push_ty(Tag::ConstInt, (self.ints.len() - 1) as u32, 0, 0, "int")
    }

    //nt: 常量浮点
    fn float_c(&mut self, v: f64) -> InstId {
        self.floats.push(v);
        self.push_ty(Tag::ConstFloat, (self.floats.len() - 1) as u32, 0, 0, "float")
    }

    //nt: 常量串
    fn str_c(&mut self, v: &str) -> InstId {
        let id = self.intern(v);
        self.push_ty(Tag::ConstStr, id, 0, 0, "str")
    }

    //nt: 常量布尔
    fn bool_c(&mut self, v: bool) -> InstId {
        self.push_ty(Tag::ConstBool, v as u32, 0, 0, "bool")
    }

    //nt: 入类型池
    fn ty_id(&mut self, t: &Ty) -> u32 {
        self.intern(&t.0)
    }

    //nt: 值转换
    fn val(&mut self, v: &Value) -> InstId {
        match v {
            Value::Local(id) => {
                //nt: 多变局部立读点
                if self.多变.get(*id).copied().unwrap_or(true) {
                    let n = self
                        .f
                        .locals
                        .get(*id)
                        .and_then(|l| l.name.clone())
                        .unwrap_or_else(|| format!("%t{}", id));
                    let sid = self.intern(&n);
                    let ty_name = self
                        .f
                        .locals
                        .get(*id)
                        .map(|l| l.ty.0.clone())
                        .unwrap_or_default();
                    return self.push_ty(Tag::ReadLocal, sid, *id as u32, 0, &ty_name);
                }
                //nt: 单定义直连
                if let Some(Some(d)) = self.定义.get(*id) {
                    return *d;
                }
                let n = self
                    .f
                    .locals
                    .get(*id)
                    .and_then(|l| l.name.clone())
                    .unwrap_or_else(|| format!("%t{}", id));
                let sid = self.intern(&n);
                let ty_name = self
                    .f
                    .locals
                    .get(*id)
                    .map(|l| l.ty.0.clone())
                    .unwrap_or_default();
                self.push_ty(Tag::ReadLocal, sid, *id as u32, 0, &ty_name)
            }
            Value::Int(i) => self.int_c(*i),
            Value::Float(f) => self.float_c(*f),
            Value::Str(s) => self.str_c(s),
            Value::Bool(b) => self.bool_c(*b),
            Value::Null => self.push(Tag::ConstNull, 0, 0, 0),
        }
    }

    //nt: 登记定义
    fn 定(&mut self, id: LocalId, inst: InstId) {
        if self.定义.len() <= id {
            self.定义.resize(id + 1, None);
        }
        self.定义[id] = Some(inst);
    }

    //nt: 位置转投影读链
    fn place(&mut self, p: &Place) -> InstId {
        let mut cur = self.val(&Value::Local(p.base));
        for pr in &p.proj {
            cur = match pr {
                Proj::Field(n) => {
                    let fid = self.intern(n);
                    self.push(Tag::GetField, cur, fid, 0)
                }
                Proj::Index(i) => {
                    let ic = self.int_c(*i);
                    self.push(Tag::GetIndex, cur, ic, 0)
                }
                Proj::Deref => self.push(Tag::Deref, cur, 0, 0),
            };
        }
        //nt: 链尾记位
        if !p.proj.is_empty() && (cur as usize) < self.places.len() {
            self.places[cur as usize] = Some(p.clone());
        }
        cur
    }

    //nt: 右值转换
    fn rvalue(&mut self, r: &Rvalue) -> InstId {
        match r {
            Rvalue::Use(p) => self.place(p),
            Rvalue::Const(v) => self.val(v),
            Rvalue::BinOp { op, lhs, rhs } => {
                let a = self.val(lhs);
                let b = self.val(rhs);
                let (st, _) = self.push_extra(&[a, b]);
                self.push(Tag::BinOp, Op::from_binop(op) as u32, st, 2)
            }
            Rvalue::Call { name, args } => {
                let nid = self.intern(name);
                let a: Vec<InstId> = args.iter().map(|v| self.val(v)).collect();
                let (st, ln) = self.push_extra(&a);
                self.push(Tag::Call, nid, st, ln)
            }
            Rvalue::FnRef { name } => {
                let nid = self.intern(name);
                self.push(Tag::FnRef, nid, 0, 0)
            }
            Rvalue::CallIndirect { callee, args } => {
                let c = self.val(callee);
                let a: Vec<InstId> = args.iter().map(|v| self.val(v)).collect();
                let (st, ln) = self.push_extra(&a);
                self.push(Tag::CallIndirect, c, st, ln)
            }
            Rvalue::ListLit { elems } => {
                let a: Vec<InstId> = elems.iter().map(|v| self.val(v)).collect();
                let (st, ln) = self.push_extra(&a);
                self.push(Tag::ListLit, st, ln, 0)
            }
            Rvalue::ListGet { list, index } => {
                let l = self.val(list);
                let i = self.val(index);
                self.push(Tag::ListGet, l, i, 0)
            }
            Rvalue::MapLit { pairs } => {
                let mut a: Vec<InstId> = Vec::new();
                for (k, v) in pairs {
                    match k {
                        MapKey::Str(s) => a.push(self.str_c(s)),
                        MapKey::Int(i) => a.push(self.int_c(*i)),
                    }
                    a.push(self.val(v));
                }
                let (st, ln) = self.push_extra(&a);
                self.push(Tag::MapLit, st, ln, 0)
            }
            Rvalue::MapGet { map, key } => {
                let m = self.val(map);
                let k = self.val(key);
                self.push(Tag::MapGet, m, k, 0)
            }
            Rvalue::VariantLit { ty, tag, args, .. } => {
                let tid = self.intern(ty);
                let a: Vec<InstId> = args.iter().map(|v| self.val(v)).collect();
                let (st, ln) = self.push_extra(&a);
                let packed = (tid << 8) | (*tag as u32 & 0xFF);
                self.push(Tag::VariantLit, packed, st, ln)
            }
            Rvalue::VariantTag(v) => {
                let x = self.val(v);
                self.push(Tag::VariantTag, x, 0, 0)
            }
            Rvalue::VariantGet { value, idx, ty } => {
                let x = self.val(value);
                let tid = self.ty_id(ty);
                //nt: 载荷型下标进变长区
                let (st, ln) = self.push_extra(&[tid, *idx as u32]);
                self.push(Tag::VariantGet, x, st, ln)
            }
            Rvalue::Ref(p) => {
                let x = self.place(p);
                let r = self.push(Tag::Ref, x, 0, 0);
                //nt: 记取址位置
                if (r as usize) < self.places.len() {
                    self.places[r as usize] = Some(p.clone());
                }
                r
            }
        }
    }
}

//nt: 单函数转换
fn 转函数(f: &Function) -> FlatFn {
    //nt: 多变局部预扫
    let n = f.locals.len();
    let mut 定义数 = vec![0usize; n];
    let mut 要址 = vec![false; n];
    let mut 块参集: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    for b in &f.blocks {
        for (pid, _) in &b.params {
            块参集.insert(*pid);
        }
        for st in &b.stmts {
            if let Stmt::Assign { lhs, rhs } = st {
                if lhs.base < n {
                    if lhs.proj.is_empty() {
                        定义数[lhs.base] += 1;
                    } else {
                        //nt: 投影写要地址
                        要址[lhs.base] = true;
                    }
                }
                if let Rvalue::Ref(p) = rhs {
                    if p.base < n {
                        要址[p.base] = true;
                    }
                }
            }
        }
    }
    let mut 多变 = vec![false; n];
    for i in 0..n {
        let l = &f.locals[i];
        let 跨块 = l.name.as_deref().map(|x| x.contains('#')).unwrap_or(false);
        多变[i] = 定义数[i] != 1 || 要址[i] || 块参集.contains(&i) || 跨块;
    }
    let mut t = 转中 {
        f,
        定义: Vec::new(),
        多变,
        块参: Vec::new(),
        insts: Vec::new(),
        itys: Vec::new(),
        lines: Vec::new(),
        cols: Vec::new(),
        lens: Vec::new(),
        places: Vec::new(),
        行: 0,
        列: 0,
        宽: 0,
        extra: Vec::new(),
        ints: Vec::new(),
        floats: Vec::new(),
        strings: Vec::new(),
        名池: std::collections::BTreeMap::new(),
    };
    //nt: 先扫块参数
    for b in &f.blocks {
        let mut ps: Vec<InstId> = Vec::new();
        for (lid, _ty) in &b.params {
            let n = f
                .locals
                .get(*lid)
                .and_then(|l| l.name.clone())
                .unwrap_or_default();
            let sid = t.intern(&n);
            ps.push(t.push(Tag::ReadLocal, sid, *lid as u32, 0));
        }
        t.块参.push(ps);
    }
    let mut blocks: Vec<FlatBlock> = Vec::new();
    for b in &f.blocks {
        let start = t.insts.len() as u32;
        //nt: 块参数定义登记
        for (k, (lid, _ty)) in b.params.iter().enumerate() {
            if let Some(&v) = t.块参[b.id as usize].get(k) {
                t.定(*lid, v);
            }
        }
        for (si, st) in b.stmts.iter().enumerate() {
            //nt: 源行列宽随语句
            let sp = b.spans.get(si).copied().unwrap_or_default();
            t.行 = sp.line as u32;
            t.列 = sp.col as u32;
            t.宽 = sp.len as u32;
            转语句(&mut t, st);
        }
        let exit = match &b.exit {
            crate::Exit::Return(v) => FExit::Return(v.as_ref().map(|v| t.val(v))),
            crate::Exit::Branch { cond, targets } => {
                let c = t.val(cond);
                let tg: Vec<(String, Vec<InstId>)> = targets
                    .iter()
                    .map(|s| {
                        let 名 = format!("bb{}", s.target);
                        let a: Vec<InstId> = s.args.iter().map(|v| t.val(v)).collect();
                        (名, a)
                    })
                    .collect();
                FExit::Branch { cond: c, targets: tg }
            }
            crate::Exit::Unreachable => FExit::Unreachable,
        };
        let name = format!("bb{}", b.id);
        blocks.push(FlatBlock {
            name,
            start,
            params: t.块参[b.id as usize].clone(),
            exit,
        });
    }
    FlatFn {
        name: f.name.clone(),
        ret: f.ret.as_ref().map(|x| x.0.clone()),
        params: f
            .locals
            .iter()
            .filter(|l| l.is_param)
            .filter_map(|l| l.name.clone())
            .collect(),
        insts: t.insts,
        itys: t.itys,
        lines: t.lines,
        cols: t.cols,
        lens: t.lens,
        places: t.places,
        extra: t.extra,
        ints: t.ints,
        floats: t.floats,
        strings: t.strings,
        blocks,
    }
}

//nt: 语句转换
fn 转语句(t: &mut 转中, st: &Stmt) {
    match st {
        Stmt::Assign { lhs, rhs } => {
            //nt: 移交源编号加一存b
            let 裸 = match rhs {
                Rvalue::Use(p) if p.proj.is_empty() => Some(p.base),
                Rvalue::Const(Value::Local(u)) => Some(*u),
                _ => None,
            };
            let nb = 裸.map(|u| u as u32 + 1).unwrap_or(0);
            //nt: 串赋值要拷的形态记c
            let nc: u32 = if matches!(rhs, Rvalue::BinOp { .. }) { 0 } else { 1 };
            //nt: 纯值赋：转右值记定义
            if lhs.proj.is_empty() {
                //nt: 裸局部读另立读点
                let v = match 裸 {
                    Some(u) => {
                        let nm = t
                            .f
                            .locals
                            .get(u)
                            .and_then(|l| l.name.clone())
                            .unwrap_or_else(|| format!("%t{}", u));
                        let sid = t.intern(&nm);
                        let ty_name = t
                            .f
                            .locals
                            .get(u)
                            .map(|l| l.ty.0.clone())
                            .unwrap_or_default();
                        t.push_ty(Tag::ReadLocal, sid, u as u32, 0, &ty_name)
                    }
                    None => t.rvalue(rhs),
                };
                //nt: 赋值收口
                let np = t.push(Tag::Nop, v, nb, nc);
                let np = np as usize;
                if np < t.places.len() {
                    t.places[np] = Some(lhs.clone());
                }
                t.定(lhs.base, v);
                return;
            }
            //nt: 投影写
            let mut cur = t.val(&Value::Local(lhs.base));
            let pc = lhs.proj.len();
            for (pi, pr) in lhs.proj.iter().enumerate() {
                let is_last = pi + 1 == pc;
                cur = match pr {
                    Proj::Field(n) => {
                        let fid = t.intern(n);
                        if is_last {
                            t.push(Tag::GetFieldAddr, cur, fid, 0)
                        } else {
                            t.push(Tag::GetField, cur, fid, 0)
                        }
                    }
                    Proj::Index(i) => {
                        let ic = t.int_c(*i);
                        if is_last {
                            t.push(Tag::GetIndexAddr, cur, ic, 0)
                        } else {
                            t.push(Tag::GetIndex, cur, ic, 0)
                        }
                    }
                    Proj::Deref => t.push(Tag::Deref, cur, 0, 0),
                };
            }
            let v = t.rvalue(rhs);
            //nt: 收口同记移交源
            let nop = t.push(Tag::Nop, v, nb, nc);
            let _ = cur;
            //nt: 记录投影写的目标位置
            let np = nop as usize;
            if np < t.places.len() {
                t.places[np] = Some(lhs.clone());
            }
        }
        Stmt::Call { name, args } => {
            let nid = t.intern(name);
            let a: Vec<InstId> = args.iter().map(|v| t.val(v)).collect();
            let (st, ln) = t.push_extra(&a);
            t.push(Tag::Call, nid, st, ln);
            let _ = (st, ln);
        }
        Stmt::Alloc { obj, class: _, ty } => {
            let tid = t.ty_id(ty);
            t.push(Tag::Alloc, *obj, tid, 0);
        }
        Stmt::Bind { var, obj } => {
            t.push(Tag::Bind, *var as u32, *obj, 0);
        }
        Stmt::Drop { obj } => {
            t.push(Tag::Drop, *obj, 0, 0);
        }
        Stmt::DropVar { lid } => {
            t.push(Tag::DropVar, *lid as u32, 0, 0);
        }
        Stmt::Kill { block } => {
            let bid = t.intern(block);
            t.push(Tag::Kill, bid, 0, 0);
        }
        Stmt::ListSet { list, index, value } => {
            let l = t.val(list);
            let i = t.val(index);
            let v = t.val(value);
            let (st, ln) = t.push_extra(&[l, i, v]);
            t.push(Tag::ListSetStmt, st, ln, 0);
        }
        Stmt::MapSet { map, key, value } => {
            let m = t.val(map);
            let k = t.val(key);
            let v = t.val(value);
            let (st, ln) = t.push_extra(&[m, k, v]);
            t.push(Tag::MapSetStmt, st, ln, 0);
        }
        Stmt::AccessBegin(p) => {
            let x = t.place(p);
            t.push(Tag::AccessBegin, x, 0, 0);
        }
        Stmt::AccessEnd(p) => {
            let x = t.place(p);
            t.push(Tag::AccessEnd, x, 0, 0);
        }
        Stmt::Assert { cond, msg, fix } => {
            let c = t.val(cond);
            //nt: 文与修文都入变长区
            let mid = t.intern(msg);
            let fid = t.intern(fix);
            let (st, ln) = t.push_extra(&[mid, fid]);
            t.push(Tag::Assert, c, st, ln);
        }
        Stmt::Asm(form) => {
            //nt: 汇编形串化
            let mut s = String::new();
            s.push_str(&form.tmpl);
            s.push('\u{1d}');
            for (c, id, rw) in &form.outs {
                s.push_str(c);
                s.push('\u{1f}');
                s.push_str(&id.to_string());
                s.push('\u{1f}');
                s.push_str(if *rw { "1" } else { "0" });
                s.push('\u{1e}');
            }
            s.push('\u{1d}');
            for (c, v) in &form.ins {
                s.push_str(c);
                s.push('\u{1f}');
                值串(v, &mut s);
                s.push('\u{1e}');
            }
            s.push('\u{1d}');
            for cl in &form.clobbers {
                s.push_str(cl);
                s.push('\u{1e}');
            }
            let sid = t.intern(&s);
            t.push(Tag::Asm, sid, 0, 0);
        }
    }
}

//nt: 汇编形串解回
pub fn 解汇编(blob: &str) -> Option<crate::AsmForm> {
    let mut it = blob.split('\u{1d}');
    let tmpl = it.next()?.to_string();
    let outs_s = it.next().unwrap_or("");
    let ins_s = it.next().unwrap_or("");
    let clob_s = it.next().unwrap_or("");
    let mut outs = Vec::new();
    for e in outs_s.split('\u{1e}') {
        if e.is_empty() {
            continue;
        }
        let mut f = e.split('\u{1f}');
        let c = f.next()?.to_string();
        let id: usize = f.next()?.parse().ok()?;
        let rw = f.next()? == "1";
        outs.push((c, id, rw));
    }
    let mut ins = Vec::new();
    for e in ins_s.split('\u{1e}') {
        if e.is_empty() {
            continue;
        }
        let mut f = e.split('\u{1f}');
        let c = f.next()?.to_string();
        let v = 值解(f.next()?)?;
        ins.push((c, v));
    }
    let clobbers: Vec<String> = clob_s
        .split('\u{1e}')
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect();
    Some(crate::AsmForm { tmpl, outs, ins, clobbers })
}

//nt: 值从汇编形串解回
fn 值解(s: &str) -> Option<Value> {
    let mut ch = s.chars();
    let h = ch.next()?;
    let r = ch.as_str();
    Some(match h {
        'L' => Value::Local(r.parse().ok()?),
        'I' => Value::Int(r.parse().ok()?),
        'F' => Value::Float(f64::from_bits(u64::from_str_radix(r, 16).ok()?)),
        'S' => {
            let (n, body) = r.split_once(':')?;
            let n: usize = n.parse().ok()?;
            let b = body.as_bytes();
            if b.len() < n {
                return None;
            }
            Value::Str(String::from_utf8_lossy(&b[..n]).into_owned())
        }
        'B' => Value::Bool(r == "1"),
        'N' => Value::Null,
        _ => return None,
    })
}

//nt: 值编码进汇编形串
fn 值串(v: &Value, s: &mut String) {

    match v {
        Value::Local(id) => {
            s.push('L');
            s.push_str(&id.to_string());
        }
        Value::Int(i) => {
            s.push('I');
            s.push_str(&i.to_string());
        }
        Value::Float(f) => {
            s.push('F');
            s.push_str(&format!("{:x}", f.to_bits()));
        }
        Value::Str(x) => {
            s.push('S');
            s.push_str(&x.len().to_string());
            s.push(':');
            s.push_str(x);
        }
        Value::Bool(b) => {
            s.push('B');
            s.push_str(if *b { "1" } else { "0" });
        }
        Value::Null => s.push('N'),
    }
}

//nt: 全程序转换
pub fn from_program(p: &Program) -> Flat {
    let mut out = Flat::default();
    for f in &p.functions {
        out.fns.push(转函数(f));
    }
    out
}

//nt: 扁平体转回树形
pub fn to_program(flat: &Flat, src: &Program) -> Program {
    let mut out = src.clone();
    for (i, ffn) in flat.fns.iter().enumerate() {
        if let Some(f) = out.functions.get_mut(i) {
            还原函数(f, ffn, src);
        }
    }
    out
}

//nt: 单函数还原
fn 还原函数(f: &mut Function, ffn: &FlatFn, src: &Program) {
    //nt: 纯赋值定义表
    let n = f.locals.len();
    let mut defs: Vec<Option<u32>> = vec![None; n];
    for (iid, inst) in ffn.insts.iter().enumerate() {
        if inst.tag != Tag::Nop {
            continue;
        }
        if let Some(Some(pl)) = ffn.places.get(iid) {
            if pl.proj.is_empty() && pl.base < n {
                defs[pl.base] = Some(inst.a);
            }
        }
    }
    //nt: 被引用的右值顶指令
    let mut 被引: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for inst in &ffn.insts {
        if inst.tag == Tag::Nop {
            被引.insert(inst.a);
        }
        let 载荷: Vec<u32> = match inst.tag {
            Tag::BinOp => (0..2).filter_map(|k| ffn.extra.get(inst.b as usize + k).copied()).collect(),
            Tag::Call | Tag::CallIndirect | Tag::VariantLit | Tag::Assert => (0..inst.c)
                .filter_map(|k| ffn.extra.get(inst.b as usize + k as usize).copied())
                .collect(),
            Tag::ListLit | Tag::MapLit | Tag::ListSetStmt | Tag::MapSetStmt => (0..inst.b)
                .filter_map(|k| ffn.extra.get(inst.a as usize + k as usize).copied())
                .collect(),
            Tag::VariantGet => (0..2).filter_map(|k| ffn.extra.get(inst.b as usize + k).copied()).collect(),
            _ => Vec::new(),
        };
        被引.extend(载荷);
    }
    for (bi, fb) in ffn.blocks.iter().enumerate() {
        let Some(b) = f.blocks.get_mut(bi) else { continue };
        let 起 = fb.start as usize;
        let 止 = ffn
            .blocks
            .get(bi + 1)
            .map(|x| x.start as usize)
            .unwrap_or(ffn.insts.len());
        let mut stmts: Vec<Stmt> = Vec::new();
        let mut spans: Vec<Span> = Vec::new();
        for iid in 起..止 {
            let 行 = ffn.lines.get(iid).copied().unwrap_or(0) as usize;
            let 列 = ffn.cols.get(iid).copied().unwrap_or(0) as usize;
            let 宽 = ffn.lens.get(iid).copied().unwrap_or(0) as usize;
            let st = 还原语句(ffn, iid, &defs, &被引, src);
            if let Some(st) = st {
                stmts.push(st);
                spans.push(Span { line: 行, col: 列, len: 宽 });
            }
        }
        b.stmts = stmts;
        b.spans = spans;
    }
}

//nt: 单语句还原
fn 还原语句(
    ffn: &FlatFn,
    iid: usize,
    defs: &[Option<u32>],
    被引: &std::collections::BTreeSet<u32>,
    src: &Program,
) -> Option<Stmt> {
    let inst = ffn.insts[iid];
    Some(match inst.tag {
        Tag::Nop => {
            let lhs = ffn.places.get(iid).and_then(|x| x.clone())?;
            let rhs = if inst.b > 0 {
                //nt: 裸局部或转交源
                Rvalue::Use(Place::local((inst.b - 1) as usize))
            } else {
                右值还原(ffn, inst.a, defs, src)?
            };
            Stmt::Assign { lhs, rhs }
        }
        Tag::Call if !被引.contains(&(iid as u32)) => {
            let name = ffn.strings.get(inst.a as usize).cloned()?;
            let args: Vec<Value> = (0..inst.c)
                .filter_map(|k| ffn.extra.get(inst.b as usize + k as usize).copied())
                .map(|id| 值还原(ffn, id, defs))
                .collect::<Option<Vec<_>>>()?;
            Stmt::Call { name, args }
        }
        Tag::ListSetStmt => {
            let lid = ffn.extra.get(inst.a as usize).copied()?;
            let vid = ffn.extra.get(inst.a as usize + 1).copied()?;
            let vvid = ffn.extra.get(inst.a as usize + 2).copied()?;
            Stmt::ListSet {
                list: 值还原(ffn, lid, defs)?,
                index: 值还原(ffn, vid, defs)?,
                value: 值还原(ffn, vvid, defs)?,
            }
        }
        Tag::MapSetStmt => {
            let mid = ffn.extra.get(inst.a as usize).copied()?;
            let kid = ffn.extra.get(inst.a as usize + 1).copied()?;
            let vid = ffn.extra.get(inst.a as usize + 2).copied()?;
            Stmt::MapSet {
                map: 值还原(ffn, mid, defs)?,
                key: 值还原(ffn, kid, defs)?,
                value: 值还原(ffn, vid, defs)?,
            }
        }
        Tag::Drop => Stmt::Drop { obj: inst.a },
        Tag::DropVar => Stmt::DropVar {
            lid: inst.a as usize,
        },
        Tag::Kill => Stmt::Kill {
            block: ffn.strings.get(inst.a as usize).cloned()?,
        },
        Tag::Assert => {
            let mid = ffn.extra.get(inst.b as usize).copied()?;
            let fid = ffn.extra.get(inst.b as usize + 1).copied()?;
            Stmt::Assert {
                cond: 值还原(ffn, inst.a, defs)?,
                msg: ffn.strings.get(mid as usize).cloned()?,
                fix: ffn.strings.get(fid as usize).cloned()?,
            }
        }
        Tag::Asm => Stmt::Asm(解汇编(ffn.strings.get(inst.a as usize)?)?),
        Tag::Alloc => Stmt::Alloc {
            obj: inst.a,
            class: crate::StorageClass::Tmp,
            ty: Ty(ffn.strings.get(inst.b as usize).cloned()?),
        },
        Tag::Bind => Stmt::Bind {
            var: inst.a as usize,
            obj: inst.b,
        },
        _ => return None,
    })
}

//nt: 指令转值
fn 值还原(ffn: &FlatFn, id: u32, defs: &[Option<u32>]) -> Option<Value> {
    //nt: 定义指令按所属局部
    if let Some(lid) = defs.iter().position(|d| *d == Some(id)) {
        return Some(Value::Local(lid));
    }
    let inst = ffn.insts.get(id as usize).copied()?;
    Some(match inst.tag {
        Tag::ConstInt => Value::Int(ffn.ints.get(inst.a as usize).copied().unwrap_or(0)),
        Tag::ConstFloat => Value::Float(ffn.floats.get(inst.a as usize).copied().unwrap_or(0.0)),
        Tag::ConstStr => Value::Str(ffn.strings.get(inst.a as usize).cloned().unwrap_or_default()),
        Tag::ConstBool => Value::Bool(inst.a != 0),
        Tag::ConstNull => Value::Null,
        Tag::ReadLocal => Value::Local(inst.b as usize),
        _ => return None,
    })
}

//nt: 指令转右值
fn 右值还原(ffn: &FlatFn, id: u32, defs: &[Option<u32>], src: &Program) -> Option<Rvalue> {
    let inst = ffn.insts.get(id as usize).copied()?;
    Some(match inst.tag {
        Tag::ReadLocal => Rvalue::Use(Place::local(inst.b as usize)),
        Tag::ConstInt => Rvalue::Const(Value::Int(ffn.ints.get(inst.a as usize).copied().unwrap_or(0))),
        Tag::ConstFloat => Rvalue::Const(Value::Float(ffn.floats.get(inst.a as usize).copied().unwrap_or(0.0))),
        Tag::ConstStr => Rvalue::Const(Value::Str(ffn.strings.get(inst.a as usize).cloned().unwrap_or_default())),
        Tag::ConstBool => Rvalue::Const(Value::Bool(inst.a != 0)),
        Tag::ConstNull => Rvalue::Const(Value::Null),
        //nt: 链尾读按记位还原
        Tag::GetField | Tag::GetIndex | Tag::Deref => {
            Rvalue::Use(ffn.places.get(id as usize).and_then(|x| x.clone())?)
        }
        Tag::BinOp => Rvalue::BinOp {
            op: Op::from_u32(inst.a)?.to_binop(),
            lhs: 值还原(ffn, ffn.extra.get(inst.b as usize).copied()?, defs)?,
            rhs: 值还原(ffn, ffn.extra.get(inst.b as usize + 1).copied()?, defs)?,
        },
        Tag::Call => Rvalue::Call {
            name: ffn.strings.get(inst.a as usize).cloned()?,
            args: (0..inst.c)
                .filter_map(|k| ffn.extra.get(inst.b as usize + k as usize).copied())
                .map(|x| 值还原(ffn, x, defs))
                .collect::<Option<Vec<_>>>()?,
        },
        Tag::FnRef => Rvalue::FnRef {
            name: ffn.strings.get(inst.a as usize).cloned()?,
        },
        Tag::CallIndirect => Rvalue::CallIndirect {
            callee: 值还原(ffn, inst.a, defs)?,
            args: (0..inst.c)
                .filter_map(|k| ffn.extra.get(inst.b as usize + k as usize).copied())
                .map(|x| 值还原(ffn, x, defs))
                .collect::<Option<Vec<_>>>()?,
        },
        Tag::ListLit => Rvalue::ListLit {
            elems: (0..inst.b)
                .filter_map(|k| ffn.extra.get(inst.a as usize + k as usize).copied())
                .map(|x| 值还原(ffn, x, defs))
                .collect::<Option<Vec<_>>>()?,
        },
        Tag::ListGet => Rvalue::ListGet {
            list: 值还原(ffn, inst.a, defs)?,
            index: 值还原(ffn, inst.b, defs)?,
        },
        Tag::MapLit => {
            let mut pairs: Vec<(MapKey, Value)> = Vec::new();
            let mut k = 0usize;
            while k + 1 < inst.b as usize {
                let kid = ffn.extra.get(inst.a as usize + k).copied()?;
                let vid = ffn.extra.get(inst.a as usize + k + 1).copied()?;
                let kinst = ffn.insts.get(kid as usize).copied()?;
                let key = match kinst.tag {
                    Tag::ConstStr => MapKey::Str(
                        ffn.strings.get(kinst.a as usize).cloned().unwrap_or_default(),
                    ),
                    Tag::ConstInt => MapKey::Int(ffn.ints.get(kinst.a as usize).copied().unwrap_or(0)),
                    _ => return None,
                };
                pairs.push((key, 值还原(ffn, vid, defs)?));
                k += 2;
            }
            Rvalue::MapLit { pairs }
        }
        Tag::MapGet => Rvalue::MapGet {
            map: 值还原(ffn, inst.a, defs)?,
            key: 值还原(ffn, inst.b, defs)?,
        },
        Tag::VariantLit => {
            let tyname = ffn.strings.get((inst.a >> 8) as usize).cloned()?;
            let tag = (inst.a & 0xFF) as usize;
            let args: Vec<Value> = (0..inst.c)
                .filter_map(|k| ffn.extra.get(inst.b as usize + k as usize).copied())
                .map(|x| 值还原(ffn, x, defs))
                .collect::<Option<Vec<_>>>()?;
            let tys: Vec<Ty> = src
                .types
                .iter()
                .find(|st| st.name == tyname)
                .and_then(|st| st.variants.get(tag))
                .map(|v| v.fields.iter().map(|(_, t)| t.clone()).collect())?;
            Rvalue::VariantLit { ty: tyname, tag, args, tys }
        }
        Tag::VariantTag => Rvalue::VariantTag(值还原(ffn, inst.a, defs)?),
        Tag::VariantGet => Rvalue::VariantGet {
            value: 值还原(ffn, inst.a, defs)?,
            idx: ffn.extra.get(inst.b as usize + 1).copied()? as usize,
            ty: Ty(
                ffn.strings
                    .get(ffn.extra.get(inst.b as usize).copied()? as usize)
                    .cloned()?,
            ),
        },
        Tag::Ref => Rvalue::Ref(ffn.places.get(id as usize).and_then(|x| x.clone())?),
        _ => return None,
    })
}

//nt: 文本化
pub fn dump(flat: &Flat) -> String {
    let mut s = String::new();
    for f in &flat.fns {
        s.push_str(&format!("fn {}(", f.name));
        s.push_str(&f.params.join(", "));
        s.push_str(")");
        if let Some(r) = &f.ret {
            s.push_str(&format!(" -> {}", r));
        }
        s.push('\n');
        for (bi, b) in f.blocks.iter().enumerate() {
            s.push_str(&format!("  {}:", b.name));
            if !b.params.is_empty() {
                let ps: Vec<String> = b.params.iter().map(|p| format!("%{}", p)).collect();
                s.push_str(&format!("({})", ps.join(", ")));
            }
            s.push('\n');
            //nt: 块指令区间
            let end = f
                .blocks
                .get(bi + 1)
                .map(|nb| nb.start as usize)
                .unwrap_or(f.insts.len());
            for i in b.start as usize..end {
                let inst = &f.insts[i];
                s.push_str(&format!("    {} {} {}\n", inst.tag as u32, inst.a, inst.b));
            }
            match &b.exit {
                FExit::Return(Some(v)) => s.push_str(&format!("    ret %{}\n", v)),
                FExit::Return(None) => s.push_str("    ret\n"),
                FExit::Branch { cond, targets } => {
                    let ts: Vec<String> = targets
                        .iter()
                        .map(|(n, args)| {
                            let a: Vec<String> = args.iter().map(|x| format!("%{}", x)).collect();
                            format!("{}({})", n, a.join(", "))
                        })
                        .collect();
                    s.push_str(&format!("    br %{} {}\n", cond, ts.join(" ")));
                }
                FExit::Unreachable => s.push_str("    unreachable\n"),
            }
        }
    }
    s
}
