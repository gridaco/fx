//! The Pratt parser. `parse` parses the whole source and requires the `End` token after it.
//! Number literals follow spec/identity.md §1: an integer literal must be the canonical form of
//! the number it reads as ([`integer_literal`] says why one is not), and a decimal literal must be
//! finite.
//!
//! Binding powers are [`BinaryOp::power`]; `??` is right-associative, every other operator binds
//! left. Prefix `!` and `-` parse their operand at power 80 and take no postfix themselves, so
//! `-a.b` is `-(a.b)`. Postfix `.name`, `.*` and `[expr]` bind tightest; after `.`, a keyword is a
//! field name. Calls are allowed only on a bare function name.
//!
//! Size limits. Parsing, evaluation, cloning and dropping all recurse over the tree, so the parser
//! bounds it and refuses an expression past a limit instead of letting a deep one overflow the
//! stack:
//! - nesting ([`MAX_NESTING`]): parentheses, prefix operands, call arguments, indexes and the
//!   right operand of `??`, which is where the parser itself recurses;
//! - depth ([`MAX_DEPTH`]): the longest path from the root to a leaf, which a long chain of
//!   left-associative operators (`1 + 1 + …`) or members (`a.b.c…`) makes deep without nesting.
//!   The chain itself is parsed by a loop;
//! - size ([`MAX_NODES`]): the number of nodes.

use super::ast::{BinaryOp, Func, Literal, UnaryOp};
use super::lexer::{Token, TokenKind, lex};
use super::{Expr, ExprError};
use crate::text::py_repr_str;
use crate::value::{as_f64, integer_literal};

/// The power at which a prefix operator parses its operand.
const PREFIX_POWER: u8 = 80;

/// How many levels an expression may nest (parentheses, prefix operands, call arguments, indexes,
/// `??` operands): how deep the parser recurses.
pub const MAX_NESTING: usize = 256;

/// The deepest tree an expression may make: a leaf has depth 1, a node one more than its deepest
/// child, so `1 + 1 + 1` and `-(-1)` have depth 3. Evaluation recurses this deep; in a debug build
/// one level of a binary operator takes about 7 KB of stack, so 128 levels fit a 2 MB thread with
/// room to spare.
pub const MAX_DEPTH: usize = 128;

/// The most nodes an expression may have.
pub const MAX_NODES: usize = 10_000;

/// Parses an expression's source text (already stripped).
pub fn parse(source: &str) -> Result<Expr, ExprError> {
    let mut parser = Parser {
        source,
        tokens: lex(source)?,
        position: 0,
        nesting: 0,
        nodes: 0,
    };
    let parsed = parser.expression(0)?;
    let next = parser.peek();
    if next.kind != TokenKind::End {
        return Err(parser.unexpected(next));
    }
    Ok(parsed.expr)
}

struct Parser<'s> {
    source: &'s str,
    tokens: Vec<Token>,
    position: usize,
    /// How many [`Parser::expression`] calls are open.
    nesting: usize,
    /// How many nodes have been made.
    nodes: usize,
}

/// A parsed subtree and its depth.
struct Parsed {
    expr: Expr,
    depth: usize,
}

fn binary_op(token: &Token) -> Option<BinaryOp> {
    if token.kind != TokenKind::Op {
        return None;
    }
    Some(match token.text.as_str() {
        "??" => BinaryOp::Coalesce,
        "||" => BinaryOp::Or,
        "&&" => BinaryOp::And,
        "==" => BinaryOp::Eq,
        "!=" => BinaryOp::Ne,
        "<" => BinaryOp::Lt,
        "<=" => BinaryOp::Le,
        ">" => BinaryOp::Gt,
        ">=" => BinaryOp::Ge,
        "+" => BinaryOp::Add,
        "-" => BinaryOp::Sub,
        "*" => BinaryOp::Mul,
        "/" => BinaryOp::Div,
        _ => return None,
    })
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        // The token list always ends with `End`; nothing reads past it.
        &self.tokens[self.position.min(self.tokens.len() - 1)]
    }

    fn take(&mut self) -> Token {
        let token = self.peek().clone();
        self.position += 1;
        token
    }

    fn is_op(&self, text: &str) -> bool {
        let token = self.peek();
        token.kind == TokenKind::Op && token.text == text
    }

    fn unexpected(&self, token: &Token) -> ExprError {
        ExprError::new(format!(
            "unexpected {} at {} in {}",
            py_repr_str(&token.text),
            token.at,
            py_repr_str(self.source)
        ))
    }

    fn expect(&mut self, text: &str) -> Result<(), ExprError> {
        let token = self.take();
        if token.kind != TokenKind::Op || token.text != text {
            return Err(ExprError::new(format!(
                "expected {} at {} in {}",
                py_repr_str(text),
                token.at,
                py_repr_str(self.source)
            )));
        }
        Ok(())
    }

    /// Makes a node whose deepest child has depth `below`, refusing a tree past the limits.
    fn node(&mut self, expr: Expr, below: usize) -> Result<Parsed, ExprError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(ExprError::new(format!(
                "the expression is too long: it has more than {MAX_NODES} parts"
            )));
        }
        let depth = below + 1;
        if depth > MAX_DEPTH {
            return Err(ExprError::new(format!(
                "the expression is nested too deeply: more than {MAX_DEPTH} operations inside \
                 one another (each operator of a chain like `a + b + c` holds the one before it); \
                 split it"
            )));
        }
        Ok(Parsed { expr, depth })
    }

    fn expression(&mut self, power: u8) -> Result<Parsed, ExprError> {
        if self.nesting >= MAX_NESTING {
            return Err(ExprError::new(format!(
                "the expression is nested too deeply: more than {MAX_NESTING} levels of \
                 parentheses, operators and calls"
            )));
        }
        self.nesting += 1;
        let parsed = self.operators(power);
        self.nesting -= 1;
        parsed
    }

    /// The operand at the start, then every binary operator stronger than `power`. A chain of
    /// left-associative operators is folded by this loop, not by recursion.
    fn operators(&mut self, power: u8) -> Result<Parsed, ExprError> {
        let mut left = self.prefix()?;
        loop {
            let Some(op) = binary_op(self.peek()) else {
                return Ok(left);
            };
            let strength = op.power();
            if strength <= power {
                return Ok(left);
            }
            self.take();
            let right = if op == BinaryOp::Coalesce {
                self.expression(strength - 1)?
            } else {
                self.expression(strength)?
            };
            let below = left.depth.max(right.depth);
            left = self.node(
                Expr::Binary(op, Box::new(left.expr), Box::new(right.expr)),
                below,
            )?;
        }
    }

    fn prefix(&mut self) -> Result<Parsed, ExprError> {
        let token = self.take();
        let node = match token.kind {
            TokenKind::Number => {
                let number = self.number(&token.text)?;
                self.node(Expr::Literal(Literal::Number(number)), 0)?
            }
            TokenKind::Str => self.node(
                Expr::Literal(Literal::Str(super::lexer::unquote(&token.text))),
                0,
            )?,
            TokenKind::Name => match token.text.as_str() {
                "true" => self.node(Expr::Literal(Literal::Bool(true)), 0)?,
                "false" => self.node(Expr::Literal(Literal::Bool(false)), 0)?,
                "null" => self.node(Expr::Literal(Literal::Null), 0)?,
                name if self.is_op("(") => self.call(name, &token)?,
                name => self.node(Expr::Name(name.to_string()), 0)?,
            },
            TokenKind::Op if token.text == "(" => {
                let inner = self.expression(0)?;
                self.expect(")")?;
                inner
            }
            TokenKind::Op if token.text == "!" || token.text == "-" => {
                let op = if token.text == "!" {
                    UnaryOp::Not
                } else {
                    UnaryOp::Neg
                };
                let operand = self.expression(PREFIX_POWER)?;
                return self.node(Expr::Unary(op, Box::new(operand.expr)), operand.depth);
            }
            _ => return Err(self.unexpected(&token)),
        };
        self.postfix(node)
    }

    /// A number literal (spec/identity.md §1): an integer literal must read back exactly; a
    /// decimal literal must be finite.
    fn number(&self, text: &str) -> Result<f64, ExprError> {
        if !text.contains('.') {
            let value = integer_literal(text).map_err(|e| ExprError::new(e.message))?;
            return Ok(value.as_number().map(as_f64).unwrap_or(0.0));
        }
        match text.parse::<f64>() {
            Ok(x) if x.is_finite() => Ok(x),
            _ => Err(ExprError::new(format!(
                "the number {} is too large",
                shorten(text)
            ))),
        }
    }

    fn call(&mut self, name: &str, token: &Token) -> Result<Parsed, ExprError> {
        let Some(func) = Func::from_name(name) else {
            let mut names: Vec<&str> = Func::ALL.iter().map(|f| f.name()).collect();
            names.sort_unstable();
            return Err(ExprError::new(format!(
                "{}() is not an expression function; write a node for it (functions: {})",
                token.text,
                names.join(", ")
            )));
        };
        self.expect("(")?;
        let mut args = Vec::new();
        let mut below = 0;
        if !self.is_op(")") {
            loop {
                let arg = self.expression(0)?;
                below = below.max(arg.depth);
                args.push(arg.expr);
                if !self.is_op(",") {
                    break;
                }
                self.take();
            }
        }
        self.expect(")")?;
        self.node(Expr::Call(func, args), below)
    }

    /// Every `.name`, `.*` and `[index]` after `node`, folded by a loop.
    fn postfix(&mut self, mut node: Parsed) -> Result<Parsed, ExprError> {
        loop {
            if self.is_op(".") {
                self.take();
                let following = self.take();
                let target = Box::new(node.expr);
                node = if following.kind == TokenKind::Op && following.text == "*" {
                    self.node(Expr::Every(target), node.depth)?
                } else if following.kind == TokenKind::Name {
                    self.node(Expr::Field(target, following.text), node.depth)?
                } else {
                    return Err(ExprError::new(format!(
                        "expected a field name at {} in {}",
                        following.at,
                        py_repr_str(self.source)
                    )));
                };
            } else if self.is_op("[") {
                self.take();
                let index = self.expression(0)?;
                self.expect("]")?;
                let below = node.depth.max(index.depth);
                node = self.node(
                    Expr::Index(Box::new(node.expr), Box::new(index.expr)),
                    below,
                )?;
            } else {
                return Ok(node);
            }
        }
    }
}

fn shorten(text: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(40).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{Scope, evaluate, template};
    use crate::val::{Val, ViewId};

    fn num(x: f64) -> Expr {
        Expr::Literal(Literal::Number(x))
    }

    fn name(n: &str) -> Expr {
        Expr::Name(n.into())
    }

    fn bin(op: BinaryOp, l: Expr, r: Expr) -> Expr {
        Expr::Binary(op, Box::new(l), Box::new(r))
    }

    #[test]
    fn precedence_and_associativity() {
        use BinaryOp::*;
        assert_eq!(
            parse("1 + 2 * 3").unwrap(),
            bin(Add, num(1.0), bin(Mul, num(2.0), num(3.0)))
        );
        assert_eq!(
            parse("a ?? b ?? c").unwrap(),
            bin(Coalesce, name("a"), bin(Coalesce, name("b"), name("c")))
        );
        assert_eq!(
            parse("a - b - c").unwrap(),
            bin(Sub, bin(Sub, name("a"), name("b")), name("c"))
        );
        assert_eq!(
            parse("-a.b").unwrap(),
            Expr::Unary(
                UnaryOp::Neg,
                Box::new(Expr::Field(Box::new(name("a")), "b".into()))
            )
        );
    }

    #[test]
    fn keywords_after_a_dot_are_fields() {
        assert_eq!(
            parse("x.true").unwrap(),
            Expr::Field(Box::new(name("x")), "true".into())
        );
        assert_eq!(parse("True").unwrap(), name("True"));
    }

    #[test]
    fn literals() {
        assert_eq!(parse("1.50").unwrap(), num(1.5));
        assert_eq!(parse("9007199254740992").unwrap(), num(9007199254740992.0));
        assert!(parse("9007199254740993").is_err());
        assert!(parse(&format!("{}.0", "9".repeat(400))).is_err());
        // An integer literal is either beyond the exact integers or not written canonically.
        assert!(
            parse("9007199254740993")
                .unwrap_err()
                .0
                .starts_with("9007199254740993 is beyond the integers a number holds exactly")
        );
        assert_eq!(
            parse("007").unwrap_err().0,
            "007 is not written in canonical form; write 7"
        );
        assert_eq!(
            parse("00").unwrap_err().0,
            "00 is not written in canonical form; write 0"
        );
    }

    // ------------------------------------------------------------------------------ size limits

    /// A scope where `x` is a list holding a list, 600 levels deep.
    struct Deep;

    impl Scope for Deep {
        fn root(&mut self, name: &str) -> Result<Val, ExprError> {
            if name != "x" {
                return Err(ExprError::new(format!("unknown name {name}")));
            }
            let mut value = Val::List(Vec::new());
            for _ in 0..600 {
                value = Val::List(vec![value]);
            }
            Ok(value)
        }

        fn view_member(&mut self, _: ViewId, _: &str) -> Result<Val, ExprError> {
            Err(ExprError::names_a_step())
        }

        fn view_item(&mut self, _: ViewId, _: &Val) -> Result<Val, ExprError> {
            Err(ExprError::names_a_step())
        }

        fn view_every(&mut self, _: ViewId) -> Result<Val, ExprError> {
            Err(ExprError::names_a_step())
        }

        fn view_len(&mut self, _: ViewId) -> Result<usize, ExprError> {
            Err(ExprError::names_a_step())
        }

        fn finish_view(&mut self, _: ViewId) -> Result<Val, ExprError> {
            Err(ExprError::names_a_step())
        }

        fn facts(&mut self, _: &Val) -> Result<Val, ExprError> {
            Err(ExprError::new("no facts"))
        }
    }

    /// Runs `work` on a thread with the 2 MB stack Rust gives a spawned thread by default, so a
    /// test proves the limits hold on it whatever `RUST_MIN_STACK` says.
    fn on_small_stack(work: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(work)
            .unwrap()
            .join()
            .unwrap();
    }

    /// Parses `source` as a whole template, evaluates it when it parses, and clones and drops
    /// the tree: everything that recurses over it.
    fn run(source: &str) -> Result<Val, ExprError> {
        let parsed = template(&format!("${{{{ {source} }}}}"))?.expect("a template");
        let whole = parsed.whole().expect("one expression").clone();
        let value = evaluate(&whole, &mut Deep);
        drop(whole);
        drop(parsed);
        value
    }

    fn refused(source: &str) -> String {
        match run(source) {
            Ok(value) => panic!("accepted as {value:?}"),
            Err(error) => error.0,
        }
    }

    const NESTED: &str = "the expression is nested too deeply: more than 256 levels of \
                          parentheses, operators and calls";
    const DEEP: &str = "the expression is nested too deeply: more than 128 operations inside one \
                        another (each operator of a chain like `a + b + c` holds the one before \
                        it); split it";
    const LONG: &str = "the expression is too long: it has more than 10000 parts";

    #[test]
    fn twenty_thousand_terms_are_refused_not_a_crash() {
        on_small_stack(|| {
            let terms = vec!["1"; 20_000];
            for op in [" + ", " - ", " * ", " / ", " && ", " || ", " == ", " < "] {
                assert_eq!(refused(&terms.join(op)), DEEP, "{op}");
            }
            // `??` is right-associative: its chain nests in the parser.
            assert_eq!(refused(&terms.join(" ?? ")), NESTED);
            assert_eq!(refused(&format!("x{}", ".a".repeat(20_000))), DEEP);
            assert_eq!(refused(&format!("x{}", "[0]".repeat(20_000))), DEEP);
            assert_eq!(refused(&format!("x{}", ".*".repeat(20_000))), DEEP);
            assert_eq!(refused(&format!("concat({})", terms.join(", "))), LONG);
        });
    }

    #[test]
    fn twenty_thousand_parentheses_are_refused_not_a_crash() {
        on_small_stack(|| {
            let n = 20_000;
            let nested =
                |open: &str, close: &str| format!("{}1{}", open.repeat(n), close.repeat(n));
            assert_eq!(refused(&nested("(", ")")), NESTED);
            assert_eq!(refused(&nested("(1 + ", ")")), NESTED);
            assert_eq!(refused(&nested("len(", ")")), NESTED);
            assert_eq!(refused(&nested("x[", "]")), NESTED);
            assert_eq!(refused(&format!("{}1", "-".repeat(n))), NESTED);
            assert_eq!(refused(&format!("{}1", "!".repeat(n))), NESTED);
            // Unbalanced or unfinished: refused by the limit before the end is reached.
            assert_eq!(refused(&"(".repeat(n)), NESTED);
            assert_eq!(refused(&"-".repeat(n)), NESTED);
        });
    }

    #[test]
    fn expressions_at_the_limits_evaluate_on_a_small_stack() {
        on_small_stack(|| {
            // 256 levels: the whole expression, 254 parentheses, and the right operands of a
            // chain 128 deep inside them.
            let terms = vec!["1"; 128].join(" + ");
            let wrapped = format!("{}{terms}{}", "(".repeat(254), ")".repeat(254));
            assert!(matches!(run(&wrapped), Ok(Val::Number(x)) if x == 128.0));
            assert_eq!(refused(&format!("({wrapped})")), NESTED);
            assert_eq!(refused(&format!("{terms} + 1")), DEEP);

            // A depth of 128: 127 prefix operators, or 127 indexes after `x`.
            let negated = format!("{}1", "-".repeat(127));
            assert!(matches!(run(&negated), Ok(Val::Number(x)) if x == -1.0));
            assert_eq!(refused(&format!("-{negated}")), DEEP);
            let indexes = format!("x{}", "[0]".repeat(127));
            assert!(matches!(run(&indexes), Ok(Val::List(_))));
            assert_eq!(refused(&format!("{indexes}[0]")), DEEP);

            // Mixed: 63 levels of `-(… + 1)` make a depth of 127.
            let mut source = String::from("1");
            for _ in 0..63 {
                source = format!("-({source} + 1)");
            }
            assert!(run(&source).is_ok());
            assert_eq!(refused(&format!("-(-{source})")), DEEP);
        });
    }

    #[test]
    fn ten_thousand_nodes_at_most() {
        on_small_stack(|| {
            // A call and its arguments: 9,999 arguments make 10,000 nodes.
            let args = |n: usize| vec!["'a'"; n].join(", ");
            let joined = run(&format!("concat({})", args(9_999))).unwrap();
            assert!(matches!(joined, Val::Str(s) if s.len() == 9_999));
            assert_eq!(refused(&format!("concat({})", args(10_000))), LONG);
        });
    }
}
