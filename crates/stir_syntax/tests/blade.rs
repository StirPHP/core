//! Blade templates: the tree of each group of directives, what's left as text, and that nothing
//! gets lost or panics whatever the input.

use std::path::Path;

use stir_syntax::{SyntaxKind, is_blade_path, parse_blade};

#[test]
fn a_template_is_known_by_its_whole_name() {
    assert!(is_blade_path(Path::new("resources/views/a.blade.php")));
    assert!(is_blade_path(Path::new(".blade.php")));
    assert!(!is_blade_path(Path::new("a.php")));
    assert!(!is_blade_path(Path::new("blade.php")));
    assert!(!is_blade_path(Path::new("a.blade.php/b")));
}

fn check_ok(src: &str) -> String {
    let parse = parse_blade(src);
    assert_eq!(parse.syntax().to_string(), src, "tree must be lossless");
    let tree = parse.debug_tree();
    assert!(parse.errors().is_empty(), "unexpected errors:\n{tree}");
    tree
}

fn check_err(src: &str) -> String {
    let parse = parse_blade(src);
    assert_eq!(parse.syntax().to_string(), src, "tree must be lossless");
    assert!(
        !parse.errors().is_empty(),
        "no errors:\n{}",
        parse.debug_tree()
    );
    parse.debug_tree()
}

macro_rules! blade {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(check_ok($src));
        }
    };
}

macro_rules! blade_err {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(check_err($src));
        }
    };
}

/// The kinds of the tree's nodes, in order, without the file.
fn nodes(src: &str) -> Vec<SyntaxKind> {
    parse_blade(src)
        .syntax()
        .descendants()
        .skip(1)
        .map(|n| n.kind())
        .collect()
}

/// Is `src` nothing but HTML, as far as Blade goes?
fn is_text(src: &str) {
    assert_eq!(nodes(src), [SyntaxKind::INLINE_HTML_STMT], "{src}");
    check_ok(src);
}

// ---------- echos and comments ----------
blade!(echos, "<p>{{ $a }}</p>{!! $b->c !!}{{{ $d }}}{{$e}}\n");
blade!(echo_before_crlf, "{{ $a }}\r\nb");
blade!(echo_ends_with_semicolon, "{{ $a; }}");
blade!(echo_of_several_lines, "{{\n    $a\n        ->b()\n}}");
blade!(echo_multibyte_around, "👋 {{ 'سلام' }} 👋");
blade!(comment, "a{{-- {{ $a }} @if <?php --}}b");
blade!(comment_between_directives, "@if ($a){{-- b --}}@endif");
blade_err!(echo_ends_at_first_close, "{{ '}}' }}");
blade_err!(raw_echo_ends_at_first_close, "{!! '!!}' !!}");
blade_err!(echo_empty, "{{ }}");
blade_err!(echo_broken_stays_inside, "{{ $a + }}<p>{{ $b }}</p>");

#[test]
fn echo_without_an_end_is_text() {
    is_text("a {{ b");
    is_text("a {!! b }}");
    is_text("{{}}");
}

#[test]
fn comment_without_an_end_is_an_echo_if_anything() {
    is_text("{{-- a");
    assert_eq!(nodes("{{-- a }}")[0], SyntaxKind::ECHO_TAG_STMT);
}

// ---------- what stays text ----------
#[test]
fn text_that_looks_like_a_directive() {
    for src in [
        "foo@bar.com",
        "foo@if($a)",
        "@media (min-width: 100px) { a { b: c } }",
        "@click=\"go($a)\"",
        "<div @click.prevent='a'>",
        "@font-face { a: b }",
        "@custom($a, $b)",
        "@custom::name($a)",
        "@if::name($a)",
        "@ if",
        "@",
        "@@",
        "@php",
        "@@if ($a)",
        "@@foreach(\n    $i as $x\n)",
        "@@verbatim @@continue @@endverbatim",
        "@@php $a @endphp",
        "@endverbatim",
        "@endphp",
        "@{{ $a }}",
        "@{!! $a !!}",
        "@{{{ $a }}}",
        "@{{ {{ $a }}",
        "<span @class([')']></span>",
        "<a @class(['k\\' => ()])></a>",
    ] {
        is_text(src);
    }
}

#[test]
fn a_directive_can_follow_anything_but_a_word() {
    for src in ["-@csrf", "é@csrf", "}@csrf", "a@@csrf"] {
        assert!(nodes(src).contains(&SyntaxKind::BLADE_DIRECTIVE), "{src}");
    }
}

#[test]
fn a_directive_right_after_a_word_is_text_even_after_a_directive() {
    assert_eq!(
        nodes("@csrf@csrf"),
        [SyntaxKind::BLADE_DIRECTIVE, SyntaxKind::INLINE_HTML_STMT]
    );
}

#[test]
fn what_is_in_a_text_directive_is_not_compiled_but_its_echos_are() {
    is_text("@custom(@csrf)");
    is_text("@@custom(@csrf)");
    assert_eq!(
        nodes("@custom({{ $a }})")[..2],
        [SyntaxKind::INLINE_HTML_STMT, SyntaxKind::ECHO_TAG_STMT]
    );
    // not this one, whose arguments end before it
    assert!(nodes("@custom(a) @csrf").contains(&SyntaxKind::BLADE_DIRECTIVE));
}

#[test]
fn an_escaped_echo_can_hold_directives_as_they_run_first() {
    assert!(nodes("@{{ @csrf }}").contains(&SyntaxKind::BLADE_DIRECTIVE));
}

#[test]
fn an_unclosed_paren_is_text_after_a_directive_without_arguments() {
    assert_eq!(
        nodes("<span @class(['(']></span>"),
        [
            SyntaxKind::INLINE_HTML_STMT,
            SyntaxKind::BLADE_DIRECTIVE,
            SyntaxKind::INLINE_HTML_STMT
        ]
    );
}

// ---------- PHP in a template ----------
blade!(php_tags, "<?php $a = 1; ?>{{ $a }}<?= $a ?>@csrf");
blade!(php_tags_hold_no_blade, "<?php $a = '{{ @if'; ?>");
blade!(php_block, "a @php $a = 1; $b = 2 @endphp b");
blade!(
    php_block_holds_no_blade,
    "@php echo \"{{ a }}\" . '@if' @endphp"
);
blade!(
    php_block_with_html,
    "@php if ($a): @endphp a @php endif; @endphp"
);
blade!(php_statement, "@php($a = 1)\n@php ($b = [')'])");
blade!(
    php_mixed_with_directives,
    "@if ($a) <?php else: ?> b <?php endif; ?>"
);
blade!(php_closes_a_directive, "<?php if ($a): ?> a @else b @endif");
blade!(
    verbatim,
    "a @verbatim {{ $a }} @if($b) @endverbatim {{ $c }}"
);
blade!(verbatim_empty, "@verbatim@endverbatim");

// ---------- conditions ----------
blade!(if_else, "@if ($a) a @elseif ($b) b @else c @endif");
blade!(
    if_nested,
    "@if ($a)@if ($b)@endif @else @if ($c)@endif @endif"
);
blade!(if_any_case, "@If ($a) @ELSEIF($b) @Else @endIf");
blade!(else_then_if_nests, "@if ($a) @else @if ($b) @endif @endif");
blade!(unless, "@unless ($a) a @else b @endunless");
blade!(isset, "@isset($a, $b['c']) a @endisset");
blade!(empty, "@empty($a) a @else b @endempty");
blade!(
    ends_mix_as_they_compile_the_same,
    "@isset($a) @endif @if ($b) @endempty"
);
blade!(
    ignored_parens,
    "@if ($a) a @else (or not) b @endif (done) @csrf (x)"
);

// ---------- loops ----------
blade!(
    foreach,
    "@foreach ($a as $k => [$b, $c]) {{ $b }} @endforeach"
);
blade!(
    foreach_by_reference_any_case,
    "@foreach ($a AS &$b)@endforeach"
);
blade!(forelse, "@forelse ($a as $b) a @empty b @endforelse");
blade!(
    forelse_nested,
    "@forelse ($a as $b)@forelse ($b as $c)@empty x @endforelse @empty y @endforelse"
);
blade!(
    forelse_holds_an_empty_condition,
    "@forelse ($a as $b)@empty($b) a @endempty @empty c @endforelse"
);
blade!(
    forelse_ended_as_foreach,
    "@forelse ($a as $b) a @endforeach"
);
blade!(for_loop, "@for ($i = 0; $i < 3; $i++) {{ $i }} @endfor");
blade!(while_loop, "@while ($a) b @endwhile");
blade!(jumps, "@foreach ($a as $b)@break @continue @endforeach");
blade!(
    jumps_with_levels,
    "@break(2) @break( 2 ) @break(-2) @continue(3)"
);
blade!(
    jumps_with_conditions,
    "@foreach ($a as $b)@break($b)@continue($b > 1)@endforeach"
);

// ---------- switch ----------
blade!(
    switch,
    "@switch($a)\n    @case(1)\n        a\n        @break\n    {{-- b --}}\n    @case('c')\n        @if ($d) e @endif\n    @default\n        f\n@endswitch"
);
blade!(
    switch_nested,
    "@switch($a)@case(1)@switch($b)@default @endswitch @endswitch"
);
blade_err!(
    switch_with_text_before_the_first_case,
    "@switch($a) text @case(1) @endswitch"
);

// ---------- flat directives ----------
blade!(
    section,
    "@extends('a')\n@section('b', 'c')\n@section('d')\n{{ $e }}\n@show\n@yield('f', $g)\n@endsection"
);
blade!(
    stacks,
    "@push('a') b @endpush @pushOnce('c', 'd') e @endPushOnce @stack('f')"
);
blade!(
    flat_ones_unmatched,
    "@endsection @endpush @stop @show @endauth @endcan @endslot @section('a')"
);
blade!(
    includes,
    "@include('a', ['b' => $c]) @includeWhen($d, 'e') @each('f', $g, 'h', 'i')"
);
blade!(
    attributes,
    "<input @checked($a) @disabled(! $b) @class(['c' => $d]) @style(['e'])>"
);
blade!(
    json_js,
    "@json($a, JSON_PRETTY_PRINT) @js(['a' => 1]) @vite(['a.js']) @vite"
);
blade!(
    nested_error_suppression,
    "<span @class(['k' => @empty($v)])></span>"
);
blade!(
    parens_in_strings,
    "@lang(foo('a)')) @include('b', ['c))' => '(d))'])) @each('e', 'f)g')"
);
blade!(cast_arguments, "@section(string) @if ((int) $a) @endif");

// ---------- conditions that stay flat ----------
blade!(auth_with_else, "@auth a @else b @endauth");
blade!(
    auth_inside_if_keeps_its_else,
    "@if ($a) @auth b @else c @endauth @else d @endif"
);
blade!(
    has_section_ends_with_endif,
    "@if ($a) @hasSection('b') c @else d @endif @endif"
);
blade!(
    condition_left_open_doesnt_take_the_endif,
    "@if ($a) @error('b') c @endif"
);
blade!(ended_by_endif_though_it_has_an_end, "@auth a @endif");
blade!(
    else_of_a_condition_we_cant_see,
    "@feature('a') b @else c @endfeature"
);
blade!(
    jump_inside_auth_may_not_run,
    "@foreach ($a as $b) @auth @break @endauth @can('c') @continue(2) @endcan @endforeach"
);
blade!(
    conditions_in_a_loop_in_a_condition,
    "@auth @foreach ($a as $b) @else @endforeach @else @endauth"
);

// ---------- arguments that aren't PHP ----------
#[test]
fn use_takes_what_blade_does() {
    for src in [
        "@use('A\\B', 'C')",
        "@use(A\\B, C)",
        "@use(\\A\\B)",
        "@use(A\\{B, C})",
        "@use('function A\\b', 'C')",
        "@use(function A\\b, C)",
        "@use(const \\A\\{B, C})",
    ] {
        assert_eq!(nodes(src), [SyntaxKind::BLADE_DIRECTIVE], "{src}");
        check_ok(src);
    }
}
blade!(use_statement, "a @use(function A\\b, C) b");

// ---------- what's wrong ----------
blade_err!(unclosed_if, "@if ($a) b");
blade_err!(unclosed_foreach, "@foreach ($a as $b) c");
blade_err!(unclosed_switch, "@switch($a) @case(1) b");
blade_err!(
    unmatched_ends,
    "@endif @endforeach @endfor @endwhile @endswitch @empty"
);
blade_err!(unmatched_case, "@case(1) a @default b");
blade_err!(forelse_without_empty, "@forelse ($a as $b) c @endforelse");
blade_err!(if_without_condition, "@if a @endif");
blade_err!(condition_with_nothing, "@if () a @endif");
blade_err!(malformed_foreach, "@foreach ($a) b @endforeach");
blade_err!(break_with_nothing, "@break()");
blade_err!(directive_argument, "@include('a' 'b')");
blade_err!(php_statement_with_statements, "@php($a = 1;)");

// ---------- it's all still there, whatever it is ----------
fn inputs(data: &str) -> Vec<(&str, &str)> {
    data.split("%%% ")
        .skip(1)
        .map(|record| {
            let (origin, input) = record.split_once('\n').unwrap();
            (origin, input.strip_suffix('\n').unwrap_or(input))
        })
        .collect()
}

const LARAVEL_INPUTS: &str = include_str!("blade/laravel_inputs.txt");
const LARAVEL_REJECTED: &str = include_str!("blade/laravel_rejected.txt");

#[test]
fn laravels_test_inputs_parse() {
    let inputs = inputs(LARAVEL_INPUTS);
    assert!(inputs.len() > 300);
    for (origin, input) in inputs {
        let parse = parse_blade(input);
        assert_eq!(parse.syntax().to_string(), input);
        assert!(
            parse.errors().is_empty(),
            "{origin}: {input}\n{}",
            parse.debug_tree()
        );
    }
}

#[test]
fn what_laravels_tests_show_to_fail_has_errors() {
    for (origin, input) in inputs(LARAVEL_REJECTED) {
        let parse = parse_blade(input);
        assert_eq!(parse.syntax().to_string(), input);
        assert!(!parse.errors().is_empty(), "{origin}: {input}");
    }
}

#[test]
fn every_prefix_of_every_input_is_kept_whole() {
    for (_, input) in inputs(LARAVEL_INPUTS)
        .into_iter()
        .chain(inputs(LARAVEL_REJECTED))
    {
        for end in (0..=input.len()).filter(|end| input.is_char_boundary(*end)) {
            let prefix = &input[..end];
            assert_eq!(parse_blade(prefix).syntax().to_string(), prefix);
        }
    }
}

#[test]
fn random_pieces_are_kept_whole() {
    const PIECES: &[&str] = &[
        "{{",
        "}}",
        "{!!",
        "!!}",
        "{{{",
        "}}}",
        "{{--",
        "--}}",
        "@{{",
        "@@",
        "@",
        "(",
        ")",
        "'",
        "\"",
        "<?php ",
        "<?= ",
        "?>",
        "@php",
        "@endphp",
        "@verbatim",
        "@endverbatim",
        "@if",
        "@elseif",
        "@else",
        "@endif",
        "@unless",
        "@endunless",
        "@isset",
        "@endisset",
        "@empty",
        "@endempty",
        "@foreach",
        "@endforeach",
        "@forelse",
        "@endforelse",
        "@for",
        "@endfor",
        "@while",
        "@endwhile",
        "@switch",
        "@case",
        "@default",
        "@endswitch",
        "@break",
        "@continue",
        "@section",
        "@auth",
        "@endauth",
        "@use",
        "@csrf",
        "@custom",
        " ",
        "\n",
        "$a",
        "$a as $b",
        "1",
        "a",
        "é",
        "👋",
        ";",
        ",",
        ":",
        "{",
        "}",
        "[",
        "]",
        "<<<'A'\n",
        "<<<A\n",
        "A",
        "#",
        "//",
        "/*",
        "*/",
        "isset",
        "empty",
        "(int)",
        "__halt_compiler();",
        "\\",
    ];
    // a small generator of numbers is all this needs, and the failures repeat
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = |below: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % below as u64) as usize
    };
    for _ in 0..20_000 {
        let len = 1 + next(24);
        let src: String = (0..len).map(|_| PIECES[next(PIECES.len())]).collect();
        let parse = std::panic::catch_unwind(|| parse_blade(&src))
            .unwrap_or_else(|_| panic!("panicked on {src:?}"));
        assert_eq!(parse.syntax().to_string(), src);
    }
}
