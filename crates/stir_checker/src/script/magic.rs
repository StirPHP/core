//! Members scripts declare on a class: what PHPDoc's `@property` and `@method` tags would say, had
//! the class been written with them. So that's what they become, and from there they're looked up,
//! hovered and completed like those.

use smol_str::SmolStr;
use stir_db::{Db, ScriptFile, Workspace};
use stir_hir::doc::Aliases;
use stir_hir::name::Scope;
use stir_hir::{ClassDef, MethodDef, PropertyDef};
use stir_phpdoc::TypeKind;
use stir_script::ScriptError;
use stir_script::rhai::{Dynamic, Map};
use stir_syntax::TextSize;

use super::{ENGINE, MAGIC_MEMBERS, ScriptFailure, compiled_script};

/// A class by its declared name, as a query's key.
#[salsa::interned(debug)]
pub(crate) struct ClassName<'db> {
    #[returns(ref)]
    pub name: SmolStr,
}

/// What scripts declare on one class.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScriptMagic {
    pub properties: Vec<PropertyDef>,
    pub methods: Vec<MethodDef>,
    pub failures: Vec<ScriptFailure>,
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
    let Some(map) = answer.try_cast::<Map>() else {
        return Err(wrong(class, "that isn't a map"));
    };
    let mut doc = String::from("/**\n");
    for tag in property_tags(class, &map)? {
        doc.push_str(&format!(" * {tag}\n"));
    }
    for tag in method_tags(class, &map)? {
        doc.push_str(&format!(" * {tag}\n"));
    }
    doc.push_str(" */");

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
            out.properties.push(p);
        }
    }
    for m in methods {
        if !out
            .methods
            .iter()
            .any(|n| n.name.eq_ignore_ascii_case(&m.name))
        {
            out.methods.push(m);
        }
    }
    Ok(())
}

fn wrong(class: &str, what: &str) -> ScriptError {
    ScriptError::new(format!(
        "`{MAGIC_MEMBERS}(\"{class}\")` should answer `#{{ properties: #{{..}}, methods: #{{..}} }}`, or nothing: {what}"
    ))
}

/// One of the maps in the answer.
fn entries(class: &str, map: &Map, key: &str) -> Result<Option<Map>, ScriptError> {
    let Some(value) = map.get(key) else {
        return Ok(None);
    };
    value
        .clone()
        .try_cast::<Map>()
        .map(Some)
        .ok_or_else(|| wrong(class, &format!("`{key}` isn't a map")))
}

fn checked_name(class: &str, name: &str) -> Result<String, ScriptError> {
    let name = name.trim_start_matches('$');
    let mut chars = name.chars();
    let starts = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_');
    if starts && chars.all(|c| c.is_alphanumeric() || c == '_') {
        return Ok(name.to_owned());
    }
    Err(wrong(class, &format!("`{name}` isn't a name for a member")))
}

/// A type or parameter list going into a tag: it has to read as what it is, and stay in the tag.
fn checked_text(class: &str, text: &str, type_text: bool) -> Result<String, ScriptError> {
    if text.contains('\n') || text.contains("*/") {
        return Err(wrong(class, &format!("`{text}` can't be part of a member")));
    }
    if type_text && let TypeKind::Error(why) = stir_phpdoc::parse_type(text, TextSize::new(0)).kind
    {
        return Err(wrong(class, &format!("`{text}` isn't a type: {why}")));
    }
    Ok(text.to_owned())
}

fn text_field(class: &str, member: &str, value: &Dynamic) -> Result<String, ScriptError> {
    value
        .clone()
        .into_string()
        .map_err(|_| wrong(class, &format!("what `{member}` says should be a string")))
}

fn property_tags(class: &str, map: &Map) -> Result<Vec<String>, ScriptError> {
    let Some(properties) = entries(class, map, "properties")? else {
        return Ok(Vec::new());
    };
    let mut tags = Vec::new();
    for (name, value) in &properties {
        let name = checked_name(class, name)?;
        // `"string"`, or `#{ type: "string", read_only: true }`
        let (ty, read_only) = match value.clone().try_cast::<Map>() {
            Some(details) => {
                let read_only = details
                    .get("read_only")
                    .is_some_and(|r| r.as_bool().unwrap_or(false));
                let ty = details
                    .get("type")
                    .ok_or_else(|| wrong(class, &format!("`{name}` has no `type`")))?;
                (text_field(class, &name, ty)?, read_only)
            }
            None => (text_field(class, &name, value)?, false),
        };
        let ty = checked_text(class, &ty, true)?;
        let tag = if read_only {
            "@property-read"
        } else {
            "@property"
        };
        tags.push(format!("{tag} {ty} ${name}"));
    }
    Ok(tags)
}

fn method_tags(class: &str, map: &Map) -> Result<Vec<String>, ScriptError> {
    let Some(methods) = entries(class, map, "methods")? else {
        return Ok(Vec::new());
    };
    let mut tags = Vec::new();
    for (name, value) in &methods {
        let name = checked_name(class, name)?;
        // `"string"`, or `#{ returns: "string", params: "int $id", is_static: true }`
        let (returns, params, is_static) = match value.clone().try_cast::<Map>() {
            Some(details) => {
                let returns = details
                    .get("returns")
                    .ok_or_else(|| wrong(class, &format!("`{name}` has no `returns`")))?;
                let params = details
                    .get("params")
                    .map(|p| text_field(class, &name, p))
                    .transpose()?;
                let is_static = details
                    .get("is_static")
                    .is_some_and(|s| s.as_bool().unwrap_or(false));
                (text_field(class, &name, returns)?, params, is_static)
            }
            None => (text_field(class, &name, value)?, None, false),
        };
        let returns = checked_text(class, &returns, true)?;
        // unless it says what it takes, it takes anything
        let params = match params {
            Some(params) => checked_text(class, &params, false)?,
            None => "mixed ...$args".to_owned(),
        };
        let modifier = if is_static { "static " } else { "" };
        tags.push(format!("@method {modifier}{returns} {name}({params})"));
    }
    Ok(tags)
}
