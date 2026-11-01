use stir_syntax::{Dialect, parse};

fn check_ok(src: &str, dialect: Dialect) -> String {
    let parse = parse(src, dialect);
    assert_eq!(parse.syntax().to_string(), src, "tree must be lossless");
    let tree = parse.debug_tree();
    assert!(parse.errors().is_empty(), "unexpected errors:\n{tree}");
    tree
}

macro_rules! php {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(check_ok(concat!("<?php\n", $src), Dialect::Php));
        }
    };
}

macro_rules! stir {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(check_ok($src, Dialect::Stir));
        }
    };
}

macro_rules! no_errors {
    ($name:ident, $dialect:ident, $src:expr) => {
        #[test]
        fn $name() {
            check_ok($src, Dialect::$dialect);
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

// ---------- StirPHP ----------
stir!(
    stir_vals_vars,
    "val name: String = \"x\"\nvar $count = 0\n$count += 1\nval total = $count * 2\n"
);
stir!(
    stir_function,
    "function greet(name: String, mut times: Int = 1): String {\n    var $out = \"\"\n    while (times > 0) {\n        $out = $out . \"Hello, ${name}! \"\n        times -= 1\n    }\n    return $out\n}\n"
);
stir!(
    stir_expression_body,
    "function double(x: Int): Int = x * 2\n"
);
stir!(
    stir_generics,
    "function <T> first(xs: List<T>): T? = xs->firstOrNull()\nclass Box<out T of Comparable<T>>(val value: T)\ninterface Sink<in T> { function put(item: T): Unit }\nfunction <K, V> merge(a: Map<K, V>, b: Map<K, V>): Map<K, V> where K of Stringable { return a }\nval m: Map<String, List<Int>> = mapOf()\n"
);
stir!(
    stir_extension_fn,
    "function String.isBlank(): Bool = trim(this) == \"\"\nfunction <T> List<T>.second(): T = this[1]\n"
);
stir!(
    stir_nullability,
    "val user = find(1)\nval n = user?->name ?: \"anon\"\nval m = user!!->name\nval l = user?->name?->length() ?: 0\n"
);
stir!(
    stir_function_types,
    "val f: (Int, String) -> Bool = { n, s -> s->length() == n }\nval g: ((Int) -> Int)? = null\nfunction <T, R> List<T>.map(transform: (T) -> R): List<R> { return [] }\n"
);
stir!(
    stir_lambdas,
    "users->filter { it->active }->map { u -> u->name }\nrun { echo 1 }\nxs->fold(0) { acc, x: Int -> acc + x }\n"
);
stir!(
    stir_when,
    "val d = when (x) {\n    is Int -> \"int\"\n    is String, null -> \"other\"\n    !is Bool -> \"no\"\n    else -> \"?\"\n}\nwhen {\n    a > b -> echo 1\n    else -> { echo 2 }\n}\n"
);
stir!(
    stir_classes,
    "sealed interface Result<out T>\ndata class Ok<T>(val value: T) : Result<T>\ndata class Err(val msg: String) : Result<Nothing>\nopen class Base(val id: Int)\nclass User(val id: Int, var $name: String) : Base(id), JsonSerializable {\n    val created: Int = 0\n    var $visits: Int = 0\n    function rename(to: String): Unit { $this->name = to }\n}\n"
);
stir!(
    stir_records,
    "record UserRow {\n    id: Int\n    name: String?\n}\nrecord Page<T> { items: List<T>; total: Int }\nval r = UserRow { id = 1, name = null }\nr->id = 2\n"
);
stir!(
    stir_typealias_unions,
    "typealias Status = \"active\" | \"banned\"\nval s: Status = \"active\"\nval x: Int | String = 1\n"
);
stir!(
    stir_is_as,
    "if (x is String && y !is Int) { val z = x as String }\nval w = a as? Int ?: 0\n"
);
stir!(stir_if_expr, "val max = if (a > b) a else b\n");
stir!(
    stir_newline_continuation,
    "val s = $a\n    ->trim()\n    ?->upper()\n    . \"x\"\nval t = 1\n-2\n"
);
stir!(
    stir_templates,
    "val s = \"Hi ${name}, ${user->name}, ${f(\"a ${b}\")} $literal {$x}\"\n"
);
stir!(
    stir_open_tag_namespace,
    "<?php\n\nnamespace App;\n\nuse Foo\\Bar\n\nfunction greet(name: String): String {\n    return \"Hello, ${name}\"\n}\n\necho greet(\"world\")\n"
);
stir!(
    stir_semicolons_still_ok,
    "val a = 1; val b = 2;\necho a + b;\n"
);

// ---------- robustness ----------
no_errors!(
    stir_init_sample,
    Stir,
    "<?php\n\nnamespace App;\n\nfunction greet(name: String, mut times: Int = 1): String {\n    var $out = \"\"\n    while (times > 0) {\n        $out = $out . \"Hello, ${name}! \"\n        times -= 1\n    }\n    return $out\n}\n\necho greet(\"world\")\n"
);

#[test]
fn recovers_from_garbage() {
    for (src, dialect) in [
        ("<?php function (", Dialect::Php),
        ("<?php class { public function }", Dialect::Php),
        ("<?php $a = ; $b = 1;", Dialect::Php),
        ("<?php if ($a { echo 1; }", Dialect::Php),
        ("<?php foo(1, 2", Dialect::Php),
        ("<?php ) ] } ;", Dialect::Php),
        ("val = \nvar x = 1\nfunction f( { }", Dialect::Stir),
        ("when (x) { is -> }", Dialect::Stir),
        ("record { : }", Dialect::Stir),
        ("val x: List<Int = 1", Dialect::Stir),
    ] {
        let parse = parse(src, dialect);
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
    insta::assert_snapshot!(parse("<?php $a = ; if ($b { }", Dialect::Php).debug_tree());
}

stir!(
    stir_stdlib_gaps,
    "function <T> List<T?>.filterNotNull(): List<T> = []\nforeach (xs as $x) { echo $x }\nforeach (m as $k => $v) {}\nval f = { x: Int -> if (x > 0) 1 else 2 }\nval g = { -> 1 }\nval list = xs\necho list->size()\n"
);

stir!(
    stir_infix_and_type_args,
    "val p = \"a\" to 1\nval xs = mutableListOf<Int>()\nval m = mapOf<String, List<Int>>()\nval c = a < b\nval d = x->get<Int>(1)\n"
);
