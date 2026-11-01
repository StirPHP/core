//! Owned PHP output AST.
//!
//! This tree describes *generated* PHP code. It is deliberately simple to construct (see the
//! builder helpers in [`crate::builders`]) and complete enough to express any PHP 8.3 program
//! the transpiler needs to produce. Parenthesization is never stored in the tree: the printer
//! inserts parentheses where PHP precedence requires them (see [`crate::prec`]).
//!
//! Nodes that carry an `origin: Option<Origin>` are recorded in the source map produced by
//! [`crate::print::print_file_with_map`].

use smol_str::SmolStr;

/// An identifier (variable name without `$`, function/method/property/constant name, ...).
pub type Ident = SmolStr;

/// Opaque source location of the StirPHP construct a PHP node was generated from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Origin {
    pub file_id: u32,
    pub start: u32,
    pub end: u32,
}

impl Origin {
    pub fn new(file_id: u32, start: u32, end: u32) -> Self {
        Self {
            file_id,
            start,
            end,
        }
    }
}

/// A (possibly namespaced) PHP name such as `Foo\Bar` or `\strlen`.
///
/// `name` never contains a leading backslash; `fully_qualified` controls whether one is printed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QName {
    pub name: SmolStr,
    pub fully_qualified: bool,
}

// ---------------------------------------------------------------------------------------------
// File level
// ---------------------------------------------------------------------------------------------

/// A complete PHP file. Printed as `<?php`, optional `declare(strict_types=1);`, optional
/// namespace, `use` imports and then the statements.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct File {
    /// File-level doc comment printed right after `<?php`.
    pub doc: Option<DocComment>,
    pub strict_types: bool,
    pub namespace: Option<Namespace>,
    pub uses: Vec<UseDecl>,
    pub stmts: Vec<Stmt>,
}

/// The namespace of a [`File`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Namespace {
    /// `None` is only meaningful for braced namespaces (`namespace { ... }` = global code).
    pub name: Option<SmolStr>,
    /// Braced form `namespace Foo { ... }` wraps the uses and statements.
    pub braced: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum UseKind {
    #[default]
    Class,
    Function,
    Const,
}

/// `use Foo\Bar as Baz;` or grouped `use Foo\{Bar, Baz as Qux};`.
///
/// Without a `group_prefix`, every item is printed as its own `use` statement.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UseDecl {
    pub kind: UseKind,
    pub group_prefix: Option<SmolStr>,
    pub items: Vec<UseItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseItem {
    /// Name without leading backslash (relative to `group_prefix` when grouped).
    pub name: SmolStr,
    pub alias: Option<Ident>,
}

// ---------------------------------------------------------------------------------------------
// Doc comments and attributes
// ---------------------------------------------------------------------------------------------

/// A structured PHPDoc block.
///
/// Printed as
/// ```text
/// /**
///  * summary line
///  *
///  * @tag text
///  */
/// ```
/// or as `/** @tag text */` when there is no summary and exactly one single-line tag that fits.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DocComment {
    /// Free-text lines (an empty string is a blank line).
    pub summary: Vec<String>,
    pub tags: Vec<DocTag>,
}

/// One `@name text` line. `text` may contain newlines; continuation lines are prefixed with ` * `.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocTag {
    /// Tag name without `@` (e.g. `param`, `phpstan-type`).
    pub name: SmolStr,
    pub text: String,
}

/// `#[A, B(1)]`
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AttributeGroup {
    pub attrs: Vec<Attribute>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub name: QName,
    /// `None` prints `#[A]`, `Some(vec![])` prints `#[A()]`.
    pub args: Option<Vec<Arg>>,
}

// ---------------------------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltinType {
    Int,
    Float,
    String,
    Bool,
    Array,
    Object,
    Callable,
    Iterable,
    Mixed,
    Void,
    Never,
    Null,
    False,
    True,
    SelfType,
    Static,
    Parent,
}

impl BuiltinType {
    pub fn as_str(self) -> &'static str {
        match self {
            BuiltinType::Int => "int",
            BuiltinType::Float => "float",
            BuiltinType::String => "string",
            BuiltinType::Bool => "bool",
            BuiltinType::Array => "array",
            BuiltinType::Object => "object",
            BuiltinType::Callable => "callable",
            BuiltinType::Iterable => "iterable",
            BuiltinType::Mixed => "mixed",
            BuiltinType::Void => "void",
            BuiltinType::Never => "never",
            BuiltinType::Null => "null",
            BuiltinType::False => "false",
            BuiltinType::True => "true",
            BuiltinType::SelfType => "self",
            BuiltinType::Static => "static",
            BuiltinType::Parent => "parent",
        }
    }
}

/// A native PHP type declaration.
///
/// The printer normalizes invalid-but-meaningful combinations: a `Nullable` inside a union, or a
/// nullable union/intersection, is printed as a union with `null` (`A|B|null`, `(A&B)|null`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Builtin(BuiltinType),
    /// A class/interface/enum name.
    Class(QName),
    /// `?T`
    Nullable(Box<Type>),
    /// `A|B`; intersection members are printed in DNF form `(A&B)|C`.
    Union(Vec<Type>),
    /// `A&B`
    Intersection(Vec<Type>),
}

// ---------------------------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Protected => "protected",
            Visibility::Private => "private",
        }
    }
}

/// Constructor property promotion modifiers on a [`Param`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Promotion {
    /// Defaults to `public` when `None` and nothing else is set.
    pub visibility: Option<Visibility>,
    /// Asymmetric visibility `private(set)` (PHP 8.4).
    pub set_visibility: Option<Visibility>,
    pub readonly: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Param {
    pub attrs: Vec<AttributeGroup>,
    pub promote: Option<Promotion>,
    pub ty: Option<Type>,
    pub by_ref: bool,
    pub variadic: bool,
    pub name: Ident,
    pub default: Option<Expr>,
    pub origin: Option<Origin>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct FunctionDecl {
    pub doc: Option<DocComment>,
    pub attrs: Vec<AttributeGroup>,
    pub by_ref: bool,
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Vec<Stmt>,
    pub origin: Option<Origin>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ClassModifiers {
    pub is_final: bool,
    pub is_abstract: bool,
    pub is_readonly: bool,
}

/// A class declaration. Also used for anonymous classes ([`ExprKind::NewAnonClass`]), where
/// `name`, `doc` and `is_final`/`is_abstract` are ignored.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ClassDecl {
    pub doc: Option<DocComment>,
    pub attrs: Vec<AttributeGroup>,
    pub modifiers: ClassModifiers,
    pub name: Ident,
    pub extends: Option<QName>,
    pub implements: Vec<QName>,
    pub members: Vec<Member>,
    pub origin: Option<Origin>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct InterfaceDecl {
    pub doc: Option<DocComment>,
    pub attrs: Vec<AttributeGroup>,
    pub name: Ident,
    pub extends: Vec<QName>,
    pub members: Vec<Member>,
    pub origin: Option<Origin>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct TraitDecl {
    pub doc: Option<DocComment>,
    pub attrs: Vec<AttributeGroup>,
    pub name: Ident,
    pub members: Vec<Member>,
    pub origin: Option<Origin>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct EnumDecl {
    pub doc: Option<DocComment>,
    pub attrs: Vec<AttributeGroup>,
    pub name: Ident,
    /// Backing type (`int` or `string`) for backed enums.
    pub backing: Option<Type>,
    pub implements: Vec<QName>,
    pub members: Vec<Member>,
    pub origin: Option<Origin>,
}

/// Top-level `const A = 1, B = 2;`
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ConstDecl {
    pub doc: Option<DocComment>,
    pub items: Vec<(Ident, Expr)>,
    pub origin: Option<Origin>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Member {
    TraitUse(TraitUse),
    Case(EnumCase),
    Const(ClassConstDecl),
    Property(PropertyDecl),
    Method(MethodDecl),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct TraitUse {
    pub traits: Vec<QName>,
    pub adaptations: Vec<TraitAdaptation>,
    pub origin: Option<Origin>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TraitAdaptation {
    /// `[T::]method as [visibility] [alias];`
    Alias {
        trait_name: Option<QName>,
        method: Ident,
        visibility: Option<Visibility>,
        alias: Option<Ident>,
    },
    /// `T::method insteadof A, B;`
    Insteadof {
        trait_name: QName,
        method: Ident,
        instead_of: Vec<QName>,
    },
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct EnumCase {
    pub doc: Option<DocComment>,
    pub attrs: Vec<AttributeGroup>,
    pub name: Ident,
    pub value: Option<Expr>,
    pub origin: Option<Origin>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassConstDecl {
    pub doc: Option<DocComment>,
    pub attrs: Vec<AttributeGroup>,
    pub is_final: bool,
    pub visibility: Option<Visibility>,
    /// Typed class constants (PHP 8.3).
    pub ty: Option<Type>,
    pub name: Ident,
    pub value: Expr,
    pub origin: Option<Origin>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropertyDecl {
    pub doc: Option<DocComment>,
    pub attrs: Vec<AttributeGroup>,
    pub visibility: Visibility,
    /// Asymmetric visibility `private(set)` (PHP 8.4).
    pub set_visibility: Option<Visibility>,
    pub is_static: bool,
    pub readonly: bool,
    pub ty: Option<Type>,
    pub name: Ident,
    pub default: Option<Expr>,
    pub origin: Option<Origin>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodDecl {
    pub doc: Option<DocComment>,
    pub attrs: Vec<AttributeGroup>,
    pub is_final: bool,
    pub is_abstract: bool,
    pub visibility: Option<Visibility>,
    pub is_static: bool,
    pub by_ref: bool,
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    /// `None` prints `;` (abstract / interface method).
    pub body: Option<Vec<Stmt>>,
    pub origin: Option<Origin>,
}

// ---------------------------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub origin: Option<Origin>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ElseIf {
    pub cond: Expr,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwitchCase {
    /// `None` is `default:`.
    pub test: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Catch {
    pub types: Vec<QName>,
    /// Optional since PHP 8.0.
    pub var: Option<Ident>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StaticVar {
    pub name: Ident,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    Expr(Expr),
    Echo(Vec<Expr>),
    Return(Option<Expr>),
    If {
        cond: Expr,
        then: Vec<Stmt>,
        elseifs: Vec<ElseIf>,
        else_: Option<Vec<Stmt>>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    DoWhile {
        body: Vec<Stmt>,
        cond: Expr,
    },
    For {
        init: Vec<Expr>,
        cond: Vec<Expr>,
        step: Vec<Expr>,
        body: Vec<Stmt>,
    },
    Foreach {
        expr: Expr,
        key: Option<Expr>,
        /// A variable, or an [`ExprKind::List`] for destructuring.
        value: Expr,
        by_ref: bool,
        body: Vec<Stmt>,
    },
    Switch {
        subject: Expr,
        cases: Vec<SwitchCase>,
    },
    /// `break;` / `break N;`
    Break(Option<u32>),
    Continue(Option<u32>),
    Try {
        body: Vec<Stmt>,
        catches: Vec<Catch>,
        finally: Option<Vec<Stmt>>,
    },
    Throw(Expr),
    Global(Vec<Ident>),
    Static(Vec<StaticVar>),
    Unset(Vec<Expr>),
    Block(Vec<Stmt>),
    /// `?>html<?php`
    InlineHtml(String),
    Label(Ident),
    Goto(Ident),
    /// Prints an empty line (useful to separate groups of statements).
    Nop,
    /// A free-standing doc comment, e.g. `/** @var Foo $x */` before an assignment.
    DocComment(DocComment),
    /// A `// line comment` (each line of the text becomes one comment line).
    Comment(String),
    Function(FunctionDecl),
    Class(ClassDecl),
    Interface(InterfaceDecl),
    Trait(TraitDecl),
    Enum(EnumDecl),
    Const(ConstDecl),
    Use(UseDecl),
    /// `namespace Foo;` (body `None`) or `namespace Foo { ... }`.
    Namespace {
        name: Option<SmolStr>,
        body: Option<Vec<Stmt>>,
    },
    /// `declare(ticks=1);`
    Declare(Vec<(Ident, Expr)>),
}

// ---------------------------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub origin: Option<Origin>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// `!`
    Not,
    /// `-`
    Neg,
    /// `+`
    Plus,
    /// `~`
    BitNot,
    PreInc,
    PreDec,
    PostInc,
    PostDec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    /// `&&`
    And,
    /// `||`
    Or,
    /// `and`
    LogicalAnd,
    /// `or`
    LogicalOr,
    /// `xor`
    LogicalXor,
    Eq,
    NotEq,
    Identical,
    NotIdentical,
    Lt,
    Le,
    Gt,
    Ge,
    Spaceship,
    Coalesce,
}

/// Compound assignment operators (`+=`, `.=`, `??=`, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssignOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Coalesce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CastKind {
    Int,
    Float,
    String,
    Bool,
    Array,
    Object,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IncludeKind {
    Include,
    IncludeOnce,
    Require,
    RequireOnce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MagicConst {
    Line,
    File,
    Dir,
    Function,
    Class,
    Trait,
    Method,
    Namespace,
    Property,
}

/// The class part of `X::y`, `new X`, `instanceof X`.
#[derive(Clone, Debug, PartialEq)]
pub enum ClassRef {
    Name(QName),
    SelfRef,
    Static,
    Parent,
    /// A dynamic class (`$cls::foo()`, `new $cls()`); parenthesized when needed.
    Expr(Box<Expr>),
}

/// A member name after `->` / `::`.
#[derive(Clone, Debug, PartialEq)]
pub enum MemberName {
    Ident(Ident),
    /// `->{expr}`, `::{expr}`, `::${expr}`
    Expr(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Positional(Expr),
    Named(Ident, Expr),
    /// `...$args`
    Spread(Expr),
    /// The `...` of a first-class callable `f(...)`; must be the only argument.
    Placeholder,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArrayItem {
    pub key: Option<Expr>,
    pub value: Expr,
    pub by_ref: bool,
    pub spread: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InterpPart {
    Lit(String),
    /// Printed as `{$expr}` when the expression starts with a variable; otherwise the whole
    /// string is printed as a concatenation.
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClosureUse {
    pub name: Ident,
    pub by_ref: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Closure {
    pub attrs: Vec<AttributeGroup>,
    pub is_static: bool,
    pub by_ref: bool,
    pub params: Vec<Param>,
    pub uses: Vec<ClosureUse>,
    pub return_type: Option<Type>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArrowFn {
    pub attrs: Vec<AttributeGroup>,
    pub is_static: bool,
    pub by_ref: bool,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Box<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatchArm {
    /// `None` is the `default` arm.
    pub conds: Option<Vec<Expr>>,
    pub body: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    // --- literals -------------------------------------------------------------------------
    Int(i64),
    Float(f64),
    /// A plain string literal (single-quoted unless escapes require double quotes).
    Str(SmolStr),
    /// `"... {$x} ..."`
    Interp(Vec<InterpPart>),
    Bool(bool),
    Null,
    Array(Vec<ArrayItem>),
    /// Destructuring pattern `[$a, , 'k' => $b]` (`None` is a skipped element).
    List(Vec<Option<ArrayItem>>),
    /// A constant or function name (`PHP_EOL`, `\strlen`).
    Name(QName),
    MagicConst(MagicConst),

    // --- variables and access -------------------------------------------------------------
    /// `$name` (use `"this"` for `$this`).
    Var(Ident),
    /// `${expr}` / `$$x`
    VarVar(Box<Expr>),
    PropFetch {
        object: Box<Expr>,
        prop: MemberName,
        nullsafe: bool,
    },
    StaticProp {
        class: ClassRef,
        prop: MemberName,
    },
    /// `X::NAME` (use name `class` for `X::class`).
    ClassConst {
        class: ClassRef,
        name: MemberName,
    },
    /// `$a[i]`, or `$a[]` when `index` is `None`.
    Index {
        base: Box<Expr>,
        index: Option<Box<Expr>>,
    },

    // --- calls ----------------------------------------------------------------------------
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
    },
    MethodCall {
        object: Box<Expr>,
        method: MemberName,
        args: Vec<Arg>,
        nullsafe: bool,
    },
    StaticCall {
        class: ClassRef,
        method: MemberName,
        args: Vec<Arg>,
    },
    New {
        class: ClassRef,
        args: Vec<Arg>,
    },
    NewAnonClass {
        class: Box<ClassDecl>,
        args: Vec<Arg>,
    },

    // --- operators ------------------------------------------------------------------------
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    AssignRef {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    AssignOp {
        op: AssignOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// `c ? a : b`, or short ternary `c ?: b` when `then` is `None`.
    Ternary {
        cond: Box<Expr>,
        then: Option<Box<Expr>>,
        else_: Box<Expr>,
    },
    Instanceof {
        expr: Box<Expr>,
        class: ClassRef,
    },
    Cast {
        kind: CastKind,
        expr: Box<Expr>,
    },
    /// `@expr`
    Silence(Box<Expr>),
    Isset(Vec<Expr>),
    Empty(Box<Expr>),
    Clone(Box<Expr>),

    // --- functions ------------------------------------------------------------------------
    Closure(Box<Closure>),
    ArrowFn(ArrowFn),
    Match {
        subject: Box<Expr>,
        arms: Vec<MatchArm>,
    },

    // --- prefix keywords ------------------------------------------------------------------
    Throw(Box<Expr>),
    Yield {
        key: Option<Box<Expr>>,
        value: Option<Box<Expr>>,
    },
    YieldFrom(Box<Expr>),
    Print(Box<Expr>),
    Exit(Option<Box<Expr>>),
    Include {
        kind: IncludeKind,
        expr: Box<Expr>,
    },

    // --- escape hatches -------------------------------------------------------------------
    /// Explicit parentheses.
    Paren(Box<Expr>),
    /// Verbatim PHP code. Treated as a primary (atomic, dereferencable) expression: wrap it in
    /// [`ExprKind::Paren`] if it contains operators.
    Raw(String),
}
