//! Builder helpers to construct the PHP AST concisely.
//!
//! ```
//! use stir_emit::ast::*;
//! let e = Expr::var("a").add(Expr::int(1)).mul(Expr::var("b"));
//! assert_eq!(stir_emit::print::print_expr(&e, &Default::default()), "($a + 1) * $b");
//! ```

use smol_str::SmolStr;

use crate::ast::*;

// ---------------------------------------------------------------------------------------------
// Names and origins
// ---------------------------------------------------------------------------------------------

impl QName {
    /// Parses `Foo\Bar` (unqualified/qualified) or `\Foo\Bar` (fully qualified).
    pub fn new(name: impl AsRef<str>) -> Self {
        let name = name.as_ref();
        match name.strip_prefix('\\') {
            Some(rest) => Self {
                name: rest.into(),
                fully_qualified: true,
            },
            None => Self {
                name: name.into(),
                fully_qualified: false,
            },
        }
    }
    /// A fully-qualified name (leading `\` printed), regardless of a leading `\` in `name`.
    pub fn fq(name: impl AsRef<str>) -> Self {
        Self {
            fully_qualified: true,
            ..Self::new(name)
        }
    }
}

impl From<&str> for QName {
    fn from(s: &str) -> Self {
        QName::new(s)
    }
}

impl From<String> for QName {
    fn from(s: String) -> Self {
        QName::new(s)
    }
}

impl From<SmolStr> for QName {
    fn from(s: SmolStr) -> Self {
        QName::new(s)
    }
}

impl From<QName> for ClassRef {
    fn from(n: QName) -> Self {
        ClassRef::Name(n)
    }
}

impl From<&str> for ClassRef {
    fn from(s: &str) -> Self {
        match s {
            "self" => ClassRef::SelfRef,
            "static" => ClassRef::Static,
            "parent" => ClassRef::Parent,
            _ => ClassRef::Name(QName::new(s)),
        }
    }
}

impl From<Expr> for ClassRef {
    fn from(e: Expr) -> Self {
        ClassRef::Expr(Box::new(e))
    }
}

impl From<&str> for MemberName {
    fn from(s: &str) -> Self {
        MemberName::Ident(s.into())
    }
}

impl From<SmolStr> for MemberName {
    fn from(s: SmolStr) -> Self {
        MemberName::Ident(s)
    }
}

impl From<Expr> for MemberName {
    fn from(e: Expr) -> Self {
        MemberName::Expr(Box::new(e))
    }
}

// ---------------------------------------------------------------------------------------------
// Doc comments and attributes
// ---------------------------------------------------------------------------------------------

impl DocComment {
    pub fn new() -> Self {
        Self::default()
    }
    /// A doc comment with a single tag, e.g. `DocComment::tag("var", "int")`.
    pub fn tag_only(name: impl Into<SmolStr>, text: impl Into<String>) -> Self {
        Self::new().tag(name, text)
    }
    /// Adds a summary line (may contain newlines).
    pub fn line(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.summary.extend(text.split('\n').map(str::to_owned));
        self
    }
    pub fn tag(mut self, name: impl Into<SmolStr>, text: impl Into<String>) -> Self {
        self.tags.push(DocTag {
            name: name.into(),
            text: text.into(),
        });
        self
    }
    pub fn is_empty(&self) -> bool {
        self.summary.is_empty() && self.tags.is_empty()
    }
}

impl Attribute {
    pub fn new(name: impl Into<QName>) -> Self {
        Self {
            name: name.into(),
            args: None,
        }
    }
    pub fn with_args(name: impl Into<QName>, args: Vec<Arg>) -> Self {
        Self {
            name: name.into(),
            args: Some(args),
        }
    }
}

impl AttributeGroup {
    pub fn new(attrs: Vec<Attribute>) -> Self {
        Self { attrs }
    }
    pub fn single(attr: Attribute) -> Self {
        Self { attrs: vec![attr] }
    }
}

// ---------------------------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------------------------

impl Type {
    pub fn int() -> Self {
        Type::Builtin(BuiltinType::Int)
    }
    pub fn float() -> Self {
        Type::Builtin(BuiltinType::Float)
    }
    pub fn string() -> Self {
        Type::Builtin(BuiltinType::String)
    }
    pub fn bool() -> Self {
        Type::Builtin(BuiltinType::Bool)
    }
    pub fn array() -> Self {
        Type::Builtin(BuiltinType::Array)
    }
    pub fn mixed() -> Self {
        Type::Builtin(BuiltinType::Mixed)
    }
    pub fn void() -> Self {
        Type::Builtin(BuiltinType::Void)
    }
    pub fn never() -> Self {
        Type::Builtin(BuiltinType::Never)
    }
    pub fn null() -> Self {
        Type::Builtin(BuiltinType::Null)
    }
    pub fn self_() -> Self {
        Type::Builtin(BuiltinType::SelfType)
    }
    pub fn static_() -> Self {
        Type::Builtin(BuiltinType::Static)
    }
    pub fn class(name: impl Into<QName>) -> Self {
        Type::Class(name.into())
    }
    pub fn nullable(self) -> Self {
        Type::Nullable(Box::new(self))
    }
    pub fn union(types: Vec<Type>) -> Self {
        Type::Union(types)
    }
    pub fn intersection(types: Vec<Type>) -> Self {
        Type::Intersection(types)
    }
}

// ---------------------------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------------------------

impl From<ExprKind> for Expr {
    fn from(kind: ExprKind) -> Self {
        Expr { kind, origin: None }
    }
}

impl From<Expr> for Arg {
    fn from(e: Expr) -> Self {
        Arg::Positional(e)
    }
}

impl From<Expr> for ArrayItem {
    fn from(e: Expr) -> Self {
        ArrayItem::value(e)
    }
}

impl ArrayItem {
    pub fn value(value: Expr) -> Self {
        Self {
            key: None,
            value,
            by_ref: false,
            spread: false,
        }
    }
    pub fn keyed(key: Expr, value: Expr) -> Self {
        Self {
            key: Some(key),
            ..Self::value(value)
        }
    }
    pub fn by_ref(value: Expr) -> Self {
        Self {
            by_ref: true,
            ..Self::value(value)
        }
    }
    pub fn spread(value: Expr) -> Self {
        Self {
            spread: true,
            ..Self::value(value)
        }
    }
}

impl Arg {
    pub fn named(name: impl Into<SmolStr>, value: Expr) -> Self {
        Arg::Named(name.into(), value)
    }
    pub fn spread(value: Expr) -> Self {
        Arg::Spread(value)
    }
}

fn bx(e: Expr) -> Box<Expr> {
    Box::new(e)
}

impl Expr {
    pub fn new(kind: ExprKind) -> Self {
        kind.into()
    }
    pub fn with_origin(mut self, origin: impl Into<Option<Origin>>) -> Self {
        self.origin = origin.into();
        self
    }

    // --- atoms --------------------------------------------------------------------------
    pub fn var(name: impl Into<SmolStr>) -> Self {
        ExprKind::Var(name.into()).into()
    }
    pub fn this() -> Self {
        Self::var("this")
    }
    pub fn int(i: i64) -> Self {
        ExprKind::Int(i).into()
    }
    pub fn float(f: f64) -> Self {
        ExprKind::Float(f).into()
    }
    pub fn string(s: impl Into<SmolStr>) -> Self {
        ExprKind::Str(s.into()).into()
    }
    pub fn bool(b: bool) -> Self {
        ExprKind::Bool(b).into()
    }
    pub fn null() -> Self {
        ExprKind::Null.into()
    }
    /// A constant (or function) name; a leading `\` makes it fully qualified.
    pub fn name(name: impl AsRef<str>) -> Self {
        ExprKind::Name(QName::new(name)).into()
    }
    pub fn magic(m: MagicConst) -> Self {
        ExprKind::MagicConst(m).into()
    }
    pub fn raw(code: impl Into<String>) -> Self {
        ExprKind::Raw(code.into()).into()
    }
    pub fn array(items: Vec<ArrayItem>) -> Self {
        ExprKind::Array(items).into()
    }
    /// A list of values without keys.
    pub fn array_of(values: Vec<Expr>) -> Self {
        Self::array(values.into_iter().map(ArrayItem::value).collect())
    }
    pub fn list(items: Vec<Option<ArrayItem>>) -> Self {
        ExprKind::List(items).into()
    }
    pub fn interp(parts: Vec<InterpPart>) -> Self {
        ExprKind::Interp(parts).into()
    }

    // --- calls and access ---------------------------------------------------------------
    /// Calls a function by name: `Expr::call_fn("\\strlen", args)`.
    pub fn call_fn(name: impl AsRef<str>, args: Vec<Arg>) -> Self {
        Self::name(name).call(args)
    }
    pub fn call(self, args: Vec<Arg>) -> Self {
        ExprKind::Call {
            callee: bx(self),
            args,
        }
        .into()
    }
    /// First-class callable `self(...)`.
    pub fn callable(self) -> Self {
        self.call(vec![Arg::Placeholder])
    }
    pub fn method(self, name: impl Into<MemberName>, args: Vec<Arg>) -> Self {
        ExprKind::MethodCall {
            object: bx(self),
            method: name.into(),
            args,
            nullsafe: false,
        }
        .into()
    }
    pub fn nullsafe_method(self, name: impl Into<MemberName>, args: Vec<Arg>) -> Self {
        ExprKind::MethodCall {
            object: bx(self),
            method: name.into(),
            args,
            nullsafe: true,
        }
        .into()
    }
    pub fn prop(self, name: impl Into<MemberName>) -> Self {
        ExprKind::PropFetch {
            object: bx(self),
            prop: name.into(),
            nullsafe: false,
        }
        .into()
    }
    pub fn nullsafe_prop(self, name: impl Into<MemberName>) -> Self {
        ExprKind::PropFetch {
            object: bx(self),
            prop: name.into(),
            nullsafe: true,
        }
        .into()
    }
    pub fn index(self, index: Expr) -> Self {
        ExprKind::Index {
            base: bx(self),
            index: Some(bx(index)),
        }
        .into()
    }
    /// `$a[]`
    pub fn push_index(self) -> Self {
        ExprKind::Index {
            base: bx(self),
            index: None,
        }
        .into()
    }
    pub fn static_call(
        class: impl Into<ClassRef>,
        name: impl Into<MemberName>,
        args: Vec<Arg>,
    ) -> Self {
        ExprKind::StaticCall {
            class: class.into(),
            method: name.into(),
            args,
        }
        .into()
    }
    pub fn static_prop(class: impl Into<ClassRef>, name: impl Into<MemberName>) -> Self {
        ExprKind::StaticProp {
            class: class.into(),
            prop: name.into(),
        }
        .into()
    }
    pub fn class_const(class: impl Into<ClassRef>, name: impl Into<MemberName>) -> Self {
        ExprKind::ClassConst {
            class: class.into(),
            name: name.into(),
        }
        .into()
    }
    /// `X::class`
    pub fn class_name(class: impl Into<ClassRef>) -> Self {
        Self::class_const(class, "class")
    }
    pub fn new_(class: impl Into<ClassRef>, args: Vec<Arg>) -> Self {
        ExprKind::New {
            class: class.into(),
            args,
        }
        .into()
    }
    pub fn new_anon(class: ClassDecl, args: Vec<Arg>) -> Self {
        ExprKind::NewAnonClass {
            class: Box::new(class),
            args,
        }
        .into()
    }

    // --- operators ----------------------------------------------------------------------
    pub fn unary(op: UnaryOp, e: Expr) -> Self {
        ExprKind::Unary { op, expr: bx(e) }.into()
    }
    #[allow(clippy::should_implement_trait)]
    pub fn not(self) -> Self {
        Self::unary(UnaryOp::Not, self)
    }
    #[allow(clippy::should_implement_trait)]
    pub fn neg(self) -> Self {
        Self::unary(UnaryOp::Neg, self)
    }
    pub fn bin(self, op: BinaryOp, rhs: Expr) -> Self {
        ExprKind::Binary {
            op,
            lhs: bx(self),
            rhs: bx(rhs),
        }
        .into()
    }
    #[allow(clippy::should_implement_trait)]
    pub fn add(self, rhs: Expr) -> Self {
        self.bin(BinaryOp::Add, rhs)
    }
    #[allow(clippy::should_implement_trait)]
    pub fn sub(self, rhs: Expr) -> Self {
        self.bin(BinaryOp::Sub, rhs)
    }
    #[allow(clippy::should_implement_trait)]
    pub fn mul(self, rhs: Expr) -> Self {
        self.bin(BinaryOp::Mul, rhs)
    }
    pub fn concat(self, rhs: Expr) -> Self {
        self.bin(BinaryOp::Concat, rhs)
    }
    pub fn and(self, rhs: Expr) -> Self {
        self.bin(BinaryOp::And, rhs)
    }
    pub fn or(self, rhs: Expr) -> Self {
        self.bin(BinaryOp::Or, rhs)
    }
    pub fn identical(self, rhs: Expr) -> Self {
        self.bin(BinaryOp::Identical, rhs)
    }
    pub fn not_identical(self, rhs: Expr) -> Self {
        self.bin(BinaryOp::NotIdentical, rhs)
    }
    pub fn coalesce(self, rhs: Expr) -> Self {
        strip_null_defaults(self).bin(BinaryOp::Coalesce, rhs)
    }
    pub fn assign(self, value: Expr) -> Self {
        ExprKind::Assign {
            target: bx(self),
            value: bx(value),
        }
        .into()
    }
    pub fn assign_ref(self, value: Expr) -> Self {
        ExprKind::AssignRef {
            target: bx(self),
            value: bx(value),
        }
        .into()
    }
    pub fn assign_op(self, op: AssignOp, value: Expr) -> Self {
        ExprKind::AssignOp {
            op,
            target: bx(self),
            value: bx(value),
        }
        .into()
    }
    pub fn ternary(self, then: Expr, else_: Expr) -> Self {
        ExprKind::Ternary {
            cond: bx(self),
            then: Some(bx(then)),
            else_: bx(else_),
        }
        .into()
    }
    /// `self ?: else_`
    pub fn short_ternary(self, else_: Expr) -> Self {
        ExprKind::Ternary {
            cond: bx(self),
            then: None,
            else_: bx(else_),
        }
        .into()
    }
    pub fn instanceof(self, class: impl Into<ClassRef>) -> Self {
        ExprKind::Instanceof {
            expr: bx(self),
            class: class.into(),
        }
        .into()
    }
    pub fn cast(self, kind: CastKind) -> Self {
        ExprKind::Cast {
            kind,
            expr: bx(self),
        }
        .into()
    }
    pub fn silence(self) -> Self {
        ExprKind::Silence(bx(self)).into()
    }
    pub fn clone_(self) -> Self {
        ExprKind::Clone(bx(self)).into()
    }
    pub fn paren(self) -> Self {
        ExprKind::Paren(bx(self)).into()
    }
    pub fn throw(self) -> Self {
        ExprKind::Throw(bx(self)).into()
    }
    pub fn yield_(value: Option<Expr>) -> Self {
        ExprKind::Yield {
            key: None,
            value: value.map(bx),
        }
        .into()
    }
    pub fn yield_kv(key: Expr, value: Expr) -> Self {
        ExprKind::Yield {
            key: Some(bx(key)),
            value: Some(bx(value)),
        }
        .into()
    }
    pub fn yield_from(self) -> Self {
        ExprKind::YieldFrom(bx(self)).into()
    }
    pub fn print(self) -> Self {
        ExprKind::Print(bx(self)).into()
    }
    pub fn isset(exprs: Vec<Expr>) -> Self {
        ExprKind::Isset(exprs).into()
    }
    pub fn empty(self) -> Self {
        ExprKind::Empty(bx(self)).into()
    }
    pub fn include(kind: IncludeKind, e: Expr) -> Self {
        ExprKind::Include { kind, expr: bx(e) }.into()
    }
    pub fn match_(subject: Expr, arms: Vec<MatchArm>) -> Self {
        ExprKind::Match {
            subject: bx(subject),
            arms,
        }
        .into()
    }
    pub fn arrow_fn(params: Vec<Param>, body: Expr) -> Self {
        ExprKind::ArrowFn(ArrowFn {
            attrs: vec![],
            is_static: false,
            by_ref: false,
            params,
            return_type: None,
            body: bx(body),
        })
        .into()
    }
    pub fn closure(closure: Closure) -> Self {
        ExprKind::Closure(Box::new(closure)).into()
    }

    /// Wraps the expression in an expression statement.
    pub fn stmt(self) -> Stmt {
        Stmt::expr(self)
    }
}

impl MatchArm {
    pub fn new(conds: Vec<Expr>, body: Expr) -> Self {
        Self {
            conds: Some(conds),
            body,
        }
    }
    pub fn default(body: Expr) -> Self {
        Self { conds: None, body }
    }
}

impl ClosureUse {
    pub fn val(name: impl Into<SmolStr>) -> Self {
        Self {
            name: name.into(),
            by_ref: false,
        }
    }
    pub fn by_ref(name: impl Into<SmolStr>) -> Self {
        Self {
            name: name.into(),
            by_ref: true,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------------------------

impl From<StmtKind> for Stmt {
    fn from(kind: StmtKind) -> Self {
        Stmt { kind, origin: None }
    }
}

impl Stmt {
    pub fn new(kind: StmtKind) -> Self {
        kind.into()
    }
    pub fn with_origin(mut self, origin: impl Into<Option<Origin>>) -> Self {
        self.origin = origin.into();
        self
    }
    pub fn expr(e: Expr) -> Self {
        StmtKind::Expr(e).into()
    }
    pub fn echo(exprs: Vec<Expr>) -> Self {
        StmtKind::Echo(exprs).into()
    }
    pub fn ret(e: Option<Expr>) -> Self {
        StmtKind::Return(e).into()
    }
    pub fn if_(cond: Expr, then: Vec<Stmt>) -> Self {
        StmtKind::If {
            cond,
            then,
            elseifs: vec![],
            else_: None,
        }
        .into()
    }
    /// Adds an `elseif` branch to an `if` statement (no-op for other statements).
    pub fn elseif(mut self, cond: Expr, body: Vec<Stmt>) -> Self {
        if let StmtKind::If { elseifs, .. } = &mut self.kind {
            elseifs.push(ElseIf { cond, body });
        }
        self
    }
    /// Sets the `else` branch of an `if` statement (no-op for other statements).
    pub fn else_(mut self, body: Vec<Stmt>) -> Self {
        if let StmtKind::If { else_, .. } = &mut self.kind {
            *else_ = Some(body);
        }
        self
    }
    pub fn while_(cond: Expr, body: Vec<Stmt>) -> Self {
        StmtKind::While { cond, body }.into()
    }
    pub fn foreach(expr: Expr, key: Option<Expr>, value: Expr, body: Vec<Stmt>) -> Self {
        StmtKind::Foreach {
            expr,
            key,
            value,
            by_ref: false,
            body,
        }
        .into()
    }
    pub fn throw(e: Expr) -> Self {
        StmtKind::Throw(e).into()
    }
    pub fn doc(doc: DocComment) -> Self {
        StmtKind::DocComment(doc).into()
    }
    pub fn comment(text: impl Into<String>) -> Self {
        StmtKind::Comment(text.into()).into()
    }
    pub fn nop() -> Self {
        StmtKind::Nop.into()
    }
    pub fn block(stmts: Vec<Stmt>) -> Self {
        StmtKind::Block(stmts).into()
    }
}

impl From<FunctionDecl> for Stmt {
    fn from(d: FunctionDecl) -> Self {
        StmtKind::Function(d).into()
    }
}
impl From<ClassDecl> for Stmt {
    fn from(d: ClassDecl) -> Self {
        StmtKind::Class(d).into()
    }
}
impl From<InterfaceDecl> for Stmt {
    fn from(d: InterfaceDecl) -> Self {
        StmtKind::Interface(d).into()
    }
}
impl From<TraitDecl> for Stmt {
    fn from(d: TraitDecl) -> Self {
        StmtKind::Trait(d).into()
    }
}
impl From<EnumDecl> for Stmt {
    fn from(d: EnumDecl) -> Self {
        StmtKind::Enum(d).into()
    }
}

// ---------------------------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------------------------

impl Param {
    pub fn new(name: impl Into<SmolStr>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
    pub fn ty(mut self, ty: Type) -> Self {
        self.ty = Some(ty);
        self
    }
    pub fn default_value(mut self, e: Expr) -> Self {
        self.default = Some(e);
        self
    }
    pub fn by_ref(mut self) -> Self {
        self.by_ref = true;
        self
    }
    pub fn variadic(mut self) -> Self {
        self.variadic = true;
        self
    }
    pub fn attr(mut self, attr: Attribute) -> Self {
        self.attrs.push(AttributeGroup::single(attr));
        self
    }
    /// Promotes the parameter to a property (`private readonly int $x`).
    pub fn promote(mut self, visibility: Visibility, readonly: bool) -> Self {
        self.promote = Some(Promotion {
            visibility: Some(visibility),
            set_visibility: None,
            readonly,
        });
        self
    }
}

impl FunctionDecl {
    pub fn new(name: impl Into<SmolStr>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
    pub fn doc(mut self, doc: DocComment) -> Self {
        self.doc = Some(doc);
        self
    }
    pub fn param(mut self, p: Param) -> Self {
        self.params.push(p);
        self
    }
    pub fn returns(mut self, ty: Type) -> Self {
        self.return_type = Some(ty);
        self
    }
    pub fn body(mut self, body: Vec<Stmt>) -> Self {
        self.body = body;
        self
    }
}

impl Closure {
    pub fn new(params: Vec<Param>, body: Vec<Stmt>) -> Self {
        Self {
            params,
            body,
            ..Self::default()
        }
    }
}

impl ClassDecl {
    pub fn new(name: impl Into<SmolStr>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
    pub fn doc(mut self, doc: DocComment) -> Self {
        self.doc = Some(doc);
        self
    }
    pub fn final_(mut self) -> Self {
        self.modifiers.is_final = true;
        self
    }
    pub fn abstract_(mut self) -> Self {
        self.modifiers.is_abstract = true;
        self
    }
    pub fn readonly(mut self) -> Self {
        self.modifiers.is_readonly = true;
        self
    }
    pub fn extends(mut self, name: impl Into<QName>) -> Self {
        self.extends = Some(name.into());
        self
    }
    pub fn implements(mut self, name: impl Into<QName>) -> Self {
        self.implements.push(name.into());
        self
    }
    pub fn attr(mut self, attr: Attribute) -> Self {
        self.attrs.push(AttributeGroup::single(attr));
        self
    }
    pub fn member(mut self, m: impl Into<Member>) -> Self {
        self.members.push(m.into());
        self
    }
}

impl InterfaceDecl {
    pub fn new(name: impl Into<SmolStr>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
    pub fn member(mut self, m: impl Into<Member>) -> Self {
        self.members.push(m.into());
        self
    }
}

impl TraitDecl {
    pub fn new(name: impl Into<SmolStr>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
    pub fn member(mut self, m: impl Into<Member>) -> Self {
        self.members.push(m.into());
        self
    }
}

impl EnumDecl {
    pub fn new(name: impl Into<SmolStr>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
    pub fn backed(mut self, ty: Type) -> Self {
        self.backing = Some(ty);
        self
    }
    pub fn member(mut self, m: impl Into<Member>) -> Self {
        self.members.push(m.into());
        self
    }
}

impl EnumCase {
    pub fn new(name: impl Into<SmolStr>, value: Option<Expr>) -> Self {
        Self {
            name: name.into(),
            value,
            ..Self::default()
        }
    }
}

impl PropertyDecl {
    pub fn new(visibility: Visibility, name: impl Into<SmolStr>) -> Self {
        Self {
            doc: None,
            attrs: vec![],
            visibility,
            set_visibility: None,
            is_static: false,
            readonly: false,
            ty: None,
            name: name.into(),
            default: None,
            origin: None,
        }
    }
    pub fn ty(mut self, ty: Type) -> Self {
        self.ty = Some(ty);
        self
    }
    pub fn default_value(mut self, e: Expr) -> Self {
        self.default = Some(e);
        self
    }
    pub fn readonly(mut self) -> Self {
        self.readonly = true;
        self
    }
    pub fn static_(mut self) -> Self {
        self.is_static = true;
        self
    }
    pub fn doc(mut self, doc: DocComment) -> Self {
        self.doc = Some(doc);
        self
    }
}

impl ClassConstDecl {
    pub fn new(name: impl Into<SmolStr>, value: Expr) -> Self {
        Self {
            doc: None,
            attrs: vec![],
            is_final: false,
            visibility: None,
            ty: None,
            name: name.into(),
            value,
            origin: None,
        }
    }
    pub fn visibility(mut self, v: Visibility) -> Self {
        self.visibility = Some(v);
        self
    }
    pub fn ty(mut self, ty: Type) -> Self {
        self.ty = Some(ty);
        self
    }
}

impl MethodDecl {
    pub fn new(visibility: Visibility, name: impl Into<SmolStr>) -> Self {
        Self {
            doc: None,
            attrs: vec![],
            is_final: false,
            is_abstract: false,
            visibility: Some(visibility),
            is_static: false,
            by_ref: false,
            name: name.into(),
            params: vec![],
            return_type: None,
            body: Some(vec![]),
            origin: None,
        }
    }
    pub fn doc(mut self, doc: DocComment) -> Self {
        self.doc = Some(doc);
        self
    }
    pub fn static_(mut self) -> Self {
        self.is_static = true;
        self
    }
    pub fn final_(mut self) -> Self {
        self.is_final = true;
        self
    }
    /// Marks the method abstract and removes its body.
    pub fn abstract_(mut self) -> Self {
        self.is_abstract = true;
        self.body = None;
        self
    }
    /// Removes the body (interface method).
    pub fn no_body(mut self) -> Self {
        self.body = None;
        self
    }
    pub fn param(mut self, p: Param) -> Self {
        self.params.push(p);
        self
    }
    pub fn returns(mut self, ty: Type) -> Self {
        self.return_type = Some(ty);
        self
    }
    pub fn body(mut self, body: Vec<Stmt>) -> Self {
        self.body = Some(body);
        self
    }
}

impl From<TraitUse> for Member {
    fn from(m: TraitUse) -> Self {
        Member::TraitUse(m)
    }
}
impl From<EnumCase> for Member {
    fn from(m: EnumCase) -> Self {
        Member::Case(m)
    }
}
impl From<ClassConstDecl> for Member {
    fn from(m: ClassConstDecl) -> Self {
        Member::Const(m)
    }
}
impl From<PropertyDecl> for Member {
    fn from(m: PropertyDecl) -> Self {
        Member::Property(m)
    }
}
impl From<MethodDecl> for Member {
    fn from(m: MethodDecl) -> Self {
        Member::Method(m)
    }
}

impl UseDecl {
    /// `use Foo\Bar;` (class import).
    pub fn class(name: impl Into<SmolStr>, alias: Option<&str>) -> Self {
        Self::single(UseKind::Class, name, alias)
    }
    pub fn function(name: impl Into<SmolStr>, alias: Option<&str>) -> Self {
        Self::single(UseKind::Function, name, alias)
    }
    pub fn constant(name: impl Into<SmolStr>, alias: Option<&str>) -> Self {
        Self::single(UseKind::Const, name, alias)
    }
    fn single(kind: UseKind, name: impl Into<SmolStr>, alias: Option<&str>) -> Self {
        let name: SmolStr = name.into();
        Self {
            kind,
            group_prefix: None,
            items: vec![UseItem {
                name: name.trim_start_matches('\\').into(),
                alias: alias.map(Into::into),
            }],
        }
    }
}

impl File {
    pub fn new() -> Self {
        Self {
            strict_types: true,
            ..Self::default()
        }
    }
    pub fn namespace(mut self, name: impl Into<SmolStr>) -> Self {
        self.namespace = Some(Namespace {
            name: Some(name.into()),
            braced: false,
        });
        self
    }
    pub fn use_(mut self, u: UseDecl) -> Self {
        self.uses.push(u);
        self
    }
    pub fn stmt(mut self, s: impl Into<Stmt>) -> Self {
        self.stmts.push(s.into());
        self
    }
}

/// `??` tolerates missing keys and nulls along a whole index chain, so
/// `($a['x'] ?? null)['y']` on its left side is just `$a['x']['y']`.
pub(crate) fn strip_null_defaults(e: Expr) -> Expr {
    let Expr { kind, origin } = e;
    let kind = match kind {
        ExprKind::Binary {
            op: BinaryOp::Coalesce,
            lhs,
            rhs,
        } if matches!(rhs.kind, ExprKind::Null) => return strip_null_defaults(*lhs),
        ExprKind::Index { base, index } => ExprKind::Index {
            base: Box::new(strip_null_defaults(*base)),
            index,
        },
        other => other,
    };
    Expr { kind, origin }
}
