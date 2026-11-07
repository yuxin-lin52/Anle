//! 形参观感分析：对每个函数形参判定是否被写
//!
//! 产出「函数名 + 形参序号 → 是否被写」的映射。被写的形态：赋值目标、写字段或下标、
//! 改写容器的内置名、交给外部函数、经函数指针调用、进容器、返回、取地址、内联汇编。

use crate::{Exit, Function, LocalId, Place, Program, Rvalue, Stmt, Value};
use std::collections::BTreeMap;
use std::collections::BTreeSet;


//nt: 改写容器的内置名
fn 改写内置(name: &str) -> bool {
    matches!(
        name,
        "push" | "pop" | "insert" | "remove" | "sort" | "put" | "clear" | "set"
    )
}

//nt: 形参观感
pub fn 形参观感(p: &Program) -> BTreeMap<(String, usize), bool> {
    let mut 表: BTreeMap<(String, usize), bool> = BTreeMap::new();
    let 外部: BTreeSet<String> = p.externs.iter().map(|e| e.name.clone()).collect();
    let 函数: BTreeSet<String> = p.functions.iter().map(|f| f.name.clone()).collect();
    for f in &p.functions {
        for (k, _) in 参序(f).iter().enumerate() {
            表.insert((f.name.clone(), k), false);
        }
    }
    //nt: 不动点
    for _ in 0..16 {
        let mut 变 = false;
        for f in &p.functions {
            let 参 = 参序(f);
            for (k, lid) in 参.iter().enumerate() {
                if 表.get(&(f.name.clone(), k)).copied().unwrap_or(false) {
                    continue;
                }
                //nt: 值活集
                let 值活 = 值活集(f, *lid);
                let mut 写 = false;
                for b in &f.blocks {
                    for s in &b.stmts {
                        if 语句写形参(s, *lid, &值活, &表, &外部, &函数) {
                            写 = true;
                        }
                    }
                    if 出口写形参(&b.exit, &值活) {
                        写 = true;
                    }
                }
                if 写 {
                    表.insert((f.name.clone(), k), true);
                    变 = true;
                }
            }
        }
        if !变 {
            break;
        }
    }
    表
}

//nt: 形参局部
fn 参序(f: &Function) -> Vec<LocalId> {
    f.locals
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_param)
        .map(|(i, _)| i)
        .collect()
}

//nt: 值里含值活集任一
fn 值含(v: &Value, 活: &std::collections::BTreeSet<LocalId>) -> bool {
    matches!(v, Value::Local(x) if 活.contains(x))
}

//nt: 位置指向该局部
fn 位指(p: &Place, lid: LocalId) -> bool {
    p.base == lid
}

//nt: 值活集
fn 值活集(f: &Function, lid: LocalId) -> std::collections::BTreeSet<LocalId> {
    let mut 活: std::collections::BTreeSet<LocalId> = std::collections::BTreeSet::new();
    活.insert(lid);
    loop {
        let before = 活.len();
        for b in &f.blocks {
            for s in &b.stmts {
                if let Stmt::Assign { lhs, rhs } = s {
                    if !lhs.proj.is_empty() {
                        continue;
                    }
                    let src = match rhs {
                        Rvalue::Const(Value::Local(u)) => Some(*u),
                        Rvalue::Use(Place { base: u, .. }) => Some(*u),
                        _ => None,
                    };
                    if let Some(u) = src {
                        if 活.contains(&u) && !活.contains(&lhs.base) {
                            活.insert(lhs.base);
                        }
                    }
                }
            }
        }
        if 活.len() == before {
            break;
        }
    }
    活
}

//nt: 语句是否写参数值活集
fn 语句写形参(
    s: &Stmt,
    lid: LocalId,
    活: &std::collections::BTreeSet<LocalId>,
    表: &BTreeMap<(String, usize), bool>,
    外部: &BTreeSet<String>,
    函数: &BTreeSet<String>,
) -> bool {
    match s {
        //nt: 值活成员重绑不算写
        Stmt::Assign { lhs, rhs } => {
            if lhs.base == lid && lhs.proj.is_empty() {
                return true;
            }
            if !lhs.proj.is_empty() && 活.contains(&lhs.base) {
                return true;
            }
            match rhs {
                Rvalue::Ref(p) => 活.contains(&p.base),
                Rvalue::Call { name, args } => {
                    let k = args.iter().position(|a| 值含(a, 活));
                    match k {
                        None => false,
                        Some(k) => {
                            if 外部.contains(name) {
                                return true; //nt: 无体
                            }
                            if !函数.contains(name) {
                                //nt: 改写容器的一批内置名
                                if 改写内置(name) && k == 0 {
                                    return true;
                                }
                                return false; //nt: 其余内置名不夺所有权
                            }
                            表.get(&(name.clone(), k)).copied().unwrap_or(true)
                        }
                    }
                }
                Rvalue::CallIndirect { args, .. } => args.iter().any(|a| 值含(a, 活)),
                //nt: 进容器 / 构造袋 ⇒ 逃逸
                Rvalue::ListLit { elems } => elems.iter().any(|e| 值含(e, 活)),
                Rvalue::VariantLit { args, .. } => args.iter().any(|a| 值含(a, 活)),
                Rvalue::MapLit { pairs } => pairs.iter().any(|(_, v)| 值含(v, 活)),
                _ => false,
            }
        }
        Stmt::Call { name, args } => {
            let Some(k) = args.iter().position(|a| 值含(a, 活)) else {
                return false;
            };
            if 外部.contains(name) {
                return true;
            }
            if !函数.contains(name) {
                if 改写内置(name) && k == 0 {
                    return true;
                }
                return false;
            }
            表.get(&(name.clone(), k)).copied().unwrap_or(true)
        }
        //nt: 写进容器即改容器
        Stmt::ListSet { list, index, value } => {
            值含(list, 活) || 值含(index, 活) || 值含(value, 活)
        }
        Stmt::MapSet { map, key, value } => {
            值含(map, 活) || 值含(key, 活) || 值含(value, 活)
        }
        //nt: 汇编不透明
        Stmt::Asm(_) => true,
        _ => false,
    }
}

//nt: 出口是否携参数值
fn 出口写形参(e: &Exit, 活: &std::collections::BTreeSet<LocalId>) -> bool {
    match e {
        Exit::Return(Some(v)) => 值含(v, 活),
        Exit::Branch { targets, .. } => targets
            .iter()
            .any(|t| t.args.iter().any(|a| 值含(a, 活))),
        _ => false,
    }
}

