//! Refactorings of a class's properties: a constructor for them, promoting them to it and back,
//! and accessors.

use stir_checker::php_version as v;
use stir_hir::ast::NodeExt;
use stir_hir::{ClassDef, ClassKind, PropertyDef};
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};

use super::fix_members::{insert_member, insert_property, members_of};
use super::fix_types::{body_indent, is_this_fetch, written_type};
use super::hierarchy::{class_of, method_of};
use super::organize::whole_lines;
use super::types::{Namer, doc_type, has};
use super::{Change, Cx, FileEdit, Kind, Offer};

pub fn offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    let Some(token) = cx.here() else { return };
    let Some(parent) = token.parent() else { return };
    let on_class_name =
        parent.kind() == NAME && parent.parent().is_some_and(|p| p.kind() == CLASS_DECL);
    let on_property = token.kind() == VARIABLE && parent.kind() == PROPERTY_ELEM;
    let on_ctor = parent.kind() == NAME
        && parent.parent().is_some_and(|p| p.kind() == METHOD_DECL)
        && token.text().eq_ignore_ascii_case("__construct");
    let on_promoted =
        token.kind() == VARIABLE && parent.kind() == PARAM && parent.child(MODIFIERS).is_some();
    if on_class_name || on_property {
        let body = parent
            .ancestors()
            .find_map(|n| n.child(CLASS_BODY))
            .or_else(|| parent.ancestors().find(|n| n.kind() == CLASS_BODY));
        if let Some(body) = body {
            constructor_offer(cx, &body, out);
        }
    }
    if on_property {
        accessor_offers(cx, &parent, None, out);
    }
    if on_promoted {
        accessor_offers(cx, &parent, Some(&token), out);
    }
    let ctor = if on_ctor {
        parent.parent()
    } else if on_promoted || (token.kind() == VARIABLE && parent.kind() == PARAM) {
        parent.ancestors().find(|n| n.kind() == METHOD_DECL)
    } else {
        None
    };
    let ctor = ctor.filter(|m| {
        m.name_text()
            .is_some_and(|n| n.eq_ignore_ascii_case("__construct"))
    });
    if let Some(ctor) = ctor {
        promote_offer(cx, &ctor, out);
        demote_offer(cx, &ctor, out);
    }
}

/// `readonly private`, as a property has it.
fn modifiers_text(decl: &SyntaxNode) -> Option<String> {
    let mods = decl.child(MODIFIERS)?;
    Some(
        mods.text()
            .to_string()
            .split_whitespace()
            .map(|w| {
                if w.eq_ignore_ascii_case("var") {
                    "public"
                } else {
                    w
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// A constructor that sets each property that doesn't have a value.
fn constructor_offer<'c>(cx: &'c Cx<'c>, body: &SyntaxNode, out: &mut Vec<Offer<'c>>) {
    let Some(owner) = body.parent().filter(|o| o.kind() == CLASS_DECL) else {
        return;
    };
    let Some(class) = class_of(cx, &body.children().next().unwrap_or_else(|| body.clone())) else {
        return;
    };
    if class.kind != ClassKind::Class
        || class.mods.is_abstract && owner.child(CLASS_BODY).is_none()
        || class
            .methods
            .iter()
            .any(|m| m.name.eq_ignore_ascii_case("__construct"))
    {
        return;
    }
    let Some(model) = cx.ctx().class(&class.name.key()) else {
        return;
    };
    // what's there to call of the parent's: only a constructor nothing has to be passed to
    let parent_call = match cx.ctx().find_method(&model.self_ty(), "__construct") {
        Some((_, m, _)) => {
            if m.sig.params.iter().any(|p| !p.has_default && !p.variadic) {
                return;
            }
            true
        }
        None => false,
    };
    let mut params = Vec::new();
    let mut assigns = Vec::new();
    let mut tags = Vec::new();
    let mut namer = Namer::new(cx, body.text_range().start());
    for decl in body.children_of(PROPERTY_DECL) {
        let is_static = decl
            .child(MODIFIERS)
            .is_some_and(|m| m.has_token(STATIC_KW));
        if is_static {
            continue;
        }
        let ty = written_type(&decl).map(|t| t.text().to_string().trim().to_owned());
        for elem in decl.children_of(PROPERTY_ELEM) {
            if elem.has_token(EQ) || elem.child(PROPERTY_HOOKS).is_some() {
                continue;
            }
            let Some(var) = elem.token(VARIABLE) else {
                continue;
            };
            let name = var.text().to_owned();
            // what PHPDoc says of it, the parameter has to say too
            let doc = class
                .properties
                .iter()
                .find(|p| p.name == name[1..])
                .and_then(|p| p.doc_ty.as_ref())
                .map(|t| cx.ctx().lower_plain(t))
                .and_then(|t| doc_type(&mut namer, &t))
                .filter(|d| ty.as_deref().is_none_or(|n| !n.eq_ignore_ascii_case(d)));
            if let Some(doc) = doc {
                tags.push(format!("@param {doc} {name}"));
            }
            params.push(match &ty {
                Some(t) => format!("{t} {name}"),
                None => name.clone(),
            });
            assigns.push(format!("\t$this->{} = {name};", &name[1..]));
        }
    }
    if params.is_empty() {
        return;
    }
    let call = if parent_call {
        "\tparent::__construct();\n"
    } else {
        ""
    };
    let doc = match tags.as_slice() {
        [] => String::new(),
        [one] => format!("/** {one} */\n"),
        many => format!(
            "/**\n{}\n */\n",
            many.iter()
                .map(|t| format!(" * {t}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    };
    let code = format!(
        "{doc}public function __construct({})\n{{\n{call}{}\n}}",
        params.join(", "),
        assigns.join("\n")
    );
    let members = members_of(body);
    let after = members
        .iter()
        .take_while(|m| m.kind() != METHOD_DECL)
        .last()
        .cloned();
    let mut edits = namer.edits();
    edits.push(insert_member(cx, body, after.as_ref(), &code));
    out.push(Offer::new(
        "constructor.generate",
        "Generate a constructor from the properties",
        Kind::RefactorRewrite,
        move || Ok(Change::new(edits)),
    ));
}

/// The property of `class` named `name` with its declaration, if it's only that one.
fn lone_property<'c>(
    body: &SyntaxNode,
    class: &'c ClassDef,
    name: &str,
) -> Option<(&'c PropertyDef, SyntaxNode)> {
    let def = class
        .properties
        .iter()
        .find(|p| p.name == name && !p.magic)?;
    let decl = body.children_of(PROPERTY_DECL).into_iter().find(|d| {
        d.children_of(PROPERTY_ELEM).iter().any(|e| {
            e.token(VARIABLE)
                .is_some_and(|v| v.text().trim_start_matches('$') == name)
        })
    })?;
    (decl.children_of(PROPERTY_ELEM).len() == 1).then_some((def, decl))
}

fn normalized_type(node: Option<SyntaxNode>) -> Option<String> {
    node.map(|t| t.text().to_string().trim().to_ascii_lowercase())
}

/// `$this->x = $x;`, as the first statements of a constructor, to become promoted parameters.
fn promote_offer<'c>(cx: &'c Cx<'c>, ctor: &SyntaxNode, out: &mut Vec<Offer<'c>>) {
    if !has(cx, v::PROMOTION) {
        return;
    }
    let Some(block) = ctor.child(BLOCK) else {
        return;
    };
    let Some(body) = ctor.parent().filter(|p| p.kind() == CLASS_BODY) else {
        return;
    };
    let Some(class) = class_of(cx, ctor) else {
        return;
    };
    let params = ctor
        .child(PARAM_LIST)
        .map(|l| l.children_of(PARAM))
        .unwrap_or_default();
    let mut edits: Vec<FileEdit> = Vec::new();
    let mut count = 0;
    let mut gone: Vec<SyntaxNode> = Vec::new();
    for stmt in block.children() {
        if stmt.kind() != EXPR_STMT {
            break;
        }
        let Some(assign) = stmt.children().next().filter(|e| e.kind() == ASSIGN_EXPR) else {
            break;
        };
        let parts: Vec<SyntaxNode> = assign.children().collect();
        let [left, right] = parts.as_slice() else {
            break;
        };
        if !assign.has_token(EQ) || assign.has_token(AMP) || right.kind() != VARIABLE_EXPR {
            break;
        }
        let param_name = right.text().to_string();
        let name = param_name.trim_start_matches('$').to_owned();
        if !is_this_fetch(left, &name) {
            break;
        }
        let Some(param) = params
            .iter()
            .find(|p| p.token(VARIABLE).is_some_and(|t| t.text() == param_name))
        else {
            break;
        };
        if param.child(MODIFIERS).is_some() || param.has_token(AMP) || param.has_token(ELLIPSIS) {
            break;
        }
        let Some((def, decl)) = lone_property(&body, class, &name) else {
            break;
        };
        if def.mods.is_static
            || def.has_default
            || decl.child(ATTRIBUTE_LIST).is_some()
            || decl
                .child(PROPERTY_ELEM)
                .is_some_and(|e| e.child(PROPERTY_HOOKS).is_some())
            || normalized_type(written_type(&decl)) != normalized_type(written_type(param))
        {
            break;
        }
        // `int $x = null` can't be promoted: its type isn't what it says
        if param.has_token(EQ)
            && written_type(param).is_some_and(|t| {
                let text = t.text().to_string().to_ascii_lowercase();
                !text.contains("null") && !text.contains('?') && !text.contains("mixed")
            })
            && param
                .children()
                .last()
                .is_some_and(|d| d.text().to_string().trim().eq_ignore_ascii_case("null"))
        {
            break;
        }
        let doc = decl.doc_comment();
        if doc.as_ref().is_some_and(|d| d.text().contains('\n')) {
            break;
        }
        let Some(mods) = modifiers_text(&decl) else {
            break;
        };
        let doc = doc.map_or_else(String::new, |d| format!("{} ", d.text()));
        let start = param
            .children_with_tokens()
            .find(|e| !e.kind().is_trivia())
            .map_or_else(|| param.text_range().start(), |e| e.text_range().start());
        edits.push(cx.insert(start, format!("{doc}{mods} ")));
        // nothing left above it in the class: the blank line after it goes too
        let at_top = members_of(&body)
            .iter()
            .take_while(|m| *m != &decl)
            .all(|m| gone.contains(m));
        edits.push(cx.edit(removal(cx, decl.text_range(), &decl, at_top), ""));
        edits.push(cx.edit(removal(cx, stmt.text_range(), &stmt, false), ""));
        gone.push(decl);
        count += 1;
    }
    if count == 0 {
        return;
    }
    let title = if count == 1 {
        "Promote the property to the constructor".to_owned()
    } else {
        format!("Promote {count} properties to the constructor")
    };
    out.push(Offer::new(
        "constructor.promote",
        title,
        Kind::RefactorRewrite,
        move || Ok(Change::new(edits)),
    ));
}

/// The range that takes a whole declaration or statement away, with the blank line it would leave
/// doubled.
fn removal(cx: &Cx<'_>, range: TextRange, node: &SyntaxNode, at_top: bool) -> TextRange {
    let trimmed = TextRange::new(
        // a comment that's in it goes along; one outside it stays
        node.text_range().start().max(range.start()),
        range.end(),
    );
    let lines = whole_lines(cx, trimmed);
    if lines == trimmed {
        return trimmed;
    }
    let before = &cx.text[..usize::from(lines.start())];
    let after = &cx.text[usize::from(lines.end())..];
    let blank_before = before.ends_with("\n\n") || before.ends_with("\r\n\r\n");
    let blank_after = if after.starts_with("\r\n") {
        2
    } else {
        usize::from(after.starts_with('\n'))
    };
    if blank_after > 0 && (blank_before || at_top || before.trim_end().ends_with('{')) {
        return TextRange::new(
            lines.start(),
            lines.end() + TextSize::from(blank_after as u32),
        );
    }
    lines
}

/// Promoted parameters back to properties and assignments.
fn demote_offer<'c>(cx: &'c Cx<'c>, ctor: &SyntaxNode, out: &mut Vec<Offer<'c>>) {
    let Some(block) = ctor.child(BLOCK) else {
        return;
    };
    let Some(body) = ctor.parent().filter(|p| p.kind() == CLASS_BODY) else {
        return;
    };
    let params = ctor
        .child(PARAM_LIST)
        .map(|l| l.children_of(PARAM))
        .unwrap_or_default();
    let promoted: Vec<&SyntaxNode> = params
        .iter()
        .filter(|p| p.child(MODIFIERS).is_some())
        .collect();
    if promoted.is_empty() {
        return;
    }
    let mut edits = Vec::new();
    let mut properties = Vec::new();
    let mut assigns = Vec::new();
    let mut namer = Namer::new(cx, body.text_range().start());
    for p in &promoted {
        if p.child(ATTRIBUTE_LIST).is_some()
            || p.has_token(AMP)
            || p.child(PROPERTY_HOOKS).is_some()
            || p.doc_comment().is_some()
        {
            return;
        }
        let Some(mods) = modifiers_text(p) else {
            return;
        };
        let Some(var) = p.token(VARIABLE) else { return };
        let Some(mods_node) = p.child(MODIFIERS) else {
            return;
        };
        let ty = written_type(p).map(|t| t.text().to_string().trim().to_owned());
        let name = var.text().to_owned();
        // the constructor's `@param` said what the property is, and now has to say it of it
        let doc = method_of(cx, ctor)
            .and_then(|(_, m)| m.sig.params.iter().find(|q| q.name == name[1..]))
            .and_then(|q| q.doc_ty.as_ref())
            .map(|t| cx.ctx().lower_plain(t))
            .and_then(|t| {
                // a parameter's PHPDoc leaves out the null its default is
                let nullable = ty.as_deref().is_some_and(|n| {
                    let n = n.to_ascii_lowercase();
                    n.starts_with('?') || n.split('|').any(|p| p.trim() == "null")
                });
                let t = if nullable { t.nullable() } else { t };
                doc_type(&mut namer, &t)
            })
            .filter(|d| ty.as_deref().is_none_or(|n| !n.eq_ignore_ascii_case(d)))
            .map_or_else(String::new, |d| format!("/** @var {d} */\n"));
        properties.push(match ty {
            Some(t) => format!("{doc}{mods} {t} {name};"),
            None => format!("{doc}{mods} {name};"),
        });
        assigns.push(format!("$this->{} = {name};", &name[1..]));
        // the modifiers, and the space after them
        let range = mods_node.text_range();
        let text = cx.slice(range);
        let start = range.start() + TextSize::from((text.len() - text.trim_start().len()) as u32);
        let mut end = range.end();
        let rest = &cx.text[usize::from(end)..];
        end += TextSize::from((rest.len() - rest.trim_start_matches([' ', '\t']).len()) as u32);
        edits.push(cx.edit(TextRange::new(start, end), ""));
    }
    edits.extend(namer.edits());
    let members = members_of(&body);
    let last_property = members
        .iter()
        .take_while(|m| *m != ctor)
        .filter(|m| matches!(m.kind(), PROPERTY_DECL | CLASS_CONST_DECL | TRAIT_USE))
        .last()
        .cloned();
    edits.push(match last_property {
        Some(after) => insert_property(cx, &body, Some(&after), &properties.join("\n")),
        None => insert_member(cx, &body, None, &properties.join("\n")),
    });
    let Some(open) = block.token(L_CURLY) else {
        return;
    };
    let Some(close) = block.token(R_CURLY) else {
        return;
    };
    let nl = cx.style.nl;
    let indent = body_indent(cx, &block);
    let lines: Vec<String> = assigns.iter().map(|a| format!("{indent}{a}")).collect();
    let empty = cx
        .slice(TextRange::new(
            open.text_range().end(),
            close.text_range().start(),
        ))
        .trim()
        .is_empty();
    if empty {
        let closing = super::style::indent_at(cx.text, ctor.text_range().start());
        edits.push(cx.edit(
            TextRange::new(open.text_range().end(), close.text_range().start()),
            format!("{nl}{}{nl}{closing}", lines.join(nl)),
        ));
    } else {
        edits.push(cx.insert(open.text_range().end(), format!("{nl}{}", lines.join(nl))));
    }
    out.push(Offer::new(
        "constructor.demote",
        "Convert promoted parameters to properties",
        Kind::RefactorRewrite,
        move || Ok(Change::new(edits)),
    ));
}

fn studly(name: &str) -> String {
    name.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), c.as_str()),
                None => String::new(),
            }
        })
        .collect()
}

/// `getX()`, `setX()` and both, for the property at a place.
fn accessor_offers<'c>(
    cx: &'c Cx<'c>,
    node: &SyntaxNode,
    promoted: Option<&stir_syntax::SyntaxToken>,
    out: &mut Vec<Offer<'c>>,
) {
    let Some(body) = node.ancestors().find(|n| n.kind() == CLASS_BODY) else {
        return;
    };
    let Some(class) = class_of(cx, node) else {
        return;
    };
    if !matches!(class.kind, ClassKind::Class | ClassKind::Trait) {
        return;
    }
    let decl = if promoted.is_some() {
        node.clone()
    } else {
        match node.parent().filter(|p| p.kind() == PROPERTY_DECL) {
            Some(d) => d,
            None => return,
        }
    };
    let var = promoted.cloned().or_else(|| node.token(VARIABLE));
    let Some(var) = var else { return };
    let name = var.text().trim_start_matches('$').to_owned();
    let mods = decl.child(MODIFIERS);
    let is_static = mods.as_ref().is_some_and(|m| m.has_token(STATIC_KW));
    let readonly =
        mods.as_ref().is_some_and(|m| m.has_token(READONLY_KW)) || class.mods.is_readonly;
    let native_ty = written_type(&decl).map(|t| t.text().to_string().trim().to_owned());
    // a promoted one's PHPDoc is the constructor's `@param`
    let doc_ty = class
        .properties
        .iter()
        .find(|p| p.name == name)
        .and_then(|p| p.doc_ty.as_ref())
        .or_else(|| {
            class
                .methods
                .iter()
                .filter(|m| m.name.eq_ignore_ascii_case("__construct"))
                .flat_map(|m| m.sig.params.iter())
                .find(|p| p.promoted.is_some() && p.name == name)
                .and_then(|p| p.doc_ty.as_ref())
        })
        .map(|t| cx.ctx().lower_plain(t));
    let is_bool = native_ty.as_deref().is_some_and(|t| {
        let t = t.to_ascii_lowercase();
        t == "bool" || t == "?bool"
    }) || doc_ty.as_ref().is_some_and(|t| t.to_string() == "bool");
    let word = studly(&name);
    if word.is_empty() {
        return;
    }
    let getter = if is_bool {
        format!("is{word}")
    } else {
        format!("get{word}")
    };
    let setter = format!("set{word}");
    let Some(model) = cx.ctx().class(&class.name.key()) else {
        return;
    };
    let exists = |m: &str| cx.ctx().find_method(&model.self_ty(), m).is_some();
    let want_getter = !exists(&getter);
    let want_setter = !exists(&setter) && !readonly;
    let start = body.text_range().start();
    let bare = name.clone();
    let build_code = move |get: bool, set: bool| -> (String, Vec<FileEdit>) {
        let name = &bare;
        let mut namer = Namer::new(cx, start);
        let doc = doc_ty
            .as_ref()
            .and_then(|t| doc_type(&mut namer, t))
            .filter(|d| {
                native_ty
                    .as_deref()
                    .is_none_or(|n| !n.eq_ignore_ascii_case(d))
            });
        let this = if is_static {
            format!("self::${name}")
        } else {
            format!("$this->{name}")
        };
        let stat = if is_static { "static " } else { "" };
        let mut code = Vec::new();
        if get {
            let ret = native_ty
                .as_ref()
                .map_or_else(String::new, |t| format!(": {t}"));
            let tag = doc
                .as_ref()
                .map_or_else(String::new, |d| format!("/** @return {d} */\n"));
            code.push(format!(
                "{tag}public {stat}function {getter}(){ret}\n{{\n\treturn {this};\n}}"
            ));
        }
        if set {
            let param = native_ty
                .as_ref()
                .map_or_else(|| format!("${name}"), |t| format!("{t} ${name}"));
            let tag = doc
                .as_ref()
                .map_or_else(String::new, |d| format!("/** @param {d} ${name} */\n"));
            code.push(format!(
                "{tag}public {stat}function {setter}({param}): void\n{{\n\t{this} = ${name};\n}}"
            ));
        }
        (code.join("\n\n"), namer.edits())
    };
    let after = members_of(&body).last().cloned();
    let body2 = body.clone();
    let make = move |get: bool, set: bool| {
        let (code, mut edits) = build_code(get, set);
        edits.push(insert_member(cx, &body2, after.as_ref(), &code));
        Change::new(edits)
    };
    let make = std::rc::Rc::new(make);
    if want_getter {
        let f = make.clone();
        out.push(Offer::new(
            format!("accessor.get:{name}"),
            format!("Generate getter `{}()`", accessor_name(is_bool, &name)),
            Kind::RefactorRewrite,
            move || Ok(f(true, false)),
        ));
    }
    if want_setter {
        let f = make.clone();
        out.push(Offer::new(
            format!("accessor.set:{name}"),
            format!("Generate setter `set{}()`", studly(&name)),
            Kind::RefactorRewrite,
            move || Ok(f(false, true)),
        ));
    }
    if want_getter && want_setter {
        let f = make;
        out.push(Offer::new(
            format!("accessor.both:{name}"),
            "Generate getter and setter",
            Kind::RefactorRewrite,
            move || Ok(f(true, true)),
        ));
    }
}

fn accessor_name(is_bool: bool, name: &str) -> String {
    if is_bool {
        format!("is{}", studly(name))
    } else {
        format!("get{}", studly(name))
    }
}
