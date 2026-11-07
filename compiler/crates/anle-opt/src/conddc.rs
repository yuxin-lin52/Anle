//! 条件 dc：去掉只读形参实参的深拷
//!
//! 找到深拷临时与消费它的调用语句；被调形参不被写时，把实参改回源值、删除深拷语句。

use anle_ir::{Exit, Place, Program, Rvalue, Stmt, Value};
use std::collections::BTreeSet;

//nt: 值里含该局部
fn 值含(v: &Value, lid: anle_ir::LocalId) -> bool {
    matches!(v, Value::Local(x) if *x == lid)
}

//nt: 位置指向该局部
fn 位指(p: &Place, lid: anle_ir::LocalId) -> bool {
    p.base == lid
}


//nt: 语句是否引用该局部
fn 语句含局部(s: &Stmt, t: anle_ir::LocalId) -> bool {
    match s {
        Stmt::Assign { lhs, rhs } => 位指(lhs, t) || 右值含局部(rhs, t),
        Stmt::Call { args, .. } => args.iter().any(|a| 值含(a, t)),
        Stmt::ListSet { list, index, value } => 值含(list, t) || 值含(index, t) || 值含(value, t),
        Stmt::MapSet { map, key, value } => 值含(map, t) || 值含(key, t) || 值含(value, t),
        Stmt::Assert { cond, .. } => 值含(cond, t),
        Stmt::AccessBegin(p) | Stmt::AccessEnd(p) => 位指(p, t),
        _ => false,
    }
}

//nt: 右值是否引用该局部
fn 右值含局部(r: &Rvalue, t: anle_ir::LocalId) -> bool {
    match r {
        Rvalue::Use(p) | Rvalue::Ref(p) => 位指(p, t),
        Rvalue::Const(v) => 值含(v, t),
        Rvalue::BinOp { lhs, rhs, .. } => 值含(lhs, t) || 值含(rhs, t),
        Rvalue::Call { args, .. } | Rvalue::CallIndirect { args, .. } => args.iter().any(|a| 值含(a, t)),
        Rvalue::ListLit { elems } => elems.iter().any(|e| 值含(e, t)),
        Rvalue::ListGet { list, index } => 值含(list, t) || 值含(index, t),
        Rvalue::MapLit { pairs } => pairs.iter().any(|(_, v)| 值含(v, t)),
        Rvalue::MapGet { map, key } => 值含(map, t) || 值含(key, t),
        Rvalue::VariantLit { args, .. } => args.iter().any(|a| 值含(a, t)),
        Rvalue::VariantTag(v) => 值含(v, t),
        Rvalue::VariantGet { value, .. } => 值含(value, t),
        Rvalue::FnRef { .. } => false,
    }
}

//nt: 出口是否携该局部
fn 出口含局部(e: &Exit, t: anle_ir::LocalId) -> bool {
    match e {
        Exit::Return(Some(v)) => 值含(v, t),
        Exit::Branch { targets, .. } => targets.iter().any(|x| x.args.iter().any(|a| 值含(a, t))),
        _ => false,
    }
}

//nt: 临时另有使用
fn 另有使用(f: &anle_ir::Function, t: anle_ir::LocalId, 跳块: usize, 跳句: usize) -> bool {
    for (bi, b) in f.blocks.iter().enumerate() {
        for (i, s) in b.stmts.iter().enumerate() {
            if bi == 跳块 && i == 跳句 {
                continue;
            }
            if 语句含局部(s, t) {
                return true;
            }
        }
        if 出口含局部(&b.exit, t) {
            return true;
        }
    }
    false
}

//nt: 条件 dc 消除
pub fn run(p: &mut Program) {
    let 观感 = anle_ir::形参观感(p);
    if 观感.is_empty() {
        return;
    }
    //nt: 被调返回堆型表
    let mut 返堆: BTreeSet<String> = BTreeSet::new();
    for f in &p.functions {
        if let Some(rt) = &f.ret {
            let rt = &rt.0;
            if rt.ends_with('*') || rt.contains('(') || rt == "str" {
                返堆.insert(f.name.clone());
            }
        }
    }
    for f in p.functions.iter_mut() {
        //nt: 名字表在改动期间不变
        let 局部观感 = 观感.clone();
        for bi in 0..f.blocks.len() {
            let mut 删: BTreeSet<usize> = BTreeSet::new();
            let 长 = f.blocks[bi].stmts.len();
            for i in 0..长 {
                //nt: 只认dc赋值
                let (t, 源) = match &f.blocks[bi].stmts[i] {
                    Stmt::Assign { lhs, rhs } if lhs.proj.is_empty() => match rhs {
                        Rvalue::Call { name, args } if name.starts_with("#dc") && args.len() == 2 => {
                            (lhs.base, args[0].clone())
                        }
                        _ => continue,
                    },
                    _ => continue,
                };
                //nt: 找消费点
                let mut 消费 = None;
                for j in (i + 1)..长 {
                    match &f.blocks[bi].stmts[j] {
                        Stmt::Assign { rhs: Rvalue::Call { name, args }, .. } => {
                            if let Some(k) = args.iter().position(|a| 值含(a, t)) {
                                消费 = Some((name.clone(), k));
                            }
                            break;
                        }
                        Stmt::Call { name, args } => {
                            if let Some(k) = args.iter().position(|a| 值含(a, t)) {
                                消费 = Some((name.clone(), k));
                            }
                            break;
                        }
                        s if 语句含局部(s, t) => break,
                        _ => {}
                    }
                }
                let Some((被调, k)) = 消费 else { continue };
                if 局部观感.get(&(被调.clone(), k)).copied() != Some(false) {
                    continue; //nt: 被写或未知 ⇒ 保留 dc
                }
                //nt: 返回堆型保留 dc
                if 返堆.contains(&被调) {
                    continue;
                }
                //nt: 改直传源
                let mut 改好 = false;
                for j in (i + 1)..长 {
                    if let Stmt::Assign { rhs: Rvalue::Call { name, args }, .. } = &mut f.blocks[bi].stmts[j] {
                        if *name == 被调 {
                            if let Some(p) = args.iter().position(|a| 值含(a, t)) {
                                args[p] = 源.clone();
                                改好 = true;
                            }
                            break;
                        }
                    }
                    if let Stmt::Call { name, args } = &mut f.blocks[bi].stmts[j] {
                        if *name == 被调 {
                            if let Some(p) = args.iter().position(|a| 值含(a, t)) {
                                args[p] = 源.clone();
                                改好 = true;
                            }
                            break;
                        }
                    }
                }
                if 改好 && !另有使用(f, t, bi, i) {
                    删.insert(i);
                }
            }
            for i in 删.iter().rev() {
                f.blocks[bi].stmts.remove(*i);
            }
        }
    }
}
