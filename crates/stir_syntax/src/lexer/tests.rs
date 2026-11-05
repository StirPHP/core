use crate::lex;

fn dump(src: &str) -> String {
    let lexed = lex(src);
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
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(dump($src));
        }
    };
}

snap!(html_and_tags, "<html><?php echo 1; ?>\n<b><?= $x ?></b>");
snap!(
    keywords_case_insensitive,
    "<?php FUNCTION foo() { RETURN NULL; }"
);
snap!(names, "<?php \\Foo\\Bar; Foo\\Bar; namespace\\Baz; foo;");
snap!(
    numbers,
    "<?php 1 1_000 0x1F 0b101 0o17 017 1.5 .5 1. 1e10 1.5E-3 1_0.0_1"
);
snap!(
    casts,
    "<?php (int)$a; ( string )$b; (BOOLEAN) $c; (array)$d; (Foo) $e;"
);
snap!(
    operators,
    "<?php $a ??= $b ?-> c; $a <=> $b; $a >>= 1; $a **= 2; $a !== $b; $a <> $b; #[Attr] ..."
);
snap!(
    comments,
    "<?php // line ?> html <?php # hash\n/* block */ /** doc */ /**/"
);
snap!(single_quoted, r#"<?php 'it\'s' b'bin';"#);
snap!(double_plain, r#"<?php "plain \" string";"#);
snap!(
    double_interp,
    r#"<?php "a $b c {$d->e['f']} ${g} $h[0] $i[k] $j[$l] $m->n $o[-1]";"#
);
snap!(
    heredoc,
    "<?php $x = <<<EOT\n  Hello $name\n  {$a[1]}\n  EOT;\n"
);
snap!(nowdoc, "<?php $x = <<<'EOT'\nraw $name\nEOT;\n");
snap!(heredoc_quoted_label, "<?php <<<\"X\"\nhi\nX;");
snap!(backtick, "<?php `ls $dir`;");
snap!(halt_compiler, "<?php __halt_compiler(); raw <?php data");
snap!(variable_variables, "<?php $$a; ${'b'};");
#[test]
fn lossless_on_garbage() {
    for src in [
        "<?php \"unterminated $x",
        "<?php /* open",
        "<?php <<<EOT\nno end",
        "<?php 'x",
        "\u{0}\u{1}<?php \u{7f} ¤",
    ] {
        let lexed = lex(src);
        let total: u32 = lexed.tokens.iter().map(|t| t.len).sum();
        assert_eq!(total as usize, src.len(), "{src:?}");
    }
}
