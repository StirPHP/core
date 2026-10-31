//! Doc block and tag AST, plus PHPStan tag-precedence helpers.

use smol_str::SmolStr;
use text_size::TextRange;

use crate::ast::{Name, TemplateParam, TypeNode, Variable};

/// A text fragment (description, summary) with its range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub text: String,
    pub range: TextRange,
}

/// A parsed `/** ... */` comment.
#[derive(Debug, Clone, PartialEq)]
pub struct DocBlock {
    pub range: TextRange,
    /// First paragraph of the free text before the tags.
    pub summary: Option<Text>,
    /// Remaining free text before the tags.
    pub description: Option<Text>,
    pub tags: Vec<Tag>,
}

/// Tool prefix of a tag (`@phpstan-param`, `@psalm-param`, `@param`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TagPrefix {
    None,
    Psalm,
    Phpstan,
}

impl TagPrefix {
    /// PHPStan precedence: phpstan > psalm > plain.
    pub fn precedence(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    /// Tag name as written, without `@` (e.g. `phpstan-param`).
    pub name: SmolStr,
    pub name_range: TextRange,
    pub prefix: TagPrefix,
    /// Whole tag incl. description.
    pub range: TextRange,
    pub kind: TagKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TagKind {
    Param(ParamTag),
    Return(TypeTag),
    Var(VarTag),
    Throws(TypeTag),
    Template(TemplateTag),
    Extends(TypeTag),
    Implements(TypeTag),
    Use(TypeTag),
    Property(PropertyTag),
    Method(MethodTag),
    TypeAlias(TypeAliasTag),
    TypeAliasImport(TypeAliasImportTag),
    Deprecated(DescriptionTag),
    Internal(DescriptionTag),
    Assert(AssertTag),
    ParamOut(ParamOutTag),
    SelfOut(TypeTag),
    Pure,
    Impure,
    /// `@phpstan-ignore id1, id2 (comment)`
    Ignore(Vec<IgnoreIdentifier>),
    /// `@phpstan-ignore-next-line`
    IgnoreNextLine,
    /// `@phpstan-ignore-line`
    IgnoreLine,
    /// Any other tag.
    Generic(DescriptionTag),
    /// A known tag whose value could not be parsed (e.g. a missing name).
    Invalid {
        message: String,
        text: Option<Text>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptionTag {
    pub description: Option<Text>,
}

/// `@return`, `@throws`, `@extends`, `@implements`, `@use`, `@phpstan-self-out`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeTag {
    pub ty: TypeNode,
    pub description: Option<Text>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamTag {
    /// `None` for typeless `@param $x desc`.
    pub ty: Option<TypeNode>,
    pub by_ref: bool,
    pub variadic: bool,
    pub name: Option<Variable>,
    pub description: Option<Text>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarTag {
    pub ty: TypeNode,
    pub name: Option<Variable>,
    pub description: Option<Text>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TemplateVariance {
    Invariant,
    Covariant,
    Contravariant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateTag {
    pub variance: TemplateVariance,
    pub param: TemplateParam,
    pub description: Option<Text>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropertyAccess {
    ReadWrite,
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PropertyTag {
    pub access: PropertyAccess,
    pub ty: TypeNode,
    pub name: Variable,
    pub description: Option<Text>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodTag {
    pub is_static: bool,
    pub return_type: Option<TypeNode>,
    pub name: Name,
    pub templates: Vec<TemplateParam>,
    pub params: Vec<MethodParam>,
    pub description: Option<Text>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodParam {
    pub ty: Option<TypeNode>,
    pub by_ref: bool,
    pub variadic: bool,
    pub name: Variable,
    /// Raw default value text after `=`.
    pub default: Option<Text>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasTag {
    pub name: Name,
    pub ty: TypeNode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasImportTag {
    pub name: Name,
    pub from: TypeNode,
    pub alias: Option<Name>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssertKind {
    Assert,
    IfTrue,
    IfFalse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssertTag {
    pub kind: AssertKind,
    /// `!Type`
    pub negated: bool,
    /// `=Type`
    pub equality: bool,
    pub ty: TypeNode,
    pub target: Option<AssertTarget>,
    pub description: Option<Text>,
}

/// `$x`, `$this->prop`, `$x->method()`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertTarget {
    /// Variable name without `$` (`this` for `$this`).
    pub param: SmolStr,
    pub member: Option<AssertMember>,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertMember {
    pub name: SmolStr,
    pub is_method: bool,
}

impl AssertTarget {
    pub fn to_source(&self) -> String {
        let mut s = format!("${}", self.param);
        if let Some(m) = &self.member {
            s.push_str("->");
            s.push_str(&m.name);
            if m.is_method {
                s.push_str("()");
            }
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamOutTag {
    pub ty: TypeNode,
    pub name: Option<Variable>,
    pub description: Option<Text>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreIdentifier {
    pub name: SmolStr,
    pub comment: Option<String>,
    pub range: TextRange,
}

/// Groups tags that override each other by PHPStan precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagGroup {
    Param,
    Return,
    Var,
    Throws,
    Template,
    Extends,
    Implements,
    Use,
    Property,
    Method,
    TypeAlias,
    Assert,
    ParamOut,
    SelfOut,
    Purity,
}

impl Tag {
    /// The tag name with any `phpstan-`/`psalm-` prefix removed.
    pub fn base_name(&self) -> &str {
        split_prefix(&self.name).1
    }

    /// The precedence group and key (e.g. param name) this tag competes in, if any.
    pub fn group_key(&self) -> Option<(TagGroup, String)> {
        let var = |v: &Option<Variable>| v.as_ref().map(|v| v.name.to_string()).unwrap_or_default();
        let class = |t: &TypeNode| match &t.kind {
            crate::TypeKind::Generic { base, .. } => base.name.to_ascii_lowercase(),
            _ => t.to_string().to_ascii_lowercase(),
        };
        Some(match &self.kind {
            TagKind::Param(p) => (TagGroup::Param, var(&p.name)),
            TagKind::Return(_) => (TagGroup::Return, String::new()),
            TagKind::Var(v) => (TagGroup::Var, var(&v.name)),
            TagKind::Throws(_) => (TagGroup::Throws, String::new()),
            TagKind::Template(t) => (TagGroup::Template, t.param.name.name.to_string()),
            TagKind::Extends(t) => (TagGroup::Extends, class(&t.ty)),
            TagKind::Implements(t) => (TagGroup::Implements, class(&t.ty)),
            TagKind::Use(t) => (TagGroup::Use, class(&t.ty)),
            TagKind::Property(p) => (TagGroup::Property, p.name.name.to_string()),
            TagKind::Method(m) => (TagGroup::Method, m.name.name.to_ascii_lowercase()),
            TagKind::TypeAlias(a) => (TagGroup::TypeAlias, a.name.name.to_string()),
            TagKind::TypeAliasImport(a) => (
                TagGroup::TypeAlias,
                a.alias.as_ref().unwrap_or(&a.name).name.to_string(),
            ),
            TagKind::Assert(a) => (
                TagGroup::Assert,
                format!(
                    "{:?}:{}",
                    a.kind,
                    a.target.as_ref().map(|t| t.to_source()).unwrap_or_default()
                ),
            ),
            TagKind::ParamOut(p) => (TagGroup::ParamOut, var(&p.name)),
            TagKind::SelfOut(_) => (TagGroup::SelfOut, String::new()),
            TagKind::Pure | TagKind::Impure => (TagGroup::Purity, String::new()),
            _ => return None,
        })
    }
}

pub(crate) fn split_prefix(name: &str) -> (TagPrefix, &str) {
    if let Some(rest) = name.strip_prefix("phpstan-") {
        (TagPrefix::Phpstan, rest)
    } else if let Some(rest) = name.strip_prefix("psalm-") {
        (TagPrefix::Psalm, rest)
    } else {
        (TagPrefix::None, name)
    }
}

impl DocBlock {
    /// Tags after applying PHPStan precedence: for each group/key (e.g. `@param $x`),
    /// only the tags with the highest prefix (phpstan > psalm > plain) are kept.
    /// Ungrouped tags are always kept. Source order is preserved.
    pub fn effective_tags(&self) -> Vec<&Tag> {
        let mut best: std::collections::HashMap<(TagGroup, String), TagPrefix> =
            std::collections::HashMap::new();
        for t in &self.tags {
            if let Some(k) = t.group_key() {
                let e = best.entry(k).or_insert(t.prefix);
                if t.prefix > *e {
                    *e = t.prefix;
                }
            }
        }
        self.tags
            .iter()
            .filter(|t| match t.group_key() {
                Some(k) => best.get(&k) == Some(&t.prefix),
                None => true,
            })
            .collect()
    }

    /// Effective `@param` tags.
    pub fn params(&self) -> Vec<&ParamTag> {
        self.effective_tags()
            .into_iter()
            .filter_map(|t| match &t.kind {
                TagKind::Param(p) => Some(p),
                _ => None,
            })
            .collect()
    }

    /// Effective `@param` for the named parameter (name without `$`).
    pub fn param(&self, name: &str) -> Option<&ParamTag> {
        self.params()
            .into_iter()
            .find(|p| p.name.as_ref().is_some_and(|n| n.name == name))
    }

    /// Effective `@return`.
    pub fn return_tag(&self) -> Option<&TypeTag> {
        self.effective_tags()
            .into_iter()
            .find_map(|t| match &t.kind {
                TagKind::Return(r) => Some(r),
                _ => None,
            })
    }

    /// Effective `@var` tags.
    pub fn vars(&self) -> Vec<&VarTag> {
        self.effective_tags()
            .into_iter()
            .filter_map(|t| match &t.kind {
                TagKind::Var(v) => Some(v),
                _ => None,
            })
            .collect()
    }

    /// Effective `@throws` tags.
    pub fn throws(&self) -> Vec<&TypeTag> {
        self.effective_tags()
            .into_iter()
            .filter_map(|t| match &t.kind {
                TagKind::Throws(v) => Some(v),
                _ => None,
            })
            .collect()
    }

    /// Effective `@template*` tags.
    pub fn templates(&self) -> Vec<&TemplateTag> {
        self.effective_tags()
            .into_iter()
            .filter_map(|t| match &t.kind {
                TagKind::Template(v) => Some(v),
                _ => None,
            })
            .collect()
    }

    pub fn is_deprecated(&self) -> bool {
        self.tags
            .iter()
            .any(|t| matches!(t.kind, TagKind::Deprecated(_)))
    }

    pub fn is_internal(&self) -> bool {
        self.tags
            .iter()
            .any(|t| matches!(t.kind, TagKind::Internal(_)))
    }

    /// All type `Error` nodes in all tags.
    pub fn type_errors(&self) -> Vec<&TypeNode> {
        let mut out = Vec::new();
        for t in &self.tags {
            t.for_each_type(&mut |ty| out.extend(ty.errors()));
        }
        out
    }
}

impl Tag {
    /// Calls `f` for each top-level type in this tag.
    pub fn for_each_type<'a>(&'a self, f: &mut dyn FnMut(&'a TypeNode)) {
        let tparam = |p: &'a TemplateParam, f: &mut dyn FnMut(&'a TypeNode)| {
            for t in [&p.bound, &p.lower_bound, &p.default].into_iter().flatten() {
                f(t);
            }
        };
        match &self.kind {
            TagKind::Param(p) => {
                if let Some(t) = &p.ty {
                    f(t)
                }
            }
            TagKind::Return(t)
            | TagKind::Throws(t)
            | TagKind::Extends(t)
            | TagKind::Implements(t)
            | TagKind::Use(t)
            | TagKind::SelfOut(t) => f(&t.ty),
            TagKind::Var(v) => f(&v.ty),
            TagKind::Template(t) => tparam(&t.param, f),
            TagKind::Property(p) => f(&p.ty),
            TagKind::Method(m) => {
                if let Some(t) = &m.return_type {
                    f(t);
                }
                for t in &m.templates {
                    tparam(t, f);
                }
                for p in &m.params {
                    if let Some(t) = &p.ty {
                        f(t);
                    }
                }
            }
            TagKind::TypeAlias(a) => f(&a.ty),
            TagKind::TypeAliasImport(a) => f(&a.from),
            TagKind::Assert(a) => f(&a.ty),
            TagKind::ParamOut(p) => f(&p.ty),
            _ => {}
        }
    }
}
