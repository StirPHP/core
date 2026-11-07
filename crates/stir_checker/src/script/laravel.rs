//! What scripts can ask about a Laravel project that would take reading many files, done here
//! rather than in a script for speed: the columns its migrations give each table
//! (`laravel_columns`), and the values of its configuration (`laravel_config`). Both read only the
//! project's own files, as written: nothing is run, so a migration that builds its columns in a
//! loop, or a configuration value computed at runtime, isn't known.

use std::collections::BTreeMap;

use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_script::rhai::{Array, Dynamic, Engine, Map};
use stir_syntax::SyntaxKind::*;
use stir_syntax::SyntaxNode;

use super::reflect::{Place, literal};
use crate::Ctx;

/// A column of a table, as its migrations leave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    /// A PHPDoc type, without `null`.
    pub ty: String,
    pub nullable: bool,
}

/// Every table the project's migrations create, with its columns in order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Schema {
    pub tables: BTreeMap<String, Vec<Column>>,
}

fn is_migration(path: &std::path::Path) -> bool {
    let path = path.to_string_lossy().replace('\\', "/");
    path.contains("/database/migrations/") && path.ends_with(".php")
}

/// What the migrations in `database/migrations` do, in the order they run: by file name.
#[salsa::tracked(returns(ref))]
pub fn laravel_schema(db: &dyn Db, ws: Workspace) -> Schema {
    let mut files: Vec<SourceFile> = ws
        .files(db)
        .iter()
        .copied()
        .filter(|f| f.origin(db) == FileOrigin::Source && is_migration(f.path(db)))
        .collect();
    files.sort_by_key(|f| f.path(db).file_name().map(|n| n.to_owned()));
    let mut schema = Schema::default();
    for file in files {
        let root = stir_db::parse(db, file).syntax();
        for call in root
            .descendants()
            .filter(|n| matches!(n.kind(), STATIC_CALL_EXPR | METHOD_CALL_EXPR))
        {
            read_schema_call(&call, &mut schema);
        }
    }
    schema
}

/// `Schema::create('posts', function (Blueprint $table) { .. })`, `Schema::table(..)`,
/// `Schema::rename('a', 'b')` and `Schema::drop('a')`.
fn read_schema_call(call: &SyntaxNode, schema: &mut Schema) {
    let Some(callee) = call.children().next() else {
        return;
    };
    let Some(method) = name_of(&callee) else {
        return;
    };
    let receiver = callee.children().next().map(|r| r.text().to_string());
    if !receiver.is_some_and(|r| r.contains("Schema")) {
        return;
    }
    let args = arguments(call);
    let Some(table) = args.first().and_then(string_value) else {
        return;
    };
    match method.as_str() {
        "create" | "table" => {
            let Some(closure) = args
                .get(1)
                .filter(|a| matches!(a.kind(), CLOSURE_EXPR | ARROW_FN_EXPR))
            else {
                return;
            };
            let columns = schema.tables.entry(table).or_default();
            read_blueprint(closure, columns);
        }
        "rename" => {
            if let Some(to) = args.get(1).and_then(string_value)
                && let Some(columns) = schema.tables.remove(&table)
            {
                schema.tables.insert(to, columns);
            }
        }
        "drop" | "dropIfExists" => {
            schema.tables.remove(&table);
        }
        _ => {}
    }
}

/// The statements of a migration's closure, on its blueprint.
fn read_blueprint(closure: &SyntaxNode, columns: &mut Vec<Column>) {
    let Some(blueprint) = closure
        .descendants_with_tokens()
        .find(|t| t.kind() == VARIABLE)
        .map(|t| t.to_string())
    else {
        return;
    };
    let Some(body) = closure.children().find(|c| c.kind() == BLOCK) else {
        return;
    };
    for stmt in body.children().filter(|c| c.kind() == EXPR_STMT) {
        let Some(expr) = stmt.children().next() else {
            continue;
        };
        // `$table->string('title')->nullable()`: the calls from the outermost in
        let mut chain = Vec::new();
        let mut at = expr;
        let on_blueprint = loop {
            if at.kind() != METHOD_CALL_EXPR {
                break false;
            }
            let Some(fetch) = at.children().next() else {
                break false;
            };
            let Some(name) = name_of(&fetch) else {
                break false;
            };
            chain.push((name, arguments(&at)));
            let Some(receiver) = fetch.children().next() else {
                break false;
            };
            if receiver.kind() == VARIABLE_EXPR && receiver.text().to_string().trim() == blueprint {
                break true;
            }
            at = receiver;
        };
        if !on_blueprint {
            continue;
        }
        let Some((method, args)) = chain.pop() else {
            continue;
        };
        let nullable = chain.iter().any(|(m, a)| {
            m == "nullable"
                && !a
                    .first()
                    .is_some_and(|v| v.text().to_string().trim() == "false")
        });
        let changes = chain.iter().any(|(m, _)| m == "change");
        apply(&method, &args, nullable, changes, columns);
    }
}

fn set(columns: &mut Vec<Column>, name: String, ty: &str, nullable: bool) {
    let column = Column {
        name,
        ty: ty.to_owned(),
        nullable,
    };
    match columns.iter_mut().find(|c| c.name == column.name) {
        Some(old) => *old = column,
        None => columns.push(column),
    }
}

const CARBON: &str = "\\Illuminate\\Support\\Carbon";

/// What a blueprint's method does to the table's columns.
fn apply(
    method: &str,
    args: &[SyntaxNode],
    nullable: bool,
    _changes: bool,
    columns: &mut Vec<Column>,
) {
    let first = args.first().and_then(string_value);
    let named = |default: &str| first.clone().unwrap_or_else(|| default.to_owned());
    let ty = match method {
        "id" => {
            set(columns, named("id"), "int", false);
            return;
        }
        "increments"
        | "bigIncrements"
        | "mediumIncrements"
        | "smallIncrements"
        | "tinyIncrements"
        | "integerIncrements"
        | "integer"
        | "bigInteger"
        | "mediumInteger"
        | "smallInteger"
        | "tinyInteger"
        | "unsignedInteger"
        | "unsignedBigInteger"
        | "unsignedMediumInteger"
        | "unsignedSmallInteger"
        | "unsignedTinyInteger"
        | "foreignId"
        | "year" => "int",
        "string" | "char" | "text" | "mediumText" | "longText" | "tinyText" | "uuid" | "ulid"
        | "ipAddress" | "macAddress" | "enum" | "set" | "foreignUuid" | "foreignUlid"
        | "binary" | "json" | "jsonb" | "date" | "dateTime" | "dateTimeTz" | "time" | "timeTz"
        | "timestamp" | "timestampTz" | "geometry" | "geography" | "vector" => "string",
        "float" | "double" | "decimal" | "unsignedDecimal" | "unsignedFloat" | "unsignedDouble" => {
            "float"
        }
        "boolean" => "bool",
        "timestamps" | "timestampsTz" | "nullableTimestamps" => {
            set(columns, "created_at".into(), CARBON, true);
            set(columns, "updated_at".into(), CARBON, true);
            return;
        }
        "softDeletes" | "softDeletesTz" | "softDeletesDatetime" => {
            set(columns, named("deleted_at"), CARBON, true);
            return;
        }
        "rememberToken" => {
            set(columns, "remember_token".into(), "string", true);
            return;
        }
        "morphs"
        | "nullableMorphs"
        | "uuidMorphs"
        | "nullableUuidMorphs"
        | "ulidMorphs"
        | "nullableUlidMorphs"
        | "numericMorphs"
        | "nullableNumericMorphs" => {
            let Some(name) = first else { return };
            let maybe = method.starts_with("nullable");
            let id = if method.contains("Uuid") || method.contains("Ulid") {
                "string"
            } else {
                "int"
            };
            set(columns, format!("{name}_type"), "string", maybe);
            set(columns, format!("{name}_id"), id, maybe);
            return;
        }
        "foreignIdFor" => {
            let column = args.get(1).and_then(string_value).or_else(|| {
                // `foreignIdFor(User::class)` is `user_id`
                let class = args.first()?.text().to_string();
                let class = class.trim().strip_suffix("::class")?;
                let base = class.rsplit('\\').next()?;
                Some(format!("{}_id", snake(base)))
            });
            if let Some(column) = column {
                set(columns, column, "int", nullable);
            }
            return;
        }
        "dropColumn" | "dropColumns" => {
            let mut names: Vec<String> = Vec::new();
            for a in args {
                if a.kind() == ARRAY_EXPR {
                    names.extend(
                        a.children()
                            .filter_map(|e| e.children().last())
                            .filter_map(|v| string_value(&v)),
                    );
                } else {
                    names.extend(string_value(a));
                }
            }
            columns.retain(|c| !names.contains(&c.name));
            return;
        }
        "dropTimestamps" | "dropTimestampsTz" => {
            columns.retain(|c| c.name != "created_at" && c.name != "updated_at");
            return;
        }
        "dropSoftDeletes" | "dropSoftDeletesTz" => {
            columns.retain(|c| c.name != "deleted_at");
            return;
        }
        "renameColumn" => {
            if let (Some(from), Some(to)) = (first, args.get(1).and_then(string_value))
                && let Some(c) = columns.iter_mut().find(|c| c.name == from)
            {
                c.name = to;
            }
            return;
        }
        _ => return,
    };
    if let Some(name) = first {
        set(columns, name, ty, nullable);
    }
}

fn name_of(fetch: &SyntaxNode) -> Option<String> {
    fetch
        .children()
        .find(|c| c.kind() == NAME)
        .map(|n| n.text().to_string().trim().to_owned())
}

/// The expressions passed to a call, in order.
fn arguments(call: &SyntaxNode) -> Vec<SyntaxNode> {
    call.children()
        .find(|c| c.kind() == ARG_LIST)
        .map(|l| {
            l.children()
                .filter(|a| matches!(a.kind(), ARG | NAMED_ARG))
                .filter_map(|a| a.children().last())
                .collect()
        })
        .unwrap_or_default()
}

fn string_value(node: &SyntaxNode) -> Option<String> {
    if node.kind() != STRING_LITERAL {
        return None;
    }
    let text = node.text().to_string();
    let text = text.trim();
    if text.starts_with('"') && text.contains('$') {
        return None;
    }
    Some(stir_hir::type_ref::unquote(text))
}

/// `BlogPost` is `blog_post`.
pub(crate) fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The plural of an English word, as Laravel's `Str::plural` mostly has it.
pub(crate) fn plural(word: &str) -> String {
    const SAME: &[&str] = &[
        "audio",
        "data",
        "deer",
        "equipment",
        "feedback",
        "fish",
        "information",
        "media",
        "metadata",
        "money",
        "news",
        "rice",
        "series",
        "sheep",
        "species",
        "staff",
    ];
    const IRREGULAR: &[(&str, &str)] = &[
        ("child", "children"),
        ("foot", "feet"),
        ("goose", "geese"),
        ("man", "men"),
        ("mouse", "mice"),
        ("ox", "oxen"),
        ("person", "people"),
        ("tooth", "teeth"),
        ("woman", "women"),
    ];
    let lower = word.to_ascii_lowercase();
    if SAME.contains(&lower.as_str()) {
        return word.to_owned();
    }
    if let Some((_, many)) = IRREGULAR.iter().find(|(one, _)| *one == lower) {
        return (*many).to_owned();
    }
    let consonant_y = lower.ends_with('y')
        && !lower
            .chars()
            .rev()
            .nth(1)
            .is_some_and(|c| "aeiou".contains(c));
    if consonant_y {
        return format!("{}ies", &word[..word.len() - 1]);
    }
    if ["s", "x", "z", "ch", "sh"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        return format!("{word}es");
    }
    format!("{word}s")
}

/// The table of a model named `class` by Laravel's convention: `blog_posts` for `App\BlogPost`.
fn table_of(class: &str) -> String {
    let base = class.rsplit('\\').next().unwrap_or(class);
    let snake = snake(base);
    match snake.rsplit_once('_') {
        Some((head, last)) => format!("{head}_{}", plural(last)),
        None => plural(&snake),
    }
}

/// `laravel_columns(model, table)`: the columns of `table`, or of the table `model` has by
/// convention, as `#{ name, type, nullable }`. None for a table no migration creates.
fn columns(model: &str, table: Dynamic) -> Array {
    let table = table
        .into_string()
        .ok()
        .unwrap_or_else(|| table_of(model.trim_start_matches('\\')));
    stir_db::attached(|db| {
        let Some(ws) = Workspace::try_get(db) else {
            return Array::new();
        };
        let schema = laravel_schema(db, ws);
        let Some(columns) = schema.tables.get(&table) else {
            return Array::new();
        };
        columns
            .iter()
            .map(|c| {
                let mut column = Map::new();
                column.insert("name".into(), c.name.as_str().into());
                let ty = if c.nullable {
                    format!("?{}", c.ty)
                } else {
                    c.ty.clone()
                };
                column.insert("type".into(), ty.into());
                column.insert("nullable".into(), c.nullable.into());
                Dynamic::from(column)
            })
            .collect()
    })
    .unwrap_or_default()
}

/// `laravel_config("auth.providers.users.model")`: the value in `config/auth.php`, where it's
/// written out literally, or the default of the `env()` it's read from.
fn config(key: &str) -> Dynamic {
    let mut parts = key.split('.');
    let Some(file_name) = parts.next() else {
        return Dynamic::UNIT;
    };
    // where Laravel has it, or anywhere else a project keeps a `config` folder
    let wanted = format!("/config/{}.php", file_name.to_ascii_lowercase());
    stir_db::attached(|db| {
        let ws = Workspace::try_get(db)?;
        // the one nearest the project's folder, not a package's
        let file = ws
            .files(db)
            .iter()
            .copied()
            .filter(|f| f.origin(db) == FileOrigin::Source)
            .filter(|f| {
                let path = f.path(db).to_string_lossy().replace('\\', "/");
                path.to_ascii_lowercase().ends_with(&wanted)
            })
            .min_by_key(|f| f.path(db).components().count())?;
        let root = stir_db::parse(db, file).syntax();
        let returned = root
            .children()
            .filter(|c| c.kind() == RETURN_STMT)
            .last()?
            .children()
            .next()?;
        let ctx = Ctx::new(db, ws);
        let place = Place { file, class: None };
        let mut value = literal(&ctx, &place, &returned, 0);
        for part in parts {
            let map = value.try_cast::<Map>()?;
            value = map.get(part)?.clone();
        }
        Some(value)
    })
    .flatten()
    .unwrap_or(Dynamic::UNIT)
}

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn("laravel_columns", columns)
        .register_fn("laravel_config", config);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_by_convention() {
        assert_eq!(table_of("App\\Models\\User"), "users");
        assert_eq!(table_of("App\\BlogPost"), "blog_posts");
        assert_eq!(table_of("Category"), "categories");
        assert_eq!(table_of("App\\Box"), "boxes");
        assert_eq!(table_of("App\\Person"), "people");
        assert_eq!(table_of("App\\Day"), "days");
        assert_eq!(table_of("App\\UserMetadata"), "user_metadata");
    }
}
