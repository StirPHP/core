//! Pretty-printer from the PHP output AST ([`crate::ast`]) to PHP 8.3 source text.
//!
//! Style follows PSR-12 / PER-CS: braces on their own line for classes, functions and methods,
//! on the same line for control structures and closures; one blank line between members and
//! around declarations; spaces around binary operators; `fn ($x) => ...`.
//!
//! Parentheses are inserted only where PHP precedence/associativity requires them (see
//! [`crate::prec`]), plus a few places where PHP requires them regardless (nested ternaries,
//! `(new Foo())->bar()`, `(clone $a)->b`, closures used as callees, ...).
//!
//! Argument lists, parameter lists, arrays and `isset(...)` that do not fit into
//! [`PrintOptions::line_width`] are broken one item per line with a trailing comma.

use crate::ast::*;
use crate::escape;
use crate::prec::{
    Prec, expr_prec, interp_is_simple, is_callable_base, is_dereferencable, is_new_variable,
    is_prefix_form, is_static_base,
};

/// Printer configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintOptions {
    /// One level of indentation.
    pub indent: String,
    /// Lists wrap one item per line when a line would exceed this many characters.
    pub line_width: usize,
}

impl Default for PrintOptions {
    fn default() -> Self {
        Self {
            indent: "    ".into(),
            line_width: 100,
        }
    }
}

/// A position in the printed output. `line` and `col` are 0-based; `col` counts bytes from the
/// start of the line (as does `offset` from the start of the file).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Position {
    pub offset: u32,
    pub line: u32,
    pub col: u32,
}

/// The output range `[start, end)` printed for a node carrying `origin`.
///
/// Mappings are ordered by `start` (pre-order); nested nodes produce nested ranges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mapping {
    pub start: Position,
    pub end: Position,
    pub origin: Origin,
}

/// Prints a complete PHP file (ends with a newline).
pub fn print_file(file: &File, opts: &PrintOptions) -> String {
    let mut p = Printer::new(opts, false, false);
    p.file(file);
    p.out
}

/// Prints a complete PHP file and returns the output ranges of every node with an origin.
pub fn print_file_with_map(file: &File, opts: &PrintOptions) -> (String, Vec<Mapping>) {
    let mut p = Printer::new(opts, false, true);
    p.file(file);
    (p.out, p.mappings.unwrap_or_default())
}

/// Prints a single expression (as if at statement level, without `;`).
pub fn print_expr(e: &Expr, opts: &PrintOptions) -> String {
    let mut p = Printer::new(opts, false, false);
    p.expr(e);
    p.out
}

/// Prints statements at indentation level 0, separated by newlines (no trailing newline).
pub fn print_stmts(stmts: &[Stmt], opts: &PrintOptions) -> String {
    let mut p = Printer::new(opts, false, false);
    p.stmt_list(stmts);
    p.out
}

/// Prints a native type declaration.
pub fn print_type(t: &Type) -> String {
    type_string(t)
}

/// Prints a doc comment (without trailing newline).
pub fn print_doc(d: &DocComment, opts: &PrintOptions) -> String {
    let mut p = Printer::new(opts, false, false);
    p.doc_inner(d);
    p.out
}

// ---------------------------------------------------------------------------------------------

struct Printer<'o> {
    opts: &'o PrintOptions,
    out: String,
    line: u32,
    line_start: usize,
    /// Column in characters (for width decisions).
    col_chars: usize,
    at_line_start: bool,
    indent: usize,
    /// Measuring mode: lists never break.
    flat: bool,
    mappings: Option<Vec<Mapping>>,
}

type ItemFn<'f, 'o, T> = &'f dyn Fn(&mut Printer<'o>, &T);

impl<'o> Printer<'o> {
    fn new(opts: &'o PrintOptions, flat: bool, map: bool) -> Self {
        Self {
            opts,
            out: String::new(),
            line: 0,
            line_start: 0,
            col_chars: 0,
            at_line_start: true,
            indent: 0,
            flat,
            mappings: map.then(Vec::new),
        }
    }

    // --- low-level writing ----------------------------------------------------------------

    fn flush_indent(&mut self) {
        if self.at_line_start {
            self.at_line_start = false;
            for _ in 0..self.indent {
                self.out.push_str(&self.opts.indent);
                self.col_chars += self.opts.indent.chars().count();
            }
        }
    }

    fn w(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.flush_indent();
        self.out.push_str(s);
        match s.rfind('\n') {
            None => self.col_chars += s.chars().count(),
            Some(i) => {
                self.line += s.matches('\n').count() as u32;
                self.line_start = self.out.len() - (s.len() - i - 1);
                self.col_chars = s[i + 1..].chars().count();
            }
        }
    }

    fn nl(&mut self) {
        self.out.push('\n');
        self.line += 1;
        self.line_start = self.out.len();
        self.col_chars = 0;
        self.at_line_start = true;
    }

    fn cur_col(&self) -> usize {
        if self.at_line_start {
            self.indent * self.opts.indent.chars().count()
        } else {
            self.col_chars
        }
    }

    fn pos(&self) -> Position {
        Position {
            offset: self.out.len() as u32,
            line: self.line,
            col: (self.out.len() - self.line_start) as u32,
        }
    }

    fn begin(&mut self, origin: Option<Origin>) -> Option<usize> {
        let origin = origin?;
        self.mappings.as_ref()?;
        self.flush_indent();
        let pos = self.pos();
        let maps = self.mappings.as_mut()?;
        maps.push(Mapping {
            start: pos,
            end: pos,
            origin,
        });
        Some(maps.len() - 1)
    }

    fn end(&mut self, idx: Option<usize>) {
        if let Some(i) = idx {
            let pos = self.pos();
            if let Some(m) = self.mappings.as_mut() {
                m[i].end = pos;
            }
        }
    }

    /// Width (in characters) of the first line `f` prints, and whether it prints a newline.
    fn measure(&self, f: impl FnOnce(&mut Printer<'o>)) -> usize {
        let mut p = Printer::new(self.opts, true, false);
        f(&mut p);
        let first = p.out.split('\n').next().unwrap_or("");
        first.chars().count()
    }

    /// Prints `open item, item close`, or one item per line with trailing comma when the flat
    /// form does not fit (`trail` = width of what follows `close` on the same line).
    /// Returns whether the list was broken.
    fn comma_list<T>(
        &mut self,
        open: &str,
        close: &str,
        items: &[T],
        force_break: bool,
        trail: usize,
        f: ItemFn<'_, 'o, T>,
    ) -> bool {
        self.w(open);
        if items.is_empty() {
            self.w(close);
            return false;
        }
        let brk = !self.flat
            && (force_break || {
                let width = self.measure(|p| {
                    for (i, it) in items.iter().enumerate() {
                        if i > 0 {
                            p.w(", ");
                        }
                        f(p, it);
                    }
                });
                self.cur_col() + width + close.len() + trail > self.opts.line_width
            });
        if brk {
            self.indent += 1;
            for it in items {
                self.nl();
                f(self, it);
                self.w(",");
            }
            self.indent -= 1;
            self.nl();
        } else {
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    self.w(", ");
                }
                f(self, it);
            }
        }
        self.w(close);
        brk
    }

    // --- names, types, attributes, docs ---------------------------------------------------

    fn qname(&mut self, q: &QName) {
        if q.fully_qualified {
            self.w("\\");
        }
        self.w(&q.name);
    }

    fn var_name(&mut self, name: &str) {
        if is_ident(name) {
            self.w("$");
            self.w(name);
        } else {
            self.w("${");
            self.w(&escape::string_literal(name));
            self.w("}");
        }
    }

    fn ty(&mut self, t: &Type) {
        self.w(&type_string(t));
    }

    fn attr_groups(&mut self, groups: &[AttributeGroup], inline: bool) {
        for g in groups {
            self.w("#[");
            for (i, a) in g.attrs.iter().enumerate() {
                if i > 0 {
                    self.w(", ");
                }
                self.qname(&a.name);
                if let Some(args) = &a.args {
                    self.args(args, 0);
                }
            }
            self.w("]");
            if inline {
                self.w(" ");
            } else {
                self.nl();
            }
        }
    }

    /// Doc comment followed by a newline.
    fn doc(&mut self, d: &Option<DocComment>) {
        if let Some(d) = d
            && !d.is_empty()
        {
            self.doc_inner(d);
            self.nl();
        }
    }

    fn doc_inner(&mut self, d: &DocComment) {
        let clean = |s: &str| s.replace("*/", "*\\/");
        if d.summary.is_empty() && d.tags.len() == 1 && !d.tags[0].text.contains('\n') {
            let t = &d.tags[0];
            let text = clean(&t.text);
            let line = if text.is_empty() {
                format!("/** @{} */", t.name)
            } else {
                format!("/** @{} {} */", t.name, text)
            };
            if self.cur_col() + line.chars().count() <= self.opts.line_width {
                self.w(&line);
                return;
            }
        }
        self.w("/**");
        let line = |p: &mut Self, s: &str| {
            p.nl();
            if s.is_empty() {
                p.w(" *");
            } else {
                p.w(" * ");
                p.w(&clean(s));
            }
        };
        for s in &d.summary {
            line(self, s);
        }
        if !d.summary.is_empty() && !d.tags.is_empty() {
            line(self, "");
        }
        for t in &d.tags {
            let mut lines = t.text.split('\n');
            let first = lines.next().unwrap_or("");
            if first.is_empty() {
                line(self, &format!("@{}", t.name));
            } else {
                line(self, &format!("@{} {}", t.name, first));
            }
            for l in lines {
                line(self, l);
            }
        }
        self.nl();
        self.w(" */");
    }

    // --- file -----------------------------------------------------------------------------

    fn file(&mut self, f: &File) {
        self.w("<?php");
        self.nl();
        if let Some(d) = &f.doc
            && !d.is_empty()
        {
            self.nl();
            self.doc_inner(d);
            self.nl();
        }
        if f.strict_types {
            self.nl();
            self.w("declare(strict_types=1);");
            self.nl();
        }
        let braced = match &f.namespace {
            Some(ns) if ns.braced => {
                self.nl();
                self.w("namespace ");
                if let Some(n) = &ns.name {
                    self.w(n);
                    self.w(" ");
                }
                self.w("{");
                self.indent += 1;
                true
            }
            Some(ns) => {
                self.nl();
                self.w("namespace ");
                self.w(ns.name.as_deref().unwrap_or(""));
                self.w(";");
                self.nl();
                false
            }
            None => false,
        };
        if !f.uses.is_empty() {
            self.nl();
            let mut prev: Option<UseKind> = None;
            for u in &f.uses {
                if prev.is_some_and(|k| k != u.kind) {
                    self.nl();
                }
                prev = Some(u.kind);
                self.use_decl(u);
                self.nl();
            }
        }
        if !f.stmts.is_empty() {
            self.nl();
            self.stmt_list(&f.stmts);
            self.nl();
        }
        if braced {
            self.indent -= 1;
            if !self.at_line_start {
                self.nl();
            }
            self.w("}");
            self.nl();
        }
    }

    fn use_decl(&mut self, u: &UseDecl) {
        let kw = match u.kind {
            UseKind::Class => "use ",
            UseKind::Function => "use function ",
            UseKind::Const => "use const ",
        };
        let item = |p: &mut Self, it: &UseItem| {
            p.w(it.name.trim_start_matches('\\'));
            if let Some(a) = &it.alias {
                p.w(" as ");
                p.w(a);
            }
        };
        match &u.group_prefix {
            Some(prefix) => {
                self.w(kw);
                self.w(prefix.trim_start_matches('\\').trim_end_matches('\\'));
                self.w("\\");
                self.comma_list("{", "};", &u.items, false, 0, &item);
            }
            None => {
                for (i, it) in u.items.iter().enumerate() {
                    if i > 0 {
                        self.nl();
                    }
                    self.w(kw);
                    item(self, it);
                    self.w(";");
                }
            }
        }
    }

    // --- statements -----------------------------------------------------------------------

    /// Prints statements, one per line, starting on the current line.
    fn stmt_list(&mut self, stmts: &[Stmt]) {
        for (i, s) in stmts.iter().enumerate() {
            if i > 0 {
                self.nl();
                let prev = &stmts[i - 1];
                if needs_blank_between(prev, &stmts[i..]) {
                    self.nl();
                }
            }
            self.stmt(s);
        }
    }

    /// `{ ... }` with the statements indented.
    fn body(&mut self, stmts: &[Stmt]) {
        self.w("{");
        if !stmts.is_empty() {
            self.indent += 1;
            self.nl();
            self.stmt_list(stmts);
            self.indent -= 1;
        }
        self.nl();
        self.w("}");
    }

    fn stmt(&mut self, s: &Stmt) {
        let m = self.begin(s.origin);
        match &s.kind {
            StmtKind::Expr(e) => {
                self.expr(e);
                self.w(";");
            }
            StmtKind::Echo(es) => {
                self.w("echo ");
                for (i, e) in es.iter().enumerate() {
                    if i > 0 {
                        self.w(", ");
                    }
                    self.expr(e);
                }
                self.w(";");
            }
            StmtKind::Return(e) => {
                self.w("return");
                if let Some(e) = e {
                    self.w(" ");
                    self.expr(e);
                }
                self.w(";");
            }
            StmtKind::If {
                cond,
                then,
                elseifs,
                else_,
            } => {
                self.w("if (");
                self.expr(cond);
                self.w(") ");
                self.body(then);
                for ei in elseifs {
                    self.w(" elseif (");
                    self.expr(&ei.cond);
                    self.w(") ");
                    self.body(&ei.body);
                }
                if let Some(e) = else_ {
                    self.w(" else ");
                    self.body(e);
                }
            }
            StmtKind::While { cond, body } => {
                self.w("while (");
                self.expr(cond);
                self.w(") ");
                self.body(body);
            }
            StmtKind::DoWhile { body, cond } => {
                self.w("do ");
                self.body(body);
                self.w(" while (");
                self.expr(cond);
                self.w(");");
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                self.w("for (");
                self.expr_seq(init);
                self.w(";");
                if !cond.is_empty() {
                    self.w(" ");
                    self.expr_seq(cond);
                }
                self.w(";");
                if !step.is_empty() {
                    self.w(" ");
                    self.expr_seq(step);
                }
                self.w(") ");
                self.body(body);
            }
            StmtKind::Foreach {
                expr,
                key,
                value,
                by_ref,
                body,
            } => {
                self.w("foreach (");
                self.expr(expr);
                self.w(" as ");
                if let Some(k) = key {
                    self.expr(k);
                    self.w(" => ");
                }
                if *by_ref {
                    self.w("&");
                }
                self.expr(value);
                self.w(") ");
                self.body(body);
            }
            StmtKind::Switch { subject, cases } => {
                self.w("switch (");
                self.expr(subject);
                self.w(") {");
                self.indent += 1;
                for c in cases {
                    self.nl();
                    match &c.test {
                        Some(t) => {
                            self.w("case ");
                            self.expr(t);
                            self.w(":");
                        }
                        None => self.w("default:"),
                    }
                    if !c.body.is_empty() {
                        self.indent += 1;
                        self.nl();
                        self.stmt_list(&c.body);
                        self.indent -= 1;
                    }
                }
                self.indent -= 1;
                self.nl();
                self.w("}");
            }
            StmtKind::Break(n) | StmtKind::Continue(n) => {
                self.w(if matches!(s.kind, StmtKind::Break(_)) {
                    "break"
                } else {
                    "continue"
                });
                if let Some(n) = n {
                    self.w(&format!(" {n}"));
                }
                self.w(";");
            }
            StmtKind::Try {
                body,
                catches,
                finally,
            } => {
                self.w("try ");
                self.body(body);
                for c in catches {
                    self.w(" catch (");
                    for (i, t) in c.types.iter().enumerate() {
                        if i > 0 {
                            self.w(" | ");
                        }
                        self.qname(t);
                    }
                    if let Some(v) = &c.var {
                        self.w(" ");
                        self.var_name(v);
                    }
                    self.w(") ");
                    self.body(&c.body);
                }
                if let Some(f) = finally {
                    self.w(" finally ");
                    self.body(f);
                }
            }
            StmtKind::Throw(e) => {
                self.w("throw ");
                self.expr(e);
                self.w(";");
            }
            StmtKind::Global(names) => {
                self.w("global ");
                for (i, n) in names.iter().enumerate() {
                    if i > 0 {
                        self.w(", ");
                    }
                    self.var_name(n);
                }
                self.w(";");
            }
            StmtKind::Static(vars) => {
                self.w("static ");
                for (i, v) in vars.iter().enumerate() {
                    if i > 0 {
                        self.w(", ");
                    }
                    self.var_name(&v.name);
                    if let Some(d) = &v.default {
                        self.w(" = ");
                        self.expr(d);
                    }
                }
                self.w(";");
            }
            StmtKind::Unset(es) => {
                self.comma_list("unset(", ");", es, false, 0, &|p, e| p.expr(e));
            }
            StmtKind::Block(stmts) => self.body(stmts),
            StmtKind::InlineHtml(html) => {
                self.w("?>");
                if html.starts_with('\n') {
                    // `?>` swallows one directly following newline.
                    self.out.push('\n');
                    self.line += 1;
                    self.line_start = self.out.len();
                    self.col_chars = 0;
                }
                self.w(html);
                self.w("<?php");
            }
            StmtKind::Label(l) => {
                self.w(l);
                self.w(":");
            }
            StmtKind::Goto(l) => {
                self.w("goto ");
                self.w(l);
                self.w(";");
            }
            StmtKind::Nop => {}
            StmtKind::DocComment(d) => self.doc_inner(d),
            StmtKind::Comment(text) => {
                for (i, l) in text.split('\n').enumerate() {
                    if i > 0 {
                        self.nl();
                    }
                    if l.is_empty() {
                        self.w("//");
                    } else {
                        // `?>` would end PHP mode inside a line comment.
                        self.w("// ");
                        self.w(&l.replace("?>", "? >"));
                    }
                }
            }
            StmtKind::Function(f) => self.function(f),
            StmtKind::Class(c) => self.class(c),
            StmtKind::Interface(i) => self.interface(i),
            StmtKind::Trait(t) => self.trait_(t),
            StmtKind::Enum(e) => self.enum_(e),
            StmtKind::Const(c) => {
                self.doc(&c.doc);
                let mm = self.begin(c.origin);
                self.w("const ");
                for (i, (n, v)) in c.items.iter().enumerate() {
                    if i > 0 {
                        self.w(", ");
                    }
                    self.w(n);
                    self.w(" = ");
                    self.expr(v);
                }
                self.w(";");
                self.end(mm);
            }
            StmtKind::Use(u) => self.use_decl(u),
            StmtKind::Namespace { name, body } => {
                self.w("namespace");
                if let Some(n) = name {
                    self.w(" ");
                    self.w(n);
                }
                match body {
                    None => self.w(";"),
                    Some(b) => {
                        self.w(" ");
                        self.body(b);
                    }
                }
            }
            StmtKind::Declare(ds) => {
                self.w("declare(");
                for (i, (n, v)) in ds.iter().enumerate() {
                    if i > 0 {
                        self.w(", ");
                    }
                    self.w(n);
                    self.w("=");
                    self.expr(v);
                }
                self.w(");");
            }
        }
        self.end(m);
    }

    fn expr_seq(&mut self, es: &[Expr]) {
        for (i, e) in es.iter().enumerate() {
            if i > 0 {
                self.w(", ");
            }
            self.expr(e);
        }
    }

    // --- declarations ---------------------------------------------------------------------

    fn param(&mut self, p: &Param) {
        let m = self.begin(p.origin);
        self.attr_groups(&p.attrs, true);
        if let Some(pr) = &p.promote {
            match pr.visibility {
                Some(v) => {
                    self.w(v.as_str());
                    self.w(" ");
                }
                None if pr.set_visibility.is_none() && !pr.readonly => self.w("public "),
                None => {}
            }
            if let Some(v) = pr.set_visibility {
                self.w(v.as_str());
                self.w("(set) ");
            }
            if pr.readonly {
                self.w("readonly ");
            }
        }
        if let Some(t) = &p.ty {
            self.ty(t);
            self.w(" ");
        }
        if p.by_ref {
            self.w("&");
        }
        if p.variadic {
            self.w("...");
        }
        self.var_name(&p.name);
        if let Some(d) = &p.default {
            self.w(" = ");
            self.expr(d);
        }
        self.end(m);
    }

    /// `(params)[: ret]`; returns whether the parameter list was broken.
    fn params_and_return(&mut self, params: &[Param], ret: &Option<Type>, trail: usize) -> bool {
        let force = params.iter().any(|p| p.promote.is_some());
        let ret_s = ret.as_ref().map(type_string);
        let trail = trail + ret_s.as_ref().map_or(0, |s| s.len() + 2);
        let broke = self.comma_list("(", ")", params, force, trail, &|p, x| p.param(x));
        if let Some(r) = ret_s {
            self.w(": ");
            self.w(&r);
        }
        broke
    }

    /// Function/method body with the brace on its own line (or on the `)` line when the
    /// parameter list was broken).
    fn fn_body(&mut self, body: &[Stmt], broke: bool) {
        if broke {
            self.w(" ");
        } else {
            self.nl();
        }
        self.body(body);
    }

    fn function(&mut self, f: &FunctionDecl) {
        self.doc(&f.doc);
        self.attr_groups(&f.attrs, false);
        let m = self.begin(f.origin);
        self.w("function ");
        if f.by_ref {
            self.w("&");
        }
        self.w(&f.name);
        let broke = self.params_and_return(&f.params, &f.return_type, 0);
        self.fn_body(&f.body, broke);
        self.end(m);
    }

    fn class_header_tail(&mut self, c: &ClassDecl) {
        if let Some(e) = &c.extends {
            self.w(" extends ");
            self.qname(e);
        }
        self.name_list(" implements ", &c.implements);
    }

    fn name_list(&mut self, kw: &str, names: &[QName]) {
        if !names.is_empty() {
            self.w(kw);
            for (i, n) in names.iter().enumerate() {
                if i > 0 {
                    self.w(", ");
                }
                self.qname(n);
            }
        }
    }

    fn class(&mut self, c: &ClassDecl) {
        self.doc(&c.doc);
        self.attr_groups(&c.attrs, false);
        let m = self.begin(c.origin);
        if c.modifiers.is_final {
            self.w("final ");
        }
        if c.modifiers.is_abstract {
            self.w("abstract ");
        }
        if c.modifiers.is_readonly {
            self.w("readonly ");
        }
        self.w("class ");
        self.w(&c.name);
        self.class_header_tail(c);
        self.nl();
        self.members(&c.members);
        self.end(m);
    }

    fn interface(&mut self, i: &InterfaceDecl) {
        self.doc(&i.doc);
        self.attr_groups(&i.attrs, false);
        let m = self.begin(i.origin);
        self.w("interface ");
        self.w(&i.name);
        self.name_list(" extends ", &i.extends);
        self.nl();
        self.members(&i.members);
        self.end(m);
    }

    fn trait_(&mut self, t: &TraitDecl) {
        self.doc(&t.doc);
        self.attr_groups(&t.attrs, false);
        let m = self.begin(t.origin);
        self.w("trait ");
        self.w(&t.name);
        self.nl();
        self.members(&t.members);
        self.end(m);
    }

    fn enum_(&mut self, e: &EnumDecl) {
        self.doc(&e.doc);
        self.attr_groups(&e.attrs, false);
        let m = self.begin(e.origin);
        self.w("enum ");
        self.w(&e.name);
        if let Some(b) = &e.backing {
            self.w(": ");
            self.ty(b);
        }
        self.name_list(" implements ", &e.implements);
        self.nl();
        self.members(&e.members);
        self.end(m);
    }

    /// `{ members }` (the brace is printed on the current line).
    fn members(&mut self, members: &[Member]) {
        self.w("{");
        if !members.is_empty() {
            self.indent += 1;
            for (i, mem) in members.iter().enumerate() {
                self.nl();
                if i > 0 && blank_between_members(&members[i - 1], mem) {
                    self.nl();
                }
                self.member(mem);
            }
            self.indent -= 1;
        }
        self.nl();
        self.w("}");
    }

    fn member(&mut self, mem: &Member) {
        match mem {
            Member::TraitUse(t) => {
                let m = self.begin(t.origin);
                self.w("use ");
                for (i, n) in t.traits.iter().enumerate() {
                    if i > 0 {
                        self.w(", ");
                    }
                    self.qname(n);
                }
                if t.adaptations.is_empty() {
                    self.w(";");
                } else {
                    self.w(" {");
                    self.indent += 1;
                    for a in &t.adaptations {
                        self.nl();
                        self.trait_adaptation(a);
                    }
                    self.indent -= 1;
                    self.nl();
                    self.w("}");
                }
                self.end(m);
            }
            Member::Case(c) => {
                self.doc(&c.doc);
                self.attr_groups(&c.attrs, false);
                let m = self.begin(c.origin);
                self.w("case ");
                self.w(&c.name);
                if let Some(v) = &c.value {
                    self.w(" = ");
                    self.expr(v);
                }
                self.w(";");
                self.end(m);
            }
            Member::Const(c) => {
                self.doc(&c.doc);
                self.attr_groups(&c.attrs, false);
                let m = self.begin(c.origin);
                if c.is_final {
                    self.w("final ");
                }
                if let Some(v) = c.visibility {
                    self.w(v.as_str());
                    self.w(" ");
                }
                self.w("const ");
                if let Some(t) = &c.ty {
                    self.ty(t);
                    self.w(" ");
                }
                self.w(&c.name);
                self.w(" = ");
                self.expr(&c.value);
                self.w(";");
                self.end(m);
            }
            Member::Property(p) => {
                self.doc(&p.doc);
                self.attr_groups(&p.attrs, false);
                let m = self.begin(p.origin);
                self.w(p.visibility.as_str());
                self.w(" ");
                if let Some(v) = p.set_visibility {
                    self.w(v.as_str());
                    self.w("(set) ");
                }
                if p.is_static {
                    self.w("static ");
                }
                if p.readonly {
                    self.w("readonly ");
                }
                if let Some(t) = &p.ty {
                    self.ty(t);
                    self.w(" ");
                }
                self.var_name(&p.name);
                if let Some(d) = &p.default {
                    self.w(" = ");
                    self.expr(d);
                }
                self.w(";");
                self.end(m);
            }
            Member::Method(md) => {
                self.doc(&md.doc);
                self.attr_groups(&md.attrs, false);
                let m = self.begin(md.origin);
                if md.is_final {
                    self.w("final ");
                }
                if md.is_abstract {
                    self.w("abstract ");
                }
                if let Some(v) = md.visibility {
                    self.w(v.as_str());
                    self.w(" ");
                }
                if md.is_static {
                    self.w("static ");
                }
                self.w("function ");
                if md.by_ref {
                    self.w("&");
                }
                self.w(&md.name);
                let trail = if md.body.is_none() { 1 } else { 0 };
                let broke = self.params_and_return(&md.params, &md.return_type, trail);
                match &md.body {
                    None => self.w(";"),
                    Some(b) => self.fn_body(b, broke),
                }
                self.end(m);
            }
        }
    }

    fn trait_adaptation(&mut self, a: &TraitAdaptation) {
        match a {
            TraitAdaptation::Alias {
                trait_name,
                method,
                visibility,
                alias,
            } => {
                if let Some(t) = trait_name {
                    self.qname(t);
                    self.w("::");
                }
                self.w(method);
                self.w(" as");
                if let Some(v) = visibility {
                    self.w(" ");
                    self.w(v.as_str());
                }
                if let Some(a) = alias {
                    self.w(" ");
                    self.w(a);
                }
                self.w(";");
            }
            TraitAdaptation::Insteadof {
                trait_name,
                method,
                instead_of,
            } => {
                self.qname(trait_name);
                self.w("::");
                self.w(method);
                self.name_list(" insteadof ", instead_of);
                self.w(";");
            }
        }
    }

    // --- expressions ----------------------------------------------------------------------

    /// An expression in a delimited position (statement, argument, parentheses, ...).
    fn expr(&mut self, e: &Expr) {
        self.sub(e, Prec::Lowest, true);
    }

    /// An operand that may appear unparenthesized if its precedence is at least `min`.
    /// `rightmost` means nothing at this parenthesization level follows the operand, so a
    /// low-precedence prefix form (`throw`, `fn`, `print`, ...) needs no parentheses.
    fn sub(&mut self, e: &Expr, min: Prec, rightmost: bool) {
        let p = expr_prec(&e.kind);
        if p < min && !(rightmost && is_prefix_form(&e.kind)) {
            self.paren(e);
        } else {
            self.expr_inner(e, rightmost);
        }
    }

    fn paren(&mut self, e: &Expr) {
        self.w("(");
        self.expr_inner(e, true);
        self.w(")");
    }

    /// The base of a postfix dereference; parenthesized unless `ok`.
    fn deref_base(&mut self, e: &Expr, ok: fn(&ExprKind) -> bool) {
        if ok(&e.kind) {
            self.expr_inner(e, false);
        } else {
            self.paren(e);
        }
    }

    /// The class of `X::...`.
    fn class_ref_static(&mut self, c: &ClassRef) {
        match c {
            ClassRef::Expr(e) => self.deref_base(e, is_static_base),
            _ => self.class_ref_simple(c),
        }
    }

    /// The class of `new X` / `instanceof X`.
    fn class_ref_new(&mut self, c: &ClassRef) {
        match c {
            ClassRef::Expr(e) => self.deref_base(e, is_new_variable),
            _ => self.class_ref_simple(c),
        }
    }

    fn class_ref_simple(&mut self, c: &ClassRef) {
        match c {
            ClassRef::Name(q) => self.qname(q),
            ClassRef::SelfRef => self.w("self"),
            ClassRef::Static => self.w("static"),
            ClassRef::Parent => self.w("parent"),
            ClassRef::Expr(e) => self.expr(e),
        }
    }

    /// `->name` / `->{expr}`
    fn member_name(&mut self, m: &MemberName) {
        match m {
            MemberName::Ident(n) => self.w(n),
            MemberName::Expr(e) => {
                self.w("{");
                self.expr(e);
                self.w("}");
            }
        }
    }

    fn args(&mut self, args: &[Arg], trail: usize) {
        self.comma_list("(", ")", args, false, trail, &|p, a| p.arg(a));
    }

    fn arg(&mut self, a: &Arg) {
        match a {
            Arg::Positional(e) => self.expr(e),
            Arg::Named(n, e) => {
                self.w(n);
                self.w(": ");
                self.expr(e);
            }
            Arg::Spread(e) => {
                self.w("...");
                self.expr(e);
            }
            Arg::Placeholder => self.w("..."),
        }
    }

    fn array_item(&mut self, it: &ArrayItem) {
        if let Some(k) = &it.key {
            self.sub(k, Prec::YieldFrom, false);
            self.w(" => ");
        }
        if it.spread {
            self.w("...");
        }
        if it.by_ref {
            self.w("&");
        }
        self.expr(&it.value);
    }

    fn assign_target(&mut self, t: &Expr) {
        match &t.kind {
            ExprKind::List(_) | ExprKind::Array(_) => self.expr_inner(t, false),
            _ => self.deref_base(t, is_dereferencable),
        }
    }

    fn expr_inner(&mut self, e: &Expr, rightmost: bool) {
        let m = self.begin(e.origin);
        match &e.kind {
            ExprKind::Int(i) => {
                if *i == i64::MIN {
                    self.w("PHP_INT_MIN");
                } else {
                    self.w(&i.to_string());
                }
            }
            ExprKind::Float(f) => self.w(&float_literal(*f)),
            ExprKind::Str(s) => self.w(&escape::string_literal(s)),
            ExprKind::Interp(parts) => self.interp(parts, rightmost),
            ExprKind::Bool(b) => self.w(if *b { "true" } else { "false" }),
            ExprKind::Null => self.w("null"),
            ExprKind::Array(items) => {
                self.comma_list("[", "]", items, false, 0, &|p, it| p.array_item(it));
            }
            ExprKind::List(items) => {
                // Lists with holes are kept on one line (a hole on its own line reads badly).
                let has_hole = items.iter().any(Option::is_none);
                let f: ItemFn<'_, 'o, Option<ArrayItem>> = &|p, it| {
                    if let Some(it) = it {
                        p.array_item(it);
                    }
                };
                if has_hole {
                    let flat = std::mem::replace(&mut self.flat, true);
                    self.comma_list("[", "]", items, false, 0, f);
                    self.flat = flat;
                } else {
                    self.comma_list("[", "]", items, false, 0, f);
                }
            }
            ExprKind::Name(q) => self.qname(q),
            ExprKind::MagicConst(mc) => self.w(magic_const(*mc)),
            ExprKind::Var(n) => self.var_name(n),
            ExprKind::VarVar(inner) => match &inner.kind {
                ExprKind::Var(_) | ExprKind::VarVar(_) => {
                    self.w("$");
                    self.expr_inner(inner, false);
                }
                _ => {
                    self.w("${");
                    self.expr(inner);
                    self.w("}");
                }
            },
            ExprKind::PropFetch {
                object,
                prop,
                nullsafe,
            } => {
                self.deref_base(object, is_dereferencable);
                self.w(if *nullsafe { "?->" } else { "->" });
                self.member_name(prop);
            }
            ExprKind::StaticProp { class, prop } => {
                self.class_ref_static(class);
                self.w("::");
                match prop {
                    MemberName::Ident(n) => self.var_name(n),
                    MemberName::Expr(e) => {
                        self.w("${");
                        self.expr(e);
                        self.w("}");
                    }
                }
            }
            ExprKind::ClassConst { class, name } => {
                self.class_ref_static(class);
                self.w("::");
                self.member_name(name);
            }
            ExprKind::Index { base, index } => {
                self.deref_base(base, is_dereferencable);
                self.w("[");
                if let Some(i) = index {
                    self.expr(i);
                }
                self.w("]");
            }
            ExprKind::Call { callee, args } => {
                self.deref_base(callee, is_callable_base);
                self.args(args, 0);
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                nullsafe,
            } => {
                self.deref_base(object, is_dereferencable);
                self.w(if *nullsafe { "?->" } else { "->" });
                self.member_name(method);
                self.args(args, 0);
            }
            ExprKind::StaticCall {
                class,
                method,
                args,
            } => {
                self.class_ref_static(class);
                self.w("::");
                self.member_name(method);
                self.args(args, 0);
            }
            ExprKind::New { class, args } => {
                self.w("new ");
                self.class_ref_new(class);
                self.args(args, 0);
            }
            ExprKind::NewAnonClass { class, args } => {
                self.w("new ");
                self.attr_groups(&class.attrs, true);
                if class.modifiers.is_readonly {
                    self.w("readonly ");
                }
                self.w("class");
                if !args.is_empty() {
                    self.w(" ");
                    self.args(args, 0);
                }
                self.class_header_tail(class);
                if class.members.is_empty() {
                    self.w(" {}");
                } else {
                    self.w(" ");
                    self.members(&class.members);
                }
            }
            ExprKind::Unary { op, expr } => {
                if op.is_postfix() {
                    self.deref_base(expr, is_dereferencable);
                    self.w(op.token());
                } else {
                    self.w(op.token());
                    let sign = match op {
                        UnaryOp::Neg | UnaryOp::PreDec => Some('-'),
                        UnaryOp::Plus | UnaryOp::PreInc => Some('+'),
                        _ => None,
                    };
                    let p = expr_prec(&expr.kind);
                    let parens = p < op.prec() && !(rightmost && is_prefix_form(&expr.kind));
                    if !parens && sign.is_some() && leading_sign(&expr.kind) == sign {
                        self.w(" ");
                    }
                    self.sub(expr, op.prec(), rightmost);
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (lmin, rmin) = op.operand_min();
                self.sub(lhs, lmin, false);
                self.w(" ");
                self.w(op.token());
                self.w(" ");
                self.sub(rhs, rmin, rightmost);
            }
            ExprKind::Assign { target, value } => {
                self.assign_target(target);
                self.w(" = ");
                self.sub(value, Prec::Assign, rightmost);
            }
            ExprKind::AssignRef { target, value } => {
                self.assign_target(target);
                self.w(" = &");
                self.sub(value, Prec::Assign, rightmost);
            }
            ExprKind::AssignOp { op, target, value } => {
                self.assign_target(target);
                self.w(" ");
                self.w(op.token());
                self.w(" ");
                self.sub(value, Prec::Assign, rightmost);
            }
            ExprKind::Ternary { cond, then, else_ } => {
                let is_ternary = |e: &Expr| matches!(e.kind, ExprKind::Ternary { .. });
                if is_ternary(cond) {
                    self.paren(cond);
                } else {
                    self.sub(cond, Prec::Coalesce, false);
                }
                match then {
                    Some(t) => {
                        self.w(" ? ");
                        if is_ternary(t) {
                            self.paren(t);
                        } else {
                            self.expr(t);
                        }
                        self.w(" : ");
                    }
                    None => self.w(" ?: "),
                }
                if is_ternary(else_) {
                    self.paren(else_);
                } else {
                    self.sub(else_, Prec::Coalesce, rightmost);
                }
            }
            ExprKind::Instanceof { expr, class } => {
                self.sub(expr, Prec::Unary, false);
                self.w(" instanceof ");
                self.class_ref_new(class);
            }
            ExprKind::Cast { kind, expr } => {
                self.w(cast_token(*kind));
                self.w(" ");
                self.sub(expr, Prec::Unary, rightmost);
            }
            ExprKind::Silence(expr) => {
                self.w("@");
                self.sub(expr, Prec::Unary, rightmost);
            }
            ExprKind::Isset(es) => {
                self.comma_list("isset(", ")", es, false, 0, &|p, e| p.expr(e));
            }
            ExprKind::Empty(e) => {
                self.w("empty(");
                self.expr(e);
                self.w(")");
            }
            ExprKind::Clone(e) => {
                self.w("clone ");
                self.sub(e, Prec::CloneNew, rightmost);
            }
            ExprKind::Closure(c) => {
                self.attr_groups(&c.attrs, true);
                if c.is_static {
                    self.w("static ");
                }
                self.w("function ");
                if c.by_ref {
                    self.w("&");
                }
                let force = false;
                self.comma_list("(", ")", &c.params, force, 0, &|p, x| p.param(x));
                if !c.uses.is_empty() {
                    self.comma_list(" use (", ")", &c.uses, false, 0, &|p, u| {
                        if u.by_ref {
                            p.w("&");
                        }
                        p.var_name(&u.name);
                    });
                }
                if let Some(r) = &c.return_type {
                    self.w(": ");
                    self.ty(r);
                }
                if c.body.is_empty() {
                    self.w(" {}");
                } else {
                    self.w(" ");
                    self.body(&c.body);
                }
            }
            ExprKind::ArrowFn(f) => {
                self.attr_groups(&f.attrs, true);
                if f.is_static {
                    self.w("static ");
                }
                self.w("fn ");
                if f.by_ref {
                    self.w("&");
                }
                self.comma_list("(", ")", &f.params, false, 0, &|p, x| p.param(x));
                if let Some(r) = &f.return_type {
                    self.w(": ");
                    self.ty(r);
                }
                self.w(" => ");
                self.sub(&f.body, Prec::Lowest, rightmost);
            }
            ExprKind::Match { subject, arms } => {
                self.w("match (");
                self.expr(subject);
                self.w(") {");
                self.indent += 1;
                for arm in arms {
                    self.nl();
                    match &arm.conds {
                        None => self.w("default"),
                        Some(cs) => self.expr_seq(cs),
                    }
                    self.w(" => ");
                    self.expr(&arm.body);
                    self.w(",");
                }
                self.indent -= 1;
                self.nl();
                self.w("}");
            }
            ExprKind::Throw(e) => {
                self.w("throw ");
                self.sub(e, Prec::Lowest, rightmost);
            }
            ExprKind::Yield { key, value } => {
                self.w("yield");
                if let Some(k) = key {
                    self.w(" ");
                    self.sub(k, Prec::YieldFrom, false);
                    self.w(" =>");
                }
                if let Some(v) = value {
                    self.w(" ");
                    self.sub(v, Prec::Yield, rightmost);
                }
            }
            ExprKind::YieldFrom(e) => {
                self.w("yield from ");
                self.sub(e, Prec::YieldFrom, rightmost);
            }
            ExprKind::Print(e) => {
                self.w("print ");
                self.sub(e, Prec::Print, rightmost);
            }
            ExprKind::Exit(e) => {
                self.w("exit(");
                if let Some(e) = e {
                    self.expr(e);
                }
                self.w(")");
            }
            ExprKind::Include { kind, expr } => {
                self.w(match kind {
                    IncludeKind::Include => "include ",
                    IncludeKind::IncludeOnce => "include_once ",
                    IncludeKind::Require => "require ",
                    IncludeKind::RequireOnce => "require_once ",
                });
                self.sub(expr, Prec::Include, rightmost);
            }
            ExprKind::Paren(inner) => self.paren(inner),
            ExprKind::Raw(code) => self.w(code),
        }
        self.end(m);
    }

    fn interp(&mut self, parts: &[InterpPart], rightmost: bool) {
        if parts.iter().all(|p| matches!(p, InterpPart::Lit(_))) {
            let s: String = parts
                .iter()
                .map(|p| match p {
                    InterpPart::Lit(s) => s.as_str(),
                    InterpPart::Expr(_) => "",
                })
                .collect();
            self.w(&escape::string_literal(&s));
            return;
        }
        if interp_is_simple(parts) {
            let mut buf = String::from("\"");
            for part in parts {
                match part {
                    InterpPart::Lit(s) => escape::double_quoted_body(s, &mut buf),
                    InterpPart::Expr(e) => {
                        buf.push('{');
                        self.w(&buf);
                        buf.clear();
                        self.expr(e);
                        buf.push('}');
                    }
                }
            }
            buf.push('"');
            self.w(&buf);
            return;
        }
        // Concatenation fallback: `'a' . f() . 'b'` (a lone expression becomes `'' . f()`).
        if parts.len() == 1 {
            self.w("'' . ");
        }
        let last = parts.len() - 1;
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                self.w(" . ");
            }
            match part {
                InterpPart::Lit(s) => self.w(&escape::string_literal(s)),
                InterpPart::Expr(e) => {
                    let min = if i == 0 && last > 0 {
                        Prec::Concat
                    } else {
                        Prec::Shift
                    };
                    self.sub(e, min, rightmost && i == last);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() || !c.is_ascii() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric() || !c.is_ascii())
}

/// A float literal that always reparses as a float (`1.0`, `1.0E+25`, `NAN`, `-INF`).
fn float_literal(f: f64) -> String {
    if f.is_nan() {
        "NAN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "INF".into() } else { "-INF".into() }
    } else {
        // Rust's `Debug` output always contains `.` or `e` and round-trips exactly.
        format!("{f:?}")
    }
}

fn leading_sign(e: &ExprKind) -> Option<char> {
    match e {
        ExprKind::Int(i) if *i < 0 && *i != i64::MIN => Some('-'),
        ExprKind::Float(f) if f.is_sign_negative() && !f.is_nan() => Some('-'),
        ExprKind::Unary {
            op: UnaryOp::Neg | UnaryOp::PreDec,
            ..
        } => Some('-'),
        ExprKind::Unary {
            op: UnaryOp::Plus | UnaryOp::PreInc,
            ..
        } => Some('+'),
        _ => None,
    }
}

fn magic_const(m: MagicConst) -> &'static str {
    match m {
        MagicConst::Line => "__LINE__",
        MagicConst::File => "__FILE__",
        MagicConst::Dir => "__DIR__",
        MagicConst::Function => "__FUNCTION__",
        MagicConst::Class => "__CLASS__",
        MagicConst::Trait => "__TRAIT__",
        MagicConst::Method => "__METHOD__",
        MagicConst::Namespace => "__NAMESPACE__",
        MagicConst::Property => "__PROPERTY__",
    }
}

fn cast_token(k: CastKind) -> &'static str {
    match k {
        CastKind::Int => "(int)",
        CastKind::Float => "(float)",
        CastKind::String => "(string)",
        CastKind::Bool => "(bool)",
        CastKind::Array => "(array)",
        CastKind::Object => "(object)",
    }
}

fn qname_string(q: &QName) -> String {
    if q.fully_qualified {
        format!("\\{}", q.name)
    } else {
        q.name.to_string()
    }
}

fn type_string(t: &Type) -> String {
    match t {
        Type::Builtin(b) => b.as_str().into(),
        Type::Class(q) => qname_string(q),
        Type::Nullable(inner) => match &**inner {
            Type::Builtin(BuiltinType::Null | BuiltinType::Mixed) => type_string(inner),
            Type::Nullable(_) | Type::Union(_) | Type::Intersection(_) => {
                union_string(&[(**inner).clone(), Type::null()])
            }
            _ => format!("?{}", type_string(inner)),
        },
        Type::Union(members) => union_string(members),
        Type::Intersection(members) => members
            .iter()
            .map(type_string)
            .collect::<Vec<_>>()
            .join("&"),
    }
}

/// Flattens nested unions/nullables; `null` goes last; intersections are parenthesized (DNF).
fn union_string(members: &[Type]) -> String {
    fn flatten<'t>(t: &'t Type, out: &mut Vec<&'t Type>, has_null: &mut bool) {
        match t {
            Type::Union(ms) => ms.iter().for_each(|m| flatten(m, out, has_null)),
            Type::Nullable(inner) => {
                *has_null = true;
                flatten(inner, out, has_null);
            }
            Type::Builtin(BuiltinType::Null) => *has_null = true,
            _ => out.push(t),
        }
    }
    let mut flat = Vec::new();
    let mut has_null = false;
    for m in members {
        flatten(m, &mut flat, &mut has_null);
    }
    let mut parts: Vec<String> = flat
        .iter()
        .map(|t| match t {
            Type::Intersection(_) => format!("({})", type_string(t)),
            _ => type_string(t),
        })
        .collect();
    if has_null {
        parts.push("null".into());
    }
    parts.join("|")
}

fn is_decl(s: &Stmt) -> bool {
    matches!(
        s.kind,
        StmtKind::Function(_)
            | StmtKind::Class(_)
            | StmtKind::Interface(_)
            | StmtKind::Trait(_)
            | StmtKind::Enum(_)
            | StmtKind::Namespace { body: Some(_), .. }
    )
}

/// Blank line between `prev` and `rest[0]`: around declarations (a doc comment or comment
/// directly before a statement is attached to it).
fn needs_blank_between(prev: &Stmt, rest: &[Stmt]) -> bool {
    if matches!(
        prev.kind,
        StmtKind::Nop | StmtKind::DocComment(_) | StmtKind::Comment(_)
    ) {
        return false;
    }
    if matches!(rest[0].kind, StmtKind::Nop) {
        return false;
    }
    if is_decl(prev) {
        return true;
    }
    rest.iter()
        .find(|s| !matches!(s.kind, StmtKind::DocComment(_) | StmtKind::Comment(_)))
        .is_some_and(is_decl)
}

fn blank_between_members(a: &Member, b: &Member) -> bool {
    fn info(m: &Member) -> (u8, bool) {
        match m {
            Member::TraitUse(_) => (0, false),
            Member::Case(c) => (1, c.doc.is_some() || !c.attrs.is_empty()),
            Member::Const(c) => (2, c.doc.is_some() || !c.attrs.is_empty()),
            Member::Property(p) => (3, p.doc.is_some() || !p.attrs.is_empty()),
            Member::Method(_) => (4, true),
        }
    }
    let (ka, da) = info(a);
    let (kb, db) = info(b);
    ka != kb || da || db
}
