//! SSA 构造：多定义局部版本化
//!
//! 做法：支配边界插块参数（φ），按支配树前序重命名，边实参在出口回填。
//! 只处理标量局部；参数、被取址者、聚合体一律跳过。

use anle_ir::{BlockId, Exit, Function, Local, Place, Program, Rvalue, Stmt, Value};
use std::collections::{BTreeMap, BTreeSet};

//nt: 后继块
fn 后继(f: &Function, b: usize) -> Vec<BlockId> {
    match &f.blocks[b].exit {
        Exit::Branch { targets, .. } => targets.iter().map(|s| s.target).collect(),
        _ => Vec::new(),
    }
}

//nt: 前驱表
fn 前驱表(f: &Function) -> Vec<Vec<BlockId>> {
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
fn 支配树(f: &Function) -> Vec<Option<BlockId>> {
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

//nt: 支配边界
fn 支配边界(f: &Function, idom: &[Option<BlockId>], ps: &[Vec<BlockId>]) -> Vec<BTreeSet<BlockId>> {
    let n = f.blocks.len();
    let mut df: Vec<BTreeSet<BlockId>> = vec![BTreeSet::new(); n];
    for b in 0..n {
        if ps[b].len() < 2 {
            continue;
        }
        let 停 = idom[b];
        for &p in &ps[b] {
            let mut 跑 = p;
            let mut 圈 = 0;
            while Some(跑) != 停 {
                //nt: 入口不插
                if 跑 != 0 {
                    df[跑 as usize].insert(b as BlockId);
                }
                match idom[跑 as usize] {
                    Some(上) => {
                        if 上 == 跑 {
                            break;
                        }
                        跑 = 上;
                    }
                    None => break,
                }
                圈 += 1;
                if 圈 > 100000 {
                    break;
                }
            }
        }
    }
    df
}

//nt: 标量判定
fn 是标量(t: &str) -> bool {
    matches!(t, "int" | "float" | "bool")
}

//nt: 新建局部
fn 新局部(f: &mut Function, 名: String, ty: anle_ir::Ty) -> anle_ir::LocalId {
    let id = f.locals.len() as anle_ir::LocalId;
    f.locals.push(Local {
        name: Some(名),
        ty,
        is_param: false,
        bare_ok: false,
        line: 0,
        col: 0,
        loop_local: false,
        fn_owned: false,
        obj: None,
    });
    id
}

//nt: 读值替换
fn 换值(v: &mut Value, 版: &BTreeMap<anle_ir::LocalId, anle_ir::LocalId>) {
    if let Value::Local(id) = v {
        if let Some(n) = 版.get(id) {
            *id = *n;
        }
    }
}

//nt: 位置基替换
fn 换位(p: &mut Place, 版: &BTreeMap<anle_ir::LocalId, anle_ir::LocalId>) {
    if let Some(n) = 版.get(&p.base) {
        p.base = *n;
    }
}

//nt: 右值里的读
fn 换右值(r: &mut Rvalue, 版: &BTreeMap<anle_ir::LocalId, anle_ir::LocalId>) {
    match r {
        Rvalue::Use(p) => 换位(p, 版),
        Rvalue::Ref(_) => {}
        Rvalue::Const(v) => 换值(v, 版),
        Rvalue::BinOp { lhs, rhs, .. } => {
            换值(lhs, 版);
            换值(rhs, 版);
        }
        Rvalue::Call { args, .. } | Rvalue::CallIndirect { args, .. } => {
            for a in args {
                换值(a, 版);
            }
        }
        Rvalue::ListLit { elems } => {
            for e in elems {
                换值(e, 版);
            }
        }
        Rvalue::ListGet { list, index } => {
            换值(list, 版);
            换值(index, 版);
        }
        Rvalue::MapLit { pairs } => {
            for (_, v) in pairs {
                换值(v, 版);
            }
        }
        Rvalue::MapGet { map, key } => {
            换值(map, 版);
            换值(key, 版);
        }
        Rvalue::VariantLit { args, .. } => {
            for a in args {
                换值(a, 版);
            }
        }
        Rvalue::VariantTag(v) => 换值(v, 版),
        Rvalue::VariantGet { value, .. } => 换值(value, 版),
        Rvalue::FnRef { .. } => {}
    }
}

//nt: 语句里的读
fn 换语句(s: &mut Stmt, 版: &BTreeMap<anle_ir::LocalId, anle_ir::LocalId>) {
    match s {
        Stmt::Assign { lhs, rhs } => {
            换右值(rhs, 版);
            换位(lhs, 版);
        }
        Stmt::Call { args, .. } => {
            for a in args {
                换值(a, 版);
            }
        }
        Stmt::ListSet { list, index, value } => {
            换值(list, 版);
            换值(index, 版);
            换值(value, 版);
        }
        Stmt::MapSet { map, key, value } => {
            换值(map, 版);
            换值(key, 版);
            换值(value, 版);
        }
        Stmt::AccessBegin(p) | Stmt::AccessEnd(p) => 换位(p, 版),
        Stmt::Assert { cond, .. } => 换值(cond, 版),
        Stmt::Asm(form) => {
            for (_, v, _) in &mut form.outs {
                if let Some(n) = 版.get(v) {
                    *v = *n;
                }
            }
            for (_, v) in &mut form.ins {
                换值(v, 版);
            }
        }
        Stmt::Alloc { .. } | Stmt::Bind { .. } | Stmt::Drop { .. } | Stmt::DropVar { .. } | Stmt::Kill { .. } => {}
    }
}

//nt: 可版本化的局部
fn 待版本化(f: &Function) -> BTreeSet<anle_ir::LocalId> {
    let n = f.locals.len();
    let mut 写数 = vec![0usize; n];
    let mut 取址: BTreeSet<anle_ir::LocalId> = BTreeSet::new();
    let mut 绑对象: BTreeSet<anle_ir::LocalId> = BTreeSet::new();
    for b in &f.blocks {
        for s in &b.stmts {
            if let Stmt::Assign { lhs, rhs } = s {
                if lhs.proj.is_empty() && (lhs.base as usize) < n {
                    写数[lhs.base as usize] += 1;
                }
                if let Rvalue::Ref(p) = rhs {
                    取址.insert(p.base);
                }
            }
            if let Stmt::Bind { var, .. } = s {
                绑对象.insert(*var);
            }
        }
    }
    let mut 出 = BTreeSet::new();
    for (i, l) in f.locals.iter().enumerate() {
        if 写数[i] < 2 {
            continue;
        }
        if l.is_param || !是标量(&l.ty.0) || 取址.contains(&(i as anle_ir::LocalId)) {
            continue;
        }
        if 绑对象.contains(&(i as anle_ir::LocalId)) {
            continue;
        }
        if l.name.as_deref().map(|x| x.contains('#')).unwrap_or(false) {
            continue;
        }
        出.insert(i as anle_ir::LocalId);
    }
    出
}

//nt: 给一条边补实参
fn 补实参(f: &mut Function, 从: BlockId, 到: BlockId, 值: Value) {
    if let Exit::Branch { targets, .. } = &mut f.blocks[从 as usize].exit {
        for s in targets.iter_mut() {
            if s.target == 到 {
                s.args.push(值.clone());
            }
        }
    }
}

//nt: 重命名一块
fn 重命名(
    f: &mut Function,
    块: BlockId,
    有φ: &[BTreeMap<anle_ir::LocalId, anle_ir::LocalId>],
    子: &[Vec<BlockId>],
    mut 版: BTreeMap<anle_ir::LocalId, anle_ir::LocalId>,
    改: &BTreeSet<anle_ir::LocalId>,
    计数: &mut usize,
) {
    for (&v, &phi) in &有φ[块 as usize] {
        版.insert(v, phi);
    }
    let 长 = f.blocks[块 as usize].stmts.len();
    for si in 0..长 {
        let mut s = std::mem::replace(
            &mut f.blocks[块 as usize].stmts[si],
            Stmt::Kill { block: String::new() },
        );
        //nt: 记原写目标
        let 原写 = match &s {
            Stmt::Assign { lhs, .. } if lhs.proj.is_empty() => Some(lhs.base),
            _ => None,
        };
        //nt: 先换读
        换语句(&mut s, &版);
        //nt: 再换写
        if let (Some(v), Stmt::Assign { lhs, .. }) = (原写, &mut s) {
            if 改.contains(&v) {
                let ty = f.locals[v as usize].ty.clone();
                let 名 = f.locals[v as usize].name.clone().unwrap_or_default();
                *计数 += 1;
                let 新 = 新局部(f, format!("{}$v{}", 名, 计数), ty);
                版.insert(v, 新);
                lhs.base = 新;
            }
        }
        f.blocks[块 as usize].stmts[si] = s;
    }
    //nt: 出口也要换版本
    match &mut f.blocks[块 as usize].exit {
        Exit::Return(Some(v)) => 换值(v, &版),
        Exit::Branch { cond, .. } => 换值(cond, &版),
        _ => {}
    }
    //nt: 出口实参
    let 目标: Vec<BlockId> = 后继(f, 块 as usize);
    for t in 目标 {
        for (&v, _) in &有φ[t as usize] {
            let 值 = Value::Local(*版.get(&v).unwrap_or(&v));
            补实参(f, 块, t, 值);
        }
    }
    let 孩: Vec<BlockId> = 子[块 as usize].clone();
    for c in 孩 {
        重命名(f, c, 有φ, 子, 版.clone(), 改, 计数);
    }
}

//nt: 单函数构造
fn 构造(f: &mut Function) {
    if f.blocks.is_empty() {
        return;
    }
    let 改 = 待版本化(f);
    if 改.is_empty() {
        return;
    }
    //nt: 原有块参数者跳过
    if f.blocks.iter().any(|b| !b.params.is_empty()) {
        return;
    }
    let idom = 支配树(f);
    let ps = 前驱表(f);
    let df = 支配边界(f, &idom, &ps);
    //nt: 定义块
    let mut 定义块: BTreeMap<anle_ir::LocalId, BTreeSet<BlockId>> = BTreeMap::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for s in &b.stmts {
            if let Stmt::Assign { lhs, .. } = s {
                if lhs.proj.is_empty() && 改.contains(&lhs.base) {
                    定义块.entry(lhs.base).or_default().insert(bi as BlockId);
                }
            }
        }
    }
    //nt: 插块参数
    let 块数 = f.blocks.len();
    let mut 有φ: Vec<BTreeMap<anle_ir::LocalId, anle_ir::LocalId>> = vec![BTreeMap::new(); 块数];
    let mut 计数 = 0usize;
    for (&v, 起点) in &定义块 {
        let ty = f.locals[v as usize].ty.clone();
        let 名 = f.locals[v as usize].name.clone().unwrap_or_default();
        let mut 队列: Vec<BlockId> = 起点.iter().copied().collect();
        let mut 已插: BTreeSet<BlockId> = BTreeSet::new();
        while let Some(b) = 队列.pop() {
            let 界: Vec<BlockId> = df[b as usize].iter().copied().collect();
            for d in 界 {
                if !已插.insert(d) {
                    continue;
                }
                计数 += 1;
                let phi = 新局部(f, format!("{}$φ{}", 名, 计数), ty.clone());
                f.blocks[d as usize].params.push((phi, ty.clone()));
                有φ[d as usize].insert(v, phi);
                if !起点.contains(&d) {
                    队列.push(d);
                }
            }
        }
    }
    //nt: 支配树子表
    let mut 子: Vec<Vec<BlockId>> = vec![Vec::new(); 块数];
    for (i, p) in idom.iter().enumerate() {
        if let Some(p) = p {
            if (*p as usize) < 块数 && i > 0 {
                子[*p as usize].push(i as BlockId);
            }
        }
    }
    let mut 计数2 = 0usize;
    重命名(f, 0, &有φ, &子, BTreeMap::new(), &改, &mut 计数2);
}

//nt: 平凡φ消除
//nt: 实参全同则直连
fn 消除平凡φ(f: &mut Function) {
    let mut 轮 = 0;
    loop {
        轮 += 1;
        if 轮 > 200 {
            break;
        }
        let mut 动了 = false;
        for bi in 0..f.blocks.len() {
            let 参 = f.blocks[bi].params.clone();
            for (k, (pid, _)) in 参.iter().enumerate() {
                if k >= f.blocks[bi].params.len() {
                    break;
                }
                let 前 = 前驱表(f);
                let mut 值: Option<anle_ir::LocalId> = None;
                let mut 可消 = true;
                let mut 边数 = 0;
                for p in &前[bi] {
                    let mut 找: Option<Value> = None;
                    if let Exit::Branch { targets, .. } = &f.blocks[*p as usize].exit {
                        for s in targets {
                            if s.target as usize == bi {
                                找 = s.args.get(k).cloned();
                            }
                        }
                    }
                    match 找 {
                        Some(Value::Local(x)) => {
                            if x == *pid {
                                可消 = false;
                                break;
                            }
                            match 值 {
                                None => 值 = Some(x),
                                Some(y) if y == x => {}
                                _ => {
                                    可消 = false;
                                    break;
                                }
                            }
                        }
                        _ => {
                            可消 = false;
                            break;
                        }
                    }
                    边数 += 1;
                }
                if !可消 || 边数 == 0 {
                    continue;
                }
                let Some(目标) = 值 else { continue };
                //nt: 换成目标值
                let mut 版: BTreeMap<anle_ir::LocalId, anle_ir::LocalId> = BTreeMap::new();
                版.insert(*pid, 目标);
                for b in &mut f.blocks {
                    for s in &mut b.stmts {
                        换语句(s, &版);
                    }
                    match &mut b.exit {
                        Exit::Return(Some(v)) => 换值(v, &版),
                        Exit::Branch { cond, .. } => 换值(cond, &版),
                        _ => {}
                    }
                }
                //nt: 删参数与该位实参
                f.blocks[bi].params.remove(k);
                for p in &前[bi] {
                    if let Exit::Branch { targets, .. } = &mut f.blocks[*p as usize].exit {
                        for s in targets.iter_mut() {
                            if s.target as usize == bi && k < s.args.len() {
                                s.args.remove(k);
                            }
                        }
                    }
                }
                动了 = true;
                break;
            }
        }
        if !动了 {
            break;
        }
    }
}

//nt: 走值里的局部
fn 走值<F: FnMut(&mut anle_ir::LocalId)>(v: &mut Value, g: &mut F) {
    if let Value::Local(id) = v {
        g(id);
    }
}

//nt: 走位置基
fn 走位<F: FnMut(&mut anle_ir::LocalId)>(p: &mut Place, g: &mut F) {
    g(&mut p.base);
}

//nt: 走右值
fn 走右值<F: FnMut(&mut anle_ir::LocalId)>(r: &mut Rvalue, g: &mut F) {
    match r {
        Rvalue::Use(p) | Rvalue::Ref(p) => 走位(p, g),
        Rvalue::Const(v) => 走值(v, g),
        Rvalue::BinOp { lhs, rhs, .. } => {
            走值(lhs, g);
            走值(rhs, g);
        }
        Rvalue::Call { args, .. } | Rvalue::CallIndirect { args, .. } => {
            for a in args {
                走值(a, g);
            }
        }
        Rvalue::ListLit { elems } => {
            for e in elems {
                走值(e, g);
            }
        }
        Rvalue::ListGet { list, index } => {
            走值(list, g);
            走值(index, g);
        }
        Rvalue::MapLit { pairs } => {
            for (_, v) in pairs {
                走值(v, g);
            }
        }
        Rvalue::MapGet { map, key } => {
            走值(map, g);
            走值(key, g);
        }
        Rvalue::VariantLit { args, .. } => {
            for a in args {
                走值(a, g);
            }
        }
        Rvalue::VariantTag(v) => 走值(v, g),
        Rvalue::VariantGet { value, .. } => 走值(value, g),
        Rvalue::FnRef { .. } => {}
    }
}

//nt: 走语句
fn 走语句<F: FnMut(&mut anle_ir::LocalId)>(s: &mut Stmt, g: &mut F) {
    match s {
        Stmt::Assign { lhs, rhs } => {
            走位(lhs, g);
            走右值(rhs, g);
        }
        Stmt::Call { args, .. } => {
            for a in args {
                走值(a, g);
            }
        }
        Stmt::Bind { var, .. } => g(var),
        Stmt::ListSet { list, index, value } => {
            走值(list, g);
            走值(index, g);
            走值(value, g);
        }
        Stmt::MapSet { map, key, value } => {
            走值(map, g);
            走值(key, g);
            走值(value, g);
        }
        Stmt::AccessBegin(p) | Stmt::AccessEnd(p) => 走位(p, g),
        Stmt::Assert { cond, .. } => 走值(cond, g),
        Stmt::Asm(form) => {
            for (_, v, _) in &mut form.outs {
                g(v);
            }
            for (_, v) in &mut form.ins {
                走值(v, g);
            }
        }
        Stmt::Alloc { .. } | Stmt::Drop { .. } | Stmt::DropVar { .. } | Stmt::Kill { .. } => {}
    }
}

//nt: 走出口
fn 走出口<F: FnMut(&mut anle_ir::LocalId)>(e: &mut Exit, g: &mut F) {
    match e {
        Exit::Return(Some(v)) => 走值(v, g),
        Exit::Branch { cond, targets } => {
            走值(cond, g);
            for s in targets {
                for a in &mut s.args {
                    走值(a, g);
                }
            }
        }
        _ => {}
    }
}

//nt: 清未引用局部
fn 清死局部(f: &mut Function) {
    let n = f.locals.len();
    if n == 0 {
        return;
    }
    let mut 用 = vec![false; n];
    for b in &mut f.blocks {
        for (pid, _) in &mut b.params {
            if (*pid as usize) < n {
                用[*pid as usize] = true;
            }
        }
        for s in &mut b.stmts {
            走语句(s, &mut |id: &mut anle_ir::LocalId| {
                if (*id as usize) < n {
                    用[*id as usize] = true;
                }
            });
        }
        走出口(&mut b.exit, &mut |id: &mut anle_ir::LocalId| {
            if (*id as usize) < n {
                用[*id as usize] = true;
            }
        });
    }
    for (i, l) in f.locals.iter().enumerate() {
        //nt: 参数与对象绑定必留
        if l.is_param || l.obj.is_some() || l.fn_owned {
            用[i] = true;
        }
    }
    let mut 映 = vec![0 as anle_ir::LocalId; n];
    let mut 新表: Vec<Local> = Vec::new();
    for i in 0..n {
        if 用[i] {
            映[i] = 新表.len() as anle_ir::LocalId;
            新表.push(f.locals[i].clone());
        }
    }
    if 新表.len() == n {
        return;
    }
    f.locals = 新表;
    for b in &mut f.blocks {
        for (pid, _) in &mut b.params {
            *pid = 映[*pid as usize];
        }
        for s in &mut b.stmts {
            走语句(s, &mut |id: &mut anle_ir::LocalId| {
                *id = 映[*id as usize];
            });
        }
        走出口(&mut b.exit, &mut |id: &mut anle_ir::LocalId| {
            *id = 映[*id as usize];
        });
    }
}

//nt: 全程序入口
pub fn run(p: &mut Program) {
    for f in &mut p.functions {
        构造(f);
        消除平凡φ(f);
        清死局部(f);
    }
}
