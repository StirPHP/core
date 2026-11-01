//! Parenthesization tests: trees deliberately nested against PHP precedence.

mod common;

use common::*;
use stir_emit::ast::*;
use stir_emit::print::print_expr;

use BinaryOp::*;

fn p(e: &Expr) -> String {
    print_expr(e, &opts())
}

#[test]
fn arithmetic_and_assoc() {
    let cases: Vec<(&str, Expr, &str)> = vec![
        (
            "mul of add",
            bin(Mul, bin(Add, v("a"), v("b")), v("c")),
            "($a + $b) * $c",
        ),
        (
            "add of mul",
            bin(Add, v("a"), bin(Mul, v("b"), v("c"))),
            "$a + $b * $c",
        ),
        (
            "left assoc sub",
            bin(Sub, bin(Sub, v("a"), v("b")), v("c")),
            "$a - $b - $c",
        ),
        (
            "right nested sub",
            bin(Sub, v("a"), bin(Sub, v("b"), v("c"))),
            "$a - ($b - $c)",
        ),
        (
            "div right nested",
            bin(Div, v("a"), bin(Mul, v("b"), v("c"))),
            "$a / ($b * $c)",
        ),
        (
            "mod left",
            bin(Mod, bin(Mul, v("a"), v("b")), v("c")),
            "$a * $b % $c",
        ),
        (
            "pow right assoc",
            bin(Pow, v("a"), bin(Pow, v("b"), v("c"))),
            "$a ** $b ** $c",
        ),
        (
            "pow left nested",
            bin(Pow, bin(Pow, v("a"), v("b")), v("c")),
            "($a ** $b) ** $c",
        ),
        (
            "neg of pow",
            un(UnaryOp::Neg, bin(Pow, v("a"), int(2))),
            "-$a ** 2",
        ),
        (
            "pow of neg",
            bin(Pow, un(UnaryOp::Neg, v("a")), int(2)),
            "(-$a) ** 2",
        ),
        ("pow of neg literal", bin(Pow, int(-2), int(2)), "(-2) ** 2"),
        (
            "pow rhs neg",
            bin(Pow, int(2), un(UnaryOp::Neg, v("a"))),
            "2 ** -$a",
        ),
        (
            "pow rhs neg not rightmost",
            bin(Mul, bin(Pow, int(2), un(UnaryOp::Neg, v("a"))), v("b")),
            "2 ** (-$a) * $b",
        ),
        (
            "shift vs add",
            bin(Shl, bin(Add, v("a"), v("b")), v("c")),
            "$a + $b << $c",
        ),
        (
            "add of shift",
            bin(Add, bin(Shl, v("a"), v("b")), v("c")),
            "($a << $b) + $c",
        ),
        (
            "bit ops",
            bin(BitAnd, bin(BitOr, v("a"), v("b")), v("c")),
            "($a | $b) & $c",
        ),
        (
            "bit ops 2",
            bin(BitOr, bin(BitAnd, v("a"), v("b")), v("c")),
            "$a & $b | $c",
        ),
        (
            "xor between",
            bin(
                BitXor,
                bin(BitOr, v("a"), v("b")),
                bin(BitAnd, v("c"), v("d")),
            ),
            "($a | $b) ^ $c & $d",
        ),
    ];
    check(cases);
}

#[test]
fn concat_php8() {
    let cases: Vec<(&str, Expr, &str)> = vec![
        (
            "concat of add",
            bin(Concat, s("x"), bin(Add, v("a"), v("b"))),
            "'x' . $a + $b",
        ),
        (
            "add of concat",
            bin(Add, bin(Concat, s("x"), v("a")), v("b")),
            "('x' . $a) + $b",
        ),
        (
            "concat of shift",
            bin(Concat, v("a"), bin(Shl, v("b"), int(1))),
            "$a . $b << 1",
        ),
        (
            "shift of concat",
            bin(Shr, bin(Concat, v("a"), v("b")), int(1)),
            "($a . $b) >> 1",
        ),
        (
            "concat chain",
            bin(Concat, bin(Concat, v("a"), v("b")), v("c")),
            "$a . $b . $c",
        ),
        (
            "concat right nested",
            bin(Concat, v("a"), bin(Concat, v("b"), v("c"))),
            "$a . ($b . $c)",
        ),
        (
            "concat of lt",
            bin(Lt, bin(Concat, v("a"), v("b")), v("c")),
            "$a . $b < $c",
        ),
        (
            "lt of concat rhs",
            bin(Concat, v("a"), bin(Lt, v("b"), v("c"))),
            "$a . ($b < $c)",
        ),
        (
            "concat float literals",
            bin(Concat, Expr::float(1.0), int(2)),
            "1.0 . 2",
        ),
    ];
    check(cases);
}

#[test]
fn comparisons_nonassoc() {
    let cases: Vec<(&str, Expr, &str)> = vec![
        (
            "eq of eq",
            bin(Eq, bin(Eq, v("a"), v("b")), v("c")),
            "($a == $b) == $c",
        ),
        (
            "eq rhs eq",
            bin(Identical, v("a"), bin(NotIdentical, v("b"), v("c"))),
            "$a === ($b !== $c)",
        ),
        (
            "lt of lt",
            bin(Lt, bin(Lt, v("a"), v("b")), v("c")),
            "($a < $b) < $c",
        ),
        (
            "eq of lt",
            bin(Eq, bin(Lt, v("a"), v("b")), v("c")),
            "$a < $b == $c",
        ),
        (
            "lt of eq",
            bin(Lt, bin(Eq, v("a"), v("b")), v("c")),
            "($a == $b) < $c",
        ),
        (
            "spaceship of add",
            bin(Spaceship, bin(Add, v("a"), int(1)), v("b")),
            "$a + 1 <=> $b",
        ),
        (
            "and of eq",
            bin(And, bin(Eq, v("a"), int(1)), bin(Gt, v("b"), int(2))),
            "$a == 1 && $b > 2",
        ),
        (
            "eq of and",
            bin(Eq, bin(And, v("a"), v("b")), v("c")),
            "($a && $b) == $c",
        ),
        (
            "or of and",
            bin(Or, bin(And, v("a"), v("b")), v("c")),
            "$a && $b || $c",
        ),
        (
            "and of or",
            bin(And, bin(Or, v("a"), v("b")), v("c")),
            "($a || $b) && $c",
        ),
        (
            "bitand of eq",
            bin(BitAnd, v("a"), bin(Eq, v("b"), v("c"))),
            "$a & $b == $c",
        ),
        (
            "eq of bitand",
            bin(Eq, bin(BitAnd, v("a"), v("b")), v("c")),
            "($a & $b) == $c",
        ),
    ];
    check(cases);
}

#[test]
fn logical_keywords() {
    let cases: Vec<(&str, Expr, &str)> = vec![
        (
            "or of and kw",
            bin(LogicalOr, bin(LogicalAnd, v("a"), v("b")), v("c")),
            "$a and $b or $c",
        ),
        (
            "and kw of or kw",
            bin(LogicalAnd, bin(LogicalOr, v("a"), v("b")), v("c")),
            "($a or $b) and $c",
        ),
        (
            "xor between",
            bin(LogicalXor, bin(LogicalOr, v("a"), v("b")), v("c")),
            "($a or $b) xor $c",
        ),
        (
            "assign in and kw",
            bin(LogicalAnd, v("a").assign(v("b")), v("c")),
            "$a = $b and $c",
        ),
        (
            "and kw in assign",
            v("a").assign(bin(LogicalAnd, v("b"), v("c"))),
            "$a = ($b and $c)",
        ),
        (
            "bool and in kw",
            bin(LogicalOr, bin(And, v("a"), v("b")), v("c")),
            "$a && $b or $c",
        ),
        (
            "kw in bool",
            bin(And, bin(LogicalOr, v("a"), v("b")), v("c")),
            "($a or $b) && $c",
        ),
    ];
    check(cases);
}

#[test]
fn coalesce_and_ternary() {
    let t = |c, a, b| Expr::ternary(c, a, b);
    let cases: Vec<(&str, Expr, &str)> = vec![
        (
            "coalesce right assoc",
            bin(Coalesce, v("a"), bin(Coalesce, v("b"), v("c"))),
            "$a ?? $b ?? $c",
        ),
        (
            "coalesce left nested",
            bin(Coalesce, bin(Coalesce, v("a"), v("b")), v("c")),
            "($a ?? $b) ?? $c",
        ),
        (
            "coalesce of or",
            bin(Coalesce, bin(Or, v("a"), v("b")), v("c")),
            "$a || $b ?? $c",
        ),
        (
            "or of coalesce",
            bin(Or, bin(Coalesce, v("a"), v("b")), v("c")),
            "($a ?? $b) || $c",
        ),
        (
            "coalesce in ternary cond",
            t(bin(Coalesce, v("a"), v("b")), int(1), int(2)),
            "$a ?? $b ? 1 : 2",
        ),
        (
            "ternary in coalesce",
            bin(Coalesce, v("a"), t(v("b"), int(1), int(2))),
            "$a ?? ($b ? 1 : 2)",
        ),
        (
            "nested ternary else",
            t(v("a"), int(1), t(v("b"), int(2), int(3))),
            "$a ? 1 : ($b ? 2 : 3)",
        ),
        (
            "nested ternary cond",
            t(t(v("a"), int(1), int(2)), int(3), int(4)),
            "($a ? 1 : 2) ? 3 : 4",
        ),
        (
            "nested ternary then",
            t(v("a"), t(v("b"), int(1), int(2)), int(3)),
            "$a ? ($b ? 1 : 2) : 3",
        ),
        (
            "short ternary nested",
            v("a").short_ternary(v("b").short_ternary(v("c"))),
            "$a ?: ($b ?: $c)",
        ),
        (
            "assign in ternary branches",
            t(v("a"), v("x").assign(int(1)), v("y").assign(int(2))),
            "$a ? $x = 1 : ($y = 2)",
        ),
        (
            "ternary in assign",
            v("x").assign(t(v("a"), int(1), int(2))),
            "$x = $a ? 1 : 2",
        ),
        (
            "ternary in binary",
            bin(Add, int(1), t(v("a"), int(1), int(2))),
            "1 + ($a ? 1 : 2)",
        ),
        (
            "coalesce throw",
            v("a").coalesce(Expr::new_("E", vec![]).throw()),
            "$a ?? throw new E()",
        ),
        (
            "coalesce throw not rightmost",
            bin(Or, v("a").coalesce(Expr::new_("E", vec![]).throw()), v("b")),
            "($a ?? throw new E()) || $b",
        ),
        (
            "ternary else throw",
            t(v("a"), int(1), v("e").throw()),
            "$a ? 1 : throw $e",
        ),
        (
            "ternary then throw",
            t(v("a"), v("e").throw(), int(1)),
            "$a ? throw $e : 1",
        ),
    ];
    check(cases);
}

#[test]
fn unary_not_instanceof_cast() {
    let cases: Vec<(&str, Expr, &str)> = vec![
        (
            "not instanceof",
            v("a").instanceof("B").not(),
            "!$a instanceof B",
        ),
        (
            "instanceof of not",
            v("a").not().instanceof("B"),
            "(!$a) instanceof B",
        ),
        (
            "neg instanceof",
            v("a").neg().instanceof("B"),
            "-$a instanceof B",
        ),
        ("not of mul", bin(Mul, v("a"), v("b")).not(), "!($a * $b)"),
        ("mul of not", bin(Mul, v("a").not(), v("b")), "!$a * $b"),
        ("not not", v("a").not().not(), "!!$a"),
        ("neg neg", v("a").neg().neg(), "- -$a"),
        ("neg neg literal", int(-1).neg(), "- -1"),
        ("neg neg float", Expr::float(-1.5).neg(), "- -1.5"),
        ("neg predec", un(UnaryOp::PreDec, v("a")).neg(), "- --$a"),
        (
            "plus plus",
            un(UnaryOp::Plus, un(UnaryOp::Plus, v("a"))),
            "+ +$a",
        ),
        (
            "plus preinc",
            un(UnaryOp::Plus, un(UnaryOp::PreInc, v("a"))),
            "+ ++$a",
        ),
        ("neg plus", un(UnaryOp::Plus, v("a")).neg(), "-+$a"),
        ("sub neg", bin(Sub, v("a"), v("b").neg()), "$a - -$b"),
        ("sub neg literal", bin(Sub, v("a"), int(-1)), "$a - -1"),
        ("add neg literal lhs", bin(Add, int(-1), v("a")), "-1 + $a"),
        (
            "bitnot of add",
            bin(Add, v("a"), v("b")).cast(CastKind::Int),
            "(int) ($a + $b)",
        ),
        (
            "cast of prop",
            v("a").prop("b").cast(CastKind::String),
            "(string) $a->b",
        ),
        (
            "cast of call",
            Expr::call_fn("f", vec![]).cast(CastKind::Bool),
            "(bool) f()",
        ),
        (
            "cast instanceof",
            v("a").cast(CastKind::Int).instanceof("B"),
            "(int) $a instanceof B",
        ),
        (
            "cast of pow",
            bin(Pow, v("a"), int(2)).cast(CastKind::Float),
            "(float) $a ** 2",
        ),
        ("silence call", Expr::call_fn("f", vec![]).silence(), "@f()"),
        (
            "silence of add",
            bin(Add, v("a"), v("b")).silence(),
            "@($a + $b)",
        ),
        (
            "bitnot",
            un(UnaryOp::BitNot, bin(BitOr, v("a"), v("b"))),
            "~($a | $b)",
        ),
        (
            "postinc in add",
            bin(Add, un(UnaryOp::PostInc, v("a")), int(1)),
            "$a++ + 1",
        ),
        (
            "add preinc",
            bin(Add, v("a"), un(UnaryOp::PreInc, v("b"))),
            "$a + ++$b",
        ),
        (
            "postdec of prop",
            un(UnaryOp::PostDec, v("a").prop("n")),
            "$a->n--",
        ),
        (
            "not assign",
            v("x").assign(Expr::call_fn("f", vec![])).not(),
            "!($x = f())",
        ),
        (
            "instanceof dynamic",
            v("a").instanceof(v("cls")),
            "$a instanceof $cls",
        ),
        (
            "instanceof dynamic call",
            v("a").instanceof(Expr::call_fn("f", vec![])),
            "$a instanceof (f())",
        ),
        (
            "instanceof static",
            v("a").instanceof("static"),
            "$a instanceof static",
        ),
    ];
    check(cases);
}

#[test]
fn dereference_bases() {
    let cases: Vec<(&str, Expr, &str)> = vec![
        (
            "new then method",
            Expr::new_("Foo", vec![]).method("bar", vec![]),
            "(new Foo())->bar()",
        ),
        (
            "new then prop",
            Expr::new_("Foo", vec![]).prop("x"),
            "(new Foo())->x",
        ),
        (
            "new then index",
            Expr::new_("Foo", vec![]).index(int(0)),
            "(new Foo())[0]",
        ),
        (
            "clone then prop",
            v("a").clone_().prop("b"),
            "(clone $a)->b",
        ),
        ("clone of prop", v("a").prop("b").clone_(), "clone $a->b"),
        (
            "clone of call",
            v("a").method("b", vec![]).clone_(),
            "clone $a->b()",
        ),
        (
            "clone of new",
            Expr::new_("Foo", vec![]).clone_(),
            "clone new Foo()",
        ),
        (
            "clone of coalesce",
            v("a").coalesce(v("b")).clone_(),
            "clone ($a ?? $b)",
        ),
        (
            "closure call",
            Expr::closure(Closure::default()).call(vec![]),
            "(function () {})()",
        ),
        (
            "arrow call",
            Expr::arrow_fn(vec![], int(1)).call(vec![]),
            "(fn () => 1)()",
        ),
        (
            "coalesce method",
            v("a").coalesce(v("b")).method("c", vec![]),
            "($a ?? $b)->c()",
        ),
        ("prop as callee", v("a").prop("f").call(vec![]), "($a->f)()"),
        (
            "static prop as callee",
            Expr::static_prop("A", "f").call(vec![]),
            "(A::$f)()",
        ),
        (
            "class const as callee",
            Expr::class_const("A", "F").call(vec![]),
            "(A::F)()",
        ),
        (
            "call result call",
            Expr::call_fn("f", vec![]).call(vec![]),
            "f()()",
        ),
        ("var callee", v("f").call(vec![]), "$f()"),
        (
            "string callee",
            s("strlen").call(args(vec![s("x")])),
            "'strlen'('x')",
        ),
        (
            "array callee",
            Expr::array_of(vec![v("o"), s("m")]).call(vec![]),
            "[$o, 'm']()",
        ),
        ("string index", s("abc").index(int(0)), "'abc'[0]"),
        ("const index", Expr::name("FOO").index(int(0)), "FOO[0]"),
        ("int prop", int(1).prop("x"), "(1)->x"),
        (
            "match method",
            Expr::match_(v("a"), vec![MatchArm::default(v("b"))]).method("c", vec![]),
            "(match ($a) {\n    default => $b,\n})->c()",
        ),
        (
            "nullsafe chain",
            v("a")
                .nullsafe_prop("b")
                .nullsafe_method("c", vec![])
                .index(int(1)),
            "$a?->b?->c()[1]",
        ),
        (
            "static on var",
            Expr::static_call(v("cls"), "make", vec![]),
            "$cls::make()",
        ),
        (
            "static on call",
            Expr::static_call(Expr::call_fn("f", vec![]), "make", vec![]),
            "f()::make()",
        ),
        (
            "static on const",
            Expr::static_call(Expr::name("FOO"), "make", vec![]),
            "(FOO)::make()",
        ),
        (
            "static on string",
            Expr::class_const(s("Foo"), "BAR"),
            "'Foo'::BAR",
        ),
        ("class of expr", Expr::class_name(v("obj")), "$obj::class"),
        (
            "new dynamic var",
            Expr::new_(v("cls"), vec![]),
            "new $cls()",
        ),
        (
            "new dynamic prop",
            Expr::new_(v("this").prop("cls"), vec![]),
            "new $this->cls()",
        ),
        (
            "new dynamic call",
            Expr::new_(Expr::call_fn("f", vec![]), vec![]),
            "new (f())()",
        ),
        (
            "new dynamic static prop",
            Expr::new_(Expr::static_prop("A", "cls"), vec![]),
            "new A::$cls()",
        ),
        (
            "new dynamic concat",
            Expr::new_(s("A").concat(v("b")), vec![]),
            "new ('A' . $b)()",
        ),
        (
            "postinc of new",
            un(UnaryOp::PostInc, v("a").index(int(0))),
            "$a[0]++",
        ),
        (
            "index of assign",
            v("a").assign(v("b")).index(int(0)),
            "($a = $b)[0]",
        ),
        (
            "interp index",
            Expr::interp(vec![InterpPart::Expr(v("a"))]).index(int(0)),
            "\"{$a}\"[0]",
        ),
    ];
    check(cases);
}

#[test]
fn assignment_contexts() {
    let cases: Vec<(&str, Expr, &str)> = vec![
        ("chain", v("a").assign(v("b").assign(int(1))), "$a = $b = 1"),
        (
            "assign in add",
            bin(Add, v("a").assign(int(1)), int(2)),
            "($a = 1) + 2",
        ),
        (
            "assign in and",
            bin(And, v("a"), v("b").assign(int(1))),
            "$a && ($b = 1)",
        ),
        (
            "compound",
            v("a").assign_op(AssignOp::Concat, bin(Concat, v("b"), v("c"))),
            "$a .= $b . $c",
        ),
        (
            "coalesce assign",
            v("a")
                .index(s("k"))
                .assign_op(AssignOp::Coalesce, Expr::array(vec![])),
            "$a['k'] ??= []",
        ),
        (
            "pow assign",
            v("a").assign_op(AssignOp::Pow, int(2)),
            "$a **= 2",
        ),
        (
            "by ref",
            v("a").assign_ref(v("b").index(int(0))),
            "$a = &$b[0]",
        ),
        (
            "list destructure",
            Expr::list(vec![
                Some(ArrayItem::value(v("a"))),
                None,
                Some(ArrayItem::value(v("b"))),
            ])
            .assign(v("arr")),
            "[$a, , $b] = $arr",
        ),
        (
            "assign yield",
            v("x").assign(Expr::yield_(Some(v("v")))),
            "$x = yield $v",
        ),
        (
            "assign print",
            v("x").assign(v("v").print()),
            "$x = print $v",
        ),
        (
            "assign require",
            v("cfg").assign(Expr::include(IncludeKind::Require, s("c.php"))),
            "$cfg = require 'c.php'",
        ),
        (
            "assign arrow",
            v("f").assign(Expr::arrow_fn(
                vec![Param::new("x")],
                bin(Add, v("x"), int(1)),
            )),
            "$f = fn ($x) => $x + 1",
        ),
        (
            "assign ternary",
            v("x").assign(Expr::ternary(v("a"), v("b"), v("c"))),
            "$x = $a ? $b : $c",
        ),
        (
            "assign or kw",
            v("x")
                .assign(v("a"))
                .bin(LogicalOr, Expr::new(ExprKind::Exit(None))),
            "$x = $a or exit()",
        ),
    ];
    check(cases);
}

#[test]
fn prefix_keywords_as_operands() {
    let y = |e| Expr::yield_(Some(e));
    let cases: Vec<(&str, Expr, &str)> = vec![
        (
            "print in add lhs",
            bin(Add, v("a").print(), int(1)),
            "(print $a) + 1",
        ),
        (
            "print in add rhs",
            bin(Add, int(1), v("a").print()),
            "1 + print $a",
        ),
        (
            "print of add",
            bin(Add, v("a"), int(1)).print(),
            "print $a + 1",
        ),
        (
            "print of and kw",
            bin(LogicalAnd, v("a"), v("b")).print(),
            "print ($a and $b)",
        ),
        (
            "yield in concat lhs",
            bin(Concat, y(v("a")), v("b")),
            "(yield $a) . $b",
        ),
        ("yield of add", y(bin(Add, v("a"), int(1))), "yield $a + 1"),
        (
            "yield of or kw",
            y(bin(LogicalOr, v("a"), v("b"))),
            "yield ($a or $b)",
        ),
        ("yield kv", Expr::yield_kv(v("k"), v("v")), "yield $k => $v"),
        (
            "yield kv yield key",
            Expr::yield_kv(y(v("k")), v("v")),
            "yield (yield $k) => $v",
        ),
        (
            "bare yield in add",
            bin(Add, Expr::yield_(None), int(1)),
            "(yield) + 1",
        ),
        (
            "call with yield",
            Expr::call_fn("f", args(vec![y(v("a"))])),
            "f(yield $a)",
        ),
        (
            "yield from",
            Expr::call_fn("g", vec![]).yield_from(),
            "yield from g()",
        ),
        (
            "yield from in add",
            bin(Add, Expr::call_fn("g", vec![]).yield_from(), int(1)),
            "(yield from g()) + 1",
        ),
        (
            "include in concat",
            bin(
                Concat,
                Expr::include(IncludeKind::IncludeOnce, s("a.php")),
                s("x"),
            ),
            "(include_once 'a.php') . 'x'",
        ),
        (
            "include of concat",
            Expr::include(
                IncludeKind::Require,
                bin(Concat, Expr::magic(MagicConst::Dir), s("/a.php")),
            ),
            "require __DIR__ . '/a.php'",
        ),
        (
            "throw of coalesce",
            v("a").coalesce(v("b")).throw(),
            "throw $a ?? $b",
        ),
        (
            "arrow in coalesce",
            v("f").coalesce(Expr::arrow_fn(vec![], int(1))),
            "$f ?? fn () => 1",
        ),
        (
            "arrow body ternary",
            Expr::arrow_fn(vec![], Expr::ternary(v("a"), int(1), int(2))),
            "fn () => $a ? 1 : 2",
        ),
        (
            "arrow in args",
            Expr::call_fn(
                "array_map",
                args(vec![
                    Expr::arrow_fn(vec![Param::new("x")], bin(Mul, v("x"), int(2))),
                    v("xs"),
                ]),
            ),
            "array_map(fn ($x) => $x * 2, $xs)",
        ),
        (
            "arrow in add lhs",
            bin(Add, Expr::arrow_fn(vec![], int(1)), int(2)),
            "(fn () => 1) + 2",
        ),
        ("not print", v("a").print().not(), "!print $a"),
        (
            "neg of print not rightmost",
            bin(Add, v("a").print().neg(), int(1)),
            "-(print $a) + 1",
        ),
        (
            "clone in pow",
            bin(Pow, v("a").clone_(), int(2)),
            "clone $a ** 2",
        ),
    ];
    check(cases);
}

#[test]
fn snapshot_of_all_cases_as_file() {
    // All the trees above, printed as a single PHP file (linted with `php -l`).
    let mut out = Vec::new();
    for (label, e) in all_trees() {
        out.push((label, e));
    }
    insta::assert_snapshot!(expr_cases(out));
}

fn all_trees() -> Vec<(&'static str, Expr)> {
    vec![
        ("mul of add", bin(Mul, bin(Add, v("a"), v("b")), v("c"))),
        ("pow neg", bin(Pow, un(UnaryOp::Neg, v("a")), int(2))),
        ("neg pow", un(UnaryOp::Neg, bin(Pow, v("a"), int(2)))),
        ("concat add", bin(Concat, s("x"), bin(Add, v("a"), v("b")))),
        ("add concat", bin(Add, bin(Concat, s("1"), v("a")), v("b"))),
        ("not instanceof", v("a").instanceof("B").not()),
        ("instanceof not", v("a").not().instanceof("B")),
        ("neg neg", int(-1).neg()),
        (
            "coalesce",
            bin(Coalesce, bin(Coalesce, v("a"), v("b")), v("c")),
        ),
        (
            "ternaries",
            Expr::ternary(
                Expr::ternary(v("a"), int(1), int(2)),
                int(3),
                Expr::ternary(v("b"), int(4), int(5)),
            ),
        ),
        (
            "new deref",
            v("x").assign(Expr::new_("ArrayObject", vec![]).method("count", vec![])),
        ),
        (
            "clone deref",
            v("x").assign(
                Expr::new_("ArrayObject", vec![])
                    .clone_()
                    .method("count", vec![]),
            ),
        ),
        (
            "assign in cond",
            bin(And, v("a"), v("b").assign(int(1))).not(),
        ),
        ("print operand", bin(Add, v("a").print(), int(1))),
        (
            "include operand",
            bin(
                Concat,
                Expr::include(IncludeKind::IncludeOnce, s("a.php")),
                s("x"),
            ),
        ),
        (
            "throw operand",
            v("x").assign(v("a").coalesce(Expr::new_("\\Exception", vec![]).throw())),
        ),
        (
            "casts",
            bin(
                Add,
                v("a").cast(CastKind::Int),
                bin(Add, v("b"), v("c")).cast(CastKind::Float),
            ),
        ),
        (
            "closure call",
            v("x").assign(Expr::closure(Closure::default()).call(vec![])),
        ),
        (
            "list",
            Expr::list(vec![
                Some(ArrayItem::value(v("a"))),
                None,
                Some(ArrayItem::value(v("b"))),
            ])
            .assign(v("arr")),
        ),
        (
            "pow chain",
            v("x").assign(bin(Pow, bin(Pow, int(2), int(3)), int(2))),
        ),
        (
            "shift concat",
            v("x").assign(bin(Shl, bin(Concat, s("1"), s("2")), int(1))),
        ),
    ]
}

fn check(cases: Vec<(&str, Expr, &str)>) {
    let mut failures = Vec::new();
    for (label, e, expected) in &cases {
        let got = p(e);
        if got != *expected {
            failures.push(format!("{label}: expected `{expected}`, got `{got}`"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
