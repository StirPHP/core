//! Declarations of a file, lowered from the syntax tree.

use std::sync::Arc;

use rowan::ast::SyntaxNodePtr;
use smol_str::SmolStr;
use stir_syntax::{
    Dialect, StirLanguage, SyntaxKind, SyntaxKind::*, SyntaxNode, TextRange, TextSize,
};

use crate::ast::NodeExt;
use crate::name::{Fqn, Scope, UseKind};
use crate::type_ref::{TypeCtx, TypeRef, unquote};

pub type NodePtr = SyntaxNodePtr<StirLanguage>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variance {
    Invariant,
    In,
    Out,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeParamDef {
    pub name: SmolStr,
    pub variance: Variance,
    pub bound: Option<TypeRef>,
    pub range: TextRange,
}

/// How a parameter binding may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Binding {
    /// StirPHP `name: T` — immutable.
    Val,
    /// StirPHP `mut name: T`.
    Mut,
    /// PHP `$name` (always mutable).
    Php,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Visibility {
    #[default]
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemberMods {
    pub visibility: Visibility,
    /// asymmetric `private(set)` etc.
    pub set_visibility: Option<Visibility>,
    pub is_static: bool,
    pub is_abstract: bool,
    pub is_final: bool,
    pub is_readonly: bool,
    pub is_override: bool,
    pub is_open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDef {
    pub name: SmolStr,
    pub binding: Binding,
    pub ty: Option<TypeRef>,
    pub has_default: bool,
    pub variadic: bool,
    pub by_ref: bool,
    /// Constructor promotion: `private int $x` / StirPHP `val x: Int` in a primary constructor.
    pub promoted: Option<PromotedProperty>,
    pub range: TextRange,
    pub name_range: TextRange,
    pub ptr: NodePtr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotedProperty {
    pub mods: MemberMods,
    /// `val` (immutable) vs `var $x` (mutable); PHP promotion is mutable unless readonly.
    pub immutable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Signature {
    pub type_params: Vec<TypeParamDef>,
    pub params: Vec<ParamDef>,
    pub ret: Option<TypeRef>,
    pub by_ref: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocText {
    pub text: SmolStr,
    pub offset: TextSize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDef {
    pub name: Fqn,
    pub scope: u32,
    /// Extension-function receiver (`function String.isBlank()`).
    pub receiver: Option<TypeRef>,
    pub sig: Signature,
    pub expr_body: bool,
    pub doc: Option<DocText>,
    pub ptr: NodePtr,
    pub name_range: TextRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClassKind {
    Class,
    Interface,
    Trait,
    Enum,
    Record,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClassMods {
    pub is_abstract: bool,
    pub is_final: bool,
    pub is_readonly: bool,
    pub is_open: bool,
    pub is_sealed: bool,
    pub is_data: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupertypeRef {
    pub ty: TypeRef,
    /// `Base(args)` in a StirPHP supertype list: the class being extended.
    pub ctor_call: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyDef {
    pub name: SmolStr,
    pub ty: Option<TypeRef>,
    pub mods: MemberMods,
    /// StirPHP `val` property.
    pub immutable: bool,
    pub has_default: bool,
    pub doc: Option<DocText>,
    pub range: TextRange,
    pub ptr: NodePtr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodDef {
    pub name: SmolStr,
    pub mods: MemberMods,
    pub sig: Signature,
    pub has_body: bool,
    pub expr_body: bool,
    pub doc: Option<DocText>,
    pub ptr: NodePtr,
    pub name_range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassConstDef {
    pub name: SmolStr,
    pub ty: Option<TypeRef>,
    pub mods: MemberMods,
    pub value: Option<NodePtr>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumCaseDef {
    pub name: SmolStr,
    pub value: Option<NodePtr>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFieldDef {
    pub name: SmolStr,
    pub ty: TypeRef,
    pub has_default: bool,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassDef {
    pub kind: ClassKind,
    pub name: Fqn,
    pub scope: u32,
    pub dialect: Dialect,
    pub mods: ClassMods,
    pub type_params: Vec<TypeParamDef>,
    pub extends: Vec<TypeRef>,
    pub implements: Vec<TypeRef>,
    /// StirPHP `: Base(), Iface` list (resolved to extends/implements later).
    pub supertypes: Vec<SupertypeRef>,
    pub primary_ctor: Option<Vec<ParamDef>>,
    pub enum_backing: Option<TypeRef>,
    pub properties: Vec<PropertyDef>,
    pub methods: Vec<MethodDef>,
    pub consts: Vec<ClassConstDef>,
    pub cases: Vec<EnumCaseDef>,
    pub fields: Vec<RecordFieldDef>,
    pub traits: Vec<TypeRef>,
    pub doc: Option<DocText>,
    pub ptr: NodePtr,
    pub name_range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeAliasDef {
    pub name: Fqn,
    pub scope: u32,
    pub type_params: Vec<TypeParamDef>,
    pub ty: TypeRef,
    pub name_range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstDef {
    pub name: Fqn,
    pub scope: u32,
    pub value: Option<NodePtr>,
    pub range: TextRange,
}

/// All declarations of one file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileItems {
    /// Name scopes, each valid from its offset until the next one.
    pub scopes: Vec<(TextSize, Arc<Scope>)>,
    pub functions: Vec<FunctionDef>,
    pub classes: Vec<ClassDef>,
    pub type_aliases: Vec<TypeAliasDef>,
    pub consts: Vec<ConstDef>,
    pub strict_types: bool,
}

impl FileItems {
    pub fn scope(&self, id: u32) -> &Arc<Scope> {
        &self.scopes[id as usize].1
    }

    /// The scope in effect at `offset`.
    pub fn scope_at(&self, offset: TextSize) -> &Arc<Scope> {
        let idx = self.scopes.partition_point(|(start, _)| *start <= offset);
        &self.scopes[idx.saturating_sub(1)].1
    }
}

pub(crate) fn lower_file(root: &SyntaxNode, dialect: Dialect) -> FileItems {
    let mut l = Lowerer {
        dialect,
        out: FileItems::default(),
        scope: Scope::default(),
    };
    l.push_scope(TextSize::new(0));
    l.stmts(root);
    l.out
}

struct Lowerer {
    dialect: Dialect,
    out: FileItems,
    scope: Scope,
}

impl Lowerer {
    fn push_scope(&mut self, at: TextSize) {
        self.out.scopes.push((at, Arc::new(self.scope.clone())));
    }

    fn scope_id(&self) -> u32 {
        (self.out.scopes.len() - 1) as u32
    }

    fn ctx<'a>(&'a self, type_params: &'a [SmolStr]) -> TypeCtx<'a> {
        TypeCtx {
            scope: &self.out.scopes.last().unwrap().1,
            dialect: self.dialect,
            type_params,
        }
    }

    fn stmts(&mut self, parent: &SyntaxNode) {
        for node in parent.children() {
            self.stmt(&node);
        }
    }

    fn stmt(&mut self, node: &SyntaxNode) {
        match node.kind() {
            NAMESPACE_DECL => {
                let ns = node
                    .child(NAME_REF)
                    .map(|n| n.text().to_string().trim().to_owned())
                    .unwrap_or_default();
                self.scope = Scope {
                    namespace: SmolStr::new(ns),
                    ..Scope::default()
                };
                self.push_scope(node.text_range().start());
                if let Some(block) = node.child(BLOCK) {
                    self.stmts(&block);
                    self.scope = Scope::default();
                    self.push_scope(node.text_range().end());
                }
            }
            USE_DECL => {
                self.use_decl(node);
                self.push_scope(node.text_range().end());
            }
            DECLARE_STMT => {
                for item in node.children_of(DECLARE_ITEM) {
                    if item
                        .name_text()
                        .is_some_and(|n| n.eq_ignore_ascii_case("strict_types"))
                        && item.text().to_string().trim_end().ends_with('1')
                    {
                        self.out.strict_types = true;
                    }
                }
                if let Some(block) = node.child(BLOCK) {
                    self.stmts(&block);
                }
            }
            FUNCTION_DECL => self.function(node),
            CLASS_DECL | INTERFACE_DECL | TRAIT_DECL | ENUM_DECL | RECORD_DECL => self.class(node),
            TYPEALIAS_DECL => self.type_alias(node),
            CONST_DECL => {
                for elem in node.children_of(CONST_ELEM) {
                    if let Some(name) = elem.name_text() {
                        self.out.consts.push(ConstDef {
                            name: Fqn::join(&self.scope.namespace, &name),
                            scope: self.scope_id(),
                            value: value_ptr(&elem),
                            range: elem.text_range(),
                        });
                    }
                }
            }
            EXPR_STMT => self.maybe_define(node),
            // Conditional declarations (`if (!function_exists('f')) { function f() {} }`).
            BLOCK | IF_STMT | ELSEIF_CLAUSE | ELSE_CLAUSE | TRY_STMT | CATCH_CLAUSE
            | FINALLY_CLAUSE | WHILE_STMT | FOR_STMT | FOREACH_STMT | SWITCH_STMT | SWITCH_CASE => {
                self.stmts(node)
            }
            _ => {}
        }
    }

    /// `define('NAME', value);`
    fn maybe_define(&mut self, node: &SyntaxNode) {
        let Some(call) = node.child(CALL_EXPR) else {
            return;
        };
        let is_define = call.child(NAME_EXPR).is_some_and(|n| {
            n.text()
                .to_string()
                .trim()
                .trim_start_matches('\\')
                .eq_ignore_ascii_case("define")
        });
        if !is_define {
            return;
        }
        let Some(args) = call.child(ARG_LIST) else {
            return;
        };
        let args = args.children_of(ARG);
        let Some(name_arg) = args.first() else { return };
        let Some(lit) = name_arg.child(STRING_LITERAL) else {
            return;
        };
        let name = unquote(lit.text().to_string().trim());
        self.out.consts.push(ConstDef {
            name: Fqn::new(&name),
            scope: self.scope_id(),
            value: args
                .get(1)
                .and_then(|a| a.children().next())
                .map(|e| NodePtr::new(&e)),
            range: node.text_range(),
        });
    }

    fn use_decl(&mut self, node: &SyntaxNode) {
        let decl_kind = use_kind_of(node);
        for clause in node.children_of(USE_CLAUSE) {
            let kind = use_kind_of(&clause).or(decl_kind).unwrap_or(UseKind::Class);
            let Some(name) = clause.child(NAME_REF) else {
                continue;
            };
            let prefix = name.text().to_string().trim().to_owned();
            if let Some(group) = clause.child(USE_GROUP) {
                for inner in group.children_of(USE_CLAUSE) {
                    let kind = use_kind_of(&inner).or(decl_kind).unwrap_or(UseKind::Class);
                    let Some(n) = inner.child(NAME_REF) else {
                        continue;
                    };
                    let full = format!("{prefix}\\{}", n.text().to_string().trim());
                    let alias = inner.name_text();
                    self.scope.add_use(kind, &full, alias.as_deref());
                }
            } else {
                let alias = clause.name_text();
                self.scope.add_use(kind, &prefix, alias.as_deref());
            }
        }
    }

    fn type_params(&self, node: &SyntaxNode, outer: &[SmolStr]) -> Vec<TypeParamDef> {
        let Some(list) = node.child(TYPE_PARAM_LIST) else {
            return Vec::new();
        };
        let mut out = self.type_params_from_list(&list, outer);
        let names = names_of(&out, outer);
        // `where T of Bound` clauses
        if let Some(wc) = node.child(WHERE_CLAUSE) {
            for pred in wc.children_of(WHERE_PREDICATE) {
                let Some(name) = pred.name_text() else {
                    continue;
                };
                if let Some(tp) = out.iter_mut().find(|t| t.name == name)
                    && let Some(ty) = first_type(&pred)
                {
                    tp.bound = Some(self.ctx(&names).lower(&ty));
                }
            }
        }
        out
    }

    fn signature(&self, node: &SyntaxNode, outer: &[SmolStr], ctor: bool) -> Signature {
        // Type params may appear before the name (`function <T> f`) or after it.
        let mut type_params = Vec::new();
        for list in node.children_of(TYPE_PARAM_LIST) {
            type_params.extend(self.type_params_from_list(&list, outer));
        }
        if let Some(wc) = node.child(WHERE_CLAUSE) {
            let names = names_of(&type_params, outer);
            for pred in wc.children_of(WHERE_PREDICATE) {
                let Some(name) = pred.name_text() else {
                    continue;
                };
                if let Some(tp) = type_params.iter_mut().find(|t| t.name == name)
                    && let Some(ty) = first_type(&pred)
                {
                    tp.bound = Some(self.ctx(&names).lower(&ty));
                }
            }
        }
        let names = names_of(&type_params, outer);
        let params = node
            .child(PARAM_LIST)
            .map(|pl| self.params(&pl, &names, ctor))
            .unwrap_or_default();
        let ret = node
            .child(RETURN_TYPE)
            .and_then(|r| first_type(&r))
            .map(|t| self.ctx(&names).lower(&t));
        Signature {
            type_params,
            params,
            ret,
            by_ref: node.has_token(AMP),
        }
    }

    fn type_params_from_list(&self, list: &SyntaxNode, outer: &[SmolStr]) -> Vec<TypeParamDef> {
        let names: Vec<SmolStr> = list
            .children_of(TYPE_PARAM)
            .iter()
            .filter_map(|tp| tp.name_text().map(SmolStr::new))
            .chain(outer.iter().cloned())
            .collect();
        list.children_of(TYPE_PARAM)
            .iter()
            .filter_map(|tp| {
                let name = SmolStr::new(tp.name_text()?);
                let variance = if tp.has_ident("in") {
                    Variance::In
                } else if tp.has_ident("out") {
                    Variance::Out
                } else {
                    Variance::Invariant
                };
                let bound = first_type(tp).map(|t| self.ctx(&names).lower(&t));
                Some(TypeParamDef {
                    name,
                    variance,
                    bound,
                    range: tp.text_range(),
                })
            })
            .collect()
    }

    fn params(&self, list: &SyntaxNode, names: &[SmolStr], ctor: bool) -> Vec<ParamDef> {
        list.children_of(PARAM)
            .iter()
            .map(|p| self.param(p, names, ctor))
            .collect()
    }

    fn param(&self, p: &SyntaxNode, names: &[SmolStr], ctor: bool) -> ParamDef {
        let var = p.token(VARIABLE);
        let (name, name_range) = match (&var, p.child(NAME)) {
            (Some(v), _) => (SmolStr::new(&v.text()[1..]), v.text_range()),
            (None, Some(n)) => (SmolStr::new(n.text().to_string().trim()), n.text_range()),
            (None, None) => (SmolStr::default(), p.text_range()),
        };
        let mods = p
            .child(MODIFIERS)
            .map(|m| member_mods(&m))
            .unwrap_or_default();
        let has_mods = p.child(MODIFIERS).is_some();
        let is_val = p.has_ident("val");
        let is_var_kw = p.has_token(VAR_KW);
        let binding = if p.has_ident("mut") {
            Binding::Mut
        } else if var.is_some() {
            Binding::Php
        } else {
            Binding::Val
        };
        let promoted = if ctor && (has_mods || is_val || is_var_kw) {
            Some(PromotedProperty {
                immutable: is_val || mods.is_readonly,
                mods: mods.clone(),
            })
        } else {
            None
        };
        ParamDef {
            name,
            binding,
            ty: first_type(p).map(|t| self.ctx(names).lower(&t)),
            has_default: p.has_token(EQ),
            variadic: p.has_token(ELLIPSIS),
            by_ref: p.has_token(AMP),
            promoted,
            range: p.text_range(),
            name_range,
            ptr: NodePtr::new(p),
        }
    }

    fn function(&mut self, node: &SyntaxNode) {
        let Some(name_node) = node.child(NAME) else {
            return;
        };
        let name = name_node.text().to_string().trim().to_owned();
        let sig = self.signature(node, &[], false);
        let names = names_of(&sig.type_params, &[]);
        let receiver = node
            .child(RECEIVER_TYPE)
            .and_then(|r| first_type(&r))
            .map(|t| self.ctx(&names).lower(&t));
        self.out.functions.push(FunctionDef {
            name: Fqn::join(&self.scope.namespace, &name),
            scope: self.scope_id(),
            receiver,
            sig,
            expr_body: node.child(FUNCTION_BODY).is_some(),
            doc: doc_of(node),
            ptr: NodePtr::new(node),
            name_range: name_node.text_range(),
        });
    }

    fn type_alias(&mut self, node: &SyntaxNode) {
        let Some(name_node) = node.child(NAME) else {
            return;
        };
        let type_params = self.type_params(node, &[]);
        let names = names_of(&type_params, &[]);
        let ty = match first_type(node) {
            Some(t) => self.ctx(&names).lower(&t),
            None => return,
        };
        self.out.type_aliases.push(TypeAliasDef {
            name: Fqn::join(&self.scope.namespace, name_node.text().to_string().trim()),
            scope: self.scope_id(),
            type_params,
            ty,
            name_range: name_node.text_range(),
        });
    }

    fn class(&mut self, node: &SyntaxNode) {
        let Some(name_node) = node.child(NAME) else {
            return;
        };
        let kind = match node.kind() {
            CLASS_DECL => ClassKind::Class,
            INTERFACE_DECL => ClassKind::Interface,
            TRAIT_DECL => ClassKind::Trait,
            ENUM_DECL => ClassKind::Enum,
            _ => ClassKind::Record,
        };
        let mut mods = ClassMods::default();
        if let Some(m) = node.child(MODIFIERS) {
            for t in m.tokens() {
                match t.kind() {
                    ABSTRACT_KW => mods.is_abstract = true,
                    FINAL_KW => mods.is_final = true,
                    READONLY_KW => mods.is_readonly = true,
                    IDENT => match t.text() {
                        "open" => mods.is_open = true,
                        "sealed" => mods.is_sealed = true,
                        "data" => mods.is_data = true,
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        let type_params = self.type_params(node, &[]);
        let names = names_of(&type_params, &[]);
        let ctx = self.ctx(&names);
        let types_in = |clause: SyntaxKind| -> Vec<TypeRef> {
            node.child(clause)
                .map(|c| {
                    c.children()
                        .filter(|t| is_type_node(t.kind()))
                        .map(|t| ctx.lower(&t))
                        .collect()
                })
                .unwrap_or_default()
        };
        let extends = types_in(EXTENDS_CLAUSE);
        let implements = types_in(IMPLEMENTS_CLAUSE);
        let supertypes = node
            .child(SUPERTYPE_LIST)
            .map(|l| {
                l.children_of(SUPERTYPE)
                    .iter()
                    .filter_map(|s| {
                        Some(SupertypeRef {
                            ty: ctx.lower(&first_type(s)?),
                            ctor_call: s.child(ARG_LIST).is_some(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let primary_ctor = node
            .child(PRIMARY_CTOR)
            .and_then(|c| c.child(PARAM_LIST))
            .map(|pl| self.params(&pl, &names, true));
        let enum_backing = if kind == ClassKind::Enum {
            node.children()
                .find(|c| is_type_node(c.kind()))
                .map(|t| ctx.lower(&t))
        } else {
            None
        };
        let mut class = ClassDef {
            kind,
            name: Fqn::join(&self.scope.namespace, name_node.text().to_string().trim()),
            scope: self.scope_id(),
            dialect: self.dialect,
            mods,
            type_params,
            extends,
            implements,
            supertypes,
            primary_ctor,
            enum_backing,
            properties: Vec::new(),
            methods: Vec::new(),
            consts: Vec::new(),
            cases: Vec::new(),
            fields: Vec::new(),
            traits: Vec::new(),
            doc: doc_of(node),
            ptr: NodePtr::new(node),
            name_range: name_node.text_range(),
        };
        if kind == ClassKind::Record {
            for f in node.children_of(RECORD_FIELD) {
                let (Some(name), Some(ty)) = (f.name_text(), first_type(&f)) else {
                    continue;
                };
                class.fields.push(RecordFieldDef {
                    name: SmolStr::new(name),
                    ty: ctx.lower(&ty),
                    has_default: f.has_token(EQ),
                    range: f.text_range(),
                });
            }
        }
        if let Some(body) = node.child(CLASS_BODY) {
            for member in body.children() {
                self.member(&member, &mut class, &names);
            }
        }
        self.out.classes.push(class);
    }

    fn member(&self, m: &SyntaxNode, class: &mut ClassDef, names: &[SmolStr]) {
        let mods = m
            .child(MODIFIERS)
            .map(|x| member_mods(&x))
            .unwrap_or_default();
        match m.kind() {
            METHOD_DECL => {
                let Some(name_node) = m.child(NAME) else {
                    return;
                };
                let sig = self.signature(m, names, true);
                class.methods.push(MethodDef {
                    name: SmolStr::new(name_node.text().to_string().trim()),
                    mods,
                    sig,
                    has_body: m.child(BLOCK).is_some() || m.child(FUNCTION_BODY).is_some(),
                    expr_body: m.child(FUNCTION_BODY).is_some(),
                    doc: doc_of(m),
                    ptr: NodePtr::new(m),
                    name_range: name_node.text_range(),
                });
            }
            PROPERTY_DECL => {
                let ty = first_type(m).map(|t| self.ctx(names).lower(&t));
                let immutable = m.has_ident("val");
                for elem in m.children_of(PROPERTY_ELEM) {
                    let name = match (elem.token(VARIABLE), elem.name_text()) {
                        (Some(v), _) => SmolStr::new(&v.text()[1..]),
                        (None, Some(n)) => SmolStr::new(n),
                        _ => continue,
                    };
                    // StirPHP puts the type inside the element (`val x: Int`).
                    let ty = ty
                        .clone()
                        .or_else(|| first_type(&elem).map(|t| self.ctx(names).lower(&t)));
                    class.properties.push(PropertyDef {
                        name,
                        ty,
                        mods: mods.clone(),
                        immutable,
                        has_default: elem.has_token(EQ),
                        doc: doc_of(m),
                        range: elem.text_range(),
                        ptr: NodePtr::new(&elem),
                    });
                }
            }
            CLASS_CONST_DECL => {
                for elem in m.children_of(CONST_ELEM) {
                    let Some(name) = elem.name_text() else {
                        continue;
                    };
                    class.consts.push(ClassConstDef {
                        name: SmolStr::new(name),
                        ty: first_type(&elem).map(|t| self.ctx(names).lower(&t)),
                        mods: mods.clone(),
                        value: value_ptr(&elem),
                        range: elem.text_range(),
                    });
                }
            }
            ENUM_CASE => {
                if let Some(name) = m.name_text() {
                    class.cases.push(EnumCaseDef {
                        name: SmolStr::new(name),
                        value: value_ptr(m),
                        range: m.text_range(),
                    });
                }
            }
            TRAIT_USE => {
                for n in m.children_of(NAME_REF) {
                    let text = n.text().to_string();
                    class.traits.push(TypeRef {
                        kind: crate::type_ref::TypeRefKind::Named(
                            self.out.scopes.last().unwrap().1.resolve_class(text.trim()),
                            Vec::new(),
                        ),
                        range: n.text_range(),
                    });
                }
            }
            _ => {}
        }
    }
}

fn names_of(tps: &[TypeParamDef], outer: &[SmolStr]) -> Vec<SmolStr> {
    tps.iter()
        .map(|t| t.name.clone())
        .chain(outer.iter().cloned())
        .collect()
}

fn use_kind_of(node: &SyntaxNode) -> Option<UseKind> {
    if node.has_token(FUNCTION_KW) {
        Some(UseKind::Function)
    } else if node.has_token(CONST_KW) {
        Some(UseKind::Const)
    } else {
        None
    }
}

pub fn is_type_node(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        NAMED_TYPE
            | NULLABLE_TYPE
            | UNION_TYPE
            | INTERSECTION_TYPE
            | PAREN_TYPE
            | FUNCTION_TYPE
            | LITERAL_TYPE
            | RECORD_TYPE
    )
}

/// The first direct child that is a type node.
pub fn first_type(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.children().find(|c| is_type_node(c.kind()))
}

/// The expression after `=` in a const element / enum case.
fn value_ptr(node: &SyntaxNode) -> Option<NodePtr> {
    let mut after_eq = false;
    for e in node.children_with_tokens() {
        match e {
            rowan::NodeOrToken::Token(t) if t.kind() == EQ => after_eq = true,
            rowan::NodeOrToken::Node(n) if after_eq => return Some(NodePtr::new(&n)),
            _ => {}
        }
    }
    None
}

fn doc_of(node: &SyntaxNode) -> Option<DocText> {
    node.doc_comment().map(|t| DocText {
        text: SmolStr::new(t.text()),
        offset: t.text_range().start(),
    })
}

fn member_mods(m: &SyntaxNode) -> MemberMods {
    let mut mods = MemberMods::default();
    let tokens = m.tokens();
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        let vis = match t.kind() {
            PUBLIC_KW => Some(Visibility::Public),
            PROTECTED_KW => Some(Visibility::Protected),
            PRIVATE_KW => Some(Visibility::Private),
            _ => None,
        };
        if let Some(v) = vis {
            if tokens.get(i + 1).is_some_and(|n| n.kind() == L_PAREN) {
                mods.set_visibility = Some(v);
                i += 4;
                continue;
            }
            mods.visibility = v;
        }
        match t.kind() {
            STATIC_KW => mods.is_static = true,
            ABSTRACT_KW => mods.is_abstract = true,
            FINAL_KW => mods.is_final = true,
            READONLY_KW => mods.is_readonly = true,
            IDENT if t.text() == "override" => mods.is_override = true,
            IDENT if t.text() == "open" => mods.is_open = true,
            _ => {}
        }
        i += 1;
    }
    mods
}
