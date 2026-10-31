use crate::{Dialect, lex};

fn dump(src: &str, dialect: Dialect) -> String {
    let lexed = lex(src, dialect);
    let mut out = String::new();
    let mut pos = 0usize;
    for tok in &lexed.tokens {
        let text = &src[pos..pos + tok.len as usize];
        out.push_str(&format!("{:?} {:?}\n", tok.kind, text));
        pos += tok.len as usize;
    }
    for err in &lexed.errors {
        out.push_str(&format!("error@{}: {}\n", err.offset, err.message));
    }
    out
}

macro_rules! snap {
    ($name:ident, $dialect:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(dump($src, Dialect::$dialect));
        }
    };
}

snap!(
    html_and_tags,
    Php,
    "<html><?php echo 1; ?>\n<b><?= $x ?></b>"
);
snap!(
    keywords_case_insensitive,
    Php,
    "<?php FUNCTION foo() { RETURN NULL; }"
);
snap!(
    names,
    Php,
    "<?php \\Foo\\Bar; Foo\\Bar; namespace\\Baz; foo;"
);
snap!(
    numbers,
    Php,
    "<?php 1 1_000 0x1F 0b101 0o17 017 1.5 .5 1. 1e10 1.5E-3 1_0.0_1"
);
snap!(
    casts,
    Php,
    "<?php (int)$a; ( string )$b; (BOOLEAN) $c; (array)$d; (Foo) $e;"
);
snap!(
    operators,
    Php,
    "<?php $a ??= $b ?-> c; $a <=> $b; $a >>= 1; $a **= 2; $a !== $b; $a <> $b; #[Attr] ..."
);
snap!(
    comments,
    Php,
    "<?php // line ?> html <?php # hash\n/* block */ /** doc */ /**/"
);
snap!(single_quoted, Php, r#"<?php 'it\'s' b'bin';"#);
snap!(double_plain, Php, r#"<?php "plain \" string";"#);
snap!(
    double_interp,
    Php,
    r#"<?php "a $b c {$d->e['f']} ${g} $h[0] $i[k] $j[$l] $m->n $o[-1]";"#
);
snap!(
    heredoc,
    Php,
    "<?php $x = <<<EOT\n  Hello $name\n  {$a[1]}\n  EOT;\n"
);
snap!(nowdoc, Php, "<?php $x = <<<'EOT'\nraw $name\nEOT;\n");
snap!(heredoc_quoted_label, Php, "<?php <<<\"X\"\nhi\nX;");
snap!(backtick, Php, "<?php `ls $dir`;");
snap!(
    halt_compiler,
    Php,
    "<?php __halt_compiler(); raw <?php data"
);
snap!(variable_variables, Php, "<?php $$a; ${'b'};");
snap!(
    stir_basic,
    Stir,
    "val name: String = \"x\"\nvar $count = 0\n"
);
snap!(
    stir_open_tag,
    Stir,
    "<?php\nfunction f(mut x: Int): Unit {}"
);
snap!(
    stir_templates,
    Stir,
    r#""Hi ${name}, $count {$x} ${user->name}""#
);
snap!(stir_nested_template, Stir, r#""a ${f("b ${c}")} d""#);
snap!(
    stir_generics,
    Stir,
    "val x: Map<String, List<Int>> = mapOf()"
);
snap!(
    stir_lambda,
    Stir,
    "users->filter { it->active }->map { u -> u->name }"
);
snap!(stir_not_null, Stir, "x!!->y ?: z");

#[test]
fn lossless_on_garbage() {
    for src in [
        "<?php \"unterminated $x",
        "<?php /* open",
        "<?php <<<EOT\nno end",
        "<?php 'x",
        "\u{0}\u{1}<?php \u{7f} ¤",
    ] {
        let lexed = lex(src, Dialect::Php);
        let total: u32 = lexed.tokens.iter().map(|t| t.len).sum();
        assert_eq!(total as usize, src.len(), "{src:?}");
    }
}
