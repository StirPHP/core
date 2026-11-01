//! PHP name resolution: namespaces and `use` imports.

use rustc_hash::FxHashMap;
use smol_str::SmolStr;

/// A fully-qualified name without the leading `\`, in its declared case.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fqn(pub SmolStr);

impl Fqn {
    pub fn new(s: &str) -> Fqn {
        Fqn(SmolStr::new(s.trim_start_matches('\\')))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// PHP class and function names are case-insensitive.
    pub fn key(&self) -> SmolStr {
        SmolStr::new(self.0.to_ascii_lowercase())
    }

    /// The last segment (`Bar` in `Foo\Bar`).
    pub fn short(&self) -> &str {
        self.0.rsplit('\\').next().unwrap_or(&self.0)
    }

    pub fn namespace(&self) -> &str {
        match self.0.rfind('\\') {
            Some(i) => &self.0[..i],
            None => "",
        }
    }

    pub fn join(namespace: &str, name: &str) -> Fqn {
        if namespace.is_empty() {
            Fqn::new(name)
        } else {
            Fqn(SmolStr::new(format!("{namespace}\\{name}")))
        }
    }
}

impl std::fmt::Display for Fqn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UseKind {
    Class,
    Function,
    Const,
}

/// Names in scope at some point of a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    pub namespace: SmolStr,
    /// lowercase alias → fully qualified name
    pub classes: FxHashMap<SmolStr, Fqn>,
    pub functions: FxHashMap<SmolStr, Fqn>,
    /// constants are case-sensitive
    pub consts: FxHashMap<SmolStr, Fqn>,
}

/// Result of resolving a function or constant name: PHP falls back to the
/// global namespace for unqualified names that aren't imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub primary: Fqn,
    pub fallback: Option<Fqn>,
}

impl Scope {
    pub fn add_use(&mut self, kind: UseKind, target: &str, alias: Option<&str>) {
        let fqn = Fqn::new(target);
        let alias = alias
            .map(SmolStr::new)
            .unwrap_or_else(|| SmolStr::new(fqn.short()));
        match kind {
            UseKind::Class => {
                self.classes
                    .insert(SmolStr::new(alias.to_ascii_lowercase()), fqn);
            }
            UseKind::Function => {
                self.functions
                    .insert(SmolStr::new(alias.to_ascii_lowercase()), fqn);
            }
            UseKind::Const => {
                self.consts.insert(alias, fqn);
            }
        }
    }

    /// Resolves a class-like name (`self`/`parent`/`static` are returned unchanged).
    pub fn resolve_class(&self, name: &str) -> Fqn {
        if let Some(rest) = name.strip_prefix('\\') {
            return Fqn::new(rest);
        }
        if name.len() > 10 && name[..10].eq_ignore_ascii_case("namespace\\") {
            return Fqn::join(&self.namespace, &name[10..]);
        }
        let lower = name.to_ascii_lowercase();
        if matches!(lower.as_str(), "self" | "parent" | "static") {
            return Fqn::new(name);
        }
        match name.split_once('\\') {
            Some((first, rest)) => match self.classes.get(first.to_ascii_lowercase().as_str()) {
                Some(target) => Fqn(SmolStr::new(format!("{target}\\{rest}"))),
                None => Fqn::join(&self.namespace, name),
            },
            None => match self.classes.get(lower.as_str()) {
                Some(target) => target.clone(),
                None => Fqn::join(&self.namespace, name),
            },
        }
    }

    pub fn resolve_function(&self, name: &str) -> Resolved {
        self.resolve_fn_or_const(name, true)
    }

    pub fn resolve_const(&self, name: &str) -> Resolved {
        self.resolve_fn_or_const(name, false)
    }

    fn resolve_fn_or_const(&self, name: &str, is_fn: bool) -> Resolved {
        if let Some(rest) = name.strip_prefix('\\') {
            return Resolved {
                primary: Fqn::new(rest),
                fallback: None,
            };
        }
        if name.len() > 10 && name[..10].eq_ignore_ascii_case("namespace\\") {
            return Resolved {
                primary: Fqn::join(&self.namespace, &name[10..]),
                fallback: None,
            };
        }
        if let Some((first, rest)) = name.split_once('\\') {
            let primary = match self.classes.get(first.to_ascii_lowercase().as_str()) {
                Some(target) => Fqn(SmolStr::new(format!("{target}\\{rest}"))),
                None => Fqn::join(&self.namespace, name),
            };
            return Resolved {
                primary,
                fallback: None,
            };
        }
        let imported = if is_fn {
            self.functions.get(name.to_ascii_lowercase().as_str())
        } else {
            self.consts.get(name)
        };
        if let Some(target) = imported {
            return Resolved {
                primary: target.clone(),
                fallback: None,
            };
        }
        if self.namespace.is_empty() {
            Resolved {
                primary: Fqn::new(name),
                fallback: None,
            }
        } else {
            Resolved {
                primary: Fqn::join(&self.namespace, name),
                fallback: Some(Fqn::new(name)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution() {
        let mut s = Scope {
            namespace: "App".into(),
            ..Default::default()
        };
        s.add_use(UseKind::Class, "Foo\\Bar", None);
        s.add_use(UseKind::Class, "Foo\\Baz", Some("Q"));
        s.add_use(UseKind::Function, "Lib\\helper", None);
        assert_eq!(s.resolve_class("Bar").as_str(), "Foo\\Bar");
        assert_eq!(s.resolve_class("bar").as_str(), "Foo\\Bar");
        assert_eq!(s.resolve_class("Q\\X").as_str(), "Foo\\Baz\\X");
        assert_eq!(s.resolve_class("Other").as_str(), "App\\Other");
        assert_eq!(s.resolve_class("\\Other").as_str(), "Other");
        assert_eq!(s.resolve_class("namespace\\X").as_str(), "App\\X");
        assert_eq!(s.resolve_function("helper").primary.as_str(), "Lib\\helper");
        let r = s.resolve_function("strlen");
        assert_eq!(r.primary.as_str(), "App\\strlen");
        assert_eq!(r.fallback.unwrap().as_str(), "strlen");
    }
}
