//! anle-ir —— 中间表示
//!
//! 结构：程序 = 若干函数；函数 = 局部变量表（带类型）+ 若干基本块；
//! 基本块 = 块参数（可 0 个）+ 一串语句 + 恰一条出口指令。
//!
//! - 汇合用块参数，不用 φ 节点
//! - 位置（Place）= 局部变量 + 投影链（`.字段` / `[下标]` / 解引用），读写以位置为单位
//! - 控制流已摊平：if / while / match 全部坍缩为分支指令 + 基本块跳转
//! - 指令集：
//!   赋值 / 存活开始·存活结束 / 分支 / 调用·返回·断言·不可达 / 释放；
//!   进入访问期·结束访问期 / 拷贝值·销毁值；
//!   分配（存储类）—— tmp / keep / unsafe 标在分配上
//!
//! 不含：SSA / φ、借用检查指令、协程指令、独立文本格式（IR 活在内存，文本仅供 dump）。
//! dump 规范：英文指令名、源名保留 + 临时 `%tN`、块 `bbN`（出现序）、类型用源名、
//! 块参数显式、2 空格缩进、行号默认不带、顺序不排序。

use anle_error::Span;
use std::fmt;

//nt: AST
pub mod build;

//nt: 编译期求值
pub mod consts;

pub use build::{build, BuildError};

pub type LocalId = usize;
pub type BlockId = usize;

// nt: 存储类来自 types
pub use anle_types::StorageClass;

//nt: 类型名
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ty(pub String);

//nt: 位置上的投影
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proj {
    Field(String),
    Index(i64),
    Deref,
}

//nt: 可寻址位置
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub base: LocalId,
    pub proj: Vec<Proj>,
}

impl Place {
    pub fn local(id: LocalId) -> Self {
        Self { base: id, proj: Vec::new() }
    }
}

//nt: 可取值
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Local(LocalId),
    Int(i64),
    Float(f64),
    Str(String),
    //nt: 真假值
    Bool(bool),
    Null,
}

//nt: map 字面量的键
#[derive(Debug, Clone, PartialEq)]
pub enum MapKey {
    Str(String),
    Int(i64),
}

//nt: 右值表达式
#[derive(Debug, Clone, PartialEq)]
pub enum Rvalue {
    Use(Place),
    Const(Value),
    BinOp { op: BinOp, lhs: Value, rhs: Value },
    //nt: 分配
    Alloc { class: StorageClass, ty: Ty },
    Call { name: String, args: Vec<Value> },
    //nt: 函数值
    FnRef { name: String },
    //nt: 间接调用
    CallIndirect { callee: Value, args: Vec<Value> },
    //nt: 列表字面量 `[1
    ListLit { elems: Vec<Value> },
    //nt: 下标读 `列表[i]
    ListGet { list: Value, index: Value },
    //nt: map 字面量 `{
    //nt: 键收str与int
    MapLit { pairs: Vec<(MapKey, Value)> },
    //nt: map 按键读
    MapGet { map: Value, key: Value },
    //nt: 和类型构造
    VariantLit { ty: String, tag: usize, args: Vec<Value>, tys: Vec<Ty> },
    //nt: 和类型判别：取tag
    VariantTag(Value),
    //nt: 和类型解构
    VariantGet { value: Value, idx: usize, ty: Ty },
    //nt: 取地址 `&位置
    Ref(Place),
}

//nt: 内联 asm 三段式
#[derive(Debug, Clone, PartialEq)]
pub struct AsmForm {
    pub tmpl: String,
    //nt: 输出
    pub outs: Vec<(String, LocalId, bool)>,
    //nt: 输入
    pub ins: Vec<(String, Value)>,
    //nt: 破坏段
    pub clobbers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
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
    //nt: 按位与 / 或 /
    BitAnd,
    BitOr,
    BitXor,
    //nt: 左移 / 右移
    Shl,
    Shr,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "add",
            BinOp::Sub => "sub",
            BinOp::Mul => "mul",
            BinOp::Div => "div",
            BinOp::Mod => "mod",
            BinOp::Eq => "eq",
            BinOp::Ne => "ne",
            BinOp::Lt => "lt",
            BinOp::Le => "le",
            BinOp::Gt => "gt",
            BinOp::Ge => "ge",
            BinOp::And => "and",
            BinOp::Or => "or",
            BinOp::BitAnd => "band",
            BinOp::BitOr => "bor",
            BinOp::BitXor => "bxor",
            BinOp::Shl => "shl",
            BinOp::Shr => "shr",
        }
    }
}

//nt: 语句串
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Assign { lhs: Place, rhs: Rvalue },
    //nt: 无返回值的调用语句
    Call { name: String, args: Vec<Value> },
    Live(Place),
    Dead(Place),
    //nt: 下标写 `列表[i]
    ListSet { list: Value, index: Value, value: Value },
    //nt: map 按键写
    MapSet { map: Value, key: Value, value: Value },
    AccessBegin(Place),
    AccessEnd(Place),
    Copy { from: Place, to: Place },
    Destroy(Place),
    Release(Place),
    //nt: 显式提前回收
    KillRegion { block: String },
    //nt: 护栏
    Assert { cond: Value, msg: String, fix: String },
    //nt: 内联 asm
    Asm(AsmForm),
}

//nt: 出口指令
#[derive(Debug, Clone, PartialEq)]
pub enum Exit {
    //nt: 统一条件跳转
    Branch { cond: Value, targets: Vec<BlockId> },
    Return(Option<Value>),
    Unreachable,
}

//nt: 基本块
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub id: BlockId,
    //nt: 块参数
    pub params: Vec<(LocalId, Ty)>,
    pub stmts: Vec<Stmt>,
    //nt: 与 `stmts
    pub spans: Vec<Span>,
    pub exit: Exit,
}

//nt: 局部变量
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local {
    //nt: 源名
    pub name: Option<String>,
    pub ty: Ty,
    pub is_param: bool,
    pub bare_ok: bool,
    //nt: 声明行列
    pub line: usize,
    pub col: usize,
    //nt: 循环体内声明
    pub loop_local: bool,
    //nt: 跨圈逃逸需护
    pub loop_escape: bool,
    //nt: fn内构造的聚合对象
    pub fn_owned: bool,
}

//nt: 函数
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    //nt: 源文件
    pub file: String,
    pub ret: Option<Ty>,
    pub locals: Vec<Local>,
    pub blocks: Vec<Block>,
    //nt: 是否**代码块**函
    pub is_block: bool,
    //nt: 块行属性 `pub
    pub is_pub: bool,
    //nt: 块行属性 `sle
    pub is_sle: bool,
    //nt: 关块行后缀 `d k
    pub kills_own: bool,
    //nt: 块行属性 `ABI
    pub is_abi: bool,
    //nt: 块行属性 `asm
    pub is_naked: bool,
    //nt: naked 函数的汇
    pub asm_lines: Vec<String>,
}

//nt: 积类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructType {
    pub name: String,
    //nt: 泛型类型参数名
    pub params: Vec<String>,
    pub fields: Vec<(String, Ty)>,
    //nt: 和类型变体
    pub variants: Vec<Variant>,
}

//nt: 和类型的一个变体
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
}

//nt: 外部函数声明
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternFn {
    //nt: C 侧符号名 = 声
    pub name: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
}

//nt: 程序
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub functions: Vec<Function>,
    //nt: 全程序的积类型定义
    pub types: Vec<StructType>,
    //nt: 外部函数声明
    pub externs: Vec<ExternFn>,
}

//nt: dump

//nt: 局部名显示
fn local_name(f: &Function, id: LocalId) -> String {
    match f.locals.get(id).and_then(|l| l.name.clone()) {
        Some(n) => n,
        None => {
            //nt: 临时编号 = 该局部
            let mut k = 0usize;
            for (i, l) in f.locals.iter().enumerate() {
                if l.name.is_none() {
                    if i == id {
                        return format!("%t{}", k);
                    }
                    k += 1;
                }
            }
            format!("%t{}", id)
        }
    }
}

fn place_str(f: &Function, p: &Place) -> String {
    let mut s = local_name(f, p.base);
    for pr in &p.proj {
        s.push_str(&match pr {
            Proj::Field(n) => format!(".{}", n),
            Proj::Index(i) => format!("[{}]", i),
            Proj::Deref => "*".to_string(),
        });
    }
    s
}

//nt: map 键显示
fn map_key_str(k: &MapKey) -> String {
    match k {
        MapKey::Str(s) => format!("\"{}\"", s),
        MapKey::Int(i) => i.to_string(),
    }
}

fn value_str(f: &Function, v: &Value) -> String {
    match v {
        Value::Local(id) => local_name(f, *id),
        Value::Int(i) => i.to_string(),
        Value::Float(x) => format!("{}", x),
        Value::Str(s) => format!("\"{}\"", escape_str(s)),
        Value::Bool(b) => (if *b { "re" } else { "fa" }).to_string(),
        Value::Null => "null".to_string(),
    }
}

fn escape_str(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            _ => o.push(c),
        }
    }
    o
}

fn rvalue_str(f: &Function, r: &Rvalue) -> String {
    match r {
        Rvalue::Use(p) => place_str(f, p),
        Rvalue::Ref(p) => format!("ref({})", place_str(f, p)),
        Rvalue::VariantLit { ty, tag, args, .. } => format!(
            "variant {}({}; {})",
            ty,
            tag,
            args.iter().map(|a| value_str(f, a)).collect::<Vec<_>>().join(", ")
        ),
        Rvalue::VariantTag(v) => format!("vtag({})", value_str(f, v)),
        Rvalue::VariantGet { value, idx, .. } => format!("vget({}, {})", value_str(f, value), idx),
        Rvalue::Const(v) => value_str(f, v),
        Rvalue::BinOp { op, lhs, rhs } => {
            format!("{}({}, {})", op.as_str(), value_str(f, lhs), value_str(f, rhs))
        }
        Rvalue::Alloc { class, .. } => format!("alloc({})", class.as_str()),
        Rvalue::Call { name, args } => format!(
            "call {}({})",
            name,
            args.iter().map(|a| value_str(f, a)).collect::<Vec<_>>().join(", ")
        ),
        Rvalue::FnRef { name } => format!("fn_ref({})", name),
        Rvalue::CallIndirect { callee, args } => format!(
            "call_indirect({}; {})",
            value_str(f, callee),
            args.iter().map(|a| value_str(f, a)).collect::<Vec<_>>().join(", ")
        ),
        Rvalue::ListLit { elems } => format!(
            "list_lit({})",
            elems.iter().map(|e| value_str(f, e)).collect::<Vec<_>>().join(", ")
        ),
        Rvalue::ListGet { list, index } => {
            format!("list_get({}, {})", value_str(f, list), value_str(f, index))
        }
        Rvalue::MapLit { pairs } => format!(
            "map_lit({})",
            pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", map_key_str(k), value_str(f, v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Rvalue::MapGet { map, key } => {
            format!("map_get({}, {})", value_str(f, map), value_str(f, key))
        }
    }
}

fn stmt_str(f: &Function, s: &Stmt) -> String {
    match s {
        Stmt::Assign { lhs, rhs } => format!("assign {}, {}", place_str(f, lhs), rvalue_str(f, rhs)),
        Stmt::Call { name, args } => format!(
            "call {}({})",
            name,
            args.iter().map(|a| value_str(f, a)).collect::<Vec<_>>().join(", ")
        ),
        Stmt::Live(p) => format!("live {}", place_str(f, p)),
        Stmt::ListSet { list, index, value } => format!(
            "list_set({}, {}, {})",
            value_str(f, list),
            value_str(f, index),
            value_str(f, value)
        ),
        Stmt::MapSet { map, key, value } => format!(
            "map_set({}, {}, {})",
            value_str(f, map),
            value_str(f, key),
            value_str(f, value)
        ),
        Stmt::Dead(p) => format!("dead {}", place_str(f, p)),
        Stmt::AccessBegin(p) => format!("access_begin {}", place_str(f, p)),
        Stmt::AccessEnd(p) => format!("access_end {}", place_str(f, p)),
        Stmt::Copy { from, to } => format!("copy {} -> {}", place_str(f, from), place_str(f, to)),
        Stmt::Destroy(p) => format!("destroy {}", place_str(f, p)),
        Stmt::Release(p) => format!("release {}", place_str(f, p)),
        Stmt::KillRegion { block } => format!("kill #:{}", block),
        Stmt::Assert { cond, msg, fix } => {
            format!("assert {}, \"{}\", \"{}\"", value_str(f, cond), escape_str(msg), escape_str(fix))
        }
        Stmt::Asm(form) => format!("asm({})", form.tmpl),
    }
}

fn exit_str(f: &Function, e: &Exit) -> String {
    match e {
        //nt: 单目标 = 无条件跳
        Exit::Branch { cond, targets } if targets.len() == 1 => format!("goto bb{}", targets[0]),
        Exit::Branch { cond, targets } => format!(
            "branch {} -> {}",
            value_str(f, cond),
            targets.iter().map(|t| format!("bb{}", t)).collect::<Vec<_>>().join(", ")
        ),
        Exit::Return(Some(v)) => format!("ret {}", value_str(f, v)),
        Exit::Return(None) => "ret".to_string(),
        Exit::Unreachable => "unreachable".to_string(),
    }
}

impl fmt::Display for Program {
    //nt: 归一化 dump
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for t in &self.types {
            let fs: Vec<String> = t
                .fields
                .iter()
                .map(|(n, ty)| format!("{}:{}", n, ty.0))
                .collect();
            writeln!(f, "type {}({})", t.name, fs.join(", "))?;
        }
        if !self.types.is_empty() {
            writeln!(f)?;
        }
        for e in &self.externs {
            let ps: Vec<String> = e.params.iter().map(|t| t.0.clone()).collect();
            writeln!(f, "outs fn {}({}) -> {}", e.name, ps.join(", "), e.ret.0)?;
        }
        if !self.externs.is_empty() {
            writeln!(f)?;
        }
        for (fi, fun) in self.functions.iter().enumerate() {
            if fi > 0 {
                writeln!(f)?;
            }            let params: Vec<String> = fun
                .locals
                .iter()
                .enumerate()
                .filter(|(_, l)| l.is_param)
                .map(|(i, l)| format!("{}:{}", local_name(fun, i), l.ty.0))
                .collect();
            match &fun.ret {
                Some(t) => writeln!(f, "fn {}({}) -> {}", fun.name, params.join(", "), t.0)?,
                None => writeln!(f, "fn {}({})", fun.name, params.join(", "))?,
            }
            let locals: Vec<String> = fun
                .locals
                .iter()
                .enumerate()
                .filter(|(_, l)| !l.is_param)
                .map(|(i, l)| format!("{}:{}", local_name(fun, i), l.ty.0))
                .collect();
            if !locals.is_empty() {
                writeln!(f, "  locals: {}", locals.join(", "))?;
            }
            for b in &fun.blocks {
                let ps: Vec<String> = b
                    .params
                    .iter()
                    .map(|(id, t)| format!("{}:{}", local_name(fun, *id), t.0))
                    .collect();
                if ps.is_empty() {
                    writeln!(f, "  bb{}:", b.id)?;
                } else {
                    writeln!(f, "  bb{}({}):", b.id, ps.join(", "))?;
                }
                for s in &b.stmts {
                    writeln!(f, "    {}", stmt_str(fun, s))?;
                }
                writeln!(f, "    {}", exit_str(fun, &b.exit))?;
            }
        }
        Ok(())
    }
}

impl Program {
    //nt: 归一化 dump 文
    pub fn dump(&self) -> String {
        format!("{}", self)
    }
}