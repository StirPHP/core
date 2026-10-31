//! NEON parser (structure follows nette/neon v3's `Parser`).

use indexmap::IndexMap;

use crate::lexer::{Token, TokenKind, lex};
use crate::{Entity, Error, Key, Value};

pub(crate) fn parse(src: &str) -> Result<Value, Error> {
    let tokens = lex(src)?;
    let mut p = Parser {
        src,
        toks: tokens,
        pos: 0,
        indent: String::new(),
    };
    while p.eat_newline() {}
    let indent = p.indent.clone();
    let v = p.parse_block(&indent, false)?;
    while p.eat_newline() {}
    if p.pos < p.toks.len() {
        return Err(p.unexpected());
    }
    Ok(v)
}

/// Array under construction, with PHP auto-index semantics.
struct ArrayBuilder {
    items: IndexMap<Key, Value>,
    next_index: i64,
}

impl ArrayBuilder {
    fn new() -> Self {
        Self {
            items: IndexMap::new(),
            next_index: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn push(&mut self, key: Option<Key>, value: Value) -> Result<(), String> {
        let key = match key {
            Some(k) => {
                if self.items.contains_key(&k) {
                    return Err(format!("Duplicated key '{}'", key_text(&k)));
                }
                k
            }
            None => Key::Int(self.next_index),
        };
        if let Key::Int(i) = key
            && i >= self.next_index
        {
            self.next_index = i.saturating_add(1);
        }
        self.items.insert(key, value);
        Ok(())
    }

    fn finish(self, empty_is_mapping: bool) -> Value {
        Value::from_entries(self.items, empty_is_mapping)
    }
}

fn key_text(k: &Key) -> String {
    match k {
        Key::Int(i) => i.to_string(),
        Key::Str(s) => s.clone(),
    }
}

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    /// Indentation of the most recently consumed newline.
    indent: String,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&TokenKind> {
        self.toks.get(self.pos).map(|t| &t.kind)
    }

    fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }

    fn at_char(&self, c: u8) -> bool {
        matches!(self.peek(), Some(TokenKind::Char(x)) if *x == c)
    }

    fn at_newline(&self) -> bool {
        matches!(self.peek(), Some(TokenKind::Newline(_)))
    }

    fn eat_char(&mut self, c: u8) -> bool {
        let r = self.at_char(c);
        if r {
            self.pos += 1;
        }
        r
    }

    fn eat_newline(&mut self) -> bool {
        if let Some(TokenKind::Newline(i)) = self.peek() {
            self.indent = i.clone();
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn offset(&self) -> usize {
        self.toks.get(self.pos).map_or(self.src.len(), |t| t.offset)
    }

    fn error(&self, msg: impl Into<String>) -> Error {
        Error::at(self.src, self.offset(), msg.into())
    }

    fn unexpected(&self) -> Error {
        let what = match self.peek() {
            None => "Unexpected end".to_string(),
            Some(TokenKind::Newline(_)) => "Unexpected end of line".to_string(),
            Some(TokenKind::Char(c)) => format!("Unexpected '{}'", *c as char),
            Some(TokenKind::Literal(s)) => format!("Unexpected '{s}'"),
            Some(TokenKind::String(s)) => format!("Unexpected string '{s}'"),
        };
        self.error(what)
    }

    fn check_indent(&self, next: &str, indent: &str) -> Result<(), Error> {
        let n = next.len().min(indent.len());
        if next.as_bytes()[..n] != indent.as_bytes()[..n] {
            return Err(self.error("Invalid combination of tabs and spaces"));
        }
        Ok(())
    }

    fn parse_block(&mut self, indent: &str, only_bullets: bool) -> Result<Value, Error> {
        let mut res = ArrayBuilder::new();
        loop {
            let item_offset = self.offset();
            let mut key = None;
            if self.eat_char(b'-') {
                // bullet item
            } else if self.at_end() || only_bullets {
                return Ok(if res.is_empty() {
                    Value::Null
                } else {
                    res.finish(false)
                });
            } else {
                let value = self.parse_value()?;
                if self.eat_char(b':') || self.eat_char(b'=') {
                    key = Some(self.to_key(value, item_offset)?);
                } else if !res.is_empty() {
                    return Err(self.unexpected());
                } else {
                    return Ok(value);
                }
            }

            let mut value = Value::Null;
            let mut close = false;
            if self.eat_newline() {
                while self.eat_newline() {}
                let next = self.indent.clone();
                self.check_indent(&next, indent)?;
                if next.len() > indent.len() {
                    value = self.parse_block(&next, false)?;
                } else if next.len() < indent.len() {
                    close = true;
                } else if key.is_some() && self.at_char(b'-') {
                    value = self.parse_block(indent, true)?;
                }
            } else if key.is_none() {
                value = self.parse_block(&format!("{indent}  "), false)?;
            } else if !self.at_end() {
                value = self.parse_value()?;
                if !self.at_end() && !self.at_newline() {
                    return Err(self.unexpected());
                }
            }

            res.push(key, value)
                .map_err(|m| Error::at(self.src, item_offset, m))?;
            if close {
                return Ok(res.finish(true));
            }

            while self.eat_newline() {}
            if self.at_end() {
                return Ok(res.finish(true));
            }
            let next = self.indent.clone();
            self.check_indent(&next, indent)?;
            if next.len() > indent.len() {
                return Err(self.error("Bad indentation"));
            } else if next.len() < indent.len() {
                return Ok(res.finish(true));
            }
        }
    }

    fn to_key(&self, v: Value, offset: usize) -> Result<Key, Error> {
        Ok(match v {
            Value::String(s) => Key::from_php_string(&s),
            Value::Int(i) => Key::Int(i),
            Value::Float(f) => Key::Str(f.to_string()),
            _ => return Err(Error::at(self.src, offset, "Unacceptable key".into())),
        })
    }

    fn parse_value(&mut self) -> Result<Value, Error> {
        let node = match self.peek() {
            Some(TokenKind::String(s)) => {
                let v = Value::String(s.clone());
                self.pos += 1;
                v
            }
            Some(TokenKind::Literal(s)) => {
                let s = s.clone();
                self.pos += 1;
                let is_key = self.at_char(b':') || self.at_char(b'=');
                parse_literal(&s, is_key)
            }
            Some(TokenKind::Char(b'[' | b'{' | b'(')) => self.parse_braces()?.0,
            _ => return Err(self.unexpected()),
        };
        self.parse_entity(node)
    }

    fn parse_entity(&mut self, node: Value) -> Result<Value, Error> {
        if !self.at_char(b'(') {
            return Ok(node);
        }
        let attributes = self.parse_braces()?.1;
        let mut entities = vec![Entity {
            value: Box::new(node),
            attributes,
        }];
        while let Some(TokenKind::Literal(s)) = self.peek() {
            let v = parse_literal(&s.clone(), false);
            self.pos += 1;
            if self.at_char(b'(') {
                let attributes = self.parse_braces()?.1;
                entities.push(Entity {
                    value: Box::new(v),
                    attributes,
                });
            } else {
                entities.push(Entity {
                    value: Box::new(v),
                    attributes: IndexMap::new(),
                });
                break;
            }
        }
        Ok(if entities.len() == 1 {
            Value::Entity(entities.pop().expect("one entity"))
        } else {
            Value::Chain(entities)
        })
    }

    /// Parses `[...]`, `{...}` or `(...)`; returns the array value and its raw entries.
    fn parse_braces(&mut self) -> Result<(Value, IndexMap<Key, Value>), Error> {
        let open = match self.peek() {
            Some(TokenKind::Char(c)) => *c,
            _ => return Err(self.unexpected()),
        };
        self.pos += 1;
        let close = match open {
            b'[' => b']',
            b'{' => b'}',
            _ => b')',
        };
        let mut res = ArrayBuilder::new();
        loop {
            while self.eat_newline() {}
            if self.eat_char(close) {
                let entries = res.items.clone();
                return Ok((res.finish(open == b'{'), entries));
            }
            let item_offset = self.offset();
            let value = self.parse_value()?;
            if self.eat_char(b':') || self.eat_char(b'=') {
                let key = self.to_key(value, item_offset)?;
                let v = if self.at_end()
                    || self.at_newline()
                    || self.at_char(b',')
                    || self.at_char(close)
                {
                    Value::Null
                } else {
                    self.parse_value()?
                };
                res.push(Some(key), v)
            } else {
                res.push(None, value)
            }
            .map_err(|m| Error::at(self.src, item_offset, m))?;

            if self.eat_char(b',') || self.eat_newline() {
                continue;
            }
            while self.eat_newline() {}
            if !self.at_char(close) {
                return Err(self.unexpected());
            }
        }
    }
}

/// Converts a bare literal. Keys keep `true`/`null`/... as strings.
pub(crate) fn parse_literal(s: &str, is_key: bool) -> Value {
    if !is_key {
        match s {
            "true" | "True" | "TRUE" | "yes" | "Yes" | "YES" | "on" | "On" | "ON" => {
                return Value::Bool(true);
            }
            "false" | "False" | "FALSE" | "no" | "No" | "NO" | "off" | "Off" | "OFF" => {
                return Value::Bool(false);
            }
            "null" | "Null" | "NULL" => return Value::Null,
            _ => {}
        }
    }
    if let Some(v) = php_numeric(s) {
        return v;
    }
    let radix = |prefix: &str, radix: u32| -> Option<Value> {
        let digits = s.strip_prefix(prefix)?;
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return None;
        }
        Some(match i64::from_str_radix(digits, radix) {
            Ok(i) => Value::Int(i),
            Err(_) => Value::Float(digits.chars().fold(0f64, |acc, c| {
                acc * radix as f64 + c.to_digit(radix).unwrap_or(0) as f64
            })),
        })
    };
    radix("0x", 16)
        .or_else(|| radix("0X", 16))
        .or_else(|| radix("0o", 8))
        .or_else(|| radix("0b", 2))
        .unwrap_or_else(|| Value::String(s.to_string()))
}

/// PHP `is_numeric` + `$value * 1`.
fn php_numeric(s: &str) -> Option<Value> {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    let mut is_float = false;
    if i < b.len() && b[i] == b'.' {
        is_float = true;
        i += 1;
        let f = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if int_digits == 0 && i == f {
            return None;
        }
    } else if int_digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let e = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j == e {
            return None;
        }
        is_float = true;
        i = j;
    }
    if i != b.len() {
        return None;
    }
    if !is_float && let Ok(v) = s.parse::<i64>() {
        return Some(Value::Int(v));
    }
    s.parse::<f64>().ok().map(Value::Float)
}
