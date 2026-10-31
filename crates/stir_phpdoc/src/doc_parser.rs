//! Doc block parser: splits a comment into free text and tags and parses tag values.

use smol_str::SmolStr;
use text_size::TextSize;

use crate::ast::*;
use crate::lexer::{Token, TokenKind as K, lex};
use crate::tags::*;
use crate::type_parser::Stream;

/// Parses a standalone type string (e.g. a `typeAliases` value). `offset` is the absolute
/// position of `src` in its file. Trailing garbage yields an `Error` node.
pub fn parse_type(src: &str, offset: TextSize) -> TypeNode {
    let toks = lex(src);
    let mut st = Stream::new(src, &toks, 0, toks.len(), offset.into());
    // A standalone type may span lines freely.
    st.depth = 1;
    let start = st.start();
    match st.parse_type() {
        Ok(t) if st.peek().is_none() => t,
        Ok(_) => {
            let msg = st.err("expected end of type".into()).msg;
            TypeNode::new(TypeKind::Error(msg), st.abs(start, src.len() as u32))
        }
        Err(e) => TypeNode::new(TypeKind::Error(e.msg), st.abs(start, src.len() as u32)),
    }
}

/// Parses a `/** ... */` doc comment located at `offset` in its file.
pub fn parse_doc_comment(src: &str, offset: TextSize) -> DocBlock {
    let base: u32 = offset.into();
    let toks = lex(src);
    let mut lo = 0;
    let mut hi = toks.len();
    if toks.first().is_some_and(|t| t.kind == K::OpenPhpdoc) {
        lo = 1;
    }
    if let Some(i) = toks[lo..].iter().rposition(|t| t.kind == K::ClosePhpdoc) {
        hi = lo + i;
    }

    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut s = lo;
    for (i, t) in toks.iter().enumerate().take(hi).skip(lo) {
        if t.kind == K::Eol {
            lines.push((s, i));
            s = i + 1;
        }
    }
    lines.push((s, hi));

    let first_sig = |(a, b): (usize, usize)| (a..b).find(|&i| toks[i].kind != K::Ws);
    let is_tag_line = |l: (usize, usize)| first_sig(l).is_some_and(|i| toks[i].kind == K::Tag);
    let first_tag = lines
        .iter()
        .position(|&l| is_tag_line(l))
        .unwrap_or(lines.len());

    let (summary, description) = body_text(src, &toks, &lines[..first_tag], base);

    let mut tags = Vec::new();
    let mut li = first_tag;
    while li < lines.len() {
        let Some(tag_idx) = first_sig(lines[li]) else {
            li += 1;
            continue;
        };
        let mut lj = li + 1;
        while lj < lines.len() && !is_tag_line(lines[lj]) {
            lj += 1;
        }
        let end = lines[lj - 1].1;
        tags.push(parse_tag(src, &toks, tag_idx, end, base));
        li = lj;
    }

    let range = text_size::TextRange::new(offset, offset + TextSize::of(src));
    DocBlock {
        range,
        summary,
        description,
        tags,
    }
}

fn body_text(
    src: &str,
    toks: &[Token],
    lines: &[(usize, usize)],
    base: u32,
) -> (Option<Text>, Option<Text>) {
    // (text, start, end) per non-blank line
    let mut ls: Vec<Option<(&str, u32, u32)>> = lines
        .iter()
        .map(|&(a, b)| {
            let first = (a..b).find(|&i| toks[i].kind != K::Ws)?;
            let last = (a..b).rev().find(|&i| toks[i].kind != K::Ws)?;
            let text = src[toks[a].start as usize..toks[last].end as usize].trim_end();
            Some((text, toks[first].start, toks[last].end))
        })
        .collect();
    while ls.last().is_some_and(Option::is_none) {
        ls.pop();
    }
    let mut i = 0;
    while i < ls.len() && ls[i].is_none() {
        i += 1;
    }
    let ls = &ls[i..];
    let mk = |chunk: &[Option<(&str, u32, u32)>], trim: bool| -> Option<Text> {
        let first = chunk.first()?.as_ref()?;
        let last = chunk.iter().rev().find_map(|l| l.as_ref())?;
        let text = chunk
            .iter()
            .map(|l| match l {
                Some((t, _, _)) if trim => t.trim(),
                Some((t, _, _)) => t,
                None => "",
            })
            .collect::<Vec<_>>()
            .join("\n");
        Some(Text {
            text: if trim {
                text
            } else {
                text.trim_start().to_string()
            },
            range: text_size::TextRange::new((base + first.1).into(), (base + last.2).into()),
        })
    };
    let split = ls.iter().position(Option::is_none).unwrap_or(ls.len());
    let summary = mk(&ls[..split], true);
    let mut j = split;
    while j < ls.len() && ls[j].is_none() {
        j += 1;
    }
    let description = mk(&ls[j..], false);
    (summary, description)
}

fn parse_tag(src: &str, toks: &[Token], tag_idx: usize, end: usize, base: u32) -> Tag {
    let tt = toks[tag_idx];
    let name = &src[tt.start as usize + 1..tt.end as usize];
    let (prefix, base_name) = split_prefix(name);
    let mut st = Stream::new(src, toks, tag_idx + 1, end, base);
    let last = (tag_idx..end)
        .rev()
        .find(|&i| !toks[i].kind.is_trivia())
        .unwrap_or(tag_idx);
    let range = st.abs(tt.start, toks[last].end);

    use TagPrefix as P;
    let prefixed = prefix != P::None;
    let kind = match base_name {
        "param" => param_tag(&mut st),
        "return" => TagKind::Return(type_tag(&mut st)),
        "var" => {
            let ty = type_recover(&mut st);
            let name = st.eat(K::Variable).map(|t| st.variable(t));
            TagKind::Var(VarTag {
                ty,
                name,
                description: rest_text(&st),
            })
        }
        "throws" => TagKind::Throws(type_tag(&mut st)),
        "template" | "template-covariant" | "template-contravariant" => {
            let variance = match base_name {
                "template-covariant" => TemplateVariance::Covariant,
                "template-contravariant" => TemplateVariance::Contravariant,
                _ => TemplateVariance::Invariant,
            };
            template_tag(&mut st, variance)
        }
        "extends" | "template-extends" => TagKind::Extends(type_tag(&mut st)),
        "implements" | "template-implements" => TagKind::Implements(type_tag(&mut st)),
        "use" | "template-use" => TagKind::Use(type_tag(&mut st)),
        "property" | "property-read" | "property-write" => {
            let access = match base_name {
                "property-read" => PropertyAccess::Read,
                "property-write" => PropertyAccess::Write,
                _ => PropertyAccess::ReadWrite,
            };
            let ty = type_recover(&mut st);
            match st.eat(K::Variable) {
                Some(v) => TagKind::Property(PropertyTag {
                    access,
                    ty,
                    name: st.variable(v),
                    description: rest_text(&st),
                }),
                None => invalid(&st, "expected property name"),
            }
        }
        "method" => method_tag(&mut st),
        "type" if prefixed => match st.eat(K::Identifier) {
            Some(n) => {
                let name = st.name(n);
                st.eat(K::Equal);
                let ty = type_recover(&mut st);
                TagKind::TypeAlias(TypeAliasTag { name, ty })
            }
            None => invalid(&st, "expected type alias name"),
        },
        "import-type" if prefixed => import_type_tag(&mut st),
        "deprecated" if !prefixed => TagKind::Deprecated(DescriptionTag {
            description: rest_text(&st),
        }),
        "internal" if !prefixed => TagKind::Internal(DescriptionTag {
            description: rest_text(&st),
        }),
        "assert" | "assert-if-true" | "assert-if-false" if prefixed => {
            let kind = match base_name {
                "assert-if-true" => AssertKind::IfTrue,
                "assert-if-false" => AssertKind::IfFalse,
                _ => AssertKind::Assert,
            };
            assert_tag(&mut st, kind)
        }
        "param-out" => {
            let ty = type_recover(&mut st);
            let name = st.eat(K::Variable).map(|t| st.variable(t));
            TagKind::ParamOut(ParamOutTag {
                ty,
                name,
                description: rest_text(&st),
            })
        }
        "self-out" | "this-out" => TagKind::SelfOut(type_tag(&mut st)),
        "pure" => TagKind::Pure,
        "impure" => TagKind::Impure,
        "ignore" if prefix == P::Phpstan => TagKind::Ignore(ignore_identifiers(&st)),
        "ignore-next-line" if prefix == P::Phpstan => TagKind::IgnoreNextLine,
        "ignore-line" if prefix == P::Phpstan => TagKind::IgnoreLine,
        _ => TagKind::Generic(DescriptionTag {
            description: rest_text(&st),
        }),
    };

    Tag {
        name: SmolStr::new(name),
        name_range: st.abs(tt.start, tt.end),
        prefix,
        range,
        kind,
    }
}

fn invalid(st: &Stream<'_>, msg: &str) -> TagKind {
    TagKind::Invalid {
        message: st.err(msg.into()).msg,
        text: rest_text(st),
    }
}

/// Parses a type; on failure, skips the malformed type (up to top-level whitespace) and
/// returns an `Error` node covering it.
fn type_recover(st: &mut Stream<'_>) -> TypeNode {
    let cp = st.checkpoint();
    let start = st.start();
    match st.parse_type() {
        Ok(t) => t,
        Err(e) => {
            st.restore(cp);
            skip_balanced_word(st);
            TypeNode::new(TypeKind::Error(e.msg), st.range(start))
        }
    }
}

fn skip_balanced_word(st: &mut Stream<'_>) {
    let mut i = st.pos;
    while i < st.end && st.toks[i].kind.is_trivia() {
        i += 1;
    }
    let mut depth = 0i32;
    while i < st.end {
        let k = st.toks[i].kind;
        if k.is_trivia() {
            // Stop at top-level whitespace, or before a `$var` even inside unbalanced
            // brackets so `@param array<int $x` still recovers the parameter name.
            let mut j = i;
            while j < st.end && st.toks[j].kind.is_trivia() {
                j += 1;
            }
            if depth <= 0
                || st
                    .toks
                    .get(j)
                    .is_some_and(|t| j < st.end && t.kind == K::Variable)
            {
                break;
            }
        }
        match k {
            K::OpenParen | K::OpenAngle | K::OpenSquare | K::OpenCurly => depth += 1,
            K::CloseParen | K::CloseAngle | K::CloseSquare | K::CloseCurly => depth -= 1,
            _ => {}
        }
        st.last_end = st.toks[i].end;
        i += 1;
    }
    st.pos = i;
}

/// Remaining tokens of the tag as description text.
fn rest_text(st: &Stream<'_>) -> Option<Text> {
    let toks = st.toks;
    let a = (st.pos..st.end).find(|&i| !toks[i].kind.is_trivia())?;
    let b = (a..st.end).rev().find(|&i| !toks[i].kind.is_trivia())?;
    let mut raw = String::new();
    for t in &toks[a..=b] {
        if t.kind == K::Eol {
            raw.push('\n');
        } else {
            raw.push_str(st.text(*t));
        }
    }
    let text = raw.lines().map(str::trim).collect::<Vec<_>>().join("\n");
    Some(Text {
        text,
        range: st.abs(toks[a].start, toks[b].end),
    })
}

fn type_tag(st: &mut Stream<'_>) -> TypeTag {
    let ty = type_recover(st);
    TypeTag {
        ty,
        description: rest_text(st),
    }
}

fn starts_with_param_var(st: &Stream<'_>) -> bool {
    matches!(
        (st.peek(), st.peek2()),
        (Some(K::Variable), _)
            | (Some(K::Intersection), Some(K::Variable | K::Variadic))
            | (Some(K::Variadic), Some(K::Variable))
    )
}

fn param_tag(st: &mut Stream<'_>) -> TagKind {
    let ty = (!starts_with_param_var(st)).then(|| type_recover(st));
    let by_ref = st.eat(K::Intersection).is_some();
    let variadic = st.eat(K::Variadic).is_some();
    let name = st.eat(K::Variable).map(|t| st.variable(t));
    TagKind::Param(ParamTag {
        ty,
        by_ref,
        variadic,
        name,
        description: rest_text(st),
    })
}

fn template_tag(st: &mut Stream<'_>, variance: TemplateVariance) -> TagKind {
    let start = st.start();
    let Some(n) = st.eat(K::Identifier) else {
        return invalid(st, "expected template name");
    };
    let name = st.name(n);
    let bound =
        (st.eat_ident("of").is_some() || st.eat_ident("as").is_some()).then(|| type_recover(st));
    let lower_bound = st.eat_ident("super").is_some().then(|| type_recover(st));
    let default = st.eat(K::Equal).is_some().then(|| type_recover(st));
    let range = st.range(start);
    TagKind::Template(TemplateTag {
        variance,
        param: TemplateParam {
            name,
            bound,
            lower_bound,
            default,
            range,
        },
        description: rest_text(st),
    })
}

fn import_type_tag(st: &mut Stream<'_>) -> TagKind {
    let Some(n) = st.eat(K::Identifier) else {
        return invalid(st, "expected imported type name");
    };
    let name = st.name(n);
    if st.eat_ident("from").is_none() {
        return invalid(st, "expected 'from'");
    }
    let from = type_recover(st);
    let alias = if st.eat_ident("as").is_some() {
        match st.eat(K::Identifier) {
            Some(a) => Some(st.name(a)),
            None => return invalid(st, "expected alias name"),
        }
    } else {
        None
    };
    TagKind::TypeAliasImport(TypeAliasImportTag { name, from, alias })
}

fn assert_tag(st: &mut Stream<'_>, kind: AssertKind) -> TagKind {
    let negated = st.eat(K::Negated).is_some();
    let equality = st.eat(K::Equal).is_some();
    let ty = type_recover(st);
    let target = match st.peek() {
        Some(K::Variable | K::This) => {
            let t = st.bump();
            let param: SmolStr = st.text(t)[1..].into();
            let mut member = None;
            if st.at(K::Arrow) && !st.ws_before() {
                st.bump();
                if let Some(m) = st.eat(K::Identifier) {
                    let is_method = if st.at(K::OpenParen) {
                        st.bump();
                        st.eat(K::CloseParen);
                        true
                    } else {
                        false
                    };
                    member = Some(AssertMember {
                        name: st.text(m).into(),
                        is_method,
                    });
                }
            }
            Some(AssertTarget {
                param,
                member,
                range: st.range(t.start),
            })
        }
        _ => None,
    };
    TagKind::Assert(AssertTag {
        kind,
        negated,
        equality,
        ty,
        target,
        description: rest_text(st),
    })
}

fn method_tag(st: &mut Stream<'_>) -> TagKind {
    let static_tok = st.eat_ident("static");
    let mut is_static = static_tok.is_some();
    let first = type_recover(st);
    let (return_type, name) = if let Some(n) = st.eat(K::Identifier) {
        (Some(first), st.name(n))
    } else if let Some(id) = first.as_identifier() {
        let name = Name {
            name: id.into(),
            range: first.range,
        };
        let ret = static_tok
            .map(|t| TypeNode::new(TypeKind::Identifier("static".into()), st.tok_range(t)));
        is_static = false;
        (ret, name)
    } else {
        return invalid(st, "expected method name");
    };

    let templates = if st.at(K::OpenAngle) {
        let cp = st.checkpoint();
        match st.parse_template_list() {
            Ok(t) => t,
            Err(_) => {
                st.restore(cp);
                return invalid(st, "invalid method templates");
            }
        }
    } else {
        Vec::new()
    };

    if st.eat(K::OpenParen).is_none() {
        return invalid(st, "expected '('");
    }
    st.depth += 1;
    let mut params = Vec::new();
    while !st.at(K::CloseParen) {
        let pstart = st.start();
        let ty = (!starts_with_param_var(st)).then(|| type_recover(st));
        let by_ref = st.eat(K::Intersection).is_some();
        let variadic = st.eat(K::Variadic).is_some();
        let Some(v) = st.eat(K::Variable) else {
            st.depth -= 1;
            return invalid(st, "expected parameter name");
        };
        let pname = st.variable(v);
        let default = if st.eat(K::Equal).is_some() {
            default_value(st)
        } else {
            None
        };
        params.push(MethodParam {
            ty,
            by_ref,
            variadic,
            name: pname,
            default,
            range: st.range(pstart),
        });
        if st.eat(K::Comma).is_none() {
            break;
        }
    }
    if st.eat(K::CloseParen).is_none() {
        st.depth -= 1;
        return invalid(st, "expected ')'");
    }
    st.depth -= 1;

    TagKind::Method(MethodTag {
        is_static,
        return_type,
        name,
        templates,
        params,
        description: rest_text(st),
    })
}

fn default_value(st: &mut Stream<'_>) -> Option<Text> {
    let mut depth = 0i32;
    let mut first: Option<Token> = None;
    let mut last: Option<Token> = None;
    while let Some(t) = st.peek_tok() {
        match t.kind {
            K::Comma | K::CloseParen if depth == 0 => break,
            K::OpenParen | K::OpenSquare | K::OpenCurly => depth += 1,
            K::CloseParen | K::CloseSquare | K::CloseCurly => depth -= 1,
            _ => {}
        }
        st.bump();
        first.get_or_insert(t);
        last = Some(t);
    }
    let (f, l) = (first?, last?);
    Some(Text {
        text: st.src[f.start as usize..l.end as usize].to_string(),
        range: st.abs(f.start, l.end),
    })
}

/// `@phpstan-ignore id.one, id.two (comment), id.three`
fn ignore_identifiers(st: &Stream<'_>) -> Vec<IgnoreIdentifier> {
    let toks = st.toks;
    let Some(a) = (st.pos..st.end).find(|&i| !toks[i].kind.is_trivia()) else {
        return Vec::new();
    };
    let b = (a..st.end)
        .rev()
        .find(|&i| !toks[i].kind.is_trivia())
        .unwrap_or(a);
    let s = st.src.as_bytes();
    let (mut i, end) = (toks[a].start as usize, toks[b].end as usize);
    let mut out = Vec::new();
    let skip_ws = |i: &mut usize| {
        while *i < end && (s[*i].is_ascii_whitespace() || s[*i] == b'*') {
            *i += 1;
        }
    };
    loop {
        skip_ws(&mut i);
        let start = i;
        while i < end
            && (s[i].is_ascii_alphanumeric() || matches!(s[i], b'_' | b'.' | b'-' | b'\\'))
        {
            i += 1;
        }
        if i == start {
            break;
        }
        let name = SmolStr::new(&st.src[start..i]);
        let name_end = i;
        let mut j = i;
        while j < end && (s[j] == b' ' || s[j] == b'\t') {
            j += 1;
        }
        let mut comment = None;
        if j < end && s[j] == b'(' {
            let cstart = j + 1;
            let mut depth = 1;
            j += 1;
            while j < end && depth > 0 {
                match s[j] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            let cend = if depth == 0 { j - 1 } else { j };
            comment = Some(st.src[cstart..cend].trim().to_string());
            i = j;
        }
        out.push(IgnoreIdentifier {
            name,
            comment,
            range: st.abs(start as u32, name_end as u32),
        });
        skip_ws(&mut i);
        if i < end && s[i] == b',' {
            i += 1;
        } else {
            break;
        }
    }
    out
}
