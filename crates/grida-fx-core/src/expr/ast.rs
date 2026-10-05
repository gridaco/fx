//! The expression tree.

/// A literal: `null`, `true`/`false`, a number, a string (already unquoted: quotes dropped and
/// every backslash pair `\X` replaced by `X`).
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `!`
    Not,
    /// `-`
    Neg,
}

impl UnaryOp {
    pub fn text(self) -> &'static str {
        match self {
            UnaryOp::Not => "!",
            UnaryOp::Neg => "-",
        }
    }
}

/// Binary operators with gnode's binding powers; a higher power binds tighter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    /// `??`, power 10, right-associative.
    Coalesce,
    /// `||`, 20
    Or,
    /// `&&`, 30
    And,
    /// `==`, 40
    Eq,
    /// `!=`, 40
    Ne,
    /// `<`, 50
    Lt,
    /// `<=`, 50
    Le,
    /// `>`, 50
    Gt,
    /// `>=`, 50
    Ge,
    /// `+`, 60
    Add,
    /// `-`, 60
    Sub,
    /// `*`, 70
    Mul,
    /// `/`, 70
    Div,
}

impl BinaryOp {
    /// The operator as written; also the `op` of type errors (`+ needs numbers, not text`).
    pub fn text(self) -> &'static str {
        match self {
            BinaryOp::Coalesce => "??",
            BinaryOp::Or => "||",
            BinaryOp::And => "&&",
            BinaryOp::Eq => "==",
            BinaryOp::Ne => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
        }
    }

    /// The binding power: `??` 10 (right-associative), `||` 20, `&&` 30, `==` `!=` 40, `<` `<=`
    /// `>` `>=` 50, `+` `-` 60, `*` `/` 70. Prefix `!` and `-` parse their operand at 80.
    pub fn power(self) -> u8 {
        match self {
            BinaryOp::Coalesce => 10,
            BinaryOp::Or => 20,
            BinaryOp::And => 30,
            BinaryOp::Eq | BinaryOp::Ne => 40,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => 50,
            BinaryOp::Add | BinaryOp::Sub => 60,
            BinaryOp::Mul | BinaryOp::Div => 70,
        }
    }
}

/// The eleven expression functions, in the order the "not an expression function" message lists
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    Accepted,
    Concat,
    Contains,
    Digest,
    Facts,
    Join,
    Len,
    Lookup,
    Max,
    Min,
    Stem,
}

impl Func {
    pub const ALL: [Func; 11] = [
        Func::Accepted,
        Func::Concat,
        Func::Contains,
        Func::Digest,
        Func::Facts,
        Func::Join,
        Func::Len,
        Func::Lookup,
        Func::Max,
        Func::Min,
        Func::Stem,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Func::Accepted => "accepted",
            Func::Concat => "concat",
            Func::Contains => "contains",
            Func::Digest => "digest",
            Func::Facts => "facts",
            Func::Join => "join",
            Func::Len => "len",
            Func::Lookup => "lookup",
            Func::Max => "max",
            Func::Min => "min",
            Func::Stem => "stem",
        }
    }

    pub fn from_name(name: &str) -> Option<Func> {
        Func::ALL.into_iter().find(|f| f.name() == name)
    }

    /// `(low, high)` argument counts; `None` high means unbounded.
    pub fn arity(self) -> (usize, Option<usize>) {
        match self {
            Func::Concat | Func::Max | Func::Min => (1, None),
            Func::Contains | Func::Join | Func::Lookup => (2, Some(2)),
            _ => (1, Some(1)),
        }
    }
}

/// An expression. Evaluation, cloning, comparing and dropping recurse over the tree, so the parser
/// bounds its depth ([`super::parser::MAX_DEPTH`]) and how deeply it nests
/// ([`super::parser::MAX_NESTING`]): a parsed tree fits a small stack.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal(Literal),
    Name(String),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// `.*`
    Every(Box<Expr>),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
}
