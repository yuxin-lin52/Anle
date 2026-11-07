//! 表达式语法树
//!
//! 表达式在语法层就解析成树，check 与中间表示共用同一份结构。
//! 不再各自扫 token 推断——两条路径判定不同步会静默分歧。
//!
//! 节点按引用形态组织：动词 + 前缀 + 类别 + 名，收成一个 `Call`。

use anle_error::Span;
use anle_lexer::Token;

//nt: 动词位
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    //nt: 取值调用
    Call,
    //nt: 增量改写
    Delta,
    //nt: 构造
    None,
}

//nt: 名字类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    //nt: 值
    Val,
    //nt: 函数
    Fn,
    //nt: 类型
    Type,
    //nt: 变体
    Variant,
    //nt: 内置
    Builtin,
}

//nt: 一元运算
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    //nt: 逻辑非
    Not,
    //nt: 负号
    Neg,
    //nt: 按位取反
    BitNot,
    //nt: 取地址
    Ref,
    //nt: 解引用
    Deref,
}

//nt: 二元运算
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    //nt: 算术
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    //nt: 比较
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    //nt: 逻辑
    And,
    Or,
    //nt: 位运算
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    //nt: 范围
    Range,
}

//nt: 子节点
pub type ExprRef = std::boxed::Box<Expr>;

//nt: 表达式
#[derive(Debug, Clone)]
pub struct Expr {
    //nt: 节点号
    pub id: u32,
    pub kind: ExprKind,
    pub span: Span,
}

//nt: 节点号发号器
fn 下一个节点号() -> u32 {
    use std::cell::Cell;
    thread_local! {
        static COUNTER: Cell<u32> = const { Cell::new(0) };
    }
    COUNTER.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    })
}

//nt: 表达式种类
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    //nt: 字面量
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,

    //nt: 名字引用
    Name { target: Target, name: String },

    //nt: 调用与限定引用
    Call {
        verb: Verb,
        target: Target,
        //nt: 模块前缀
        module: Option<String>,
        //nt: 块前缀
        block: Option<String>,
        name: String,
        args: Vec<Expr>,
    },

    //nt: 位置运算
    Index { base: ExprRef, index: ExprRef },
    //nt: 切片
    Slice { base: ExprRef, from: ExprRef, to: ExprRef },
    //nt: 字段
    Field { base: ExprRef, name: String },
    //nt: 解引用
    Deref(ExprRef),

    //nt: 运算
    Unary { op: UnOp, operand: ExprRef },
    Binary { op: BinOp, lhs: ExprRef, rhs: ExprRef },
    //nt: 三元
    Ternary { cond: ExprRef, then: ExprRef, otherwise: ExprRef },

    //nt: 字面量容器
    ListLit(Vec<Expr>),
    //nt: 键值表字面量
    MapLit(Vec<(Expr, Expr)>),
}

impl Expr {
    //nt: 造节点
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Self { id: 下一个节点号(), kind, span }
    }

    //nt: 节点号
    pub fn id(&self) -> u32 {
        self.id
    }
}

//nt: 相等只看结构与位置
impl PartialEq for Expr {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.span == other.span
    }
}

//nt: 语法树文本
impl std::fmt::Display for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            ExprKind::Int(n) => write!(f, "{}", n),
            ExprKind::Float(x) => write!(f, "{}", x),
            ExprKind::Str(s) => write!(f, "\"{}\"", s),
            ExprKind::Bool(b) => write!(f, "{}", if *b { "re" } else { "fa" }),
            ExprKind::Null => write!(f, "null"),
            ExprKind::Name { name, .. } => write!(f, "{}", name),
            ExprKind::Call { verb, target, module, block, name, args } => {
                let mut s = String::new();
                match verb {
                    Verb::Call => s.push_str("call "),
                    Verb::Delta => s.push_str("delta "),
                    Verb::None => {}
                }
                if let Some(m) = module {
                    s.push_str(&format!("##:{} ", m));
                }
                if let Some(b) = block {
                    s.push_str(&format!("#:{} ", b));
                }
                s.push_str(&format!("{:?}(\"{}\")", target, name));
                if !args.is_empty() {
                    let a: Vec<String> = args.iter().map(|x| x.to_string()).collect();
                    s.push_str(&format!("({})", a.join(", ")));
                }
                write!(f, "{}", s)
            }
            ExprKind::Index { base, index } => write!(f, "{}[{}]", base, index),
            ExprKind::Slice { base, from, to } => write!(f, "{}[{}~{}]", base, from, to),
            ExprKind::Field { base, name } => write!(f, "{}.{}", base, name),
            ExprKind::Deref(e) => write!(f, "*{}", e),
            ExprKind::Unary { op, operand } => write!(f, "({:?} {})", op, operand),
            ExprKind::Binary { op, lhs, rhs } => write!(f, "({:?} {} {})", op, lhs, rhs),
            ExprKind::Ternary { cond, then, otherwise } => {
                write!(f, "({} ? {} : {})", cond, then, otherwise)
            }
            ExprKind::ListLit(xs) => {
                let a: Vec<String> = xs.iter().map(|x| x.to_string()).collect();
                write!(f, "[{}]", a.join(", "))
            }
            ExprKind::MapLit(ps) => {
                let a: Vec<String> = ps.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
                write!(f, "{{{}}}", a.join(", "))
            }
        }
    }
}

//nt: 表达式解析器
pub struct Parser<'a> {
    //nt: 记号流
    toks: &'a [Token],
    //nt: 位置表
    spans: &'a [(usize, usize)],
    //nt: 游标
    pub i: usize,
    //nt: 行号
    pub line: usize,
}

impl<'a> Parser<'a> {
    //nt: 构造
    pub fn new(toks: &'a [Token], spans: &'a [(usize, usize)], line: usize) -> Self {
        Self { toks, spans, i: 0, line }
    }

    //nt: 看当前
    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.i)
    }

    //nt: 取跨度
    fn span(&self) -> Span {
        let (c, l) = self.spans.get(self.i).copied().unwrap_or((1, 0));
        Span { line: self.line, col: c, len: l }
    }

    //nt: 收尾检查
    pub fn done(&self) -> bool {
        self.i >= self.toks.len()
    }

    //nt: 解析入口
    pub fn parse(&mut self) -> Result<Expr, String> {
        let e = self.ternary()?;
        if !self.done() {
            return Err(format!("表达式没读完，剩余 {:?}", &self.toks[self.i..]));
        }
        Ok(e)
    }

    //nt: 三元最低
    fn ternary(&mut self) -> Result<Expr, String> {
        let cond = self.or()?;
        if matches!(self.peek(), Some(Token::Sym("?"))) {
            self.i += 1;
            let a = self.ternary()?;
            self.expect(Token::Sym(":"))?;
            let b = self.ternary()?;
            let span = cond.span;
            return Ok(Expr::new(ExprKind::Ternary {
                cond: Box::new(cond),
                then: Box::new(a),
                otherwise: Box::new(b),
            }, span));
        }
        Ok(cond)
    }

    //nt: 或
    fn or(&mut self) -> Result<Expr, String> {
        let mut l = self.and()?;
        while matches!(self.peek(), Some(Token::Kw("or"))) {
            self.i += 1;
            let r = self.and()?;
            let span = l.span;
            l = Expr::new(ExprKind::Binary { op: BinOp::Or, lhs: Box::new(l), rhs: Box::new(r) }, span);
        }
        Ok(l)
    }

    //nt: 与
    fn and(&mut self) -> Result<Expr, String> {
        let mut l = self.cmp()?;
        while matches!(self.peek(), Some(Token::Kw("and"))) {
            self.i += 1;
            let r = self.cmp()?;
            let span = l.span;
            l = Expr::new(ExprKind::Binary { op: BinOp::And, lhs: Box::new(l), rhs: Box::new(r) }, span);
        }
        Ok(l)
    }

    //nt: 比较
    fn cmp(&mut self) -> Result<Expr, String> {
        let mut l = self.bitor()?;
        loop {
            let op = match self.peek() {
                Some(Token::Sym("==")) => BinOp::Eq,
                Some(Token::Sym("!=")) => BinOp::Ne,
                Some(Token::Sym("<")) => BinOp::Lt,
                Some(Token::Sym("<=")) => BinOp::Le,
                Some(Token::Sym(">")) => BinOp::Gt,
                Some(Token::Sym(">=")) => BinOp::Ge,
                _ => return Ok(l),
            };
            self.i += 1;
            let r = self.bitor()?;
            let span = l.span;
            l = Expr::new(ExprKind::Binary { op, lhs: Box::new(l), rhs: Box::new(r) }, span);
        }
    }

    //nt: 位或
    fn bitor(&mut self) -> Result<Expr, String> {
        let mut l = self.bitxor()?;
        while matches!(self.peek(), Some(Token::Sym("|"))) {
            self.i += 1;
            let r = self.bitxor()?;
            let span = l.span;
            l = Expr::new(ExprKind::Binary { op: BinOp::BitOr, lhs: Box::new(l), rhs: Box::new(r) }, span);
        }
        Ok(l)
    }

    //nt: 位异或
    fn bitxor(&mut self) -> Result<Expr, String> {
        let mut l = self.bitand()?;
        while matches!(self.peek(), Some(Token::Sym("^"))) {
            self.i += 1;
            let r = self.bitand()?;
            let span = l.span;
            l = Expr::new(ExprKind::Binary { op: BinOp::BitXor, lhs: Box::new(l), rhs: Box::new(r) }, span);
        }
        Ok(l)
    }

    //nt: 位与
    fn bitand(&mut self) -> Result<Expr, String> {
        let mut l = self.shift()?;
        while matches!(self.peek(), Some(Token::Sym("&"))) {
            self.i += 1;
            let r = self.shift()?;
            let span = l.span;
            l = Expr::new(ExprKind::Binary { op: BinOp::BitAnd, lhs: Box::new(l), rhs: Box::new(r) }, span);
        }
        Ok(l)
    }

    //nt: 移位
    fn shift(&mut self) -> Result<Expr, String> {
        let mut l = self.add()?;
        loop {
            let op = match self.peek() {
                Some(Token::Sym("<<")) => BinOp::Shl,
                Some(Token::Sym(">>")) => BinOp::Shr,
                _ => return Ok(l),
            };
            self.i += 1;
            let r = self.add()?;
            let span = l.span;
            l = Expr::new(ExprKind::Binary { op, lhs: Box::new(l), rhs: Box::new(r) }, span);
        }
    }

    //nt: 加减
    fn add(&mut self) -> Result<Expr, String> {
        let mut l = self.mul()?;
        loop {
            let op = match self.peek() {
                Some(Token::Sym("+")) => BinOp::Add,
                Some(Token::Sym("-")) => BinOp::Sub,
                _ => return Ok(l),
            };
            self.i += 1;
            let r = self.mul()?;
            let span = l.span;
            l = Expr::new(ExprKind::Binary { op, lhs: Box::new(l), rhs: Box::new(r) }, span);
        }
    }

    //nt: 乘除模
    fn mul(&mut self) -> Result<Expr, String> {
        let mut l = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Token::Sym("*")) => BinOp::Mul,
                Some(Token::Sym("/")) => BinOp::Div,
                Some(Token::Sym("%")) => BinOp::Mod,
                _ => return Ok(l),
            };
            self.i += 1;
            let r = self.unary()?;
            let span = l.span;
            l = Expr::new(ExprKind::Binary { op, lhs: Box::new(l), rhs: Box::new(r) }, span);
        }
    }

    //nt: 一元
    fn unary(&mut self) -> Result<Expr, String> {
        let span = self.span();
        let op = match self.peek() {
            Some(Token::Kw("not")) => Some(UnOp::Not),
            Some(Token::Sym("-")) => Some(UnOp::Neg),
            Some(Token::Sym("!")) => Some(UnOp::BitNot),
            Some(Token::Sym("&")) => Some(UnOp::Ref),
            _ => None,
        };
        //nt: 一元加号无意义
        if matches!(self.peek(), Some(Token::Sym("+"))) {
            self.i += 1;
            return self.unary();
        }
        if let Some(op) = op {
            self.i += 1;
            let e = self.unary()?;
            return Ok(Expr::new(ExprKind::Unary { op, operand: Box::new(e) }, span));
        }
        self.postfix()
    }

    //nt: 后缀
    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.atom()?;
        loop {
            let span = e.span;
            match self.peek() {
                Some(Token::Sym("[")) => {
                    self.i += 1;
                    let a = self.or()?;
                    if matches!(self.peek(), Some(Token::Sym("~"))) {
                        self.i += 1;
                        let b = self.or()?;
                        self.expect(Token::Sym("]"))?;
                        e = Expr::new(ExprKind::Slice { base: Box::new(e), from: Box::new(a), to: Box::new(b) }, span);
                    } else {
                        self.expect(Token::Sym("]"))?;
                        e = Expr::new(ExprKind::Index { base: Box::new(e), index: Box::new(a) }, span);
                    }
                }
                Some(Token::Sym(".")) => {
                    self.i += 1;
                    let n = match self.peek() {
                        Some(Token::Ident(n)) | Some(Token::Str(n)) => n.clone(),
                        _ => return Err("字段访问要写字段名".into()),
                    };
                    self.i += 1;
                    e = Expr::new(ExprKind::Field { base: Box::new(e), name: n.to_string() }, span);
                }
                _ => return Ok(e),
            }
        }
    }

    //nt: 吃掉期望记号
    fn expect(&mut self, t: Token) -> Result<(), String> {
        if self.peek() == Some(&t) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("期望 {:?}，来的是 {:?}", t, self.peek()))
        }
    }

    //nt: 实参表
    fn args(&mut self) -> Result<Vec<Expr>, String> {
        self.expect(Token::Sym("("))?;
        let mut xs = Vec::new();
        while !matches!(self.peek(), Some(Token::Sym(")"))) {
            xs.push(self.or()?);
            if matches!(self.peek(), Some(Token::Sym(","))) {
                self.i += 1;
            }
        }
        self.expect(Token::Sym(")"))?;
        Ok(xs)
    }

    //nt: 跨块引用
    fn cross_expr(&mut self, span: Span) -> Result<Expr, String> {
        let mut module = None;
        if matches!(self.peek(), Some(Token::HashHashColon)) {
            self.i += 1;
            let mut m = self.take_name()?;
            while matches!(self.peek(), Some(Token::Sym("."))) {
                self.i += 1;
                m.push('.');
                m.push_str(&self.take_name()?);
            }
            module = Some(m);
            self.expect(Token::HashColon)?;
        } else {
            self.i += 1;
        }
        let block = self.take_name()?;
        let target = match self.peek().cloned() {
            Some(Token::Kw("val")) => {
                self.i += 1;
                Target::Val
            }
            Some(Token::Kw("fn")) => {
                self.i += 1;
                Target::Fn
            }
            _ => return Err("跨块引用后接 val 或 fn".into()),
        };
        let name = self.take_name()?;
        let args = if matches!(self.peek(), Some(Token::Sym("("))) {
            self.args()?
        } else {
            Vec::new()
        };
        Ok(Expr::new(ExprKind::Call {
            verb: Verb::Call,
            target,
            module,
            block: Some(block),
            name,
            args,
        }, span))
    }

    //nt: 调用形态
    fn call_expr(&mut self, span: Span) -> Result<Expr, String> {
        self.i += 1;
        if matches!(self.peek(), Some(Token::HashColon) | Some(Token::HashHashColon)) {
            return self.cross_expr(span);
        }
        let (target, name) = match self.peek().cloned() {
            Some(Token::Kw("val")) => {
                self.i += 1;
                let n = self.take_name()?;
                //nt: 有实参即间接调用
                let args = if matches!(self.peek(), Some(Token::Sym("("))) {
                    self.args()?
                } else {
                    Vec::new()
                };
                return Ok(Expr::new(ExprKind::Call {
                    verb: Verb::Call,
                    target: Target::Val,
                    module: None,
                    block: None,
                    name: n,
                    args,
                }, span));
            }
            Some(Token::Kw("fn")) => (Target::Fn, self.take_name_after()?),
            _ => return Err("call 后应接 val 或 fn".into()),
        };
        let args = self.args()?;
        Ok(Expr::new(ExprKind::Call {
            verb: Verb::Call,
            target,
            module: None,
            block: None,
            name,
            args,
        }, span))
    }

    //nt: 取名字
    fn take_name(&mut self) -> Result<String, String> {
        match self.peek().cloned() {
            Some(Token::Ident(n)) | Some(Token::Str(n)) => {
                self.i += 1;
                Ok(n.to_string())
            }
            _ => Err("这里要写名字".into()),
        }
    }

    //nt: 跳过类别取名字
    fn take_name_after(&mut self) -> Result<String, String> {
        self.i += 1;
        self.take_name()
    }

    //nt: 原子
    fn atom(&mut self) -> Result<Expr, String> {
        let span = self.span();
        match self.peek().cloned() {
            Some(Token::Int(n)) => {
                self.i += 1;
                Ok(Expr::new(ExprKind::Int(n), span))
            }
            Some(Token::Float(x)) => {
                self.i += 1;
                Ok(Expr::new(ExprKind::Float(x), span))
            }
            Some(Token::Str(s)) | Some(Token::RawStr(s)) => {
                self.i += 1;
                if matches!(self.peek(), Some(Token::Sym("("))) {
                    let args = self.args()?;
                    return Ok(Expr::new(ExprKind::Call {
                        verb: Verb::None,
                        target: Target::Type,
                        module: None,
                        block: None,
                        name: s.to_string(),
                        args,
                    }, span));
                }
                Ok(Expr::new(ExprKind::Str(s.to_string()), span))
            }
            Some(Token::Kw("re")) => {
                self.i += 1;
                Ok(Expr::new(ExprKind::Bool(true), span))
            }
            Some(Token::Kw("fa")) => {
                self.i += 1;
                Ok(Expr::new(ExprKind::Bool(false), span))
            }
            Some(Token::Kw("null")) => {
                self.i += 1;
                Ok(Expr::new(ExprKind::Null, span))
            }
            Some(Token::Sym("(")) => {
                self.i += 1;
                let e = self.or()?;
                self.expect(Token::Sym(")"))?;
                Ok(e)
            }
            Some(Token::Sym("[")) => {
                self.i += 1;
                let mut xs = Vec::new();
                while !matches!(self.peek(), Some(Token::Sym("]"))) {
                    xs.push(self.or()?);
                    if matches!(self.peek(), Some(Token::Sym(","))) {
                        self.i += 1;
                    }
                }
                self.expect(Token::Sym("]"))?;
                Ok(Expr::new(ExprKind::ListLit(xs), span))
            }
            Some(Token::Sym("{")) => {
                self.i += 1;
                let mut ps = Vec::new();
                while !matches!(self.peek(), Some(Token::Sym("}"))) {
                    let k = self.or()?;
                    self.expect(Token::Sym(":"))?;
                    let v = self.or()?;
                    ps.push((k, v));
                    if matches!(self.peek(), Some(Token::Sym(","))) {
                        self.i += 1;
                    }
                }
                self.expect(Token::Sym("}"))?;
                Ok(Expr::new(ExprKind::MapLit(ps), span))
            }
            Some(Token::Kw("call")) => self.call_expr(span),
            Some(Token::Kw("fn")) => {
                self.i += 1;
                let n = self.take_name()?;
                Ok(Expr::new(ExprKind::Name { target: Target::Fn, name: n }, span))
            }
            Some(Token::Kw(k)) if matches!(k, "list" | "que" | "stack" | "map") => {
                self.i += 1;
                let args = if matches!(self.peek(), Some(Token::Sym("("))) {
                    self.args()?
                } else {
                    Vec::new()
                };
                Ok(Expr::new(ExprKind::Call {
                    verb: Verb::None,
                    target: Target::Type,
                    module: None,
                    block: None,
                    name: k.to_string(),
                    args,
                }, span))
            }
            Some(Token::Kw("fn")) => {
                self.i += 1;
                let n = self.take_name()?;
                Ok(Expr::new(ExprKind::Name { target: Target::Fn, name: n }, span))
            }
            Some(Token::Kw("val")) => {
                self.i += 1;
                let n = self.take_name()?;
                let (verb, args) = if matches!(self.peek(), Some(Token::Sym("("))) {
                    (Verb::Delta, self.args()?)
                } else {
                    (Verb::Call, Vec::new())
                };
                Ok(Expr::new(ExprKind::Call {
                    verb,
                    target: Target::Val,
                    module: None,
                    block: None,
                    name: n,
                    args,
                }, span))
            }
            Some(Token::Ident(n)) => {
                self.i += 1;
                if matches!(self.peek(), Some(Token::Sym("("))) {
                    let args = self.args()?;
                    return Ok(Expr::new(ExprKind::Call {
                        verb: Verb::None,
                        target: Target::Builtin,
                        module: None,
                        block: None,
                        name: n.to_string(),
                        args,
                    }, span));
                }
                Ok(Expr::new(ExprKind::Name { target: Target::Builtin, name: n.to_string() }, span))
            }
            other => Err(format!("这里缺一个值，来的是 {:?}", other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anle_lexer::lex;

    fn 解析(src: &str) -> String {
        let all = lex(src).unwrap();
        let ts: Vec<Token> = all.iter().filter(|s| s.tok != Token::Newline).map(|s| s.tok.clone()).collect();
        let sp: Vec<(usize, usize)> = all.iter().filter(|s| s.tok != Token::Newline).map(|s| (s.col, s.len)).collect();
        let mut p = Parser::new(&ts, &sp, 1);
        p.parse().unwrap().to_string()
    }

    #[test]
    fn 优先级() {
        assert_eq!(解析("1 + 2 * 3"), "(Add 1 (Mul 2 3))");
        assert_eq!(解析("1 < 2 and 3 > 4"), "(And (Lt 1 2) (Gt 3 4))");
        assert_eq!(解析("a | b & c"), "(BitOr a (BitAnd b c))");
    }

    #[test]
    fn 后缀() {
        assert_eq!(解析("表[1]"), "表[1]");
        assert_eq!(解析("表[1~2]"), "表[1~2]");
        assert_eq!(解析("位.x"), "位.x");
        assert_eq!(解析("a > 0 ? 1 : 2"), "((Gt a 0) ? 1 : 2)");
    }

    #[test]
    fn 字面量容器() {
        assert_eq!(解析("[1, 2, 3]"), "[1, 2, 3]");
        assert_eq!(解析("{\"甲\": 1}"), "{\"甲\": 1}");
    }

    #[test]
    fn 增量形态() {
        println!("delta = {}", 解析("val \"次\" (+2)"));
        println!("构造 = {}", 解析("\"点\"(3, 4)"));
        println!("跨块 = {}", 解析("call #:工具块 val \"常数\""));
    }
}

#[cfg(test)]
mod 覆盖 {
    use crate::{parse_all, Stmt};
    use anle_lexer::lex;

    fn 统计(stmts: &[Stmt], 总: &mut usize, 有: &mut usize) {
        for s in stmts {
            match s {
                Stmt::Line { exprs, kind, .. } => {
                    *总 += 1;
                    if !exprs.is_empty() {
                        *有 += 1;
                    } else {
                        let 前: Vec<String> = kind.tokens().iter().take(6).map(|t| format!("{:?}", t)).collect();
                        println!("  未解析 第{}行: {}", line, 前.join(" "));
                    }
                }
                Stmt::Struct { body, .. } | Stmt::Fn { body, .. } => 统计(body, 总, 有),
            }
        }
    }

    #[test]
    #[ignore]
    fn 真实文件覆盖率() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../..");
        for f in ["_验收检查/全特性.an", "anle/std/strings.an", "anle/std/collections.an"] {
            let p = format!("{}/{}", root, f);
            let Ok(src) = std::fs::read_to_string(&p) else { continue };
            let Ok(toks) = lex(&src) else { continue };
            let (ast, _) = parse_all(toks);
            let (mut 总, mut 有) = (0usize, 0usize);
            for b in &ast.blocks {
                统计(&b.stmts, &mut 总, &mut 有);
            }
            println!("{}: 语句 {}，有表达式 {}", f, 总, 有);
        }
    }
}
