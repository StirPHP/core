//! Randomized precedence check.
//!
//! Generates random operator trees over small literals, evaluates them in Rust with PHP 8
//! semantics, and prints them with the printer. Under `cargo test` this only checks that
//! generation works; with `STIR_EMIT_PHP_OUT=<dir>` it also writes `prec_fuzz.php` and
//! `prec_fuzz.expected` so the printed PHP can be run (`php prec_fuzz.php | diff - prec_fuzz.expected`)
//! to confirm that every printed expression parses back to the same tree.

mod common;

use stir_emit::ast::*;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Val {
    Int(i64),
    Bool(bool),
    Null,
}

impl Val {
    fn to_int(self) -> i64 {
        match self {
            Val::Int(i) => i,
            Val::Bool(b) => b as i64,
            Val::Null => 0,
        }
    }
    fn truthy(self) -> bool {
        match self {
            Val::Int(i) => i != 0,
            Val::Bool(b) => b,
            Val::Null => false,
        }
    }
    fn json(self) -> String {
        match self {
            Val::Int(i) => i.to_string(),
            Val::Bool(b) => b.to_string(),
            Val::Null => "null".into(),
        }
    }
}

/// PHP 8 loose comparison for our value domain: with a bool or null operand both sides are
/// compared as bools, otherwise as ints.
fn cmp(a: Val, b: Val) -> std::cmp::Ordering {
    match (a, b) {
        (Val::Int(x), Val::Int(y)) => x.cmp(&y),
        _ => a.truthy().cmp(&b.truthy()),
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const BINOPS: &[BinaryOp] = &[
    BinaryOp::Add,
    BinaryOp::Sub,
    BinaryOp::Mul,
    BinaryOp::Mod,
    BinaryOp::Pow,
    BinaryOp::BitAnd,
    BinaryOp::BitOr,
    BinaryOp::BitXor,
    BinaryOp::Shl,
    BinaryOp::Shr,
    BinaryOp::And,
    BinaryOp::Or,
    BinaryOp::LogicalAnd,
    BinaryOp::LogicalOr,
    BinaryOp::LogicalXor,
    BinaryOp::Eq,
    BinaryOp::NotEq,
    BinaryOp::Identical,
    BinaryOp::Lt,
    BinaryOp::Ge,
    BinaryOp::Spaceship,
    BinaryOp::Coalesce,
];

const LIMIT: i64 = 1 << 40;

fn gen_expr(rng: &mut Rng, depth: u32) -> Option<(Expr, Val)> {
    if depth == 0 || rng.below(5) == 0 {
        return Some(match rng.below(12) {
            0 => (Expr::bool(true), Val::Bool(true)),
            1 => (Expr::bool(false), Val::Bool(false)),
            2 => (Expr::null(), Val::Null),
            3 | 4 => {
                let i = -(rng.below(5) as i64) - 1;
                (Expr::int(i), Val::Int(i))
            }
            _ => {
                let i = rng.below(7) as i64;
                (Expr::int(i), Val::Int(i))
            }
        });
    }
    let r = rng.below(20);
    if r < 12 {
        let op = BINOPS[rng.below(BINOPS.len() as u64) as usize];
        let (l, lv) = gen_expr(rng, depth - 1)?;
        let (rr, rv) = gen_expr(rng, depth - 1)?;
        let v = eval_bin(op, lv, rv)?;
        Some((l.bin(op, rr), v))
    } else if r < 16 {
        let (e, ev) = gen_expr(rng, depth - 1)?;
        match rng.below(6) {
            0 => Some((e.not(), Val::Bool(!ev.truthy()))),
            1 => Some((e.neg(), Val::Int(-ev.to_int()))),
            2 => Some((Expr::unary(UnaryOp::Plus, e), Val::Int(ev.to_int()))),
            3 => match ev {
                Val::Int(i) => Some((Expr::unary(UnaryOp::BitNot, e), Val::Int(!i))),
                _ => None,
            },
            4 => Some((e.cast(CastKind::Int), Val::Int(ev.to_int()))),
            _ => Some((e.cast(CastKind::Bool), Val::Bool(ev.truthy()))),
        }
    } else if r < 18 {
        let (c, cv) = gen_expr(rng, depth - 1)?;
        let (a, av) = gen_expr(rng, depth - 1)?;
        let (b, bv) = gen_expr(rng, depth - 1)?;
        if rng.below(3) == 0 {
            Some((c.short_ternary(b), if cv.truthy() { cv } else { bv }))
        } else {
            Some((c.ternary(a, b), if cv.truthy() { av } else { bv }))
        }
    } else {
        let (e, ev) = gen_expr(rng, depth - 1)?;
        Some((Expr::var("t").assign(e), ev))
    }
}

fn eval_bin(op: BinaryOp, a: Val, b: Val) -> Option<Val> {
    use std::cmp::Ordering::*;
    let (x, y) = (a.to_int(), b.to_int());
    let v = match op {
        BinaryOp::Add => Val::Int(x + y),
        BinaryOp::Sub => Val::Int(x - y),
        BinaryOp::Mul => Val::Int(x.checked_mul(y)?),
        BinaryOp::Mod => {
            if y == 0 {
                return None;
            }
            Val::Int(x % y)
        }
        BinaryOp::Pow => {
            if !(0..=3).contains(&y) {
                return None;
            }
            Val::Int(x.checked_pow(y as u32)?)
        }
        BinaryOp::BitAnd => Val::Int(x & y),
        BinaryOp::BitOr => Val::Int(x | y),
        BinaryOp::BitXor => Val::Int(x ^ y),
        BinaryOp::Shl => {
            if !(0..8).contains(&y) {
                return None;
            }
            Val::Int(x << y)
        }
        BinaryOp::Shr => {
            if !(0..8).contains(&y) {
                return None;
            }
            Val::Int(x >> y)
        }
        BinaryOp::And | BinaryOp::LogicalAnd => Val::Bool(a.truthy() && b.truthy()),
        BinaryOp::Or | BinaryOp::LogicalOr => Val::Bool(a.truthy() || b.truthy()),
        BinaryOp::LogicalXor => Val::Bool(a.truthy() ^ b.truthy()),
        BinaryOp::Eq => Val::Bool(cmp(a, b) == Equal),
        BinaryOp::NotEq => Val::Bool(cmp(a, b) != Equal),
        BinaryOp::Identical => Val::Bool(a == b),
        BinaryOp::Lt => Val::Bool(cmp(a, b) == Less),
        BinaryOp::Ge => Val::Bool(cmp(a, b) != Less),
        BinaryOp::Spaceship => Val::Int(match cmp(a, b) {
            Less => -1,
            Equal => 0,
            Greater => 1,
        }),
        BinaryOp::Coalesce => {
            if a == Val::Null {
                b
            } else {
                a
            }
        }
        _ => return None,
    };
    match v {
        Val::Int(i) if i.abs() >= LIMIT => None,
        v => Some(v),
    }
}

fn generate(n: usize) -> (Vec<Stmt>, String) {
    let mut rng = Rng(0x5eed_1234);
    let mut stmts = Vec::new();
    let mut expected = String::new();
    while stmts.len() < n {
        if let Some((e, v)) = gen_expr(&mut rng, 4) {
            let call = Expr::call_fn("json_encode", vec![e.into()]);
            stmts.push(Stmt::echo(vec![call, Expr::string("\n")]));
            expected.push_str(&v.json());
            expected.push('\n');
        }
    }
    (stmts, expected)
}

#[test]
fn prec_fuzz() {
    let (stmts, expected) = generate(1500);
    assert_eq!(expected.lines().count(), 1500);
    let php = common::file(stmts);
    // Deterministic generation.
    assert_eq!(php, common::file(generate(1500).0));
    if let Ok(dir) = std::env::var("STIR_EMIT_PHP_OUT") {
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("prec_fuzz.php"), php).unwrap();
        std::fs::write(dir.join("prec_fuzz.expected"), expected).unwrap();
    }
}
