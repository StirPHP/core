use stir_syntax::parse;

fn check_ok(src: &str) -> String {
    let parse = parse(src);
    assert_eq!(parse.syntax().to_string(), src, "tree must be lossless");
    let tree = parse.debug_tree();
    assert!(parse.errors().is_empty(), "unexpected errors:\n{tree}");
    tree
}

macro_rules! php {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(check_ok(concat!("<?php\n", $src)));
        }
    };
}

// ---------- PHP ----------
php!(
    php_precedence,
    "$a = 1 + 2 * 3 . 'x' ?? $b ?: $c;\n!$a instanceof B && -$c ** 2;\n$x = $y = &$z;\n$a >= $b >> 1;\n"
);
php!(php_assign_in_unary, "!$a = foo(); $a + $b = 3;");
php!(
    php_calls,
    "foo(1, ...$a, name: 2); $o->m()?->n; A::b(); A::$c; A::C; $a::class; strlen(...); new Foo; new Foo()->bar(); new $cls->prop(1);"
);
php!(
    php_class,
    "#[Attr(1)]\nfinal readonly class A extends B implements C, D {\n    use T1, T2 { T1::m insteadof T2; m as protected n; }\n    public const int X = 1, Y = 2;\n    private static ?array $p = null, $q;\n    public private(set) string $name { get => $this->name; set(string $v) { $this->name = $v; } }\n    public function __construct(private readonly int $id = 0, protected ...$rest) {}\n    abstract protected function &m(int|string $a, (A&B)|null $b, Foo &$c): static;\n}\n"
);
php!(
    php_enum_interface_trait,
    "enum Suit: string implements HasLabel { case Hearts = 'H'; case Spades = 'S'; const Wild = self::Spades; public function label(): string { return ucfirst($this->name); } }\ninterface I extends J, K { public function f(); }\ntrait T { abstract public function g(): void; }"
);
php!(
    php_control_flow,
    "if ($a) { echo 1; } elseif ($b) echo 2; else { echo 3; }\nwhile ($i--) continue;\ndo { $i++; } while ($i < 10);\nfor ($i = 0, $j = 1; $i < 3; $i++) {}\nforeach ($xs as $k => &$v) {}\nforeach ($xs as [$a, $b]) {}\nswitch ($x) { case 1: case 2: echo 'a'; break; default: echo 'b'; }\ntry { f(); } catch (A|B $e) { } catch (C) { } finally { }\n"
);
php!(
    php_alt_syntax,
    "if ($a): ?>yes<?php elseif ($b): echo 1; else: echo 2; endif;\nforeach ($a as $b): endforeach;\nwhile (0): endwhile;\nfor (;;): break; endfor;\nswitch ($a): case 1: break; endswitch;\n"
);
php!(
    php_namespaces,
    "namespace App\\Models;\nuse Foo\\Bar, Baz as Q;\nuse function strlen;\nuse const Foo\\X;\nuse Some\\{A, B as C, function d};\nconst A = 1, B = 2;\n"
);
php!(
    php_closures,
    "$f = function &($a) use ($b, &$c): int { return 1; };\n$g = static fn(int $x): int => $x * 2;\n$h = #[Pure] fn() => 1;\n"
);
php!(
    php_strings,
    "$s = \"a $b {$c->d} ${e} $f[0]\";\n$h = <<<EOT\n  x $y\n  EOT;\n$n = <<<'N'\nraw\nN;\n$sh = `ls`;\n"
);
php!(
    php_match_and_misc,
    "$r = match ($x) { 1, 2 => 'a', default => throw new E() };\nlist($a, , $b) = $arr; [$a, [$b]] = $c; ['k' => $v] = $d;\nisset($a, $b['c']); empty($x); exit(1); die; print 'x';\n$a = (int) $b; @f(); yield $k => $v; yield from g(); $x = yield;\nglobal $a, $b; static $s = 1; unset($a[0]); goto end; end:\n__halt_compiler();raw stuff"
);
php!(
    php_inline_html,
    "?>\n<h1><?= $title ?></h1>\n<?php echo 'x' ?>\n"
);
php!(
    php_keywords_as_names,
    "$a->list(); A::class; $a->default; Foo::new(); function list2() {} class X { function print() {} const FOREACH = 1; }"
);
php!(
    php_declare,
    "declare(strict_types=1);\ndeclare(ticks=1) { f(); }\n"
);
php!(
    php_heredoc_interp,
    "echo <<<HTML\n<div>{$a['b']}</div>\nHTML;\n"
);

// ---------- robustness ----------
#[test]
fn recovers_from_garbage() {
    for src in [
        "<?php function (",
        "<?php class { public function }",
        "<?php $a = ; $b = 1;",
        "<?php if ($a { echo 1; }",
        "<?php foo(1, 2",
        "<?php ) ] } ;",
    ] {
        let parse = parse(src);
        assert_eq!(parse.syntax().to_string(), src);
        assert!(
            !parse.errors().is_empty(),
            "expected errors for {src:?}:\n{}",
            parse.debug_tree()
        );
    }
}

#[test]
fn errors_snapshot() {
    insta::assert_snapshot!(parse("<?php $a = ; if ($b { }").debug_tree());
}

#[test]
fn non_keywords_read_as_types_in_parameters() {
    for src in [
        "<?php function f(mut $x) {}",
        "<?php function f(val $x) {}",
        "<?php function f(mut &$x, val ...$y) {}",
        "<?php $a = fn(open $x) => $x;",
    ] {
        let parse = parse(src);
        assert!(parse.errors().is_empty(), "{}", parse.debug_tree());
        assert!(parse.debug_tree().contains("NAMED_TYPE"), "{src}");
    }
}

/// Things PHP rejects at parse time.
#[test]
fn rejects_what_php_rejects() {
    for src in [
        "<?php val $x = 1;",
        "<?php var $x = 1;",
        "<?php $x = $a as int;",
        "<?php $x = $a is int;",
        "<?php $x = $a!!;",
        "<?php $x = if ($a) 1 else 2;",
        "<?php $x = when ($a) { else -> 1 };",
        "<?php $x = { a -> a };",
        "<?php $xs->map { 1 };",
        "<?php $a to $b;",
        "<?php echo 1\necho 2;",
        "<?php function f(): T? {}",
        "<?php function f(int? $x) {}",
        "<?php function f($x: int) {}",
        "<?php function f(x: int) {}",
        "<?php function f(): (int) -> int {}",
        "<?php function f(array<int> $x) {}",
        "<?php function f('a'|'b' $x) {}",
        "<?php function f(1 $x) {}",
        "<?php function f(): int = 1;",
        "<?php function f(var $x) {}",
        "<?php function f(?A|B $x) {}",
        "<?php function f(A|B&C $x) {}",
        "<?php function f(list $x) {}",
        "<?php function <T> f() {}",
        "<?php function f<T>() {}",
        "<?php class A<T> {}",
        "<?php class A($x) {}",
        "<?php class A : B {}",
        "<?php class A extends ?B {}",
        "<?php class A;",
        "<?php data class A {}",
        "<?php sealed class A {}",
        "<?php class A { val x: int = 1; }",
        "<?php class A { var $x: int = 1; }",
        "<?php record A { x: int }",
        "<?php typealias A = int;",
        "<?php foreach ($a as int as $v) {}",
        "<?php $x = (Foo) -> 1;",
    ] {
        let parse = parse(src);
        assert_eq!(parse.syntax().to_string(), src);
        assert!(!parse.errors().is_empty(), "{src}:\n{}", parse.debug_tree());
    }
}

#[test]
fn accepts_real_php_types() {
    for src in [
        "<?php function f(?A $a, A|B|null $b, A&B $c, (A&B)|null $d, A &$e, A|B &...$f): static {}",
        "<?php function f(array $a, callable $b, self $c, parent $d, null $e, false $f): void {}",
        "<?php class A { public const array X = []; const int|string Y = 1; const Z = 1; }",
        "<?php $a >= 1; $a >> 1; $a >>= 1; $a > 1;",
        "<?php $a ? : $b; $a ?: $b;",
        "<?php ?>a<?= $x ?>b<?php // c ?>d",
        "<?php echo \"${a} {$b} $c\";",
        "<?php $a->as; $a->is(1); A::val; foo(val: 1);",
    ] {
        check_ok(src);
    }
}
