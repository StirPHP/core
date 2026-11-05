//! Declarations of a file, lowered from the syntax tree.

use std::sync::Arc;

use rowan::ast::SyntaxNodePtr;
use smol_str::SmolStr;
use stir_syntax::{PhpLanguage, SyntaxKind, SyntaxKind::*, SyntaxNode, TextRange, TextSize};

use stir_phpdoc::{DocBlock, TagKind};

use crate::ast::NodeExt;
use crate::attrs::{StubAttrs, stub_attrs};
use crate::doc::{self, Aliases, DocCx};
use crate::name::{Fqn, Scope, UseKind};
use crate::type_ref::{TypeCtx, TypeRef, TypeRefKind, unquote};

pub type NodePtr = SyntaxNodePtr<PhpLanguage>;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDef {
    pub name: SmolStr,
    /// The native type (for stubs, the `#[LanguageLevelTypeAware]` one).
    pub ty: Option<TypeRef>,
    /// The PHPDoc `@param` type; merged with `ty` by the checker.
    pub doc_ty: Option<TypeRef>,
    pub has_default: bool,
    /// `= null`: a PHP parameter with this default implicitly accepts null.
    pub default_is_null: bool,
    /// The type of the default value, when it's a literal.
    pub default_ty: Option<TypeRef>,
    /// `@param-out`: what a by-reference parameter holds after the call.
    pub out_ty: Option<TypeRef>,
    /// `@param-closure-this`: what `$this` is inside a closure passed for this parameter.
    pub closure_this: Option<TypeRef>,
    pub variadic: bool,
    pub by_ref: bool,
    /// Constructor promotion: `private int $x`.
    pub promoted: Option<PromotedProperty>,
    pub range: TextRange,
    pub name_range: TextRange,
    pub ptr: NodePtr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotedProperty {
    pub mods: MemberMods,
    /// Promotion is mutable unless readonly.
    pub immutable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Signature {
    /// The `@template` type parameters.
    pub type_params: Vec<TypeParamDef>,
    pub params: Vec<ParamDef>,
    /// The native return type.
    pub ret: Option<TypeRef>,
    /// The PHPDoc `@return` type.
    pub doc_ret: Option<TypeRef>,
    pub by_ref: bool,
    /// `@phpstan-assert-if-true` / `-if-false` on parameters.
    pub asserts: Vec<AssertDef>,
}

/// When an assert holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssertWhen {
    /// `@phpstan-assert`: once the call has returned.
    Always,
    IfTrue,
    IfFalse,
}

/// What a call says about one of its parameters: `@phpstan-assert` and its forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertDef {
    /// The parameter, or `this`.
    pub param: SmolStr,
    /// What's asserted is about this property of it, or (ending in `()`) what this method returns.
    pub member: Option<SmolStr>,
    pub ty: TypeRef,
    /// `!Type`
    pub negated: bool,
    pub when: AssertWhen,
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
    pub sig: Signature,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClassMods {
    pub is_abstract: bool,
    pub is_final: bool,
    pub is_readonly: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyDef {
    pub name: SmolStr,
    /// The native type.
    pub ty: Option<TypeRef>,
    /// The PHPDoc `@var` (or `@property`) type.
    pub doc_ty: Option<TypeRef>,
    /// Declared by a class-level `@property` tag.
    pub magic: bool,
    pub mods: MemberMods,
    /// Can't be written: a `@property-read` tag.
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
    /// Magic (`@method`) methods count as having a body.
    pub has_body: bool,
    /// Declared by a class-level `@method` tag (its `ptr` is the class node).
    pub magic: bool,
    pub doc: Option<DocText>,
    pub ptr: NodePtr,
    pub name_range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassConstDef {
    pub name: SmolStr,
    pub ty: Option<TypeRef>,
    /// The PHPDoc `@var` type.
    pub doc_ty: Option<TypeRef>,
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
pub struct ClassDef {
    pub kind: ClassKind,
    pub name: Fqn,
    pub scope: u32,
    pub mods: ClassMods,
    pub type_params: Vec<TypeParamDef>,
    pub extends: Vec<TypeRef>,
    pub implements: Vec<TypeRef>,
    pub enum_backing: Option<TypeRef>,
    pub properties: Vec<PropertyDef>,
    pub methods: Vec<MethodDef>,
    pub consts: Vec<ClassConstDef>,
    pub cases: Vec<EnumCaseDef>,
    pub traits: Vec<TypeRef>,
    /// `@phpstan-type` / `@psalm-type` aliases declared on the class (for
    /// `@phpstan-import-type` in other classes).
    pub type_aliases: Vec<(SmolStr, TypeRef)>,
    /// Every alias in scope in the class's PHPDoc, imported ones too.
    pub aliases: Aliases,
    /// `@mixin` types: their members are available on this class.
    pub mixins: Vec<TypeRef>,
    /// `use T { method as alias; }`: by alias, the method it stands for.
    pub trait_aliases: Vec<(SmolStr, SmolStr)>,
    pub doc: Option<DocText>,
    pub ptr: NodePtr,
    pub name_range: TextRange,
}

impl ClassDef {
    /// `new class { ... }`: it has a name only inside the checker.
    pub fn is_anonymous(&self) -> bool {
        self.name.as_str().starts_with("class@anonymous")
    }
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

pub(crate) fn lower_file(root: &SyntaxNode, php_version: (u8, u8)) -> FileItems {
    let mut l = Lowerer {
        hash: None,
        php_version,
        out: FileItems::default(),
        scope: Scope::default(),
        aliases: Aliases::default(),
    };
    l.push_scope(TextSize::new(0));
    l.stmts(root);
    l.out
}

/// How a class is named.
enum ClassName<'a> {
    Declared(&'a SyntaxNode),
    Anonymous(String),
}

struct Lowerer {
    /// Hash of the file's text, once needed.
    hash: Option<u64>,
    /// Target version for the phpstorm-stubs attributes.
    php_version: (u8, u8),
    out: FileItems,
    scope: Scope,
    /// Type aliases of the class being lowered.
    aliases: Aliases,
}

impl Lowerer {
    fn push_scope(&mut self, at: TextSize) {
        self.out.scopes.push((at, Arc::new(self.scope.clone())));
    }

    fn scope_id(&self) -> u32 {
        (self.out.scopes.len() - 1) as u32
    }

    fn cur_scope(&self) -> &Scope {
        &self.out.scopes.last().unwrap().1
    }

    fn doc_cx<'a>(&'a self, type_params: &'a [SmolStr]) -> DocCx<'a> {
        DocCx {
            scope: self.cur_scope(),
            type_params,
            aliases: &self.aliases,
        }
    }

    /// The parsed doc comment of `node`.
    fn doc_block(&self, node: &SyntaxNode) -> Option<DocBlock> {
        let t = node.doc_comment()?;
        Some(doc::parse(t.text(), t.text_range().start()))
    }

    fn attrs(&self, node: &SyntaxNode) -> StubAttrs {
        stub_attrs(node, self.cur_scope(), self.php_version)
    }

    /// The `#[LanguageLevelTypeAware]` type, if any (`Some(None)`: no native type).
    fn type_aware(&self, attrs: &StubAttrs, names: &[SmolStr]) -> Option<Option<TypeRef>> {
        let (text, offset) = attrs.type_aware.as_ref()?;
        if text.trim().is_empty() {
            return Some(None);
        }
        Some(Some(doc::lower_type_str(
            &self.doc_cx(names),
            text,
            *offset,
        )))
    }

    fn ctx<'a>(&'a self, type_params: &'a [SmolStr]) -> TypeCtx<'a> {
        TypeCtx {
            scope: &self.out.scopes.last().unwrap().1,
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
            CLASS_DECL | INTERFACE_DECL | TRAIT_DECL | ENUM_DECL => self.class(node),
            CONST_DECL => {
                if self.attrs(node).unavailable {
                    return;
                }
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
                return self.stmts(node);
            }
            _ => {}
        }
        if !matches!(node.kind(), NAMESPACE_DECL | USE_DECL) {
            self.anonymous_classes(node);
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

    fn signature(
        &self,
        node: &SyntaxNode,
        outer: &[SmolStr],
        ctor: bool,
        doc: Option<&DocBlock>,
    ) -> Signature {
        let type_params = doc.map_or_else(Vec::new, |doc| {
            doc::templates(doc, outer, self.cur_scope(), &self.aliases)
        });
        let names = names_of(&type_params, outer);
        let mut params = node
            .child(PARAM_LIST)
            .map(|pl| self.params(&pl, &names, ctor, doc))
            .unwrap_or_default();
        let mut ret = node
            .child(RETURN_TYPE)
            .and_then(|r| first_type(&r))
            .map(|t| self.ctx(&names).lower(&t));
        if let Some(t) = self.type_aware(&self.attrs(node), &names) {
            ret = t;
        }
        let doc_ret = doc
            .and_then(|d| d.return_tag())
            .map(|r| self.doc_cx(&names).lower(&r.ty));
        // `func_get_args()`: the function takes more than it declares
        if node.descendants().any(|d| {
            d.kind() == NAME_EXPR && {
                let name = d.text().to_string().to_ascii_lowercase();
                matches!(
                    name.trim().trim_start_matches('\\'),
                    "func_get_args" | "func_num_args" | "func_get_arg"
                )
            }
        }) && !params.iter().any(|p| p.variadic)
        {
            params.push(ParamDef {
                name: SmolStr::new("__args"),
                ty: None,
                doc_ty: None,
                has_default: true,
                default_is_null: false,
                default_ty: None,
                out_ty: None,
                closure_this: None,
                variadic: true,
                by_ref: false,
                promoted: None,
                range: node.text_range(),
                name_range: node.text_range(),
                ptr: NodePtr::new(node),
            });
        }
        let asserts = doc
            .map(|d| {
                d.effective_tags()
                    .into_iter()
                    .filter_map(|t| match &t.kind {
                        stir_phpdoc::TagKind::Assert(a) => {
                            let target = a.target.as_ref()?;
                            Some(AssertDef {
                                param: target.param.clone(),
                                member: target.member.as_ref().map(|m| {
                                    if m.is_method {
                                        SmolStr::new(format!("{}()", m.name))
                                    } else {
                                        m.name.clone()
                                    }
                                }),
                                ty: self.doc_cx(&names).lower(&a.ty),
                                negated: a.negated,
                                when: match a.kind {
                                    stir_phpdoc::AssertKind::IfTrue => AssertWhen::IfTrue,
                                    stir_phpdoc::AssertKind::IfFalse => AssertWhen::IfFalse,
                                    stir_phpdoc::AssertKind::Assert => AssertWhen::Always,
                                },
                            })
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Signature {
            type_params,
            params,
            ret,
            doc_ret,
            by_ref: node.has_token(AMP),
            asserts,
        }
    }

    fn params(
        &self,
        list: &SyntaxNode,
        names: &[SmolStr],
        ctor: bool,
        doc: Option<&DocBlock>,
    ) -> Vec<ParamDef> {
        list.children_of(PARAM)
            .iter()
            .filter_map(|p| {
                let attrs = self.attrs(p);
                if attrs.unavailable {
                    return None;
                }
                let mut def = self.param(p, names, ctor);
                if let Some(t) = self.type_aware(&attrs, names) {
                    def.ty = t;
                }
                if let Some(doc) = doc {
                    def.doc_ty = doc
                        .param(&def.name)
                        .and_then(|t| t.ty.as_ref())
                        .map(|t| self.doc_cx(names).lower(t));
                    if doc::optional_marker(doc, &def.name) {
                        def.has_default = true;
                    }
                    for tag in doc.effective_tags() {
                        match &tag.kind {
                            TagKind::ParamOut(out)
                                if out.name.as_ref().is_some_and(|n| n.name == def.name) =>
                            {
                                def.out_ty = Some(self.doc_cx(names).lower(&out.ty));
                            }
                            // `@param-closure-this Type $name`
                            TagKind::Generic(g) if tag.name.ends_with("param-closure-this") => {
                                let Some(text) = &g.description else { continue };
                                let Some((ty, name)) = text.text.rsplit_once('$') else {
                                    continue;
                                };
                                if name.split_whitespace().next() == Some(def.name.as_str()) {
                                    let t = stir_phpdoc::parse_type(ty.trim(), text.range.start());
                                    def.closure_this = Some(self.doc_cx(names).lower(&t));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Some(def)
            })
            .collect()
    }

    fn param(&self, p: &SyntaxNode, names: &[SmolStr], ctor: bool) -> ParamDef {
        let (name, name_range) = match p.token(VARIABLE) {
            Some(v) => (SmolStr::new(&v.text()[1..]), v.text_range()),
            None => (SmolStr::default(), p.text_range()),
        };
        let mods = p
            .child(MODIFIERS)
            .map(|m| member_mods(&m))
            .unwrap_or_default();
        let promoted = if ctor && p.child(MODIFIERS).is_some() {
            Some(PromotedProperty {
                immutable: mods.is_readonly,
                mods: mods.clone(),
            })
        } else {
            None
        };
        ParamDef {
            name,
            ty: first_type(p).map(|t| self.ctx(names).lower(&t)),
            doc_ty: None,
            has_default: p.has_token(EQ),
            default_is_null: p.children().any(|c| {
                c.kind() == NAME_EXPR && c.text().to_string().trim().eq_ignore_ascii_case("null")
            }),
            default_ty: p.children().find_map(|c| literal_type_ref(&c)),
            out_ty: None,
            closure_this: None,
            variadic: p.has_token(ELLIPSIS),
            by_ref: p.has_token(AMP),
            promoted,
            range: p.text_range(),
            name_range,
            ptr: NodePtr::new(p),
        }
    }

    fn function(&mut self, node: &SyntaxNode) {
        self.function_item(node);
        // a function declared inside another is global once that one has run
        for nested in node
            .descendants()
            .skip(1)
            .filter(|d| d.kind() == FUNCTION_DECL)
        {
            self.function_item(&nested);
        }
    }

    fn function_item(&mut self, node: &SyntaxNode) {
        let Some(name_node) = node.child(NAME) else {
            return;
        };
        if self.attrs(node).unavailable {
            return;
        }
        let name = name_node.text().to_string().trim().to_owned();
        let doc = self.doc_block(node);
        let sig = self.signature(node, &[], false, doc.as_ref());
        self.out.functions.push(FunctionDef {
            name: Fqn::join(&self.scope.namespace, &name),
            scope: self.scope_id(),
            sig,
            doc: doc_of(node),
            ptr: NodePtr::new(node),
            name_range: name_node.text_range(),
        });
    }

    /// `new class { ... }`: named after where it is, which keeps it apart from those in other
    /// files by the file's text.
    fn anonymous_classes(&mut self, node: &SyntaxNode) {
        for anon in node.descendants().filter(|d| d.kind() == ANON_CLASS) {
            let at = u32::from(anon.text_range().start());
            let name = format!("class@anonymous#{:x}#{at}", self.file_hash(node));
            self.class_inner(&anon, ClassName::Anonymous(name), None, Vec::new());
        }
    }

    fn file_hash(&mut self, node: &SyntaxNode) -> u64 {
        if let Some(h) = self.hash {
            return h;
        }
        use std::hash::{Hash, Hasher};
        let root = node.ancestors().last().unwrap_or_else(|| node.clone());
        let mut hasher = rustc_hash::FxHasher::default();
        root.text().to_string().hash(&mut hasher);
        let h = hasher.finish();
        self.hash = Some(h);
        h
    }

    fn class(&mut self, node: &SyntaxNode) {
        let Some(name_node) = node.child(NAME) else {
            return;
        };
        if self.attrs(node).unavailable {
            return;
        }
        let doc = self.doc_block(node);
        let mut local_aliases = Vec::new();
        if let Some(d) = &doc {
            let native: Vec<SmolStr> = d
                .templates()
                .iter()
                .map(|t| t.param.name.name.clone())
                .collect();
            let (aliases, locals) = doc::class_aliases(d, self.cur_scope(), &native);
            self.aliases = aliases;
            local_aliases = locals;
        }
        self.class_inner(
            node,
            ClassName::Declared(&name_node),
            doc.as_ref(),
            local_aliases,
        );
        self.aliases = Aliases::default();
    }

    fn class_inner(
        &mut self,
        node: &SyntaxNode,
        name: ClassName<'_>,
        doc: Option<&DocBlock>,
        type_aliases: Vec<(SmolStr, TypeRef)>,
    ) {
        let kind = match node.kind() {
            CLASS_DECL => ClassKind::Class,
            INTERFACE_DECL => ClassKind::Interface,
            TRAIT_DECL => ClassKind::Trait,
            _ => ClassKind::Enum,
        };
        let mut mods = ClassMods::default();
        if let Some(m) = node.child(MODIFIERS) {
            for t in m.tokens() {
                match t.kind() {
                    ABSTRACT_KW => mods.is_abstract = true,
                    FINAL_KW => mods.is_final = true,
                    READONLY_KW => mods.is_readonly = true,
                    _ => {}
                }
            }
        }
        let type_params = doc.map_or_else(Vec::new, |d| {
            doc::templates(d, &[], self.cur_scope(), &self.aliases)
        });
        let names = names_of(&type_params, &[]);
        let ctx = self.ctx(&names);
        let types_in = |clause: SyntaxKind| -> Vec<TypeRef> {
            node.child(clause)
                .map(|c| {
                    c.children()
                        .filter_map(|t| {
                            if is_type_node(t.kind()) {
                                Some(ctx.lower(&t))
                            } else if t.kind() == NAME_REF {
                                let text = t.text().to_string();
                                Some(TypeRef {
                                    kind: TypeRefKind::Named(
                                        ctx.scope.resolve_class(text.trim()),
                                        Vec::new(),
                                    ),
                                    range: t.text_range(),
                                })
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut extends = types_in(EXTENDS_CLAUSE);
        let mut implements = types_in(IMPLEMENTS_CLAUSE);
        if let Some(d) = doc {
            let dcx = self.doc_cx(&names);
            for tag in d.effective_tags() {
                match &tag.kind {
                    TagKind::Extends(t) => refine_super(&mut extends, dcx.lower(&t.ty)),
                    TagKind::Implements(t) => refine_super(&mut implements, dcx.lower(&t.ty)),
                    _ => {}
                }
            }
        }
        let enum_backing = if kind == ClassKind::Enum {
            node.children()
                .find(|c| is_type_node(c.kind()))
                .map(|t| ctx.lower(&t))
        } else {
            None
        };
        let mut class = ClassDef {
            kind,
            name: match &name {
                ClassName::Declared(n) => {
                    Fqn::join(&self.scope.namespace, n.text().to_string().trim())
                }
                ClassName::Anonymous(n) => Fqn::new(n),
            },
            scope: self.scope_id(),
            mods,
            type_params,
            extends,
            implements,
            enum_backing,
            properties: Vec::new(),
            methods: Vec::new(),
            consts: Vec::new(),
            cases: Vec::new(),
            traits: Vec::new(),
            type_aliases,
            aliases: self.aliases.clone(),
            mixins: Vec::new(),
            trait_aliases: Vec::new(),
            doc: doc_of(node),
            ptr: NodePtr::new(node),
            name_range: match &name {
                ClassName::Declared(n) => n.text_range(),
                ClassName::Anonymous(_) => node.text_range(),
            },
        };

        if let Some(body) = node.child(CLASS_BODY) {
            for member in body.children() {
                self.member(&member, &mut class, &names);
            }
        }
        if let Some(d) = doc {
            self.magic_members(d, node, &mut class, &names);
            let dcx = self.doc_cx(&names);
            for tag in d.effective_tags() {
                if tag.name.ends_with("mixin")
                    && let TagKind::Generic(g) = &tag.kind
                    && let Some(text) = &g.description
                {
                    let first = text.text.split_whitespace().next().unwrap_or("");
                    let t = stir_phpdoc::parse_type(first, text.range.start());
                    class.mixins.push(dcx.lower(&t));
                }
            }
        }
        self.out.classes.push(class);
    }

    /// `@property*` and `@method` tags on a class.
    fn magic_members(
        &self,
        doc: &DocBlock,
        node: &SyntaxNode,
        class: &mut ClassDef,
        names: &[SmolStr],
    ) {
        let ptr = NodePtr::new(node);
        for tag in doc.effective_tags() {
            match &tag.kind {
                TagKind::Property(p) => {
                    if class.properties.iter().any(|x| x.name == p.name.name) {
                        continue;
                    }
                    class.properties.push(PropertyDef {
                        name: p.name.name.clone(),
                        ty: None,
                        doc_ty: Some(self.doc_cx(names).lower(&p.ty)),
                        magic: true,
                        mods: MemberMods::default(),
                        immutable: p.access == stir_phpdoc::PropertyAccess::Read,
                        has_default: false,
                        doc: None,
                        range: tag.range,
                        ptr,
                    });
                }
                TagKind::Method(m) => {
                    if class
                        .methods
                        .iter()
                        .any(|x| x.name.eq_ignore_ascii_case(&m.name.name))
                    {
                        continue;
                    }
                    let mut tps: Vec<TypeParamDef> = Vec::new();
                    let mut mnames: Vec<SmolStr> =
                        m.templates.iter().map(|t| t.name.name.clone()).collect();
                    mnames.extend(names.iter().cloned());
                    let dcx = self.doc_cx(&mnames);
                    for t in &m.templates {
                        tps.push(TypeParamDef {
                            name: t.name.name.clone(),
                            variance: Variance::Invariant,
                            bound: t.bound.as_ref().map(|b| dcx.lower(b)),
                            range: t.range,
                        });
                    }
                    let params = m
                        .params
                        .iter()
                        .map(|mp| ParamDef {
                            name: mp.name.name.clone(),
                            ty: None,
                            doc_ty: mp.ty.as_ref().map(|t| dcx.lower(t)),
                            has_default: mp.default.is_some(),
                            default_is_null: false,
                            default_ty: None,
                            out_ty: None,
                            closure_this: None,
                            variadic: mp.variadic,
                            by_ref: mp.by_ref,
                            promoted: None,
                            range: mp.range,
                            name_range: mp.name.range,
                            ptr,
                        })
                        .collect();
                    class.methods.push(MethodDef {
                        name: m.name.name.clone(),
                        mods: MemberMods {
                            is_static: m.is_static,
                            ..MemberMods::default()
                        },
                        sig: Signature {
                            type_params: tps,
                            params,
                            ret: None,
                            doc_ret: m.return_type.as_ref().map(|t| dcx.lower(t)),
                            by_ref: false,
                            asserts: Vec::new(),
                        },
                        has_body: true,
                        magic: true,
                        doc: None,
                        ptr,
                        name_range: m.name.range,
                    });
                }
                _ => {}
            }
        }
    }

    fn member(&self, m: &SyntaxNode, class: &mut ClassDef, names: &[SmolStr]) {
        let mods = m
            .child(MODIFIERS)
            .map(|x| member_mods(&x))
            .unwrap_or_default();
        let attrs = self.attrs(m);
        if attrs.unavailable {
            return;
        }
        let doc = self.doc_block(m);
        match m.kind() {
            METHOD_DECL => {
                let Some(name_node) = m.child(NAME) else {
                    return;
                };
                let sig = self.signature(m, names, true, doc.as_ref());
                class.methods.push(MethodDef {
                    name: SmolStr::new(name_node.text().to_string().trim()),
                    mods,
                    sig,
                    magic: false,
                    has_body: m.child(BLOCK).is_some(),
                    doc: doc_of(m),
                    ptr: NodePtr::new(m),
                    name_range: name_node.text_range(),
                });
            }
            PROPERTY_DECL => {
                let mut ty = first_type(m).map(|t| self.ctx(names).lower(&t));
                if let Some(t) = self.type_aware(&attrs, names) {
                    ty = t;
                }
                let doc_vars = doc.as_ref().map(|d| d.vars()).unwrap_or_default();
                for elem in m.children_of(PROPERTY_ELEM) {
                    let Some(var) = elem.token(VARIABLE) else {
                        continue;
                    };
                    let name = SmolStr::new(&var.text()[1..]);
                    let doc_ty = doc_vars
                        .iter()
                        .find(|v| v.name.as_ref().is_none_or(|n| n.name == name))
                        .map(|v| self.doc_cx(names).lower(&v.ty));
                    class.properties.push(PropertyDef {
                        name,
                        ty: ty.clone(),
                        doc_ty,
                        magic: false,
                        mods: mods.clone(),
                        immutable: false,
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
                    let doc_ty = doc
                        .as_ref()
                        .and_then(|d| d.vars().first().map(|v| self.doc_cx(names).lower(&v.ty)));
                    class.consts.push(ClassConstDef {
                        name: SmolStr::new(name),
                        ty: first_type(&elem).map(|t| self.ctx(names).lower(&t)),
                        doc_ty,
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
                if let Some(d) = &doc {
                    let dcx = self.doc_cx(names);
                    for tag in d.effective_tags() {
                        if let TagKind::Use(t) = &tag.kind {
                            refine_super(&mut class.traits, dcx.lower(&t.ty));
                        }
                    }
                }
                if let Some(adaptations) = m.child(TRAIT_ADAPTATIONS) {
                    for alias in adaptations.children_of(TRAIT_ALIAS) {
                        // `[T::]method as [visibility] [alias]`
                        let names: Vec<SmolStr> = alias
                            .children_of(NAME)
                            .iter()
                            .map(|n| SmolStr::new(n.text().to_string().trim()))
                            .collect();
                        if let [method, to] = names.as_slice() {
                            class.trait_aliases.push((to.clone(), method.clone()));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// The type of a literal default value.
fn literal_type_ref(node: &SyntaxNode) -> Option<TypeRef> {
    let text = node.text().to_string();
    let text = text.trim();
    let kind = match node.kind() {
        NAME_EXPR => match text.to_ascii_lowercase().as_str() {
            "true" => TypeRefKind::Php(crate::type_ref::PhpBuiltin::True),
            "false" => TypeRefKind::Php(crate::type_ref::PhpBuiltin::False),
            "null" => TypeRefKind::Php(crate::type_ref::PhpBuiltin::Null),
            _ => return None,
        },
        STRING_LITERAL => TypeRefKind::Literal(crate::type_ref::Literal::String(SmolStr::new(
            unquote(text),
        ))),
        LITERAL => TypeRefKind::Literal(crate::type_ref::Literal::Int(crate::type_ref::parse_int(
            text,
        )?)),
        _ => return None,
    };
    Some(TypeRef {
        kind,
        range: node.text_range(),
    })
}

/// Replaces the native supertype that `doc` (an `@extends`/`@implements` type)
/// refines, giving it type arguments.
fn refine_super(natives: &mut [TypeRef], doc: TypeRef) {
    let TypeRefKind::Named(name, _) = &doc.kind else {
        return;
    };
    if let Some(n) = natives
        .iter_mut()
        .find(|n| matches!(&n.kind, TypeRefKind::Named(m, _) if m.key() == name.key()))
    {
        n.kind = doc.kind;
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
        NAMED_TYPE | NULLABLE_TYPE | UNION_TYPE | INTERSECTION_TYPE | PAREN_TYPE
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
            _ => {}
        }
        i += 1;
    }
    mods
}
