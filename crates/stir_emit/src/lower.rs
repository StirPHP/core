//! Lowering of StirPHP syntax trees into the PHP output AST.
//!
//! The lowering is driven by the checker's [`TypedFile`]: expression types
//! decide e.g. whether `a->b` is a record field (`$a['b']`) or a property, and
//! whether `==` becomes `===`. It assumes the file type-checks.

use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;
use stir_checker::{Ctx, TypedFile};
use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_hir::ast::NodeExt;
use stir_hir::type_ref::{StirBuiltin, TypeRef, TypeRefKind, parse_int, unquote};
use stir_hir::{FileItems, FunctionDef, ItemLoc};
use stir_syntax::{SyntaxKind, SyntaxKind::*, SyntaxNode, SyntaxToken};
use stir_types::{LitValue, Ty, TyKind};

use crate::ast::*;
use crate::transpile::{EmitOptions, Emitted};

// ---------------------------------------------------------------- entry points

pub(crate) fn lower_file(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    opts: &EmitOptions,
) -> Emitted {
    let ctx = Ctx::new(db, ws);
    let typed = stir_checker::typed_file(db, ws, file);
    let mut l = Lowerer::new(db, &ctx, file, typed, opts);
    let root = stir_db::parse(db, file).syntax();
    let mut out = File {
        strict_types: true,
        ..File::default()
    };
    let mut stmts = Vec::new();
    for child in root.children() {
        if child.kind() == NAMESPACE_DECL && child.child(BLOCK).is_none() {
            out.namespace = Some(Namespace {
                name: child.child(NAME_REF).map(|n| SmolStr::new(text(&n))),
                braced: false,
            });
            continue;
        }
        l.stmt(&child, &mut stmts);
    }
    if l.needs_runtime
        && let Some(req) = &opts.runtime_require
    {
        stmts.insert(
            0,
            Stmt::new(StmtKind::Expr(Expr::new(ExprKind::Include {
                kind: IncludeKind::RequireOnce,
                expr: Box::new(Expr::raw(req.clone())),
            }))),
        );
    }
    out.stmts = stmts;
    let (code, mappings) = crate::print::print_file_with_map(&out, &opts.print);
    Emitted {
        code,
        mappings,
        needs_runtime: l.needs_runtime,
    }
}

pub(crate) fn lower_runtime(db: &dyn Db, ws: Workspace, opts: &EmitOptions) -> Emitted {
    let ctx = Ctx::new(db, ws);
    let mut body = Vec::new();
    for &file in ws.files(db) {
        if file.origin(db) != FileOrigin::Stub || file.dialect(db) != stir_syntax::Dialect::Stir {
            continue;
        }
        let typed = stir_checker::typed_file(db, ws, file);
        let mut l = Lowerer::new(db, &ctx, file, typed, opts);
        // the runtime itself never inlines
        l.inline = false;
        let root = stir_db::parse(db, file).syntax();
        for child in root.children() {
            if child.kind() == NAMESPACE_DECL && child.child(BLOCK).is_none() {
                continue;
            }
            l.stmt(&child, &mut body);
        }
    }
    let out = File {
        strict_types: true,
        stmts: vec![Stmt::new(StmtKind::Namespace {
            name: Some("Stir".into()),
            body: Some(body),
        })],
        ..File::default()
    };
    let (code, mappings) = crate::print::print_file_with_map(&out, &opts.print);
    Emitted {
        code,
        mappings,
        needs_runtime: false,
    }
}

// ---------------------------------------------------------------- helpers

fn text(node: &SyntaxNode) -> String {
    node.text().to_string().trim().to_owned()
}

fn first_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
}

fn exprs_of(node: &SyntaxNode) -> Vec<SyntaxNode> {
    node.children()
        .filter(|c| stir_checker_is_expr(c.kind()))
        .collect()
}

fn stir_checker_is_expr(k: SyntaxKind) -> bool {
    matches!(
        k,
        LITERAL
            | STRING_LITERAL
            | INTERP_STRING
            | HEREDOC
            | SHELL_EXEC
            | VARIABLE_EXPR
            | VARIABLE_VARIABLE
            | NAME_EXPR
            | ARRAY_EXPR
            | LIST_EXPR
            | PAREN_EXPR
            | BIN_EXPR
            | PREFIX_EXPR
            | POSTFIX_EXPR
            | ASSIGN_EXPR
            | TERNARY_EXPR
            | ELVIS_EXPR
            | NOT_NULL_EXPR
            | CAST_EXPR
            | AS_EXPR
            | IS_EXPR
            | INSTANCEOF_EXPR
            | CALL_EXPR
            | METHOD_CALL_EXPR
            | NULLSAFE_METHOD_CALL_EXPR
            | PROPERTY_FETCH_EXPR
            | NULLSAFE_PROPERTY_FETCH_EXPR
            | STATIC_CALL_EXPR
            | STATIC_PROPERTY_FETCH_EXPR
            | CLASS_CONST_FETCH_EXPR
            | INDEX_EXPR
            | NEW_EXPR
            | CLONE_EXPR
            | CLOSURE_EXPR
            | ARROW_FN_EXPR
            | LAMBDA_EXPR
            | MATCH_EXPR
            | WHEN_EXPR
            | IF_EXPR
            | ISSET_EXPR
            | EMPTY_EXPR
            | EVAL_EXPR
            | EXIT_EXPR
            | PRINT_EXPR
            | INCLUDE_EXPR
            | YIELD_EXPR
            | YIELD_FROM_EXPR
            | THROW_EXPR
            | ERROR_SUPPRESS_EXPR
            | MAGIC_CONST
            | RECORD_LITERAL
    )
}

fn is_stmt(k: SyntaxKind) -> bool {
    matches!(
        k,
        EXPR_STMT
            | ECHO_STMT
            | RETURN_STMT
            | IF_STMT
            | WHILE_STMT
            | DO_WHILE_STMT
            | FOR_STMT
            | FOREACH_STMT
            | SWITCH_STMT
            | BREAK_STMT
            | CONTINUE_STMT
            | TRY_STMT
            | THROW_STMT
            | GLOBAL_STMT
            | STATIC_VAR_STMT
            | UNSET_STMT
            | BLOCK
            | VAL_DECL
            | VAR_DECL
            | ECHO_TAG_STMT
            | EMPTY_STMT
            | INLINE_HTML_STMT
            | DECLARE_STMT
            | NAMESPACE_DECL
            | FUNCTION_DECL
            | CLASS_DECL
            | INTERFACE_DECL
            | TRAIT_DECL
            | ENUM_DECL
            | RECORD_DECL
            | TYPEALIAS_DECL
            | GOTO_STMT
            | LABEL_STMT
            | USE_DECL
            | CONST_DECL
    )
}

fn op_of(node: &SyntaxNode) -> Option<SyntaxKind> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
        .map(|t| t.kind())
}

fn q(name: &str) -> QName {
    QName {
        name: SmolStr::new(name.trim_start_matches('\\')),
        fully_qualified: true,
    }
}

fn strip_platform(t: &Ty) -> Ty {
    match t.kind() {
        TyKind::Platform(i) => i.clone(),
        _ => t.clone(),
    }
}

fn is_safe_family(t: &Ty) -> bool {
    !t.is_php() && !matches!(t.kind(), TyKind::Mixed | TyKind::AnyPlatform)
}

/// The PHP native type for a StirPHP/PHP type, if one exists.
pub(crate) fn native_type(t: &Ty) -> Option<Type> {
    let t = strip_platform(t);
    Some(match t.kind() {
        TyKind::Int | TyKind::PhpInt | TyKind::IntRange { .. } => Type::Builtin(BuiltinType::Int),
        TyKind::Float | TyKind::PhpFloat => Type::Builtin(BuiltinType::Float),
        TyKind::String
        | TyKind::PhpString
        | TyKind::NonEmptyString
        | TyKind::NumericString
        | TyKind::ClassString(_) => Type::Builtin(BuiltinType::String),
        TyKind::Bool | TyKind::PhpBool => Type::Builtin(BuiltinType::Bool),
        TyKind::Literal(l) => match &l.value {
            LitValue::Int(_) => Type::Builtin(BuiltinType::Int),
            LitValue::Float(_) => Type::Builtin(BuiltinType::Float),
            LitValue::String(_) => Type::Builtin(BuiltinType::String),
            LitValue::Bool(true) => Type::Builtin(BuiltinType::True),
            LitValue::Bool(false) => Type::Builtin(BuiltinType::False),
        },
        TyKind::Unit | TyKind::Void => Type::Builtin(BuiltinType::Void),
        TyKind::Nothing | TyKind::Never => Type::Builtin(BuiltinType::Never),
        TyKind::Null => Type::Builtin(BuiltinType::Null),
        TyKind::List(_)
        | TyKind::MutableList(_)
        | TyKind::Map(_, _)
        | TyKind::MutableMap(_, _)
        | TyKind::Record(_)
        | TyKind::Array { .. }
        | TyKind::PhpList { .. }
        | TyKind::Shape(_) => Type::Builtin(BuiltinType::Array),
        TyKind::Fn(_) => Type::Class(q("Closure")),
        TyKind::Callable { closure: true, .. } => Type::Class(q("Closure")),
        TyKind::Callable { .. } => Type::Builtin(BuiltinType::Callable),
        TyKind::Iterable { .. } => Type::Builtin(BuiltinType::Iterable),
        TyKind::Object => Type::Builtin(BuiltinType::Object),
        TyKind::Mixed | TyKind::AnyPlatform => Type::Builtin(BuiltinType::Mixed),
        TyKind::Class { name, .. } => Type::Class(q(name)),
        TyKind::SelfType {
            kind: stir_types::SelfKind::Static,
            ..
        } => Type::Builtin(BuiltinType::Static),
        TyKind::SelfType { class, .. } => return native_type(class),
        TyKind::Union(_) => {
            let mut members: Vec<Type> = Vec::new();
            for m in t.members() {
                let n = native_type(m)?;
                if !members.contains(&n) {
                    members.push(n);
                }
            }
            // `true|false` → `bool`
            let tf = [
                Type::Builtin(BuiltinType::True),
                Type::Builtin(BuiltinType::False),
            ];
            if tf.iter().all(|x| members.contains(x)) {
                members.retain(|m| !tf.contains(m));
                members.push(Type::Builtin(BuiltinType::Bool));
            }
            if members.len() == 1 {
                members.pop().unwrap()
            } else if members.len() == 2 && members.contains(&Type::Builtin(BuiltinType::Null)) {
                let other = members
                    .into_iter()
                    .find(|m| *m != Type::Builtin(BuiltinType::Null))
                    .unwrap();
                Type::Nullable(Box::new(other))
            } else {
                Type::Union(members)
            }
        }
        TyKind::Intersection(_) => Type::Intersection(
            t.members()
                .iter()
                .map(native_type)
                .collect::<Option<Vec<_>>>()?,
        ),
        TyKind::Param { .. }
        | TyKind::Error
        | TyKind::Resource
        | TyKind::Scalar
        | TyKind::Numeric
        | TyKind::ArrayKey => {
            return None;
        }
        _ => return None,
    })
}

/// Does the PHPDoc type say more than the native type?
fn doc_needed(t: &Ty) -> bool {
    let doc = stir_types::to_phpdoc(t);
    match native_type(t) {
        Some(n) => crate::print::print_type(&n) != doc,
        None => true,
    }
}

/// The mangled PHP name of an extension function (`String.length` → `String_length`).
pub(crate) fn extension_name(f: &FunctionDef) -> String {
    let head = f
        .receiver
        .as_ref()
        .map(receiver_head)
        .unwrap_or_else(|| "Any".into());
    format!("{head}_{}", f.name.short())
}

fn receiver_head(t: &TypeRef) -> String {
    match &t.kind {
        TypeRefKind::Stir(b, args) => {
            // concrete type arguments distinguish `List<Int>.sum` from `List<Float>.sum`
            let mut s = stir_head(*b).to_owned();
            for a in args {
                if !matches!(a.kind, TypeRefKind::Param(_)) {
                    s.push_str(&receiver_head(a));
                }
            }
            s
        }
        TypeRefKind::Php(b) => format!("{b:?}").to_ascii_lowercase(),
        TypeRefKind::Param(_) => "Any".into(),
        TypeRefKind::Named(n, _) => n.short().to_owned(),
        TypeRefKind::Nullable(inner) => format!("Nullable{}", receiver_head(inner)),
        TypeRefKind::Function { .. } => "Fn".into(),
        _ => "Value".into(),
    }
}

fn stir_head(b: StirBuiltin) -> &'static str {
    match b {
        StirBuiltin::Int => "Int",
        StirBuiltin::Float => "Float",
        StirBuiltin::String => "String",
        StirBuiltin::Bool => "Bool",
        StirBuiltin::Unit => "Unit",
        StirBuiltin::Nothing => "Nothing",
        StirBuiltin::Any => "Any",
        StirBuiltin::List => "List",
        StirBuiltin::MutableList => "MutableList",
        StirBuiltin::Map => "Map",
        StirBuiltin::MutableMap => "MutableMap",
    }
}

/// Cheap, side-effect-free expressions that can be duplicated.
fn is_trivial(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Var(_)
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Name(_)
    )
}

// ---------------------------------------------------------------- the lowerer

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bind {
    Val,
    Var,
}

struct Lowerer<'a, 'db> {
    db: &'db dyn Db,
    ctx: &'a Ctx<'db>,
    file: SourceFile,
    items: &'db FileItems,
    typed: &'a TypedFile,
    inline: bool,
    needs_runtime: bool,
    tmp: u32,
    /// Locals visible in the current function, innermost frame last.
    frames: Vec<FxHashMap<SmolStr, Bind>>,
    /// Statements to emit before the statement currently being lowered.
    pre: Vec<Stmt>,
    /// > 0 while lowering code that may not run (`&&` right side, branches):
    /// pre-statements can't be used there.
    conditional: u32,
    /// In an extension function body: `this` is `$__this`.
    in_extension: bool,
    /// Renames applied while inlining (`x` → `$__i3_x`).
    renames: Vec<FxHashMap<SmolStr, Expr>>,
    /// Extension functions currently being inlined (no recursion by construction).
    inlining: Vec<ItemLoc>,
}

impl<'a, 'db> Lowerer<'a, 'db> {
    fn new(
        db: &'db dyn Db,
        ctx: &'a Ctx<'db>,
        file: SourceFile,
        typed: &'a TypedFile,
        opts: &EmitOptions,
    ) -> Self {
        Lowerer {
            db,
            ctx,
            file,
            items: stir_hir::file_items(db, file),
            typed,
            inline: opts.inline_extensions,
            needs_runtime: false,
            tmp: 0,
            frames: vec![FxHashMap::default()],
            pre: Vec::new(),
            conditional: 0,
            in_extension: false,
            renames: Vec::new(),
            inlining: Vec::new(),
        }
    }

    fn origin(&self, node: &SyntaxNode) -> Option<Origin> {
        let r = node.text_range();
        Some(Origin::new(0, u32::from(r.start()), u32::from(r.end())))
    }

    fn ty(&self, node: &SyntaxNode) -> Option<Ty> {
        self.typed.types.get(&node.text_range()).cloned()
    }

    fn fresh(&mut self, hint: &str) -> SmolStr {
        self.tmp += 1;
        SmolStr::new(format!("__{hint}{}", self.tmp))
    }

    fn declare(&mut self, name: &str, b: Bind) {
        self.frames
            .last_mut()
            .unwrap()
            .insert(SmolStr::new(name), b);
    }

    fn visible(&self, name: &str) -> Option<Bind> {
        self.frames.iter().rev().find_map(|f| f.get(name).copied())
    }

    fn renamed(&self, name: &str) -> Option<Expr> {
        self.renames.last().and_then(|r| r.get(name).cloned())
    }

    // ------------------------------------------------------------ statements

    fn block(&mut self, node: &SyntaxNode) -> Vec<Stmt> {
        let mut out = Vec::new();
        match node.kind() {
            BLOCK => {
                for c in node.children() {
                    self.stmt(&c, &mut out);
                }
            }
            _ => self.stmt(node, &mut out),
        }
        out
    }

    /// Lowers a statement, flushing pre-statements before it.
    fn stmt(&mut self, node: &SyntaxNode, out: &mut Vec<Stmt>) {
        let saved = std::mem::take(&mut self.pre);
        let mut produced = Vec::new();
        self.stmt_inner(node, &mut produced);
        let pre = std::mem::replace(&mut self.pre, saved);
        out.extend(pre);
        out.extend(produced);
    }

    fn stmt_inner(&mut self, node: &SyntaxNode, out: &mut Vec<Stmt>) {
        let origin = self.origin(node);
        let push = |out: &mut Vec<Stmt>, kind: StmtKind| out.push(Stmt { kind, origin });
        match node.kind() {
            EXPR_STMT => {
                let Some(e) = exprs_of(node).into_iter().next() else {
                    return;
                };
                match e.kind() {
                    WHEN_EXPR => self.when_stmt(&e, out),
                    IF_EXPR => {
                        let v = self.expr(&e);
                        push(out, StmtKind::Expr(v));
                    }
                    _ => {
                        let v = self.expr(&e);
                        push(out, StmtKind::Expr(v));
                    }
                }
            }
            ECHO_STMT | ECHO_TAG_STMT => {
                let es = exprs_of(node).iter().map(|e| self.expr(e)).collect();
                push(out, StmtKind::Echo(es));
            }
            RETURN_STMT => {
                let v = exprs_of(node).first().map(|e| self.expr(e));
                push(out, StmtKind::Return(v));
            }
            BLOCK => {
                self.frames.push(FxHashMap::default());
                let b = self.block(node);
                self.frames.pop();
                push(out, StmtKind::Block(b));
            }
            IF_STMT => {
                let cond = exprs_of(node)
                    .first()
                    .map(|c| self.expr(c))
                    .unwrap_or_else(|| Expr::bool(true));
                let body = node
                    .children()
                    .find(|c| is_stmt(c.kind()))
                    .map(|b| self.block(&b))
                    .unwrap_or_default();
                let mut elseifs = Vec::new();
                let mut else_ = None;
                for clause in node.children() {
                    match clause.kind() {
                        ELSEIF_CLAUSE => {
                            let c = exprs_of(&clause)
                                .first()
                                .map(|c| self.expr(c))
                                .unwrap_or_else(|| Expr::bool(true));
                            let b = clause
                                .children()
                                .find(|c| is_stmt(c.kind()))
                                .map(|b| self.block(&b))
                                .unwrap_or_default();
                            elseifs.push(ElseIf { cond: c, body: b });
                        }
                        ELSE_CLAUSE => {
                            else_ = Some(
                                clause
                                    .children()
                                    .find(|c| is_stmt(c.kind()))
                                    .map(|b| self.block(&b))
                                    .unwrap_or_default(),
                            );
                        }
                        _ => {}
                    }
                }
                push(
                    out,
                    StmtKind::If {
                        cond,
                        then: body,
                        elseifs,
                        else_,
                    },
                );
            }
            WHILE_STMT => {
                let cond = exprs_of(node)
                    .first()
                    .map(|c| self.expr(c))
                    .unwrap_or_else(|| Expr::bool(true));
                let body = node
                    .children()
                    .find(|c| is_stmt(c.kind()))
                    .map(|b| self.block(&b))
                    .unwrap_or_default();
                push(out, StmtKind::While { cond, body });
            }
            DO_WHILE_STMT => {
                let body = node
                    .children()
                    .find(|c| is_stmt(c.kind()))
                    .map(|b| self.block(&b))
                    .unwrap_or_default();
                let cond = exprs_of(node)
                    .first()
                    .map(|c| self.expr(c))
                    .unwrap_or_else(|| Expr::bool(true));
                push(out, StmtKind::DoWhile { body, cond });
            }
            FOR_STMT => {
                let part = |this: &mut Self, k: SyntaxKind| -> Vec<Expr> {
                    node.child(k)
                        .map(|p| exprs_of(&p).iter().map(|e| this.expr(e)).collect())
                        .unwrap_or_default()
                };
                let init = part(self, FOR_INIT);
                let cond = part(self, FOR_COND);
                let step = part(self, FOR_STEP);
                let body = node
                    .children()
                    .find(|c| is_stmt(c.kind()))
                    .map(|b| self.block(&b))
                    .unwrap_or_default();
                push(
                    out,
                    StmtKind::For {
                        init,
                        cond,
                        step,
                        body,
                    },
                );
            }
            FOREACH_STMT => {
                let es = exprs_of(node);
                let subject = es.first().map(|e| self.expr(e)).unwrap_or_else(Expr::null);
                let (key, value) = match es.len() {
                    3 => (Some(&es[1]), &es[2]),
                    2 => (None, &es[1]),
                    _ => return,
                };
                let by_ref = value.kind() == PREFIX_EXPR && op_of(value) == Some(AMP);
                let bind = |this: &mut Self, n: &SyntaxNode| -> Expr {
                    if n.kind() == VARIABLE_EXPR {
                        this.declare(text(n).trim_start_matches('$'), Bind::Var);
                    }
                    let n = if n.kind() == PREFIX_EXPR {
                        exprs_of(n).into_iter().next().unwrap_or(n.clone())
                    } else {
                        n.clone()
                    };
                    this.expr(&n)
                };
                let key = key.map(|k| bind(self, k));
                let value = bind(self, value);
                let body = node
                    .children()
                    .find(|c| is_stmt(c.kind()))
                    .map(|b| self.block(&b))
                    .unwrap_or_default();
                push(
                    out,
                    StmtKind::Foreach {
                        expr: subject,
                        key,
                        value,
                        by_ref,
                        body,
                    },
                );
            }
            SWITCH_STMT => {
                let subject = exprs_of(node)
                    .first()
                    .map(|e| self.expr(e))
                    .unwrap_or_else(Expr::null);
                let cases = node
                    .children_of(SWITCH_CASE)
                    .iter()
                    .map(|c| {
                        let test = exprs_of(c).first().map(|e| self.expr(e));
                        let mut body = Vec::new();
                        for s in c.children().filter(|s| is_stmt(s.kind())) {
                            self.stmt(&s, &mut body);
                        }
                        SwitchCase { test, body }
                    })
                    .collect();
                push(out, StmtKind::Switch { subject, cases });
            }
            BREAK_STMT | CONTINUE_STMT => {
                let n = exprs_of(node)
                    .first()
                    .and_then(|e| parse_int(&text(e)))
                    .map(|v| v as u32);
                push(
                    out,
                    if node.kind() == BREAK_STMT {
                        StmtKind::Break(n)
                    } else {
                        StmtKind::Continue(n)
                    },
                );
            }
            TRY_STMT => {
                let mut body = Vec::new();
                let mut catches = Vec::new();
                let mut finally = None;
                for c in node.children() {
                    match c.kind() {
                        BLOCK => body = self.block(&c),
                        CATCH_CLAUSE => {
                            let types = c
                                .children_of(NAME_REF)
                                .iter()
                                .map(|n| self.class_qname(&text(n), n))
                                .collect();
                            let var = c.token(VARIABLE).map(|v| SmolStr::new(&v.text()[1..]));
                            if let Some(v) = &var {
                                self.declare(v, Bind::Var);
                            }
                            let b = c.child(BLOCK).map(|b| self.block(&b)).unwrap_or_default();
                            catches.push(Catch {
                                types,
                                var,
                                body: b,
                            });
                        }
                        FINALLY_CLAUSE => finally = c.child(BLOCK).map(|b| self.block(&b)),
                        _ => {}
                    }
                }
                push(
                    out,
                    StmtKind::Try {
                        body,
                        catches,
                        finally,
                    },
                );
            }
            THROW_STMT => {
                if let Some(e) = exprs_of(node).first() {
                    let v = self.expr(e);
                    push(out, StmtKind::Throw(v));
                }
            }
            UNSET_STMT => {
                let es = exprs_of(node).iter().map(|e| self.expr(e)).collect();
                push(out, StmtKind::Unset(es));
            }
            GLOBAL_STMT => {
                let names = node
                    .descendants()
                    .filter(|d| d.kind() == VARIABLE_EXPR)
                    .map(|v| SmolStr::new(text(&v).trim_start_matches('$')))
                    .collect();
                push(out, StmtKind::Global(names));
            }
            STATIC_VAR_STMT => {
                let vars = node
                    .children_of(STATIC_VAR)
                    .iter()
                    .filter_map(|sv| {
                        let name = SmolStr::new(&sv.token(VARIABLE)?.text()[1..]);
                        let default = exprs_of(sv).first().map(|e| self.expr(e));
                        Some(StaticVar { name, default })
                    })
                    .collect();
                push(out, StmtKind::Static(vars));
            }
            VAL_DECL => {
                let Some(name) = node.name_text() else { return };
                self.declare(&name, Bind::Val);
                if let Some(e) = exprs_of(node).first() {
                    let v = self.expr(e);
                    push(out, StmtKind::Expr(Expr::var(name).assign(v)));
                }
            }
            VAR_DECL => {
                let Some(var) = node.token(VARIABLE) else {
                    return;
                };
                let name = &var.text()[1..];
                self.declare(name, Bind::Var);
                if let Some(e) = exprs_of(node).first() {
                    let v = self.expr(e);
                    push(out, StmtKind::Expr(Expr::var(name).assign(v)));
                }
            }
            INLINE_HTML_STMT => push(out, StmtKind::InlineHtml(text(node))),
            LABEL_STMT => push(
                out,
                StmtKind::Label(SmolStr::new(node.name_text().unwrap_or_default())),
            ),
            GOTO_STMT => push(
                out,
                StmtKind::Goto(SmolStr::new(node.name_text().unwrap_or_default())),
            ),
            USE_DECL => self.use_decl(node, out),
            NAMESPACE_DECL => {
                let name = node.child(NAME_REF).map(|n| SmolStr::new(text(&n)));
                let body = node.child(BLOCK).map(|b| self.block(&b));
                push(out, StmtKind::Namespace { name, body });
            }
            DECLARE_STMT => {
                // `strict_types` is always emitted at the top
                if let Some(b) = node.child(BLOCK) {
                    let inner = self.block(&b);
                    out.extend(inner);
                }
            }
            CONST_DECL => {
                let items = node
                    .children_of(CONST_ELEM)
                    .iter()
                    .filter_map(|e| {
                        let name = SmolStr::new(e.name_text()?);
                        let v = exprs_of(e).first().map(|x| self.expr(x))?;
                        Some((name, v))
                    })
                    .collect();
                push(
                    out,
                    StmtKind::Const(ConstDecl {
                        doc: None,
                        items,
                        origin,
                    }),
                );
            }
            FUNCTION_DECL => self.function_decl(node, out),
            CLASS_DECL | INTERFACE_DECL | TRAIT_DECL | ENUM_DECL => self.class_decl(node, out),
            // records and type aliases are compile-time only
            RECORD_DECL | TYPEALIAS_DECL | EMPTY_STMT => {}
            _ => {}
        }
    }

    fn use_decl(&mut self, node: &SyntaxNode, out: &mut Vec<Stmt>) {
        let kind = if node.has_token(FUNCTION_KW) {
            UseKind::Function
        } else if node.has_token(CONST_KW) {
            UseKind::Const
        } else {
            UseKind::Class
        };
        let mut items = Vec::new();
        for c in node.children_of(USE_CLAUSE) {
            let Some(name) = c.child(NAME_REF) else {
                continue;
            };
            let base = text(&name);
            if let Some(group) = c.child(USE_GROUP) {
                for inner in group.children_of(USE_CLAUSE) {
                    if let Some(n) = inner.child(NAME_REF) {
                        items.push(UseItem {
                            name: SmolStr::new(format!("{base}\\{}", text(&n))),
                            alias: inner.name_text().map(SmolStr::new),
                        });
                    }
                }
            } else {
                // drop imports of compile-time-only types (records, aliases)
                let fqn = base.trim_start_matches('\\').to_ascii_lowercase();
                let is_compile_time = self.ctx.index.type_aliases.contains_key(fqn.as_str())
                    || self
                        .ctx
                        .class(&fqn)
                        .is_some_and(|c| c.kind() == stir_hir::ClassKind::Record);
                if kind == UseKind::Class && is_compile_time {
                    continue;
                }
                items.push(UseItem {
                    name: SmolStr::new(base),
                    alias: c.name_text().map(SmolStr::new),
                });
            }
        }
        if !items.is_empty() {
            out.push(Stmt::new(StmtKind::Use(UseDecl {
                kind,
                group_prefix: None,
                items,
            })));
        }
    }

    // ------------------------------------------------------------ declarations

    fn types_doc(&self, t: &Ty) -> (Option<Type>, Option<String>) {
        let native = native_type(t);
        let doc = doc_needed(t).then(|| stir_types::to_phpdoc(t));
        (native, doc)
    }

    /// Lowers parameters; returns them plus `@param` doc tags.
    fn params(
        &mut self,
        list: Option<SyntaxNode>,
        sig: Option<&stir_checker::model_api::FnSigView>,
        doc: &mut DocComment,
    ) -> Vec<Param> {
        let mut out = Vec::new();
        let Some(list) = list else { return out };
        for (i, p) in list.children_of(PARAM).iter().enumerate() {
            let var = p.token(VARIABLE);
            let name = match (&var, p.name_text()) {
                (Some(v), _) => SmolStr::new(&v.text()[1..]),
                (None, Some(n)) => SmolStr::new(n),
                _ => continue,
            };
            self.declare(
                &name,
                if var.is_some() || p.has_ident("mut") {
                    Bind::Var
                } else {
                    Bind::Val
                },
            );
            let ty = sig.and_then(|s| s.params.get(i).cloned());
            let (native, d) = match &ty {
                Some(t) => self.types_doc(t),
                None => (None, None),
            };
            let variadic = p.has_token(ELLIPSIS);
            if let Some(d) = d {
                doc.tags.push(DocTag {
                    name: "param".into(),
                    text: format!("{d} {}${name}", if variadic { "..." } else { "" }),
                });
            }
            let default = exprs_of(p).first().map(|e| self.expr(e));
            out.push(Param {
                attrs: Vec::new(),
                promote: None,
                ty: native,
                by_ref: p.has_token(AMP),
                variadic,
                name,
                default,
                origin: self.origin(p),
            });
        }
        out
    }

    fn template_tags(
        &self,
        params: &[(SmolStr, Option<Ty>, stir_types::Variance)],
        doc: &mut DocComment,
    ) {
        for (name, bound, variance) in params {
            let tag = match variance {
                stir_types::Variance::Out => "template-covariant",
                stir_types::Variance::In => "template-contravariant",
                stir_types::Variance::Invariant => "template",
            };
            let text = match bound {
                Some(b) => format!("{name} of {}", stir_types::to_phpdoc(b)),
                None => name.to_string(),
            };
            doc.tags.push(DocTag {
                name: tag.into(),
                text,
            });
        }
    }

    fn body_stmts(&mut self, node: &SyntaxNode, ret: &Ty) -> Vec<Stmt> {
        if let Some(b) = node.child(FUNCTION_BODY) {
            let Some(e) = exprs_of(&b).into_iter().next() else {
                return Vec::new();
            };
            let saved = std::mem::take(&mut self.pre);
            let v = self.expr(&e);
            let mut out = std::mem::replace(&mut self.pre, saved);
            if matches!(ret.kind(), TyKind::Unit | TyKind::Void) {
                out.push(Stmt::expr(v));
            } else if matches!(ret.kind(), TyKind::Nothing | TyKind::Never) {
                out.push(Stmt::expr(v));
            } else {
                out.push(Stmt::ret(Some(v)));
            }
            out
        } else if let Some(b) = node.child(BLOCK) {
            self.block(&b)
        } else {
            Vec::new()
        }
    }

    fn function_decl(&mut self, node: &SyntaxNode, out: &mut Vec<Stmt>) {
        let ptr = stir_hir::NodePtr::new(node);
        let Some((idx, f)) = self
            .items
            .functions
            .iter()
            .enumerate()
            .find(|(_, f)| f.ptr == ptr)
        else {
            return;
        };
        let loc = ItemLoc {
            file: self.file,
            index: idx as u32,
        };
        let sig = stir_checker::model_api::fn_sig_view(self.ctx, loc);
        let mut doc = DocComment::default();
        self.template_tags(&sig.type_params, &mut doc);
        let saved_frames = std::mem::replace(&mut self.frames, vec![FxHashMap::default()]);
        let mut params = Vec::new();
        let name;
        if let Some(recv) = &sig.receiver {
            self.in_extension = true;
            let (native, d) = self.types_doc(recv);
            if let Some(d) = d {
                doc.tags.push(DocTag {
                    name: "param".into(),
                    text: format!("{d} $__this"),
                });
            }
            params.push(Param {
                ty: native,
                by_ref: stir_checker::is_by_ref_receiver(recv),
                name: "__this".into(),
                ..Param::default()
            });
            name = SmolStr::new(extension_name(f));
        } else {
            name = SmolStr::new(f.name.short());
        }
        params.extend(self.params(node.child(PARAM_LIST), Some(&sig), &mut doc));
        let (ret_native, ret_doc) = self.types_doc(&sig.ret);
        if let Some(d) = ret_doc {
            doc.tags.push(DocTag {
                name: "return".into(),
                text: d,
            });
        }
        let body = self.body_stmts(node, &sig.ret);
        self.in_extension = false;
        self.frames = saved_frames;
        out.push(Stmt::new(StmtKind::Function(FunctionDecl {
            doc: (!doc.is_empty()).then_some(doc),
            attrs: Vec::new(),
            by_ref: false,
            name,
            params,
            return_type: ret_native,
            body,
            origin: self.origin(node),
        })));
    }

    fn class_decl(&mut self, node: &SyntaxNode, out: &mut Vec<Stmt>) {
        let ptr = stir_hir::NodePtr::new(node);
        let Some(def) = self.items.classes.iter().find(|c| c.ptr == ptr) else {
            return;
        };
        let Some(model) = self.ctx.class(def.name.as_str()) else {
            return;
        };
        let view = stir_checker::model_api::class_view(self.ctx, &model);
        let mut doc = DocComment::default();
        self.template_tags(&view.type_params, &mut doc);
        for (tag, t) in &view.super_docs {
            doc.tags.push(DocTag {
                name: (*tag).into(),
                text: t.clone(),
            });
        }
        let mut members = Vec::new();
        let mut ctor_init: Vec<Stmt> = Vec::new();
        let mut ctor_params: Vec<Param> = Vec::new();
        let mut has_explicit_ctor = false;
        let mut ctor_doc = DocComment::default();
        let mut parent_call: Option<Vec<Arg>> = None;
        let saved_frames = std::mem::replace(&mut self.frames, vec![FxHashMap::default()]);

        // Kotlin-style supertype list with a constructor call: `: Base(args)`
        if let Some(list) = node.child(SUPERTYPE_LIST) {
            for st in list.children_of(SUPERTYPE) {
                if let Some(args) = st.child(ARG_LIST) {
                    // evaluated inside the constructor, where the ctor params are in scope
                    let _ = args;
                }
            }
        }
        // primary constructor
        if let Some(pc) = node.child(PRIMARY_CTOR).and_then(|c| c.child(PARAM_LIST)) {
            for (i, p) in pc.children_of(PARAM).iter().enumerate() {
                let var = p.token(VARIABLE);
                let name = match (&var, p.name_text()) {
                    (Some(v), _) => SmolStr::new(&v.text()[1..]),
                    (None, Some(n)) => SmolStr::new(n),
                    _ => continue,
                };
                self.declare(&name, Bind::Val);
                let ty = view.ctor_params.get(i).cloned();
                let (native, d) = ty
                    .as_ref()
                    .map(|t| self.types_doc(t))
                    .unwrap_or((None, None));
                let promoted_val = p.has_ident("val");
                let promoted_var = p.has_token(VAR_KW);
                let mods = p.child(MODIFIERS);
                let vis = mods
                    .as_ref()
                    .map(member_visibility)
                    .unwrap_or(Visibility::Public);
                let promote = (promoted_val || promoted_var || mods.is_some()).then(|| Promotion {
                    visibility: Some(vis),
                    set_visibility: None,
                    readonly: promoted_val
                        || mods.as_ref().is_some_and(|m| m.has_token(READONLY_KW)),
                });
                if let Some(d) = d {
                    ctor_doc.tags.push(DocTag {
                        name: "param".into(),
                        text: format!("{d} ${name}"),
                    });
                }
                let default = exprs_of(p).first().map(|e| self.expr(e));
                // a readonly property must be typed
                let native = native.or_else(|| {
                    promote
                        .as_ref()
                        .filter(|p| p.readonly)
                        .map(|_| Type::Builtin(BuiltinType::Mixed))
                });
                ctor_params.push(Param {
                    attrs: Vec::new(),
                    promote,
                    ty: native,
                    by_ref: false,
                    variadic: p.has_token(ELLIPSIS),
                    name,
                    default,
                    origin: self.origin(p),
                });
            }
            if let Some(list) = node.child(SUPERTYPE_LIST) {
                for st in list.children_of(SUPERTYPE) {
                    if let Some(args) = st.child(ARG_LIST) {
                        parent_call = Some(self.args(&args));
                    }
                }
            }
        }
        if let Some(body) = node.child(CLASS_BODY) {
            for m in body.children() {
                match m.kind() {
                    PROPERTY_DECL => self.property(&m, &model, &mut members, &mut ctor_init),
                    METHOD_DECL => {
                        if let Some(n) = m.child(SyntaxKind::NAME)
                            && text(&n).eq_ignore_ascii_case("__construct")
                        {
                            has_explicit_ctor = true;
                        }
                        self.method(&m, &model, &mut members);
                    }
                    CLASS_CONST_DECL => {
                        let vis = m.child(MODIFIERS).as_ref().map(member_visibility);
                        for e in m.children_of(CONST_ELEM) {
                            let Some(name) = e.name_text() else { continue };
                            let Some(v) = exprs_of(&e).first().map(|x| self.expr(x)) else {
                                continue;
                            };
                            members.push(Member::Const(ClassConstDecl {
                                doc: None,
                                attrs: Vec::new(),
                                is_final: false,
                                visibility: vis,
                                ty: None,
                                name: name.into(),
                                value: v,
                                origin: self.origin(&e),
                            }));
                        }
                    }
                    ENUM_CASE => {
                        let Some(name) = m.name_text() else { continue };
                        let value = exprs_of(&m).first().map(|x| self.expr(x));
                        members.push(Member::Case(EnumCase {
                            doc: None,
                            attrs: Vec::new(),
                            name: name.into(),
                            value,
                            origin: self.origin(&m),
                        }));
                    }
                    TRAIT_USE => {
                        let traits = m
                            .children_of(NAME_REF)
                            .iter()
                            .map(|n| self.class_qname(&text(n), n))
                            .collect();
                        members.push(Member::TraitUse(TraitUse {
                            traits,
                            adaptations: Vec::new(),
                            origin: self.origin(&m),
                        }));
                    }
                    _ => {}
                }
            }
        }
        // constructor: primary-constructor promotion, parent call, property initialization
        if !ctor_params.is_empty()
            || parent_call.is_some()
            || (!ctor_init.is_empty() && !has_explicit_ctor)
        {
            let mut body = Vec::new();
            if let Some(args) = parent_call {
                body.push(Stmt::expr(Expr::new(ExprKind::StaticCall {
                    class: ClassRef::Parent,
                    method: MemberName::Ident("__construct".into()),
                    args,
                })));
            }
            body.extend(std::mem::take(&mut ctor_init));
            members.insert(
                0,
                Member::Method(MethodDecl {
                    doc: (!ctor_doc.is_empty()).then_some(ctor_doc),
                    attrs: Vec::new(),
                    is_final: false,
                    is_abstract: false,
                    visibility: Some(Visibility::Public),
                    is_static: false,
                    by_ref: false,
                    name: "__construct".into(),
                    params: ctor_params,
                    return_type: None,
                    body: Some(body),
                    origin: None,
                }),
            );
        } else if !ctor_init.is_empty() {
            // prepend initializers into the explicit constructor
            for m in &mut members {
                if let Member::Method(md) = m
                    && md.name.eq_ignore_ascii_case("__construct")
                    && let Some(b) = &mut md.body
                {
                    let mut init = std::mem::take(&mut ctor_init);
                    init.append(b);
                    *b = init;
                }
            }
        }
        self.frames = saved_frames;
        let name = SmolStr::new(def.name.short());
        let doc = (!doc.is_empty()).then_some(doc);
        let origin = self.origin(node);
        let kind = match def.kind {
            stir_hir::ClassKind::Interface => StmtKind::Interface(InterfaceDecl {
                doc,
                attrs: Vec::new(),
                name,
                extends: view.interfaces.iter().map(|n| q(n)).collect(),
                members,
                origin,
            }),
            stir_hir::ClassKind::Trait => StmtKind::Trait(TraitDecl {
                doc,
                attrs: Vec::new(),
                name,
                members,
                origin,
            }),
            stir_hir::ClassKind::Enum => StmtKind::Enum(EnumDecl {
                doc,
                attrs: Vec::new(),
                name,
                backing: view.enum_backing.as_ref().and_then(native_type),
                implements: view.interfaces.iter().map(|n| q(n)).collect(),
                members,
                origin,
            }),
            _ => {
                let all_readonly = members.iter().all(|m| match m {
                    Member::Property(p) => p.readonly,
                    Member::Method(md) if md.name.eq_ignore_ascii_case("__construct") => md
                        .params
                        .iter()
                        .all(|p| p.promote.as_ref().is_none_or(|pr| pr.readonly)),
                    _ => true,
                });
                StmtKind::Class(ClassDecl {
                    doc,
                    attrs: Vec::new(),
                    modifiers: ClassModifiers {
                        is_final: model.is_final(),
                        is_abstract: def.mods.is_abstract || def.mods.is_sealed,
                        is_readonly: def.mods.is_data && all_readonly,
                    },
                    name,
                    extends: view.parent.as_ref().map(|n| q(n)),
                    implements: view.interfaces.iter().map(|n| q(n)).collect(),
                    members: if def.mods.is_data && all_readonly {
                        strip_readonly(members)
                    } else {
                        members
                    },
                    origin,
                })
            }
        };
        out.push(Stmt { kind, origin });
    }

    fn property(
        &mut self,
        m: &SyntaxNode,
        model: &stir_checker::model_api::ClassModelRef<'db>,
        members: &mut Vec<Member>,
        ctor_init: &mut Vec<Stmt>,
    ) {
        let immutable = m.has_ident("val");
        let mods = m.child(MODIFIERS);
        let vis = mods
            .as_ref()
            .map(member_visibility)
            .unwrap_or(Visibility::Public);
        let is_static = mods.as_ref().is_some_and(|x| x.has_token(STATIC_KW));
        let readonly = immutable || mods.as_ref().is_some_and(|x| x.has_token(READONLY_KW));
        for e in m.children_of(PROPERTY_ELEM) {
            let name = match (e.token(VARIABLE), e.name_text()) {
                (Some(v), _) => SmolStr::new(&v.text()[1..]),
                (None, Some(n)) => SmolStr::new(n),
                _ => continue,
            };
            let ty = stir_checker::model_api::property_ty(self.ctx, model, &name);
            let (native, d) = ty
                .as_ref()
                .map(|t| self.types_doc(t))
                .unwrap_or((None, None));
            let default_node = exprs_of(&e).into_iter().next();
            let mut default = None;
            if let Some(dn) = default_node {
                let v = self.expr(&dn);
                if !readonly && is_constant_expr(&v) {
                    default = Some(v);
                } else if is_static {
                    default = Some(v);
                } else {
                    ctor_init.push(Stmt::expr(Expr::this().prop(name.as_str()).assign(v)));
                }
            }
            members.push(Member::Property(PropertyDecl {
                doc: d.map(|d| DocComment {
                    summary: Vec::new(),
                    tags: vec![DocTag {
                        name: "var".into(),
                        text: d,
                    }],
                }),
                attrs: Vec::new(),
                visibility: vis,
                set_visibility: None,
                is_static,
                readonly: readonly && !is_static,
                ty: native.or_else(|| readonly.then(|| Type::Builtin(BuiltinType::Mixed))),
                name,
                default,
                origin: self.origin(&e),
            }));
        }
    }

    fn method(
        &mut self,
        m: &SyntaxNode,
        model: &stir_checker::model_api::ClassModelRef<'db>,
        members: &mut Vec<Member>,
    ) {
        let ptr = stir_hir::NodePtr::new(m);
        let Some(md) = model.def.methods.iter().find(|x| x.ptr == ptr) else {
            return;
        };
        let sig = stir_checker::model_api::method_sig_view(self.ctx, model, md);
        let mut doc = DocComment::default();
        self.template_tags(&sig.type_params, &mut doc);
        let saved = std::mem::replace(&mut self.frames, vec![FxHashMap::default()]);
        let params = self.params(m.child(PARAM_LIST), Some(&sig), &mut doc);
        let is_ctor = md.name.eq_ignore_ascii_case("__construct");
        let (ret_native, ret_doc) = if is_ctor {
            (None, None)
        } else {
            self.types_doc(&sig.ret)
        };
        if let Some(d) = ret_doc {
            doc.tags.push(DocTag {
                name: "return".into(),
                text: d,
            });
        }
        let body = md.has_body.then(|| self.body_stmts(m, &sig.ret));
        self.frames = saved;
        let interface = model.kind() == stir_hir::ClassKind::Interface;
        members.push(Member::Method(MethodDecl {
            doc: (!doc.is_empty()).then_some(doc),
            attrs: Vec::new(),
            is_final: md.mods.is_final,
            is_abstract: !interface && (md.mods.is_abstract || !md.has_body),
            visibility: Some(match md.mods.visibility {
                stir_hir::Visibility::Public => Visibility::Public,
                stir_hir::Visibility::Protected => Visibility::Protected,
                stir_hir::Visibility::Private => Visibility::Private,
            }),
            is_static: md.mods.is_static,
            by_ref: false,
            name: md.name.clone(),
            params,
            return_type: ret_native,
            body: if interface { None } else { body },
            origin: self.origin(m),
        }));
    }

    fn class_qname(&self, name: &str, node: &SyntaxNode) -> QName {
        match name.to_ascii_lowercase().as_str() {
            "self" | "static" | "parent" => QName {
                name: SmolStr::new(name),
                fully_qualified: false,
            },
            _ => {
                let scope = self.items.scope_at(node.text_range().start());
                q(scope.resolve_class(name).as_str())
            }
        }
    }

    fn class_ref(&self, node: &SyntaxNode) -> ClassRef {
        let t = text(node);
        match t.to_ascii_lowercase().as_str() {
            "self" => ClassRef::SelfRef,
            "static" => ClassRef::Static,
            "parent" => ClassRef::Parent,
            _ => ClassRef::Name(self.class_qname(&t, node)),
        }
    }

    // ------------------------------------------------------------ expressions

    fn args(&mut self, list: &SyntaxNode) -> Vec<Arg> {
        let mut out = Vec::new();
        for a in list.children() {
            match a.kind() {
                ARG => {
                    let Some(e) = exprs_of(&a).into_iter().next() else {
                        continue;
                    };
                    let v = self.expr(&e);
                    out.push(if a.has_token(ELLIPSIS) {
                        Arg::Spread(v)
                    } else {
                        Arg::Positional(v)
                    });
                }
                NAMED_ARG => {
                    let Some(n) = a.name_text() else { continue };
                    if let Some(e) = exprs_of(&a).into_iter().next() {
                        let v = self.expr(&e);
                        out.push(Arg::Named(n.into(), v));
                    }
                }
                LAMBDA_EXPR => {
                    let v = self.expr(&a);
                    out.push(Arg::Positional(v));
                }
                CALLABLE_PLACEHOLDER => out.push(Arg::Placeholder),
                _ => {}
            }
        }
        out
    }

    /// All arguments of a call node (`ARG_LIST`s plus a trailing lambda).
    fn call_args(&mut self, call: &SyntaxNode) -> Vec<Arg> {
        let mut out = Vec::new();
        for c in call.children() {
            match c.kind() {
                ARG_LIST => out.extend(self.args(&c)),
                ARG => {
                    if let Some(e) = c.children().find(|x| stir_checker_is_expr(x.kind())) {
                        let v = self.expr(&e);
                        out.push(Arg::Positional(v));
                    }
                }
                _ => {}
            }
        }
        out
    }

    pub(crate) fn expr(&mut self, node: &SyntaxNode) -> Expr {
        let origin = self.origin(node);
        let e = self.expr_inner(node);
        if e.origin.is_none() {
            e.with_origin(origin)
        } else {
            e
        }
    }

    fn conditional<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.conditional += 1;
        let r = f(self);
        self.conditional -= 1;
        r
    }

    fn expr_inner(&mut self, node: &SyntaxNode) -> Expr {
        match node.kind() {
            LITERAL => {
                let Some(tok) = first_token(node) else {
                    return Expr::null();
                };
                match tok.kind() {
                    INT_NUMBER => match parse_int(tok.text()) {
                        Some(v) => Expr::int(v),
                        None => Expr::raw(tok.text().to_owned()),
                    },
                    _ => match tok.text().replace('_', "").parse::<f64>() {
                        Ok(v) => Expr::float(v),
                        Err(_) => Expr::raw(tok.text().to_owned()),
                    },
                }
            }
            STRING_LITERAL => Expr::string(unquote(&text(node))),
            INTERP_STRING => {
                let mut parts = Vec::new();
                for c in node.children_with_tokens() {
                    match c {
                        rowan::NodeOrToken::Token(t) if t.kind() == STRING_CONTENT => {
                            parts.push(InterpPart::Lit(decode_dq(t.text())))
                        }
                        rowan::NodeOrToken::Node(n)
                            if matches!(n.kind(), INTERP_EXPR | INTERP_VAR) =>
                        {
                            if let Some(e) = exprs_of(&n).into_iter().next() {
                                let v = self.expr(&e);
                                parts.push(InterpPart::Expr(v));
                            }
                        }
                        _ => {}
                    }
                }
                Expr::interp(parts)
            }
            HEREDOC | SHELL_EXEC | MAGIC_CONST => Expr::raw(text(node)),
            PAREN_EXPR => exprs_of(node)
                .first()
                .map(|e| self.expr(e))
                .unwrap_or_else(Expr::null),
            VARIABLE_EXPR => {
                let t = text(node);
                let name = t.trim_start_matches('$');
                if self.typed.implicit_this.contains(&node.text_range()) {
                    return Expr::this().prop(name);
                }
                if let Some(r) = self.renamed(name) {
                    return r;
                }
                Expr::var(name)
            }
            VARIABLE_VARIABLE => Expr::raw(text(node)),
            NAME_EXPR => self.name_expr(node),
            BIN_EXPR => self.binary(node),
            PREFIX_EXPR => {
                let op = op_of(node);
                let Some(e) = exprs_of(node).into_iter().next() else {
                    return Expr::null();
                };
                let v = self.expr(&e);
                match op {
                    Some(BANG) => v.not(),
                    Some(MINUS) => Expr::unary(UnaryOp::Neg, v),
                    Some(PLUS) => Expr::unary(UnaryOp::Plus, v),
                    Some(TILDE) => Expr::unary(UnaryOp::BitNot, v),
                    Some(INC) => Expr::unary(UnaryOp::PreInc, v),
                    Some(DEC) => Expr::unary(UnaryOp::PreDec, v),
                    _ => v,
                }
            }
            POSTFIX_EXPR => {
                let op = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .filter(|t| !t.kind().is_trivia())
                    .last()
                    .map(|t| t.kind());
                let Some(e) = exprs_of(node).into_iter().next() else {
                    return Expr::null();
                };
                let v = self.expr(&e);
                Expr::unary(
                    if op == Some(INC) {
                        UnaryOp::PostInc
                    } else {
                        UnaryOp::PostDec
                    },
                    v,
                )
            }
            ASSIGN_EXPR => self.assign(node),
            TERNARY_EXPR => {
                let es = exprs_of(node);
                if es.len() < 3 {
                    return Expr::null();
                }
                let c = self.expr(&es[0]);
                let (a, b) = self.conditional(|this| (this.expr(&es[1]), this.expr(&es[2])));
                fold_ternary(c, a, b)
            }
            ELVIS_EXPR => {
                let es = exprs_of(node);
                if es.len() < 2 {
                    return Expr::null();
                }
                let l = self.expr(&es[0]);
                let r = self.conditional(|this| this.expr(&es[1]));
                l.coalesce(r)
            }
            NOT_NULL_EXPR => {
                let Some(e) = exprs_of(node).into_iter().next() else {
                    return Expr::null();
                };
                let v = self.expr(&e);
                let err = Expr::new_(
                    "\\TypeError",
                    vec![Arg::Positional(Expr::string(format!(
                        "`{}` was null",
                        text(&e)
                    )))],
                )
                .throw();
                v.coalesce(err)
            }
            IS_EXPR => {
                let Some(e) = exprs_of(node).into_iter().next() else {
                    return Expr::bool(false);
                };
                let v = self.expr(&e);
                let Some(tn) = stir_hir::first_type(node) else {
                    return Expr::bool(false);
                };
                let t = self
                    .typed
                    .pattern_types
                    .get(&tn.text_range())
                    .cloned()
                    .unwrap_or_else(Ty::error);
                let (subject, pre) = self.once(v);
                let check = self.type_check(&subject, &t);
                let check = if node.has_token(BANG) {
                    check.not()
                } else {
                    check
                };
                match pre {
                    Some(assign) => assign_then(assign, check),
                    None => check,
                }
            }
            AS_EXPR => {
                let Some(e) = exprs_of(node).into_iter().next() else {
                    return Expr::null();
                };
                let v = self.expr(&e);
                let Some(tn) = stir_hir::first_type(node) else {
                    return v;
                };
                let t = self
                    .typed
                    .pattern_types
                    .get(&tn.text_range())
                    .cloned()
                    .unwrap_or_else(Ty::error);
                let src = self.ty(&e).unwrap_or_else(Ty::error);
                // statically known to succeed: no runtime check
                if !t.is_error()
                    && !src.is_any_platform()
                    && stir_types::is_assignable(self.ctx, &src, &t)
                {
                    return v;
                }
                // the stdlib's casts at the PHP boundary are trusted
                if self.typed_file_is_stdlib() {
                    return v;
                }
                let safe = node.has_token(QUESTION);
                let tmp = self.fresh("c");
                let check = self.type_check(&Expr::var(tmp.as_str()), &t);
                if matches!(check.kind, ExprKind::Bool(true)) {
                    return v;
                }
                if self.conditional == 0 {
                    // evaluate once, before the statement
                    self.pre.push(Stmt::expr(Expr::var(tmp.as_str()).assign(v)));
                    let failure = if safe {
                        Expr::null()
                    } else {
                        Expr::new_(
                            "\\TypeError",
                            vec![Arg::Positional(Expr::string(format!(
                                "cast to {} failed",
                                stir_types::to_phpdoc(&t)
                            )))],
                        )
                        .throw()
                    };
                    return check.ternary(Expr::var(tmp.as_str()), failure);
                }
                let failure = if safe {
                    Expr::null()
                } else {
                    Expr::new_(
                        "\\TypeError",
                        vec![Arg::Positional(Expr::string(format!(
                            "cast to {} failed",
                            stir_types::to_phpdoc(&t)
                        )))],
                    )
                    .throw()
                };
                assign_then(Expr::var(tmp.as_str()).assign(v), check)
                    .ternary(Expr::var(tmp.as_str()), failure)
            }
            INSTANCEOF_EXPR => {
                let Some(e) = exprs_of(node).into_iter().next() else {
                    return Expr::bool(false);
                };
                let v = self.expr(&e);
                let class = match node.children().nth(1) {
                    Some(r) if r.kind() == NAME_EXPR => self.class_ref(&r),
                    Some(r) => ClassRef::Expr(Box::new(self.expr(&r))),
                    None => return Expr::bool(false),
                };
                v.instanceof(class)
            }
            CAST_EXPR => Expr::raw(text(node)),
            CALL_EXPR => self.call(node),
            METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR => self.method_call(node),
            PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR => self.property_fetch(node),
            STATIC_CALL_EXPR => {
                let Some(fetch) = node.children().next() else {
                    return Expr::null();
                };
                let class = self.static_base(&fetch);
                let method = fetch
                    .child(SyntaxKind::NAME)
                    .map(|n| MemberName::Ident(text(&n).into()))
                    .unwrap_or(MemberName::Ident("__invoke".into()));
                let args = self.call_args(node);
                Expr::new(ExprKind::StaticCall {
                    class,
                    method,
                    args,
                })
            }
            STATIC_PROPERTY_FETCH_EXPR => {
                let class = self.static_base(node);
                let name = node
                    .descendants_with_tokens()
                    .filter_map(|e| e.into_token())
                    .filter(|t| t.kind() == VARIABLE)
                    .last()
                    .map(|t| t.text()[1..].to_owned())
                    .unwrap_or_default();
                Expr::static_prop(class, name.as_str())
            }
            CLASS_CONST_FETCH_EXPR => {
                let name = node
                    .child(SyntaxKind::NAME)
                    .map(|n| text(&n))
                    .unwrap_or_default();
                let class = self.static_base(node);
                if name.eq_ignore_ascii_case("class") {
                    return Expr::new(ExprKind::ClassConst {
                        class,
                        name: MemberName::Ident("class".into()),
                    });
                }
                Expr::class_const(class, name.as_str())
            }
            INDEX_EXPR => {
                let es = exprs_of(node);
                let Some(base) = es.first() else {
                    return Expr::null();
                };
                let bt = self
                    .ty(base)
                    .map(|t| strip_platform(&stir_types::without_null(&t)));
                let b = self.expr(base);
                match es.get(1) {
                    Some(i) => {
                        let iv = self.expr(i);
                        let e = b.index(iv);
                        // `map[k]` is `V?`: a missing key reads as null
                        if bt.as_ref().is_some_and(|t| {
                            matches!(t.kind(), TyKind::Map(_, _) | TyKind::MutableMap(_, _))
                        }) && !is_assign_target(node)
                        {
                            e.coalesce(Expr::null())
                        } else {
                            e
                        }
                    }
                    None => b.push_index(),
                }
            }
            ARRAY_EXPR => {
                let mut items = Vec::new();
                for el in node.children_of(ARRAY_ELEM) {
                    let parts = exprs_of(&el);
                    let spread = el.has_token(ELLIPSIS);
                    match parts.as_slice() {
                        [k, v] => {
                            let k = self.expr(k);
                            let (v, by_ref) = self.maybe_ref(v);
                            items.push(ArrayItem {
                                key: Some(k),
                                value: v,
                                by_ref,
                                spread: false,
                            });
                        }
                        [v] => {
                            let (v, by_ref) = self.maybe_ref(v);
                            items.push(ArrayItem {
                                key: None,
                                value: v,
                                by_ref,
                                spread,
                            });
                        }
                        _ => {}
                    }
                }
                Expr::array(items)
            }
            LIST_EXPR => Expr::raw(text(node)),
            NEW_EXPR => {
                if node.child(ANON_CLASS).is_some() {
                    return Expr::raw(text(node));
                }
                let Some(target) = node
                    .children()
                    .find(|c| c.kind() != ARG_LIST && c.kind() != ARG)
                else {
                    return Expr::null();
                };
                let class = if target.kind() == NAME_EXPR {
                    self.class_ref(&target)
                } else {
                    ClassRef::Expr(Box::new(self.expr(&target)))
                };
                let args = self.call_args(node);
                Expr::new_(class, args)
            }
            CLONE_EXPR => {
                if let Some(e) = exprs_of(node).into_iter().next() {
                    return self.expr(&e).clone_();
                }
                let args = node
                    .child(ARG_LIST)
                    .map(|a| self.args(&a))
                    .unwrap_or_default();
                Expr::call_fn("clone", args)
            }
            CLOSURE_EXPR | ARROW_FN_EXPR => self.php_closure(node),
            LAMBDA_EXPR => self.lambda(node),
            MATCH_EXPR => {
                let subject = exprs_of(node)
                    .first()
                    .map(|e| self.expr(e))
                    .unwrap_or_else(Expr::null);
                let arms = node
                    .children_of(MATCH_ARM)
                    .iter()
                    .map(|arm| {
                        let conds = arm
                            .child(MATCH_CONDITIONS)
                            .map(|c| exprs_of(&c).iter().map(|e| self.expr(e)).collect());
                        let body = exprs_of(arm)
                            .last()
                            .map(|e| self.conditional(|this| this.expr(e)))
                            .unwrap_or_else(Expr::null);
                        MatchArm { conds, body }
                    })
                    .collect();
                Expr::match_(subject, arms)
            }
            WHEN_EXPR => self.when_expr(node),
            IF_EXPR => self.if_expr(node),
            ISSET_EXPR => {
                let es = node
                    .child(ARG_LIST)
                    .map(|a| self.args(&a))
                    .unwrap_or_default();
                Expr::isset(
                    es.into_iter()
                        .filter_map(|a| match a {
                            Arg::Positional(e) => Some(e),
                            _ => None,
                        })
                        .collect(),
                )
            }
            EMPTY_EXPR => {
                let e = node
                    .child(ARG_LIST)
                    .and_then(|a| a.child(ARG))
                    .and_then(|a| exprs_of(&a).into_iter().next());
                e.map(|e| self.expr(&e).empty())
                    .unwrap_or_else(|| Expr::bool(true))
            }
            EXIT_EXPR => {
                let a = node
                    .child(ARG_LIST)
                    .and_then(|a| a.child(ARG))
                    .and_then(|a| exprs_of(&a).into_iter().next())
                    .map(|e| Box::new(self.expr(&e)));
                Expr::new(ExprKind::Exit(a))
            }
            PRINT_EXPR => exprs_of(node)
                .first()
                .map(|e| self.expr(e).print())
                .unwrap_or_else(Expr::null),
            THROW_EXPR => exprs_of(node)
                .first()
                .map(|e| self.expr(e).throw())
                .unwrap_or_else(Expr::null),
            YIELD_EXPR | YIELD_FROM_EXPR | INCLUDE_EXPR | EVAL_EXPR => Expr::raw(text(node)),
            ERROR_SUPPRESS_EXPR => exprs_of(node)
                .first()
                .map(|e| self.expr(e).silence())
                .unwrap_or_else(Expr::null),
            RECORD_LITERAL => self.record_literal(node),
            _ => Expr::raw(text(node)),
        }
    }

    fn maybe_ref(&mut self, n: &SyntaxNode) -> (Expr, bool) {
        if n.kind() == PREFIX_EXPR
            && op_of(n) == Some(AMP)
            && let Some(inner) = exprs_of(n).into_iter().next()
        {
            return (self.expr(&inner), true);
        }
        (self.expr(n), false)
    }

    fn name_expr(&mut self, node: &SyntaxNode) -> Expr {
        let t = text(node);
        match t.to_ascii_lowercase().as_str() {
            "true" => return Expr::bool(true),
            "false" => return Expr::bool(false),
            "null" => return Expr::null(),
            _ => {}
        }
        if self.typed.implicit_this.contains(&node.text_range()) {
            return Expr::this().prop(t.as_str());
        }
        if let Some(r) = self.renamed(&t) {
            return r;
        }
        if t == "this" && self.in_extension {
            return Expr::var("__this");
        }
        if !t.contains('\\') && (self.visible(&t).is_some() || t == "it") {
            return Expr::var(t.as_str());
        }
        Expr::name(t)
    }

    fn static_base(&mut self, node: &SyntaxNode) -> ClassRef {
        match node.children().next() {
            Some(b) if b.kind() == NAME_EXPR => self.class_ref(&b),
            Some(b) => ClassRef::Expr(Box::new(self.expr(&b))),
            None => ClassRef::Static,
        }
    }

    fn binary(&mut self, node: &SyntaxNode) -> Expr {
        let es = exprs_of(node);
        let Some(op) = op_of(node) else {
            return Expr::null();
        };
        if es.len() != 2 {
            return Expr::raw(text(node));
        }
        let lt = self.ty(&es[0]);
        let rt = self.ty(&es[1]);
        let l = self.expr(&es[0]);
        let short_circuit = matches!(op, AMP2 | PIPE2 | AND_KW | OR_KW | QUESTION2);
        let r = if short_circuit {
            self.conditional(|this| this.expr(&es[1]))
        } else {
            self.expr(&es[1])
        };
        let safe = |t: &Option<Ty>| t.as_ref().is_some_and(is_safe_family);
        let both_int = |t: &Option<Ty>| {
            t.as_ref()
                .is_some_and(|t| stir_types::is_assignable(self.ctx, t, &Ty::int()))
        };
        let bop = match op {
            PLUS => BinaryOp::Add,
            MINUS => BinaryOp::Sub,
            STAR => BinaryOp::Mul,
            SLASH => {
                if both_int(&lt) && both_int(&rt) {
                    // StirPHP integer division truncates
                    return Expr::call_fn("\\intdiv", vec![Arg::Positional(l), Arg::Positional(r)]);
                }
                BinaryOp::Div
            }
            PERCENT => BinaryOp::Mod,
            POW => BinaryOp::Pow,
            DOT => BinaryOp::Concat,
            AMP => BinaryOp::BitAnd,
            PIPE => BinaryOp::BitOr,
            CARET => BinaryOp::BitXor,
            SHL => BinaryOp::Shl,
            SHR => BinaryOp::Shr,
            AMP2 => BinaryOp::And,
            PIPE2 => BinaryOp::Or,
            AND_KW => BinaryOp::LogicalAnd,
            OR_KW => BinaryOp::LogicalOr,
            XOR_KW => BinaryOp::LogicalXor,
            // StirPHP `==` is strict (no PHP loose comparison)
            EQ2 => {
                if safe(&lt) || safe(&rt) {
                    BinaryOp::Identical
                } else {
                    BinaryOp::Eq
                }
            }
            NEQ => {
                if safe(&lt) || safe(&rt) {
                    BinaryOp::NotIdentical
                } else {
                    BinaryOp::NotEq
                }
            }
            EQ3 => BinaryOp::Identical,
            NEQ2 => BinaryOp::NotIdentical,
            L_ANGLE => BinaryOp::Lt,
            LTEQ => BinaryOp::Le,
            R_ANGLE => BinaryOp::Gt,
            GTEQ => BinaryOp::Ge,
            SPACESHIP => BinaryOp::Spaceship,
            QUESTION2 => BinaryOp::Coalesce,
            PIPE_GT => return r.call(vec![Arg::Positional(l)]),
            _ => return Expr::raw(text(node)),
        };
        fold_binary(bop, l, r)
    }

    fn assign(&mut self, node: &SyntaxNode) -> Expr {
        let es = exprs_of(node);
        if es.len() != 2 {
            return Expr::raw(text(node));
        }
        let op = op_of(node).unwrap_or(EQ);
        let lt = self.ty(&es[0]);
        let target = self.expr(&es[0]);
        let by_ref = op == EQ && es[1].kind() == PREFIX_EXPR && op_of(&es[1]) == Some(AMP);
        let value = if by_ref {
            exprs_of(&es[1])
                .first()
                .map(|e| self.expr(e))
                .unwrap_or_else(Expr::null)
        } else {
            self.expr(&es[1])
        };
        let aop = match op {
            EQ => {
                return if by_ref {
                    target.assign_ref(value)
                } else {
                    target.assign(value)
                };
            }
            PLUS_EQ => AssignOp::Add,
            MINUS_EQ => AssignOp::Sub,
            STAR_EQ => AssignOp::Mul,
            SLASH_EQ => {
                let rt = self.ty(&es[1]);
                let int = |t: &Option<Ty>| {
                    t.as_ref()
                        .is_some_and(|t| stir_types::is_assignable(self.ctx, t, &Ty::int()))
                };
                if int(&lt) && int(&rt) {
                    let div = Expr::call_fn(
                        "\\intdiv",
                        vec![Arg::Positional(target.clone()), Arg::Positional(value)],
                    );
                    return target.assign(div);
                }
                AssignOp::Div
            }
            PERCENT_EQ => AssignOp::Mod,
            POW_EQ => AssignOp::Pow,
            DOT_EQ => AssignOp::Concat,
            AMP_EQ => AssignOp::BitAnd,
            PIPE_EQ => AssignOp::BitOr,
            CARET_EQ => AssignOp::BitXor,
            SHL_EQ => AssignOp::Shl,
            SHR_EQ => AssignOp::Shr,
            QUESTION2_EQ => AssignOp::Coalesce,
            _ => return Expr::raw(text(node)),
        };
        target.assign_op(aop, value)
    }

    fn property_fetch(&mut self, node: &SyntaxNode) -> Expr {
        let es = exprs_of(node);
        let Some(base) = es.first() else {
            return Expr::null();
        };
        let nullsafe = node.kind() == NULLSAFE_PROPERTY_FETCH_EXPR;
        let bt = self
            .ty(base)
            .map(|t| strip_platform(&stir_types::without_null(&t)));
        let b = self.expr(base);
        let Some(name) = node.child(SyntaxKind::NAME).map(|n| text(&n)) else {
            return Expr::raw(text(node));
        };
        if bt
            .as_ref()
            .is_some_and(|t| matches!(t.kind(), TyKind::Record(_)))
        {
            // records are arrays
            let e = b.index(Expr::string(name.as_str()));
            return if nullsafe {
                e.coalesce(Expr::null())
            } else {
                e
            };
        }
        if nullsafe {
            b.nullsafe_prop(name.as_str())
        } else {
            b.prop(name.as_str())
        }
    }

    fn call(&mut self, node: &SyntaxNode) -> Expr {
        let Some(callee) = node.children().next() else {
            return Expr::null();
        };
        if callee.kind() == NAME_EXPR {
            let t = text(&callee);
            if self.typed.implicit_this.contains(&callee.text_range()) {
                let args = self.call_args(node);
                return Expr::this().method(t.as_str(), args);
            }
            if !t.contains('\\')
                && (self.visible(&t).is_some() || self.renamed(&t).is_some() || t == "it")
            {
                let f = self.name_expr(&callee);
                let args = self.call_args(node);
                return f.call(args);
            }
            let args = self.call_args(node);
            let name = match self.typed.function_calls.get(&node.text_range()) {
                Some(fqn) => {
                    if fqn.starts_with("Stir\\") && fqn.matches('\\').count() == 1 {
                        self.needs_runtime |= !self.is_runtime_file();
                    }
                    format!("\\{fqn}")
                }
                None => t,
            };
            return Expr::call_fn(name, args);
        }
        let f = self.expr(&callee);
        let args = self.call_args(node);
        f.call(args)
    }

    /// Is the code being lowered right now from the stdlib (possibly inlined)?
    fn typed_file_is_stdlib(&self) -> bool {
        self.file.origin(self.db) == FileOrigin::Stub
    }

    fn is_runtime_file(&self) -> bool {
        self.file.origin(self.db) == FileOrigin::Stub
    }

    fn method_call(&mut self, node: &SyntaxNode) -> Expr {
        let Some(fetch) = node.children().next() else {
            return Expr::null();
        };
        let es = exprs_of(&fetch);
        let Some(base) = es.first() else {
            return Expr::null();
        };
        let nullsafe = fetch.kind() == NULLSAFE_PROPERTY_FETCH_EXPR;
        let name = fetch
            .child(SyntaxKind::NAME)
            .map(|n| text(&n))
            .unwrap_or_default();
        if let Some(loc) = self.typed.extension_calls.get(&node.text_range()).copied() {
            return self.extension_call(node, base, loc, nullsafe);
        }
        let b = self.expr(base);
        let args = self.call_args(node);
        if nullsafe {
            b.nullsafe_method(name.as_str(), args)
        } else {
            b.method(name.as_str(), args)
        }
    }

    /// `recv->ext(args)` → `\Ns\Recv_ext($recv, args)`, or the inlined body.
    fn extension_call(
        &mut self,
        node: &SyntaxNode,
        base: &SyntaxNode,
        loc: ItemLoc,
        nullsafe: bool,
    ) -> Expr {
        let def = self.ctx.function_def(loc);
        let fqn = format!("\\{}\\{}", def.name.namespace(), extension_name(def));
        let fqn = fqn.replace("\\\\", "\\");
        let recv = self.expr(base);
        let args = self.call_args(node);
        let (recv, pre) = if nullsafe {
            self.once(recv)
        } else {
            (recv, None)
        };
        let call = if self.inline && !self.inlining.contains(&loc) {
            self.inline_call(loc, recv.clone(), args.clone())
                .unwrap_or_else(|| {
                    let mut all = vec![Arg::Positional(recv.clone())];
                    all.extend(args.clone());
                    Expr::call_fn(fqn.as_str(), all)
                })
        } else {
            if loc.file.origin(self.db) == FileOrigin::Stub && !self.is_runtime_file() {
                self.needs_runtime = true;
            }
            let mut all = vec![Arg::Positional(recv.clone())];
            all.extend(args);
            Expr::call_fn(fqn.as_str(), all)
        };
        if nullsafe {
            let check = recv.identical(Expr::null()).ternary(Expr::null(), call);
            return match pre {
                Some(assign) => assign_then_value(assign, check),
                None => check,
            };
        }
        call
    }

    /// Inlines an expression-bodied extension function. Arguments that are
    /// not trivial are bound once to temporaries.
    fn inline_call(&mut self, loc: ItemLoc, recv: Expr, args: Vec<Arg>) -> Option<Expr> {
        let def = self.ctx.function_def(loc);
        if !def.expr_body || args.iter().any(|a| !matches!(a, Arg::Positional(_))) {
            return None;
        }
        let root = stir_db::parse(self.db, loc.file).syntax();
        let fnode = def.ptr.to_node(&root);
        let body = fnode
            .child(FUNCTION_BODY)
            .and_then(|b| exprs_of(&b).into_iter().next())?;
        let params: Vec<SmolStr> = def.sig.params.iter().map(|p| p.name.clone()).collect();
        // defaults for missing args
        let defaults: Vec<Option<SyntaxNode>> = fnode
            .child(PARAM_LIST)
            .map(|pl| {
                pl.children_of(PARAM)
                    .iter()
                    .map(|p| exprs_of(p).into_iter().next())
                    .collect()
            })
            .unwrap_or_default();
        let mut values: Vec<Expr> = vec![recv];
        for (i, _) in params.iter().enumerate() {
            match args.get(i) {
                Some(Arg::Positional(e)) => values.push(e.clone()),
                _ => {
                    let d = defaults.get(i).cloned().flatten()?;
                    let typed = stir_checker::typed_file(self.db, self.ctx.ws, loc.file);
                    let saved = std::mem::replace(&mut self.typed, as_typed(typed));
                    let v = self.expr(&d);
                    self.typed = saved;
                    values.push(v);
                }
            }
        }
        // bind non-trivial values to temporaries
        let mut map = FxHashMap::default();
        let mut binds = Vec::new();
        for (i, v) in values.into_iter().enumerate() {
            let name = if i == 0 {
                SmolStr::new("this")
            } else {
                params[i - 1].clone()
            };
            if is_trivial(&v) {
                map.insert(name, v);
            } else {
                let tmp = self.fresh("i");
                binds.push(Expr::var(tmp.as_str()).assign(v));
                map.insert(name, Expr::var(tmp.as_str()));
            }
        }
        // lower the body in the callee's file context
        let typed = stir_checker::typed_file(self.db, self.ctx.ws, loc.file);
        let saved_typed = std::mem::replace(&mut self.typed, as_typed(typed));
        let saved_file = std::mem::replace(&mut self.file, loc.file);
        let saved_items =
            std::mem::replace(&mut self.items, stir_hir::file_items(self.db, loc.file));
        let saved_ext = std::mem::replace(&mut self.in_extension, false);
        self.renames.push(map);
        self.inlining.push(loc);
        let saved_frames = std::mem::replace(&mut self.frames, vec![FxHashMap::default()]);
        let body_expr = self.expr(&body);
        self.frames = saved_frames;
        self.inlining.pop();
        self.renames.pop();
        self.in_extension = saved_ext;
        self.items = saved_items;
        self.file = saved_file;
        self.typed = saved_typed;
        if binds.is_empty() {
            return Some(body_expr);
        }
        if self.conditional == 0 {
            // hoist the bindings before the current statement
            for b in binds {
                self.pre.push(Stmt::expr(b));
            }
            Some(body_expr)
        } else {
            // inside a branch: bind via an immediately-invoked arrow function
            let params: Vec<Param> = Vec::new();
            let mut e = body_expr;
            for b in binds.into_iter().rev() {
                e = assign_then_value(b, e);
            }
            let _ = params;
            Some(e)
        }
    }

    /// Evaluates `e` once: returns the expression to use and an optional
    /// assignment that must run first.
    fn once(&mut self, e: Expr) -> (Expr, Option<Expr>) {
        if is_trivial(&e) {
            return (e, None);
        }
        let tmp = self.fresh("t");
        (
            Expr::var(tmp.as_str()),
            Some(Expr::var(tmp.as_str()).assign(e)),
        )
    }

    /// A runtime check that `v` has type `t` (erasure semantics).
    fn type_check(&self, v: &Expr, t: &Ty) -> Expr {
        let call = |f: &str| Expr::call_fn(f, vec![Arg::Positional(v.clone())]);
        match strip_platform(t).kind() {
            TyKind::Int | TyKind::PhpInt | TyKind::IntRange { .. } => call("\\is_int"),
            TyKind::Float | TyKind::PhpFloat => call("\\is_float"),
            TyKind::String | TyKind::PhpString | TyKind::NonEmptyString | TyKind::NumericString => {
                call("\\is_string")
            }
            TyKind::Bool | TyKind::PhpBool => call("\\is_bool"),
            TyKind::Null | TyKind::Unit | TyKind::Void => v.clone().identical(Expr::null()),
            TyKind::List(_) | TyKind::MutableList(_) | TyKind::PhpList { .. } => call("\\is_array")
                .and(Expr::call_fn(
                    "\\array_is_list",
                    vec![Arg::Positional(v.clone())],
                )),
            TyKind::Map(_, _)
            | TyKind::MutableMap(_, _)
            | TyKind::Array { .. }
            | TyKind::Shape(_) => call("\\is_array"),
            TyKind::Record(r) => {
                let mut e = call("\\is_array");
                for f in &r.fields {
                    e = e.and(Expr::call_fn(
                        "\\array_key_exists",
                        vec![
                            Arg::Positional(Expr::string(f.name.as_str())),
                            Arg::Positional(v.clone()),
                        ],
                    ));
                }
                e
            }
            TyKind::Fn(_) | TyKind::Callable { closure: true, .. } => {
                v.clone().instanceof("\\Closure")
            }
            TyKind::Callable { .. } => call("\\is_callable"),
            TyKind::Object => call("\\is_object"),
            TyKind::Iterable { .. } => call("\\is_iterable"),
            TyKind::Class { name, .. } => v.clone().instanceof(format!("\\{name}").as_str()),
            TyKind::SelfType { class, .. } => self.type_check(v, class),
            TyKind::Literal(l) => {
                let lit = match &l.value {
                    LitValue::Int(i) => Expr::int(*i),
                    LitValue::Float(f) => Expr::float(f.get()),
                    LitValue::String(s) => Expr::string(s.as_str()),
                    LitValue::Bool(b) => Expr::bool(*b),
                };
                v.clone().identical(lit)
            }
            TyKind::Union(_) => {
                let checks: Vec<Expr> = t.members().iter().map(|m| self.type_check(v, m)).collect();
                if checks
                    .iter()
                    .any(|c| matches!(c.kind, ExprKind::Bool(true)))
                {
                    return Expr::bool(true);
                }
                checks
                    .into_iter()
                    .reduce(|a, b| a.or(b))
                    .unwrap_or_else(|| Expr::bool(false))
            }
            TyKind::Intersection(_) => t
                .members()
                .iter()
                .map(|m| self.type_check(v, m))
                .filter(|c| !matches!(c.kind, ExprKind::Bool(true)))
                .reduce(|a, b| a.and(b))
                .unwrap_or_else(|| Expr::bool(true)),
            TyKind::Nothing | TyKind::Never => Expr::bool(false),
            _ => Expr::bool(true),
        }
    }

    fn record_literal(&mut self, node: &SyntaxNode) -> Expr {
        let ty = self.ty(node);
        let Some(TyKind::Record(r)) = ty.as_ref().map(|t| t.kind().clone()) else {
            return Expr::array(Vec::new());
        };
        let mut given: FxHashMap<SmolStr, Expr> = FxHashMap::default();
        for f in node.children_of(RECORD_LITERAL_FIELD) {
            if let (Some(n), Some(e)) = (f.name_text(), exprs_of(&f).into_iter().next()) {
                let v = self.expr(&e);
                given.insert(SmolStr::new(n), v);
            }
        }
        // declaration order, with defaults from the record declaration
        let decl_defaults = self.record_defaults(r.name.as_deref());
        let items = r
            .fields
            .iter()
            .map(|f| {
                let v = given
                    .remove(&f.name)
                    .or_else(|| decl_defaults.get(&f.name).cloned())
                    .unwrap_or_else(Expr::null);
                ArrayItem {
                    key: Some(Expr::string(f.name.as_str())),
                    value: v,
                    by_ref: false,
                    spread: false,
                }
            })
            .collect();
        Expr::array(items)
    }

    fn record_defaults(&mut self, name: Option<&str>) -> FxHashMap<SmolStr, Expr> {
        let mut out = FxHashMap::default();
        let Some(model) = name.and_then(|n| self.ctx.class(n)) else {
            return out;
        };
        let root = stir_db::parse(self.db, model.file).syntax();
        let node = model.def.ptr.to_node(&root);
        for f in node.children_of(RECORD_FIELD) {
            if let (Some(n), Some(e)) = (f.name_text(), exprs_of(&f).into_iter().next()) {
                // defaults are constant expressions; lower them without file context
                let v = Expr::raw(text(&e));
                out.insert(SmolStr::new(n), v);
            }
        }
        out
    }

    // ------------------------------------------------------------ functions as values

    /// Outer locals referenced inside `node` (by `$name` or bare name).
    fn captures(&self, node: &SyntaxNode, own: &FxHashSet<SmolStr>) -> Vec<(SmolStr, Bind)> {
        let mut seen = FxHashSet::default();
        let mut out = Vec::new();
        for d in node.descendants() {
            let name = match d.kind() {
                VARIABLE_EXPR => SmolStr::new(text(&d).trim_start_matches('$')),
                NAME_EXPR => SmolStr::new(text(&d)),
                _ => continue,
            };
            if name == "this" || own.contains(&name) || !seen.insert(name.clone()) {
                continue;
            }
            if let Some(b) = self.visible(&name) {
                out.push((name, b));
            }
        }
        out
    }

    fn lambda(&mut self, node: &SyntaxNode) -> Expr {
        let fn_ty = self.ty(node).map(|t| strip_platform(&t));
        let (param_tys, ret) = match fn_ty.as_ref().map(|t| t.kind().clone()) {
            Some(TyKind::Fn(f)) => (
                f.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>(),
                f.ret.clone(),
            ),
            _ => (Vec::new(), Ty::error()),
        };
        let mut own: FxHashSet<SmolStr> = FxHashSet::default();
        let mut names: Vec<SmolStr> = Vec::new();
        match node.child(LAMBDA_PARAM_LIST) {
            Some(pl) => {
                for p in pl.children_of(PARAM) {
                    let n = match (p.token(VARIABLE), p.name_text()) {
                        (Some(v), _) => SmolStr::new(&v.text()[1..]),
                        (None, Some(n)) => SmolStr::new(n),
                        _ => continue,
                    };
                    names.push(n);
                }
            }
            None if param_tys.len() == 1 => names.push("it".into()),
            None => {}
        }
        own.extend(names.iter().cloned());
        // locals declared inside the lambda body
        for d in node.descendants() {
            match d.kind() {
                VAL_DECL => {
                    if let Some(n) = d.name_text() {
                        own.insert(SmolStr::new(n));
                    }
                }
                VAR_DECL => {
                    if let Some(v) = d.token(VARIABLE) {
                        own.insert(SmolStr::new(&v.text()[1..]));
                    }
                }
                _ => {}
            }
        }
        let caps = self.captures(node, &own);
        let params: Vec<Param> = names
            .iter()
            .enumerate()
            .map(|(i, n)| Param {
                ty: param_tys.get(i).and_then(native_type),
                name: n.clone(),
                ..Param::default()
            })
            .collect();
        self.frames
            .push(names.iter().map(|n| (n.clone(), Bind::Val)).collect());
        let stmts: Vec<SyntaxNode> = node.children().filter(|c| is_stmt(c.kind())).collect();
        let unit = matches!(ret.kind(), TyKind::Unit | TyKind::Void);
        let mutates_outer = caps.iter().any(|(_, b)| *b == Bind::Var);
        // a single expression and no mutable captures: an arrow function
        if stmts.len() == 1 && stmts[0].kind() == EXPR_STMT && !mutates_outer && !unit {
            let e = exprs_of(&stmts[0]).into_iter().next();
            let saved = std::mem::take(&mut self.pre);
            let body = self.conditional(|this| e.map(|e| this.expr(&e)).unwrap_or_else(Expr::null));
            self.pre = saved;
            self.frames.pop();
            return Expr::new(ExprKind::ArrowFn(ArrowFn {
                attrs: Vec::new(),
                is_static: false,
                by_ref: false,
                params,
                return_type: native_type(&ret).filter(|_| !ret.is_error()),
                body: Box::new(body),
            }));
        }
        let mut body = Vec::new();
        let n = stmts.len();
        for (i, s) in stmts.iter().enumerate() {
            if i + 1 == n && !unit && matches!(s.kind(), EXPR_STMT) {
                if let Some(e) = exprs_of(s).into_iter().next() {
                    let saved = std::mem::take(&mut self.pre);
                    let v = self.expr(&e);
                    let pre = std::mem::replace(&mut self.pre, saved);
                    body.extend(pre);
                    body.push(Stmt::ret(Some(v)));
                }
            } else {
                self.stmt(s, &mut body);
            }
        }
        self.frames.pop();
        let uses = caps
            .into_iter()
            .map(|(n, b)| ClosureUse {
                name: n,
                by_ref: b == Bind::Var,
            })
            .collect();
        Expr::closure(Closure {
            attrs: Vec::new(),
            is_static: false,
            by_ref: false,
            params,
            uses,
            return_type: native_type(&ret).filter(|_| !ret.is_error()),
            body,
        })
    }

    /// PHP-style `function () use (...) {}` / `fn () =>` written in a `.sphp` file.
    fn php_closure(&mut self, node: &SyntaxNode) -> Expr {
        let mut doc = DocComment::default();
        self.frames.push(FxHashMap::default());
        let params = self.params(node.child(PARAM_LIST), None, &mut doc);
        let return_type = node
            .child(RETURN_TYPE)
            .and_then(|r| stir_hir::first_type(&r))
            .and_then(|t| self.typed.types.get(&t.text_range()).and_then(native_type));
        let out = if node.kind() == ARROW_FN_EXPR {
            let body = exprs_of(node)
                .last()
                .map(|e| self.conditional(|this| this.expr(e)))
                .unwrap_or_else(Expr::null);
            Expr::new(ExprKind::ArrowFn(ArrowFn {
                attrs: Vec::new(),
                is_static: node.has_token(STATIC_KW),
                by_ref: false,
                params,
                return_type,
                body: Box::new(body),
            }))
        } else {
            let uses = node
                .child(CLOSURE_USE)
                .map(|u| {
                    u.children_of(CLOSURE_USE_VAR)
                        .iter()
                        .filter_map(|v| {
                            Some(ClosureUse {
                                name: SmolStr::new(&v.token(VARIABLE)?.text()[1..]),
                                by_ref: v.has_token(AMP),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let body = node
                .child(BLOCK)
                .map(|b| self.block(&b))
                .unwrap_or_default();
            Expr::closure(Closure {
                attrs: Vec::new(),
                is_static: node.has_token(STATIC_KW),
                by_ref: false,
                params,
                uses,
                return_type,
                body,
            })
        };
        self.frames.pop();
        out
    }

    /// Wraps statements in an immediately-invoked closure that captures the
    /// referenced outer locals by reference.
    fn iife(&self, source: &SyntaxNode, body: Vec<Stmt>) -> Expr {
        let caps = self.captures(source, &FxHashSet::default());
        let uses = caps
            .into_iter()
            .map(|(n, _)| ClosureUse {
                name: n,
                by_ref: true,
            })
            .collect();
        Expr::closure(Closure {
            attrs: Vec::new(),
            is_static: false,
            by_ref: false,
            params: Vec::new(),
            uses,
            return_type: None,
            body,
        })
        .call(Vec::new())
    }

    /// The value of a branch (expression, or a block whose last expression is
    /// the value) as an expression.
    fn branch_expr(&mut self, node: &SyntaxNode) -> Expr {
        match node.kind() {
            BLOCK => {
                let stmts: Vec<SyntaxNode> =
                    node.children().filter(|c| is_stmt(c.kind())).collect();
                if stmts.len() == 1
                    && stmts[0].kind() == EXPR_STMT
                    && let Some(e) = exprs_of(&stmts[0]).into_iter().next()
                {
                    return self.expr(&e);
                }
                let mut body = Vec::new();
                let n = stmts.len();
                for (i, s) in stmts.iter().enumerate() {
                    if i + 1 == n && s.kind() == EXPR_STMT {
                        if let Some(e) = exprs_of(s).into_iter().next() {
                            let v = self.expr(&e);
                            body.push(Stmt::ret(Some(v)));
                        }
                    } else {
                        self.stmt(s, &mut body);
                    }
                }
                self.iife(node, body)
            }
            k if is_stmt(k) => {
                let mut body = Vec::new();
                self.stmt(node, &mut body);
                self.iife(node, body)
            }
            _ => self.expr(node),
        }
    }

    fn if_expr(&mut self, node: &SyntaxNode) -> Expr {
        let parts: Vec<SyntaxNode> = node.children().collect();
        let Some(cond) = parts.first() else {
            return Expr::null();
        };
        let c = self.expr(cond);
        let a = self.conditional(|this| {
            parts
                .get(1)
                .map(|b| this.branch_expr(b))
                .unwrap_or_else(Expr::null)
        });
        let b = self.conditional(|this| {
            parts
                .get(2)
                .map(|b| this.branch_expr(b))
                .unwrap_or_else(Expr::null)
        });
        fold_ternary(c, a, b)
    }

    /// One `when` condition as a boolean expression on `subject`.
    fn when_condition(&mut self, c: &SyntaxNode, subject: Option<&Expr>) -> Expr {
        if let Some(tn) = stir_hir::first_type(c) {
            let t = self
                .typed
                .pattern_types
                .get(&tn.text_range())
                .cloned()
                .unwrap_or_else(Ty::error);
            let check = self.type_check(subject.unwrap_or(&Expr::null()), &t);
            return if c.has_token(BANG) {
                check.not()
            } else {
                check
            };
        }
        let Some(e) = exprs_of(c).into_iter().next() else {
            return Expr::bool(false);
        };
        let v = self.expr(&e);
        match subject {
            Some(s) => s.clone().identical(v),
            None => v,
        }
    }

    fn when_subject(&mut self, node: &SyntaxNode) -> (Option<Expr>, Option<Expr>) {
        let Some(s) = exprs_of(node).into_iter().next() else {
            return (None, None);
        };
        let v = self.expr(&s);
        if let Some(bound) = node.child(SyntaxKind::NAME).map(|n| text(&n)) {
            self.declare(&bound, Bind::Val);
            return (
                Some(Expr::var(bound.as_str())),
                Some(Expr::var(bound.as_str()).assign(v)),
            );
        }
        let (e, pre) = self.once(v);
        (Some(e), pre)
    }

    fn when_expr(&mut self, node: &SyntaxNode) -> Expr {
        let (subject, pre) = self.when_subject(node);
        let mut arms = Vec::new();
        for arm in node.children_of(WHEN_ARM) {
            let conds: Vec<Expr> = arm
                .children_of(WHEN_CONDITION)
                .iter()
                .map(|c| self.when_condition(c, subject.as_ref()))
                .collect();
            let body = arm.children().filter(|c| c.kind() != WHEN_CONDITION).last();
            let value = self.conditional(|this| {
                body.map(|b| this.branch_expr(&b))
                    .unwrap_or_else(Expr::null)
            });
            if arm.has_token(ELSE_KW) {
                arms.push(MatchArm {
                    conds: None,
                    body: value,
                });
            } else {
                arms.push(MatchArm {
                    conds: Some(conds),
                    body: value,
                });
            }
        }
        let m = Expr::match_(Expr::bool(true), arms);
        match pre {
            Some(assign) if self.conditional == 0 => {
                self.pre.push(Stmt::expr(assign));
                m
            }
            Some(assign) => assign_then_value(assign, m),
            None => m,
        }
    }

    /// `when` used as a statement: an `if`/`elseif` chain.
    fn when_stmt(&mut self, node: &SyntaxNode, out: &mut Vec<Stmt>) {
        let (subject, pre) = self.when_subject(node);
        if let Some(p) = pre {
            out.push(Stmt::expr(p));
        }
        let mut chain: Vec<(Option<Expr>, Vec<Stmt>)> = Vec::new();
        for arm in node.children_of(WHEN_ARM) {
            let conds: Vec<Expr> = arm
                .children_of(WHEN_CONDITION)
                .iter()
                .map(|c| self.when_condition(c, subject.as_ref()))
                .collect();
            let cond = conds.into_iter().reduce(|a, b| a.or(b));
            let body = arm.children().filter(|c| c.kind() != WHEN_CONDITION).last();
            let stmts = match body {
                Some(b) if b.kind() == BLOCK => self.block(&b),
                Some(b) if is_stmt(b.kind()) => self.block(&b),
                Some(b) => {
                    let mut v = Vec::new();
                    let saved = std::mem::take(&mut self.pre);
                    let e = self.expr(&b);
                    v.extend(std::mem::replace(&mut self.pre, saved));
                    v.push(Stmt::expr(e));
                    v
                }
                None => Vec::new(),
            };
            chain.push((if arm.has_token(ELSE_KW) { None } else { cond }, stmts));
        }
        let mut it = chain.into_iter();
        let Some((first_cond, first_body)) = it.next() else {
            return;
        };
        let Some(first_cond) = first_cond else {
            out.extend(first_body);
            return;
        };
        let mut elseifs = Vec::new();
        let mut else_ = None;
        for (c, b) in it {
            match c {
                Some(c) => elseifs.push(ElseIf { cond: c, body: b }),
                None => else_ = Some(b),
            }
        }
        out.push(Stmt {
            kind: StmtKind::If {
                cond: first_cond,
                then: first_body,
                elseifs,
                else_,
            },
            origin: self.origin(node),
        });
    }
}

// ---------------------------------------------------------------- small utilities

/// `(assign) && check` without changing `check`'s value: `(($t = e) || true) && check`.
fn assign_then(assign: Expr, check: Expr) -> Expr {
    // `null !== ($t = e)` would change semantics for null; use an always-true wrapper
    let always = Expr::call_fn("\\is_null", vec![Arg::Positional(assign)]).or(Expr::bool(true));
    always.and(check)
}

/// Evaluates `assign` then yields `value`: `[$t = e, value][1]`.
fn assign_then_value(assign: Expr, value: Expr) -> Expr {
    Expr::array_of(vec![assign, value]).index(Expr::int(1))
}

fn is_constant_expr(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::Name(_)
        | ExprKind::ClassConst { .. } => true,
        ExprKind::Unary { expr, .. } => is_constant_expr(expr),
        ExprKind::Array(items) => items
            .iter()
            .all(|i| is_constant_expr(&i.value) && i.key.as_ref().is_none_or(is_constant_expr)),
        _ => false,
    }
}

fn is_assign_target(node: &SyntaxNode) -> bool {
    node.parent()
        .is_some_and(|p| p.kind() == ASSIGN_EXPR && p.children().next().as_ref() == Some(node))
}

fn member_visibility(m: &SyntaxNode) -> Visibility {
    if m.has_token(PRIVATE_KW) {
        Visibility::Private
    } else if m.has_token(PROTECTED_KW) {
        Visibility::Protected
    } else {
        Visibility::Public
    }
}

/// In a `readonly class`, members must not repeat `readonly`.
fn strip_readonly(members: Vec<Member>) -> Vec<Member> {
    members
        .into_iter()
        .map(|m| match m {
            Member::Property(mut p) => {
                p.readonly = false;
                Member::Property(p)
            }
            Member::Method(mut md) => {
                for p in &mut md.params {
                    if let Some(pr) = &mut p.promote {
                        pr.readonly = false;
                    }
                }
                Member::Method(md)
            }
            other => other,
        })
        .collect()
}

/// Decodes the escape sequences of double-quoted string content.
fn decode_dq(s: &str) -> String {
    unquote(&format!("\"{s}\""))
}

/// The typed info of another file has the same lifetime as the database
/// borrow; this narrows it to the lowerer's lifetime.
fn as_typed<'a>(t: &'a std::sync::Arc<TypedFile>) -> &'a TypedFile {
    t.as_ref()
}

/// Constant folding for code produced by inlining (`null === null`, `'' . $x`).
fn fold_binary(op: BinaryOp, l: Expr, r: Expr) -> Expr {
    let lit = |e: &Expr| -> Option<LitKey> {
        Some(match &e.kind {
            ExprKind::Null => LitKey::Null,
            ExprKind::Bool(b) => LitKey::Bool(*b),
            ExprKind::Int(i) => LitKey::Int(*i),
            ExprKind::Str(s) => LitKey::Str(s.clone()),
            _ => return None,
        })
    };
    match op {
        BinaryOp::Identical | BinaryOp::NotIdentical => {
            if let (Some(a), Some(b)) = (lit(&l), lit(&r)) {
                return Expr::bool((a == b) == (op == BinaryOp::Identical));
            }
        }
        BinaryOp::Concat => {
            if matches!(&l.kind, ExprKind::Str(s) if s.is_empty()) {
                return r;
            }
            if matches!(&r.kind, ExprKind::Str(s) if s.is_empty()) {
                return l;
            }
        }
        BinaryOp::And => match (&l.kind, &r.kind) {
            (ExprKind::Bool(true), _) => return r,
            (_, ExprKind::Bool(true)) => return l,
            _ => {}
        },
        BinaryOp::Or => match (&l.kind, &r.kind) {
            (ExprKind::Bool(false), _) => return r,
            (_, ExprKind::Bool(false)) => return l,
            _ => {}
        },
        _ => {}
    }
    l.bin(op, r)
}

fn fold_ternary(c: Expr, a: Expr, b: Expr) -> Expr {
    match c.kind {
        ExprKind::Bool(true) => a,
        ExprKind::Bool(false) => b,
        _ => c.ternary(a, b),
    }
}

#[derive(PartialEq)]
enum LitKey {
    Null,
    Bool(bool),
    Int(i64),
    Str(SmolStr),
}
