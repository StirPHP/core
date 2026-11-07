//! Members scripts declare on a class: what PHPDoc's `@property` and `@method` tags would say, had
//! the class been written with them. So that's what they become, and from there they're looked up,
//! hovered and completed like those.
//!
//! Two ways to say it: `magic_members(class_name)` lists what a class has, once per class, and
//! `magic_method(type, name, is_static)` and `magic_property(type, name)` answer for one name on
//! one type, for what depends on the type's arguments or on the name itself. Either can forward a
//! call to another type, as `__call` and `__callStatic` so often do.

use smol_str::SmolStr;
use stir_db::{Db, ScriptFile, Workspace};
use stir_hir::doc::Aliases;
use stir_hir::name::Scope;
use stir_hir::{ClassDef, MethodDef, PropertyDef};
use stir_phpdoc::TypeKind;
use stir_script::ScriptError;
use stir_script::rhai::{Dynamic, Map};
use stir_syntax::TextSize;

use super::{ENGINE, MAGIC_MEMBERS, MAGIC_METHOD, MAGIC_PROPERTY, ScriptFailure, compiled_script};
use crate::MemberKind;

/// A class by its declared name, as a query's key.
#[salsa::interned(debug)]
pub(crate) struct ClassName<'db> {
    #[returns(ref)]
    pub name: SmolStr,
}

/// Where a member a script declares is, for going to its definition: a member of a class.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    pub class: SmolStr,
    pub kind: MemberKind,
    pub name: SmolStr,
}

/// What scripts declare on one class.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScriptMagic {
    pub properties: Vec<PropertyDef>,
    pub methods: Vec<MethodDef>,
    /// The type calls of methods the class hasn't got go to, as written: `static` is the class.
    pub forward_calls: Option<String>,
    /// Where members are declared, by kind and name, when that's not the class.
    pub origins: Vec<(MemberKind, SmolStr, Origin)>,
    /// What properties take when they're written, by name, where that's not what they give.
    pub write_types: Vec<(SmolStr, String)>,
    pub failures: Vec<ScriptFailure>,
}

impl ScriptMagic {
    pub fn write_type(&self, name: &str) -> Option<&str> {
        self.write_types
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.as_str())
    }

    pub fn origin(&self, kind: MemberKind, name: &str) -> Option<&Origin> {
        self.origins
            .iter()
            .find(|(k, n, _)| {
                *k == kind
                    && match kind {
                        MemberKind::Method => n.eq_ignore_ascii_case(name),
                        _ => n == name,
                    }
            })
            .map(|(_, _, o)| o)
    }
}

/// Asks the scripts about a class. Once per class, not per lookup, until a script changes.
#[salsa::tracked(returns(ref))]
pub(crate) fn script_magic<'db>(
    db: &'db dyn Db,
    ws: Workspace,
    class: ClassName<'db>,
) -> ScriptMagic {
    let name = class.name(db);
    let key = name.to_ascii_lowercase();
    let index = stir_hir::def_index(db, ws);
    let Some(&loc) = index
        .classes
        .get(key.as_str())
        .and_then(|locs| locs.first())
    else {
        return ScriptMagic::default();
    };
    let def = &stir_hir::file_items(db, loc.file).classes[loc.index as usize];
    let mut out = ScriptMagic::default();
    for &script in &super::script_index(db, ws).magic {
        if let Err(error) = ask_script(db, script, name, def, &mut out) {
            out.failures
                .push(ScriptFailure::new(script, MAGIC_MEMBERS, error, None));
        }
    }
    out
}

fn ask_script(
    db: &dyn Db,
    script: ScriptFile,
    class: &str,
    def: &ClassDef,
    out: &mut ScriptMagic,
) -> Result<(), ScriptError> {
    let Some(ast) = &compiled_script(db, script).ast else {
        return Ok(());
    };
    let answer = stir_script::call(&ENGINE, ast, MAGIC_MEMBERS, vec![class.into()])?;
    if answer.is_unit() {
        return Ok(());
    }
    let wrong = |what: &str| wrong(&format!("{MAGIC_MEMBERS}(\"{class}\")"), what);
    let Some(map) = answer.try_cast::<Map>() else {
        return Err(wrong("that isn't a map"));
    };
    let mut doc = String::from("/**\n");
    let mut origins = Vec::new();
    let mut writes = Vec::new();
    if let Some(properties) = entries(&wrong, &map, "properties")? {
        for (name, value) in &properties {
            let Some(details) = value.clone().try_cast::<Map>() else {
                continue;
            };
            if let Some(takes) = details.get("write") {
                let takes = text_field(&wrong, name, takes)?;
                writes.push((
                    SmolStr::new(name.trim_start_matches('$')),
                    checked_text(&wrong, &takes, true)?,
                ));
            }
        }
    }
    for (tag, origin) in property_tags(&wrong, &map)? {
        doc.push_str(&format!(" * {tag}\n"));
        origins.extend(origin);
    }
    for (tag, origin) in method_tags(&wrong, &map)? {
        doc.push_str(&format!(" * {tag}\n"));
        origins.extend(origin);
    }
    doc.push_str(" */");
    if out.forward_calls.is_none()
        && let Some(to) = map.get("forward_calls")
    {
        let to = text_field(&wrong, "forward_calls", to)?;
        out.forward_calls = Some(checked_text(&wrong, &to, true)?);
    }

    // what's in a script is as if it was written outside any namespace, so names are complete
    let names: Vec<SmolStr> = def.type_params.iter().map(|p| p.name.clone()).collect();
    let block = stir_hir::doc::parse(&doc, TextSize::new(0));
    let (mut properties, mut methods) = (Vec::new(), Vec::new());
    stir_hir::magic_members(
        &block,
        &Scope::default(),
        &Aliases::default(),
        &names,
        def.ptr,
        &mut properties,
        &mut methods,
    );
    // the first script to declare something is the one that counts
    for p in properties {
        if !out.properties.iter().any(|q| q.name == p.name) {
            if let Some(o) = origins
                .iter()
                .find(|(k, n, _)| *k == MemberKind::Property && n == &p.name)
            {
                out.origins.push(o.clone());
            }
            if let Some(w) = writes.iter().find(|(n, _)| n == &p.name) {
                out.write_types.push(w.clone());
            }
            out.properties.push(p);
        }
    }
    for m in methods {
        if !out
            .methods
            .iter()
            .any(|n| n.name.eq_ignore_ascii_case(&m.name))
        {
            if let Some(o) = origins
                .iter()
                .find(|(k, n, _)| *k == MemberKind::Method && n.eq_ignore_ascii_case(&m.name))
            {
                out.origins.push(o.clone());
            }
            out.methods.push(m);
        }
    }
    Ok(())
}

/// What one script's answer means, for one name on one type.
#[derive(Debug, Clone)]
pub(crate) enum Answer {
    /// It's there, as this `@method` or `@property` tag would declare it.
    Member { tag: String, origin: Option<Origin> },
    /// What it is, is what the same name is on a value of this type, written in terms of the
    /// type asked about (`static`). `fluent`: a result that's that value is the one asked about.
    Forward { to: String, fluent: bool },
}

/// Asks `script`'s `magic_method` or `magic_property` about `name` on a value of the type that
/// `receiver` describes.
pub(crate) fn ask_member(
    script: ScriptFile,
    receiver: Dynamic,
    name: &str,
    is_static: Option<bool>,
) -> Result<Option<Answer>, ScriptError> {
    let (function, kind) = match is_static {
        Some(_) => (MAGIC_METHOD, MemberKind::Method),
        None => (MAGIC_PROPERTY, MemberKind::Property),
    };
    let Some(ast) = stir_db::attached(|db| compiled_script(db, script).ast.clone()).flatten()
    else {
        return Ok(None);
    };
    let mut args = vec![receiver, name.into()];
    args.extend(is_static.map(Dynamic::from));
    let answer = stir_script::call(&ENGINE, &ast, function, args)?;
    if answer.is_unit() {
        return Ok(None);
    }
    let wrong = |what: &str| wrong(&format!("{function}(.., \"{name}\")"), what);
    let checked = checked_name(&wrong, name)?;
    if let Some(map) = answer.clone().try_cast::<Map>()
        && let Some(to) = map.get("forward_to")
    {
        let to = text_field(&wrong, "forward_to", to)?;
        let fluent = map
            .get("fluent")
            .is_some_and(|f| f.as_bool().unwrap_or(false));
        return Ok(Some(Answer::Forward {
            to: checked_text(&wrong, &to, true)?,
            fluent,
        }));
    }
    let (tag, origin) = match kind {
        MemberKind::Method => method_tag(&wrong, &checked, &answer)?,
        _ => property_tag(&wrong, &checked, &answer)?,
    };
    Ok(Some(Answer::Member {
        tag,
        origin: origin.map(|(_, _, o)| o),
    }))
}

fn wrong(call: &str, what: &str) -> ScriptError {
    ScriptError::new(format!(
        "`{call}` should answer `#{{ properties: #{{..}}, methods: #{{..}} }}`, a member, or nothing: {what}"
    ))
}

type Wrong<'a> = dyn Fn(&str) -> ScriptError + 'a;

/// One of the maps in the answer.
fn entries(wrong: &Wrong<'_>, map: &Map, key: &str) -> Result<Option<Map>, ScriptError> {
    let Some(value) = map.get(key) else {
        return Ok(None);
    };
    value
        .clone()
        .try_cast::<Map>()
        .map(Some)
        .ok_or_else(|| wrong(&format!("`{key}` isn't a map")))
}

fn checked_name(wrong: &Wrong<'_>, name: &str) -> Result<String, ScriptError> {
    let name = name.trim_start_matches('$');
    let mut chars = name.chars();
    let starts = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_');
    if starts && chars.all(|c| c.is_alphanumeric() || c == '_') {
        return Ok(name.to_owned());
    }
    Err(wrong(&format!("`{name}` isn't a name for a member")))
}

/// A type or parameter list going into a tag: it has to read as what it is, and stay in the tag.
fn checked_text(wrong: &Wrong<'_>, text: &str, type_text: bool) -> Result<String, ScriptError> {
    if text.contains('\n') || text.contains("*/") {
        return Err(wrong(&format!("`{text}` can't be part of a member")));
    }
    if type_text && let TypeKind::Error(why) = stir_phpdoc::parse_type(text, TextSize::new(0)).kind
    {
        return Err(wrong(&format!("`{text}` isn't a type: {why}")));
    }
    Ok(text.to_owned())
}

fn text_field(wrong: &Wrong<'_>, member: &str, value: &Dynamic) -> Result<String, ScriptError> {
    value
        .clone()
        .into_string()
        .map_err(|_| wrong(&format!("what `{member}` says should be a string")))
}

/// `declared_at: "App\\Post::comments"`, or `"App\\Post::$title"` for a property.
fn origin_of(
    wrong: &Wrong<'_>,
    kind: MemberKind,
    name: &str,
    details: &Map,
) -> Result<Option<(MemberKind, SmolStr, Origin)>, ScriptError> {
    let Some(at) = details.get("declared_at") else {
        return Ok(None);
    };
    let at = text_field(wrong, name, at)?;
    let Some((class, member)) = at.trim_start_matches('\\').rsplit_once("::") else {
        return Err(wrong(&format!(
            "`declared_at` should be `Class::method` or `Class::$property`, not `{at}`"
        )));
    };
    let (target, member) = match member.strip_prefix('$') {
        Some(p) => (MemberKind::Property, p),
        None => (MemberKind::Method, member),
    };
    Ok(Some((
        kind,
        SmolStr::new(name),
        Origin {
            class: SmolStr::new(class.to_ascii_lowercase()),
            kind: target,
            name: SmolStr::new(member),
        },
    )))
}

type Tagged = (String, Option<(MemberKind, SmolStr, Origin)>);

fn property_tag(wrong: &Wrong<'_>, name: &str, value: &Dynamic) -> Result<Tagged, ScriptError> {
    // `"string"`, or `#{ type: "string", read_only: true, declared_at: "Class::method" }`
    let (ty, read_only, origin) = match value.clone().try_cast::<Map>() {
        Some(details) => {
            let read_only = details
                .get("read_only")
                .is_some_and(|r| r.as_bool().unwrap_or(false));
            let ty = details
                .get("type")
                .ok_or_else(|| wrong(&format!("`{name}` has no `type`")))?;
            let origin = origin_of(wrong, MemberKind::Property, name, &details)?;
            (text_field(wrong, name, ty)?, read_only, origin)
        }
        None => (text_field(wrong, name, value)?, false, None),
    };
    let ty = checked_text(wrong, &ty, true)?;
    let tag = if read_only {
        "@property-read"
    } else {
        "@property"
    };
    Ok((format!("{tag} {ty} ${name}"), origin))
}

fn property_tags(wrong: &Wrong<'_>, map: &Map) -> Result<Vec<Tagged>, ScriptError> {
    let Some(properties) = entries(wrong, map, "properties")? else {
        return Ok(Vec::new());
    };
    let mut tags = Vec::new();
    for (name, value) in &properties {
        let name = checked_name(wrong, name)?;
        tags.push(property_tag(wrong, &name, value)?);
    }
    Ok(tags)
}

fn method_tag(wrong: &Wrong<'_>, name: &str, value: &Dynamic) -> Result<Tagged, ScriptError> {
    // `"string"`, or `#{ returns: "string", params: "int $id", is_static: true }`
    let (returns, params, is_static, origin) = match value.clone().try_cast::<Map>() {
        Some(details) => {
            let returns = details
                .get("returns")
                .ok_or_else(|| wrong(&format!("`{name}` has no `returns`")))?;
            let params = details
                .get("params")
                .map(|p| text_field(wrong, name, p))
                .transpose()?;
            let is_static = details
                .get("is_static")
                .is_some_and(|s| s.as_bool().unwrap_or(false));
            let origin = origin_of(wrong, MemberKind::Method, name, &details)?;
            (text_field(wrong, name, returns)?, params, is_static, origin)
        }
        None => (text_field(wrong, name, value)?, None, false, None),
    };
    let returns = checked_text(wrong, &returns, true)?;
    // unless it says what it takes, it takes anything
    let params = match params {
        Some(params) => checked_text(wrong, &params, false)?,
        None => "mixed ...$args".to_owned(),
    };
    let modifier = if is_static { "static " } else { "" };
    Ok((
        format!("@method {modifier}{returns} {name}({params})"),
        origin,
    ))
}

fn method_tags(wrong: &Wrong<'_>, map: &Map) -> Result<Vec<Tagged>, ScriptError> {
    let Some(methods) = entries(wrong, map, "methods")? else {
        return Ok(Vec::new());
    };
    let mut tags = Vec::new();
    for (name, value) in &methods {
        let name = checked_name(wrong, name)?;
        tags.push(method_tag(wrong, &name, value)?);
    }
    Ok(tags)
}

/// The member a tag that [`ask_member`] answered declares, on a class with `def`.
pub(crate) fn member_of_tag(def: &ClassDef, tag: &str) -> (Vec<PropertyDef>, Vec<MethodDef>) {
    let doc = format!("/**\n * {tag}\n */");
    let names: Vec<SmolStr> = def.type_params.iter().map(|p| p.name.clone()).collect();
    let block = stir_hir::doc::parse(&doc, TextSize::new(0));
    let (mut properties, mut methods) = (Vec::new(), Vec::new());
    stir_hir::magic_members(
        &block,
        &Scope::default(),
        &Aliases::default(),
        &names,
        def.ptr,
        &mut properties,
        &mut methods,
    );
    (properties, methods)
}
