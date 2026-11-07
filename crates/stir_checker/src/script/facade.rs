//! What scripts see of types and of the file they're running for.

use std::sync::{Arc, Mutex, OnceLock};

use rustc_hash::FxHashMap;
use stir_db::{Db, Diagnostic, Severity, SourceFile, Workspace};
use stir_script::rhai::{Array, Dynamic, Engine, EvalAltResult, INT, ImmutableString, Map};
use stir_syntax::TextRange;
use stir_types::{LitValue, Ty, TyKind};

use super::Node;
use crate::model_api::{self, MemberTy};
use crate::{Ctx, TypedFile};

/// The most a rule can report for one file: more than that is a rule gone wrong.
const MAX_REPORTS: usize = 10_000;

type Failing<T> = Result<T, Box<EvalAltResult>>;

/// A type, as a script has it.
#[derive(Clone)]
pub struct TypeFacade {
    ty: Ty,
    ws: Workspace,
}

impl TypeFacade {
    pub(crate) fn new(ty: Ty, ws: Workspace) -> Self {
        TypeFacade { ty, ws }
    }

    fn with_model<R>(&self, f: impl FnOnce(&Ctx<'_>) -> R) -> Option<R> {
        stir_db::attached(|db| f(&Ctx::new(db, self.ws)))
    }

    /// Nothing is known of it.
    fn is_unknown(&self) -> bool {
        self.ty.is_error() || self.ty.is_untyped() || matches!(self.ty.kind(), TyKind::Mixed)
    }

    /// Whether every member of the type, other than `null`, is one that `test` accepts.
    fn every(&self, test: impl Fn(&Ty) -> bool) -> bool {
        let members = self.ty.members();
        let mut rest = members.iter().filter(|m| !m.is_null()).peekable();
        rest.peek().is_some() && rest.all(test)
    }

    /// The class of an object type, if it's one class and maybe `null`.
    fn class_name(&self) -> Option<ImmutableString> {
        let base = stir_types::without_null(&self.ty);
        match base.kind() {
            TyKind::Class { name, .. } => Some(name.as_str().into()),
            TyKind::SelfType { class, .. } => match class.kind() {
                TyKind::Class { name, .. } => Some(name.as_str().into()),
                _ => None,
            },
            _ => None,
        }
    }

    /// The class it names, for `Foo::class` and other `class-string<Foo>`.
    fn class_string(&self) -> Option<ImmutableString> {
        let TyKind::ClassString(Some(of)) = self.ty.kind() else {
            return None;
        };
        TypeFacade::new(of.clone(), self.ws).class_name()
    }

    /// The type arguments of a generic class type, such as `int` and `App\User` for
    /// `Collection<int, App\User>`, or of an array or list type.
    fn type_args(&self) -> Array {
        let base = stir_types::without_null(&self.ty);
        let args: Vec<Ty> = match base.kind() {
            TyKind::Class { args, .. } => args.to_vec(),
            TyKind::SelfType { class, .. } => match class.kind() {
                TyKind::Class { args, .. } => args.to_vec(),
                _ => Vec::new(),
            },
            TyKind::Array { key, value, .. } => vec![key.clone(), value.clone()],
            TyKind::List { elem, .. } => vec![elem.clone()],
            _ => Vec::new(),
        };
        args.into_iter()
            .map(|t| Dynamic::from(TypeFacade::new(t, self.ws)))
            .collect()
    }

    /// The value of a literal type.
    fn literal(&self) -> Dynamic {
        match self.ty.kind() {
            TyKind::Literal(LitValue::Int(i)) => (*i as INT).into(),
            TyKind::Literal(LitValue::Bool(b)) => (*b).into(),
            TyKind::Literal(LitValue::String(s)) => s.as_str().into(),
            TyKind::Literal(LitValue::Float(f)) => f.get().into(),
            _ => Dynamic::UNIT,
        }
    }

    /// Does a value of this type fit `type_text`, a PHPDoc type? Not if nothing's known of it.
    fn is_subtype_of(&self, type_text: &str) -> bool {
        if self.is_unknown() {
            return false;
        }
        self.with_model(|ctx| {
            let Ok(target) = super::hooks::parse_type(ctx, type_text, None) else {
                return false;
            };
            stir_types::is_assignable(ctx, &self.ty, &target)
        })
        .unwrap_or(false)
    }

    /// What's on the type, nearest declaration first.
    fn members(&self) -> Array {
        let base = stir_types::without_null(&self.ty);
        self.with_model(|ctx| {
            let ws = self.ws;
            model_api::members(ctx, &base)
                .into_iter()
                .map(|m| {
                    let (kind, ty) = match m.ty {
                        MemberTy::Property { ty, .. } => ("property", Some(ty)),
                        MemberTy::Method(sig) => ("method", Some(sig.ret)),
                        MemberTy::Const(ty) => ("const", Some(ty)),
                        MemberTy::Case => ("case", None),
                    };
                    let mut member = Map::new();
                    member.insert("name".into(), m.name.as_str().into());
                    member.insert("kind".into(), kind.into());
                    member.insert("is_static".into(), m.is_static.into());
                    let ty = ty.map_or(Dynamic::UNIT, |t| Dynamic::from(TypeFacade::new(t, ws)));
                    member.insert("type".into(), ty);
                    Dynamic::from(member)
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// What calling the method `name` on a value of this type returns, as its signature says.
    fn method_type(&self, name: &str) -> Dynamic {
        let base = stir_types::without_null(&self.ty);
        let ws = self.ws;
        self.with_model(|ctx| {
            let TyKind::Class { .. } = base.kind() else {
                return Dynamic::UNIT;
            };
            let Some((owner, m, map)) = ctx.find_method(&base, name) else {
                return Dynamic::UNIT;
            };
            let ret = stir_types::subst(&ctx.method_sig(&owner, m).ret, &map);
            let ret = crate::body::resolve_self(&ret, &base);
            Dynamic::from(TypeFacade::new(ret, ws))
        })
        .unwrap_or(Dynamic::UNIT)
    }

    fn has_member(&self, name: &str, method: bool) -> bool {
        let base = stir_types::without_null(&self.ty);
        self.with_model(|ctx| {
            model_api::members(ctx, &base).iter().any(|m| match m.ty {
                MemberTy::Method(_) if method => m.name.eq_ignore_ascii_case(name),
                MemberTy::Property { .. } if !method => m.name == name,
                _ => false,
            })
        })
        .unwrap_or(false)
    }
}

/// The types `ctx.get_type` knows.
enum Types {
    /// A rule's: the file's, checked by the time rules run, and asked for only if they want them.
    Checked(OnceLock<Arc<TypedFile>>),
    /// A hook's: what the checker has so far for this call. A hook runs within the check of its
    /// file, so it can't ask for more without going in circles.
    AtCall(FxHashMap<TextRange, Ty>),
}

struct Shared {
    ws: Workspace,
    file: SourceFile,
    types: Types,
    /// What a rule reported. A hook can't report: it can run for any number of paths through the
    /// code, and only some of them count.
    reports: Option<Mutex<Vec<Diagnostic>>>,
}

/// The `ctx` a script is handed.
#[derive(Clone)]
pub struct ScriptCtx {
    shared: Arc<Shared>,
    /// What a report is about, unless it says otherwise.
    at: TextRange,
    /// What the signature says, for a hook.
    declared: Option<Ty>,
}

impl ScriptCtx {
    /// For a rule run over `file`.
    pub(crate) fn for_rules(ws: Workspace, file: SourceFile) -> Self {
        ScriptCtx {
            shared: Arc::new(Shared {
                ws,
                file,
                types: Types::Checked(OnceLock::new()),
                reports: Some(Mutex::new(Vec::new())),
            }),
            at: TextRange::default(),
            declared: None,
        }
    }

    /// For a hook on a call of `file`, knowing the `types` of its parts.
    pub(crate) fn for_call(
        ws: Workspace,
        file: SourceFile,
        types: FxHashMap<TextRange, Ty>,
        at: TextRange,
        declared: Ty,
    ) -> Self {
        ScriptCtx {
            shared: Arc::new(Shared {
                ws,
                file,
                types: Types::AtCall(types),
                reports: None,
            }),
            at,
            declared: Some(declared),
        }
    }

    /// This context, reporting at `at`.
    pub(crate) fn at(&self, at: TextRange) -> Self {
        ScriptCtx { at, ..self.clone() }
    }

    /// What's been reported, leaving none.
    pub(crate) fn take_reports(&self) -> Vec<Diagnostic> {
        let Some(reports) = &self.shared.reports else {
            return Vec::new();
        };
        std::mem::take(&mut *reports.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn type_of(&self, node: &Node) -> TypeFacade {
        let ty = match &self.shared.types {
            Types::AtCall(types) => types.get(&node.range()).cloned(),
            Types::Checked(typed) => {
                let typed = typed.get().cloned().or_else(|| {
                    let (ws, file) = (self.shared.ws, self.shared.file);
                    let found =
                        stir_db::attached(|db: &dyn Db| crate::typed_file(db, ws, file).clone())?;
                    Some(typed.get_or_init(|| found).clone())
                });
                typed.and_then(|t| t.types.get(&node.range()).cloned())
            }
        };
        TypeFacade::new(ty.unwrap_or_else(Ty::untyped), self.shared.ws)
    }

    fn report(
        &self,
        severity: Severity,
        message: &str,
        identifier: &str,
        options: Option<&Map>,
    ) -> Failing<()> {
        let Some(reports) = &self.shared.reports else {
            return Err("only rules can report: this is a hook".into());
        };
        check_identifier(identifier)?;
        let mut at = self.at;
        let mut level = 0;
        if let Some(options) = options {
            if let Some(node) = options.get("node") {
                let Some(node) = node.read_lock::<Node>() else {
                    return Err("`node` in a report's options should be a node".into());
                };
                at = node.range();
            }
            if let Some(wanted) = options.get("level") {
                level = match wanted.as_int() {
                    Ok(n @ 0..=10) => n as u8,
                    _ => return Err("`level` in a report's options should be 0 to 10".into()),
                };
            }
        }
        let path = stir_db::attached(|db: &dyn Db| self.shared.file.path(db).clone());
        let d = Diagnostic::custom(path.unwrap_or_default(), at, severity, identifier, message)
            .with_php_level(level);
        let mut reports = reports.lock().unwrap_or_else(|e| e.into_inner());
        if reports.len() >= MAX_REPORTS {
            return Err(format!("it reported more than {MAX_REPORTS} problems in one file").into());
        }
        reports.push(d);
        Ok(())
    }
}

/// What PHPStan accepts: `app.noVarDump`, with the dot that keeps it apart from our own codes.
fn check_identifier(identifier: &str) -> Failing<()> {
    let part = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric());
    if identifier.contains('.') && identifier.split('.').all(part) {
        return Ok(());
    }
    Err(format!("`{identifier}` isn't an identifier: they look like `app.noVarDump`").into())
}

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_type_with_name::<ScriptCtx>("Context")
        // Rhai keeps `type_of` for itself: this is the name PHPStan's scope has for it
        .register_fn("get_type", |c: &mut ScriptCtx, n: Node| c.type_of(&n))
        .register_get("file", |c: &mut ScriptCtx| {
            let file = c.shared.file;
            stir_db::attached(|db: &dyn Db| file.path(db).display().to_string()).unwrap_or_default()
        })
        .register_get("declared_return_type", |c: &mut ScriptCtx| {
            let ws = c.shared.ws;
            c.declared
                .clone()
                .map_or(Dynamic::UNIT, |t| Dynamic::from(TypeFacade::new(t, ws)))
        })
        .register_fn("error", |c: &mut ScriptCtx, m: &str, i: &str| {
            c.report(Severity::Error, m, i, None)
        })
        .register_fn("error", |c: &mut ScriptCtx, m: &str, i: &str, o: Map| {
            c.report(Severity::Error, m, i, Some(&o))
        })
        .register_fn("warning", |c: &mut ScriptCtx, m: &str, i: &str| {
            c.report(Severity::Warning, m, i, None)
        })
        .register_fn("warning", |c: &mut ScriptCtx, m: &str, i: &str, o: Map| {
            c.report(Severity::Warning, m, i, Some(&o))
        });
    engine
        .register_type_with_name::<TypeFacade>("Type")
        .register_fn("to_string", |t: &mut TypeFacade| t.ty.to_string())
        .register_fn("is_null", |t: &mut TypeFacade| t.ty.is_null())
        .register_fn("is_nullable", |t: &mut TypeFacade| {
            t.ty.members().iter().any(Ty::is_null)
        })
        .register_fn("is_unknown", |t: &mut TypeFacade| t.is_unknown())
        .register_fn("is_string", |t: &mut TypeFacade| {
            t.every(|m| {
                matches!(
                    m.kind(),
                    TyKind::String | TyKind::Literal(LitValue::String(_)) | TyKind::ClassString(_)
                ) || m == &Ty::non_empty_string()
                    || m == &Ty::numeric_string()
            })
        })
        .register_fn("is_int", |t: &mut TypeFacade| {
            t.every(|m| {
                matches!(
                    m.kind(),
                    TyKind::Int | TyKind::IntRange { .. } | TyKind::Literal(LitValue::Int(_))
                )
            })
        })
        .register_fn("is_float", |t: &mut TypeFacade| {
            t.every(|m| {
                matches!(
                    m.kind(),
                    TyKind::Float | TyKind::Literal(LitValue::Float(_))
                )
            })
        })
        .register_fn("is_bool", |t: &mut TypeFacade| {
            t.every(|m| matches!(m.kind(), TyKind::Bool | TyKind::Literal(LitValue::Bool(_))))
        })
        .register_fn("is_array", |t: &mut TypeFacade| {
            t.every(|m| {
                matches!(
                    m.kind(),
                    TyKind::Array { .. } | TyKind::List { .. } | TyKind::Shape(_)
                )
            })
        })
        .register_fn("is_object", |t: &mut TypeFacade| {
            t.every(|m| {
                matches!(
                    m.kind(),
                    TyKind::Class { .. } | TyKind::SelfType { .. } | TyKind::Object
                )
            })
        })
        .register_fn("is_subtype_of", |t: &mut TypeFacade, target: &str| {
            t.is_subtype_of(target)
        })
        .register_fn("without_null", |t: &mut TypeFacade| {
            TypeFacade::new(stir_types::without_null(&t.ty), t.ws)
        })
        .register_get("class_name", |t: &mut TypeFacade| {
            t.class_name().map_or(Dynamic::UNIT, Dynamic::from)
        })
        .register_get("class_string", |t: &mut TypeFacade| {
            t.class_string().map_or(Dynamic::UNIT, Dynamic::from)
        })
        .register_get("literal", |t: &mut TypeFacade| t.literal())
        .register_get("type_args", |t: &mut TypeFacade| t.type_args())
        .register_fn("types", |t: &mut TypeFacade| {
            let ws = t.ws;
            t.ty.members()
                .iter()
                .map(|m| Dynamic::from(TypeFacade::new(m.clone(), ws)))
                .collect::<Array>()
        })
        .register_fn("members", |t: &mut TypeFacade| t.members())
        .register_fn("method_type", |t: &mut TypeFacade, name: &str| {
            t.method_type(name)
        })
        .register_fn("has_method", |t: &mut TypeFacade, name: &str| {
            t.has_member(name, true)
        })
        .register_fn("has_property", |t: &mut TypeFacade, name: &str| {
            t.has_member(name, false)
        });
}
