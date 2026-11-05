//! What the formatter does to small pieces of PHP. Each input is formatted, compared, and formatted
//! again, which must change nothing.

use stir_fmt::{Indent, Options, format};

#[track_caller]
fn check(input: &str, want: &str) {
    let options = Options::default();
    let got = format(input, &options).unwrap();
    assert_eq!(got, want, "from:\n{input}");
    let again = format(&got, &options).unwrap();
    assert_eq!(again, got, "not stable:\n{input}");
}

/// That something's left as it is.
#[track_caller]
fn keeps(input: &str) {
    check(input, input);
}

#[test]
fn spaces_around_operators() {
    check("<?php\n$a=1+2*3;\n", "<?php\n$a = 1 + 2 * 3;\n");
    check("<?php\n$a  .=  $b.'c';\n", "<?php\n$a  .= $b . 'c';\n");
    check("<?php\n$x=$a?$b:$c;\n", "<?php\n$x = $a ? $b : $c;\n");
    check("<?php\n$x=$a?:$c;\n", "<?php\n$x = $a ?: $c;\n");
    check("<?php\n$x=$a ? : $c;\n", "<?php\n$x = $a ?: $c;\n");
    check("<?php\n$x=$a??$c;\n", "<?php\n$x = $a ?? $c;\n");
    check("<?php\n$x=$a<=>$c;\n", "<?php\n$x = $a <=> $c;\n");
    check(
        "<?php\nif($a&&!$b||$c){}\n",
        "<?php\nif ($a && !$b || $c) {}\n",
    );
    check("<?php\n$a = & $b;\n", "<?php\n$a = &$b;\n");
    check(
        "<?php\n$a = $b & $c;\n$a = $b&$c;\n",
        "<?php\n$a = $b & $c;\n$a = $b & $c;\n",
    );
    check("<?php\n$x = ! $a;\n", "<?php\n$x = !$a;\n");
    check(
        "<?php\n$x = - $a;\n$y = $a ++;\n",
        "<?php\n$x = -$a;\n$y = $a++;\n",
    );
    check("<?php\n$x = @ foo();\n", "<?php\n$x = @foo();\n");
    check("<?php\n$x = (int)$a;\n", "<?php\n$x = (int) $a;\n");
    check(
        "<?php\n$x = $a instanceof  Foo;\n",
        "<?php\n$x = $a instanceof Foo;\n",
    );
}

#[test]
fn what_would_change_meaning_stays_apart() {
    keeps("<?php\n$x = - -$a;\n$y = + +$a;\n$z = $a - -$b;\n");
    keeps("<?php\n$x = 1 . 2;\n");
    keeps("<?php\n$x = new Foo;\n");
}

#[test]
fn alignment_before_operators_is_allowed() {
    keeps("<?php\n$a   = 1;\n$bcd = 2;\n");
    keeps("<?php\n$a = [\n    'a'   => 1,\n    'bcd' => 2,\n];\n");
    check("<?php\n$a = ['a'=>1];\n", "<?php\n$a = ['a' => 1];\n");
    keeps("<?php\nfoo();   // one\nbarbaz(); // two\n");
}

#[test]
fn brackets_and_commas() {
    check("<?php\nfoo( $a , $b,$c ) ;\n", "<?php\nfoo($a, $b, $c);\n");
    check("<?php\n$a = [ 1,2 , ];\n", "<?php\n$a = [1, 2,];\n");
    check(
        "<?php\n$a [ 0 ] = $b -> c ( );\n",
        "<?php\n$a[0] = $b->c();\n",
    );
    check("<?php\nFoo :: bar ();\n", "<?php\nFoo::bar();\n");
    check("<?php\n$a?->b ( );\n", "<?php\n$a?->b();\n");
    check("<?php\n[, $a] = $b;\n", "<?php\n[, $a] = $b;\n");
    check("<?php\nlist( , $a) = $b;\n", "<?php\nlist(, $a) = $b;\n");
    check(
        "<?php\nfoo(... $a);\nbar(...);\n",
        "<?php\nfoo(...$a);\nbar(...);\n",
    );
    check("<?php\n$x->{ $a }();\n", "<?php\n$x->{$a}();\n");
    check("<?php\nfoo(a : 1, b:2);\n", "<?php\nfoo(a: 1, b: 2);\n");
    check(
        "<?php\nisset ($a);\narray (1);\nempty ($a);\n",
        "<?php\nisset($a);\narray(1);\nempty($a);\n",
    );
    check("<?php\nreturn(1);\n", "<?php\nreturn (1);\n");
    check(
        "<?php\necho(1);\nprint (1);\n",
        "<?php\necho (1);\nprint(1);\n",
    );
}

#[test]
fn keywords_before_parentheses() {
    check(
        "<?php\nif($a){}elseif($b){}else{}\nwhile($a){}\nfor($i=0;$i<1;$i++){}\nforeach($a as $k=>$v){}\nswitch($a){}\n",
        "<?php\nif ($a) {} elseif ($b) {} else {}\nwhile ($a) {}\nfor ($i = 0; $i < 1; $i++) {}\nforeach ($a as $k => $v) {}\nswitch ($a) {}\n",
    );
    check(
        "<?php\nfor(;;){}\nfor( ; ; ){}\n",
        "<?php\nfor (;;) {}\nfor (;;) {}\n",
    );
    check(
        "<?php\ntry{}catch(A|B $e){}finally{}\n",
        "<?php\ntry {} catch (A|B $e) {} finally {}\n",
    );
    check(
        "<?php\n$f=function($a)use($b){};\n$g=fn ($a)=>1;\n",
        "<?php\n$f = function ($a) use ($b) {};\n$g = fn($a) => 1;\n",
    );
    check(
        "<?php\n$x=match($a){1=>2};\n",
        "<?php\n$x = match ($a) { 1 => 2 };\n",
    );
    check(
        "<?php\ndeclare (strict_types = 1);\n",
        "<?php\n\ndeclare(strict_types=1);\n",
    );
}

#[test]
fn declarations() {
    check(
        "<?php\nclass A extends B implements C{\n}\n",
        "<?php\nclass A extends B implements C\n{\n}\n",
    );
    check(
        "<?php\nclass A { public $x; function f() { return 1; } }\n",
        "<?php\nclass A\n{\n    public $x;\n    function f()\n    {\n        return 1;\n    }\n}\n",
    );
    check("<?php\nclass A {}\n", "<?php\nclass A {}\n");
    check("<?php\nclass A { }\n", "<?php\nclass A {}\n");
    keeps("<?php\nclass A\n{\n}\n");
    check(
        "<?php\nfunction f( int  $a , ?string $b=null ) : ?int {\n    return 1;\n}\n",
        "<?php\nfunction f(int $a, ?string $b = null): ?int\n{\n    return 1;\n}\n",
    );
    check(
        "<?php\nfunction f(int | string $a, A & B $b): void {}\n",
        "<?php\nfunction f(int|string $a, A&B $b): void {}\n",
    );
    check(
        "<?php\nfunction & f( & $a, ...$b) {}\n",
        "<?php\nfunction &f(&$a, ...$b) {}\n",
    );
    keeps("<?php\nfunction f() {}\n");
    keeps("<?php\nfunction f()\n{\n}\n");
    check(
        "<?php\nfunction f(\n    int $a,\n    int $b\n): int\n{\n    return 1;\n}\n",
        "<?php\nfunction f(\n    int $a,\n    int $b\n): int {\n    return 1;\n}\n",
    );
    check(
        "<?php\nenum A : string { case B = 'b'; }\n",
        "<?php\nenum A: string\n{\n    case B = 'b';\n}\n",
    );
    keeps("<?php\n$a = new class {\n    public $x;\n};\n");
    keeps("<?php\n$a = new class(1) extends B implements C {\n    public $x;\n};\n");
    keeps("<?php\n#[Attr(1)]\nclass A {}\n");
    keeps("<?php\nclass A\n{\n    #[Attr]\n    #[Other(1, 2)]\n    public int $x = 1;\n}\n");
    keeps(
        "<?php\nclass A\n{\n    use B, C {\n        B::x insteadof C;\n        C::x as y;\n    }\n}\n",
    );
    keeps(
        "<?php\nclass A\n{\n    public string $x {\n        get => 'a';\n        set {\n            $this->x = $value;\n        }\n    }\n    private(set) int $y;\n}\n",
    );
}

#[test]
fn control_structures_join_their_braces() {
    check(
        "<?php\nif ($a)\n{\n    b();\n}\nelse\n{\n    c();\n}\n",
        "<?php\nif ($a) {\n    b();\n} else {\n    c();\n}\n",
    );
    check(
        "<?php\ntry\n{\n    b();\n}\ncatch (E $e)\n{\n}\nfinally\n{\n}\n",
        "<?php\ntry {\n    b();\n} catch (E $e) {\n} finally {\n}\n",
    );
    check(
        "<?php\ndo\n{\n    b();\n}\nwhile ($a);\n",
        "<?php\ndo {\n    b();\n} while ($a);\n",
    );
    check(
        "<?php\nif ($a) { b(); c(); }\n",
        "<?php\nif ($a) {\n    b();\n    c();\n}\n",
    );
    // a comment keeps its place
    keeps("<?php\nif ($a) { // why\n    b();\n} // done\nelse {\n    c();\n}\n");
    keeps("<?php\nif ($a)\n    b();\nelse\n    c();\n");
    keeps("<?php\nif ($a) return;\n");
}

#[test]
fn indentation_follows_nesting() {
    check(
        "<?php\nif ($a) {\nb();\n      if ($c) {\n d();\n  }\n}\n",
        "<?php\nif ($a) {\n    b();\n    if ($c) {\n        d();\n    }\n}\n",
    );
    check(
        "<?php\nfoo(\n$a,\n  [\n1,\n],\n);\n",
        "<?php\nfoo(\n    $a,\n    [\n        1,\n    ],\n);\n",
    );
    keeps("<?php\nfoo($a, function () {\n    b();\n}, $c);\n");
    keeps("<?php\nfoo(bar(\n    $a\n));\n");
    keeps("<?php\n$a = array_map(fn($x) => $x, [\n    1,\n]);\n");
    check(
        "<?php\nfoo($a,\n          $b);\n",
        "<?php\nfoo($a,\n    $b);\n",
    );
}

#[test]
fn switch_cases_and_their_statements() {
    check(
        "<?php\nswitch ($a) {\ncase 1:\nb();\nbreak;\n    default:\n            c();\n}\n",
        "<?php\nswitch ($a) {\n    case 1:\n        b();\n        break;\n    default:\n        c();\n}\n",
    );
    keeps(
        "<?php\nswitch ($a) {\n    case 1:\n        b();\n        // no break\n    case 2:\n        c();\n}\n",
    );
    keeps("<?php\nswitch ($a) {\n    // the first\n    case 1:\n        b();\n}\n");
    keeps("<?php\nswitch ($a) {\n    case 1: {\n        b();\n    }\n}\n");
}

#[test]
fn match_arms() {
    keeps("<?php\n$x = match ($a) {\n    1, 2 => 'a',\n    default => 'b',\n};\n");
    check(
        "<?php\n$x = match ($a) {\n1 => 'a',\n     default => 'b',\n};\n",
        "<?php\n$x = match ($a) {\n    1 => 'a',\n    default => 'b',\n};\n",
    );
}

#[test]
fn continuation_lines_keep_how_they_are_laid_out() {
    keeps("<?php\n$a = $b\n    ->c()\n    ->d();\n");
    keeps("<?php\n$root\n    ->children()\n        ->scalar('a')\n        ->end()\n    ->end();\n");
    check("<?php\n$a = $b\n->c();\n", "<?php\n$a = $b\n    ->c();\n");
    check(
        "<?php\nreturn $a\n&& $b;\n",
        "<?php\nreturn $a\n    && $b;\n",
    );
    keeps("<?php\nif (\n    $a\n    && $b\n) {\n}\n");
    keeps("<?php\nif ($a\n    && $b) {\n}\n");
    keeps("<?php\nif ($a &&\n    ($b &&\n    $c) ||\n    ($d &&\n     $e)) {\n}\n");
    keeps("<?php\nfunction f($a,\n    $b)\n{\n    c();\n}\n");
    keeps("<?php\n$a\n    ->b()\n;\n");
    keeps("<?php\n$a = $b\n    ->c()\n->d();\n");
    keeps("<?php\nif ($a) {\n    $b\n        ->c()\n    ;\n}\n");
    keeps("<?php\n$x = $a\n    ? $b\n    : $c;\n");
    // moved along with the statement
    check(
        "<?php\nif ($a) {\n  $b = $c\n      ->d();\n}\n",
        "<?php\nif ($a) {\n    $b = $c\n        ->d();\n}\n",
    );
    keeps(
        "<?php\n$a = $b\n    ->c(function () {\n        d();\n    })\n    ->e([\n        1,\n    ]);\n",
    );
}

#[test]
fn blank_lines() {
    check("<?php\na();\n\n\n\nb();\n", "<?php\na();\n\nb();\n");
    check(
        "<?php\nfunction f()\n{\n\n    a();\n\n}\n",
        "<?php\nfunction f()\n{\n    a();\n}\n",
    );
    check("<?php\nfoo(\n\n    $a\n\n);\n", "<?php\nfoo(\n    $a\n);\n");
    check("<?php\na();   \nb();\t\n", "<?php\na();\nb();\n");
    check("<?php\na();\n\n\n", "<?php\na();\n");
    check("<?php\na();", "<?php\na();\n");
}

#[test]
fn the_top_of_a_file() {
    check(
        "<?php\nnamespace A;\nuse B;\nuse C;\nuse function D\\e;\nuse const D\\F;\nclass G {}\n",
        "<?php\n\nnamespace A;\n\nuse B;\nuse C;\n\nuse function D\\e;\n\nuse const D\\F;\n\nclass G {}\n",
    );
    check(
        "<?php\ndeclare(strict_types=1);\nnamespace A;\n",
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace A;\n",
    );
    keeps("<?php declare(strict_types=1);\n\nnamespace A;\n");
    keeps("<?php\n\n/**\n * Header.\n */\n\nnamespace A;\n");
    keeps("<?php\n// Header\n\nnamespace A;\n");
    keeps("#!/usr/bin/env php\n<?php\n\nnamespace A;\n");
    keeps(
        "<?php\nnamespace A {\n    class B {}\n}\n\nnamespace C {\n    class D {}\n}\n"
            .replace("<?php\nnamespace", "<?php\n\nnamespace")
            .as_str(),
    );
    check("<?php\n\n\n\nfoo();\n", "<?php\n\nfoo();\n");
    keeps("<?php\nfoo();\n");
    keeps(
        "<?php\nuse A\\{B, C};\n"
            .replace("<?php\nuse", "<?php\n\nuse")
            .as_str(),
    );
    check(
        "<?php\n\nuse A \\ { B , C };\n",
        "<?php\n\nuse A\\{B, C};\n",
    );
}

#[test]
fn line_endings_are_the_files() {
    check(
        "<?php\r\nfunction f() {\r\nreturn 1;\r\n}\r\n",
        "<?php\r\nfunction f()\r\n{\r\n    return 1;\r\n}\r\n",
    );
}

#[test]
fn comments() {
    check(
        "<?php\nclass A\n{\n/**\n * Doc.\n */\n    function f() {}\n}\n",
        "<?php\nclass A\n{\n    /**\n     * Doc.\n     */\n    function f() {}\n}\n",
    );
    check(
        "<?php\nif ($a) {\n        /*\n         * Stars.\n         */\n        b();\n}\n",
        "<?php\nif ($a) {\n    /*\n     * Stars.\n     */\n    b();\n}\n",
    );
    // other block comments move as a whole
    check(
        "<?php\nif ($a) {\n        /*\n          text\n        */\n        b();\n}\n",
        "<?php\nif ($a) {\n    /*\n      text\n    */\n    b();\n}\n",
    );
    check(
        "<?php\n// trailing   \na();\n",
        "<?php\n// trailing\na();\n",
    );
    keeps("<?php\n# hash\na(); # hash\n");
    keeps("<?php\na(/* one */ $b, /* two */ $c);\n");
    keeps("<?php\nfoo(\n    // first\n    $a,\n    // last\n);\n");
    keeps("<?php\nclass A\n{\n    // only a comment\n}\n");
    keeps("<?php\nfunction f()\n{\n    a();\n    // at the end\n}\n");
    keeps("<?php\nif ($a) {\n    b();\n// about the else\n} else {\n    c();\n}\n");
}

#[test]
fn strings_stay_as_they_are() {
    keeps("<?php\n$a = \"x {$b [ 0 ]} y\";\n");
    keeps("<?php\n$a = 'multi\n   line';\n");
    keeps("<?php\n$a = `ls  {$b}`;\n");
    keeps("<?php\n$a = <<<EOT\n  text {$b [ 0 ]}\n    more   \n  EOT;\n");
    keeps("<?php\n$a = <<<'EOT'\n  text\n  EOT;\n");
    // the first line moves, what's after it doesn't
    check(
        "<?php\nif ($a) {\n$b = <<<EOT\n    text\nEOT;\n}\n",
        "<?php\nif ($a) {\n    $b = <<<EOT\n    text\nEOT;\n}\n",
    );
    keeps("<?php\nfoo(<<<EOT\n  text\n  EOT, $b);\n");
    keeps("<?php\nfoo(<<<EOT\n  text\n  EOT\n);\n");
}

#[test]
fn html_stays_as_it_is() {
    keeps("<html>\n<?php echo $a; ?>\n  <b>  x  </b>\n");
    keeps("<?= $a ?>\n");
    check("<div><?=$a?></div>\n", "<div><?= $a ?></div>\n");
    keeps("<?php if ($a): ?>\n  <b>x</b>\n<?php else: ?>\n  <i>y</i>\n<?php endif; ?>\n");
    keeps("<?php\n  foreach ($a as $b):\n    echo $b;\n  endforeach;\n?>\n<p>text</p>\n");
    keeps("<?php\n__halt_compiler(); data  here\n  more");
}

#[test]
fn alternative_syntax() {
    check(
        "<?php\nif ($a) :\nb();\nelseif ($c):\nd();\nelse:\ne();\nendif;\n",
        "<?php\nif ($a):\n    b();\nelseif ($c):\n    d();\nelse:\n    e();\nendif;\n",
    );
    check(
        "<?php\nforeach ($a as $b):\nc();\nendforeach;\nwhile ($a):\nb();\nendwhile;\n",
        "<?php\nforeach ($a as $b):\n    c();\nendforeach;\nwhile ($a):\n    b();\nendwhile;\n",
    );
    check(
        "<?php\nswitch ($a):\ncase 1:\nb();\nendswitch;\n",
        "<?php\nswitch ($a):\n    case 1:\n        b();\nendswitch;\n",
    );
}

#[test]
fn closures_on_one_line_may_stay() {
    keeps("<?php\n$f = function () { return 1; };\n");
    keeps("<?php\nfoo(function ($a) { return $a; }, $b);\n");
    keeps("<?php\n$a = new class { public $x; };\n");
    keeps(
        "<?php\nclass A\n{\n    public string $x { get => 'a'; }\n    public int $y { get { return 1; } }\n}\n",
    );
}

#[test]
fn tabs_when_asked() {
    let options = Options {
        indent: Indent::Tabs,
    };
    let got = format("<?php\nif ($a) {\n    b(\n  $c\n    );\n}\n", &options).unwrap();
    assert_eq!(got, "<?php\nif ($a) {\n\tb(\n\t\t$c\n\t);\n}\n");
    assert_eq!(format(&got, &options).unwrap(), got);
    // and a tab reads as a level
    let got = format("<?php\nif ($a) {\n\tb();\n}\n", &Options::default()).unwrap();
    assert_eq!(got, "<?php\nif ($a) {\n    b();\n}\n");
}

#[test]
fn broken_files_are_refused() {
    assert_eq!(
        format("<?php\nif (\n", &Options::default()),
        Err(stir_fmt::Error::Syntax)
    );
}

#[test]
fn a_range_takes_only_what_touches_it() {
    let source = "<?php\n$a=1;\n$b=2;\n$c=3;\n";
    let at = source.find("$b").unwrap() as u32;
    let range = stir_syntax::TextRange::new(at.into(), (at + 5).into());
    let edits = stir_fmt::range_edits(source, range, &Options::default()).unwrap();
    assert_eq!(
        stir_fmt::apply(source, &edits),
        "<?php\n$a=1;\n$b = 2;\n$c=3;\n"
    );
}
