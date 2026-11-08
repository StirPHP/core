//! The directives of Laravel's Blade compiler, and how each one is read.
//!
//! The names come from the `compile*` methods of Laravel's `BladeCompiler` and its
//! `Concerns\Compiles*` traits. A method's name is how it's found (`method_exists` doesn't mind
//! the case), so `@endIf` and `@ENDIF` are `@endif`. A name that isn't here is text.

use crate::{SyntaxKind, SyntaxKind::*};

/// How a directive's parentheses are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Args {
    /// PHP expressions.
    Php,
    /// Parentheses the compile method never looks at: `@else (or not)` is still `@else`.
    Ignored,
    /// Not PHP, kept as one token: `@use(function App\helper, Alias)`.
    Raw,
}

/// A directive that compiles to the start or the end of an `if` without being one here: these
/// stay flat, but an `@else` inside one is theirs, not that of an `@if` around them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Condition {
    None,
    /// Ends with a directive of its own, which is what's expected of it, so an `@endif` after
    /// `@error` still goes to the `@if` around them.
    Opens,
    /// `@hasSection` has no end of its own: it's `@endif` that ends it.
    OpensUntilEndif,
    Closes,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Directive {
    pub(crate) name: &'static str,
    /// The token it becomes. Control flow takes the keyword of what it compiles to, so that it
    /// builds the same statement that keyword does.
    pub(crate) kind: SyntaxKind,
    pub(crate) args: Args,
    pub(crate) condition: Condition,
}

const fn plain(name: &'static str) -> Directive {
    Directive {
        name,
        kind: BLADE_DIRECTIVE_NAME,
        args: Args::Php,
        condition: Condition::None,
    }
}

/// Takes no arguments.
const fn bare(name: &'static str) -> Directive {
    Directive {
        name,
        kind: BLADE_DIRECTIVE_NAME,
        args: Args::Ignored,
        condition: Condition::None,
    }
}

const fn opens(name: &'static str) -> Directive {
    Directive {
        name,
        kind: BLADE_DIRECTIVE_NAME,
        args: Args::Php,
        condition: Condition::Opens,
    }
}

const fn opens_until_endif(name: &'static str) -> Directive {
    Directive {
        name,
        kind: BLADE_DIRECTIVE_NAME,
        args: Args::Php,
        condition: Condition::OpensUntilEndif,
    }
}

const fn closes(name: &'static str) -> Directive {
    Directive {
        name,
        kind: BLADE_DIRECTIVE_NAME,
        args: Args::Ignored,
        condition: Condition::Closes,
    }
}

/// Control flow with a condition or other PHP in its parentheses.
const fn control(name: &'static str, kind: SyntaxKind) -> Directive {
    Directive {
        name,
        kind,
        args: Args::Php,
        condition: Condition::None,
    }
}

/// Control flow without arguments.
const fn end(name: &'static str, kind: SyntaxKind) -> Directive {
    Directive {
        name,
        kind,
        args: Args::Ignored,
        condition: Condition::None,
    }
}

/// Every directive, sorted by name.
///
/// Every closing directive of an `if` is `ENDIF_KW`, as they all compile to `endif`, so Blade's
/// mixing of them (`@isset(...) ... @endif`) works. A bare `@empty` is decided in the lexer: with
/// parentheses it's `EMPTY_KW`, without them `ENDFOREACH_KW`, as it ends the loop of `@forelse`.
/// `@unless` is `BANG` because it compiles to `if (! (...))`.
const DIRECTIVES: &[Directive] = &[
    bare("append"),
    opens("auth"),
    plain("aware"),
    plain("bool"),
    control("break", BREAK_KW),
    opens("can"),
    opens("canany"),
    opens("cannot"),
    control("case", CASE_KW),
    plain("checked"),
    plain("choice"),
    plain("class"),
    plain("component"),
    plain("componentfirst"),
    opens("context"),
    control("continue", CONTINUE_KW),
    bare("csrf"),
    plain("dd"),
    end("default", DEFAULT_KW),
    plain("disabled"),
    plain("dump"),
    plain("each"),
    end("else", ELSE_KW),
    plain("elseauth"),
    plain("elsecan"),
    plain("elsecanany"),
    plain("elsecannot"),
    plain("elseguest"),
    control("elseif", ELSEIF_KW),
    plain("elsepush"),
    plain("elsepushif"),
    control("empty", EMPTY_KW),
    closes("endauth"),
    closes("endcan"),
    closes("endcanany"),
    closes("endcannot"),
    bare("endcomponent"),
    bare("endcomponentclass"),
    bare("endcomponentfirst"),
    closes("endcontext"),
    end("endempty", ENDIF_KW),
    closes("endenv"),
    closes("enderror"),
    end("endfor", ENDFOR_KW),
    end("endforeach", ENDFOREACH_KW),
    end("endforelse", ENDIF_KW),
    bare("endfragment"),
    closes("endguest"),
    end("endif", ENDIF_KW),
    end("endisset", ENDIF_KW),
    bare("endlang"),
    closes("endonce"),
    bare("endprepend"),
    closes("endprependonce"),
    closes("endproduction"),
    bare("endpush"),
    closes("endpushif"),
    closes("endpushonce"),
    bare("endsection"),
    closes("endsession"),
    bare("endslot"),
    end("endswitch", ENDSWITCH_KW),
    end("endunless", ENDIF_KW),
    end("endwhile", ENDWHILE_KW),
    opens("env"),
    opens("error"),
    plain("extends"),
    plain("extendsfirst"),
    plain("fonts"),
    control("for", FOR_KW),
    control("foreach", FOREACH_KW),
    control("forelse", FOREACH_KW),
    plain("fragment"),
    opens("guest"),
    opens_until_endif("hassection"),
    opens_until_endif("hasstack"),
    control("if", IF_KW),
    plain("include"),
    plain("includefirst"),
    plain("includeif"),
    plain("includeisolated"),
    plain("includeunless"),
    plain("includewhen"),
    plain("inject"),
    control("isset", ISSET_KW),
    plain("js"),
    plain("json"),
    plain("lang"),
    plain("method"),
    opens("once"),
    bare("overwrite"),
    bare("parent"),
    plain("php"),
    plain("prepend"),
    opens("prependonce"),
    Directive {
        name: "production",
        kind: BLADE_DIRECTIVE_NAME,
        args: Args::Ignored,
        condition: Condition::Opens,
    },
    plain("props"),
    plain("push"),
    opens("pushif"),
    opens("pushonce"),
    plain("readonly"),
    plain("required"),
    plain("section"),
    opens_until_endif("sectionmissing"),
    plain("selected"),
    opens("session"),
    bare("show"),
    plain("slot"),
    plain("stack"),
    bare("stop"),
    plain("style"),
    control("switch", SWITCH_KW),
    control("unless", BANG),
    plain("unset"),
    Directive {
        name: "use",
        kind: BLADE_DIRECTIVE_NAME,
        args: Args::Raw,
        condition: Condition::None,
    },
    plain("vite"),
    bare("vitereactrefresh"),
    control("while", WHILE_KW),
    plain("yield"),
];

/// The directive called `name`, whatever its case, without the `@`.
pub(crate) fn directive(name: &str) -> Option<&'static Directive> {
    let name = name.to_ascii_lowercase();
    DIRECTIVES
        .binary_search_by(|d| d.name.cmp(name.as_str()))
        .ok()
        .map(|i| &DIRECTIVES[i])
}

/// The names of Blade's own directives, in lower case and without the `@`.
pub fn directive_names() -> impl Iterator<Item = &'static str> {
    DIRECTIVES.iter().map(|d| d.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_for_its_binary_search() {
        for pair in DIRECTIVES.windows(2) {
            assert!(pair[0].name < pair[1].name, "{}", pair[1].name);
        }
    }

    #[test]
    fn names_are_found_whatever_their_case() {
        assert_eq!(directive("endIf").map(|d| d.kind), Some(ENDIF_KW));
        assert_eq!(directive("PUSHONCE").map(|d| d.name), Some("pushonce"));
        assert!(directive("media").is_none());
    }
}
