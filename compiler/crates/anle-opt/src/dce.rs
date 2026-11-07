//! 不可达块消除
//!
//! 从入口块做可达性标记，删去标记外的块。
//! 不可达块没有执行路径进入，删去不改变任何可观察行为。

use anle_ir::{Exit, Program};

//nt: 每函数可达标记
fn 可达_of(f: &anle_ir::Function) -> Vec<bool> {
    let mut vis = vec![false; f.blocks.len()];
    let mut stack = vec![0usize];
    vis[0] = true;
    while let Some(bi) = stack.pop() {
        let b = &f.blocks[bi];
        let targets: Vec<usize> = match &b.exit {
            Exit::Branch { targets, .. } => targets.iter().map(|s| s.target).collect(),
            _ => vec![],
        };
        for t in targets {
            if (t as usize) < vis.len() && !vis[t] {
                vis[t] = true;
                stack.push(t);
            }
        }
    }
    vis
}

//nt: 跑一遍
pub fn run(prog: &mut Program) {
    for f in &mut prog.functions {
        if f.blocks.is_empty() {
            continue;
        }
        let vis = 可达_of(f);
        let mut remap: Vec<Option<usize>> = vec![None; f.blocks.len()];
        let mut new_blocks = Vec::with_capacity(f.blocks.len());
        for (bi, mut b) in std::mem::take(&mut f.blocks).into_iter().enumerate() {
            if vis[bi] {
                remap[bi] = Some(new_blocks.len());
                b.id = new_blocks.len();
                new_blocks.push(b);
            }
        }
        for b in &mut new_blocks {
            if let Exit::Branch { targets, .. } = &mut b.exit {
                for s in targets.iter_mut() {
                    if let Some(n) = remap[s.target] {
                        s.target = n;
                    }
                }
            }
        }
        f.blocks = new_blocks;
    }
}
