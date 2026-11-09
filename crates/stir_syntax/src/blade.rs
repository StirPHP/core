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

/// Is `name`, without the `@`, one of Blade's own directives?
pub fn is_directive(name: &str) -> bool {
    directive(name).is_some()
}

/// What a directive that compiles to part of an `if` does, for those that stay flat in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatPart {
    Opens,
    Else,
    Closes,
}

/// The part of a flat condition the directive `name` is, without the `@`.
pub fn flat_part(name: &str) -> Option<FlatPart> {
    let d = directive(name)?;
    match d.condition {
        Condition::Opens | Condition::OpensUntilEndif => Some(FlatPart::Opens),
        Condition::Closes => Some(FlatPart::Closes),
        Condition::None if d.kind == ENDIF_KW => Some(FlatPart::Closes),
        Condition::None if d.name.starts_with("else") => Some(FlatPart::Else),
        Condition::None => None,
    }
}

/// Where the parts of a tag at the start of a text are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tag {
    /// The length of `<x-`, or of `</x-`.
    pub(crate) start: usize,
    /// Where the name ends, and for a tag that opens, the attributes start.
    pub(crate) name_end: usize,
    /// Where the attributes end, and `>` or `/>` starts.
    pub(crate) attrs_end: usize,
    pub(crate) end: usize,
    pub(crate) closes: bool,
}

/// Regular expressions' `\s`.
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_tag_name_byte(b: u8) -> bool {
    is_word(b) || matches!(b, b'-' | b':' | b'.')
}

fn is_attr_name_byte(b: u8) -> bool {
    is_word(b) || matches!(b, b'-' | b':' | b'.' | b'@' | b'%')
}

/// `<x-`, with what's allowed around the `<` and the `/`.
fn tag_prefix(b: &[u8]) -> Option<(usize, bool)> {
    let mut i = 1;
    let closes = b.get(1) == Some(&b'/');
    if closes {
        i += 1;
    }
    while b.get(i).copied().is_some_and(is_space) {
        i += 1;
    }
    (b.get(i) == Some(&b'x') && matches!(b.get(i + 1), Some(b'-' | b':')))
        .then_some((i + 2, closes))
}

/// Past the first `)` that balances the `(` at `open`. As in Laravel's regular expression, this
/// counts them all, those in a string too.
pub(crate) fn balanced_parens(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0u32;
    for (i, c) in b.iter().enumerate().skip(open) {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// `@class(...)` or `@style(...)` at `at`: where it ends.
pub(crate) fn class_or_style_end(b: &[u8], at: usize) -> Option<usize> {
    let rest = &b[at..];
    if rest.starts_with(b"@class(") || rest.starts_with(b"@style(") {
        balanced_parens(b, at + 6)
    } else {
        None
    }
}

/// `{{ $attributes ... }}` at `at`: where it ends. What's in it can't have a `}`.
pub(crate) fn attribute_bag_end(b: &[u8], at: usize) -> Option<usize> {
    if !b[at..].starts_with(b"{{") {
        return None;
    }
    let mut i = at + 2;
    while b.get(i).copied().is_some_and(is_space) {
        i += 1;
    }
    if !b[i..].starts_with(b"$attributes") {
        return None;
    }
    let close = i + b[i..].iter().position(|c| *c == b'}')?;
    b[close..].starts_with(b"}}").then_some(close + 2)
}

/// `:$name` at `at`: where it ends.
pub(crate) fn short_attr_end(b: &[u8], at: usize) -> Option<usize> {
    if !b[at..].starts_with(b":$") {
        return None;
    }
    let len = b[at + 2..].iter().take_while(|c| is_word(**c)).count();
    (len > 0).then_some(at + 2 + len)
}

/// Past a Blade comment at `at`, which Laravel has taken out before it looks for tags.
pub(crate) fn comment_end(b: &[u8], at: usize) -> Option<usize> {
    if !b[at..].starts_with(b"{{--") {
        return None;
    }
    let text = &b[at + 4..];
    text.windows(4)
        .position(|w| w == b"--}}")
        .map(|i| at + 4 + i + 4)
}

/// Finds where the attributes of a tag end, as Laravel's regular expressions do: this goes through
/// the same choices in the same order, and goes back on them the same way, so what's a tag there
/// is one here. What's found not to lead anywhere is remembered, which is what keeps it from
/// taking long on what's only a long way from being a tag.
struct AttrMatcher<'a> {
    b: &'a [u8],
    closing_itself: bool,
    dead_ends: std::collections::HashSet<usize>,
}

/// More attributes than anyone writes: what the matching in PHP would give up on too.
const MAX_ATTRS: u32 = 2000;

impl AttrMatcher<'_> {
    /// From `at`, on to the end of the tag: where the attributes end. Spaces at the end count as
    /// being in them.
    fn rest(&mut self, at: usize, depth: u32) -> Option<usize> {
        if depth > MAX_ATTRS || self.dead_ends.contains(&at) {
            return None;
        }
        let (next, spaced) = self.skip_space(at);
        if spaced
            && next < self.b.len()
            && let Some(end) = self.attribute(next, depth)
        {
            return Some(end);
        }
        if self.ends_at(next) {
            return Some(next);
        }
        self.dead_ends.insert(at);
        None
    }

    fn skip_space(&self, mut at: usize) -> (usize, bool) {
        let mut spaced = false;
        loop {
            if self.b.get(at).copied().is_some_and(is_space) {
                spaced = true;
                at += 1;
            } else if at < self.b.len()
                && let Some(end) = comment_end(self.b, at)
            {
                at = end;
            } else {
                return (at, spaced);
            }
        }
    }

    fn ends_at(&self, at: usize) -> bool {
        if self.closing_itself {
            return self.b[at..].starts_with(b"/>");
        }
        self.b.get(at) == Some(&b'>') && !matches!(self.b[at - 1], b'/' | b'=' | b'-')
    }

    fn attribute(&mut self, at: usize, depth: u32) -> Option<usize> {
        let whole = class_or_style_end(self.b, at)
            .or_else(|| attribute_bag_end(self.b, at))
            .or_else(|| short_attr_end(self.b, at));
        if let Some(end) = whole
            && let Some(found) = self.rest(end, depth + 1)
        {
            return Some(found);
        }
        let name = self.b[at..]
            .iter()
            .take_while(|c| is_attr_name_byte(**c))
            .count();
        if name == 0 {
            return None;
        }
        let name_end = at + name;
        if self.b.get(name_end) == Some(&b'=') {
            let value = name_end + 1;
            match self.b.get(value) {
                Some(q @ (b'"' | b'\'')) => {
                    if let Some(len) = self.b[value + 1..].iter().position(|c| c == q)
                        && let Some(found) = self.rest(value + 1 + len + 1, depth + 1)
                    {
                        return Some(found);
                    }
                }
                Some(_) => {
                    let len = self.b[value..]
                        .iter()
                        .take_while(|c| !matches!(c, b'\'' | b'"' | b'=' | b'<' | b'>'))
                        .count();
                    for end in (value + 1..=value + len).rev() {
                        if let Some(found) = self.rest(end, depth + 1) {
                            return Some(found);
                        }
                    }
                }
                None => {}
            }
        }
        self.rest(name_end, depth + 1)
    }
}

/// The tag at the start of `text`, which has a `<`, if it's one by Laravel's compiler. Whatever
/// isn't is text there, and is here.
pub(crate) fn tag_at(text: &str) -> Option<Tag> {
    let b = text.as_bytes();
    let (start, closes) = tag_prefix(b)?;
    let name_len = b[start..]
        .iter()
        .take_while(|c| is_tag_name_byte(**c))
        .count();
    let name_end = start + name_len;
    if closes {
        // `</x-slot` is any end of a slot: this comes first in Laravel, and is all it asks of it
        if b[start..].starts_with(b"slot") {
            let close = name_end + b[name_end..].iter().position(|c| *c == b'>')?;
            return Some(Tag {
                start,
                name_end,
                attrs_end: close,
                end: close + 1,
                closes,
            });
        }
        let close = name_end + b[name_end..].iter().take_while(|c| is_space(**c)).count();
        return (b.get(close) == Some(&b'>')).then_some(Tag {
            start,
            name_end,
            attrs_end: close,
            end: close + 1,
            closes,
        });
    }
    for closing_itself in [true, false] {
        let mut matcher = AttrMatcher {
            b,
            closing_itself,
            dead_ends: std::collections::HashSet::new(),
        };
        if let Some(attrs_end) = matcher.rest(name_end, 0) {
            return Some(Tag {
                start,
                name_end,
                attrs_end,
                end: attrs_end + if closing_itself { 2 } else { 1 },
                closes,
            });
        }
    }
    None
}

/// Is this name of an attribute, as written, one whose value is PHP?
pub fn attr_is_bound(name: &str) -> bool {
    let name = name.strip_prefix("bind:").unwrap_or_else(|| {
        name.strip_prefix(':')
            .filter(|rest| !rest.starts_with(':'))
            .unwrap_or("")
    });
    !name.is_empty() && name.bytes().all(|c| is_tag_name_byte(c) || c == b'@')
}

/// The name of an attribute without what says how it's read.
pub fn attr_key(name: &str) -> &str {
    if let Some(rest) = name.strip_prefix("bind:") {
        return rest;
    }
    if let Some(rest) = name.strip_prefix("::") {
        return rest;
    }
    if attr_is_bound(name) {
        return &name[1..];
    }
    name
}

/// Laravel's `Str::camel`.
pub fn camel(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut upper = false;
    for c in key.chars() {
        if matches!(c, '-' | '_' | ' ') {
            upper = !out.is_empty() || upper;
            continue;
        }
        if upper {
            out.extend(c.to_uppercase());
        } else if out.is_empty() {
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
        upper = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tag's text, split where it has its parts.
    fn parts(text: &str) -> Option<(&str, &str, &str)> {
        let tag = tag_at(text)?;
        Some((
            &text[tag.start..tag.name_end],
            &text[tag.name_end..tag.attrs_end],
            &text[tag.attrs_end..tag.end],
        ))
    }

    #[test]
    fn tags_are_what_laravels_regular_expressions_take_them_for() {
        assert_eq!(parts("<x-a/>"), Some(("a", "", "/>")));
        assert_eq!(parts("<x-a>"), Some(("a", "", ">")));
        assert_eq!(parts("< x:a.b-c::d />"), Some(("a.b-c::d", " ", "/>")));
        assert_eq!(parts("<x-a b=c/>"), Some(("a", " b=c", "/>")));
        assert_eq!(
            parts("<x-a b=c d=\"e\" />"),
            Some(("a", " b=c d=\"e\" ", "/>"))
        );
        assert_eq!(parts("</x-a >"), Some(("a", " ", ">")));
        assert_eq!(
            parts("</x-slot name=\"a\">"),
            Some(("slot", " name=\"a\"", ">"))
        );
        assert_eq!(
            parts("<x-a @class(['b' => ($c)]) />"),
            Some(("a", " @class(['b' => ($c)]) ", "/>"))
        );
        assert_eq!(
            parts("<x-a {{ $attributes->c() }}>"),
            Some(("a", " {{ $attributes->c() }}", ">"))
        );
        assert_eq!(
            parts("<x-a {{-- \" --}} b>"),
            Some(("a", " {{-- \" --}} b", ">"))
        );
    }

    #[test]
    fn what_laravel_doesnt_take_for_a_tag() {
        for text in [
            "<x>",
            "<xa>",
            "<x->",
            "<x-a->",
            "<x-a b=>",
            "<x-a b=\"c\"d=\"e\">",
            "<x-a {{ $b }}>",
            "<x-a @if($b)>",
            "<x-a @class(['b)'])>",
            "<x-a b=\"c>",
            "<x-a",
            "</x-a b>",
            "<x-a é>",
        ] {
            assert_eq!(tag_at(text), None, "{text}");
        }
    }

    #[test]
    fn a_long_way_from_being_a_tag_doesnt_take_long() {
        let text = format!("<x-a{}", " b=c".repeat(5000));
        assert_eq!(tag_at(&text), None);
    }

    #[test]
    fn attributes_are_named_as_laravel_has_them() {
        assert!(attr_is_bound(":a-b"));
        assert!(attr_is_bound("bind:a"));
        assert!(!attr_is_bound("::a"));
        assert!(!attr_is_bound(":"));
        assert!(!attr_is_bound("a"));
        assert_eq!(attr_key(":a-b"), "a-b");
        assert_eq!(attr_key("::a"), "a");
        assert_eq!(attr_key("bind:a"), "a");
        assert_eq!(attr_key("wire:click"), "wire:click");
        assert_eq!(camel("user-id"), "userId");
        assert_eq!(camel("user_id"), "userId");
        assert_eq!(camel("userId"), "userId");
        assert_eq!(camel("Title"), "title");
    }

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
