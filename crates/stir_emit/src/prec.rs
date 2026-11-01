//! PHP 8 operator precedence and associativity, table-driven.
//!
//! The levels mirror the `%left`/`%right`/`%precedence` declarations of
//! `Zend/zend_language_parser.y` (lowest first). Notable PHP 8 facts encoded here:
//! - `.` binds looser than `+`/`-` and `<<`/`>>`;
//! - `!` binds looser than `instanceof`, which binds looser than unary `-`/casts/`@`;
//! - `**` is right-associative and binds tighter than unary minus;
//! - `??` is right-associative; comparisons are non-associative;
//! - `throw`, arrow functions, `include`, `print`, `yield` are low-precedence *prefix* forms.

use crate::ast::{AssignOp, BinaryOp, ExprKind, InterpPart, UnaryOp};

/// Precedence levels, lowest to highest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Prec {
    /// `throw`, arrow function (`fn () => ...`)
    Lowest,
    /// `include`, `require`, ...
    Include,
    /// `or`
    LogicalOr,
    /// `xor`
    LogicalXor,
    /// `and`
    LogicalAnd,
    /// `print`
    Print,
    /// `yield`
    Yield,
    /// `=>` (yield key)
    DoubleArrow,
    /// `yield from`
    YieldFrom,
    /// `=`, `+=`, ... `??=`
    Assign,
    /// `? :` (non-associative in PHP 8: nested ternaries always need parentheses)
    Ternary,
    /// `??` (right)
    Coalesce,
    /// `||`
    BoolOr,
    /// `&&`
    BoolAnd,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    /// `&`
    BitAnd,
    /// `== != === !== <=>` (non-assoc)
    Equality,
    /// `< <= > >=` (non-assoc)
    Relational,
    /// `.`
    Concat,
    /// `<< >>`
    Shift,
    /// `+ -`
    Additive,
    /// `* / %`
    Multiplicative,
    /// `!`
    Not,
    /// `instanceof`
    Instanceof,
    /// `~`, unary `+`/`-`, `++`/`--`, casts, `@`
    Unary,
    /// `**` (right)
    Pow,
    /// `clone`, `new`
    CloneNew,
    /// Atoms and postfix dereferences.
    Primary,
}

impl Prec {
    /// The next-higher level (saturating).
    pub fn succ(self) -> Prec {
        use Prec::*;
        const ALL: [Prec; 29] = [
            Lowest,
            Include,
            LogicalOr,
            LogicalXor,
            LogicalAnd,
            Print,
            Yield,
            DoubleArrow,
            YieldFrom,
            Assign,
            Ternary,
            Coalesce,
            BoolOr,
            BoolAnd,
            BitOr,
            BitXor,
            BitAnd,
            Equality,
            Relational,
            Concat,
            Shift,
            Additive,
            Multiplicative,
            Not,
            Instanceof,
            Unary,
            Pow,
            CloneNew,
            Primary,
        ];
        let i = self as usize;
        ALL[(i + 1).min(ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assoc {
    Left,
    Right,
    NonAssoc,
}

/// `(token, precedence, associativity)` for every binary operator.
pub const BINARY_TABLE: &[(BinaryOp, &str, Prec, Assoc)] = &[
    (BinaryOp::LogicalOr, "or", Prec::LogicalOr, Assoc::Left),
    (BinaryOp::LogicalXor, "xor", Prec::LogicalXor, Assoc::Left),
    (BinaryOp::LogicalAnd, "and", Prec::LogicalAnd, Assoc::Left),
    (BinaryOp::Coalesce, "??", Prec::Coalesce, Assoc::Right),
    (BinaryOp::Or, "||", Prec::BoolOr, Assoc::Left),
    (BinaryOp::And, "&&", Prec::BoolAnd, Assoc::Left),
    (BinaryOp::BitOr, "|", Prec::BitOr, Assoc::Left),
    (BinaryOp::BitXor, "^", Prec::BitXor, Assoc::Left),
    (BinaryOp::BitAnd, "&", Prec::BitAnd, Assoc::Left),
    (BinaryOp::Eq, "==", Prec::Equality, Assoc::NonAssoc),
    (BinaryOp::NotEq, "!=", Prec::Equality, Assoc::NonAssoc),
    (BinaryOp::Identical, "===", Prec::Equality, Assoc::NonAssoc),
    (
        BinaryOp::NotIdentical,
        "!==",
        Prec::Equality,
        Assoc::NonAssoc,
    ),
    (BinaryOp::Spaceship, "<=>", Prec::Equality, Assoc::NonAssoc),
    (BinaryOp::Lt, "<", Prec::Relational, Assoc::NonAssoc),
    (BinaryOp::Le, "<=", Prec::Relational, Assoc::NonAssoc),
    (BinaryOp::Gt, ">", Prec::Relational, Assoc::NonAssoc),
    (BinaryOp::Ge, ">=", Prec::Relational, Assoc::NonAssoc),
    (BinaryOp::Concat, ".", Prec::Concat, Assoc::Left),
    (BinaryOp::Shl, "<<", Prec::Shift, Assoc::Left),
    (BinaryOp::Shr, ">>", Prec::Shift, Assoc::Left),
    (BinaryOp::Add, "+", Prec::Additive, Assoc::Left),
    (BinaryOp::Sub, "-", Prec::Additive, Assoc::Left),
    (BinaryOp::Mul, "*", Prec::Multiplicative, Assoc::Left),
    (BinaryOp::Div, "/", Prec::Multiplicative, Assoc::Left),
    (BinaryOp::Mod, "%", Prec::Multiplicative, Assoc::Left),
    (BinaryOp::Pow, "**", Prec::Pow, Assoc::Right),
];

fn binary_entry(op: BinaryOp) -> (&'static str, Prec, Assoc) {
    let (_, tok, prec, assoc) = BINARY_TABLE
        .iter()
        .find(|(o, ..)| *o == op)
        .expect("every binary operator is in BINARY_TABLE");
    (tok, *prec, *assoc)
}

impl BinaryOp {
    pub fn token(self) -> &'static str {
        binary_entry(self).0
    }
    pub fn prec(self) -> Prec {
        binary_entry(self).1
    }
    pub fn assoc(self) -> Assoc {
        binary_entry(self).2
    }
    /// Minimum precedence of the `(lhs, rhs)` operands that may appear without parentheses.
    pub fn operand_min(self) -> (Prec, Prec) {
        let p = self.prec();
        match self.assoc() {
            Assoc::Left => (p, p.succ()),
            Assoc::Right => (p.succ(), p),
            Assoc::NonAssoc => (p.succ(), p.succ()),
        }
    }
}

impl AssignOp {
    pub fn token(self) -> &'static str {
        match self {
            AssignOp::Add => "+=",
            AssignOp::Sub => "-=",
            AssignOp::Mul => "*=",
            AssignOp::Div => "/=",
            AssignOp::Mod => "%=",
            AssignOp::Pow => "**=",
            AssignOp::Concat => ".=",
            AssignOp::BitAnd => "&=",
            AssignOp::BitOr => "|=",
            AssignOp::BitXor => "^=",
            AssignOp::Shl => "<<=",
            AssignOp::Shr => ">>=",
            AssignOp::Coalesce => "??=",
        }
    }
}

impl UnaryOp {
    pub fn token(self) -> &'static str {
        match self {
            UnaryOp::Not => "!",
            UnaryOp::Neg => "-",
            UnaryOp::Plus => "+",
            UnaryOp::BitNot => "~",
            UnaryOp::PreInc | UnaryOp::PostInc => "++",
            UnaryOp::PreDec | UnaryOp::PostDec => "--",
        }
    }
    pub fn is_postfix(self) -> bool {
        matches!(self, UnaryOp::PostInc | UnaryOp::PostDec)
    }
    pub fn prec(self) -> Prec {
        match self {
            UnaryOp::Not => Prec::Not,
            _ => Prec::Unary,
        }
    }
}

/// Whether an interpolated string can be printed as `"...{$expr}..."` (every expression part
/// must start with a variable); otherwise it is printed as a concatenation.
pub fn interp_is_simple(parts: &[InterpPart]) -> bool {
    parts.iter().all(|p| match p {
        InterpPart::Lit(_) => true,
        InterpPart::Expr(e) => starts_with_variable(&e.kind),
    })
}

fn starts_with_variable(e: &ExprKind) -> bool {
    match e {
        ExprKind::Var(_) => true,
        ExprKind::PropFetch { object, .. }
        | ExprKind::MethodCall { object, .. }
        | ExprKind::Index { base: object, .. } => starts_with_variable(&object.kind),
        ExprKind::Call { callee, .. } => starts_with_variable(&callee.kind),
        _ => false,
    }
}

/// The precedence of an expression as printed (without added parentheses).
pub fn expr_prec(e: &ExprKind) -> Prec {
    match e {
        ExprKind::Int(i) if *i < 0 && *i != i64::MIN => Prec::Unary,
        ExprKind::Float(f) if f.is_sign_negative() && !f.is_nan() => Prec::Unary,
        // Printed as a concatenation chain (`'a' . f() . 'b'`).
        ExprKind::Interp(parts) if !interp_is_simple(parts) => Prec::Concat,
        ExprKind::Unary { op, .. } => op.prec(),
        ExprKind::Binary { op, .. } => op.prec(),
        ExprKind::Assign { .. } | ExprKind::AssignRef { .. } | ExprKind::AssignOp { .. } => {
            Prec::Assign
        }
        ExprKind::Ternary { .. } => Prec::Ternary,
        ExprKind::Instanceof { .. } => Prec::Instanceof,
        ExprKind::Cast { .. } | ExprKind::Silence(_) => Prec::Unary,
        ExprKind::Clone(_) | ExprKind::New { .. } | ExprKind::NewAnonClass { .. } => Prec::CloneNew,
        ExprKind::ArrowFn(_) | ExprKind::Throw(_) => Prec::Lowest,
        ExprKind::Yield { .. } => Prec::Yield,
        ExprKind::YieldFrom(_) => Prec::YieldFrom,
        ExprKind::Print(_) => Prec::Print,
        ExprKind::Include { .. } => Prec::Include,
        _ => Prec::Primary,
    }
}

/// Prefix forms whose operand extends to the right (`throw x`, `print x`, `-x`, `(int) x`,
/// `fn () => x`, ...). Such an expression in the *rightmost* position of its context can never
/// capture a following operator, so it needs no parentheses there even when its precedence is
/// lower than the surrounding operator's (e.g. `$a ?? throw new E()`).
pub fn is_prefix_form(e: &ExprKind) -> bool {
    match e {
        ExprKind::Unary { op, .. } => !op.is_postfix(),
        ExprKind::Int(i) => *i < 0,
        ExprKind::Float(f) => f.is_sign_negative(),
        ExprKind::Cast { .. }
        | ExprKind::Silence(_)
        | ExprKind::Throw(_)
        | ExprKind::ArrowFn(_)
        | ExprKind::Yield { .. }
        | ExprKind::YieldFrom(_)
        | ExprKind::Print(_)
        | ExprKind::Include { .. }
        | ExprKind::Clone(_) => true,
        _ => false,
    }
}

/// Whether `e` can be followed by `->`, `?->` or `[...]` without parentheses.
///
/// `new` is deliberately excluded (`(new Foo())->bar()` is required before PHP 8.4).
pub fn is_dereferencable(e: &ExprKind) -> bool {
    match e {
        ExprKind::Var(_)
        | ExprKind::VarVar(_)
        | ExprKind::Paren(_)
        | ExprKind::Raw(_)
        | ExprKind::Call { .. }
        | ExprKind::MethodCall { .. }
        | ExprKind::StaticCall { .. }
        | ExprKind::PropFetch { .. }
        | ExprKind::StaticProp { .. }
        | ExprKind::ClassConst { .. }
        | ExprKind::Index { .. }
        | ExprKind::Name(_)
        | ExprKind::MagicConst(_)
        | ExprKind::Str(_)
        | ExprKind::Array(_) => true,
        ExprKind::Interp(parts) => interp_is_simple(parts),
        _ => false,
    }
}

/// Whether `e` can be the callee of `e(...)` without parentheses (and still mean a call of the
/// value of `e`: `$a->b()` would be a method call, so a property fetch needs parentheses).
pub fn is_callable_base(e: &ExprKind) -> bool {
    match e {
        ExprKind::PropFetch { .. } | ExprKind::StaticProp { .. } | ExprKind::ClassConst { .. } => {
            false
        }
        ExprKind::MagicConst(_) => false,
        _ => is_dereferencable(e),
    }
}

/// Whether `e` can be the class part of `e::x` without parentheses.
pub fn is_static_base(e: &ExprKind) -> bool {
    match e {
        // `FOO::x` would name class FOO.
        ExprKind::Name(_) | ExprKind::MagicConst(_) => false,
        _ => is_dereferencable(e),
    }
}

/// Whether `e` can be the class of `new e` / `instanceof e` without parentheses
/// (PHP's `new_variable`: variables with `[...]`, `->prop`, `::$prop`, but no calls).
pub fn is_new_variable(e: &ExprKind) -> bool {
    use crate::ast::ClassRef;
    match e {
        ExprKind::Var(_) | ExprKind::VarVar(_) => true,
        ExprKind::Index {
            base,
            index: Some(_),
        } => is_new_variable(&base.kind),
        ExprKind::PropFetch { object, .. } => is_new_variable(&object.kind),
        ExprKind::StaticProp { class, .. } => match class {
            ClassRef::Expr(e) => is_new_variable(&e.kind),
            _ => true,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_every_operator_once() {
        use BinaryOp::*;
        let all = [
            Add,
            Sub,
            Mul,
            Div,
            Mod,
            Pow,
            Concat,
            BitAnd,
            BitOr,
            BitXor,
            Shl,
            Shr,
            And,
            Or,
            LogicalAnd,
            LogicalOr,
            LogicalXor,
            Eq,
            NotEq,
            Identical,
            NotIdentical,
            Lt,
            Le,
            Gt,
            Ge,
            Spaceship,
            Coalesce,
        ];
        assert_eq!(all.len(), BINARY_TABLE.len());
        for op in all {
            assert_eq!(
                BINARY_TABLE.iter().filter(|e| e.0 == op).count(),
                1,
                "{op:?}"
            );
        }
    }

    #[test]
    fn php8_ordering_facts() {
        use BinaryOp::*;
        assert!(Concat.prec() < Add.prec());
        assert!(Concat.prec() < Shl.prec());
        assert!(Shl.prec() < Add.prec());
        assert!(Add.prec() < Mul.prec());
        assert!(Mul.prec() < Prec::Not);
        assert!(Prec::Not < Prec::Instanceof);
        assert!(Prec::Instanceof < Prec::Unary);
        assert!(Prec::Unary < Pow.prec());
        assert!(Coalesce.prec() < Or.prec());
        assert!(Prec::Ternary < Coalesce.prec());
        assert!(Prec::Assign < Prec::Ternary);
        assert!(LogicalAnd.prec() < Prec::Assign);
        assert!(LogicalOr.prec() < LogicalXor.prec());
        assert!(LogicalXor.prec() < LogicalAnd.prec());
        assert!(Eq.prec() < Lt.prec());
        assert!(BitOr.prec() < BitXor.prec() && BitXor.prec() < BitAnd.prec());
        assert!(BitAnd.prec() < Eq.prec());
        assert!(And.prec() < BitOr.prec());
        assert_eq!(Coalesce.assoc(), Assoc::Right);
        assert_eq!(Pow.assoc(), Assoc::Right);
        assert_eq!(Eq.assoc(), Assoc::NonAssoc);
        assert_eq!(Lt.assoc(), Assoc::NonAssoc);
        assert_eq!(Sub.assoc(), Assoc::Left);
    }

    #[test]
    fn operand_min_by_assoc() {
        assert_eq!(
            BinaryOp::Sub.operand_min(),
            (Prec::Additive, Prec::Multiplicative)
        );
        assert_eq!(BinaryOp::Pow.operand_min(), (Prec::CloneNew, Prec::Pow));
        assert_eq!(
            BinaryOp::Coalesce.operand_min(),
            (Prec::BoolOr, Prec::Coalesce)
        );
        assert_eq!(
            BinaryOp::Eq.operand_min(),
            (Prec::Relational, Prec::Relational)
        );
    }

    #[test]
    fn succ_saturates() {
        assert_eq!(Prec::Primary.succ(), Prec::Primary);
        assert_eq!(Prec::Lowest.succ(), Prec::Include);
        assert_eq!(Prec::Pow.succ(), Prec::CloneNew);
    }
}
