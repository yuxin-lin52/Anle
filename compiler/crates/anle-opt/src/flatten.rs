//! 聚合平铺判定
//!
//! 按 fn 扫 `list(积类型)` 局部：其 ListGet 结果临时若只被
//! 「字段读 / 字段写」触碰即判平铺；比较 / 传 fn / 存第二处 /
//! push 实参 / 下标写值则保持把手布局。
//! 结果进 `Program.平铺`，两后端按其改布局。

use anle_ir::{LocalId, Program, Rvalue, Stmt, Value};

//nt: 序列元素类型
fn 序元素型(t: &str) -> Option<String> {
    let (head, inner) = t.split_once('(')?;
    let tail = inner.strip_suffix(')')?;
    match head {
        "list" | "que" | "stack" => Some(tail.trim().to_string()),
        _ => None,
    }
}
use std::collections::{BTreeMap, BTreeSet};

pub fn run(prog: &mut Program) {
    let mut 出: BTreeMap<String, BTreeSet<LocalId>> = BTreeMap::new();
    for f in &prog.functions {
        //nt: 候选 = 积元素列表局部
        let mut 候选: BTreeSet<LocalId> = BTreeSet::new();
        for (i, l) in f.locals.iter().enumerate() {
            let et = 序元素型(&l.ty.0);
            let Some(et) = et else { continue };
            let 是积 = prog
                .types
                .iter()
                .any(|t| t.name == et && t.variants.is_empty());
            if 是积 {
                候选.insert(i);
            }
        }
        if 候选.is_empty() {
            continue;
        }
        //nt: 元素临时 → 来源列表
        let mut 元源: BTreeMap<LocalId, LocalId> = BTreeMap::new();
        for b in &f.blocks {
            for st in &b.stmts {
                if let Stmt::Assign { lhs, rhs } = st {
                    if let Rvalue::ListGet { list: Value::Local(l), .. } = rhs {
                        if 候选.contains(l) {
                            元源.insert(lhs.base, *l);
                        }
                    }
                }
            }
        }
        //nt: 违规元素临时集
        let mut 犯: BTreeSet<LocalId> = BTreeSet::new();
        let mut 摘值 = |v: &Value, 元源: &BTreeMap<LocalId, LocalId>, 犯: &mut BTreeSet<LocalId>| {
            if let Value::Local(l) = v {
                if 元源.contains_key(l) {
                    犯.insert(*l);
                }
            }
        };
        for b in &f.blocks {
            for st in &b.stmts {
                match st {
                    Stmt::Assign { lhs, rhs } => {
                        //nt: 元素用法逐类看
                        match rhs {
                            Rvalue::BinOp { lhs: a, rhs: c, .. } => {
                                摘值(a, &元源, &mut 犯);
                                摘值(c, &元源, &mut 犯);
                            }
                            Rvalue::Call { args, .. } | Rvalue::CallIndirect { args, .. } => {
                                for a in args {
                                    摘值(a, &元源, &mut 犯);
                                }
                            }
                            Rvalue::Use(p) => {
                                //nt: 存第二处
                                if 元源.contains_key(&p.base) && lhs.base != p.base {
                                    犯.insert(p.base);
                                }
                            }
                            Rvalue::ListLit { elems } => {
                                for e in elems {
                                    摘值(e, &元源, &mut 犯);
                                }
                            }
                            _ => {}
                        }
                    }
                    Stmt::Call { name, args } => {
                        //nt: 元素实参逐个看
                        for a in args {
                            摘值(a, &元源, &mut 犯);
                        }
                        let _ = name;
                    }
                    Stmt::ListSet { value, .. } => {
                        摘值(value, &元源, &mut 犯);
                    }
                    _ => {}
                }
                //nt: 出口实参
                match &b.exit {
                    anle_ir::Exit::Return(Some(v)) => 摘值(v, &元源, &mut 犯),
                    anle_ir::Exit::Branch { targets, .. } => {
                        for s in targets {
                            for a in &s.args {
                                摘值(a, &元源, &mut 犯);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        //nt: 无犯者平铺
        let 好: BTreeSet<LocalId> = 候选.difference(&犯).copied().collect();
        if !好.is_empty() {
            出.insert(f.name.clone(), 好);
        }
    }
    prog.平铺 = 出;
}
