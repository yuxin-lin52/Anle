//! 标量常量折叠
//!
//! 两操作数皆为常量的 BinOp 就地折成 Const；整数按 C 语义回绕，
//! 除零与移位越界不折以保留护栏路径；str 不折。

use anle_ir::{BinOp, Program, Rvalue, Stmt, Value};

//nt: 整数二元
fn 折整(op: &BinOp, a: i64, b: i64) -> Option<i64> {
    Some(match op {
        BinOp::Add => a.wrapping_add(b),
        BinOp::Sub => a.wrapping_sub(b),
        BinOp::Mul => a.wrapping_mul(b),
        BinOp::Div => {
            if b == 0 {
                return None;
            }
            a.wrapping_div(b)
        }
        BinOp::Mod => {
            if b == 0 {
                return None;
            }
            a.wrapping_rem(b)
        }
        BinOp::BitAnd => a & b,
        BinOp::BitOr => a | b,
        BinOp::BitXor => a ^ b,
        BinOp::Shl => {
            if !(0..64).contains(&b) {
                return None;
            }
            a << b
        }
        BinOp::Shr => {
            if !(0..64).contains(&b) {
                return None;
            }
            a >> b
        }
        BinOp::Lt => (a < b) as i64,
        BinOp::Le => (a <= b) as i64,
        BinOp::Gt => (a > b) as i64,
        BinOp::Ge => (a >= b) as i64,
        BinOp::Eq => (a == b) as i64,
        BinOp::Ne => (a != b) as i64,
        BinOp::And => (a != 0 && b != 0) as i64,
        BinOp::Or => (a != 0 || b != 0) as i64,
    })
}

//nt: 浮点二元
fn 折浮(op: &BinOp, a: f64, b: f64) -> Option<f64> {
    Some(match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div => a / b,
        BinOp::Lt => ((a < b) as i64) as f64,
        BinOp::Le => ((a <= b) as i64) as f64,
        BinOp::Gt => ((a > b) as i64) as f64,
        BinOp::Ge => ((a >= b) as i64) as f64,
        BinOp::Eq => ((a == b) as i64) as f64,
        BinOp::Ne => ((a != b) as i64) as f64,
        _ => return None,
    })
}

//nt: 跑一遍
pub fn run(prog: &mut Program) {
    for f in &mut prog.functions {
        for b in &mut f.blocks {
            for st in &mut b.stmts {
                if let Stmt::Assign { rhs, .. } = st {
                    if let Rvalue::BinOp { op, lhs, rhs: r } = rhs {
                        let folded = match (lhs, r) {
                            (Value::Int(a), Value::Int(b)) => 折整(op, *a, *b).map(Value::Int),
                            (Value::Float(a), Value::Float(b)) => 折浮(op, *a, *b).map(Value::Float),
                            _ => None,
                        };
                        if let Some(c) = folded {
                            *rhs = Rvalue::Const(c);
                        }
                    }
                }
            }
        }
    }
}
