//! 循环不变量外提
//!
//! 自然循环内、操作数都在循环外定义的纯表达式，提到循环前置块。
//! 只处理无副作用的右值；抛错与写内存的一律不动。

use anle_ir::{BinOp, BlockId, Exit, LocalId, Program, Rvalue, Stmt, Value};
use std::collections::{BTreeMap, BTreeSet};

//nt: 后继块
fn 后继(f: &anle_ir::Function, b: usize) -> Vec<BlockId> {
    match &f.blocks[b].exit {
        Exit::Branch { targets, .. } => targets.iter().map(|s| s.target).collect(),
        _ => Vec::new(),
    }
}

//nt: 前驱表
fn 前驱表(f: &anle_ir::Function) -> Vec<Vec<BlockId>> {
    let mut ps: Vec<Vec<BlockId>> = vec![Vec::new(); f.blocks.len()];
    for i in 0..f.blocks.len() {
        for t in 后继(f, i) {
            if (t as usize) < ps.len() {
                ps[t as usize].push(i as BlockId);
            }
        }
    }
    ps
}

//nt: 链深
fn 链深(idom: &[Option<BlockId>], mut b: BlockId) -> usize {
    let mut d = 0;
    while let Some(p) = idom[b as usize] {
        d += 1;
        b = p;
        if d > 100000 {
            break;
        }
    }
    d
}

//nt: 支配链求交
fn 求交(idom: &[Option<BlockId>], mut a: BlockId, mut b: BlockId) -> BlockId {
    let mut 圈 = 0;
    while a != b {
        let da = 链深(idom, a);
        let db = 链深(idom, b);
        if da >= db {
            match idom[a as usize] {
                Some(p) => a = p,
                None => return a,
            }
        } else {
            match idom[b as usize] {
                Some(p) => b = p,
                None => return b,
            }
        }
        圈 += 1;
        if 圈 > 100000 {
            break;
        }
    }
    a
}

//nt: 立即支配者
fn 支配树(f: &anle_ir::Function) -> Vec<Option<BlockId>> {
    let n = f.blocks.len();
    let mut idom: Vec<Option<BlockId>> = vec![None; n];
    if n == 0 {
        return idom;
    }
    let ps = 前驱表(f);
    let mut 变 = true;
    let mut 轮 = 0;
    while 变 && 轮 < 500 {
        变 = false;
        轮 += 1;
        for b in 1..n {
            let mut 新: Option<BlockId> = None;
            for &p in &ps[b] {
                //nt: 入口视作已入场
                if idom[p as usize].is_none() && p != 0 {
                    continue;
                }
                新 = match 新 {
                    None => Some(p),
                    Some(x) => Some(求交(&idom, x, p)),
                };
            }
            if 新 != idom[b] {
                idom[b] = 新;
                变 = true;
            }
        }
    }
    idom
}

//nt: 是否支配
fn 支配(idom: &[Option<BlockId>], a: BlockId, b: BlockId) -> bool {
    if a == b {
        return true;
    }
    let mut 走 = b;
    let mut 圈 = 0;
    while let Some(p) = idom[走 as usize] {
        if p == a {
            return true;
        }
        走 = p;
        圈 += 1;
        if 圈 > 100000 {
            break;
        }
    }
    false
}

//nt: 右值里的局部
fn 操作数(r: &Rvalue, out: &mut Vec<LocalId>) {
    match r {
        Rvalue::Use(p) => out.push(p.base),
        Rvalue::Const(Value::Local(id)) => out.push(*id),
        Rvalue::BinOp { lhs, rhs, .. } => {
            if let Value::Local(id) = lhs {
                out.push(*id);
            }
            if let Value::Local(id) = rhs {
                out.push(*id);
            }
        }
        Rvalue::Call { args, .. } => {
            for a in args {
                if let Value::Local(id) = a {
                    out.push(*id);
                }
            }
        }
        _ => {}
    }
}

//nt: 无副作用判定
fn 纯(r: &Rvalue) -> bool {
    match r {
        Rvalue::Const(_) => true,
        Rvalue::Use(p) => p.proj.is_empty(),
        Rvalue::BinOp { op, .. } => !matches!(op, BinOp::Div | BinOp::Mod),
        //nt: 只认不改不抛的内置
        Rvalue::Call { name, .. } => name == "len" || name == "len_utf8",
        _ => false,
    }
}

//nt: 单函数外提
fn 外提(f: &mut anle_ir::Function) {
    if f.blocks.len() < 2 {
        return;
    }
    let idom = 支配树(f);
    let ps = 前驱表(f);
    //nt: 自然循环
    let mut 圈集: BTreeMap<BlockId, BTreeSet<BlockId>> = BTreeMap::new();
    for u in 0..f.blocks.len() {
        for v in 后继(f, u) {
            if !支配(&idom, v, u as BlockId) {
                continue;
            }
            let 集 = 圈集.entry(v).or_default();
            集.insert(v);
            let mut 栈 = vec![u as BlockId];
            while let Some(x) = 栈.pop() {
                if 集.insert(x) {
                    for &p in &ps[x as usize] {
                        栈.push(p);
                    }
                }
            }
        }
    }
    for (头, 节点) in 圈集 {
        if 节点.len() < 2 {
            continue;
        }
        //nt: 前置块要唯一
        let mut 外: Vec<BlockId> = Vec::new();
        for &p in &ps[头 as usize] {
            if !节点.contains(&p) {
                外.push(p);
            }
        }
        if 外.len() != 1 {
            continue;
        }
        let 前 = 外[0];
        //nt: 有释放或重绑就不动
        let mut 有生命周期 = false;
        for &b in &节点 {
            for s in &f.blocks[b as usize].stmts {
                if matches!(s, Stmt::Drop { .. } | Stmt::Bind { .. }) {
                    有生命周期 = true;
                }
            }
        }
        if 有生命周期 {
            continue;
        }
        //nt: 定义块表
        let mut 定义: BTreeMap<LocalId, BlockId> = BTreeMap::new();
        for (bi, b) in f.blocks.iter().enumerate() {
            //nt: 块参数算本块定义
            for (pid, _) in &b.params {
                定义.insert(*pid, bi as BlockId);
            }
            for s in &b.stmts {
                if let Stmt::Assign { lhs, .. } = s {
                    //nt: 投影写也算本块定义
                    定义.insert(lhs.base, bi as BlockId);
                }
            }
        }
        //nt: 改长度的调用禁外提
        let mut 改长 = false;
        for &b in &节点 {
            for s in &f.blocks[b as usize].stmts {
                if let Stmt::Call { name, .. } = s {
                    if matches!(name.as_str(), "push" | "pop" | "insert" | "remove" | "clear") {
                        改长 = true;
                    }
                }
            }
        }
        if 改长 {
            continue;
        }
        let mut 再 = true;
        let mut 轮 = 0;
        while 再 && 轮 < 100 {
            再 = false;
            轮 += 1;
            let mut 搬: Vec<(BlockId, usize)> = Vec::new();
            for &b in &节点 {
                let 块 = b as usize;
                for (si, s) in f.blocks[块].stmts.iter().enumerate() {
                    let Stmt::Assign { lhs, rhs } = s else {
                        continue;
                    };
                    if !lhs.proj.is_empty() || !纯(rhs) {
                        continue;
                    }
                    let mut 参 = Vec::new();
                    操作数(rhs, &mut 参);
                    let 好 = 参.iter().all(|id| match 定义.get(id) {
                        Some(d) => !节点.contains(d),
                        None => true,
                    });
                    if 好 {
                        搬.push((b, si));
                    }
                }
            }
            let mut 移: Vec<Stmt> = Vec::new();
            for (b, si) in 搬.iter().rev() {
                移.push(f.blocks[*b as usize].stmts.remove(*si));
            }
            移.reverse();
            if 移.is_empty() {
                continue;
            }
            再 = true;
            for s in 移 {
                if let Stmt::Assign { lhs, .. } = &s {
                    if lhs.proj.is_empty() {
                        定义.insert(lhs.base, 前);
                    }
                }
                f.blocks[前 as usize].stmts.push(s);
            }
        }
    }
}

//nt: 全程序入口
pub fn run(p: &mut Program) {
    for f in &mut p.functions {
        外提(f);
    }
}
