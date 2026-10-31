//! PHPStan type grammar, following `phpstan/phpdoc-parser`'s `TypeParser`.

use smol_str::SmolStr;
use text_size::{TextRange, TextSize};

use crate::ast::*;
use crate::lexer::{Token, TokenKind as K};

pub(crate) struct PErr {
    pub msg: String,
}

pub(crate) type PResult<T> = Result<T, PErr>;

#[derive(Clone, Copy)]
pub(crate) struct Checkpoint {
    pos: usize,
    last_end: u32,
    depth: u32,
}

/// Cursor over a slice of tokens. Horizontal whitespace is always skipped; newlines are
/// skipped while inside brackets (`depth > 0`).
pub(crate) struct Stream<'a> {
    pub src: &'a str,
    pub toks: &'a [Token],
    pub pos: usize,
    pub end: usize,
    pub base: u32,
    pub depth: u32,
    pub last_end: u32,
}

impl<'a> Stream<'a> {
    pub fn new(src: &'a str, toks: &'a [Token], pos: usize, end: usize, base: u32) -> Self {
        let last_end = if pos > 0 { toks[pos - 1].end } else { 0 };
        Self {
            src,
            toks,
            pos,
            end,
            base,
            depth: 0,
            last_end,
        }
    }

    fn sig(&self, mut i: usize) -> usize {
        while i < self.end {
            match self.toks[i].kind {
                K::Ws => i += 1,
                K::Eol if self.depth > 0 => i += 1,
                _ => break,
            }
        }
        i
    }

    pub fn peek_tok(&self) -> Option<Token> {
        let i = self.sig(self.pos);
        (i < self.end).then(|| self.toks[i])
    }

    pub fn peek(&self) -> Option<K> {
        self.peek_tok().map(|t| t.kind)
    }

    pub fn peek2(&self) -> Option<K> {
        let i = self.sig(self.pos);
        if i >= self.end {
            return None;
        }
        let j = self.sig(i + 1);
        (j < self.end).then(|| self.toks[j].kind)
    }

    pub fn at(&self, k: K) -> bool {
        self.peek() == Some(k)
    }

    /// Current token is an identifier with exactly this text.
    pub fn at_ident(&self, text: &str) -> bool {
        self.peek_tok()
            .is_some_and(|t| t.kind == K::Identifier && self.text(t) == text)
    }

    /// Whether whitespace precedes the next significant token.
    pub fn ws_before(&self) -> bool {
        let i = self.sig(self.pos);
        i > self.pos
    }

    pub fn bump(&mut self) -> Token {
        let i = self.sig(self.pos);
        let t = self.toks[i];
        self.pos = i + 1;
        self.last_end = t.end;
        t
    }

    pub fn eat(&mut self, k: K) -> Option<Token> {
        self.at(k).then(|| self.bump())
    }

    pub fn eat_ident(&mut self, text: &str) -> Option<Token> {
        self.at_ident(text).then(|| self.bump())
    }

    pub fn expect(&mut self, k: K, what: &str) -> PResult<Token> {
        match self.eat(k) {
            Some(t) => Ok(t),
            None => Err(self.err(format!("expected {what}"))),
        }
    }

    pub fn text(&self, t: Token) -> &'a str {
        &self.src[t.start as usize..t.end as usize]
    }

    pub fn err(&self, msg: String) -> PErr {
        let found = match self.peek_tok() {
            Some(t) if t.kind == K::Eol => "end of line".to_string(),
            Some(t) => format!("'{}'", self.text(t)),
            None => "end of input".to_string(),
        };
        PErr {
            msg: format!("Unexpected {found}, {msg}"),
        }
    }

    /// Start offset (relative) of the next significant token.
    pub fn start(&self) -> u32 {
        self.peek_tok().map_or(self.last_end, |t| t.start)
    }

    pub fn range(&self, start: u32) -> TextRange {
        let end = self.last_end.max(start);
        self.abs(start, end)
    }

    pub fn abs(&self, start: u32, end: u32) -> TextRange {
        TextRange::new(
            TextSize::from(self.base + start),
            TextSize::from(self.base + end),
        )
    }

    pub fn tok_range(&self, t: Token) -> TextRange {
        self.abs(t.start, t.end)
    }

    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            pos: self.pos,
            last_end: self.last_end,
            depth: self.depth,
        }
    }

    pub fn restore(&mut self, c: Checkpoint) {
        self.pos = c.pos;
        self.last_end = c.last_end;
        self.depth = c.depth;
    }

    fn open(&mut self, k: K, what: &str) -> PResult<Token> {
        let t = self.expect(k, what)?;
        self.depth += 1;
        Ok(t)
    }

    fn close(&mut self, k: K, what: &str) -> PResult<Token> {
        let t = self.expect(k, what)?;
        self.depth -= 1;
        Ok(t)
    }

    pub fn variable(&self, t: Token) -> Variable {
        Variable {
            name: SmolStr::new(&self.text(t)[1..]),
            range: self.tok_range(t),
        }
    }

    pub fn name(&self, t: Token) -> Name {
        Name {
            name: SmolStr::new(self.text(t)),
            range: self.tok_range(t),
        }
    }

    // ---------------------------------------------------------------- types

    /// Top-level type: `?Atomic` | `Atomic (| Atomic)*` | `Atomic (& Atomic)*`.
    pub fn parse_type(&mut self) -> PResult<TypeNode> {
        let start = self.start();
        if self.at(K::Nullable) {
            return self.parse_nullable();
        }
        let first = self.parse_atomic()?;
        self.parse_union_or_intersection(start, first)
    }

    fn parse_union_or_intersection(&mut self, start: u32, first: TypeNode) -> PResult<TypeNode> {
        if self.at(K::Union) {
            let mut types = vec![first];
            while self.eat(K::Union).is_some() {
                types.push(self.parse_atomic()?);
            }
            Ok(TypeNode::new(TypeKind::Union(types), self.range(start)))
        } else if self.at(K::Intersection) && !self.amp_is_ref() {
            let mut types = vec![first];
            while self.at(K::Intersection) && !self.amp_is_ref() {
                self.bump();
                types.push(self.parse_atomic()?);
            }
            Ok(TypeNode::new(
                TypeKind::Intersection(types),
                self.range(start),
            ))
        } else {
            Ok(first)
        }
    }

    /// `&` followed by a variable or `...` is a by-reference marker, not an intersection.
    fn amp_is_ref(&self) -> bool {
        matches!(self.peek2(), Some(K::Variable | K::Variadic))
    }

    /// Type inside parentheses: additionally allows conditional types.
    fn sub_parse(&mut self) -> PResult<TypeNode> {
        let start = self.start();
        if self.at(K::Nullable) {
            return self.parse_nullable();
        }
        if self.at(K::Variable) {
            let v = self.bump();
            let var = self.variable(v);
            if !self.at_ident("is") {
                return Err(self.err("expected 'is'".into()));
            }
            return self.parse_conditional(start, ConditionalSubject::Parameter(var));
        }
        let t = self.parse_atomic()?;
        if self.at_ident("is") {
            return self.parse_conditional(start, ConditionalSubject::Type(t));
        }
        self.parse_union_or_intersection(start, t)
    }

    fn parse_conditional(&mut self, start: u32, subject: ConditionalSubject) -> PResult<TypeNode> {
        self.bump(); // is
        let negated = self.eat_ident("not").is_some();
        let target = self.parse_type()?;
        self.expect(K::Nullable, "'?'")?;
        let if_type = self.parse_type()?;
        self.expect(K::Colon, "':'")?;
        let else_type = self.sub_parse()?;
        Ok(TypeNode::new(
            TypeKind::Conditional(Box::new(ConditionalType {
                subject,
                negated,
                target,
                if_type,
                else_type,
            })),
            self.range(start),
        ))
    }

    fn parse_nullable(&mut self) -> PResult<TypeNode> {
        let start = self.start();
        self.bump();
        let inner = self.parse_atomic()?;
        Ok(TypeNode::new(
            TypeKind::Nullable(Box::new(inner)),
            self.range(start),
        ))
    }

    fn parse_atomic(&mut self) -> PResult<TypeNode> {
        let start = self.start();
        let Some(tok) = self.peek_tok() else {
            return Err(self.err("expected type".into()));
        };
        match tok.kind {
            K::OpenParen => {
                self.open(K::OpenParen, "'('")?;
                let mut inner = self.sub_parse()?;
                self.close(K::CloseParen, "')'")?;
                inner.range = self.range(start);
                Ok(self.parse_array_suffix(start, inner))
            }
            K::This => {
                self.bump();
                let t = TypeNode::new(TypeKind::This, self.range(start));
                Ok(self.parse_array_suffix(start, t))
            }
            K::Identifier => self.parse_identifier_type(),
            K::Int | K::Float | K::SingleQuoted | K::DoubleQuoted => {
                self.bump();
                let c = self.const_literal(tok);
                Ok(TypeNode::new(TypeKind::Const(c), self.range(start)))
            }
            _ => Err(self.err("expected type".into())),
        }
    }

    fn parse_identifier_type(&mut self) -> PResult<TypeNode> {
        let start = self.start();
        let tok = self.bump();
        let name = self.name(tok);

        if self.at(K::DoubleColon) && !self.ws_before() {
            self.bump();
            let mut pat = String::new();
            while matches!(self.peek(), Some(K::Identifier | K::Wildcard))
                && (pat.is_empty() || !self.ws_before())
            {
                let t = self.bump();
                pat.push_str(self.text(t));
            }
            if pat.is_empty() {
                return Err(self.err("expected constant name".into()));
            }
            let t = TypeNode::new(
                TypeKind::Const(ConstExpr::ClassConst {
                    class: name.name,
                    name: pat.into(),
                }),
                self.range(start),
            );
            return Ok(self.parse_array_suffix(start, t));
        }

        let mut ty = TypeNode::new(TypeKind::Identifier(name.name.clone()), name.range);
        if !self.ws_before() {
            match self.peek() {
                Some(K::OpenAngle) => {
                    let mut parsed = None;
                    if is_callable_name(&name.name) {
                        let cp = self.checkpoint();
                        match self.parse_callable(start, name.clone(), true) {
                            Ok(t) => parsed = Some(t),
                            Err(_) => self.restore(cp),
                        }
                    }
                    ty = match parsed {
                        Some(t) => t,
                        None => self.parse_generic(start, name)?,
                    };
                }
                Some(K::OpenParen) => {
                    let cp = self.checkpoint();
                    match self.parse_callable(start, name, false) {
                        Ok(t) => ty = t,
                        Err(e) => {
                            // Clearly-intended callables report their error; otherwise
                            // `Foo (desc)` is just an identifier followed by text.
                            if is_callable_name(&ty_name(&ty)) {
                                return Err(e);
                            }
                            self.restore(cp)
                        }
                    }
                }
                Some(K::OpenCurly) => {
                    if let Some(kind) = ArrayShapeKind::from_name(&name.name) {
                        ty = self.parse_array_shape(start, kind)?;
                    } else if name.name.eq_ignore_ascii_case("object") {
                        ty = self.parse_object_shape(start)?;
                    }
                }
                _ => {}
            }
        }
        Ok(self.parse_array_suffix(start, ty))
    }

    fn parse_array_suffix(&mut self, start: u32, mut ty: TypeNode) -> TypeNode {
        while self.at(K::OpenSquare) && !self.ws_before() {
            let cp = self.checkpoint();
            self.bump();
            self.depth += 1;
            if self.eat(K::CloseSquare).is_some() {
                self.depth -= 1;
                ty = TypeNode::new(TypeKind::Array(Box::new(ty)), self.range(start));
                continue;
            }
            match self.parse_type() {
                Ok(offset) if self.at(K::CloseSquare) => {
                    self.bump();
                    self.depth -= 1;
                    ty = TypeNode::new(
                        TypeKind::OffsetAccess {
                            ty: Box::new(ty),
                            offset: Box::new(offset),
                        },
                        self.range(start),
                    );
                }
                _ => {
                    self.restore(cp);
                    break;
                }
            }
        }
        ty
    }

    fn parse_generic(&mut self, start: u32, base: Name) -> PResult<TypeNode> {
        self.open(K::OpenAngle, "'<'")?;
        let mut args = Vec::new();
        loop {
            args.push(self.parse_generic_arg()?);
            if self.eat(K::Comma).is_none() || self.at(K::CloseAngle) {
                break;
            }
        }
        self.close(K::CloseAngle, "',' or '>'")?;
        Ok(TypeNode::new(
            TypeKind::Generic { base, args },
            self.range(start),
        ))
    }

    fn parse_generic_arg(&mut self) -> PResult<GenericArg> {
        if let Some(t) = self.eat(K::Wildcard) {
            return Ok(GenericArg {
                variance: Variance::Bivariant,
                ty: TypeNode::new(TypeKind::Identifier("mixed".into()), self.tok_range(t)),
            });
        }
        let variance_follows = !matches!(
            self.peek2(),
            None | Some(
                K::Comma
                    | K::CloseAngle
                    | K::Union
                    | K::Intersection
                    | K::OpenSquare
                    | K::OpenAngle
                    | K::DoubleColon
                    | K::OpenCurly
                    | K::OpenParen
            )
        );
        let variance = if variance_follows && self.eat_ident("covariant").is_some() {
            Variance::Covariant
        } else if variance_follows && self.eat_ident("contravariant").is_some() {
            Variance::Contravariant
        } else {
            Variance::Invariant
        };
        Ok(GenericArg {
            variance,
            ty: self.parse_type()?,
        })
    }

    fn parse_callable(&mut self, start: u32, name: Name, templates: bool) -> PResult<TypeNode> {
        let templates = if templates {
            self.parse_template_list()?
        } else {
            Vec::new()
        };
        self.open(K::OpenParen, "'('")?;
        let mut params = Vec::new();
        while !self.at(K::CloseParen) {
            params.push(self.parse_callable_param()?);
            if self.eat(K::Comma).is_none() {
                break;
            }
        }
        self.close(K::CloseParen, "',' or ')'")?;
        self.expect(K::Colon, "':' and a return type")?;
        let return_type = if self.at(K::Nullable) {
            self.parse_nullable()?
        } else {
            self.parse_atomic()?
        };
        Ok(TypeNode::new(
            TypeKind::Callable(Box::new(CallableType {
                name,
                templates,
                params,
                return_type,
            })),
            self.range(start),
        ))
    }

    fn parse_callable_param(&mut self) -> PResult<CallableParam> {
        let start = self.start();
        // `...mixed` (variadic marker before the type) is accepted as well as `mixed ...`.
        let leading_variadic = self.eat(K::Variadic).is_some();
        let ty = self.parse_type()?;
        let by_ref = self.eat(K::Intersection).is_some();
        let variadic = self.eat(K::Variadic).is_some() || leading_variadic;
        let name = self.eat(K::Variable).map(|t| self.variable(t));
        let optional = self.eat(K::Equal).is_some();
        Ok(CallableParam {
            ty,
            by_ref,
            variadic,
            name,
            optional,
            range: self.range(start),
        })
    }

    /// `<T, U of Foo = Bar>`
    pub fn parse_template_list(&mut self) -> PResult<Vec<TemplateParam>> {
        self.open(K::OpenAngle, "'<'")?;
        let mut out = Vec::new();
        loop {
            out.push(self.parse_template_param()?);
            if self.eat(K::Comma).is_none() || self.at(K::CloseAngle) {
                break;
            }
        }
        self.close(K::CloseAngle, "',' or '>'")?;
        Ok(out)
    }

    /// `T [of|as Bound] [super Lower] [= Default]`
    pub fn parse_template_param(&mut self) -> PResult<TemplateParam> {
        let start = self.start();
        let t = self.expect(K::Identifier, "template name")?;
        let name = self.name(t);
        let bound = if self.eat_ident("of").is_some() || self.eat_ident("as").is_some() {
            Some(self.parse_type()?)
        } else {
            None
        };
        let lower_bound = if self.eat_ident("super").is_some() {
            Some(self.parse_type()?)
        } else {
            None
        };
        let default = if self.eat(K::Equal).is_some() {
            Some(self.parse_type()?)
        } else {
            None
        };
        Ok(TemplateParam {
            name,
            bound,
            lower_bound,
            default,
            range: self.range(start),
        })
    }

    fn parse_array_shape(&mut self, start: u32, kind: ArrayShapeKind) -> PResult<TypeNode> {
        self.open(K::OpenCurly, "'{'")?;
        let mut items = Vec::new();
        let mut sealed = true;
        let mut unsealed = None;
        loop {
            if self.at(K::CloseCurly) {
                break;
            }
            if self.eat(K::Variadic).is_some() {
                sealed = false;
                if self.at(K::OpenAngle) {
                    unsealed = Some(Box::new(self.parse_unsealed()?));
                }
                self.eat(K::Comma);
                break;
            }
            items.push(self.parse_shape_item()?);
            if self.eat(K::Comma).is_none() {
                break;
            }
        }
        self.close(K::CloseCurly, "',' or '}'")?;
        Ok(TypeNode::new(
            TypeKind::ArrayShape(Box::new(ArrayShape {
                kind,
                items,
                sealed,
                unsealed,
            })),
            self.range(start),
        ))
    }

    fn parse_unsealed(&mut self) -> PResult<UnsealedType> {
        let start = self.start();
        self.open(K::OpenAngle, "'<'")?;
        let first = self.parse_type()?;
        let (key, value) = if self.eat(K::Comma).is_some() && !self.at(K::CloseAngle) {
            let v = self.parse_type()?;
            self.eat(K::Comma);
            (Some(first), v)
        } else {
            (None, first)
        };
        self.close(K::CloseAngle, "'>'")?;
        Ok(UnsealedType {
            key,
            value,
            range: self.range(start),
        })
    }

    fn try_shape_key(&mut self) -> Option<ShapeKey> {
        let t = self.peek_tok()?;
        let kind = match t.kind {
            K::Identifier => ShapeKeyKind::Identifier(self.text(t).into()),
            K::Int => ShapeKeyKind::Int(self.text(t).into()),
            K::SingleQuoted | K::DoubleQuoted => {
                let quote = if t.kind == K::SingleQuoted {
                    Quote::Single
                } else {
                    Quote::Double
                };
                ShapeKeyKind::String {
                    value: unquote(self.text(t), quote),
                    quote,
                }
            }
            _ => return None,
        };
        self.bump();
        Some(ShapeKey {
            kind,
            range: self.tok_range(t),
        })
    }

    fn parse_shape_item(&mut self) -> PResult<ArrayShapeItem> {
        let start = self.start();
        let cp = self.checkpoint();
        if let Some(key) = self.try_shape_key() {
            let optional = self.eat(K::Nullable).is_some();
            if self.eat(K::Colon).is_some() {
                let value = self.parse_type()?;
                return Ok(ArrayShapeItem {
                    key: Some(key),
                    optional,
                    value,
                    range: self.range(start),
                });
            }
        }
        self.restore(cp);
        let value = self.parse_type()?;
        Ok(ArrayShapeItem {
            key: None,
            optional: false,
            value,
            range: self.range(start),
        })
    }

    fn parse_object_shape(&mut self, start: u32) -> PResult<TypeNode> {
        self.open(K::OpenCurly, "'{'")?;
        let mut items = Vec::new();
        while !self.at(K::CloseCurly) {
            let istart = self.start();
            let key = match self.try_shape_key() {
                Some(k) if !matches!(k.kind, ShapeKeyKind::Int(_)) => k,
                _ => return Err(self.err("expected object shape key".into())),
            };
            let optional = self.eat(K::Nullable).is_some();
            self.expect(K::Colon, "':'")?;
            let value = self.parse_type()?;
            items.push(ObjectShapeItem {
                key,
                optional,
                value,
                range: self.range(istart),
            });
            if self.eat(K::Comma).is_none() {
                break;
            }
        }
        self.close(K::CloseCurly, "',' or '}'")?;
        Ok(TypeNode::new(
            TypeKind::ObjectShape(items),
            self.range(start),
        ))
    }

    fn const_literal(&self, t: Token) -> ConstExpr {
        let raw = self.text(t);
        match t.kind {
            K::Int => ConstExpr::Int {
                raw: raw.into(),
                value: parse_php_int(raw),
            },
            K::Float => ConstExpr::Float {
                raw: raw.into(),
                value: raw.replace('_', "").parse().unwrap_or(f64::NAN),
            },
            K::SingleQuoted => ConstExpr::String {
                value: unquote(raw, Quote::Single),
                quote: Quote::Single,
            },
            _ => ConstExpr::String {
                value: unquote(raw, Quote::Double),
                quote: Quote::Double,
            },
        }
    }
}

fn ty_name(t: &TypeNode) -> SmolStr {
    t.as_identifier().map(SmolStr::new).unwrap_or_default()
}

pub(crate) fn is_callable_name(name: &str) -> bool {
    let n = name.strip_prefix('\\').unwrap_or(name).to_ascii_lowercase();
    matches!(
        n.as_str(),
        "callable" | "pure-callable" | "closure" | "pure-closure"
    )
}

/// Parses a PHP integer literal (`0x1F`, `0o17`, `017`, `0b101`, `1_000`, `-5`).
pub fn parse_php_int(raw: &str) -> Option<i64> {
    let s = raw.replace('_', "");
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, &s[..]),
    };
    let lower = s.to_ascii_lowercase();
    let (digits, radix) = if let Some(h) = lower.strip_prefix("0x") {
        (h, 16)
    } else if let Some(o) = lower.strip_prefix("0o") {
        (o, 8)
    } else if let Some(b) = lower.strip_prefix("0b") {
        (b, 2)
    } else if lower.len() > 1 && lower.starts_with('0') {
        (&lower[1..], 8)
    } else {
        (lower.as_str(), 10)
    };
    let v = i128::from_str_radix(digits, radix).ok()?;
    i64::try_from(if neg { -v } else { v }).ok()
}

/// Strips quotes and processes escapes (PHP semantics).
pub(crate) fn unquote(raw: &str, quote: Quote) -> String {
    let inner = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(&n) = chars.peek() else {
            out.push('\\');
            break;
        };
        match quote {
            Quote::Single => {
                if n == '\'' || n == '\\' {
                    out.push(n);
                    chars.next();
                } else {
                    out.push('\\');
                }
            }
            Quote::Double => {
                let simple = match n {
                    'n' => Some('\n'),
                    't' => Some('\t'),
                    'r' => Some('\r'),
                    'v' => Some('\x0b'),
                    'e' => Some('\x1b'),
                    'f' => Some('\x0c'),
                    '\\' => Some('\\'),
                    '$' => Some('$'),
                    '"' => Some('"'),
                    _ => None,
                };
                if let Some(s) = simple {
                    out.push(s);
                    chars.next();
                } else if n == 'x' {
                    chars.next();
                    let mut h = String::new();
                    while h.len() < 2 && chars.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                        h.push(chars.next().unwrap_or_default());
                    }
                    match u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) {
                        Some(ch) if !h.is_empty() => out.push(ch),
                        _ => out.push_str("\\x"),
                    }
                } else if n == 'u' {
                    chars.next();
                    if chars.peek() == Some(&'{') {
                        chars.next();
                        let mut h = String::new();
                        while let Some(&c) = chars.peek() {
                            chars.next();
                            if c == '}' {
                                break;
                            }
                            h.push(c);
                        }
                        if let Some(ch) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32)
                        {
                            out.push(ch);
                        }
                    } else {
                        out.push_str("\\u");
                    }
                } else if n.is_digit(8) {
                    let mut o = String::new();
                    while o.len() < 3 && chars.peek().is_some_and(|c| c.is_digit(8)) {
                        o.push(chars.next().unwrap_or_default());
                    }
                    if let Some(ch) = u32::from_str_radix(&o, 8).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                } else {
                    out.push('\\');
                }
            }
        }
    }
    out
}
