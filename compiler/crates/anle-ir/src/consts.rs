//! 编译期求值（常量折叠）
//!
//! 只折字面量与运算符：整数 / 浮点 / 字符串 / 真假值，
//! 算术 `+ - * / %`、比较、`and` / `or` / `not`、括号。
//! 含名字、调用、下标的表达式返回 None，交调用方按普通表达式处理。

use anle_lexer::Token;

use crate::Value;

//nt: 折叠结果
#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
}

impl Const {
    //nt: 转 IR 值
    pub fn 转值(&self) -> Value {
        match self {
            Const::Int(n) => Value::Int(*n),
            Const::Float(x) => Value::Float(*x),
            Const::Str(t) => Value::Str(t.clone()),
            Const::Bool(b) => Value::Bool(*b),
        }
    }

    pub fn as_token(&self) -> Token {
        match self {
            Const::Int(n) => Token::Int(*n),
            Const::Float(x) => Token::Float(*x),
            Const::Str(s) => Token::Str(s.clone()),
            Const::Bool(b) => {
                if *b {
                    Token::Kw("re")
                } else {
                    Token::Kw("fa")
                }
            }
        }
    }
}

//nt: 折整个记号段
pub fn 折叠(toks: &[Token]) -> Option<Const> {
    let mut p = 解 { t: toks, i: 0 };
    let v = p.或()?;
    if p.i != toks.len() {
        return None;
    }
    Some(v)
}

//nt: 折条件真假
pub fn 折叠条件(toks: &[Token]) -> Option<bool> {
    match 折叠(toks)? {
        Const::Bool(b) => Some(b),
        _ => None,
    }
}

struct 解<'a> {
    t: &'a [Token],
    i: usize,
}

impl<'a> 解<'a> {
    fn 或(&mut self) -> Option<Const> {
        let mut l = self.与()?;
        while matches!(self.t.get(self.i), Some(Token::Kw("or"))) {
            self.i += 1;
            let r = self.与()?;
            l = Const::Bool(作真(&l) || 作真(&r));
        }
        Some(l)
    }

    fn 与(&mut self) -> Option<Const> {
        let mut l = self.比()?;
        while matches!(self.t.get(self.i), Some(Token::Kw("and"))) {
            self.i += 1;
            let r = self.比()?;
            l = Const::Bool(作真(&l) && 作真(&r));
        }
        Some(l)
    }

    fn 比(&mut self) -> Option<Const> {
        let mut l = self.加()?;
        loop {
            let op = match self.t.get(self.i) {
                Some(Token::Sym(s)) if ["==", "!=", "<", "<=", ">", ">="].contains(s) => *s,
                _ => return Some(l),
            };
            self.i += 1;
            let r = self.加()?;
            let b = 比较(&l, &r, op)?;
            l = Const::Bool(b);
        }
    }

    fn 加(&mut self) -> Option<Const> {
        let mut l = self.乘()?;
        loop {
            let op = match self.t.get(self.i) {
                Some(Token::Sym(s)) if *s == "+" || *s == "-" => *s,
                _ => return Some(l),
            };
            self.i += 1;
            let r = self.乘()?;
            l = 加减(&l, &r, op)?;
        }
    }

    fn 乘(&mut self) -> Option<Const> {
        let mut l = self.元()?;
        loop {
            let op = match self.t.get(self.i) {
                Some(Token::Sym(s)) if *s == "*" || *s == "/" || *s == "%" => *s,
                _ => return Some(l),
            };
            self.i += 1;
            let r = self.元()?;
            l = 乘除(&l, &r, op)?;
        }
    }

    fn 元(&mut self) -> Option<Const> {
        match self.t.get(self.i)? {
            Token::Int(n) => {
                self.i += 1;
                Some(Const::Int(*n))
            }
            Token::Float(x) => {
                self.i += 1;
                Some(Const::Float(*x))
            }
            Token::Str(s) | Token::RawStr(s) => {
                self.i += 1;
                Some(Const::Str(s.clone()))
            }
            Token::Kw("re") => {
                self.i += 1;
                Some(Const::Bool(true))
            }
            Token::Kw("fa") => {
                self.i += 1;
                Some(Const::Bool(false))
            }
            Token::Kw("not") => {
                self.i += 1;
                let v = self.元()?;
                Some(Const::Bool(!作真(&v)))
            }
            Token::Sym("(") => {
                self.i += 1;
                let v = self.或()?;
                if matches!(self.t.get(self.i), Some(Token::Sym(")"))) {
                    self.i += 1;
                    Some(v)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

fn 作真(c: &Const) -> bool {
    match c {
        Const::Bool(b) => *b,
        Const::Int(n) => *n != 0,
        Const::Float(x) => *x != 0.0,
        Const::Str(s) => !s.is_empty(),
    }
}

fn 比较(l: &Const, r: &Const, op: &str) -> Option<bool> {
    //nt: 数值比较
    let 数值 = |c: &Const| match c {
        Const::Int(n) => Some(*n as f64),
        Const::Float(x) => Some(*x),
        _ => None,
    };
    if let (Some(a), Some(b)) = (数值(l), 数值(r)) {
        return Some(match op {
            "==" => a == b,
            "!=" => a != b,
            "<" => a < b,
            "<=" => a <= b,
            ">" => a > b,
            ">=" => a >= b,
            _ => return None,
        });
    }
    if let (Const::Str(a), Const::Str(b)) = (l, r) {
        return Some(match op {
            "==" => a == b,
            "!=" => a != b,
            "<" => a < b,
            "<=" => a <= b,
            ">" => a > b,
            ">=" => a >= b,
            _ => return None,
        });
    }
    if let (Const::Bool(a), Const::Bool(b)) = (l, r) {
        return Some(match op {
            "==" => a == b,
            "!=" => a != b,
            _ => return None,
        });
    }
    None
}

fn 加减(l: &Const, r: &Const, op: &str) -> Option<Const> {
    match (l, r) {
        (Const::Int(a), Const::Int(b)) => Some(Const::Int(if op == "+" { a + b } else { a - b })),
        (Const::Str(a), Const::Str(b)) if op == "+" => Some(Const::Str(format!("{}{}", a, b))),
        _ => {
            let (a, b) = (作浮(l)?, 作浮(r)?);
            Some(Const::Float(if op == "+" { a + b } else { a - b }))
        }
    }
}

fn 乘除(l: &Const, r: &Const, op: &str) -> Option<Const> {
    if let (Const::Int(a), Const::Int(b)) = (l, r) {
        return match op {
            "*" => Some(Const::Int(a * b)),
            "/" => {
                if *b == 0 {
                    None
                } else {
                    Some(Const::Int(a / b))
                }
            }
            "%" => {
                if *b == 0 {
                    None
                } else {
                    Some(Const::Int(a % b))
                }
            }
            _ => None,
        };
    }
    let (a, b) = (作浮(l)?, 作浮(r)?);
    match op {
        "*" => Some(Const::Float(a * b)),
        "/" => {
            if b == 0.0 {
                None
            } else {
                Some(Const::Float(a / b))
            }
        }
        _ => None,
    }
}

fn 作浮(c: &Const) -> Option<f64> {
    match c {
        Const::Int(n) => Some(*n as f64),
        Const::Float(x) => Some(*x),
        _ => None,
    }
}

//nt: 带环境折叠
//nt: 查变量与调函数
pub fn 折叠带环境(
    toks: &[Token],
    取变量: &mut dyn FnMut(&str) -> Option<Const>,
    调函数: &mut dyn FnMut(&str, &[Const]) -> Option<Const>,
) -> Option<Const> {
    let mut p = 解带环境 { t: toks, i: 0, 取变量, 调函数 };
    let v = p.或()?;
    if p.i != toks.len() {
        return None;
    }
    Some(v)
}

struct 解带环境<'a> {
    t: &'a [Token],
    i: usize,
    取变量: &'a mut dyn FnMut(&str) -> Option<Const>,
    调函数: &'a mut dyn FnMut(&str, &[Const]) -> Option<Const>,
}

impl<'a> 解带环境<'a> {
    fn 或(&mut self) -> Option<Const> {
        let mut l = self.与()?;
        while matches!(self.t.get(self.i), Some(Token::Kw("or"))) {
            self.i += 1;
            let r = self.与()?;
            l = Const::Bool(作真(&l) || 作真(&r));
        }
        Some(l)
    }

    fn 与(&mut self) -> Option<Const> {
        let mut l = self.比()?;
        while matches!(self.t.get(self.i), Some(Token::Kw("and"))) {
            self.i += 1;
            let r = self.比()?;
            l = Const::Bool(作真(&l) && 作真(&r));
        }
        Some(l)
    }

    fn 比(&mut self) -> Option<Const> {
        let mut l = self.加()?;
        loop {
            let op = match self.t.get(self.i) {
                Some(Token::Sym(s)) if ["==", "!=", "<", "<=", ">", ">="].contains(s) => *s,
                _ => return Some(l),
            };
            self.i += 1;
            let r = self.加()?;
            l = Const::Bool(比较(&l, &r, op)?);
        }
    }

    fn 加(&mut self) -> Option<Const> {
        let mut l = self.乘()?;
        loop {
            let op = match self.t.get(self.i) {
                Some(Token::Sym(s)) if *s == "+" || *s == "-" => *s,
                _ => return Some(l),
            };
            self.i += 1;
            let r = self.乘()?;
            l = 加减(&l, &r, op)?;
        }
    }

    fn 乘(&mut self) -> Option<Const> {
        let mut l = self.元()?;
        loop {
            let op = match self.t.get(self.i) {
                Some(Token::Sym(s)) if *s == "*" || *s == "/" || *s == "%" => *s,
                _ => return Some(l),
            };
            self.i += 1;
            let r = self.元()?;
            l = 乘除(&l, &r, op)?;
        }
    }

    fn 元(&mut self) -> Option<Const> {
        match self.t.get(self.i)? {
            Token::Int(n) => {
                self.i += 1;
                Some(Const::Int(*n))
            }
            Token::Float(x) => {
                self.i += 1;
                Some(Const::Float(*x))
            }
            Token::Str(s) | Token::RawStr(s) => {
                self.i += 1;
                Some(Const::Str(s.clone()))
            }
            Token::Kw("re") => {
                self.i += 1;
                Some(Const::Bool(true))
            }
            Token::Kw("fa") => {
                self.i += 1;
                Some(Const::Bool(false))
            }
            Token::Kw("not") => {
                self.i += 1;
                Some(Const::Bool(!作真(&self.元()?)))
            }
            //nt: 读环境
            Token::Kw("val") => {
                let n = match self.t.get(self.i + 1) {
                    Some(Token::Str(s)) | Some(Token::Ident(s)) => s.clone(),
                    _ => return None,
                };
                self.i += 2;
                (self.取变量)(&n)
            }
            //nt: 折实参再调用
            Token::Kw("call") => {
                if !matches!(self.t.get(self.i + 1), Some(Token::Kw("fn"))) {
                    return None;
                }
                let n = match self.t.get(self.i + 2) {
                    Some(Token::Str(s)) | Some(Token::Ident(s)) => s.clone(),
                    _ => return None,
                };
                if !matches!(self.t.get(self.i + 3), Some(Token::Sym("("))) {
                    return None;
                }
                self.i += 4;
                let mut 实参: Vec<Const> = Vec::new();
                loop {
                    if matches!(self.t.get(self.i), Some(Token::Sym(")"))) {
                        self.i += 1;
                        break;
                    }
                    let a = self.或()?;
                    实参.push(a);
                    match self.t.get(self.i) {
                        Some(Token::Sym(",")) => {
                            self.i += 1;
                        }
                        Some(Token::Sym(")")) => {
                            self.i += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                (self.调函数)(&n, &实参)
            }
            Token::Ident(n) => {
                let name = n.clone();
                self.i += 1;
                (self.取变量)(&name)
            }
            Token::Sym("(") => {
                self.i += 1;
                let v = self.或()?;
                if matches!(self.t.get(self.i), Some(Token::Sym(")"))) {
                    self.i += 1;
                    Some(v)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}