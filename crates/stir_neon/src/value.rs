//! NEON value model.

use std::fmt::{self, Display, Write};

use indexmap::IndexMap;

/// A mapping key. Like PHP array keys, decimal-integer strings become `Int`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Int(i64),
    Str(String),
}

impl Key {
    /// Converts a string the way PHP converts array keys (`"12"` → `12`, `"012"` stays a string).
    pub fn from_php_string(s: &str) -> Key {
        let b = s.as_bytes();
        let digits = b.strip_prefix(b"-").unwrap_or(b);
        let canonical = !digits.is_empty()
            && digits.iter().all(u8::is_ascii_digit)
            && (digits[0] != b'0' || digits.len() == 1)
            && s != "-0";
        if canonical && let Ok(i) = s.parse::<i64>() {
            return Key::Int(i);
        }
        Key::Str(s.to_string())
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Key::Str(s) => Some(s),
            Key::Int(_) => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Key::Int(i) => Some(*i),
            Key::Str(_) => None,
        }
    }
}

impl From<&str> for Key {
    fn from(s: &str) -> Self {
        Key::from_php_string(s)
    }
}

impl From<String> for Key {
    fn from(s: String) -> Self {
        Key::from_php_string(&s)
    }
}

impl From<i64> for Key {
    fn from(i: i64) -> Self {
        Key::Int(i)
    }
}

impl Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Int(i) => write!(f, "{i}"),
            Key::Str(s) => write_quoted(f, s),
        }
    }
}

/// `Foo(a, b: c)`
#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    pub value: Box<Value>,
    /// Attributes; positional ones get sequential integer keys.
    pub attributes: IndexMap<Key, Value>,
}

/// A parsed NEON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Quoted strings and bare literals (dates are kept as strings too).
    String(String),
    /// An array whose keys are exactly `0..n` (or an empty `[]`).
    Sequence(Vec<Value>),
    /// Any other array (or an empty `{}`).
    Mapping(IndexMap<Key, Value>),
    Entity(Entity),
    /// Chained entities, e.g. `Foo(1)::bar(2)` (the second entity's value is `::bar`).
    Chain(Vec<Entity>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_sequence(&self) -> Option<&[Value]> {
        match self {
            Value::Sequence(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_mapping(&self) -> Option<&IndexMap<Key, Value>> {
        match self {
            Value::Mapping(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_entity(&self) -> Option<&Entity> {
        match self {
            Value::Entity(e) => Some(e),
            _ => None,
        }
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Value::Sequence(_) | Value::Mapping(_))
    }

    /// Array entries (sequence items keyed `0..n`); `None` for non-arrays.
    pub fn entries(&self) -> Option<Vec<(Key, &Value)>> {
        match self {
            Value::Sequence(s) => Some(
                s.iter()
                    .enumerate()
                    .map(|(i, v)| (Key::Int(i as i64), v))
                    .collect(),
            ),
            Value::Mapping(m) => Some(m.iter().map(|(k, v)| (k.clone(), v)).collect()),
            _ => None,
        }
    }

    /// Array values in order (sequence items or mapping values).
    pub fn values(&self) -> Option<Vec<&Value>> {
        match self {
            Value::Sequence(s) => Some(s.iter().collect()),
            Value::Mapping(m) => Some(m.values().collect()),
            _ => None,
        }
    }

    /// Looks up a key in a mapping (or an index in a sequence).
    pub fn get(&self, key: impl Into<Key>) -> Option<&Value> {
        match (self, key.into()) {
            (Value::Mapping(m), k) => m.get(&k),
            (Value::Sequence(s), Key::Int(i)) => usize::try_from(i).ok().and_then(|i| s.get(i)),
            _ => None,
        }
    }

    /// Builds an array value from entries, choosing `Sequence` when keys are `0..n`.
    pub fn from_entries(entries: IndexMap<Key, Value>, empty_is_mapping: bool) -> Value {
        if entries.is_empty() {
            return if empty_is_mapping {
                Value::Mapping(entries)
            } else {
                Value::Sequence(Vec::new())
            };
        }
        let is_list = entries
            .keys()
            .enumerate()
            .all(|(i, k)| *k == Key::Int(i as i64));
        if is_list {
            Value::Sequence(entries.into_values().collect())
        } else {
            Value::Mapping(entries)
        }
    }
}

fn write_quoted(f: &mut impl Write, s: &str) -> fmt::Result {
    f.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\t' => f.write_str("\\t")?,
            '\r' => f.write_str("\\r")?,
            c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
            c => f.write_char(c)?,
        }
    }
    f.write_char('"')
}

fn write_attrs(f: &mut fmt::Formatter<'_>, m: &IndexMap<Key, Value>) -> fmt::Result {
    let positional = m.keys().enumerate().all(|(i, k)| *k == Key::Int(i as i64));
    for (i, (k, v)) in m.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        if !positional {
            write!(f, "{k}: ")?;
        }
        write!(f, "{v}")?;
    }
    Ok(())
}

/// Inline NEON encoding (strings always double-quoted).
impl Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => {
                if !x.is_finite() || x.abs() >= 1e16 {
                    write!(f, "{x:e}")
                } else if x.fract() == 0.0 {
                    write!(f, "{x:.1}")
                } else {
                    write!(f, "{x}")
                }
            }
            Value::String(s) => write_quoted(f, s),
            Value::Sequence(items) => {
                f.write_char('[')?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_char(']')
            }
            Value::Mapping(m) => {
                f.write_char('{')?;
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                f.write_char('}')
            }
            Value::Entity(e) => {
                write!(f, "{}(", e.value)?;
                write_attrs(f, &e.attributes)?;
                f.write_char(')')
            }
            Value::Chain(es) => {
                for (i, e) in es.iter().enumerate() {
                    if i > 0 {
                        f.write_char(' ')?;
                    }
                    write!(f, "{}(", e.value)?;
                    write_attrs(f, &e.attributes)?;
                    f.write_char(')')?;
                }
                Ok(())
            }
        }
    }
}
