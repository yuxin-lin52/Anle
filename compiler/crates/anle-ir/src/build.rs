//! AST → 中间表示的构建器
//!
//! 层级语法：结构行 `:` 开体、独占行 `;` 收体、`else:` 紧跟 `;`。
//!
//! 处理：块 → 函数；`add val` 声明 / `val "名" =` 改值 / `print(...)`；
//! `if`（含 `else`）/ `while` / `for … in …` / `match` + `case`；
//! 表达式优先级：算术（`* / %` 与 `+ -`）→ 比较 → `and` → `or`；
//! 结构体字段读写；跨模块可见性门禁（`pub` / `hide`）。
//!
//! 容器字面量与 `pop` / `sort` 遇到即报错。

use anle_error::Span;
use anle_lexer::Token;
use anle_parser::{Block as AstBlock, Program as AstProgram, Stmt as AstStmt, TokStream};
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    AsmForm, BinOp, Block, BlockId, ExternFn, Exit, Function, Local, LocalId, MapKey, Place, Proj,
    Program, Rvalue, Stmt, StructType, Ty, Value, Variant,
};

//nt: 构建错误
#[derive(Debug, Clone, PartialEq)]
pub struct BuildError {
    pub message: String,
    pub code: String,
    //nt: 源行号
    pub line: usize,
    //nt: 错误所在的**源文件
    //nt: 渲染时据此取该文件的
    pub file: String,
}

thread_local! {
    //nt: 当前处理的源行号
    static CUR_LINE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    //nt: 泛型类型声明表
    static GENERICS: std::cell::RefCell<BTreeMap<String, Vec<String>>> =
        std::cell::RefCell::new(BTreeMap::new());
    //nt: 泛型实例待建清单
    static INST_TODO: std::cell::RefCell<Vec<(String, Vec<String>)>> =
        std::cell::RefCell::new(Vec::new());
    //nt: 常量 fn 表
    #[allow(non_upper_case_globals)]
    static 常量fn表: std::cell::RefCell<BTreeMap<String, (Vec<String>, Vec<Token>)>> =
        std::cell::RefCell::new(BTreeMap::new());
    //nt: 泛型 fn 模板
    #[allow(non_upper_case_globals)]
    static 泛型fn表: std::cell::RefCell<
        BTreeMap<String, (Vec<(String, Vec<String>)>, Vec<(String, Ty)>, Ty, Vec<Token>, String, String)>,
    > =
        std::cell::RefCell::new(BTreeMap::new());
    //nt: 泛型语句模板
    #[allow(non_upper_case_globals)]
    static 泛型语句体fn表: std::cell::RefCell<
        BTreeMap<String, (Vec<(String, Vec<String>)>, Vec<(String, Ty)>, Ty, Vec<AstStmt>, String, String)>,
    > =
        std::cell::RefCell::new(BTreeMap::new());
    //nt: fn引用的本块val
    #[allow(non_upper_case_globals)]
    static fn_used表: std::cell::RefCell<BTreeMap<String, BTreeMap<String, Ty>>> =
        std::cell::RefCell::new(BTreeMap::new());
    //nt: 类型参数代换
    #[allow(non_upper_case_globals)]
    static 类型代换表: std::cell::RefCell<Option<(Vec<String>, Vec<String>)>> =
        const { std::cell::RefCell::new(None) };
}

//nt: add fix "名
fn 常量名显示(toks: &[Token]) -> String {
    match toks.get(2) {
        Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
        _ => "?".to_string(),
    }
}

//nt: 某具体类型支持某能力
fn 类型能力(型: &str, op: &str) -> bool {
    let 算术 = ["+", "-", "*", "/", "%"];
    let 比较 = ["<", "<=", ">", ">="];
    let 相等 = ["==", "!="];
    match 型 {
        "int" | "float" | "int_8" | "int_16" | "int_32" | "int_64" | "uint_8" | "uint_16"
        | "uint_32" | "uint_64" => {
            算术.contains(&op) || 比较.contains(&op) || 相等.contains(&op)
        }
        "str" => 比较.contains(&op) || 相等.contains(&op),
        "bool" => 相等.contains(&op),
        //nt: 自定义类型
        _ => 相等.contains(&op),
    }
}

//nt: 逃逸复印分类
enum 复印 {
    无需,
    拷贝(String, i64),
    无法,
}

//nt: 该型是否沾区域
fn 区域单(t: &str, 区域: &BTreeSet<String>) -> bool {
    if t.ends_with('*') || t == "str" {
        return true;
    }
    let n = t.split('(').next().unwrap_or(t);
    if matches!(n, "list" | "que" | "stack" | "map") {
        return true;
    }
    区域.contains(t)
}

//nt: 收语句里的写
fn 语句写(s: &Stmt, out: &mut BTreeSet<LocalId>) {
    match s {
        Stmt::Assign { lhs, .. } => {
            out.insert(lhs.base);
        }
        Stmt::Copy { to, .. } => {
            out.insert(to.base);
        }
        Stmt::Asm(f) => f.outs.iter().for_each(|(_, id, _)| {
            out.insert(*id);
        }),
        _ => {}
    }
}

//nt: 跨圈逃逸判定
fn 跨圈(f: &Function, dst: LocalId, h: usize, c: usize) -> bool {
    for b in f.blocks.iter() {
        if b.id >= h && b.id <= c {
            continue;
        }
        let mut 读 = BTreeSet::new();
        let mut 写 = BTreeSet::new();
        let mut 写过 = false;
        for s in b.stmts.iter() {
            读.clear();
            写.clear();
            语句读(s, &mut 读);
            语句写(s, &mut 写);
            if 读.contains(&dst) && !写过 {
                return true;
            }
            if 写.contains(&dst) {
                写过 = true;
            }
        }
        读.clear();
        match &b.exit {
            Exit::Branch { cond, .. } => 值用(cond, &mut 读),
            Exit::Return(Some(v)) => 值用(v, &mut 读),
            _ => {}
        }
        if 读.contains(&dst) && !写过 {
            return true;
        }
    }
    false
}

//nt: 沾区域的积/和类型
fn 区型表(types: &[StructType]) -> BTreeSet<String> {
    let mut 集: BTreeSet<String> = BTreeSet::new();
    loop {
        let mut 加 = false;
        for t in types {
            if 集.contains(&t.name) {
                continue;
            }
            let 有 = t.fields.iter().any(|(_, ty)| 区域单(&ty.0, &集))
                || t.variants.iter().any(|v| v.fields.iter().any(|(_, ty)| 区域单(&ty.0, &集)));
            if 有 {
                集.insert(t.name.clone());
                加 = true;
            }
        }
        if !加 {
            return 集;
        }
    }
}

//nt: 该型可深拷
fn 可深拷(型: &str, types: &[StructType]) -> bool {
    let 标 = |t: &str| {
        matches!(
            t,
            "int" | "float" | "bool" | "str" | "int_8" | "int_16" | "int_32" | "int_64"
                | "uint_8" | "uint_16" | "uint_32" | "uint_64"
        )
    };
    if 标(型) {
        return true;
    }
    if 型.ends_with('*') {
        return false; // 裸指针：无从知道指向何物，不深拷
    }
    let 内 = |p: &str| 型.strip_prefix(p).and_then(|r| r.strip_suffix(')')).map(|x| x.trim().to_string());
    //nt: 容器元素仍限标量与串
    let 叶 = |t: &str| 标(t) || t == "str";
    for 前 in ["list(", "que(", "stack("] {
        if let Some(e) = 内(前) {
            return 叶(&e);
        }
    }
    if let Some(kv) = 内("map(") {
        let v = kv.rsplit(',').next().unwrap_or("").trim();
        return 叶(v);
    }
    let Some(st) = types.iter().find(|t| t.name == 型) else {
        return false;
    };
    st.variants.is_empty() && st.fields.iter().all(|(_, ty)| 可深拷(&ty.0, types))
}

//nt: 逃逸值复印
fn 复印方式(型: &str, 区域: &BTreeSet<String>, types: &[StructType]) -> 复印 {
    let 标量 = |t: &str| {
        matches!(
            t,
            "int" | "float" | "bool" | "int_8" | "int_16" | "int_32" | "int_64" | "uint_8"
                | "uint_16" | "uint_32" | "uint_64"
        )
    };
    if 标量(型) || !区域单(型, 区域) {
        return 复印::无需;
    }
    if 型 == "str" {
        return 复印::拷贝("#str_copy".into(), 0);
    }
    let 内 = |p: &str| -> Option<String> {
        型.strip_prefix(p)?.strip_suffix(')').map(|s| s.trim().to_string())
    };
    for 前 in ["list(", "que(", "stack("] {
        if let Some(e) = 内(前) {
            if e == "str" {
                return 复印::拷贝("#dc_list".into(), 1);
            }
            if 标量(&e) {
                return 复印::拷贝("#dc_list".into(), 0);
            }
            return 复印::无法;
        }
    }
    if let Some(kv) = 内("map(") {
        let v = kv.rsplit(',').next().unwrap_or("").trim();
        if v == "str" {
            return 复印::拷贝("#dc_map".into(), 1);
        }
        if 标量(v) {
            return 复印::拷贝("#dc_map".into(), 0);
        }
        return 复印::无法;
    }
    //nt: 结构体按字段深拷
    if 可深拷(型, types) {
        return 复印::拷贝(format!("#dcdeep_{}", 型), 0);
    }
    复印::无法
}

//nt: 收值里的局部
fn 值用(v: &Value, out: &mut BTreeSet<LocalId>) {
    if let Value::Local(id) = v {
        out.insert(*id);
    }
}

//nt: 收位置里的局部
fn 位用(p: &Place, out: &mut BTreeSet<LocalId>) {
    out.insert(p.base);
}

//nt: 收右值里的局部
fn 右用(r: &Rvalue, out: &mut BTreeSet<LocalId>) {
    match r {
        Rvalue::Use(p) | Rvalue::Ref(p) => 位用(p, out),
        Rvalue::Const(v) => 值用(v, out),
        Rvalue::BinOp { lhs, rhs, .. } => {
            值用(lhs, out);
            值用(rhs, out);
        }
        Rvalue::Call { args, .. } => args.iter().for_each(|a| 值用(a, out)),
        Rvalue::CallIndirect { callee, args } => {
            值用(callee, out);
            args.iter().for_each(|a| 值用(a, out));
        }
        Rvalue::ListLit { elems } => elems.iter().for_each(|e| 值用(e, out)),
        Rvalue::ListGet { list, index } => {
            值用(list, out);
            值用(index, out);
        }
        Rvalue::MapLit { pairs } => pairs.iter().for_each(|(_, v)| 值用(v, out)),
        Rvalue::MapGet { map, key } => {
            值用(map, out);
            值用(key, out);
        }
        Rvalue::VariantLit { args, .. } => args.iter().for_each(|a| 值用(a, out)),
        Rvalue::VariantTag(v) => 值用(v, out),
        Rvalue::VariantGet { value, .. } => 值用(value, out),
        Rvalue::Alloc { .. } | Rvalue::FnRef { .. } => {}
    }
}

//nt: 收语句里的读
fn 语句读(s: &Stmt, out: &mut BTreeSet<LocalId>) {
    match s {
        Stmt::Assign { rhs, .. } => 右用(rhs, out),
        Stmt::Call { args, .. } => args.iter().for_each(|a| 值用(a, out)),
        Stmt::ListSet { list, index, value } => {
            值用(list, out);
            值用(index, out);
            值用(value, out);
        }
        Stmt::MapSet { map, key, value } => {
            值用(map, out);
            值用(key, out);
            值用(value, out);
        }
        Stmt::Copy { from, .. } => 位用(from, out),
        Stmt::Assert { cond, .. } => 值用(cond, out),
        Stmt::Asm(f) => f.ins.iter().for_each(|(_, v)| 值用(v, out)),
        Stmt::Live(_) | Stmt::Dead(_) | Stmt::AccessBegin(_) | Stmt::AccessEnd(_)
        | Stmt::Destroy(_) | Stmt::Release(_) | Stmt::KillRegion { .. } => {}
    }
}

//nt: 实例化点核对
fn 核能力(
    名: &str,
    带能力: &[(String, Vec<String>)],
    实参类型: &[String],
) -> Result<(), BuildError> {
    for ((参数, 能力), 实) in 带能力.iter().zip(实参类型.iter()) {
        for op in 能力 {
            if !类型能力(实, op) {
                return Err(BuildError {
                    message: format!(
                        "泛型 fn「{}」的类型参数「{}」声明了能力「{}」，但实参类型「{}」不支持",
                        名, 参数, op, 实
                    ),
                    code: "E201".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                });
            }
        }
    }
    Ok(())
}

//nt: 编译期执行一个 fn
fn 编译期执行(
    名: &str,
    实参: &[crate::consts::Const],
    程序常量: &BTreeMap<String, crate::consts::Const>,
    深: usize,
) -> Option<crate::consts::Const> {
    if 深 > 32 {
        return None; // 递归深度配额，防止编译期打转
    }
    let (形参, 体) = 常量fn表.with(|t| t.borrow().get(名).cloned())?;
    if 形参.len() != 实参.len() {
        return None;
    }
    let mut 环: BTreeMap<String, crate::consts::Const> = 程序常量.clone();
    for (k, v) in 形参.iter().zip(实参.iter()) {
        环.insert(k.clone(), v.clone());
    }
    crate::consts::折叠带环境(
        &体,
        &mut |n| 环.get(n).cloned(),
        &mut |f, a| 编译期执行(f, a, 程序常量, 深 + 1),
    )
}

//nt: 读类型参数表
fn 读类型参数表(toks: &[Token], 开: usize) -> Option<Vec<(String, Vec<String>)>> {
    let 止 = 括号止(toks, 开)?;
    let mut 出 = Vec::new();
    for 段 in toks[开 + 1..止].split(|t| *t == Token::Sym(",")) {
        //nt: 按段解析
        match 段 {
            [Token::Ident(n)] => 出.push((n.clone(), Vec::new())),
            [Token::Ident(n), Token::Sym(op)] => 出.push((n.clone(), vec![(*op).to_string()])),
            _ => return None,
        }
    }
    if 出.is_empty() {
        None
    } else {
        Some(出)
    }
}

//nt: 从 `开` 处的
fn 括号止(toks: &[Token], 开: usize) -> Option<usize> {
    let mut 深 = 0i32;
    for (k, t) in toks.iter().enumerate().skip(开) {
        match t {
            Token::Sym("(") => 深 += 1,
            Token::Sym(")") => {
                深 -= 1;
                if 深 == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

//nt: 这个名字是不是泛型类
fn 泛型判别(名: &str) -> bool {
    GENERICS.with(|g| g.borrow().contains_key(名))
}

//nt: 取调用实参段
fn 取实参段(t: &[Token], 左: usize) -> Vec<Vec<Token>> {
    let mut 深 = 0i32;
    let mut 内 = Vec::new();
    for tk in t.iter().skip(左) {
        match tk {
            Token::Sym("(") => {
                深 += 1;
                if 深 == 1 {
                    continue;
                }
            }
            Token::Sym(")") => {
                深 -= 1;
                if 深 == 0 {
                    break;
                }
            }
            _ => {}
        }
        内.push(tk.clone());
    }
    split_args(&内)
}

//nt: 类型串 → toke
fn 类型串转token(s: &str) -> Vec<Token> {
    let 基 = s.trim_end_matches('*');
    let mut 出 = vec![Token::Ident(基.to_string())];
    for _ in 0..(s.len() - 基.len()) {
        出.push(Token::Sym("*"));
    }
    出
}

//nt: 把类型参数名整词换成
fn 替换类型参数token(toks: &[Token], 参数: &[String], 实参: &[String]) -> Vec<Token> {
    toks.iter()
        .map(|t| match t {
            Token::Ident(n) => match 参数.iter().position(|p| p == n) {
                Some(k) => Token::Ident(实参[k].clone()),
                None => t.clone(),
            },
            _ => t.clone(),
        })
        .collect()
}

//nt: 单态化一份泛型 fn
fn 建fn实例(
    bx: &mut Builder,
    实例名: &str,
    类型参数: &[String],
    实参类型: &[String],
    形参: &[(String, Ty)],
    返回: &Ty,
    体: &[Token],
) -> Result<(), BuildError> {
    let _ = bx;
    let mut 行: Vec<Token> = vec![
        Token::Kw("add"),
        Token::Kw("fn"),
        Token::Str(实例名.to_string()),
        Token::Sym("("),
    ];
    for (k, (名, 型)) in 形参.iter().enumerate() {
        if k > 0 {
            行.push(Token::Sym(","));
        }
        行.push(Token::Str(名.clone()));
        let 型串 = 替换参数(&型.0, 类型参数, 实参类型);
        行.extend(类型串转token(&型串));
    }
    行.push(Token::Sym(")"));
    let 返串 = 替换参数(&返回.0, 类型参数, 实参类型);
    行.extend(类型串转token(&返串));
    行.push(Token::Sym("="));
    行.extend(替换类型参数token(体, 类型参数, 实参类型));
    bx.line(&行)
}

//nt: 类型串拆成
fn 拆类型(s: &str) -> (String, Vec<String>) {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("fn(") {
        let mut 深 = 1i32;
        let mut 闭 = None;
        for (k, c) in rest.char_indices() {
            match c {
                '(' => 深 += 1,
                ')' => {
                    深 -= 1;
                    if 深 == 0 {
                        闭 = Some(k);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(闭) = 闭 {
            let mut 实参 = 切类型实参(&rest[..闭]);
            let 返 = rest[闭 + 1..].trim();
            if !返.is_empty() {
                实参.push(返.to_string());
            }
            return ("fn".to_string(), 实参);
        }
    }
    match s.find('(') {
        None => (s.to_string(), Vec::new()),
        Some(k) => {
            let 名 = s[..k].trim().to_string();
            let 内 = s[k + 1..].trim_end_matches(')');
            let 实参 = 切类型实参(内);
            (名, 实参)
        }
    }
}

//nt: 按顶层逗号切类型
fn 切类型实参(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut 深 = 0i32;
    for c in s.chars() {
        match c {
            '(' => {
                深 += 1;
                cur.push(c);
            }
            ')' => {
                深 -= 1;
                cur.push(c);
            }
            ',' if 深 == 0 => {
                let t = cur.trim().to_string();
                if !t.is_empty() {
                    out.push(t);
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let t = cur.trim().to_string();
    if !t.is_empty() {
        out.push(t);
    }
    out
}

//nt: 类型模式统一
fn 统一类型(pat: &str, got: &str, 参数: &[String], 绑定: &mut [Option<String>]) -> bool {
    let (p, g) = (pat.trim(), got.trim());
    if p.ends_with('*') && g.ends_with('*') {
        return 统一类型(&p[..p.len() - 1], &g[..g.len() - 1], 参数, 绑定);
    }
    let (pn, pa) = 拆类型(p);
    if pa.is_empty() {
        if let Some(i) = 参数.iter().position(|x| *x == pn) {
            return match &绑定[i] {
                Some(x) => x == g,
                None => {
                    绑定[i] = Some(g.to_string());
                    true
                }
            };
        }
    }
    let (gn, ga) = 拆类型(g);
    if pn != gn || pa.len() != ga.len() {
        return false;
    }
    pa.iter().zip(ga.iter()).all(|(a, b)| 统一类型(a, b, 参数, 绑定))
}

//nt: 把类型串里的类型参数
fn 替换参数(s: &str, 参数: &[String], 实参: &[String]) -> String {
    let mut 出 = String::new();
    let 字 = |c: char| c.is_alphanumeric() || c == '_';
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < cs.len() {
        if 字(cs[i]) {
            let 起 = i;
            while i < cs.len() && 字(cs[i]) {
                i += 1;
            }
            let 词: String = cs[起..i].iter().collect();
            match 参数.iter().position(|p| *p == 词) {
                Some(k) => 出.push_str(&实参[k]),
                None => 出.push_str(&词),
            }
            continue;
        }
        出.push(cs[i]);
        i += 1;
    }
    出
}

//nt: 构建错误
//nt: 递归收集所有语句行的
//nt: 供跨块引用登记等"全
fn collect_ast_lines<'a>(stmts: &'a [AstStmt], out: &mut Vec<&'a TokStream>) {
    for s in stmts {
        match s {
            AstStmt::Line { toks, .. } => out.push(toks),
            AstStmt::Struct { head, body, .. } | AstStmt::Fn { head, body, .. } => {
                out.push(head);
                collect_ast_lines(body, out);
            }
        }
    }
}

//nt: 同上，带行号
fn collect_ast_lines_ln<'a>(stmts: &'a [AstStmt], out: &mut Vec<(usize, &'a TokStream)>) {
    for s in stmts {
        match s {
            AstStmt::Line { line, toks, .. } => out.push((*line, toks)),
            AstStmt::Struct { line, head, body, .. } | AstStmt::Fn { line, head, body, .. } => {
                out.push((*line, head));
                collect_ast_lines_ln(body, out);
            }
        }
    }
}

fn err<T>(m: impl Into<String>) -> Result<T, BuildError> {
    Err(BuildError {
        message: m.into(),
        code: "E309".into(),
        line: CUR_LINE.with(|c| c.get()),
        file: String::new(),
    })
}

//nt: 结构行的种类
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    If,
    Else,
    While,
    Match,
    For,
}

fn struct_kind(head: &[Token]) -> Result<Kind, BuildError> {
    match head.first() {
        Some(Token::Kw("if")) => Ok(Kind::If),
        Some(Token::Kw("else")) => Ok(Kind::Else),
        Some(Token::Kw("while")) => Ok(Kind::While),
        Some(Token::Kw("match")) => Ok(Kind::Match),
        Some(Token::Kw("for")) => Ok(Kind::For),
        other => err(format!("不认识的结构行：{:?}", other)),
    }
}

fn is_else(head: &[Token]) -> bool {
    matches!(struct_kind(head), Ok(Kind::Else))
}

//nt: 构建器

//nt: 内置/转换调用的返回
fn builtin_ret_ty(name: &str) -> Ty {
    match name {
        "concat" | "sub_str" | "nth_char" | "char" | "str" | "read_file" => Ty("str".into()),
        "float" => Ty("float".into()),
        //nt: 类型转换
        "bool" => Ty("bool".into()),
//nt: 运行子程序
        "run" => Ty("int".into()),
        "has" | "empty" => Ty("bool".into()),
        //nt: 容器构造
        "que" => Ty("que".into()),
        "stack" => Ty("stack".into()),
        //nt: 取环境 / 表内容
        "args" | "keys" => Ty("list(str)".into()),
        "vals" => Ty("list(int)".into()),
        //nt: 定长族转换
        "int_8" | "int_16" | "int_32" | "int_64" | "uint_8" | "uint_16" | "uint_32" | "uint_64" => {
            Ty(name.to_string())
        }
        "str_from_c" => Ty("str".into()),
        _ => Ty("int".into()),
    }
}

//nt: 判断 ## 形态
fn hh_is_fn_form(t: &[Token], start: usize) -> bool {
    let mut j = start;
    loop {
        match t.get(j) {
            Some(Token::Ident(_)) | Some(Token::Str(_)) => {
                j += 1;
                if matches!(t.get(j), Some(Token::Sym("."))) {
                    j += 1;
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    if !matches!(t.get(j), Some(Token::HashColon)) {
        return false;
    }
    matches!(t.get(j + 1), Some(Token::Ident(_)) | Some(Token::Kw(_)))
        && matches!(t.get(j + 2), Some(Token::Kw("fn")))
}

//nt: 字段类型查表
fn field_ty_in(
    types: &[StructType],
    idx: &std::collections::HashMap<String, usize>,
    owner: &str,
    field: &str,
) -> String {
    idx.get(owner)
        .and_then(|&i| types[i].fields.iter().find(|(n, _)| n == field))
        .map(|(_, ty)| ty.0.clone())
        .unwrap_or_else(|| "int".into())
}

//nt: list
fn elem_ty_of(ty: &str) -> Option<String> {
    for pre in ["list(", "que(", "stack("] {
        if let Some(inner) = ty.strip_prefix(pre).and_then(|s| s.strip_suffix(')')) {
            return Some(inner.to_string());
        }
    }
    if let Some(inner) = ty.strip_prefix("map(").and_then(|s| s.strip_suffix(')')) {
        //nt: map
        let mut depth = 0i32;
        for (i, c) in inner.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => return Some(inner[i + 1..].trim().to_string()),
                _ => {}
            }
        }
    }
    None
}

//nt: 容器字面量 RHS
fn lit_hint(decl: &Ty, after: &[Token]) -> Option<Ty> {
    match after.first() {
        Some(Token::Sym("[")) | Some(Token::Sym("{")) => Some(decl.clone()),
        //nt: 传元素类型
        Some(Token::Kw("que")) | Some(Token::Kw("stack")) => Some(decl.clone()),
        Some(Token::Ident(n)) if n == "que" || n == "stack" => Some(decl.clone()),
        _ => None,
    }
}

///nt: 取序列元素类型
fn 序列元素类型(t: &str) -> Option<String> {
    let (head, inner) = t.split_once('(')?;
    let tail = inner.strip_suffix(')')?;
    match head {
        "list" | "que" | "stack" => Some(tail.trim().to_string()),
        _ => None,
    }
}

//nt: 容器类型位
fn container_ty_at(toks: &[Token], i: usize) -> Option<String> {
    container_ty_at_raw(toks, i).map(|s| 代换类型(&s))
}

fn container_ty_at_raw(toks: &[Token], i: usize) -> Option<String> {
    let name = match toks.get(i)? {
        Token::Kw(k) => (*k).to_string(),
        _ => return None,
    };
    if name != "list" && name != "que" && name != "stack" && name != "map" {
        return None;
    }
    if !matches!(toks.get(i + 1), Some(Token::Sym("("))) {
        return Some(name); // 裸容器名
    }
    let mut depth = 0i32;
    let mut end = None;
    for (k, tk) in toks.iter().enumerate().skip(i + 1) {
        match tk {
            Token::Sym("(") => depth += 1,
            Token::Sym(")") => {
                depth -= 1;
                if depth == 0 {
                    end = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    let inner = &toks[i + 2..end];
    let arg_name = |ts: &[Token]| -> Option<String> {
        if matches!(ts.first(), Some(Token::Kw("fn"))) && matches!(ts.get(1), Some(Token::Sym("("))) {
            return parse_type_ref(ts, 0).ok().map(|(t, _, _)| t.0);
        }
        let base = match ts.first()? {
            Token::Kw(t) => (*t).to_string(),
            Token::Ident(n) => n.clone(),
            _ => return None,
        };
        if matches!(ts.get(1), Some(Token::Sym("*"))) {
            Some(format!("{}*", base))
        } else {
            Some(base)
        }
    };
    if name == "map" {
        let args = split_args(inner);
        if args.len() != 2 {
            return None;
        }
        let k = arg_name(&args[0])?;
        let v = arg_name(&args[1])?;
        return Some(format!("map({}, {})", k, v));
    }
    let e = arg_name(inner)?;
    Some(format!("{}({})", name, e))
}

//nt: 块内公式函数的签名
//nt: params 与字段
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct FnSig {
    params: Vec<Ty>,
    ret: Ty,
    //nt: 外部函数声明
    is_outs: bool,
    //nt: fn用的val槽参数
    hidden: Vec<(String, Ty)>,
}

//nt: 取函数返回类型
fn fn_ret_of(s: &str) -> Option<String> {
    let rest = s.strip_prefix("fn(")?;
    let mut 深 = 1i32;
    for (k, c) in rest.char_indices() {
        match c {
            '(' => 深 += 1,
            ')' => {
                深 -= 1;
                if 深 == 0 {
                    let ret = rest[k + 1..].trim();
                    return if ret.is_empty() { None } else { Some(ret.to_string()) };
                }
            }
            _ => {}
        }
    }
    None
}

struct Builder {
    f: Function,
    //nt: 本 Builder
    block: String,
    //nt: 本 Builder
    module: String,
    names: BTreeMap<String, LocalId>,
    //nt: 本块内已登记的公式函
    fns: BTreeMap<String, FnSig>,
    //nt: 全程序的公式函数
    global_fns: std::rc::Rc<BTreeMap<String, FnSig>>,
    //nt: 全程序跨块取值
    //nt: 这些 val 的存储
    cross_vals: std::rc::Rc<BTreeMap<String, BTreeMap<String, Ty>>>,
    //nt: 本块里作**指针参数
    exposed: BTreeMap<String, LocalId>,
    //nt: 容器字面量在 IR
    lit_ty: Option<Ty>,
    //nt: 当前处理的源码行号
    cur_line: usize,
    cur_col: usize,
    cur_len: usize,
    //nt: 循环体嵌套层
    循环深: usize,
    //nt: 全程序的和类型变体
    //nt: 下接
//nt: 标了 `pub` 的
    pub_blocks: std::rc::Rc<BTreeSet<String>>,
    //nt: 被 hide 的条目
    hidden_items: std::rc::Rc<BTreeSet<String>>,
    //nt: 每块类型表
    block_types: std::rc::Rc<BTreeMap<String, BTreeSet<String>>>,
    sum_variants: std::rc::Rc<BTreeMap<String, (String, usize, Vec<Ty>)>>,
    //nt: 当前右值的期望类型
    //nt: 变体构造据此选中实例
    期望类型: Option<String>,
    //nt: 已折叠的常量值
    常量值: BTreeMap<String, crate::consts::Const>,
    //nt: fn内可中途give
    语句体返回: bool,
    //nt: 全程序的积类型定义
    types: std::rc::Rc<Vec<StructType>>,
    //nt: 类型名 → type
    type_idx: std::rc::Rc<std::collections::HashMap<String, usize>>,
    //nt: 模块尾名 → 全路径
    mod_by_tail: std::rc::Rc<BTreeMap<String, Vec<String>>>,
    //nt: 函数定义生成的独立
    extra: Vec<Function>,
    //nt: 外部函数声明
    externs: Vec<ExternFn>,
    blocks: Vec<Block>,
    cur: usize,
    next_id: BlockId,
}

//nt: 解析泛型语句头
fn 解析泛型语句头(
    head: &[Token],
) -> Option<(String, Vec<(String, Vec<String>)>, Vec<(String, Ty)>, Ty)> {
    let h: Vec<Token> = head.iter().cloned().filter(|t| *t != Token::Sym(":")).collect();
    let is_rebind = matches!(h.first(), Some(Token::Kw("fn")));
    let is_hide_fn = matches!(h.first(), Some(Token::Kw("add")))
        && matches!(h.get(1), Some(Token::Kw("hide")))
        && matches!(h.get(2), Some(Token::Kw("fn")));
    let name_at = if is_rebind { 1 } else if is_hide_fn { 3 } else { 2 };
    let name = match h.get(name_at) {
        Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
        _ => return None,
    };
    if !matches!(h.get(name_at + 1), Some(Token::Sym("("))) {
        return None;
    }
    let 类型参数 = 读类型参数表(&h, name_at + 1)?;
    let 止 = 括号止(&h, name_at + 1)? + 1;
    let (形参, 帕止) = parse_params(&h[止..]).ok()?;
    let (ret, _, _) = parse_type_ref(&h[止..], 帕止).ok()?;
    Some((name, 类型参数, 形参, ret))
}

impl Builder {
    //nt: 给定名是否是某个模块
    fn mod_has(&self, given: &str) -> bool {
        self.mod_by_tail.values().any(|v| v.iter().any(|m| m == given))
    }

//nt: 模块路径尾名退回：
    //nt: 解析模块路径
    fn resolve_module(&self, given: &str) -> Result<Option<String>, BuildError> {
        if self.mod_by_tail.values().any(|v| v.iter().any(|m| m == given)) {
            return Ok(Some(given.to_string()));
        }
        let cands = match self.mod_by_tail.get(given) {
            Some(c) => c,
            None => return Ok(None),
        };
        if cands.len() == 1 {
            return Ok(Some(cands[0].clone()));
        }
        Err(BuildError {
            message: format!(
                "模块「{}」有歧义——{} 都叫这名，请写全路径（如 `##:{}`）",
                given,
                cands.join("、"),
                cands[0]
            ),
            code: "E304".into(),
            line: CUR_LINE.with(|c| c.get()),
            file: String::new(),
        })
    }

//nt: 跨模块引用的可见性门
    fn check_pub(&self, blk_mod: &str, blk: &str) -> Result<(), BuildError> {
        if blk_mod == self.module {
            return Ok(());
        }
        if self.pub_blocks.contains(&format!("{}|{}", blk_mod, blk)) {
            return Ok(());
        }
        Err(BuildError {
            message: format!(
                "块「{}」（模块 {}）是私有的——跨模块只能调标了 pub 的块",
                blk, blk_mod
            ),
            code: "E201".into(),
            line: CUR_LINE.with(|c| c.get()),
            file: String::new(),
        })
    }

    #[allow(clippy::too_many_arguments)] // 构造器参数多是历史形态；收结构体留待重构专项
    fn new(
        name: String,
        file: String,
        is_block: bool,
        is_pub: bool,
        is_sle: bool,
        is_kill: bool,
        is_abi: bool,
        is_naked: bool,
        asm_lines: Vec<String>,
        block: String,
        module: String,
        types: std::rc::Rc<Vec<StructType>>,
        type_idx: std::rc::Rc<std::collections::HashMap<String, usize>>,
        mod_by_tail: std::rc::Rc<BTreeMap<String, Vec<String>>>,
        global_fns: std::rc::Rc<BTreeMap<String, FnSig>>,
        cross_vals: std::rc::Rc<BTreeMap<String, BTreeMap<String, Ty>>>,
        sum_variants: std::rc::Rc<BTreeMap<String, (String, usize, Vec<Ty>)>>,
        pub_blocks: std::rc::Rc<BTreeSet<String>>,
        hidden_items: std::rc::Rc<BTreeSet<String>>,
        block_types: std::rc::Rc<BTreeMap<String, BTreeSet<String>>>,
    ) -> Self {
        Self {
            f: Function {
                name,
                file,
                ret: None,
                locals: Vec::new(),
                blocks: Vec::new(),
                is_block,
                is_pub,
                is_sle,
                kills_own: is_kill,
                is_abi,
                is_naked,
                asm_lines,
            },
            block,
            module,
            names: BTreeMap::new(),
            fns: BTreeMap::new(),
            global_fns,
            cross_vals,
            exposed: BTreeMap::new(),
            lit_ty: None,
            期望类型: None,
            语句体返回: false,
            常量值: BTreeMap::new(),
            cur_line: CUR_LINE.with(|c| c.get()),
            cur_col: 0,
            cur_len: 0,
            循环深: 0,
            sum_variants,
            mod_by_tail,
            pub_blocks,
            hidden_items,
            block_types,
            type_idx,
            types,
            extra: Vec::new(),
            externs: Vec::new(),
            blocks: vec![Block {
                id: 0,
                params: Vec::new(),
                stmts: Vec::new(),
                spans: Vec::new(),
                exit: Exit::Unreachable,
            }],
            cur: 0,
            next_id: 1,
        }
    }

    //nt: 开一个新基本块，返回
    fn nb(&mut self) -> BlockId {
        let id = self.next_id;
        self.next_id += 1;
        self.blocks.push(Block {
            id,
            params: Vec::new(),
            stmts: Vec::new(),
            spans: Vec::new(),
            exit: Exit::Unreachable,
        });
        id
    }

    fn switch_to(&mut self, id: BlockId) {
        self.cur = self.blocks.iter().position(|b| b.id == id).expect("块应已存在");
    }

    fn emit(&mut self, s: Stmt) {
        let c = self.cur;
        let sp = Span { line: self.cur_line, col: self.cur_col, len: self.cur_len };
        self.blocks[c].stmts.push(s);
        self.blocks[c].spans.push(sp);
    }

    fn set_exit(&mut self, e: Exit) {
        self.blocks[self.cur].exit = e;
    }

    //nt: 无条件跳转
    fn goto(&mut self, target: BlockId) {
        self.set_exit(Exit::Branch { cond: Value::Int(1), targets: vec![target] });
    }

    fn new_local(&mut self, name: Option<String>, ty: Ty, is_param: bool, bare_ok: bool) -> LocalId {
        let (line, col) = (self.cur_line, self.cur_col);
        let loop_local = self.循环深 > 0;
        self.f.locals.push(Local { name, ty, is_param, bare_ok, line, col, loop_local, loop_escape: false, fn_owned: false });
        self.f.locals.len() - 1
    }

    //nt: 字段类型查表
    fn field_ty_of(&self, owner: &str, field: &str) -> String {
        field_ty_in(&self.types, &self.type_idx, owner, field)
    }

    //nt: 槽自动就位
    //nt: 存储全局化后随时可取
    fn 槽或建(&mut self, slot_key: &str) -> Option<LocalId> {
        if let Some(&id) = self.names.get(slot_key) {
            return Some(id);
        }
        let (blk_full, vn) = slot_key.rsplit_once('#')?;
        let vty = self.cross_vals.get(blk_full)?.get(vn)?.clone();
        let id = self.new_local(Some(slot_key.to_string()), vty, false, false);
        self.names.insert(slot_key.to_string(), id);
        Some(id)
    }

    fn temp(&mut self, ty: Ty) -> LocalId {
        self.new_local(None, ty, false, false)
    }

    //nt: 值的类型
    fn ty_of_value(&self, v: Value) -> Ty {
        match v {
            Value::Local(id) => {
                self.f.locals.get(id).map(|l| l.ty.clone()).unwrap_or_else(|| Ty("int".into()))
            }
            Value::Float(_) => Ty("float".into()),
            Value::Str(_) => Ty("str".into()),
            Value::Bool(_) => Ty("bool".into()),
            Value::Null => Ty("null".into()),
            Value::Int(_) => Ty("int".into()),
        }
    }

    //nt: 值是不是 str
    fn is_str_value(&self, v: &Value) -> bool {
        match v {
            Value::Str(_) => true,
            Value::Local(id) => self.f.locals.get(*id).map(|l| l.ty.0 == "str").unwrap_or(false),
            _ => false,
        }
    }

    //nt: 落一条二元运算到临时
    //nt: 字段链
    //nt: 供「名字后」与「下标
    fn field_chain(
        &mut self,
        mut cur: usize,
        mut cur_ty: String,
        t: &[Token],
        i: &mut usize,
    ) -> Result<Value, BuildError> {
        while matches!(t.get(*i), Some(Token::Sym("."))) {
            let field = match t.get(*i + 1) {
                Some(Token::Ident(f)) | Some(Token::Str(f)) => f.clone(),
                _ => return err("字段访问后应是字段名"),
            };
            *i += 2;
            //nt: 字段访问只对**积类
            let owner0 = cur_ty.trim_end_matches('*').to_string();
            if !self.type_idx.contains_key(&owner0) {
                return Err(BuildError {
                    message: format!(
                        "{} 没有字段——字段访问（.名）只对 add type 定义的积类型用；和类型请用 match 解构",
                        cur_ty
                    ),
                    code: "E303".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                });
            }
            if cur_ty.ends_with('*') {
                let c = self.bin(BinOp::Ne, Value::Local(cur), Value::Null);
                self.emit(Stmt::Assert {
                    cond: c,
                    msg: "空引用——把手是 null，取不到字段".into(),
                    fix: "取字段前先判一声（x != null），或用 case 接住空的情况".into(),
                });
            }
            let owner = cur_ty.trim_end_matches('*').to_string();
            let ft = self.field_ty_of(&owner, &field);
            let tmp = self.temp(Ty(ft.clone()));
            self.emit(Stmt::Assign {
                lhs: Place::local(tmp),
                rhs: Rvalue::Use(Place { base: cur, proj: vec![Proj::Field(field)] }),
            });
            cur = tmp;
            cur_ty = ft;
        }
        Ok(Value::Local(cur))
    }

    //nt: 值是否是 float
    fn is_float_value(&self, v: &Value) -> bool {
        match v {
            Value::Float(_) => true,
            Value::Local(id) => self.f.locals.get(*id).map(|l| l.ty.0 == "float").unwrap_or(false),
            _ => false,
        }
    }

    //nt: 被调fn的槽实参
    //nt: 同块传槽指针
    fn 槽实参(
        &mut self,
        sig: &FnSig,
        call_key: &str,
        caller_is_fn: bool,
    ) -> Result<Vec<Value>, BuildError> {
        let mut out = Vec::new();
        for (vn, _ty) in &sig.hidden {
            let mut it = call_key.splitn(3, '|');
            let _m = it.next().unwrap_or("");
            let blk = it.next().unwrap_or("");
            if blk == self.block {
                //nt: 传槽指针或地址
                match self.exposed.get(vn) {
                    Some(&pid) => out.push(Value::Local(pid)),
                    None => match self.names.get(vn) {
                        Some(&lid) => {
                            let tmp = self.temp(Ty(format!("{}*", _ty.0)));
                            self.emit(Stmt::Assign {
                                lhs: Place::local(tmp),
                                rhs: Rvalue::Ref(Place::local(lid)),
                            });
                            out.push(Value::Local(tmp));
                        }
                        None => {
                            return Err(BuildError {
                                message: format!("fn 要用本块 val「{}」——本块还没建它", vn),
                                code: "E304".into(),
                                line: CUR_LINE.with(|c| c.get()),
                                file: String::new(),
                            })
                        }
                    },
                }
            } else {
                match self.exposed.get(vn) {
                    Some(&pid) => out.push(Value::Local(pid)),
                    None => {
                        let slot_key = format!("{}|{}#{}", self.module, blk, vn);
                        match self.槽或建(&slot_key) {
                            Some(sid) => {
                                let tmp = self.temp(Ty(format!("{}*", _ty.0)));
                                self.emit(Stmt::Assign {
                                    lhs: Place::local(tmp),
                                    rhs: Rvalue::Ref(Place::local(sid)),
                                });
                                out.push(Value::Local(tmp));
                            }
                            None => {
                                return Err(BuildError {
                                    message: format!(
                                        "块「{}」的 val「{}」不存在——先 add val 建它，sle 块还要先 call #:{}",
                                        blk, vn, blk
                                    ),
                                    code: "E304".into(),
                                    line: CUR_LINE.with(|c| c.get()),
                                    file: String::new(),
                                })
                            }
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    //nt: 类型串是否是容器
    fn is_container_ty(t: &str) -> bool {
        matches!(t.split('(').next().unwrap_or("").trim(), "list" | "que" | "stack" | "map")
    }

    //nt: map
    fn map_val_ty(ty: &str) -> Option<String> {
        let inner = ty.strip_prefix("map(")?.strip_suffix(')')?;
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

    //nt: 值是否是容器
    fn is_container_value(&self, v: &Value) -> bool {
        match v {
            Value::Local(id) => self
                .f
                .locals
                .get(*id)
                .map(|l| Self::is_container_ty(&l.ty.0))
                .unwrap_or(false),
            _ => false,
        }
    }

    //nt: 值是否是 bool
    fn is_bool_value(&self, v: &Value) -> bool {
        match v {
            Value::Bool(_) => true,
            Value::Local(id) => self.f.locals.get(*id).map(|l| l.ty.0 == "bool").unwrap_or(false),
            _ => false,
        }
    }

    fn bin(&mut self, op: BinOp, lhs: Value, rhs: Value) -> Value {
        let ty = match op {
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::And
            | BinOp::Or => Ty("bool".into()),
            //nt: 字符串拼接
            BinOp::Add if self.is_str_value(&lhs) || self.is_str_value(&rhs) => Ty("str".into()),
            //nt: float 参与 +
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div
                if self.is_float_value(&lhs) || self.is_float_value(&rhs) =>
            {
                Ty("float".into())
            }
            _ => Ty("int".into()),
        };
        let t = self.temp(ty);
        self.emit(Stmt::Assign { lhs: Place::local(t), rhs: Rvalue::BinOp { op, lhs, rhs } });
        Value::Local(t)
    }

    fn finish(mut self) -> Result<Function, BuildError> {
        //nt: 只在当前块**还没有
        if matches!(self.blocks[self.cur].exit, Exit::Unreachable) {
            self.set_exit(Exit::Return(None));
        }
        self.f.blocks = self.blocks;
        Ok(self.f)
    }


    //nt: if 头折成常量
    fn 条件常量(&self, head: &[Token]) -> Option<bool> {
        let 体: Vec<Token> = head.iter().filter(|t| **t != Token::Sym(":")).cloned().collect();
        let 条 = match 体.first() {
            Some(Token::Kw("if")) => &体[1..],
            _ => &体[..],
        };
        let mut 换: Vec<Token> = Vec::with_capacity(条.len());
        let mut k = 0usize;
        while k < 条.len() {
            if matches!(条.get(k), Some(Token::Kw("val"))) {
                if let Some(Token::Str(n)) | Some(Token::Ident(n)) = 条.get(k + 1) {
                    if let Some(c) = self.常量值.get(n) {
                        换.push(c.as_token());
                        k += 2;
                        continue;
                    }
                }
            }
            换.push(条[k].clone());
            k += 1;
        }
        crate::consts::折叠条件(&换)
    }

    //nt: 泛型构造
    fn 反解实例(&self, 名: &str, args: &[Vec<Token>]) -> Result<String, BuildError> {
        let 模板 = self
            .types
            .get(self.type_idx[名])
            .cloned()
            .ok_or_else(|| BuildError {
                message: format!("类型「{}」没登记", 名),
                code: "E304".into(),
                line: CUR_LINE.with(|c| c.get()),
                file: String::new(),
            })?;
        let mut 绑定: Vec<Option<String>> = vec![None; 模板.params.len()];
        for (k, a) in args.iter().enumerate() {
            if k >= 模板.fields.len() {
                break;
            }
            let got = match self.推断类型(a) {
                Some(g) => g,
                None => continue,
            };
            let pat = 模板.fields[k].1 .0.clone();
            if let Some(pi) = 模板.params.iter().position(|p| *p == pat) {
                绑定[pi] = Some(got);
            } else if let Some(pi) = 模板.params.iter().position(|p| pat == format!("{}*", p)) {
                if let Some(内) = got.strip_suffix('*') {
                    绑定[pi] = Some(内.to_string());
                }
            }
        }
        if 绑定.iter().any(|x| x.is_none()) {
            return Err(BuildError {
                message: format!(
                    "泛型类型「{}」推不出类型参数——给每个实参一个明确的类型（字面量或带类型的 val）",
                    名
                ),
                code: "E201".into(),
                line: CUR_LINE.with(|c| c.get()),
                file: String::new(),
            });
        }
        let 实参: Vec<String> = 绑定.into_iter().map(|x| x.unwrap()).collect();
        let 实例名 = format!("{}({})", 名, 实参.join(", "));
        if !self.type_idx.contains_key(&实例名) {
            return Err(BuildError {
                message: format!(
                    "泛型实例「{}」还没生成——请先写一条带类型的声明 `add val \"x\" {}(…) = …`",
                    实例名, 实例名
                ),
                code: "E201".into(),
                line: CUR_LINE.with(|c| c.get()),
                file: String::new(),
            });
        }
        Ok(实例名)
    }

    //nt: 按 token 形状
    fn 推断类型(&self, toks: &[Token]) -> Option<String> {
        match toks {
            [Token::Int(_)] => Some("int".to_string()),
            [Token::Float(_)] => Some("float".to_string()),
            [Token::Str(_)] | [Token::RawStr(_)] => Some("str".to_string()),
            [Token::Kw("re")] | [Token::Kw("fa")] => Some("bool".to_string()),
            [Token::Kw("val"), Token::Str(n)] | [Token::Kw("val"), Token::Ident(n)]
            | [Token::Kw("call"), Token::Kw("val"), Token::Str(n)]
            | [Token::Kw("call"), Token::Kw("val"), Token::Ident(n)] => {
                let id = *self.names.get(n)?;
                self.f.locals.get(id).map(|l| l.ty.0.clone())
            }
            [Token::Kw("fn"), Token::Str(n)] | [Token::Kw("fn"), Token::Ident(n)] => {
                let key = format!("{}|{}|{}", self.module, self.block, n);
                let sig = self.fns.get(&key).or_else(|| self.global_fns.get(&key))?;
                let ps: Vec<String> = sig.params.iter().map(|t| t.0.clone()).collect();
                Some(format!("fn({}) {}", ps.join(", "), sig.ret.0))
            }
            //nt: val "名".字段
            [Token::Kw("val"), Token::Str(n), Token::Sym("."), Token::Ident(f)]
            | [Token::Kw("val"), Token::Ident(n), Token::Sym("."), Token::Ident(f)]
            | [Token::Kw("call"), Token::Kw("val"), Token::Str(n), Token::Sym("."), Token::Ident(f)]
            | [Token::Kw("call"), Token::Kw("val"), Token::Ident(n), Token::Sym("."), Token::Ident(f)] => {
                let id = *self.names.get(n)?;
                let 主 = self.f.locals.get(id)?.ty.0.clone();
                Some(self.field_ty_of(&主, f))
            }
            //nt: 名.字段
            [Token::Ident(n), Token::Sym("."), Token::Ident(f)] => {
                let id = *self.names.get(n)?;
                let 主 = self.f.locals.get(id)?.ty.0.clone();
                Some(self.field_ty_of(&主, f))
            }
            [Token::Ident(n)] => {
                let id = *self.names.get(n)?;
                self.f.locals.get(id).map(|l| l.ty.0.clone())
            }
            _ => None,
        }
    }

    //nt: 内联 asm
    fn 建asm(&mut self, 内区: &[Token]) -> Result<AsmForm, BuildError> {
        let 段 = anle_lexer::asm::解析隔离区(内区).map_err(|e| BuildError {
            message: e,
            code: "E201".into(),
            line: CUR_LINE.with(|c| c.get()),
            file: String::new(),
        })?;
        let mut outs = Vec::new();
        for (约束, 名, 读写) in 段.outs {
            let id = *self.names.get(&名).ok_or_else(|| BuildError {
                message: format!("asm 输出绑定的名字「{}」本块没有——先 add val 建它", 名),
                code: "E304".into(),
                line: CUR_LINE.with(|c| c.get()),
                file: String::new(),
            })?;
            outs.push((约束, id, 读写));
        }
        let mut ins = Vec::new();
        for (约束, 表达) in 段.ins {
            let v = self.expr(&表达)?;
            ins.push((约束, v));
        }
        Ok(AsmForm {
            tmpl: 段.tmpl,
            outs,
            ins,
            clobbers: 段.clobbers,
        })
    }

    fn expr(&mut self, toks: &[Token]) -> Result<Value, BuildError> {
        if toks.is_empty() {
            return err("表达式是空的");
        }
        //nt: call前缀读
        if matches!(toks.first(), Some(Token::Kw("call")))
            && matches!(toks.get(1), Some(Token::Kw("val")))
            && !matches!(toks.get(3), Some(Token::Sym("(")))
        {
            return self.expr(&toks[1..]);
        }
        //nt: 内联汇编
        if toks.first().map(anle_lexer::asm::是asm词).unwrap_or(false)
            && matches!(toks.get(1), Some(Token::Sym("(")))
        {
            let 内区 = anle_lexer::asm::括号内(toks, 1)
                .ok_or_else(|| BuildError {
                    message: "asm 的括号没闭合".into(),
                    code: "E301".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                })?
                .to_vec();
            let form = self.建asm(&内区)?;
            let 首 = match form.outs.first() {
                Some((_, id, _)) => *id,
                None => return err("asm 没有输出约束，不能当事赋值给变量——它是动作语句"),
            };
            self.emit(Stmt::Asm(form));
            return Ok(Value::Local(首));
        }
        let mut i = 0usize;
        let v = self.parse_or(toks, &mut i)?;
        //nt: 三元表达式
        if matches!(toks.get(i), Some(Token::Sym("?"))) {
            let colon = toks
                .iter()
                .enumerate()
                .skip(i + 1)
                .find(|(_, t)| matches!(t, Token::Sym(":")))
                .map(|(k, _)| k)
                .ok_or_else(|| BuildError { message: "三元缺 `:`".into() , code: "E301".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
            let then_toks: Vec<Token> = toks[i + 1..colon].to_vec();
            let else_toks: Vec<Token> = toks[colon + 1..].to_vec();
            let tmp = self.temp(Ty("int".into()));
            let bb_then = self.nb();
            let bb_else = self.nb();
            let bb_done = self.nb();
            self.set_exit(Exit::Branch { cond: v, targets: vec![bb_then, bb_else] });
            self.switch_to(bb_then);
            let tv = self.expr(&then_toks)?;
            self.emit(Stmt::Assign { lhs: Place::local(tmp), rhs: Rvalue::Const(tv.clone()) });
            self.goto(bb_done);
            self.switch_to(bb_else);
            let ev = self.expr(&else_toks)?;
            self.emit(Stmt::Assign { lhs: Place::local(tmp), rhs: Rvalue::Const(ev) });
            self.goto(bb_done);
            self.switch_to(bb_done);
            //nt: 用真值分支的实际类型
            let real_ty = match &tv {
                Value::Local(id) => self.f.locals.get(*id).map(|l| l.ty.clone()),
                Value::Int(_) => Some(Ty("int".into())),
                Value::Float(_) => Some(Ty("float".into())),
                Value::Bool(_) => Some(Ty("bool".into())),
                Value::Str(_) => Some(Ty("str".into())),
                _ => None,
            };
            if let Some(t) = real_ty {
                if let Some(l) = self.f.locals.get_mut(tmp) {
                    l.ty = t;
                }
            }
            return Ok(Value::Local(tmp));
        }
        if i != toks.len() {
            return err(format!("表达式没读完（多余：{:?}）", &toks[i..]));
        }
        Ok(v)
    }

    fn parse_or(&mut self, t: &[Token], i: &mut usize) -> Result<Value, BuildError> {
        let mut l = self.parse_and(t, i)?;
        while matches!(t.get(*i), Some(Token::Kw("or"))) {
            *i += 1;
            let r = self.parse_and(t, i)?;
            l = self.bin(BinOp::Or, l, r);
        }
        Ok(l)
    }

    fn parse_and(&mut self, t: &[Token], i: &mut usize) -> Result<Value, BuildError> {
        let mut l = self.parse_cmp(t, i)?;
        while matches!(t.get(*i), Some(Token::Kw("and"))) {
            *i += 1;
            let r = self.parse_cmp(t, i)?;
            l = self.bin(BinOp::And, l, r);
        }
        Ok(l)
    }

    fn parse_cmp(&mut self, t: &[Token], i: &mut usize) -> Result<Value, BuildError> {
        let mut l = self.parse_bitor(t, i)?;
        while let Some(op) = match t.get(*i) {
            Some(Token::Sym("==")) => Some(BinOp::Eq),
            Some(Token::Sym("!=")) => Some(BinOp::Ne),
            Some(Token::Sym("<=")) => Some(BinOp::Le),
            Some(Token::Sym(">=")) => Some(BinOp::Ge),
            Some(Token::Sym("<")) => Some(BinOp::Lt),
            Some(Token::Sym(">")) => Some(BinOp::Gt),
            _ => None,
        } {
            *i += 1;
            let r = self.parse_bitor(t, i)?;
//nt: 链式比较不做
            if self.is_bool_value(&l) != self.is_bool_value(&r) {
                return err("链式比较（如 `1 < x < 5`）v0 不做——拆成两个比较，用 `and` 连（如 `1 < x and x < 5`）");
            }
            l = self.bin(op, l, r);
        }
        Ok(l)
    }

//nt: 位运算层
    fn parse_bitor(&mut self, t: &[Token], i: &mut usize) -> Result<Value, BuildError> {
        let mut l = self.parse_bitxor(t, i)?;
        while matches!(t.get(*i), Some(Token::Sym("|"))) {
            *i += 1;
            let r = self.parse_bitxor(t, i)?;
            l = self.bin(BinOp::BitOr, l, r);
        }
        Ok(l)
    }

    fn parse_bitxor(&mut self, t: &[Token], i: &mut usize) -> Result<Value, BuildError> {
        let mut l = self.parse_bitand(t, i)?;
        while matches!(t.get(*i), Some(Token::Sym("^"))) {
            *i += 1;
            let r = self.parse_bitand(t, i)?;
            l = self.bin(BinOp::BitXor, l, r);
        }
        Ok(l)
    }

    fn parse_bitand(&mut self, t: &[Token], i: &mut usize) -> Result<Value, BuildError> {
        let mut l = self.parse_shift(t, i)?;
        while matches!(t.get(*i), Some(Token::Sym("&"))) {
            *i += 1;
            let r = self.parse_shift(t, i)?;
            l = self.bin(BinOp::BitAnd, l, r);
        }
        Ok(l)
    }

    fn parse_shift(&mut self, t: &[Token], i: &mut usize) -> Result<Value, BuildError> {
        let mut l = self.parse_add(t, i)?;
        while let Some(op) = match t.get(*i) {
            Some(Token::Sym("<<")) => Some(BinOp::Shl),
            Some(Token::Sym(">>")) => Some(BinOp::Shr),
            _ => None,
        } {
            *i += 1;
            let r = self.parse_add(t, i)?;
            l = self.bin(op, l, r);
        }
        Ok(l)
    }

    fn parse_add(&mut self, t: &[Token], i: &mut usize) -> Result<Value, BuildError> {
        let mut l = self.parse_mul(t, i)?;
        while let Some(op) = match t.get(*i) {
            Some(Token::Sym("+")) => Some(BinOp::Add),
            Some(Token::Sym("-")) => Some(BinOp::Sub),
            _ => None,
        } {
            *i += 1;
            let r = self.parse_mul(t, i)?;
            l = self.bin(op, l, r);
        }
        Ok(l)
    }

    fn parse_mul(&mut self, t: &[Token], i: &mut usize) -> Result<Value, BuildError> {
        let mut l = self.operand(t, i)?;
        while let Some(op) = match t.get(*i) {
            Some(Token::Sym("*")) => Some(BinOp::Mul),
            Some(Token::Sym("/")) => Some(BinOp::Div),
            Some(Token::Sym("%")) => Some(BinOp::Mod),
            _ => None,
        } {
            *i += 1;
            let r = self.operand(t, i)?;
            l = self.bin(op, l, r);
        }
        Ok(l)
    }

    //nt: 下标读与切片
    fn index_read(
        &mut self,
        id: LocalId,
        vty: String,
        t: &[Token],
        i: &mut usize,
    ) -> Result<Value, BuildError> {
        let mut depth = 0i32;
        let mut close = None;
        for (k, tk) in t.iter().enumerate().skip(*i) {
            match tk {
                Token::Sym("[") => depth += 1,
                Token::Sym("]") => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let close = close.ok_or_else(|| BuildError { message: "下标缺 ]".into() , code: "E302".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
        if t[*i + 1..close].iter().any(|tk| *tk == Token::Sym("~")) {
            let pos = t[*i + 1..close].iter().position(|tk| *tk == Token::Sym("~")).unwrap();
            let a = self.expr(&t[*i + 1..*i + 1 + pos])?;
            let b2 = self.expr(&t[*i + 1 + pos + 1..close])?;
            *i = close + 1;
            let st = self.temp(Ty(vty.clone()));
            self.emit(Stmt::Assign {
                lhs: Place::local(st),
                rhs: Rvalue::Call {
                    name: "slice".into(),
                    args: vec![Value::Local(id), a, b2],
                },
            });
            return Ok(Value::Local(st));
        }
        let idx = self.expr(&t[*i + 1..close])?;
        *i = close + 1;
        if vty == "str" {
            let tmp = self.temp(Ty("str".into()));
            self.emit(Stmt::Assign {
                lhs: Place::local(tmp),
                rhs: Rvalue::Call { name: "nth_char".into(), args: vec![Value::Local(id), idx] },
            });
            return Ok(Value::Local(tmp));
        }
        let is_map = vty == "map" || vty.starts_with("map(");
        let tmp = self.temp(Ty(if is_map {
            Self::map_val_ty(&vty).unwrap_or_else(|| "int".into())
        } else {
            elem_ty_of(&vty).unwrap_or_else(|| "int".into())
        }));
        self.emit(Stmt::Assign {
            lhs: Place::local(tmp),
            rhs: if is_map {
                Rvalue::MapGet { map: Value::Local(id), key: idx }
            } else {
                Rvalue::ListGet { list: Value::Local(id), index: idx }
            },
        });
        if matches!(t.get(*i), Some(Token::Sym("."))) {
            let ety = self.f.locals.get(tmp).map(|l| l.ty.0.clone()).unwrap_or_default();
            return self.field_chain(tmp, ety, t, i);
        }
        Ok(Value::Local(tmp))
    }

    fn operand(&mut self, t: &[Token], i: &mut usize) -> Result<Value, BuildError> {
        match t.get(*i) {
            //nt: 一元负号
            Some(Token::Sym("-")) => {
                //nt: 整数与浮点字面量都收
                if let Some(Token::Int(n)) = t.get(*i + 1) {
                    *i += 2;
                    Ok(Value::Int(-n))
                } else if let Some(Token::Float(x)) = t.get(*i + 1) {
                    *i += 2;
                    Ok(Value::Float(-x))
                } else {
                    err("一元负号后要跟数字字面量".to_string())
                }
            }
            Some(Token::Int(n)) => {
                *i += 1;
                Ok(Value::Int(*n))
            }
            Some(Token::Float(x)) => {
                *i += 1;
                Ok(Value::Float(*x))
            }
            Some(Token::Str(s)) => {
                *i += 1;
                Ok(Value::Str(s.clone()))
            }
//nt: 原样串
            Some(Token::RawStr(s)) => {
                *i += 1;
                Ok(Value::Str(s.clone()))
            }
//nt: 逻辑非
            //nt: 最简实现
            Some(Token::Kw("not")) => {
                *i += 1;
                let v = self.operand(t, i)?;
                let t2 = self.temp(Ty("bool".into()));
                self.emit(Stmt::Assign {
                    lhs: Place::local(t2),
                    rhs: Rvalue::BinOp { op: BinOp::Eq, lhs: v, rhs: Value::Bool(false) },
                });
                Ok(Value::Local(t2))
            }
            Some(Token::Kw("re")) => {
                *i += 1;
                Ok(Value::Bool(true))
            }
            Some(Token::Kw("fa")) => {
                *i += 1;
                Ok(Value::Bool(false))
            }
            Some(Token::Kw("null")) => {
                *i += 1;
                Ok(Value::Null)
            }
            //nt: 括号表达式
            Some(Token::Sym("(")) => {
                let mut d = 0i32;
                let mut end = None;
                for (k, tk) in t.iter().enumerate().skip(*i) {
                    match tk {
                        Token::Sym("(") => d += 1,
                        Token::Sym(")") => {
                            d -= 1;
                            if d == 0 {
                                end = Some(k);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let end = end.ok_or_else(|| BuildError { message: "括号没闭合".into() , code: "E301".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
                let inner: Vec<Token> = t[*i + 1..end].to_vec();
                *i = end + 1;
                self.expr(&inner)
            }
            //nt: take #
            Some(Token::Kw("take")) => {
                let base = *i + 1;
                if !matches!(t.get(base), Some(Token::HashColon)) {
                    return err("take 后接 `#:块 val \"名\"`——形如 `take #:工具块 val \"常数\"`");
                }
                let blk = match t.get(base + 1) {
                    Some(Token::Ident(n)) => n.clone(),
                    Some(Token::Kw(k)) => (*k).to_string(),
                    _ => return err("`take #:` 后应接块名"),
                };
                if !matches!(t.get(base + 2), Some(Token::Kw("val"))) {
                    return err("take 后接 `#:块 val \"名\"`——形如 `take #:工具块 val \"常数\"`");
                }
                let vname = match t.get(base + 3) {
                    Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                    _ => return err("`take #:块 val` 后应接 val 名"),
                };
                let slot_key = format!("{}|{}#{}", self.module, blk, vname);
                let sid = match self.槽或建(&slot_key) {
                    Some(id) => id,
                    None => {
                        return Err(BuildError {
                            message: format!(
                                "块「{}」的 val「{}」不存在——先 add val 建它，sle 块还要先 call #:{}",
                                blk, vname, blk
                            ),
                            code: "E309".into(),
                            line: CUR_LINE.with(|c| c.get()),
                            file: String::new(),
                        })
                    }
                };
                *i = base + 4;
                let sty = self.f.locals.get(sid).map(|l| l.ty.0.clone()).unwrap_or_default();
                let tmp = self.temp(Ty(sty.clone()));
                //nt: 字符串复印
                let copy_call = if sty == "str" {
                    Some(("#str_copy".to_string(), vec![Value::Local(sid)]))
                } else if sty.starts_with("list") {
                    let str_elem = if sty.contains("str") { 1 } else { 0 };
                    Some(("#dc_list".to_string(), vec![Value::Local(sid), Value::Int(str_elem)]))
                } else if sty.starts_with("map") {
                    let str_val = if sty.contains("str") { 1 } else { 0 };
                    Some(("#dc_map".to_string(), vec![Value::Local(sid), Value::Int(str_val)]))
                } else {
                    None
                };
                match copy_call {
                    Some((name, args)) => {
                        self.emit(Stmt::Assign {
                            lhs: Place::local(tmp),
                            rhs: Rvalue::Call { name, args },
                        });
                    }
                    None => {
                        self.emit(Stmt::Assign { lhs: Place::local(tmp), rhs: Rvalue::Use(Place::local(sid)) });
                    }
                }
                Ok(Value::Local(tmp))
            }
            //nt: 动词位互斥
            Some(Token::Kw("call")) if matches!(t.get(*i + 1), Some(Token::Kw("take"))) => {
                err("call 与 take 不得同时出现——要引用槽写 `call #:块 val \"名\"`，要复印写 `take #:块 val \"名\"`")
            }
            //nt: 跨块取 val
//nt: 同块取 val
            Some(Token::Kw("call")) if matches!(t.get(*i + 1), Some(Token::Kw("val"))) => {
                let vname = match t.get(*i + 2) {
                    Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                    _ => return err("`call val` 后应接 val 名"),
                };
                //nt: 槽load读
                if let Some(&pid) = self.exposed.get(&vname) {
                    if !matches!(t.get(*i + 3), Some(Token::Sym("("))) {
                        let pty = self
                            .f
                            .locals
                            .get(pid)
                            .map(|l| l.ty.0.clone())
                            .unwrap_or_default();
                        let vty = Ty(pty.trim_end_matches('*').to_string());
                        let tmp = self.new_local(None, vty.clone(), false, false);
                        self.emit(Stmt::Assign {
                            lhs: Place::local(tmp),
                            rhs: Rvalue::Use(Place { base: pid, proj: vec![Proj::Deref] }),
                        });
                        *i += 3;
                        if matches!(t.get(*i), Some(Token::Sym("."))) {
                            return self.field_chain(tmp, vty.0.clone(), t, i);
                        }
                        if matches!(t.get(*i), Some(Token::Sym("["))) {
                            return self.index_read(tmp, vty.0.clone(), t, i);
                        }
                        return Ok(Value::Local(tmp));
                    }
                }
                let id = *self.names.get(&vname).ok_or_else(|| BuildError {
                    message: format!("本块没有 val「{}」——同块取 val 取的是本块的", vname),
                    code: "E304".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                })?;
                //nt: 间接调用
                if matches!(t.get(*i + 3), Some(Token::Sym("("))) {
                    let fty = self
                        .f
                        .locals
                        .get(id)
                        .map(|l| l.ty.0.clone())
                        .unwrap_or_default();
                    let ret = fn_ret_of(&fty).ok_or_else(|| BuildError {
                        message: format!("val「{}」不是函数类型——不能直接调", vname),
                        code: "E201".into(),
                        line: CUR_LINE.with(|c| c.get()),
                        file: String::new(),
                    })?;
                    *i += 4;
                    let mut depth = 1i32;
                    let mut inner: Vec<Token> = Vec::new();
                    while *i < t.len() {
                        match &t[*i] {
                            Token::Sym("(") => depth += 1,
                            Token::Sym(")") => {
                                depth -= 1;
                                if depth == 0 {
                                    *i += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        inner.push(t[*i].clone());
                        *i += 1;
                    }
                    if depth != 0 {
                        return err("调用的括号没闭合");
                    }
                    let mut vals = Vec::new();
                    for a in split_args(&inner) {
                        vals.push(self.expr(&a)?);
                    }
                    let tmp = self.temp(Ty(ret));
                    self.emit(Stmt::Assign {
                        lhs: Place::local(tmp),
                        rhs: Rvalue::CallIndirect { callee: Value::Local(id), args: vals },
                    });
                    return Ok(Value::Local(tmp));
                }
                *i += 3;
                let vty = self
                    .f
                    .locals
                    .get(id)
                    .map(|l| l.ty.0.clone())
                    .unwrap_or_default();
                if matches!(t.get(*i), Some(Token::Sym("."))) {
                    return self.field_chain(id, vty, t, i);
                }
                if matches!(t.get(*i), Some(Token::Sym("["))) {
                    return self.index_read(id, vty, t, i);
                }
                return Ok(Value::Local(id));
            }
            //nt: call #
            //nt: 合成一支
            //nt: 只接 **val 取
            //nt: fn 取用交给后面的
            Some(Token::Kw("call"))
                if (matches!(t.get(*i + 1), Some(Token::HashColon))
                    && matches!(t.get(*i + 3), Some(Token::Kw("val"))))
                    || (matches!(t.get(*i + 1), Some(Token::HashHashColon))
                        && take_kind(t) == Some("val")) =>
            {
                let mut colon_at: Option<usize> = None; // `#:` 的位置（跨模块取用用；模块名长度可变）
                let (blk_mod, blk) = if matches!(t.get(*i + 1), Some(Token::HashColon)) {
                    let blk = match t.get(*i + 2) {
                        Some(Token::Ident(n)) => n.clone(),
                        Some(Token::Kw(k)) => (*k).to_string(),
                        _ => return err("`call #:` 后应接块名"),
                    };
                    (self.module.clone(), blk)
                } else {
                    //nt: 模块名拼接
                    let mut m = String::new();
                    let mut j = 2usize;
                    while let Some(tok) = t.get(j) {
                        match tok {
                            Token::Ident(n) | Token::Str(n) => {
                                if !m.is_empty() {
                                    m.push('.');
                                }
                                m.push_str(n);
                                j += 1;
                                if matches!(t.get(j), Some(Token::Sym("."))) {
                                    j += 1;
                                } else {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                    if m.is_empty() || !matches!(t.get(j), Some(Token::HashColon)) {
                        return err("`##:` 后应接模块名再接 `#:块`——形如 `##:前端.拆词 #:分句`");
                    }
                    let blk = match t.get(j + 1) {
                        Some(Token::Ident(n)) => n.clone(),
                        Some(Token::Kw(k)) => (*k).to_string(),
                        _ => return err("`##:模块 #:` 后应接块名"),
                    };
                    colon_at = Some(j);
                    //nt: 尾名退回
                    let full = if self.mod_has(&m) {
                        m.clone()
                    } else {
                        match self.resolve_module(&m)? {
                            Some(f) => f,
                            None => return err(format!("模块「{}」找不到", m)),
                        }
                    };
                    (full, blk)
                };
                //nt: val 名
                let (vname, adv) = match colon_at {
                    None => (
                        match t.get(*i + 4) {
                            Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                            _ => return err("`call #:块 val` 后应接 val 名"),
                        },
                        5,
                    ),
                    Some(j) => (
                        match t.get(j + 3) {
                            Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                            _ => return err("`call ##:模块 #:块 val` 后应接 val 名"),
                        },
                        j + 4,
                    ),
                };
//nt: 跨模块取 val ⇒
                self.check_pub(&blk_mod, &blk)?;
                let slot_key = format!("{}|{}#{}", blk_mod, blk, vname);
                let sid = match self.槽或建(&slot_key) {
                    Some(id) => id,
                    None => {
                        return Err(BuildError {
                            message: format!(
                                "块「{}」的 val「{}」不存在——先 add val 建它，sle 块还要先 call #:{}",
                                blk, vname, blk
                            ),
                            code: "E309".into(),
                            line: CUR_LINE.with(|c| c.get()),
                            file: String::new(),
                        })
                    }
                };
                *i += adv;
                Ok(Value::Local(sid))
            }
            //nt: fn 调用
            Some(Token::Kw("call")) if matches!(t.get(*i + 1), Some(Token::Kw("fn"))) => {
                let fname = match t.get(*i + 2) {
                    Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                    _ => return err("`call fn` 后应接函数名"),
                };
                if !matches!(t.get(*i + 3), Some(Token::Sym("("))) {
                    return err("调用要写实参表 `(…)`");
                }
                let key = format!("{}|{}|{}", self.module, self.block, fname);
                //nt: 泛型 fn
                if !self.fns.contains_key(&key) && !self.global_fns.contains_key(&key) {
                    let 模板 = 泛型fn表.with(|g| g.borrow().get(&fname).cloned());
                    if let Some((带能力, 形参, 返回, 体, _模, _块)) = 模板 {
                        let 类型参数: Vec<String> = 带能力.iter().map(|(n, _)| n.clone()).collect();
                        let 实参段 = 取实参段(t, *i + 3);
                        let mut 绑定: Vec<Option<String>> = vec![None; 类型参数.len()];
                        for (k, a) in 实参段.iter().enumerate() {
                            if k >= 形参.len() {
                                break;
                            }
                            let got = match self.推断类型(a) {
                                Some(g) => g,
                                None => continue,
                            };
                            let pat = 形参[k].1 .0.clone();
                            let _ = 统一类型(&pat, &got, &类型参数, &mut 绑定);
                        }
                        if 绑定.iter().any(|x| x.is_none()) {
                            return err(format!(
                                "调用泛型 fn「{}」时推不出类型参数——实参要写成字面量或带类型的 val",
                                fname
                            ));
                        }
                        let 实参类型: Vec<String> = 绑定.into_iter().map(|x| x.unwrap()).collect();
                        核能力(&fname, &带能力, &实参类型)?;
                        let 实例名 = format!("{}@{}", fname, 实参类型.join(","));
                        if !self.fns.contains_key(&format!("{}|{}|{}", self.module, self.block, 实例名))
                            && !self.global_fns.contains_key(&format!("{}|{}|{}", self.module, self.block, 实例名))
                        {
                            建fn实例(self, &实例名, &类型参数, &实参类型, &形参, &返回, &体)?;
                        }
                        let 实例键 = format!("{}|{}|{}", self.module, self.block, 实例名);
                        if let Some(s) = self
                            .fns
                            .get(&实例键)
                            .or_else(|| self.global_fns.get(&实例键))
                            .cloned()
                        {
                            let _ = s;
                            //nt: 直接把这次调用改写成
                            let mut 新: Vec<Token> = t.to_vec();
                            if let Some(slot) = 新.get_mut(*i + 2) {
                                *slot = Token::Ident(实例名);
                            }
                            return self.operand(&新, i);
                        }
                    }
                    //nt: 泛型实例化
                    let 语句模板 = 泛型语句体fn表.with(|g| g.borrow().get(&fname).cloned());
                    if let Some((带能力, 形参, 返回, 体, _模, _块)) = 语句模板 {
                        let 类型参数: Vec<String> = 带能力.iter().map(|(n, _)| n.clone()).collect();
                        let 实参段 = 取实参段(t, *i + 3);
                        let mut 绑定: Vec<Option<String>> = vec![None; 类型参数.len()];
                        for (k, a) in 实参段.iter().enumerate() {
                            if k >= 形参.len() {
                                break;
                            }
                            let got = match self.推断类型(a) {
                                Some(g) => g,
                                None => continue,
                            };
                            let pat = 形参[k].1 .0.clone();
                            let _ = 统一类型(&pat, &got, &类型参数, &mut 绑定);
                        }
                        if 绑定.iter().any(|x| x.is_none()) {
                            return err(format!(
                                "调用泛型 fn「{}」时推不出类型参数——实参要写成字面量或带类型的 val",
                                fname
                            ));
                        }
                        let 实参类型: Vec<String> = 绑定.into_iter().map(|x| x.unwrap()).collect();
                        核能力(&fname, &带能力, &实参类型)?;
                        let 实例名 = format!("{}@{}", fname, 实参类型.join(","));
                        let 实例键 = format!("{}|{}|{}", self.module, self.block, 实例名);
                        if !self.fns.contains_key(&实例键) && !self.global_fns.contains_key(&实例键) {
                            self.建语句体实例(&实例名, &类型参数, &实参类型, &形参, &返回, &体)?;
                        }
                        if self
                            .fns
                            .get(&实例键)
                            .or_else(|| self.global_fns.get(&实例键))
                            .is_some()
                        {
                            let mut 新: Vec<Token> = t.to_vec();
                            if let Some(slot) = 新.get_mut(*i + 2) {
                                *slot = Token::Ident(实例名);
                            }
                            return self.operand(&新, i);
                        }
                    }
                }
                //nt: 本块表 → 全局表退
                let sig = self
                    .fns
                    .get(&key)
                    .or_else(|| self.global_fns.get(&key))
                    .cloned()
                    .ok_or_else(|| BuildError {
                    message: format!("还没建过 fn「{}」——先用 add fn 建", fname),
                    code: "E304".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                })?;
                *i += 4;
                let mut depth = 1i32;
                let mut inner: Vec<Token> = Vec::new();
                while *i < t.len() {
                    match &t[*i] {
                        Token::Sym("(") => depth += 1,
                        Token::Sym(")") => {
                            depth -= 1;
                            if depth == 0 {
                                *i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    inner.push(t[*i].clone());
                    *i += 1;
                }
                if depth != 0 {
                    return err("调用的括号没闭合");
                }
                let mut vals = Vec::new();
                for a in split_args(&inner) {
                    vals.push(self.expr(&a)?);
                }
                //nt: fn的槽实参
                if !sig.hidden.is_empty() {
                    let caller_is_fn = self.f.name.contains('#');
                    vals.extend(self.槽实参(&sig, &key, caller_is_fn)?);
                }
                let tmp = self.temp(sig.ret.clone());
                //nt: 外部函数 ⇒ C 符
                let call_name = if sig.is_outs { fname.clone() } else { key };
                self.emit(Stmt::Assign {
                    lhs: Place::local(tmp),
                    rhs: Rvalue::Call { name: call_name, args: vals },
                });
                Ok(Value::Local(tmp))
            }
            //nt: 跨块取用
            Some(Token::Kw("call")) if matches!(t.get(*i + 1), Some(Token::HashColon)) => {
                *i += 1; // 跳过 call，落在 `#:`
                let blk = match t.get(*i + 1) {
                    Some(Token::Ident(n)) => n.clone(),
                    Some(Token::Kw(k)) => (*k).to_string(),
                    _ => return err("`#:` 后应接块名"),
                };
                if !matches!(t.get(*i + 2), Some(Token::Kw("fn"))) {
                    return err("跨块取用目前只支持 `#:块 fn \"名\"(… )`");
                }
                let fname = match t.get(*i + 3) {
                    Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                    _ => return err("`#:块 fn` 后应接函数名"),
                };
                if !matches!(t.get(*i + 4), Some(Token::Sym("("))) {
                    return err("跨块取用要写实参表 `(…)`");
                }
                let key = format!("{}|{}|{}", self.module, blk, fname);
                let sig = self.global_fns.get(&key).cloned().ok_or_else(|| BuildError {
                    message: format!("块「{}」里没有 fn「{}」", blk, fname),
                    code: "E304".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                })?;
                *i += 5;
                let mut depth = 1i32;
                let mut inner: Vec<Token> = Vec::new();
                while *i < t.len() {
                    match &t[*i] {
                        Token::Sym("(") => depth += 1,
                        Token::Sym(")") => {
                            depth -= 1;
                            if depth == 0 {
                                *i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    inner.push(t[*i].clone());
                    *i += 1;
                }
                if depth != 0 {
                    return err("跨块调用的括号没闭合");
                }
                let mut vals = Vec::new();
                for a in split_args(&inner) {
                    vals.push(self.expr(&a)?);
                }
                //nt: 跨块槽实参
                if !sig.hidden.is_empty() {
                    vals.extend(self.槽实参(&sig, &key, true)?);
                }
                let tmp = self.temp(sig.ret.clone());
                let call_name = if sig.is_outs { fname.clone() } else { key };
                self.emit(Stmt::Assign {
                    lhs: Place::local(tmp),
                    rhs: Rvalue::Call { name: call_name, args: vals },
                });
                Ok(Value::Local(tmp))
            }
            //nt: 跨模块取用
//nt: 跨模块引用直接写来源
            //nt: 走点分模块
            Some(Token::Kw("call"))
                if matches!(t.get(*i + 1), Some(Token::HashHashColon))
                    && hh_is_fn_form(t, *i + 2) =>
            {
                *i += 1; // 跳过 call，落在 `##:`
                //nt: 模块名 = 点分链
                let mut module = String::new();
                let mut j = *i + 1;
                while let Some(tok) = t.get(j) {
                    match tok {
                        Token::Ident(n) | Token::Str(n) => {
                            if !module.is_empty() {
                                module.push('.');
                            }
                            module.push_str(n);
                            j += 1;
                            if matches!(t.get(j), Some(Token::Sym("."))) {
                                j += 1;
                            } else {
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                if module.is_empty() || !matches!(t.get(j), Some(Token::HashColon)) {
                    return err("`##:` 后应接模块名再接 `#:块`——形如 `##:前端.拆词 #:分句`");
                }
                let blk = match t.get(j + 1) {
                    Some(Token::Ident(n)) => n.clone(),
                    Some(Token::Kw(k)) => (*k).to_string(),
                    _ => return err("`##:模块 #:` 后应接块名"),
                };
                if !matches!(t.get(j + 2), Some(Token::Kw("fn"))) {
                    return err("跨模块取用目前只支持 `##:模块 #:块 fn \"名\"(… )`");
                }
                let fname = match t.get(j + 3) {
                    Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                    _ => return err("`##:模块 #:块 fn` 后应接函数名"),
                };
                if !matches!(t.get(j + 4), Some(Token::Sym("("))) {
                    return err("跨模块取用要写实参表 `(…)`");
                }
                let key = format!("{}|{}|{}", module, blk, fname);
//nt: 模块路径尾名退回
                let key = if self.global_fns.contains_key(&key) {
                    key
                } else if let Some(full) = self.resolve_module(&module)? {
                    format!("{}|{}|{}", full, blk, fname)
                } else {
                    //nt: 模块压根没装载
                    return err(format!(
                        "模块「{}」找不到——单文件编译不装载模块；请按目录工程编（入口放 main.an），或检查模块文件在不在",
                        module
                    ));
                };
//nt: 跨模块 ⇒ pub
                let full_mod = key.split('|').next().unwrap_or(&module).to_string();
                self.check_pub(&full_mod, &blk)?;
                if self
                    .hidden_items
                    .contains(&format!("{}|{}|fn|{}", full_mod, blk, fname))
                {
                    return Err(BuildError {
                        message: format!(
                            "fn「{}」在块「{}」（模块 {}）里被 hide 了——它的公开面里没有这项",
                            fname, blk, full_mod
                        ),
                        code: "E201".into(),
                        line: CUR_LINE.with(|c| c.get()),
                        file: String::new(),
                    });
                }
                let mut 调用键 = key.clone();
                let sig = match self.global_fns.get(&key).cloned().or_else(|| self.fns.get(&key).cloned()) {
                    Some(s) => s,
                    None => {
                        let 模板 = 泛型语句体fn表.with(|g| g.borrow().get(&fname).cloned());
                        let Some((带能力, 形参, 返回, 体, _模, _块)) = 模板 else {
                            return Err(BuildError {
                                message: format!("模块「{}」的块「{}」里没有 fn「{}」", module, blk, fname),
                                code: "E304".into(),
                                line: CUR_LINE.with(|c| c.get()),
                                file: String::new(),
                            });
                        };
                        let 类型参数: Vec<String> = 带能力.iter().map(|(n, _)| n.clone()).collect();
                        let 实参段 = 取实参段(t, j + 4);
                        let mut 绑定: Vec<Option<String>> = vec![None; 类型参数.len()];
                        for (k, a) in 实参段.iter().enumerate() {
                            if k >= 形参.len() {
                                break;
                            }
                            let got = match self.推断类型(a) {
                                Some(g) => g,
                                None => continue,
                            };
                            let pat = 形参[k].1 .0.clone();
                            let _ = 统一类型(&pat, &got, &类型参数, &mut 绑定);
                        }
                        if 绑定.iter().any(|x| x.is_none()) {
                            return err(format!(
                                "调用泛型 fn「{}」时推不出类型参数——实参要写成字面量或带类型的 val",
                                fname
                            ));
                        }
                        let 实参类型: Vec<String> = 绑定.into_iter().map(|x| x.unwrap()).collect();
                        核能力(&fname, &带能力, &实参类型)?;
                        let 实例名 = format!("{}@{}", fname, 实参类型.join(","));
                        let 实例键 = format!("{}|{}|{}", full_mod, blk, 实例名);
                        if self.fns.get(&实例键).is_none() && self.global_fns.get(&实例键).is_none() {
                            let 旧模块 = self.module.clone();
                            let 旧块 = self.block.clone();
                            self.module = full_mod.clone();
                            self.block = blk.clone();
                            let r = self.建语句体实例(
                                &实例名, &类型参数, &实参类型, &形参, &返回, &体,
                            );
                            self.module = 旧模块;
                            self.block = 旧块;
                            r?;
                        }
                        调用键 = 实例键.clone();
                        self.fns.get(&实例键).cloned().ok_or_else(|| BuildError {
                            message: format!("模块「{}」的块「{}」里没有 fn「{}」", module, blk, fname),
                            code: "E304".into(),
                            line: CUR_LINE.with(|c| c.get()),
                            file: String::new(),
                        })?
                    }
                };
                *i = j + 5;
                let mut depth = 1i32;
                let mut inner: Vec<Token> = Vec::new();
                while *i < t.len() {
                    match &t[*i] {
                        Token::Sym("(") => depth += 1,
                        Token::Sym(")") => {
                            depth -= 1;
                            if depth == 0 {
                                *i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    inner.push(t[*i].clone());
                    *i += 1;
                }
                if depth != 0 {
                    return err("跨模块调用的括号没闭合");
                }
                let mut vals = Vec::new();
                for a in split_args(&inner) {
                    vals.push(self.expr(&a)?);
                }
                //nt: 跨模块槽实参
                if !sig.hidden.is_empty() {
                    vals.extend(self.槽实参(&sig, &调用键, true)?);
                }
                let tmp = self.temp(sig.ret.clone());
                let call_name = if sig.is_outs { fname.clone() } else { 调用键 };
                self.emit(Stmt::Assign {
                    lhs: Place::local(tmp),
                    rhs: Rvalue::Call { name: call_name, args: vals },
                });
                Ok(Value::Local(tmp))
            }
            //nt: 跨块须写 call
            Some(Token::HashColon) => {
                err("跨块取用要写 `call`——形如 `call #:块 fn \"名\"(…)`")
            }
//nt: 类型名当函数
            Some(Token::Kw(k)) if matches!(t.get(*i + 1), Some(Token::Sym("("))) => {
                let name = (*k).to_string();
                *i += 2;
                let mut depth = 1i32;
                let mut inner: Vec<Token> = Vec::new();
                while *i < t.len() {
                    match &t[*i] {
                        Token::Sym("(") => depth += 1,
                        Token::Sym(")") => {
                            depth -= 1;
                            if depth == 0 {
                                *i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    inner.push(t[*i].clone());
                    *i += 1;
                }
                if depth != 0 {
                    return err("调用括号没闭合");
                }
                let mut vals = Vec::new();
                //nt: 传元素类型
                let pending = if name == "que" || name == "stack" {
                    self.lit_ty.take()
                } else {
                    None
                };
                if let Some(src) = pending.as_ref() {
                    if let Some(elem) = 序列元素类型(&src.0) {
                        self.lit_ty = Some(Ty(format!("list({})", elem)));
                    }
                }
                for a in split_args(&inner) {
                    vals.push(self.expr(&a)?);
                }
                if pending.is_some() {
                    let _ = self.lit_ty.take();
                }
                let tmp_ty = pending.unwrap_or_else(|| builtin_ret_ty(&name));
                let tmp = self.temp(tmp_ty);
                self.emit(Stmt::Assign { lhs: Place::local(tmp), rhs: Rvalue::Call { name, args: vals } });
                Ok(Value::Local(tmp))
            }
            //nt: map 字面量
            Some(Token::Sym("{")) => {
                let mut depth = 0i32;
                let mut end = None;
                for (k, tk) in t.iter().enumerate().skip(*i) {
                    match tk {
                        Token::Sym("{") => depth += 1,
                        Token::Sym("}") => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(k);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let end = end.ok_or_else(|| BuildError { message: "map 字面量缺 }".into() , code: "E302".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
                let inner = t[*i + 1..end].to_vec();
                *i = end + 1;
                let mut pairs: Vec<(MapKey, Value)> = Vec::new();
                for seg in split_args(&inner) {
                    //nt: 键收str与int
                    let k = match seg.first() {
                        Some(Token::Str(k)) => MapKey::Str(k.clone()),
                        Some(Token::Int(n)) => MapKey::Int(*n),
                        _ => return err("map 字面量的键须是字符串或整数字面量"),
                    };
                    if !matches!(seg.get(1), Some(Token::Sym(":"))) {
                        return err("map 字面量键值之间缺 :");
                    }
                    pairs.push((k, self.expr(&seg[2..])?));
                }
                let lty = self.lit_ty.take().unwrap_or_else(|| Ty("map".into()));
                let tmp = self.temp(lty);
                self.emit(Stmt::Assign {
                    lhs: Place::local(tmp),
                    rhs: Rvalue::MapLit { pairs },
                });
                Ok(Value::Local(tmp))
            }
            //nt: 列表字面量
            Some(Token::Sym("[")) => {
                let start = *i;
                let mut depth = 0i32;
                let mut end = None;
                for (k, tk) in t.iter().enumerate().skip(start) {
                    match tk {
                        Token::Sym("[") => depth += 1,
                        Token::Sym("]") => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(k);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let end = end.ok_or_else(|| BuildError { message: "列表字面量缺 ]".into() , code: "E302".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
                let inner = t[start + 1..end].to_vec();
                *i = end + 1;
                let mut elems = Vec::new();
                for a in split_args(&inner) {
                    let v = self.expr(&a)?;
//nt: 第②道关卡
                    if self.is_container_value(&v) {
                        return err("列表里不能直接装容器（`[[1], [2]]` 不行）——要分组请用键值表（如 map(str, list(int))）或结构体字段装列表");
                    }
                    elems.push(v);
                }
                let lty = self.lit_ty.take().unwrap_or_else(|| Ty("list".into()));
                let tmp = self.temp(lty);
                self.emit(Stmt::Assign { lhs: Place::local(tmp), rhs: Rvalue::ListLit { elems } });
                Ok(Value::Local(tmp))
            }
//nt: 显式取 val 当前
            //nt: 取值必须带类别标签
            Some(Token::Kw("val")) => {
                let name = match t.get(*i + 1) {
                    Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                    _ => return err("`val` 后应接名字"),
                };
                *i += 2;
                //nt: 本块直取或槽load
                //nt: 槽load 后同走投影
                let id = match self.names.get(&name) {
                    Some(id) => *id,
                    None => {
                        if let Some(&pid) = self.exposed.get(&name) {
                            let pty = self.f.locals.get(pid).map(|l| l.ty.0.clone()).unwrap_or_default();
                            let vty = Ty(pty.trim_end_matches('*').to_string());
                            let tmp = self.temp(vty.clone());
                            self.emit(Stmt::Assign {
                                lhs: Place::local(tmp),
                                rhs: Rvalue::Use(Place { base: pid, proj: vec![Proj::Deref] }),
                            });
                            tmp
                        } else {
                            return err(format!("val「{}」没建过", name));
                        }
                    }
                };
                //nt: 字段投影
                let vty = self
                    .f
                    .locals
                    .get(id)
                    .map(|l| l.ty.0.clone())
                    .unwrap_or_default();
                if matches!(t.get(*i), Some(Token::Sym("."))) {
                    //nt: field_chai
                    return self.field_chain(id, vty, t, i);
                }
                //nt: 下标投影
                if matches!(t.get(*i), Some(Token::Sym("["))) {
                    return self.index_read(id, vty, t, i);
                }
                //nt: delta 后缀
                if matches!(t.get(*i), Some(Token::Sym("("))) {
                    let mut depth = 0i32;
                    let mut close = None;
                    for (k, tk) in t.iter().enumerate().skip(*i) {
                        match tk {
                            Token::Sym("(") => depth += 1,
                            Token::Sym(")") => {
                                depth -= 1;
                                if depth == 0 {
                                    close = Some(k);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    let close = close.ok_or_else(|| BuildError { message: "delta 括号没闭合".into() , code: "E302".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
                    let op = match t.get(*i + 1) {
                        Some(Token::Sym("+")) => BinOp::Add,
                        Some(Token::Sym("-")) => BinOp::Sub,
                        Some(Token::Sym("*")) => BinOp::Mul,
                        Some(Token::Sym("/")) => BinOp::Div,
                        _ => return err("delta 括号应以运算符开头（如 `(+1)`、`(+ 表达式)`）"),
                    };
                    let rhs = self.expr(&t[*i + 2..close])?;
                    let nv = self.bin(op, Value::Local(id), rhs);
                    *i = close + 1;
                    return Ok(nv);
                }
                Ok(Value::Local(id))
            }
            Some(Token::Kw("fn")) if matches!(t.get(*i + 1), Some(Token::Str(_) | Token::Ident(_)))
                && !matches!(t.get(*i + 2), Some(Token::Sym("("))) =>
            {
                let fname = match t.get(*i + 1) {
                    Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                    _ => return err("`fn` 后应接函数名"),
                };
                *i += 2;
                let key = format!("{}|{}|{}", self.module, self.block, fname);
                let sig = self
                    .fns
                    .get(&key)
                    .or_else(|| self.global_fns.get(&key))
                    .cloned()
                    .ok_or_else(|| BuildError {
                        message: format!("还没建过 fn「{}」——先用 add fn 建", fname),
                        code: "E304".into(),
                        line: CUR_LINE.with(|c| c.get()),
                        file: String::new(),
                    })?;
                let ps: Vec<String> = sig.params.iter().map(|t| t.0.clone()).collect();
                let tmp = self.temp(Ty(format!("fn({}) {}", ps.join(", "), sig.ret.0)));
                self.emit(Stmt::Assign {
                    lhs: Place::local(tmp),
                    rhs: Rvalue::FnRef { name: key },
                });
                Ok(Value::Local(tmp))
            }
            Some(Token::Ident(n)) => {
                //nt: fn 参数 / fo
                if let Some(bid) = self.names.get(n) {
                    if !self.f.locals.get(*bid).map(|l| l.bare_ok).unwrap_or(false) {
                        return err(format!(
                            "裸名「{}」——取 val 请写 val \"{}\"（全面显式：fn 与 val 可重名，裸名消不了歧义）",
                            n, n
                        ));
                    }
                }
                //nt: 下标读
                if matches!(t.get(*i + 1), Some(Token::Sym("["))) {
                    let base = *self
                        .names
                        .get(n)
                        .ok_or_else(|| BuildError { message: format!("还没建过「{}」", n) , code: "E304".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
                    let open = *i + 1;
                    let mut depth = 0i32;
                    let mut close = None;
                    for (k, tk) in t.iter().enumerate().skip(open) {
                        match tk {
                            Token::Sym("[") => depth += 1,
                            Token::Sym("]") => {
                                depth -= 1;
                                if depth == 0 {
                                    close = Some(k);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    let close = close.ok_or_else(|| BuildError { message: "下标缺 ]".into() , code: "E302".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
                    let idx_toks = t[open + 1..close].to_vec();
                    *i = close + 1;
                    //nt: map 键读
                    let base_is_map = self
                        .f
                        .locals
                        .get(base)
                        .map(|l| l.ty.0 == "map" || l.ty.0.starts_with("map("))
                        .unwrap_or(false);
                    if base_is_map {
                        let key = self.expr(&idx_toks)?;
                        //nt: map 下标读的结果
//nt: 类型按实际表达式推断
                        let vty = self
                            .f
                            .locals
                            .get(base)
                            .and_then(|l| Self::map_val_ty(&l.ty.0))
                            .unwrap_or_else(|| "int".into());
                        let tmp = self.temp(Ty(vty));
                        self.emit(Stmt::Assign {
                            lhs: Place::local(tmp),
                            rhs: Rvalue::MapGet { map: Value::Local(base), key },
                        });
                        return Ok(Value::Local(tmp));
                    }
                    //nt: 字符串下标 / 切片
                    let base_is_str = self
                        .f
                        .locals
                        .get(base)
                        .map(|l| l.ty.0 == "str")
                        .unwrap_or(false);
                    if base_is_str {
                        if idx_toks.contains(&Token::Sym("~")) {
                            let pos = idx_toks.iter().position(|tk| *tk == Token::Sym("~")).unwrap();
                            let a = self.expr(&idx_toks[..pos])?;
                            let b = self.expr(&idx_toks[pos + 1..])?;
                            let st = self.temp(Ty("str".into()));
                            self.emit(Stmt::Assign {
                                lhs: Place::local(st),
                                rhs: Rvalue::Call {
                                    name: "sub_str".into(),
                                    args: vec![Value::Local(base), a, b],
                                },
                            });
                            return Ok(Value::Local(st));
                        }
                        let idx = self.expr(&idx_toks)?;
                        let st = self.temp(Ty("str".into()));
                        self.emit(Stmt::Assign {
                            lhs: Place::local(st),
                            rhs: Rvalue::Call {
                                name: "nth_char".into(),
                                args: vec![Value::Local(base), idx],
                            },
                        });
                        return Ok(Value::Local(st));
                    }
                    //nt: 切片
                    //nt: 队列/栈只进只出
                    let slice_base_filo = self
                        .f
                        .locals
                        .get(base)
                        .map(|l| l.ty.0.starts_with("que") || l.ty.0.starts_with("stack"))
                        .unwrap_or(false);
                    if slice_base_filo {
                        return err("队列/栈只进只出——切片只对列表用");
                    }
                    if idx_toks.contains(&Token::Sym("~")) {
                        let pos = idx_toks.iter().position(|tk| *tk == Token::Sym("~")).unwrap();
                        let a = self.expr(&idx_toks[..pos])?;
                        let b = self.expr(&idx_toks[pos + 1..])?;
                        //nt: 切片结果类型 =
//nt: 类型按实际表达式推断
                        let base_ty = self
                            .f
                            .locals
                            .get(base)
                            .map(|l| l.ty.0.clone())
                            .unwrap_or_else(|| "list".into());
                        let st = self.temp(Ty(base_ty));
                        self.emit(Stmt::Assign {
                            lhs: Place::local(st),
                            rhs: Rvalue::Call {
                                name: "slice".into(),
                                args: vec![Value::Local(base), a, b],
                            },
                        });
                        return Ok(Value::Local(st));
                    }
                    let base_ty = self
                        .f
                        .locals
                        .get(base)
                        .map(|l| l.ty.0.clone())
                        .unwrap_or_default();
                    let ety = elem_ty_of(&base_ty).unwrap_or_else(|| "int".into());
                    let tmp = self.temp(Ty(ety.clone()));
                    let idx = self.expr(&idx_toks)?;
                    self.emit(Stmt::Assign {
                        lhs: Place::local(tmp),
                        rhs: Rvalue::ListGet { list: Value::Local(base), index: idx },
                    });
                    //nt: 下标读后接字段
                    if matches!(t.get(*i), Some(Token::Sym("."))) {
                        return self.field_chain(tmp, ety, t, i);
                    }
                    return Ok(Value::Local(tmp));
                }
//nt: 字段读
                //nt: 每级经把手先判空
                if matches!(t.get(*i + 1), Some(Token::Sym("."))) {
                    let cur = *self
                        .names
                        .get(n)
                        .ok_or_else(|| BuildError { message: format!("还没建过「{}」", n) , code: "E304".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
                    let cur_ty = self
                        .f
                        .locals
                        .get(cur)
                        .map(|l| l.ty.0.clone())
                        .unwrap_or_default();
                    *i += 1; // 指向 "."
                    return self.field_chain(cur, cur_ty, t, i);
                }
                //nt: 积类型构造
                if matches!(t.get(*i + 1), Some(Token::Sym("(")))
                    && (self.type_idx.contains_key(n) || 泛型判别(n))
                {
                    *i += 2; // 跳过 类型名 与 (
                    let mut depth = 1i32;
                    let mut inner: Vec<Token> = Vec::new();
                    while *i < t.len() {
                        match &t[*i] {
                            Token::Sym("(") => depth += 1,
                            Token::Sym(")") => {
                                depth -= 1;
                                if depth == 0 {
                                    *i += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        inner.push(t[*i].clone());
                        *i += 1;
                    }
                    if depth != 0 {
                        return err("构造括号没闭合");
                    }
                    let args = split_args(&inner);
                    //nt: 泛型名 ⇒ 按实参类
                    let 键 = if 泛型判别(n) {
                        self.反解实例(n, &args)?
                    } else {
                        n.clone()
                    };
                    let st = self.types[self.type_idx[&键]].clone();
                    if args.len() != st.fields.len() {
                        return err(format!(
                            "类型「{}」有 {} 个字段，构造给了 {} 个",
                            st.name,
                            st.fields.len(),
                            args.len()
                        ));
                    }
                    let sid = self.new_local(None, Ty(st.name.clone()), false, false);
                    for (k, a) in args.iter().enumerate() {
                        let v = self.expr(a)?;
                        self.emit(Stmt::Assign {
                            lhs: Place { base: sid, proj: vec![Proj::Field(st.fields[k].0.clone())] },
                            rhs: Rvalue::Const(v),
                        });
                    }
                    return Ok(Value::Local(sid));
                }
                //nt: 内置表达式调用
                const BUILTIN_EXPRS: &[&str] = &[
                    "len", "len_utf8", "empty", "has", "index", "concat",
                    "code", "char", "que", "stack", "args", "read_file", "keys", "vals", "slice",
                    "read_8", "read_16", "read_32", "read_64",
                    //nt: 时间与随机 / FF
                    "rnd", "str_to_c", "str_from_c", "save_file",
                ];
                if matches!(t.get(*i + 1), Some(Token::Sym("(")))
                    && BUILTIN_EXPRS.contains(&n.as_str())
                {
                    let name = n.clone();
                    *i += 2;
                    let mut depth = 1i32;
                    let mut inner: Vec<Token> = Vec::new();
                    while *i < t.len() {
                        match &t[*i] {
                            Token::Sym("(") => depth += 1,
                            Token::Sym(")") => {
                                depth -= 1;
                                if depth == 0 {
                                    *i += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        inner.push(t[*i].clone());
                        *i += 1;
                    }
                    if depth != 0 {
                        return err("调用括号没闭合");
                    }
                    let mut vals = Vec::new();
                    for a in split_args(&inner) {
                        vals.push(self.expr(&a)?);
                    }
                    //nt: 串首参改写
                    let mut name = name.clone();
                    if name == "index" {
                        let 首参串 = match vals.first() {
                            Some(Value::Local(id)) => self
                                .f
                                .locals
                                .get(*id)
                                .map(|l| l.ty.0 == "str")
                                .unwrap_or(false),
                            Some(Value::Str(_)) => true,
                            _ => false,
                        };
                        if 首参串 {
                            name = "str_index".into();
                        }
                    }
                    //nt: slice返回同型
                    let 返型 = if name == "slice" {
                        match vals.first() {
                            Some(Value::Local(id)) => self
                                .f
                                .locals
                                .get(*id)
                                .map(|l| l.ty.0.clone())
                                .unwrap_or_else(|| "int".into()),
                            _ => "int".into(),
                        }
                    } else {
                        builtin_ret_ty(&name).0
                    };
                    let tmp = self.temp(Ty(返型));
                    self.emit(Stmt::Assign { lhs: Place::local(tmp), rhs: Rvalue::Call { name, args: vals } });
                    return Ok(Value::Local(tmp));
                }
                //nt: 裸名调用已废止
                if matches!(t.get(*i + 1), Some(Token::Sym("("))) && !self.sum_variants.contains_key(n) {
                    let key = format!("{}|{}", self.block, n);
                    let outs_sig = self
                        .fns
                        .get(&key)
                        .filter(|s| s.is_outs)
                        .cloned()
                        .or_else(|| self.global_fns.get(&key).filter(|s| s.is_outs).cloned());
                    if let Some(sig) = outs_sig {
                        *i += 2;
                        let mut depth = 1i32;
                        let mut inner: Vec<Token> = Vec::new();
                        while *i < t.len() {
                            match &t[*i] {
                                Token::Sym("(") => depth += 1,
                                Token::Sym(")") => {
                                    depth -= 1;
                                    if depth == 0 {
                                        *i += 1;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            inner.push(t[*i].clone());
                            *i += 1;
                        }
                        if depth != 0 {
                            return err("调用括号没闭合");
                        }
                        let mut vals = Vec::new();
                        for a in split_args(&inner) {
                            vals.push(self.expr(&a)?);
                        }
                        //nt: C 符号名 = 声明
                        let tmp = self.temp(sig.ret.clone());
                        self.emit(Stmt::Assign {
                            lhs: Place::local(tmp),
                            rhs: Rvalue::Call { name: n.clone(), args: vals },
                        });
                        return Ok(Value::Local(tmp));
                    }
                    //nt: 内置函数调用
                    //nt: 内置名不经 call
                    if matches!(t.get(*i + 1), Some(Token::Sym("("))) {
                        *i += 2;
                        let mut depth = 1i32;
                        let mut inner: Vec<Token> = Vec::new();
                        while *i < t.len() {
                            match &t[*i] {
                                Token::Sym("(") => depth += 1,
                                Token::Sym(")") => {
                                    depth -= 1;
                                    if depth == 0 {
                                        *i += 1;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            inner.push(t[*i].clone());
                            *i += 1;
                        }
                        if depth != 0 {
                            return err("调用括号没闭合");
                        }
                        let mut vals = Vec::new();
                        for a in split_args(&inner) {
                            vals.push(self.expr(&a)?);
                        }
                        //nt: 有值的内置 ⇒ 只出
                        //nt: 此处若先无条件 em
                        //nt: 于是 pop/rem
                        //nt: 语句层的调用由 0e
                        const BUILTIN_VALUE_NAMES: &[&str] = &[
                            "len", "len_utf8", "index", "empty", "has", "pop", "run", "remove",
                            //nt: 取串内容的函数
                            "code", "char", "concat", "read_file", "str",
                        ];
                        if BUILTIN_VALUE_NAMES.contains(&n.as_str()) {
                            let mut n = n.clone();
                            //nt: 串首参改写
                            if n == "index" {
                                let 首参串 = match vals.first() {
                                    Some(Value::Local(id)) => self
                                        .f
                                        .locals
                                        .get(*id)
                                        .map(|l| l.ty.0 == "str")
                                        .unwrap_or(false),
                                    Some(Value::Str(_)) => true,
                                    _ => false,
                                };
                                if 首参串 {
                                    n = "str_index".into();
                                }
                            }
                            let mut rt = builtin_ret_ty(&n);
                            //nt: pop 的结果类型
                            if n == "pop" {
                                if let Some(Value::Local(bid)) = vals.first() {
                                    if let Some(l) = self.f.locals.get(*bid) {
                                        rt = Ty(elem_ty_of(&l.ty.0).unwrap_or_else(|| "int".into()));
                                    }
                                }
                            }
                            let tmp = self.temp(rt);
                            self.emit(Stmt::Assign {
                                lhs: Place::local(tmp),
                                rhs: Rvalue::Call { name: n.clone(), args: vals },
                            });
                            return Ok(Value::Local(tmp));
                        }
                    }
                    return err(format!(
                        "调用要写 `call`——形如 `call fn \"{}\"(…)`（同块调用也要写）",
                        n
                    ));
                }
                //nt: 和类型构造
                if matches!(t.get(*i + 1), Some(Token::Sym("("))) {
                    if let Some((模板型, tag0, 模板载荷)) = self.sum_variants.get(n).cloned() {
                        //nt: 期望类型若是该和类型
                        let (tyname, tag, ftypes) = match self.期望类型.clone() {
                            Some(t) if t != 模板型 => {
                                let 实例载荷 = self
                                    .type_idx
                                    .get(&t)
                                    .and_then(|i| self.types.get(*i))
                                    .and_then(|st| {
                                        st.variants
                                            .iter()
                                            .find(|v| v.name == *n)
                                            .map(|v| v.fields.iter().map(|(_, ty)| ty.clone()).collect())
                                    });
                                match 实例载荷 {
                                    Some(载) => (t, tag0, 载),
                                    None => (模板型, tag0, 模板载荷),
                                }
                            }
                            _ => (模板型, tag0, 模板载荷),
                        };
                        *i += 2;
                        let mut depth = 1i32;
                        let mut inner: Vec<Token> = Vec::new();
                        while *i < t.len() {
                            match &t[*i] {
                                Token::Sym("(") => depth += 1,
                                Token::Sym(")") => {
                                    depth -= 1;
                                    if depth == 0 {
                                        *i += 1;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            inner.push(t[*i].clone());
                            *i += 1;
                        }
                        if depth != 0 {
                            return err("变体构造的括号没闭合");
                        }
                        let mut vals = Vec::new();
                        for a in split_args(&inner) {
                            vals.push(self.expr(&a)?);
                        }
                        let tmp = self.temp(Ty(tyname.clone()));
                        self.emit(Stmt::Assign {
                            lhs: Place::local(tmp),
                            rhs: Rvalue::VariantLit { ty: tyname, tag, args: vals, tys: ftypes },
                        });
                        return Ok(Value::Local(tmp));
                    }
                }
                //nt: 暴露 val
                if let Some(&pid) = self.exposed.get(n) {
                    let pty = self.f.locals.get(pid).map(|l| l.ty.0.clone()).unwrap_or_default();
                    let inner = pty.strip_suffix('*').unwrap_or("int").to_string();
                    let tmp = self.temp(Ty(inner));
                    self.emit(Stmt::Assign {
                        lhs: Place::local(tmp),
                        rhs: Rvalue::Use(Place { base: pid, proj: vec![Proj::Deref] }),
                    });
                    return Ok(Value::Local(tmp));
                }
                *i += 1;
                match self.names.get(n) {
                    Some(id) => Ok(Value::Local(*id)),
                    None => err(format!("还没建过「{}」——先用 add val 建 (fn={})", n, self.f.name)),
                }
            }
            //nt: 表达式形态 matc
            Some(Token::Kw("match")) => self.match_expr(t, i),
            other => err(format!("表达式里不认识的 token：{:?}", other)),
        }
    }


    fn line(&mut self, toks: &[Token]) -> Result<(), BuildError> {
        if toks.is_empty() {
            return Ok(());
        }

        //nt: 中途give返回
        if self.语句体返回 && matches!(toks.first(), Some(Token::Kw("give"))) {
            let v = self.expr(&toks[1..])?;
            self.set_exit(Exit::Return(Some(v)));
            let 死块 = self.nb();
            self.switch_to(死块);
            self.set_exit(Exit::Unreachable);
            return Ok(());
        }

        //nt: 泛型构造落名
        //nt: 声明类型给出实例 ⇒
        if matches!(toks.first(), Some(Token::Kw("add")))
            && matches!(toks.get(1), Some(Token::Kw("val")) | Some(Token::Kw("fix")))
        {
            if let (Some(Token::Ident(名)), Some(eq)) =
                (toks.get(3), toks.iter().position(|t| *t == Token::Sym("=")))
            {
                if 泛型判别(名) && matches!(toks.get(4), Some(Token::Sym("("))) {
                    if let Some(闭) = toks[5..eq].iter().position(|t| *t == Token::Sym(")")) {
                        let 实参: Vec<String> = toks[5..5 + 闭]
                            .split(|t| *t == Token::Sym(","))
                            .map(|段| {
                                段.iter()
                                    .filter_map(|t| match t {
                                        Token::Ident(x) => Some(x.clone()),
                                        Token::Kw(k) => Some((*k).to_string()),
                                        Token::Sym("*") => Some("*".to_string()),
                                        _ => None,
                                    })
                                    .collect::<String>()
                            })
                            .filter(|s| !s.is_empty())
                            .collect();
                        let 实例名 = format!("{}({})", 名, 实参.join(", "));
                        let 右值同模板 = matches!(toks.get(eq + 1), Some(Token::Ident(x)) if x == 名)
                            && matches!(toks.get(eq + 2), Some(Token::Sym("(")));
                        if 右值同模板 {
                            let mut 新: Vec<Token> = toks.to_vec();
                            新[eq + 1] = Token::Ident(实例名);
                            return self.line(&新);
                        }
                    }
                }
            }
        }

        //nt: add type
        if is_add_type(toks) {
            return Ok(());
        }

        //nt: add hide t
        //nt: add hide f
        if matches!(toks.first(), Some(Token::Kw("add")))
            && matches!(toks.get(1), Some(Token::Kw("hide")))
            && !matches!(toks.get(2), Some(Token::Kw("fn")))
        {
            return Ok(());
        }

        //nt: 分支零 add fn
        //nt: add fn「…」与
//nt: 重绑定行 = 无 a
        let is_rebind = matches!(toks.first(), Some(Token::Kw("fn")));
        let is_hide_fn = matches!(toks.first(), Some(Token::Kw("add")))
            && matches!(toks.get(1), Some(Token::Kw("hide")))
            && matches!(toks.get(2), Some(Token::Kw("fn")));
        if (matches!(toks.first(), Some(Token::Kw("add"))) && matches!(toks.get(1), Some(Token::Kw("fn"))))
            || is_hide_fn
            || is_rebind
        {
            let name_at = if is_rebind {
                1
            } else if is_hide_fn {
                3
            } else {
                2
            };
            let name = match toks.get(name_at) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                _ => return err(format!("fn 后应是名字：{:?}", toks)),
            };
            //nt: 泛型 fn
            if matches!(toks.get(name_at + 1), Some(Token::Sym("("))) {
                if let Some(类型参数) = 读类型参数表(toks, name_at + 1) {
                    let 类型参数表止 = 括号止(toks, name_at + 1)
                        .ok_or_else(|| BuildError {
                            message: "泛型 fn 的类型参数表没闭合".into(),
                            code: "E301".into(),
                            line: CUR_LINE.with(|c| c.get()),
                            file: String::new(),
                        })?
                        + 1;
                    let (形参, 帕止) = parse_params(&toks[类型参数表止..])?;
                    let (ret, _限, _体起) = parse_type_ref(&toks[类型参数表止..], 帕止)?;
                    let 体 = match toks.iter().position(|t| *t == Token::Sym("=")) {
                        Some(eq) => toks[eq + 1..].to_vec(),
                        None => Vec::new(),
                    };
                    泛型fn表.with(|g| {
                        g.borrow_mut().insert(
                            name.clone(),
                            (类型参数, 形参, ret, 体, self.module.clone(), self.block.clone()),
                        );
                    });
                    return Ok(());
                }
            }
            let (params, after) = parse_params(toks)?;
            //nt: 登记常量 fn
            if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
                let 体 = toks[eq + 1..].to_vec();
                let 形参名: Vec<String> = params.iter().map(|(n, _)| n.clone()).collect();
                常量fn表.with(|t| {
                    t.borrow_mut().insert(name.clone(), (形参名, 体));
                });
            }
            //nt: 返回类型
            let (ret, _限定, _) = parse_type_ref(toks, after)
                .map_err(|_| BuildError {
                    message: "add fn 缺返回类型（函数边界类型必写）".into(),
                    code: "E301".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                })?;
            //nt: add 不带 = ⇒
            //nt: 不登记不编译
            let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) else {
                return Ok(());
            };

            //nt: 先登记签名，再编译体
            let mut fb = Builder::new(
                format!("{}|{}|{}", self.module, self.block, name),
                self.f.file.clone(),
                false, // fn 生成的函数——不是块，不参与链式执行
                false,
                false,
                false,
                false, // is_kill
                false, // is_naked（fn 函数不是 naked）
                Vec::new(),
                self.block.clone(),
                self.module.clone(),
                std::rc::Rc::clone(&self.types),
                std::rc::Rc::clone(&self.type_idx),
                std::rc::Rc::clone(&self.mod_by_tail),
                std::rc::Rc::clone(&self.global_fns),
                std::rc::Rc::clone(&self.cross_vals),
                std::rc::Rc::clone(&self.sum_variants),
                std::rc::Rc::clone(&self.pub_blocks),
                std::rc::Rc::clone(&self.hidden_items),
                std::rc::Rc::clone(&self.block_types),
            );
            fb.cur_col = self.cur_col;
            fb.cur_len = self.cur_len;
            for (pname, pty) in &params {
                let id = fb.new_local(Some(pname.clone()), pty.clone(), true, true);
                fb.names.insert(pname.clone(), id);
            }
            //nt: fn的槽隐藏参数
            let hidden = 注入槽参数_prep(&mut fb, &format!("{}|{}|{}", self.module, self.block, name));
            let sig = FnSig {
                params: params.iter().map(|(_, t)| t.clone()).collect(),
                ret: ret.clone(),
                is_outs: false,
                hidden,
            };
            self.fns.insert(
                format!("{}|{}|{}", self.module, self.block, name),
                sig,
            );
            let v = fb.expr(&toks[eq + 1..])?;
            //nt: 出口设在**当前块
            fb.set_exit(Exit::Return(Some(v)));
            let mut f = fb.finish()?;
            f.ret = Some(ret.clone());

            self.extra.push(f);
            return Ok(());
        }

//nt: 分支甲 add ou
        //nt: 无等号无体
        if matches!(toks.first(), Some(Token::Kw("add")))
            && matches!(toks.get(1), Some(Token::Kw("outs")))
            && matches!(toks.get(2), Some(Token::Kw("fn")))
        {
            let name = match toks.get(3) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                _ => return err("add outs fn 后应是名字"),
            };
            if toks.iter().any(|t| *t == Token::Sym("=")) {
                return err("外部函数声明没有函数体——函数体在 C 那边");
            }
            let (params, after) = parse_params(toks)?;
            let ret = match toks.get(after) {
                Some(Token::Kw(t)) => Ty((*t).to_string()),
                _ => return err("add outs fn 缺返回类型（外部函数边界类型必写）"),
            };
            let param_tys: Vec<Ty> = params.iter().map(|(_, t)| t.clone()).collect();
            self.fns.insert(
                format!("{}|{}", self.block, name.clone()),
                FnSig { params: param_tys.clone(), ret: ret.clone(), is_outs: true, hidden: Vec::new() },
            );
            self.externs.push(ExternFn { name, params: param_tys, ret });
            return Ok(());
        }

        //nt: 分支乙 call f
        if matches!(toks.first(), Some(Token::Kw("call"))) && matches!(toks.get(1), Some(Token::Kw("fn"))) {
            let name = match toks.get(2) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                _ => return err(format!("call fn 后应是名字：{:?}", toks)),
            };
            let key = format!("{}|{}|{}", self.module, self.block, name);
            let sig = self
                .fns
                .get(&key)
                .cloned()
                .or_else(|| self.global_fns.get(&key).cloned())
                .ok_or_else(|| BuildError {
                    message: format!("还没建过 fn「{}」——先用 add fn 建", name),
                    code: "E304".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                })?;
            let inner = between_parens(toks)?;
            let mut vals = Vec::new();
            for a in split_args(&inner) {
                vals.push(self.expr(&a)?);
            }
            //nt: fn的槽实参
            if !sig.hidden.is_empty() {
                let caller_is_fn = self.f.name.contains('#');
                vals.extend(self.槽实参(&sig, &key, caller_is_fn)?);
            }
            let t = self.temp(sig.ret.clone());
            let call_name = if sig.is_outs { name.clone() } else { key };
            self.emit(Stmt::Assign { lhs: Place::local(t), rhs: Rvalue::Call { name: call_name, args: vals } });
            return Ok(());
        }

        //nt: 分支丙 call #
        //nt: 分支丙乙 call
        //nt: IR 调用名用**模
        //nt: 限定口径，codeg
        let (blk, blk_mod) = if matches!(toks.first(), Some(Token::Kw("call")))
            && matches!(toks.get(1), Some(Token::HashColon))
            && toks.get(3).is_none()
        {
            let blk = match toks.get(2) {
                Some(Token::Ident(n)) => n.clone(),
                Some(Token::Kw(k)) => (*k).to_string(),
                _ => return err("`call #:` 后应接块名"),
            };
            (blk, self.module.clone())
        } else if matches!(toks.first(), Some(Token::Kw("call")))
            && matches!(toks.get(1), Some(Token::HashHashColon))
            && !is_take_form(toks) // 取用形态（`val` / `fn`）⇒ 让给 0c2 / 调用分支，不在此跑块
        {
            let mut m = String::new();
            let mut j = 2usize;
            while let Some(tok) = toks.get(j) {
                match tok {
                    Token::Ident(n) | Token::Str(n) => {
                        if !m.is_empty() {
                            m.push('.');
                        }
                        m.push_str(n);
                        j += 1;
                        if matches!(toks.get(j), Some(Token::Sym("."))) {
                            j += 1;
                        } else {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            if m.is_empty() || !matches!(toks.get(j), Some(Token::HashColon)) {
                return err("`##:` 后应接模块名再接 `#:块`——形如 `##:前端.拆词 #:分句`");
            }
            let blk = match toks.get(j + 1) {
                Some(Token::Ident(n)) => n.clone(),
                Some(Token::Kw(k)) => (*k).to_string(),
                _ => return err("`##:模块 #:` 后应接块名"),
            };
            //nt: 块名后仍有别的 to
            if toks.get(j + 2).is_some() {
                return err("跨模块跑整块写 `call ##:模块 #:块`；取 fn / val 用各自形态");
            }
            let full = if self.mod_has(&m) {
                m.clone()
            } else {
                match self.resolve_module(&m)? {
                    Some(f) => f,
                    None => return err(format!("模块「{}」找不到", m)),
                }            };
            (blk, full)
        } else {
            (String::new(), String::new())
        };
        if !blk.is_empty() {
//nt: 跨模块跑块 ⇒ 目标
            self.check_pub(&blk_mod, &blk)?;
            let mut args: Vec<Value> = Vec::new();
            if let Some(vals) = self.cross_vals.get(&format!("{}|{}", blk_mod, blk)).cloned() {
                for (vname, vty) in vals {
                    let slot_key = format!("{}|{}#{}", blk_mod, blk, vname);
                    let sid = match self.names.get(&slot_key) {
                        Some(&id) => id,
                        None => {
                            let id = self.new_local(Some(slot_key.clone()), vty.clone(), false, false);
                            self.names.insert(slot_key, id);
                            id
                        }
                    };
                    let ptmp = self.temp(Ty(format!("{}*", vty.0)));
                    self.emit(Stmt::Assign {
                        lhs: Place::local(ptmp),
                        rhs: Rvalue::Ref(Place::local(sid)),
                    });
                    args.push(Value::Local(ptmp));
                }
            }
            self.emit(Stmt::Call {
                name: format!("{}|{}", blk_mod, blk),
                args,
            });
            return Ok(());
        }

//nt: callfn语句
        if matches!(toks.first(), Some(Token::Kw("call")))
            && matches!(toks.get(1), Some(Token::Kw("fn")))
        {
            self.expr(toks)?;
            return Ok(());
        }

//nt: 分支丙乙次 cal
        if matches!(toks.first(), Some(Token::Kw("call")))
            && matches!(toks.get(1), Some(Token::Kw("val")))
        {
            let vname = match toks.get(2) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                _ => return err("`call val` 后应接 val 名"),
            };
            if !matches!(toks.get(3), Some(Token::Sym("="))) {
                return err(
                    "`call val \"名\"` 作语句要接 `= 新值`；只读取请放进表达式（`= call val \"名\"`）",
                );
            }
            let id = *self.names.get(&vname).ok_or_else(|| BuildError {
                message: format!("本块没有 val「{}」——同块写 val 写的是本块的", vname),
                code: "E304".into(),
                line: CUR_LINE.with(|c| c.get()),
                file: String::new(),
            })?;
            let v = self.expr(&toks[4..])?;
            self.emit(Stmt::Assign { lhs: Place::local(id), rhs: Rvalue::Const(v) });
            return Ok(());
        }

        //nt: 分支丙丁 call
        if matches!(toks.first(), Some(Token::Kw("call")))
            && (matches!(toks.get(1), Some(Token::HashColon))
                || matches!(toks.get(1), Some(Token::HashHashColon)))
        {
            //nt: 块所属模块
            let (blk_mod, blk_idx) = if matches!(toks.get(1), Some(Token::HashHashColon)) {
                let mut m = String::new();
                let mut j = 2usize;
                while let Some(tok) = toks.get(j) {
                    match tok {
                        Token::Ident(n) | Token::Str(n) => {
                            if !m.is_empty() {
                                m.push('.');
                            }
                            m.push_str(n);
                            j += 1;
                            if matches!(toks.get(j), Some(Token::Sym("."))) {
                                j += 1;
                            } else {
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                if m.is_empty() || !matches!(toks.get(j), Some(Token::HashColon)) {
                    return err("`##:` 后应接模块名再接 `#:块`——形如 `##:前端.拆词 #:分句`");
                }
                let full = if self.mod_has(&m) {
                    m.clone()
                } else {
                    match self.resolve_module(&m)? {
                        Some(f) => f,
                        None => return err(format!("模块「{}」找不到", m)),
                    }
                };
                (full, j + 1)
            } else {
                (self.module.clone(), 2usize)
            };
            //nt: 跨块fn裸语句
            if matches!(toks.get(blk_idx + 1), Some(Token::Kw("fn"))) {
                self.expr(toks)?;
                return Ok(());
            }
            if !matches!(toks.get(blk_idx + 1), Some(Token::Kw("val"))) {
                return err("跨块写 val 要写 `#:块 val \"名\"`");
            }
            let blk = match toks.get(blk_idx) {
                Some(Token::Ident(n)) => n.clone(),
                Some(Token::Kw(k)) => (*k).to_string(),
                _ => return err("`call #:` 后应接块名"),
            };
            let vname = match toks.get(blk_idx + 2) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                _ => return err("`#:块 val` 后应接 val 名"),
            };
//nt: 跨模块写 val ⇒
            self.check_pub(&blk_mod, &blk)?;
            let slot_key = format!("{}|{}#{}", blk_mod, blk, vname);
            let sid = match self.槽或建(&slot_key) {
                Some(id) => id,
                None => {
                    return Err(BuildError {
                        message: format!(
                            "块「{}」的 val「{}」不存在——先 add val 建它，sle 块还要先 call #:{}",
                            blk, vname, blk
                        ),
                        code: "E309".into(),
                        line: CUR_LINE.with(|c| c.get()),
                        file: String::new(),
                    })
                }
            };
            if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
                let v = self.expr(&toks[eq + 1..])?;
                self.emit(Stmt::Assign { lhs: Place::local(sid), rhs: Rvalue::Const(v) });
            }
            return Ok(());
        }

        //nt: 分支丁 下标写
        //nt: 写位置要显式
        if matches!(toks.first(), Some(Token::Ident(_))) && matches!(toks.get(1), Some(Token::Sym("["))) {
            return Err(BuildError {
                message: "下标写左边要显式——写 `val \"名\"[下标] = 值`".into(),
                code: "E303".into(),
                line: CUR_LINE.with(|c| c.get()),
                file: String::new(),
            });
        }
        let val_lhs = matches!(toks.first(), Some(Token::Kw("val")))
            && matches!(toks.get(2), Some(Token::Sym("[")));
        if val_lhs {
            let (base_name, open) = {
                let n = match toks.get(1) {
                    Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                    _ => unreachable!(),
                };
                (n, 2usize) // `[` 的下标（val "名" [ ⇒ 偏移 2）
            };
            let base = *self
                .names
                .get(&base_name)
                .ok_or_else(|| BuildError { message: format!("还没建过「{}」", base_name) , code: "E304".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
            let mut depth = 0i32;
            let mut close = None;
            for (k, tk) in toks.iter().enumerate().skip(open) {
                match tk {
                    Token::Sym("[") => depth += 1,
                    Token::Sym("]") => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(k);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let close = close.ok_or_else(|| BuildError { message: "下标缺 ]".into() , code: "E302".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
            let eq = toks
                .iter()
                .position(|t| *t == Token::Sym("="))
                .ok_or_else(|| BuildError { message: "下标写缺 = 表达式".into() , code: "E301".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
            let idx = self.expr(&toks[open + 1..close])?;
            //nt: 穿透写
            if matches!(toks.get(close + 1), Some(Token::Sym("."))) {
                let field = match toks.get(close + 2) {
                    Some(Token::Ident(f)) | Some(Token::Str(f)) => f.clone(),
                    _ => return err("下标后的 `.` 之后应接字段名"),
                };
                let val = self.expr(&toks[eq + 1..])?;
                let base_ty = self
                    .f
                    .locals
                    .get(base)
                    .map(|l| l.ty.0.clone())
                    .unwrap_or_default();
                let ety = elem_ty_of(&base_ty).unwrap_or_else(|| "int".into());
                let h = self.temp(Ty(ety.clone()));
                self.emit(Stmt::Assign {
                    lhs: Place::local(h),
                    rhs: Rvalue::ListGet { list: Value::Local(base), index: idx },
                });
                //nt: 经把手写 ⇒ 先判空
                if ety.ends_with('*') {
                    let c = self.bin(BinOp::Ne, Value::Local(h), Value::Null);
                    self.emit(Stmt::Assert {
                        cond: c,
                        msg: "空引用——把手是 null，写不到字段".into(),
                        fix: "写字段前先判一声（x != null）".into(),
                    });
                }
                self.emit(Stmt::Assign {
                    lhs: Place { base: h, proj: vec![Proj::Field(field)] },
                    rhs: Rvalue::Const(val),
                });
                return Ok(());
            }
            let v = self.expr(&toks[eq + 1..])?;
            //nt: base 是 map
            let base_is_map = self
                .f
                .locals
                .get(base)
                .map(|l| l.ty.0 == "map" || l.ty.0.starts_with("map("))
                .unwrap_or(false);
            if base_is_map {
                self.emit(Stmt::MapSet {
                    map: Value::Local(base),
                    key: idx,
                    value: v,
                });
                return Ok(());
            }
//nt: 队列/栈只进只出
            let base_is_filo = self
                .f
                .locals
                .get(base)
                .map(|l| l.ty.0.starts_with("que") || l.ty.0.starts_with("stack"))
                .unwrap_or(false);
            if base_is_filo {
                return err("队列/栈只进只出——下标赋值只对列表用");
            }
            self.emit(Stmt::ListSet {
                list: Value::Local(base),
                index: idx,
                value: v,
            });
            return Ok(());
        }

        //nt: 分支丙回 字段写
        //nt: 写位置要显式
        if matches!(toks.first(), Some(Token::Ident(_))) && matches!(toks.get(1), Some(Token::Sym("."))) {
            return Err(BuildError {
                message: "字段写左边要显式——写 `val \"名\".字段 = 值`".into(),
                code: "E303".into(),
                line: CUR_LINE.with(|c| c.get()),
                file: String::new(),
            });
        }
        let 字段写名位 = match toks.first() {
            Some(Token::Kw("val")) if matches!(toks.get(2), Some(Token::Sym("."))) => Some(1usize),
            _ => None,
        };
        if let Some(ni) = 字段写名位 {
            let base_name = match toks.get(ni) {
                Some(Token::Ident(n)) | Some(Token::Str(n)) => n.clone(),
                _ => return err("字段写左边应是名字"),
            };
            let base = *self
                .names
                .get(&base_name)
                .ok_or_else(|| BuildError { message: format!("还没建过「{}」", base_name) , code: "E304".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
            let field = match toks.get(ni + 2) {
                Some(Token::Ident(f)) | Some(Token::Str(f)) => f.clone(),
                _ => return err("字段访问后应是字段名"),
            };
            //nt: 字段写同样只对**积
            let w_ty = self
                .f
                .locals
                .get(base)
                .map(|l| l.ty.0.trim_end_matches('*').to_string())
                .unwrap_or_default();
            if !self.type_idx.contains_key(&w_ty) {
                return Err(BuildError {
                    message: format!(
                        "{} 没有字段——字段写（.名 = 值）只对 add type 定义的积类型用",
                        w_ty
                    ),
                    code: "E303".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                });
            }
            let eq = toks
                .iter()
                .position(|t| *t == Token::Sym("="))
                .ok_or_else(|| BuildError { message: "字段写缺 = 表达式".into() , code: "E301".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
            let v = self.expr(&toks[eq + 1..])?;
            self.emit(Stmt::Assign {
                lhs: Place { base, proj: vec![Proj::Field(field)] },
                rhs: Rvalue::Const(v),
            });
            return Ok(());
        }

        //nt: val delta
//nt: 表达式、写回。无当前
        if matches!(toks.first(), Some(Token::Kw("val")))
            && matches!(toks.get(2), Some(Token::Sym("(")))
        {
            let vname = match toks.get(1) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                _ => return err("`val` 后应接名字"),
            };
            let id = *self.names.get(&vname).ok_or_else(|| {
                BuildError {
                message: format!("还没建过 val「{}」——先用 add val 建 (fn={})", vname, self.f.name),
                code: "E304".into(),
                line: CUR_LINE.with(|c| c.get()),
                file: String::new(),
            }
            })?;
            //nt: 找配对右括号
            let mut depth = 0i32;
            let mut close = None;
            for (k, tk) in toks.iter().enumerate().skip(2) {
                match tk {
                    Token::Sym("(") => depth += 1,
                    Token::Sym(")") => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(k);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let close = close.ok_or_else(|| BuildError { message: "delta 括号没闭合".into() , code: "E302".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
            let op = match toks.get(3) {
                Some(Token::Sym("+")) => BinOp::Add,
                Some(Token::Sym("-")) => BinOp::Sub,
                Some(Token::Sym("*")) => BinOp::Mul,
                Some(Token::Sym("/")) => BinOp::Div,
                _ => return err("delta 括号应以运算符开头（如 `(+1)`、`(+ 表达式)`）"),
            };
            let rhs = self.expr(&toks[4..close])?;
            let nv = self.bin(op, Value::Local(id), rhs);
            let store = match nv {
                Value::Local(src) => Rvalue::Use(Place { base: src, proj: vec![] }),
                other => Rvalue::Const(other),
            };
            self.emit(Stmt::Assign { lhs: Place::local(id), rhs: store });
            return Ok(());
        }

        //nt: kill 语句
        if matches!(toks.first(), Some(Token::Kw("kill"))) {
            //nt: 取 kill 目标
            let name = toks
                .iter()
                .skip(1)
                .filter_map(|t| match t {
                    Token::Ident(n) | Token::Str(n) => Some(n.clone()),
                    _ => None,
                })
                .next_back()
                .ok_or_else(|| BuildError {
                    message: "kill 后应写块名（如 kill #:块名）".into(),
                    code: "E201".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                })?;
            self.emit(Stmt::KillRegion { block: name });
            return Ok(());
        }

        //nt: 分支戊零 内联汇编
        //nt: 括号内是隔离区，按
        if toks.first().map(anle_lexer::asm::是asm词).unwrap_or(false)
            && matches!(toks.get(1), Some(Token::Sym("(")))
        {
            let 内区 = anle_lexer::asm::括号内(toks, 1)
                .ok_or_else(|| BuildError {
                    message: "asm 的括号没闭合".into(),
                    code: "E301".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                })?
                .to_vec();
            let form = self.建asm(&内区)?;
            self.emit(Stmt::Asm(form));
            return Ok(());
        }

        //nt: 分支戊 调用语句
        if matches!(toks.first(), Some(Token::Ident(_))) && matches!(toks.get(1), Some(Token::Sym("("))) {
            let name = match toks.first() {
                Some(Token::Ident(n)) => n.clone(),
                _ => unreachable!(),
            };
            let inner = between_parens(toks)?;
            let mut vals = Vec::new();
            for a in split_args(&inner) {
                vals.push(self.expr(&a)?);
            }
//nt: 第③道关卡
            if (name == "push" && vals.len() >= 2 && self.is_container_value(&vals[1]))
                || (name == "insert" && vals.len() >= 3 && self.is_container_value(&vals[2]))
            {
                return err("容器里不能装容器——塞进列表的值不能是容器；要分组请用键值表（如 map(str, list(int))）或结构体字段装列表");
            }
            self.emit(Stmt::Call { name, args: vals });
            return Ok(());
        }

        //nt: 分支一 print
        if matches!(toks.first(), Some(Token::Ident(n)) if n == "print") {
            let inner = between_parens(toks)?;
            let mut vals = Vec::new();
            for a in split_args(&inner) {
                vals.push(self.expr(&a)?);
            }
            self.emit(Stmt::Call { name: "print".into(), args: vals });
            return Ok(());
        }

        //nt: 分支二 add val
        if matches!(toks.first(), Some(Token::Kw("add")))
            && matches!(toks.get(1), Some(Token::Kw("val")) | Some(Token::Kw("fix")))
        {
            let name = match toks.get(2) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                _ => return err(format!("add val 后应是名字：{:?}", toks)),
            };
            let explicit_ty: Option<Ty> = match container_ty_at(toks, 3) {
                Some(t) => Some(Ty(t)),
                None => match toks.get(3) {
                    Some(Token::Kw(t)) => {
                        if *t == "fn" && matches!(toks.get(4), Some(Token::Sym("("))) {
                            match parse_type_ref(toks, 3) {
                                Ok((ty, _, _)) => Some(ty),
                                Err(_) => Some(Ty((*t).to_string())),
                            }
                        } else {
                            Some(Ty((*t).to_string()))
                        }
                    }
                    //nt: 自定义类型名是 Id
                    //nt: 带类型参数
                    Some(Token::Ident(n)) => {
                        if matches!(toks.get(4), Some(Token::Sym("("))) {
                            match parse_type_ref(toks, 3) {
                                Ok((t, _, _)) => Some(t),
                                Err(_) => Some(Ty(n.clone())),
                            }
                        } else {
                            Some(Ty(if matches!(toks.get(4), Some(Token::Sym("*"))) {
                                format!("{}*", n)
                            } else {
                                n.clone()
                            }))
                        }
                    }
//nt: 跨块 / 跨模块取类
                    //nt: 跨块前缀
                    Some(Token::HashColon) | Some(Token::HashHashColon) => {
                        let cross_mod = matches!(toks.get(3), Some(Token::HashHashColon));
                        //nt: 跨模块前缀
                        let (blk_mod, colon_at) = if cross_mod {
                            let mut m = String::new();
                            let mut j = 4usize;
                            while let Some(tok) = toks.get(j) {
                                match tok {
                                    Token::Ident(n) | Token::Str(n) => {
                                        if !m.is_empty() { m.push('.'); }
                                        m.push_str(n);
                                        j += 1;
                                        if matches!(toks.get(j), Some(Token::Sym("."))) { j += 1; } else { break; }
                                    }
                                    _ => break,
                                }
                            }
                            if m.is_empty() || !matches!(toks.get(j), Some(Token::HashColon)) {
                                return err("`##:` 后应接模块名再接 `#:块`——形如 `##:前端.拆词 #:点 type \"node\"`");
                            }
                            let full = if self.mod_has(&m) { m.clone() } else {
                                match self.resolve_module(&m)? {
                                    Some(f) => f,
                                    None => return err(format!("模块「{}」找不到", m)),
                                }
                            };
                            (full, j)
                        } else {
                            (self.module.clone(), 3usize)
                        };
                        let blk = match toks.get(colon_at + 1) {
                            Some(Token::Ident(n)) => n.clone(),
                            Some(Token::Kw(k)) => (*k).to_string(),
                            _ => return err("`#:` 后应接块名"),
                        };
                        if !matches!(toks.get(colon_at + 2), Some(Token::Kw("type"))) {
                            return err("跨块取类型要写 `#:块 type \"名\"`");
                        }
                        let tname = match toks.get(colon_at + 3) {
                            Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                            _ => return err("`#:块 type` 后应接类型名"),
                        };
                        //nt: 跨模块 ⇒ pub
                        if cross_mod {
                            self.check_pub(&blk_mod, &blk)?;
                            if self.hidden_items.contains(&format!("{}|{}|type|{}", blk_mod, blk, tname)) {
                                return Err(BuildError {
                                    message: format!("type「{}」在块「{}」（模块 {}）里被 hide 了——它的公开面里没有这项", tname, blk, blk_mod),
                                    code: "E201".into(),
                                    line: CUR_LINE.with(|c| c.get()),
                                    file: String::new(),
                                });
                            }
                        }
                        let has = self
                            .block_types
                            .get(&format!("{}|{}", blk_mod, blk))
                            .map(|m| m.contains(&tname))
                            .unwrap_or(false);
                        if !has {
                            return err(format!("块「{}」里没有 type「{}」", blk, tname));
                        }
                        //nt: 身份用类型名本身
                        Some(Ty(tname))
                    }
                    //nt: 没有类型标注 ⇒ 按
                    _ => None,
                },
            }
            .map(|t| Ty(代换类型(&t.0)));
            let eq0 = toks.iter().position(|t| *t == Token::Sym("="));
            let pre = match (&explicit_ty, eq0) {
                (None, Some(e)) => Some(self.expr(&toks[e + 1..])?),
                _ => None,
            };
            let ty = match explicit_ty {
                Some(t) => t,
                None => self.ty_of_value(pre.clone().expect("缺类型必有值")),
            };
        //nt: 字面量 RHS ⇒
        let lit = toks
            .iter()
            .position(|t| *t == Token::Sym("="))
            .and_then(|eq| lit_hint(&ty, &toks[eq + 1..]));
        //nt: 被跨块取用的 val
        let is_exposed = self
            .cross_vals
            .get(&format!("{}|{}", self.module, self.block))
            .map(|m| m.contains_key(&name))
            .unwrap_or(false);
        //nt: 定长族
        //nt: 定长溢出在安全层报错
        if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
            let after = &toks[eq + 1..];
            if let Some(v) = int_literal_val(after) {
                if let Some((lo, hi)) = width_range(&ty.0) {
                    if v < lo || v > hi {
                        return err(format!(
                            "val「{}」声明是 {}，字面量 {} 超出范围（{}..={}）",
                            name, ty.0, v, lo, hi
                        ));
                    }
                }
            }
        }
        //nt: fn内构造标记
        //nt: 供边界复印
        let ty0 = ty.0.clone();
        let fn自有 = !self.f.is_block
            && {
                let 是聚合 = ty0.ends_with('*')
                    || matches!(
                        ty0.split('(').next().unwrap_or("").trim(),
                        "list" | "map" | "que" | "stack"
                    )
                    || self.types.iter().any(|t| t.name == ty0);
                let 构造形 = match toks
                    .iter()
                    .position(|t| *t == Token::Sym("="))
                    .and_then(|eq| toks.get(eq + 1))
                {
                    Some(Token::Sym("[")) | Some(Token::Sym("{")) => true,
                    Some(Token::Ident(_)) | Some(Token::Str(_)) => matches!(
                        toks.iter().position(|t| *t == Token::Sym("=")).and_then(|eq| toks.get(eq + 2)),
                        Some(Token::Sym("("))
                    ),
                    _ => false,
                };
                是聚合 && 构造形
            };
        let id = if is_exposed {
            let pid = self.new_local(Some(name.clone()), Ty(format!("{}*", ty.0)), true, false);
            self.exposed.insert(name.clone(), pid);
            pid
        } else {
            self.new_local(Some(name.clone()), ty, false, false)
        };
            self.names.insert(name, id);
            if fn自有 {
                if let Some(l) = self.f.locals.get_mut(id) {
                    l.fn_owned = true;
                }
            }
            if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
                self.lit_ty = lit;
                //nt: 缺类型时上面已求过一
                //nt: 期望类型 = 这个
                let 期望 = self.f.locals.get(id).map(|l| l.ty.0.clone());
                //nt: fix 常量
                let 折叠常 = if matches!(toks.get(1), Some(Token::Kw("fix"))) {
                    toks.iter().position(|t| *t == Token::Sym("=")).and_then(|e| {
                        let 右 = &toks[e + 1..];
                        crate::consts::折叠(右).or_else(|| {
                            let 常量 = &self.常量值;
                            crate::consts::折叠带环境(
                                右,
                                &mut |n| 常量.get(n).cloned(),
                                &mut |f, a| 编译期执行(f, a, 常量, 0),
                            )
                        })
                    })
                } else {
                    None
                };
                if let Some(c) = &折叠常 {
                    let 常量名 = match toks.get(2) {
                        Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                        _ => String::new(),
                    };
                    if !常量名.is_empty() {
                        self.常量值.insert(常量名, c.clone());
                    }
                }
                //nt: fix 是编译期常量
                if matches!(toks.get(1), Some(Token::Kw("fix"))) && 折叠常.is_none() {
                    return Err(BuildError {
                        message: format!(
                            "常量「{}」的右值不是编译期可求值的表达式——常量要求字面量、运算符、\
                             或参数全为常量的 fn 调用",
                            常量名显示(toks)
                        ),
                        code: "E201".into(),
                        line: CUR_LINE.with(|c| c.get()),
                        file: String::new(),
                    });
                }
                let 折叠值 = 折叠常.as_ref().map(|c| c.转值());
                let v = match 折叠值 {
                    Some(c) => c,
                    None => match pre {
                        Some(v) => v,
                        None => {
                            let 旧 = self.期望类型.take();
                            self.期望类型 = 期望;
                            let r = self.expr(&toks[eq + 1..]);
                            self.期望类型 = 旧;
                            r?
                        }
                    },
                };
                self.lit_ty = None;
                //nt: 暴露 val ⇒ 初
                let lhs = if is_exposed {
                    Place { base: id, proj: vec![Proj::Deref] }
                } else {
                    Place::local(id)
                };
                self.emit(Stmt::Assign { lhs, rhs: Rvalue::Const(v) });
            }
            return Ok(());
        }

        //nt: 分支三 val 名
        if matches!(toks.first(), Some(Token::Kw("val"))) {
            let name = match toks.get(1) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                _ => return err(format!("val 后应是名字：{:?}", toks)),
            };
            //nt: 暴露 val
            if let Some(&pid) = self.exposed.get(&name) {
                if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
                    let v = self.expr(&toks[eq + 1..])?;
                    let 写回 = match v {
                        Value::Local(id) => Place::local(id),
                        other => {
                            //nt: 字面量写回
                            let rv = match other {
                                Value::Int(i) => Rvalue::Const(Value::Int(i)),
                                Value::Float(f) => Rvalue::Const(Value::Float(f)),
                                Value::Bool(b) => Rvalue::Const(Value::Bool(b)),
                                Value::Str(s) => Rvalue::Const(Value::Str(s)),
                                Value::Null => Rvalue::Const(Value::Null),
                                _ => return err("内部：该形态暂不支持写回本块 val"),
                            };
                            self.emit(Stmt::Assign {
                                lhs: Place { base: pid, proj: vec![Proj::Deref] },
                                rhs: rv,
                            });
                            Place { base: pid, proj: vec![Proj::Deref] }
                        }
                    };
                    self.emit(Stmt::Assign {
                        lhs: Place { base: pid, proj: vec![Proj::Deref] },
                        rhs: Rvalue::Use(写回),
                    });
                }
                return Ok(());
            }
            let id = *self.names.get(&name).ok_or_else(|| BuildError {
                message: format!("还没建过 val「{}」(fn={})", name, self.f.name),
                code: "E304".into(),
                line: CUR_LINE.with(|c| c.get()),
                file: String::new(),
            })?;
            if let Some(eq) = toks.iter().position(|t| *t == Token::Sym("=")) {
                let tty = self.f.locals.get(id).map(|l| l.ty.clone());
                self.lit_ty = tty.as_ref().and_then(|t| lit_hint(t, &toks[eq + 1..]));
                let v = self.expr(&toks[eq + 1..])?;
                self.lit_ty = None;
                self.emit(Stmt::Assign { lhs: Place::local(id), rhs: Rvalue::Const(v) });
            }
            return Ok(());
        }

        err(format!(
            "暂不支持的语句（当前支持 add val / add fn / add type / add outs fn / val \"名\" = / call fn / call #:块 / call ##:模块 #:块 / take / print / if / while / for / match / kill / asm / 下标写 / 字段写）：{:?}",
            toks
        ))
    }

    //nt: for 变量 in
    fn for_list_stmt(&mut self, head: &[Token], body: &[AstStmt]) -> Result<(), BuildError> {
        let var = match head.get(1) {
            Some(Token::Ident(n)) | Some(Token::Str(n)) => n.clone(),
            _ => return err("for 后应是循环变量"),
        };
        //nt: 列表表达式
        let lst_toks: Vec<Token> = head[3..head.len() - 1].to_vec();
        let lst = self.expr(&lst_toks)?;
        let lid = match lst {
            Value::Local(id) => id,
            other => {
//nt: 字面量等 ⇒ 物化到
                //nt: for c in "
                let ty = self.ty_of_value(other.clone());
                let t = self.temp(ty);
                self.emit(Stmt::Assign { lhs: Place::local(t), rhs: Rvalue::Const(other) });
                t
            }
        };
        //nt: 元素类型
        let lst_ty = self.f.locals.get(lid).map(|l| l.ty.0.clone()).unwrap_or_default();
        let elem = if lst_ty == "str" {
//nt: for c in 字
            "str".to_string()
        } else if matches!(lst_toks.first(), Some(Token::Ident(n)) if n == "keys") {
            "str".to_string()
        } else {
            self.f
                .locals
                .get(lid)
                .and_then(|l| elem_ty_of(&l.ty.0))
                .unwrap_or_else(|| "int".into())
        };
        let vid = self.new_local(Some(var.clone()), Ty(elem), false, true);
        self.names.insert(var, vid);
        let idx = self.temp(Ty("int".into()));
        self.emit(Stmt::Assign { lhs: Place::local(idx), rhs: Rvalue::Const(Value::Int(1)) });
        //nt: 串遍历重置游标
        if lst_ty == "str" {
            self.emit(Stmt::Call {
                name: "#str_iter_start".into(),
                args: vec![Value::Local(lid)],
            });
        }
        //nt: 长度只算一次
        let len_t = self.temp(Ty("int".into()));
        self.emit(Stmt::Assign {
            lhs: Place::local(len_t),
            rhs: Rvalue::Call { name: "len".into(), args: vec![Value::Local(lid)] },
        });

        let head_id = self.nb();
        let body_id = self.nb();
        let exit_id = self.nb();
        self.goto(head_id);

        self.switch_to(head_id);
        let c = self.bin(BinOp::Le, Value::Local(idx), Value::Local(len_t));
        self.set_exit(Exit::Branch { cond: c, targets: vec![body_id, exit_id] });

        self.switch_to(body_id);
//nt: 循环变量 = 容器[
        if lst_ty == "str" {
            self.emit(Stmt::Assign {
                lhs: Place::local(vid),
                rhs: Rvalue::Call {
                    name: "#str_iter_next".into(),
                    args: vec![Value::Local(lid), Value::Local(idx)],
                },
            });
        } else {
            self.emit(Stmt::Assign {
                lhs: Place::local(vid),
                rhs: Rvalue::ListGet { list: Value::Local(lid), index: Value::Local(idx) },
            });
        }
        self.循环深 += 1;
        self.body(body)?;
        self.循环深 -= 1;
        let nxt = self.bin(BinOp::Add, Value::Local(idx), Value::Int(1));
        self.emit(Stmt::Assign { lhs: Place::local(idx), rhs: Rvalue::Const(nxt) });
        self.goto(head_id);

        self.switch_to(exit_id);
        Ok(())
    }

    //nt: 范围循环
    fn for_range_stmt(&mut self, head: &[Token], body: &[AstStmt]) -> Result<(), BuildError> {
        //nt: head = [fo
        if head.len() < 5 || !matches!(head.get(2), Some(Token::Kw("in"))) {
            return err("for 目前只支持 `for 变量 in 甲~乙:`（范围循环）；遍历容器要等容器接入");
        }
        let var = match head.get(1) {
            Some(Token::Ident(n)) | Some(Token::Str(n)) => n.clone(),
            _ => return err("for 后应是循环变量"),
        };
        //nt: 顶层波浪
        let mut 深 = 0i32;
        let mut 波浪 = None;
        for (i, t) in head.iter().enumerate().skip(3) {
            match t {
                Token::Sym("(") | Token::Sym("[") | Token::Sym("{") => 深 += 1,
                Token::Sym(")") | Token::Sym("]") | Token::Sym("}") => 深 -= 1,
                Token::Sym("~") if 深 == 0 => {
                    波浪 = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let 波浪 = match 波浪 {
            Some(i) => i,
            None => return err("范围循环缺少 `~`"),
        };
        let 止 = if matches!(head.last(), Some(Token::Sym(":"))) {
            head.len() - 1
        } else {
            head.len()
        };
        if 波浪 == 3 || 波浪 + 1 >= 止 {
            return err("范围循环两端都要写表达式");
        }
        let lo = self.expr(&head[3..波浪])?;
        let hi = self.expr(&head[波浪 + 1..止])?;

        //nt: 循环变量 = 块内普
        let id = self.new_local(Some(var.clone()), Ty("int".into()), false, true);
        self.names.insert(var, id);
        self.emit(Stmt::Assign { lhs: Place::local(id), rhs: Rvalue::Const(lo) });

        let head_id = self.nb();
        let body_id = self.nb();
        let exit_id = self.nb();
        self.goto(head_id);

        self.switch_to(head_id);
        let c = self.bin(BinOp::Le, Value::Local(id), hi);
        self.set_exit(Exit::Branch { cond: c, targets: vec![body_id, exit_id] });

        self.switch_to(body_id);
        self.循环深 += 1;
        self.body(body)?;
        self.循环深 -= 1;
        let nxt = self.bin(BinOp::Add, Value::Local(id), Value::Int(1));
        self.emit(Stmt::Assign { lhs: Place::local(id), rhs: Rvalue::Const(nxt) });
        self.goto(head_id);

        self.switch_to(exit_id);
        Ok(())
    }

    //nt: 结构行主体
    fn head_expr_tokens(head: &[Token]) -> &[Token] {
        if head.len() >= 2 {
            &head[1..head.len() - 1]
        } else {
            &[]
        }
    }

    fn if_stmt(
        &mut self,
        head: &[Token],
        body: &[AstStmt],
        else_body: Option<&Vec<AstStmt>>,
    ) -> Result<(), BuildError> {
        let c = self.expr(Self::head_expr_tokens(head))?;
        let then_id = self.nb();
        let else_id = self.nb();
        let next_id = self.nb();

        self.set_exit(Exit::Branch { cond: c, targets: vec![then_id, else_id] });

        self.switch_to(then_id);
        self.body(body)?;
        self.goto(next_id);

        self.switch_to(else_id);
        if let Some(eb) = else_body {
            self.body(eb)?;
        }
        self.goto(next_id);

        self.switch_to(next_id);
        Ok(())
    }

    fn while_stmt(&mut self, head: &[Token], body: &[AstStmt]) -> Result<(), BuildError> {
        //nt: while <con
        let head_toks = Self::head_expr_tokens(head);
        let (cond_toks, max_toks) = match head_toks.iter().position(|t| matches!(t, Token::Kw("max"))) {
            Some(p) => (&head_toks[..p], Some(&head_toks[p + 1..])),
            None => (head_toks, None),
        };
        //nt: max 计数器
        let cnt = match max_toks {
            Some(_) => {
                let c = self.new_local(None, Ty("int".into()), false, false);
                self.emit(Stmt::Assign { lhs: Place::local(c), rhs: Rvalue::Const(Value::Int(0)) });
                Some(c)
            }
            None => None,
        };

        let head_id = self.nb();
        let body_id = self.nb();
        let exit_id = self.nb();
        self.goto(head_id);

        self.switch_to(head_id);
        let c = self.expr(cond_toks)?;
        self.set_exit(Exit::Branch { cond: c, targets: vec![body_id, exit_id] });

        self.switch_to(body_id);
        //nt: max 护栏
        if let (Some(cnt), Some(mx)) = (cnt, max_toks) {
            //nt: max 上限必须是整
            let mv = self.expr(mx)?;
            let mt = self.ty_of_value(mv.clone());
            if mt.0 != "int" {
                return err(format!(
                    "while 的 max 上限必须是整数——给的是 {}（如 `max 100` 或 int 变量/算式）",
                    mt.0
                ));
            }
            let c1 = self.bin(BinOp::Add, Value::Local(cnt), Value::Int(1));
            self.emit(Stmt::Assign { lhs: Place::local(cnt), rhs: Rvalue::Const(c1) });
            let ok = self.bin(BinOp::Le, Value::Local(cnt), mv);
            self.emit(Stmt::Assert {
                cond: ok,
                msg: "循环超过 max 上限——疑似死循环".into(),
                fix: "检查循环的退出条件是否能达成，或把 max 上限调大".into(),
            });
        }
        self.循环深 += 1;
        self.body(body)?;
        self.循环深 -= 1;
        self.goto(head_id);

        self.switch_to(exit_id);
        Ok(())
    }

    //nt: match
    fn match_stmt(&mut self, head: &[Token], body: &[AstStmt]) -> Result<(), BuildError> {
        let v = self.expr(Self::head_expr_tokens(head))?;
        let exit_id = self.nb();

        let mut cases: Vec<(TokStream, Vec<AstStmt>)> = Vec::new();
        for s in body {
            match s {
                AstStmt::Struct { head: h, body: cb, .. } => {
                    if !matches!(h.first(), Some(Token::Kw("case"))) {
                        return err("match 体里只能是 case 分支");
                    }
                    cases.push((h.clone(), cb.clone()));
                }
                AstStmt::Line { toks: t, .. } if t.is_empty() => {}
                _ => return err("match 体里只能是 case 分支"),
            }
        }

        for (h, cb) in &cases {
            let ht = Self::head_expr_tokens(h).to_vec();
            //nt: case 变体名
            if let Some(Token::Ident(vname)) = ht.first() {
                if let Some((_, tag, ftypes)) = self.sum_variants.get(vname).cloned() {
                    let tagv = self.temp(Ty("int".into()));
                    self.emit(Stmt::Assign {
                        lhs: Place::local(tagv),
                        rhs: Rvalue::VariantTag(v.clone()),
                    });
                    let t = self.bin(BinOp::Eq, Value::Local(tagv), Value::Int(tag as i64));
                    let body_id = self.nb();
                    let next_check = self.nb();
                    self.set_exit(Exit::Branch { cond: t, targets: vec![body_id, next_check] });
                    self.switch_to(body_id);
                    //nt: 解构绑定
                    if matches!(ht.get(1), Some(Token::Sym("("))) {
                        let mut d = 0i32;
                        let mut end = None;
                        for (k, tk) in ht.iter().enumerate().skip(1) {
                            match tk {
                                Token::Sym("(") => d += 1,
                                Token::Sym(")") => {
                                    d -= 1;
                                    if d == 0 {
                                        end = Some(k);
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        if let Some(end) = end {
                            for (idx, a) in split_args(&ht[2..end]).into_iter().enumerate() {
                                let bname = match a.first() {
                                    Some(Token::Ident(n)) | Some(Token::Str(n)) => n.clone(),
                                    _ => continue,
                                };
                                let bty = ftypes.get(idx).cloned().unwrap_or(Ty("int".into()));
                                let bid = self.new_local(Some(bname.clone()), bty.clone(), false, true);
                                self.names.insert(bname, bid);
                                self.emit(Stmt::Assign {
                                    lhs: Place::local(bid),
                                    rhs: Rvalue::VariantGet { value: v.clone(), idx, ty: bty },
                                });
                            }
                        }
                    }
                    self.body(cb)?;
                    self.goto(exit_id);
                    self.switch_to(next_check);
                    continue;
                }
            }
            //nt: case _
            if ht.len() == 1 && matches!(ht[0], Token::Ident(ref n) if n == "_") {
                self.body(cb)?;
                self.goto(exit_id);
                continue;
            }
            let cv = self.expr(Self::head_expr_tokens(h))?;
            let t = self.bin(BinOp::Eq, v.clone(), cv);
            let body_id = self.nb();
            let next_check = self.nb();
            self.set_exit(Exit::Branch { cond: t, targets: vec![body_id, next_check] });

            self.switch_to(body_id);
            self.body(cb)?;
            self.goto(exit_id);

            self.switch_to(next_check);
        }
        //nt: 都没命中⇒跳出口
        self.goto(exit_id);
        self.switch_to(exit_id);
        Ok(())
    }

    //nt: 表达式形态 matc
    //nt: 语句形态 match
    fn match_expr(&mut self, t: &[Token], i: &mut usize) -> Result<Value, BuildError> {
        *i += 1; // 跳过 `match`
        //nt: 对象字面量
        let mut depth = 0i32;
        let mut brace = None;
        let mut j = *i;
        while j < t.len() {
            match &t[j] {
                Token::Sym("(") | Token::Sym("[") => depth += 1,
                Token::Sym(")") | Token::Sym("]") => depth -= 1,
                Token::Sym("{") => {
                    if depth == 0 {
                        brace = Some(j);
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        let brace = match brace {
            Some(b) => b,
            None => return err("表达式形态 match 的 `{` 没找到——形如 `match 对象 { case 变体(绑定): 表达式, … }`"),
        };
        let obj_toks: Vec<Token> = t[*i..brace].to_vec();
        if obj_toks.is_empty() {
            return err("match 后要跟对象");
        }
        //nt: 解析 case 列表
        let mut arms: Vec<(Vec<Token>, Vec<Token>)> = Vec::new();
        j = brace + 1;
        loop {
            if j >= t.len() {
                return err("表达式形态 match 的 `}` 没闭合");
            }
            if matches!(&t[j], Token::Sym("}")) {
                j += 1;
                break;
            }
            if !matches!(&t[j], Token::Kw("case")) {
                return err("表达式形态 match 的体里只能是 case 分支（形如 `case 得(v): 得(v*2)`）");
            }
            j += 1;
            //nt: 模式：到顶层`:`
            let pat_start = j;
            let mut d2 = 0i32;
            while j < t.len() {
                match &t[j] {
                    Token::Sym("(") => d2 += 1,
                    Token::Sym(")") => d2 -= 1,
                    Token::Sym(":") if d2 == 0 => break,
                    _ => {}
                }
                j += 1;
            }
            if j >= t.len() {
                return err("表达式形态 match 的 case 缺 `:`");
            }
            let pat: Vec<Token> = t[pat_start..j].to_vec();
            j += 1; // 跳过 `:`
            //nt: 分支表达式
            let ex_start = j;
            let mut d3 = 0i32;
            while j < t.len() {
                match &t[j] {
                    Token::Sym("(") | Token::Sym("[") => d3 += 1,
                    Token::Sym(")") | Token::Sym("]") => d3 -= 1,
                    Token::Sym("{") => d3 += 1,
                    Token::Sym("}") if d3 == 0 => break,
                    Token::Sym("}") => d3 -= 1,
                    Token::Sym(",") if d3 == 0 => break,
                    _ => {}
                }
                j += 1;
            }
            let ex: Vec<Token> = t[ex_start..j].to_vec();
            if matches!(t.get(j), Some(Token::Sym(","))) {
                j += 1;
            }
            arms.push((pat, ex));
        }
        *i = j;
        if arms.is_empty() {
            return err("表达式形态 match 至少要有一个 case");
        }
        let v = self.expr(&obj_toks)?;
        let tmp = self.temp(Ty("int".into())); // 占位，随后按首条分支实际类型修正
        let exit_id = self.nb();
        let mut first_ty: Option<Ty> = None;
        for (pat, ex) in &arms {
            //nt: case _
            if pat.len() == 1 && matches!(&pat[0], Token::Ident(n) if n == "_") {
                let av = self.expr(ex)?;
                if first_ty.is_none() {
                    first_ty = Some(self.ty_of_value(av.clone()));
                }
                self.emit(Stmt::Assign { lhs: Place::local(tmp), rhs: Rvalue::Const(av) });
                self.goto(exit_id);
                continue;
            }
            let vname = match pat.first() {
                Some(Token::Ident(n)) => n.clone(),
                _ => return err("表达式形态 match 的 case 模式只支持 `变体(绑定…)` 与 `_`"),
            };
            let (_, tag, ftypes) = match self.sum_variants.get(&vname).cloned() {
                Some(x) => x,
                None => return err(format!("「{}」不是和类型变体——表达式形态 match 的 case 只按变体判别", vname)),
            };
            let tagv = self.temp(Ty("int".into()));
            self.emit(Stmt::Assign { lhs: Place::local(tagv), rhs: Rvalue::VariantTag(v.clone()) });
            let cmp = self.bin(BinOp::Eq, Value::Local(tagv), Value::Int(tag as i64));
            let body_id = self.nb();
            let next_id = self.nb();
            self.set_exit(Exit::Branch { cond: cmp, targets: vec![body_id, next_id] });
            self.switch_to(body_id);
            //nt: 解构绑定
            if matches!(pat.get(1), Some(Token::Sym("("))) {
                let mut d = 0i32;
                let mut end = None;
                for (k, tk) in pat.iter().enumerate().skip(1) {
                    match tk {
                        Token::Sym("(") => d += 1,
                        Token::Sym(")") => {
                            d -= 1;
                            if d == 0 {
                                end = Some(k);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(end) = end {
                    for (idx, a) in split_args(&pat[2..end]).into_iter().enumerate() {
                        let bname = match a.first() {
                            Some(Token::Ident(n)) | Some(Token::Str(n)) => n.clone(),
                            _ => continue,
                        };
                        let bty = ftypes.get(idx).cloned().unwrap_or(Ty("int".into()));
                        let bid = self.new_local(Some(bname.clone()), bty.clone(), false, true);
                        self.names.insert(bname, bid);
                        self.emit(Stmt::Assign {
                            lhs: Place::local(bid),
                            rhs: Rvalue::VariantGet { value: v.clone(), idx, ty: bty },
                        });
                    }
                }
            }
            let av = self.expr(ex)?;
            if first_ty.is_none() {
                first_ty = Some(self.ty_of_value(av.clone()));
            }
            self.emit(Stmt::Assign { lhs: Place::local(tmp), rhs: Rvalue::Const(av) });
            self.goto(exit_id);
            self.switch_to(next_id);
        }
        //nt: 一个分支都没命中 ⇒
        self.emit(Stmt::Assert {
            cond: Value::Bool(false),
            msg: "match 没有命中任何分支——穷尽性检查被绕过".into(),
            fix: "补上缺的 case，或写 `case _:` 兜底".into(),
        });
        self.goto(exit_id);
        self.switch_to(exit_id);
        if let Some(ty) = first_ty {
            if let Some(l) = self.f.locals.get_mut(tmp) {
                l.ty = ty;
            }
        }
        Ok(Value::Local(tmp))
    }

    //nt: 语句体 fn
    fn stmt_fn(&mut self, head: &[Token], body: &[AstStmt]) -> Result<Function, BuildError> {
        let h: Vec<Token> = head.iter().cloned().filter(|t| *t != Token::Sym(":")).collect();
        let is_rebind = matches!(h.first(), Some(Token::Kw("fn")));
        let is_hide_fn = matches!(h.first(), Some(Token::Kw("add")))
            && matches!(h.get(1), Some(Token::Kw("hide")))
            && matches!(h.get(2), Some(Token::Kw("fn")));
        let name_at = if is_rebind { 1 } else if is_hide_fn { 3 } else { 2 };
        let name = match h.get(name_at) {
            Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
            _ => return err("语句体 fn 后应是名字"),
        };
        let (params, after) = parse_params(&h)?;
        let (ret, _限, _) = parse_type_ref(&h, after).map_err(|_| BuildError {
            message: "语句体 fn 缺返回类型（函数边界类型必写）".into(),
            code: "E301".into(),
            line: CUR_LINE.with(|c| c.get()),
            file: String::new(),
        })?;
        self.stmt_fn_body(&name, params, ret, body)
    }

    //nt: 编泛型实例
    fn 建语句体实例(
        &mut self,
        实例名: &str,
        类型参数: &[String],
        实参类型: &[String],
        形参: &[(String, Ty)],
        返回: &Ty,
        体: &[AstStmt],
    ) -> Result<(), BuildError> {
        let 具体形参: Vec<(String, Ty)> = 形参
            .iter()
            .map(|(n, t)| (n.clone(), Ty(替换参数(&t.0, 类型参数, 实参类型))))
            .collect();
        let 具体返回 = Ty(替换参数(&返回.0, 类型参数, 实参类型));
        let key = format!("{}|{}|{}", self.module, self.block, 实例名);
        self.fns.insert(
            key,
            FnSig {
                params: 具体形参.iter().map(|(_, t)| t.clone()).collect(),
                ret: 具体返回.clone(),
                is_outs: false,
                hidden: Vec::new(),
            },
        );
        let prev = 类型代换表.with(|c| {
            c.replace(Some((类型参数.to_vec(), 实参类型.to_vec())))
        });
        let r = self.stmt_fn_body(实例名, 具体形参, 具体返回, 体);
        类型代换表.with(|c| {
            c.replace(prev);
        });
        let f = r?;
        self.extra.push(f);
        Ok(())
    }

    //nt: 编语句体函数体
    fn stmt_fn_body(
        &mut self,
        name: &str,
        params: Vec<(String, Ty)>,
        ret: Ty,
        body: &[AstStmt],
    ) -> Result<Function, BuildError> {
        //nt: 末尾必须是 give
        let (last, init) = body.split_last().ok_or_else(|| BuildError {
            message: "语句体 fn 是空的——最后要写 `give 表达式`".into(),
            code: "E301".into(),
            line: CUR_LINE.with(|c| c.get()),
            file: String::new(),
        })?;
        let give_toks: Vec<Token> = match last {
            AstStmt::Line { toks, .. } if matches!(toks.first(), Some(Token::Kw("give"))) => {
                toks[1..].to_vec()
            }
            _ => {
                return Err(BuildError {
                    message: "语句体 fn 的最后一条必须是 `give 表达式`".into(),
                    code: "E301".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                })
            }
        };
        let key = format!("{}|{}|{}", self.module, self.block, name);
        let mut fb = Builder::new(
            format!("{}|{}|{}", self.module, self.block, name),
            self.f.file.clone(),
            false,
            false,
            false,
            false,
            false,
            false,
            Vec::new(),
            self.block.clone(),
            self.module.clone(),
            std::rc::Rc::clone(&self.types),
            std::rc::Rc::clone(&self.type_idx),
            std::rc::Rc::clone(&self.mod_by_tail),
            std::rc::Rc::clone(&self.global_fns),
            std::rc::Rc::clone(&self.cross_vals),
            std::rc::Rc::clone(&self.sum_variants),
            std::rc::Rc::clone(&self.pub_blocks),
            std::rc::Rc::clone(&self.hidden_items),
            std::rc::Rc::clone(&self.block_types),
        );
        fb.cur_col = self.cur_col;
        fb.cur_len = self.cur_len;
        //nt: 形参先建，槽参后补
        //nt: 与调用点同序
        for (pname, pty) in &params {
            let id = fb.new_local(Some(pname.clone()), pty.clone(), true, true);
            fb.names.insert(pname.clone(), id);
        }
        //nt: fn的槽隐藏参数
        let hidden = 注入槽参数_prep(&mut fb, &key);
        self.fns.insert(
            key.clone(),
            FnSig {
                params: params.iter().map(|(_, t)| t.clone()).collect(),
                ret: ret.clone(),
                is_outs: false,
                hidden,
            },
        );
        fb.语句体返回 = true;
        fb.body(init)?;
        let v = fb.expr(&give_toks)?;
        fb.set_exit(Exit::Return(Some(v)));
        let mut f = fb.finish()?;
        f.ret = Some(ret);
        Ok(f)
    }

    //nt: 走一段语句序列
    fn body(&mut self, stmts: &[AstStmt]) -> Result<(), BuildError> {
        let mut j = 0usize;
        while j < stmts.len() {
            match &stmts[j] {
                AstStmt::Line { line, col, len, toks } => {
                    self.cur_line = *line;
                    self.cur_col = *col;
                    self.cur_len = *len;
                    CUR_LINE.set(*line);
                    let t = toks.clone();
                    self.line(&t)?;
                    j += 1;
                }
                AstStmt::Fn { head, body, .. } => {
                    if let Some((name, 类型参数, 形参, 返回)) = 解析泛型语句头(head) {
                        泛型语句体fn表.with(|g| {
                            g.borrow_mut().insert(
                                name,
                                (类型参数, 形参, 返回, body.clone(), self.module.clone(), self.block.clone()),
                            );
                        });
                    } else {
                        let s = self.stmt_fn(head, body)?;
                        self.extra.push(s);
                    }
                    j += 1;
                }
                AstStmt::Struct { head, body, .. } => {
                    let kind = struct_kind(head)?;
                    match kind {
                        Kind::For => {
                            //nt: for i in 甲
                            let is_range = head.contains(&Token::Sym("~"));
                            let h = head.clone();
                            let ib = body.clone();
                            if is_range {
                                self.for_range_stmt(&h, &ib)?;
                            } else {
                                self.for_list_stmt(&h, &ib)?;
                            }
                            j += 1;
                        }
                        Kind::Else => return err("else 不能单独出现（须紧跟其 if 体的 ; 之后）"),
                        Kind::If => {
                            let eb: Option<Vec<AstStmt>> = match stmts.get(j + 1) {
                                Some(AstStmt::Struct { head: h, body: b, .. }) if is_else(h) => Some(b.clone()),
                                _ => None,
                            };
                            let h = head.clone();
                            let ib = body.clone();
                            //nt: 编译期分支裁剪
                            match self.条件常量(&h) {
                                Some(true) => self.body(&ib)?,
                                Some(false) => {
                                    if let Some(eb2) = eb.as_ref() {
                                        self.body(eb2)?;
                                    }
                                }
                                None => self.if_stmt(&h, &ib, eb.as_ref())?,
                            }
                            j += if eb.is_some() { 2 } else { 1 };
                        }
                        Kind::While => {
                            let h = head.clone();
                            let ib = body.clone();
                            self.while_stmt(&h, &ib)?;
                            j += 1;
                        }
                        Kind::Match => {
                            let h = head.clone();
                            let ib = body.clone();
                            self.match_stmt(&h, &ib)?;
                            j += 1;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}



//nt: 代换类型参数
fn 代换类型(s: &str) -> String {
    let m = 类型代换表.with(|c| c.borrow().clone());
    match m {
        Some((参数, 实参)) => 替换参数(s, &参数, &实参),
        None => s.to_string(),
    }
}

//nt: 解析类型引用
fn parse_type_ref(
    toks: &[Token],
    at: usize,
) -> Result<(Ty, Option<(String, String, String)>, usize), BuildError> {
    let (t, a, b) = parse_type_ref_raw(toks, at)?;
    Ok((Ty(代换类型(&t.0)), a, b))
}

fn parse_type_ref_raw(
    toks: &[Token],
    at: usize,
) -> Result<(Ty, Option<(String, String, String)>, usize), BuildError> {
    match toks.get(at) {
        Some(Token::Kw(t)) => {
            let 基 = (*t).to_string();
            if matches!(基.as_str(), "list" | "que" | "stack" | "map")
                && matches!(toks.get(at + 1), Some(Token::Sym("(")))
            {
                let mut 深 = 0i32;
                let mut 尽 = None;
                for (k, tk) in toks.iter().enumerate().skip(at + 1) {
                    match tk {
                        Token::Sym("(") => 深 += 1,
                        Token::Sym(")") => {
                            深 -= 1;
                            if 深 == 0 {
                                尽 = Some(k);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let 尽 = 尽.ok_or_else(|| BuildError {
                    message: format!("容器类型「{}」的括号没闭合", 基),
                    code: "E201".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                })?;
                let mut 实参: Vec<String> = Vec::new();
                for 段 in split_args(&toks[at + 2..尽]) {
                    let (ty, _, _) = parse_type_ref(&段, 0)?;
                    实参.push(ty.0);
                }
                Ok((Ty(format!("{}({})", 基, 实参.join(", "))), None, 尽 + 1))
            } else if 基 == "fn" && matches!(toks.get(at + 1), Some(Token::Sym("("))) {
                let mut 深 = 0i32;
                let mut 尽 = None;
                for (k, tk) in toks.iter().enumerate().skip(at + 1) {
                    match tk {
                        Token::Sym("(") => 深 += 1,
                        Token::Sym(")") => {
                            深 -= 1;
                            if 深 == 0 {
                                尽 = Some(k);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let 尽 = 尽.ok_or_else(|| BuildError {
                    message: "函数类型「fn」的括号没闭合".into(),
                    code: "E201".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                })?;
                let mut 实参: Vec<String> = Vec::new();
                for 段 in split_args(&toks[at + 2..尽]) {
                    let (t, _, _) = parse_type_ref(&段, 0)?;
                    实参.push(t.0);
                }
                let (ret, _, 止) = parse_type_ref(toks, 尽 + 1)?;
                Ok((Ty(format!("fn({}) {}", 实参.join(", "), ret.0)), None, 止))
            } else {
                Ok((Ty(基), None, at + 1))
            }
        }
        Some(Token::Ident(n)) => {
            //nt: 泛型类型用法
            let 参数 = GENERICS.with(|g| g.borrow().get(n).cloned());
            if let Some(ps) = 参数 {
                if !matches!(toks.get(at + 1), Some(Token::Sym("("))) {
                    return Err(BuildError {
                        message: format!("泛型类型「{}」要写类型参数（如 `{}(int, str)`）", n, n),
                        code: "E201".into(),
                        line: CUR_LINE.with(|c| c.get()),
                        file: String::new(),
                    });
                }
                let mut 深 = 0i32;
                let mut 尽 = None;
                for (k, t) in toks.iter().enumerate().skip(at + 1) {
                    match t {
                        Token::Sym("(") => 深 += 1,
                        Token::Sym(")") => {
                            深 -= 1;
                            if 深 == 0 {
                                尽 = Some(k);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let 尽 = 尽.ok_or_else(|| BuildError {
                    message: format!("泛型类型「{}」的实参括号没闭合", n),
                    code: "E201".into(),
                    line: CUR_LINE.with(|c| c.get()),
                    file: String::new(),
                })?;
                let mut 实参: Vec<String> = Vec::new();
                for 段 in split_args(&toks[at + 2..尽]) {
                    let (t, _, _) = parse_type_ref(&段, 0)?;
                    if t.0.ends_with('*') || !t.0.contains('(') {
                        实参.push(t.0);
                    } else {
                        return Err(BuildError {
                            message: format!("泛型实参只能是单层类型（`{}` 太复杂）", t.0),
                            code: "E201".into(),
                            line: CUR_LINE.with(|c| c.get()),
                            file: String::new(),
                        });
                    }
                }
                if 实参.len() != ps.len() {
                    return Err(BuildError {
                        message: format!(
                            "泛型类型「{}」要 {} 个类型参数，这里给了 {} 个",
                            n,
                            ps.len(),
                            实参.len()
                        ),
                        code: "E201".into(),
                        line: CUR_LINE.with(|c| c.get()),
                        file: String::new(),
                    });
                }
                INST_TODO.with(|q| q.borrow_mut().push((n.clone(), 实参.clone())));
                let 名 = format!("{}({})", n, 实参.join(", "));
                return Ok((Ty(名), None, 尽 + 1));
            }
            if matches!(toks.get(at + 1), Some(Token::Sym("*"))) {
                Ok((Ty(format!("{}*", n)), None, at + 2))
            } else {
                Ok((Ty(n.clone()), None, at + 1))
            }
        }
        Some(Token::HashColon) | Some(Token::HashHashColon) => {
            let cross_mod = matches!(toks.get(at), Some(Token::HashHashColon));
            let (blk_mod, colon_at) = if cross_mod {
                let mut m = String::new();
                let mut j = at + 1;
                while let Some(tok) = toks.get(j) {
                    match tok {
                        Token::Ident(x) | Token::Str(x) => {
                            if !m.is_empty() {
                                m.push('.');
                            }
                            m.push_str(x);
                            j += 1;
                            if matches!(toks.get(j), Some(Token::Sym("."))) {
                                j += 1;
                            } else {
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                if m.is_empty() || !matches!(toks.get(j), Some(Token::HashColon)) {
                    return err("`##:` 后应接模块名再接 `#:块`——形如 `##:前端.拆词 #:分句`");
                }
                (m, j)
            } else {
                (String::new(), at)
            };
            let blk = match toks.get(colon_at + 1) {
                Some(Token::Ident(n)) => n.clone(),
                Some(Token::Kw(k)) => (*k).to_string(),
                _ => return err("`#:` 后应接块名"),
            };
            if !matches!(toks.get(colon_at + 2), Some(Token::Kw("type"))) {
                return err("跨块取类型要写 `#:块 type \"名\"`");
            }
            let tname = match toks.get(colon_at + 3) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                _ => return err("`#:块 type` 后应接类型名"),
            };
            let mut end = colon_at + 4;
            let ty = if matches!(toks.get(end), Some(Token::Sym("*"))) {
                end += 1;
                Ty(format!("{}*", tname))
            } else {
                Ty(tname.clone())
            };
            Ok((ty, Some((blk_mod, blk, tname)), end))
        }
        _ => err("这里应当是类型（内置类型名 / 自定义类型名 / `#:块 type \"名\"`）"),
    }
}

//nt: 解析参数表
fn parse_params(toks: &[Token]) -> Result<(Vec<(String, Ty)>, usize), BuildError> {
    let open = toks
        .iter()
        .position(|t| *t == Token::Sym("("))
        .ok_or_else(|| BuildError { message: "add fn 缺参数表".into() , code: "E301".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
    let mut depth = 0i32;
    let mut end = None;
    for (k, t) in toks.iter().enumerate().skip(open) {
        match t {
            Token::Sym("(") => depth += 1,
            Token::Sym(")") => {
                depth -= 1;
                if depth == 0 {
                    end = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.ok_or_else(|| BuildError { message: "add fn 参数表没闭合".into() , code: "E301".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
    let mut out = Vec::new();
    for a in split_args(&toks[open + 1..end]) {
        let name = match a.first() {
            Some(Token::Ident(n)) | Some(Token::Str(n)) => n.clone(),
            _ => return err(format!("参数缺名字：{:?}", a)),
        };
        let (ty, _限定, _) = match a.get(1) {
            Some(_) => parse_type_ref(&a, 1)?,
            None => return err(format!("参数「{}」缺类型", name)),
        };
        out.push((name, ty));
    }
    Ok((out, end + 1))
}

fn between_parens(toks: &[Token]) -> Result<Vec<Token>, BuildError> {
    let open = toks
        .iter()
        .position(|t| *t == Token::Sym("("))
        .ok_or_else(|| BuildError { message: "print 后缺 (".into() , code: "E301".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })?;
    let mut depth = 0i32;
    let mut out = Vec::new();
    for t in &toks[open..] {
        match t {
            Token::Sym("(") => {
                depth += 1;
                if depth == 1 {
                    continue;
                }
            }
            Token::Sym(")") => {
                depth -= 1;
                if depth == 0 {
                    return Ok(out);
                }
            }
            _ => {}
        }
        out.push(t.clone());
    }
    err("括号没闭合")
}

fn split_args(inner: &[Token]) -> Vec<Vec<Token>> {
    let mut out: Vec<Vec<Token>> = Vec::new();
    let mut cur = Vec::new();
    let mut depth = 0i32;
    for t in inner {
        match t {
            Token::Sym("(") => depth += 1,
            Token::Sym(")") => depth -= 1,
            //nt: 方括号/花括号同样是
            Token::Sym("[") | Token::Sym("{") => depth += 1,
            Token::Sym("]") | Token::Sym("}") => depth -= 1,
            Token::Sym(",") if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(t.clone());
    }
    if !cur.is_empty() || !out.is_empty() {
        out.push(cur);
    }
    out.into_iter().filter(|a| !a.is_empty()).collect()
}

//nt: 判addtype行

fn is_add_type(toks: &[Token]) -> bool {
    matches!(toks.first(), Some(Token::Kw("add"))) && matches!(toks.get(1), Some(Token::Kw("type")))
}

//nt: add type "
fn parse_type_def(toks: &[Token]) -> Result<StructType, BuildError> {
    let name = match toks.get(2) {
        Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
        _ => return err("add type 后应是类型名"),
    };
    //nt: 泛型参数表
    let mut params: Vec<String> = Vec::new();
    let mut 名后 = 3usize;
    if matches!(toks.get(3), Some(Token::Sym("("))) {
        let mut 深度 = 0i32;
        let mut 尽 = None;
        for (k, t) in toks.iter().enumerate().skip(3) {
            match t {
                Token::Sym("(") => 深度 += 1,
                Token::Sym(")") => {
                    深度 -= 1;
                    if 深度 == 0 {
                        尽 = Some(k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let 尽 = 尽.ok_or_else(|| BuildError {
            message: "泛型参数表括号没闭合".into(),
            code: "E201".into(),
            line: CUR_LINE.with(|c| c.get()),
            file: String::new(),
        })?;
        for 项 in split_args(&toks[4..尽]) {
            match 项.as_slice() {
                [Token::Ident(n)] => params.push(n.clone()),
                _ => {
                    return Err(BuildError {
                        message: "泛型参数表里只能写裸名（`add type \"对子\"(A, B) = …`）".into(),
                        code: "E201".into(),
                        line: CUR_LINE.with(|c| c.get()),
                        file: String::new(),
                    })
                }
            }
        }
        if params.is_empty() {
            return Err(BuildError {
                message: "泛型参数表的括号是空的".into(),
                code: "E201".into(),
                line: CUR_LINE.with(|c| c.get()),
                file: String::new(),
            });
        }
        名后 = 尽 + 1;
    }
    let eq = match toks.iter().skip(名后).position(|t| *t == Token::Sym("=")) {
        Some(k) => 名后 + k,
        None => {
            return err(
                "add type 要写 `=`——积类型 `add type \"名\" = (字段…)`；和类型 `add type \"名\" = 变体(…) | …`",
            )
        }
    };
    //nt: 积类型
    if matches!(toks.get(eq + 1), Some(Token::Sym("("))) {
        let fields = parse_field_list(&toks[eq + 2..])?;
        return Ok(StructType { name, params, fields, variants: Vec::new() });
    }
    //nt: 和类型
    let mut variants = Vec::new();
    for seg in split_top(&toks[eq + 1..]) {
        if seg.is_empty() {
            continue;
        }
        let vname = match seg.first() {
            Some(Token::Ident(n)) | Some(Token::Str(n)) => n.clone(),
            _ => return err("变体缺名字"),
        };
        let fields = if matches!(seg.get(1), Some(Token::Sym("("))) {
            parse_variant_fields(&seg[2..])?
        } else {
            Vec::new()
        };
        variants.push(Variant { name: vname, fields });
    }
    if variants.is_empty() {
        return err("和类型至少要有一个变体");
    }
    Ok(StructType { name, params, fields: Vec::new(), variants })
}

//nt: 解析积类型字段
fn parse_field_list(toks: &[Token]) -> Result<Vec<(String, Ty)>, BuildError> {
    let end = matching_close(toks)?;
    let mut fields = Vec::new();
    for a in split_args(&toks[..end]) {
        let fname = match a.first() {
            Some(Token::Ident(n)) | Some(Token::Str(n)) => n.clone(),
            _ => return err("字段缺名字"),
        };
        //nt: 类型带泛型实参
        let 尾 = &a[1..];
        if 尾.is_empty() {
            return err(format!("字段「{}」缺类型", fname));
        }
        let mut fty = String::new();
        for t in 尾.iter() {
            match t {
                Token::Kw(x) => {
                    if !fty.is_empty()
                        && !fty.ends_with('(')
                        && !fty.ends_with(',')
                        && !fty.ends_with(' ')
                    {
                        fty.push(' ');
                    }
                    fty.push_str(x);
                }
                Token::Ident(x) => {
                    if !fty.is_empty()
                        && !fty.ends_with('(')
                        && !fty.ends_with(',')
                        && !fty.ends_with(' ')
                    {
                        fty.push(' ');
                    }
                    fty.push_str(x);
                }
                Token::Sym("(") => fty.push('('),
                Token::Sym(")") => fty.push(')'),
                Token::Sym(",") => fty.push_str(", "),
                Token::Sym("*") => fty.push('*'),
                _ => {}
            }
        }
        fields.push((fname, Ty(fty)));
    }
    Ok(fields)
}

//nt: 解析变体字段
//nt: 无名者按序号
fn parse_variant_fields(toks: &[Token]) -> Result<Vec<(String, Ty)>, BuildError> {
    let end = matching_close(toks)?;
    let mut fields = Vec::new();
    for (idx, a) in split_args(&toks[..end]).into_iter().enumerate() {
        //nt: 具名字段
        let (fname, fty_tok) = if a.len() >= 2 && !matches!(a.get(1), Some(Token::Sym("*"))) {
            match a.first() {
                Some(Token::Ident(n)) | Some(Token::Str(n)) => (n.clone(), a.get(1)),
                _ => (format!("#{}", idx), a.first()),
            }
        } else {
            (format!("#{}", idx), a.first())
        };
        let fty = match fty_tok {
            Some(Token::Kw(t)) => (*t).to_string(),
            Some(Token::Ident(n)) => n.clone(),
            _ => return err(format!("变体字段「{}」缺类型", fname)),
        };
        //nt: 把手 `*` 可落在
        let fty = if a.iter().any(|t| matches!(t, Token::Sym("*"))) {
            format!("{}*", fty)
        } else {
            fty
        };
        fields.push((fname, Ty(fty)));
    }
    Ok(fields)
}

//nt: 取整数字面量
fn int_literal_val(toks: &[Token]) -> Option<i64> {
    match toks {
        [Token::Int(v)] => Some(*v),
        [Token::Sym("-"), Token::Int(v)] => Some(-v),
        _ => None,
    }
}

//nt: 定长族的可表示范围
fn width_range(ty: &str) -> Option<(i64, i64)> {
    match ty {
        "int_8" => Some((-128, 127)),
        "uint_8" => Some((0, 255)),
        "int_16" => Some((-32_768, 32_767)),
        "uint_16" => Some((0, 65_535)),
        "int_32" => Some((-2_147_483_648, 2_147_483_647)),
        "uint_32" => Some((0, 4_294_967_295)),
        "int_64" | "uint_64" | _ => None,
    }
}

fn matching_close(toks: &[Token]) -> Result<usize, BuildError> {
    let mut depth = 0i32;
    for (k, t) in toks.iter().enumerate() {
        match t {
            Token::Sym("(") => depth += 1,
            Token::Sym(")") => {
                if depth == 0 {
                    return Ok(k);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    Err(BuildError { message: "字段表没闭合".into() , code: "E301".into() , line: CUR_LINE.with(|c| c.get()) , file: String::new() })
}

//nt: 按 `|` 做**顶
fn split_top(toks: &[Token]) -> Vec<Vec<Token>> {
    let mut out = Vec::new();
    let mut cur: Vec<Token> = Vec::new();
    let mut depth = 0i32;
    for t in toks {
        match t {
            Token::Sym("(") => {
                depth += 1;
                cur.push(t.clone());
            }
            Token::Sym(")") => {
                depth -= 1;
                cur.push(t.clone());
            }
            Token::Sym(s) if s.starts_with('|') && s.len() == 1 && depth == 0 => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(t.clone()),
        }
    }
    out.push(cur);
    out
}

//nt: call ##
//nt: 0c'
//nt: call ##
fn take_kind(toks: &[Token]) -> Option<&'static str> {
    let mut j = 2usize;
    while let Some(tok) = toks.get(j) {
        match tok {
            Token::Ident(_) | Token::Str(_) => {
                j += 1;
                if matches!(toks.get(j), Some(Token::Sym("."))) {
                    j += 1;
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    if !matches!(toks.get(j), Some(Token::HashColon)) {
        return None;
    }
    match toks.get(j + 2) {
        Some(Token::Kw("val")) => Some("val"),
        Some(Token::Kw("fn")) => Some("fn"),
        _ => None,
    }
}

fn is_take_form(toks: &[Token]) -> bool {
    let mut j = 2usize;
    while let Some(tok) = toks.get(j) {
        match tok {
            Token::Ident(_) | Token::Str(_) => {
                j += 1;
                if matches!(toks.get(j), Some(Token::Sym("."))) {
                    j += 1;
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    matches!(toks.get(j), Some(Token::HashColon))
        && (matches!(toks.get(j + 2), Some(Token::Kw("val")))
            || matches!(toks.get(j + 2), Some(Token::Kw("fn"))))
}

fn build_block(
    b: &AstBlock,
    types: &[StructType],
    type_idx: &std::rc::Rc<std::collections::HashMap<String, usize>>,
    global_fns: &BTreeMap<String, FnSig>,
    cross_vals: &BTreeMap<String, BTreeMap<String, Ty>>,
    sum_variants: &BTreeMap<String, (String, usize, Vec<Ty>)>,
    mod_by_tail: &BTreeMap<String, Vec<String>>,
    pub_blocks: &BTreeSet<String>,
    hidden_items: &BTreeSet<String>,
    block_types: &BTreeMap<String, BTreeSet<String>>,
) -> Result<(Function, Vec<Function>, Vec<ExternFn>), BuildError> {
    let mut bx = Builder::new(
        b.name.clone(),
        b.file.clone(),
        true, // 代码块函数——链式执行的候选链根
        b.attrs.iter().any(|a| a == "pub"),
        b.attrs.iter().any(|a| a == "sle"),
        b.attrs.iter().any(|a| a == "kill"),
        b.attrs.iter().any(|a| a == "ABI"),
        b.attrs.iter().any(|a| a == "asm"),
        b.asm_lines.clone(),
        b.name.clone(),
        b.module.clone(),
        std::rc::Rc::new(types.to_vec()),
        std::rc::Rc::clone(type_idx),
        std::rc::Rc::new(mod_by_tail.clone()),
        std::rc::Rc::new(global_fns.clone()),
        std::rc::Rc::new(cross_vals.clone()),
        std::rc::Rc::new(sum_variants.clone()),
        std::rc::Rc::new(pub_blocks.clone()),
        std::rc::Rc::new(hidden_items.clone()),
        std::rc::Rc::new(block_types.clone()),
    );
    bx.body(&b.stmts)?;
    let extra = std::mem::take(&mut bx.extra);
    let externs = std::mem::take(&mut bx.externs);
    Ok((bx.finish()?, extra, externs))
}

//nt: 预扫泛型语句模板
fn 预扫泛型语句体(stmts: &[AstStmt], module: &str, block: &str) {
    for s in stmts {
        match s {
            AstStmt::Fn { head, body, .. } => {
                if let Some((name, 类型参数, 形参, 返回)) = 解析泛型语句头(head) {
                    泛型语句体fn表.with(|g| {
                        g.borrow_mut().insert(
                            name,
                            (类型参数, 形参, 返回, body.clone(), module.to_string(), block.to_string()),
                        );
                    });
                }
                预扫泛型语句体(body, module, block);
            }
            AstStmt::Struct { body, .. } => 预扫泛型语句体(body, module, block),
            _ => {}
        }
    }
}

//nt: 收引用返槽参数
fn 注入槽参数_prep(fb: &mut Builder, key: &str) -> Vec<(String, Ty)> {
    let used = fn_used表.with(|g| g.borrow().get(key).cloned()).unwrap_or_default();
    for (vn, vty) in &used {
        let pid = fb.new_local(Some(vn.clone()), Ty(format!("{}*", vty.0)), true, false);
        fb.exposed.insert(vn.clone(), pid);
    }
    used.into_iter().collect()
}

//nt: fn头里的名
fn head_tokens_name(head: &TokStream) -> Option<String> {
    let mut i = 0usize;
    while let Some(tk) = head.get(i) {
        if let Token::Kw("fn") = tk {
            return match head.get(i + 1) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => Some(n.clone()),
                _ => None,
            };
        }
        i += 1;
    }
    None
}

//nt: 扫一行引用
fn 扫行引用(
    toks: &TokStream,
    module: &str,
    block: &str,
    fname: &str,
    fn局部: &BTreeSet<String>,
    val_tys: &BTreeMap<String, Ty>,
    fn_used: &mut BTreeMap<String, BTreeMap<String, Ty>>,
    fn_calls: &mut BTreeMap<String, BTreeSet<String>>,
) {
    let key = format!("{}|{}|{}", module, block, fname);
    let mut i = 0usize;
    while let Some(tk) = toks.get(i) {
        match tk {
            //nt: val读写
            Token::Kw("val") => {
                let 前是建 = i > 0
                    && (matches!(toks.get(i - 1), Some(Token::Kw("add")))
                        || (i > 1 && matches!(toks.get(i - 2), Some(Token::Kw("hide")))));
                if !前是建 {
                    if let (Some(Token::Str(v)) | Some(Token::Ident(v))) = toks.get(i + 1) {
                        if !fn局部.contains(v) {
                            if let Some(ty) = val_tys
                                .get(&format!("{}|{}|{}", module, block, v))
                                .cloned()
                            {
                                fn_used.entry(key.clone()).or_default().insert(v.clone(), ty);
                            }
                        }
                    }
                }
                i += 2;
            }
            //nt: call val读
            Token::Kw("call") if matches!(toks.get(i + 1), Some(Token::Kw("val"))) => {
                if let (Some(Token::Str(v)) | Some(Token::Ident(v))) = toks.get(i + 2) {
                    if !fn局部.contains(v) {
                        if let Some(ty) = val_tys
                            .get(&format!("{}|{}|{}", module, block, v))
                            .cloned()
                        {
                            fn_used.entry(key.clone()).or_default().insert(v.clone(), ty);
                        }
                    }
                }
                i += 3;
            }
            //nt: 同块调用边
            Token::Kw("call") if matches!(toks.get(i + 1), Some(Token::Kw("fn"))) => {
                if let (Some(Token::Str(c)) | Some(Token::Ident(c))) = toks.get(i + 2) {
                    fn_calls
                        .entry(key.clone())
                        .or_default()
                        .insert(format!("{}|{}|{}", module, block, c));
                }
                i += 3;
            }
            //nt: 跨块调用边
            Token::Kw("call") if matches!(toks.get(i + 1), Some(Token::HashColon)) => {
                if matches!(toks.get(i + 3), Some(Token::Kw("fn"))) {
                    let blk = match toks.get(i + 2) {
                        Some(Token::Ident(b)) => b.clone(),
                        Some(Token::Kw(b)) => (*b).to_string(),
                        _ => String::new(),
                    };
                    if let (Some(Token::Str(c)) | Some(Token::Ident(c))) = toks.get(i + 4) {
                        if !blk.is_empty() {
                            fn_calls
                                .entry(key.clone())
                                .or_default()
                                .insert(format!("{}|{}|{}", module, blk, c));
                        }
                    }
                }
                i += 3;
            }
            //nt: 公式体裸名
            Token::Ident(v) => {
                let 前是建 = i > 0
                    && (matches!(toks.get(i - 1), Some(Token::Kw("add")))
                        || (i > 1 && matches!(toks.get(i - 2), Some(Token::Kw("hide")))));
                if !前是建 && !fn局部.contains(v) {
                    if let Some(ty) =
                        val_tys.get(&format!("{}|{}|{}", module, block, v)).cloned()
                    {
                        fn_used.entry(key.clone()).or_default().insert(v.clone(), ty);
                    }
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
}

//nt: 扫fn体引用
#[allow(clippy::too_many_arguments)]
fn 扫fn引用(
    stmts: &[AstStmt],
    head: &TokStream,
    module: &str,
    block: &str,
    fname: &str,
    参数名: &BTreeSet<String>,
    val_tys: &BTreeMap<String, Ty>,
    fn_used: &mut BTreeMap<String, BTreeMap<String, Ty>>,
    fn_calls: &mut BTreeMap<String, BTreeSet<String>>,
) {
    //nt: 一遍收fn局部名
    let mut fn局部: BTreeSet<String> = 参数名.clone();
    收局部名(stmts, &mut fn局部);
    //nt: 二遍记引用
    扫行引用(head, module, block, fname, &fn局部, val_tys, fn_used, fn_calls);
    for s in stmts {
        match s {
            AstStmt::Line { toks, .. } => {
                扫行引用(toks, module, block, fname, &fn局部, val_tys, fn_used, fn_calls);
            }
            AstStmt::Struct { head, body, .. } => {
                //nt: 条件头也要扫
                扫行引用(head, module, block, fname, &fn局部, val_tys, fn_used, fn_calls);
                扫fn引用(body, head, module, block, fname, &fn局部, val_tys, fn_used, fn_calls);
            }
            _ => {}
        }
    }
}

//nt: 收fn局部名
fn 收局部名(stmts: &[AstStmt], out: &mut BTreeSet<String>) {
    for s in stmts {
        match s {
            AstStmt::Line { toks, .. } => {
                let mut i = 0usize;
                while let Some(tk) = toks.get(i) {
                    if matches!(tk, Token::Kw("add")) || matches!(tk, Token::Kw("hide")) {
                        let at = if matches!(tk, Token::Kw("hide")) { i + 2 } else { i + 1 };
                        if matches!(toks.get(at), Some(Token::Kw("val"))) {
                            if let (Some(Token::Str(v)) | Some(Token::Ident(v))) = toks.get(at + 1) {
                                out.insert(v.clone());
                            }
                        }
                        i = at + 1;
                        continue;
                    }
                    i += 1;
                }
            }
            AstStmt::Struct { body, .. } | AstStmt::Fn { body, .. } => {
                收局部名(body, out);
            }
            _ => {}
        }
    }
}

//nt: 语法层语句树 → 中
pub fn build(ast: &AstProgram) -> Result<Program, BuildError> {
    //nt: 预扫类型名
    //nt: Anle 删了 us
    //nt: 必须唯一才能指称
    //nt: 和类型变体名同一规则
    let mut types: Vec<StructType> = Vec::new();
    let mut type_first: BTreeMap<String, (String, usize)> = BTreeMap::new(); // 名 → (文件, 行号) 首次定义
    //nt: 首次定义位置
    let base_of = |f: &str| -> String { if f.is_empty() { "本文件".to_string() } else { f.to_string() } };
    for b in &ast.blocks {
        let mut flat_ln: Vec<(usize, &TokStream)> = Vec::new();
        collect_ast_lines_ln(&b.stmts, &mut flat_ln);
        for (line, toks) in flat_ln {
                if is_add_type(toks) {
                    if let Some(Token::Str(n) | Token::Ident(n)) = toks.get(2) {
                        if let Some((f0, l0)) = type_first.get(n) {
                            return Err(BuildError {
                                message: format!(
                                    "类型「{}」重复定义——第一次在 {} 第 {} 行；类型名全程序唯一（跨模块也一样），请改写名",
                                    n, base_of(f0), l0
                                ),
                                code: "E201".into(),
                                line: line,
                                file: b.file.clone(),
                            });
                        }
                        type_first.insert(n.clone(), (b.file.clone(), line));
                    }
                    let st = parse_type_def(toks)?;
                    for v in &st.variants {
                        if let Some((f0, l0)) = type_first.get(&v.name) {
                            return Err(BuildError {
                                message: format!(
                                    "变体「{}」重复定义——第一次在 {} 第 {} 行；变体名全程序唯一，请改写名",
                                    v.name, base_of(f0), l0
                                ),
                                code: "E201".into(),
                                line: line,
                                file: b.file.clone(),
                            });
                        }
                        type_first.insert(v.name.clone(), (b.file.clone(), line));
                    }
                    types.push(st);
                }
                    }
    }

    //nt: 预扫泛型语句模板
    for b in &ast.blocks {
        预扫泛型语句体(&b.stmts, &b.module, &b.name);
    }

    //nt: 预扫函数签名
    let mut global_fns: BTreeMap<String, FnSig> = BTreeMap::new();
    for b in &ast.blocks {
        //nt: 递归展开
        //nt: 只扫顺序语句会漏体内
        let mut flat_lines: Vec<&TokStream> = Vec::new();
        collect_ast_lines(&b.stmts, &mut flat_lines);
        for toks in flat_lines {
            if matches!(toks.first(), Some(Token::Kw("add")))
                && matches!(toks.get(1), Some(Token::Kw("fn")))
            {
                let name = match toks.get(2) {
                    Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                    _ => continue,
                };
                if let Ok((params, after)) = parse_params(toks) {
//nt: 返回类型
                    //nt: 返回自定义类型的 f
                    if let Ok((ret, _限, _)) = parse_type_ref(toks, after) {
                        global_fns.insert(
                            format!("{}|{}|{}", b.module, b.name, name),
                            FnSig {
                                params: params.iter().map(|(_, t)| t.clone()).collect(),
                                ret,
                                is_outs: false,
                                hidden: Vec::new(),
                            },
                        );
                    }
                }
            }
            //nt: 外部函数声明同样进全
            if matches!(toks.first(), Some(Token::Kw("add")))
                && matches!(toks.get(1), Some(Token::Kw("outs")))
                && matches!(toks.get(2), Some(Token::Kw("fn")))
            {
                let name = match toks.get(3) {
                    Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                    _ => continue,
                };
                if let Ok((params, after)) = parse_params(toks) {
                    if let Ok((ret, _限, _)) = parse_type_ref(toks, after) {
                        global_fns.insert(
                            format!("{}|{}|{}", b.module, b.name, name),
                            FnSig {
                                params: params.iter().map(|(_, t)| t.clone()).collect(),
                                ret,
                                is_outs: true,
                                hidden: Vec::new(),
                            },
                        );
                    }
                }
            }
                }
    }

    //nt: 预扫值类型
    //nt: 先收全程序 val
    let mut val_tys: BTreeMap<String, Ty> = BTreeMap::new();
    for b in &ast.blocks {
        let mut flat_lines: Vec<&TokStream> = Vec::new();
        collect_ast_lines(&b.stmts, &mut flat_lines);
        for toks in flat_lines {
                if matches!(toks.first(), Some(Token::Kw("add")))
                    && matches!(toks.get(1), Some(Token::Kw("val")))
                {
                    let name = match toks.get(2) {
                        Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                        _ => continue,
                    };
                    let ty = match container_ty_at(toks, 3) {
                        Some(t) => Ty(t),
                        None => match toks.get(3) {
                            Some(Token::Kw(t)) => Ty((*t).to_string()),
                            Some(Token::Ident(n)) => {
                                if matches!(toks.get(4), Some(Token::Sym("*"))) {
                                    Ty(format!("{}*", n))
                                } else {
                                    Ty(n.clone())
                                }
                            }
                            _ => continue,
                        },
                    };
                    val_tys.insert(format!("{}|{}|{}", b.module, b.name, name), ty);
                }
                    }
    }
    //nt: fn引用与调用边
    let mut fn_used: BTreeMap<String, BTreeMap<String, Ty>> = BTreeMap::new();
    let mut fn_calls: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    //nt: perfn扫描
    for b in &ast.blocks {
        for s in &b.stmts {
            if let AstStmt::Fn { body, head, .. } = s {
                let fname = head_tokens_name(head);
                if let Some(fname) = fname {
                    let mut fn局部: BTreeSet<String> = BTreeSet::new();
                    let mut k = 0usize;
                    let mut 深度 = 0i32;
                    while let Some(tk) = head.get(k) {
                        match tk {
                            Token::Sym("(") => {
                                深度 += 1;
                                k += 1;
                                continue;
                            }
                            Token::Sym(")") => break,
                            _ => {}
                        }
                        if 深度 > 0 {
                            if let (Token::Str(pn) | Token::Ident(pn)) = tk {
                                fn局部.insert(pn.clone());
                            }
                        }
                        k += 1;
                    }
                    收局部名(body, &mut fn局部);
                    扫fn引用(body, head, &b.module, &b.name, &fname, &fn局部, &val_tys, &mut fn_used, &mut fn_calls);
                }
            } else if let AstStmt::Line { toks, .. } = s {
                //nt: 公式体fn也要扫
                if matches!(toks.first(), Some(Token::Kw("add")))
                    && matches!(toks.get(1), Some(Token::Kw("fn")))
                    && toks.iter().any(|t| matches!(t, Token::Sym("=")))
                {
                    let fname = match toks.get(2) {
                        Some(Token::Str(n)) | Some(Token::Ident(n)) => Some(n.clone()),
                        _ => None,
                    };
                    if let Some(fname) = fname {
                        let mut 参数名: BTreeSet<String> = BTreeSet::new();
                        let mut k = 0usize;
                        let mut 深度 = 0i32;
                        while k < toks.len() {
                            match toks.get(k) {
                                Some(Token::Sym("(")) => {
                                    深度 += 1;
                                    k += 1;
                                    continue;
                                }
                                Some(Token::Sym(")")) => {
                                    if 深度 > 0 {
                                        深度 -= 1;
                                        if 深度 == 0 {
                                            k += 1;
                                            continue;
                                        }
                                    }
                                }
                                _ => {}
                            }
                            if 深度 > 0 {
                                if let (Some(Token::Str(pn)) | Some(Token::Ident(pn))) = toks.get(k) {
                                    参数名.insert(pn.clone());
                                }
                            }
                            k += 1;
                        }
                        扫行引用(toks, &b.module, &b.name, &fname, &参数名, &val_tys, &mut fn_used, &mut fn_calls);
                    }
                }
            }
        }
    }
    //nt: 调用图闭包
    let mut changed = true;
    while changed {
        changed = false;
        let calls: Vec<(String, BTreeSet<String>)> =
            fn_calls.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (caller, callees) in calls {
            let used = fn_used.get(&caller).cloned().unwrap_or_default();
            for c in callees {
                let cu = fn_used.get(&c).cloned().unwrap_or_default();
                for (vn, vt) in cu {
                    let e = fn_used.entry(caller.clone()).or_default();
                    if !e.contains_key(&vn) {
                        e.insert(vn, vt);
                        changed = true;
                    }
                }
            }
        }
    }
    //nt: 补填global
    for (k, sig) in global_fns.iter_mut() {
        if sig.hidden.is_empty() {
            if let Some(used) = fn_used.get(k) {
                sig.hidden = used.iter().map(|(v, t)| (v.clone(), t.clone())).collect();
            }
        }
    }
    fn_used表.with(|g| *g.borrow_mut() = fn_used.clone());
    let mut cross_vals: BTreeMap<String, BTreeMap<String, Ty>> = BTreeMap::new();
    //nt: 被引块的val暴露
    let mut 被引块: BTreeSet<String> = BTreeSet::new();
    for (k, _) in &fn_calls {
        let mut it = k.splitn(3, '|');
        let m = it.next().unwrap_or("");
        let blk = it.next().unwrap_or("");
        被引块.insert(format!("{}|{}", m, blk));
    }
    //nt: fn引用的val暴露
    for (k, used) in &fn_used {
        let mut it = k.splitn(3, '|');
        let m = it.next().unwrap_or("");
        let blk = it.next().unwrap_or("");
        if !被引块.contains(&format!("{}|{}", m, blk)) {
            continue;
        }
        for (v, ty) in used {
            cross_vals
                .entry(format!("{}|{}", m, blk))
                .or_default()
                .insert(v.clone(), ty.clone());
        }
    }
    fn_used表.with(|g| *g.borrow_mut() = fn_used.clone());
//nt: 模块尾名 → 全路径
    let mut mod_by_tail: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for b in &ast.blocks {
        let m = &b.module;
        if !m.is_empty() {
            let tail = m.rsplit('.').next().unwrap_or(m).to_string();
            let v = mod_by_tail.entry(tail).or_default();
            if !v.contains(m) {
                v.push(m.clone());
            }
        }
    }
    for b in &ast.blocks {
        let mut flat_lines: Vec<&TokStream> = Vec::new();
        collect_ast_lines(&b.stmts, &mut flat_lines);
        for toks in flat_lines {
                let mut i = 0usize;
                while i + 4 < toks.len() {
                    if matches!(toks.get(i), Some(Token::Kw("call")))
                        && matches!(toks.get(i + 1), Some(Token::HashColon))
                        && matches!(toks.get(i + 3), Some(Token::Kw("val")))
                    {
                        let blk = match toks.get(i + 2) {
                            Some(Token::Ident(n)) => n.clone(),
                            Some(Token::Kw(k)) => (*k).to_string(),
                            _ => { i += 1; continue; }
                        };
                        if let Some(Token::Str(v) | Token::Ident(v)) = toks.get(i + 4) {
                            let ty = val_tys.get(&format!("{}|{}|{}", b.module, blk, v)).cloned();
                            if let Some(ty) = ty {
                                cross_vals
                                    .entry(format!("{}|{}", b.module, blk))
                                    .or_default()
                                    .insert(v.clone(), ty);
                            }
                        }
                    }
                    //nt: 跨模块取 val
                    if matches!(toks.get(i), Some(Token::Kw("call")))
                        && matches!(toks.get(i + 1), Some(Token::HashHashColon))
                    {
                        let mut m = String::new();
                        let mut j = i + 2;
                        while let Some(tok) = toks.get(j) {
                            match tok {
                                Token::Ident(n) | Token::Str(n) => {
                                    if !m.is_empty() {
                                        m.push('.');
                                    }
                                    m.push_str(n);
                                    j += 1;
                                    if matches!(toks.get(j), Some(Token::Sym("."))) {
                                        j += 1;
                                    } else {
                                        break;
                                    }
                                }
                                _ => break,
                            }
                        }
                        if !m.is_empty()
                            && matches!(toks.get(j), Some(Token::HashColon))
                            && matches!(toks.get(j + 2), Some(Token::Kw("val")))
                        {
                            let blk = match toks.get(j + 1) {
                                Some(Token::Ident(n)) => n.clone(),
                                Some(Token::Kw(k)) => (*k).to_string(),
                                _ => { i += 1; continue; }
                            };
                            if let Some(Token::Str(v) | Token::Ident(v)) = toks.get(j + 3) {
                                //nt: 尾名退回
                                let known = mod_by_tail.values().flatten().any(|x| x == &m);
                                let full = if known {
                                    m.clone()
                                } else {
                                    match mod_by_tail.get(m.rsplit('.').next().unwrap_or(&m)) {
                                        Some(v) if v.len() == 1 => v[0].clone(),
                                        _ => String::new(),
                                    }
                                };
                                if !full.is_empty() {
                                    let ty = val_tys.get(&format!("{}|{}|{}", full, blk, v)).cloned();
                                    if let Some(ty) = ty {
                                        cross_vals
                                            .entry(format!("{}|{}", full, blk))
                                            .or_default()
                                            .insert(v.clone(), ty);
                                    }
                                }
                            }
                        }
                    }
//nt: take #
                    //nt: take 是跨块通道
                    if matches!(toks.get(i), Some(Token::Kw("take")))
                        && matches!(toks.get(i + 1), Some(Token::HashColon))
                        && matches!(toks.get(i + 3), Some(Token::Kw("val")))
                    {
                        let blk = match toks.get(i + 2) {
                            Some(Token::Ident(n)) => n.clone(),
                            Some(Token::Kw(k)) => (*k).to_string(),
                            _ => { i += 1; continue; }
                        };
                        if let Some(Token::Str(v) | Token::Ident(v)) = toks.get(i + 4) {
                            let ty = val_tys.get(&format!("{}|{}|{}", b.module, blk, v)).cloned();
                            if let Some(ty) = ty {
                                cross_vals
                                    .entry(format!("{}|{}", b.module, blk))
                                    .or_default()
                                    .insert(v.clone(), ty);
                            }
                        }
                    }
                    i += 1;
                }
                    }
    }

    //nt: 预扫和类型
    let mut sum_variants: BTreeMap<String, (String, usize, Vec<Ty>)> = BTreeMap::new();
    for st in &types {
        for (tag, v) in st.variants.iter().enumerate() {
            sum_variants.insert(
                v.name.clone(),
                (st.name.clone(), tag, v.fields.iter().map(|(_, t)| t.clone()).collect()),
            );
        }
    }

    //nt: 预扫块类型
    let mut block_types: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for b in &ast.blocks {
        let key = format!("{}|{}", b.module, b.name);
        let mut flat: Vec<&TokStream> = Vec::new();
        collect_ast_lines(&b.stmts, &mut flat);
        for toks in flat {
                if is_add_type(toks) {
                    if let Some(Token::Str(n) | Token::Ident(n)) = toks.get(2) {
                        block_types.entry(key.clone()).or_default().insert(n.clone());
                    }
                }
                    }
    }

//nt: 预扫公开块
    let pub_blocks: BTreeSet<String> = ast
        .blocks
        .iter()
        .filter(|b| b.attrs.iter().any(|a| a == "pub"))
        .map(|b| format!("{}|{}", b.module, b.name))
        .collect();

    //nt: 预扫隐藏项
    let mut hidden_items: BTreeSet<String> = BTreeSet::new();
    for b in &ast.blocks {
        let is_pub = pub_blocks.contains(&format!("{}|{}", b.module, b.name));
        let mut lines: Vec<(usize, &TokStream)> = Vec::new();
        collect_ast_lines_ln(&b.stmts, &mut lines);
        for (line_no, toks) in lines {
            if !matches!(toks.first(), Some(Token::Kw("add")))
                || !matches!(toks.get(1), Some(Token::Kw("hide")))
            {
                continue;
            }
            let (cat, what) = match toks.get(2) {
                Some(Token::Kw("fn")) => ("fn", "函数"),
                Some(Token::Kw("type")) => ("type", "类型"),
                _ => {
                    return Err(BuildError {
                        message: format!(
                            "hide 只能对 fn / type 写（`add hide fn \"名\"` / `add hide type \"名\"`）——val 本就在公开面里，不许单独藏"
                        ),
                        code: "E203".into(),
                        line: line_no,
                        file: b.file.clone(),
                    })
                }
            };
            let name = match toks.get(3) {
                Some(Token::Str(n)) | Some(Token::Ident(n)) => n.clone(),
                _ => {
                    return Err(BuildError {
                        message: format!("`add hide {}` 后应接{}名", cat, what),
                        code: "E203".into(),
                        line: line_no,
                        file: b.file.clone(),
                    })
                }
            };
            if !is_pub {
                return Err(BuildError {
                    message: format!(
                        "块「{}」不是 pub 块，不许 hide——hide 是给 pub 块「开窗又拉帘」用的",
                        b.name
                    ),
                    code: "E203".into(),
                    line: line_no,
                    file: b.file.clone(),
                });
            }
            hidden_items.insert(format!("{}|{}|{}|{}", b.module, b.name, cat, name));
        }
    }

    //nt: 泛型声明表就位
    GENERICS.with(|g| {
        let mut g = g.borrow_mut();
        g.clear();
        for t in &types {
            if !t.params.is_empty() {
                g.insert(t.name.clone(), t.params.clone());
            }
        }
    });
    INST_TODO.with(|q| q.borrow_mut().clear());

    //nt: 类型标注预扫
    {
        let mut 线: Vec<(usize, &TokStream)> = Vec::new();
        for b in &ast.blocks {
            线.clear();
            collect_ast_lines_ln(&b.stmts, &mut 线);
            for (_l, toks) in &线 {
                for (k, t) in toks.iter().enumerate() {
                    if let Token::Ident(n) = t {
                        if 泛型判别(n) && matches!(toks.get(k + 1), Some(Token::Sym("("))) {
                            let _ = parse_type_ref(toks, k);
                        }
                    }
                }
            }
        }
    }
    let 预 = INST_TODO.with(|q| std::mem::take(&mut *q.borrow_mut()));
    展开实例(&mut types, 预);
    //nt: 实例进表后重建索引
    let type_idx: std::rc::Rc<std::collections::HashMap<String, usize>> = std::rc::Rc::new(
        types.iter().enumerate().map(|(i, t)| (t.name.clone(), i)).collect(),
    );

    let mut functions = Vec::new();
    let mut externs: Vec<ExternFn> = Vec::new();
    for b in &ast.blocks {
        let (mut main, extra, ex) =
            build_block(b, &types, &type_idx, &global_fns, &cross_vals, &sum_variants, &mod_by_tail, &pub_blocks, &hidden_items, &block_types)?;
        //nt: 块函数 IR 名用
        //nt: 与 call 块前缀
        main.name = format!("{}|{}", b.module, b.name);
        functions.push(main);
        functions.extend(extra);
        externs.extend(ex);
    }
    //nt: 泛型实例展开
    let 待实例 = INST_TODO.with(|q| std::mem::take(&mut *q.borrow_mut()));
    展开实例(&mut types, 待实例);

    //nt: 逃逸调用值复印
    let mut 名集: BTreeSet<String> = BTreeSet::new();
    for f in functions.iter() {
        名集.insert(f.name.clone());
        if let Some(短) = f.name.rsplit('|').next() {
            名集.insert(短.to_string());
        }
    }
    let 区域 = 区型表(&types);
    for f in functions.iter_mut() {
        //nt: 回边圈范围
        let mut 圈: Vec<(usize, usize)> = Vec::new();
        for b in f.blocks.iter() {
            if let Exit::Branch { targets, .. } = &b.exit {
                for t in targets.iter() {
                    if *t < b.id {
                        圈.push((*t, b.id));
                    }
                }
            }
        }
        let mut 复印表: Vec<Vec<Option<(String, i64)>>> = Vec::new();
        let mut 护标: BTreeSet<LocalId> = BTreeSet::new();
        for b in f.blocks.iter() {
            let mut 表 = vec![None; b.stmts.len()];
            //nt: 最内层圈
            let mut 本圈: Option<(usize, usize)> = None;
            for (h, c) in 圈.iter() {
                if *h <= b.id && b.id <= *c {
                    let 更内 = match 本圈 {
                        None => true,
                        Some((hh, cc)) => *h > hh || (*h == hh && *c < cc),
                    };
                    if 更内 {
                        本圈 = Some((*h, *c));
                    }
                }
            }
            if let Some((h, c)) = 本圈 {
                for i in 0..b.stmts.len() {
                    //nt: 赋值两形态都算
                    let (tmp, 调名, 是调用) = match &b.stmts[i] {
                        Stmt::Assign { lhs, rhs: Rvalue::Call { name, .. } } => {
                            (lhs.base, name.as_str(), true)
                        }
                        Stmt::Assign { lhs, .. } => (lhs.base, "", false),
                        _ => continue,
                    };
                    let 是临时 = match f.locals.get(tmp) {
                        Some(l) => l.name.is_none(),
                        None => true,
                    };
                    let (dst, 位) = if !是调用 {
                        (tmp, i)
                    } else if 是临时 {
                        match b.stmts.get(i + 1) {
                            Some(Stmt::Assign { lhs, rhs }) => {
                                let src = match rhs {
                                    Rvalue::Const(Value::Local(u)) => *u,
                                    Rvalue::Use(p) => p.base,
                                    _ => continue,
                                };
                                if src != tmp {
                                    continue;
                                }
                                (lhs.base, i + 1)
                            }
                            _ => continue,
                        }
                    } else {
                        (tmp, i)
                    };
                    let Some(l) = f.locals.get(dst) else { continue };
                    if l.loop_local {
                        continue;
                    }
                    if 是调用 && !名集.contains(调名) {
                        continue;
                    }
                    if !跨圈(f, dst, h, c) {
                        continue;
                    }
                    match 复印方式(&l.ty.0, &区域, &types) {
                        复印::无需 => {}
                        复印::拷贝(名, 旗) => {
                            表[位] = Some((名, 旗));
                            护标.insert(dst);
                        }
                        复印::无法 => {
                            护标.insert(dst);
                        }
                    }
                }
            }
            复印表.push(表);
        }
        for id in 护标 {
            if let Some(l) = f.locals.get_mut(id) {
                l.loop_escape = true;
            }
        }
        for (bi, b) in f.blocks.iter_mut().enumerate() {
            let mut 新: Vec<Stmt> = Vec::with_capacity(b.stmts.len());
            let mut 新宽: Vec<Span> = Vec::with_capacity(b.spans.len());
            for (i, s) in b.stmts.iter().enumerate() {
                let sp = b.spans.get(i).copied().unwrap_or_default();
                新.push(s.clone());
                新宽.push(sp);
                if let Some((名, 旗)) = &复印表[bi][i] {
                    let lhs = match s {
                        Stmt::Assign { lhs, .. } => lhs.clone(),
                        _ => unreachable!(),
                    };
                    let mut args = vec![Value::Local(lhs.base)];
                    if 名 != "#str_copy" {
                        args.push(Value::Int(*旗));
                    }
                    新.push(Stmt::Assign { lhs, rhs: Rvalue::Call { name: 名.clone(), args } });
                    新宽.push(sp);
                }
            }
            b.stmts = 新;
            b.spans = 新宽;
        }
    }
    Ok(Program { functions, types, externs })
}

//nt: 把待实例清单落成具体
fn 展开实例(types: &mut Vec<StructType>, 待实例: Vec<(String, Vec<String>)>) {
    for (名, 实参) in 待实例 {
        let 实例名 = format!("{}({})", 名, 实参.join(", "));
        if types.iter().any(|t| t.name == 实例名) {
            continue;
        }
        let 模板 = match types.iter().find(|t| t.name == 名) {
            Some(t) => t.clone(),
            None => continue,
        };
        let 字段 = 模板
            .fields
            .iter()
            .map(|(n, ty)| (n.clone(), Ty(替换参数(&ty.0, &模板.params, &实参))))
            .collect();
        let 变体 = 模板
            .variants
            .iter()
            .map(|v| Variant {
                name: v.name.clone(),
                fields: v
                    .fields
                    .iter()
                    .map(|(n, ty)| (n.clone(), Ty(替换参数(&ty.0, &模板.params, &实参))))
                    .collect(),
            })
            .collect();
        types.push(StructType { name: 实例名, params: Vec::new(), fields: 字段, variants: 变体 });
    }
}