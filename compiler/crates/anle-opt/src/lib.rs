//! 中端 pass 框架
//!
//! 依赖只到 anle-ir——每个 pass 是纯 IR→IR 变换，可独立测试。
//! 判据纪律：每加一个 pass，行为不变（矩阵 + 全特性 + guard）。

pub mod conddc;
pub mod flatten;
pub mod constfold;
pub mod dce;
pub mod inline;
pub mod licm;
pub mod ssa;

use anle_ir::Program;

//nt: pass 名册
pub fn passes() -> Vec<(&'static str, fn(&mut Program))> {
    vec![
        ("conddc", conddc::run),
        ("constfold", constfold::run),
        ("inline", inline::run),
        ("dce", dce::run),
        ("ssa", ssa::run),
        ("licm", licm::run),
        //nt: SSA 稳定后判平铺
        ("flatten", flatten::run),
        //nt: SSA 后重折叠
        ("constfold2", constfold::run),
        ("dce2", dce::run),
    ]
}

//nt: 跑一遍全 pass
pub fn run(prog: &mut Program) {
    for (name, pass) in passes() {
        pass(prog);
        let _ = name;
    }
}
