//! 表达式体函数内联
//!
//! 候选 = 单块且出口 Return(Some)、形参全标量、体只含 Assign/Call 的函数。
//! 体只含 Assign/Call 的函数。调用点就地展开：全部局部重编新号，形参先以实参赋值，
//! 出口值替换原调用结果。单遍不级联——克隆体内的新调用不再展开。

use anle_ir::{Exit, Function, Local, Place, Program, Rvalue, Stmt, Value};

//nt: 标量形参可内联
fn 标量(t: &str) -> bool {
    matches!(t, "int" | "float" | "bool")
}

//nt: 内联候选
fn 候选(f: &Function) -> bool {
    if f.blocks.len() != 1 || !matches!(f.blocks[0].exit, Exit::Return(Some(_))) {
        return false;
    }
    if !f.blocks[0].params.is_empty() {
        return false;
    }
    let ps: Vec<&Local> = f.locals.iter().filter(|l| l.is_param).collect();
    if ps.is_empty() || !ps.iter().all(|l| 标量(&l.ty.0)) {
        return false;
    }
    f.blocks[0]
        .stmts
        .iter()
        .all(|st| matches!(st, Stmt::Assign { .. } | Stmt::Call { .. }))
}

//nt: 展开一个调用点
fn 展开(
    callee: &Function,
    lhs: &Place,
    args: &[Value],
    起: usize,
) -> (Vec<Stmt>, Vec<Local>) {
    let n = callee.locals.len();
    let remap: Vec<usize> = (0..n).map(|k| 起 + k).collect();
    let mut 新locals: Vec<Local> = Vec::new();
    for l in &callee.locals {
        新locals.push(Local {
            name: None,
            ty: l.ty.clone(),
            is_param: false,
            bare_ok: false,
            line: 0,
            col: 0,
            loop_local: false,
            fn_owned: false,
            obj: None,
        });
    }
    let mv = |v: &Value| -> Value {
        match v {
            Value::Local(id) if *id < n => Value::Local(remap[*id]),
            other => other.clone(),
        }
    };
    let mp = |p: &Place| -> Place {
        Place { base: if p.base < n { remap[p.base] } else { p.base }, proj: p.proj.clone() }
    };
    let mut out: Vec<Stmt> = Vec::new();
    //nt: 实参喂形参
    let 形参: Vec<usize> = callee.locals.iter().enumerate().filter(|(_, l)| l.is_param).map(|(i, _)| i).collect();
    for (k, pid) in 形参.iter().enumerate() {
        let Some(a) = args.get(k) else { break };
        let rv = match a {
            Value::Local(u) => Rvalue::Use(Place { base: *u, proj: vec![] }),
            other => Rvalue::Const(other.clone()),
        };
        out.push(Stmt::Assign { lhs: Place::local(remap[*pid]), rhs: rv });
    }
    //nt: 体克隆
    for st in &callee.blocks[0].stmts {
        if let Stmt::Assign { lhs, rhs } = st {
            let rhs2 = match rhs {
                Rvalue::Use(p) => Rvalue::Use(mp(p)),
                Rvalue::Const(v) => Rvalue::Const(mv(v)),
                Rvalue::BinOp { op, lhs: a, rhs: b } => Rvalue::BinOp { op: op.clone(), lhs: mv(a), rhs: mv(b) },
                Rvalue::Call { name, args: a2 } => Rvalue::Call { name: name.clone(), args: a2.iter().map(mv).collect() },
                Rvalue::ListLit { elems } => Rvalue::ListLit { elems: elems.iter().map(mv).collect() },
                Rvalue::ListGet { list, index } => Rvalue::ListGet { list: mv(list), index: mv(index) },
                Rvalue::MapLit { pairs } => Rvalue::MapLit { pairs: pairs.iter().map(|(k2, v)| (k2.clone(), mv(v))).collect() },
                Rvalue::MapGet { map, key } => Rvalue::MapGet { map: mv(map), key: mv(key) },
                Rvalue::VariantTag(v) => Rvalue::VariantTag(mv(v)),
                Rvalue::Ref(p) => Rvalue::Ref(mp(p)),
                other => other.clone(),
            };
            out.push(Stmt::Assign { lhs: mp(lhs), rhs: rhs2 });
        }
    }
    //nt: 出口值替换调用结果
    if let Exit::Return(Some(v)) = &callee.blocks[0].exit {
        let rv = match mv(v) {
            Value::Local(id) => Rvalue::Use(Place::local(id)),
            other => Rvalue::Const(other),
        };
        out.push(Stmt::Assign { lhs: lhs.clone(), rhs: rv });
    }
    (out, 新locals)
}

//nt: 跑一遍
pub fn run(prog: &mut Program) {
    //nt: 快照候选
    let cands: Vec<(String, Function)> = prog
        .functions
        .iter()
        .filter(|f| 候选(f))
        .map(|f| (f.name.clone(), f.clone()))
        .collect();
    if cands.is_empty() {
        return;
    }
    for fi in 0..prog.functions.len() {
        //nt: 本函数要展开的调用点
        let mut 替换: Vec<(usize, usize, usize)> = Vec::new();
        for (bi, b) in prog.functions[fi].blocks.iter().enumerate() {
            for (si, st) in b.stmts.iter().enumerate() {
                if let Stmt::Assign { rhs: Rvalue::Call { name, args }, .. } = st {
                    if let Some(ci) = cands.iter().position(|(k, _)| k == name) {
                        let 形参数 = cands[ci].1.locals.iter().filter(|l| l.is_param).count();
                        if args.len() == 形参数 {
                            替换.push((bi, si, ci));
                        }
                    }
                }
            }
        }
        if 替换.is_empty() {
            continue;
        }
        let mut 新locals: Vec<Local> = Vec::new();
        let mut 新stmts: Vec<Vec<Stmt>> = vec![Vec::new(); prog.functions[fi].blocks.len()];
        for (bi, b) in prog.functions[fi].blocks.iter().enumerate() {
            for (si, st) in b.stmts.iter().enumerate() {
                match 替换.iter().find(|(b2, s2, _)| *b2 == bi && *s2 == si) {
                    Some((_, _, ci)) => {
                        if let Stmt::Assign { lhs, rhs: Rvalue::Call { args, .. } } = st {
                            let 起 = prog.functions[fi].locals.len() + 新locals.len();
                            let (seg, 局) = 展开(&cands[*ci].1, lhs, args, 起);
                            新locals.extend(局);
                            新stmts[bi].extend(seg);
                        }
                    }
                    None => 新stmts[bi].push(st.clone()),
                }
            }
        }
        let f = &mut prog.functions[fi];
        f.locals.extend(新locals);
        for (bi, b) in f.blocks.iter_mut().enumerate() {
            b.stmts = std::mem::take(&mut 新stmts[bi]);
        }
    }
}
