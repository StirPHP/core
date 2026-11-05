//! What scripts see of the syntax tree: nodes by the names PHP has for them, never the tree itself.

use std::sync::LazyLock;

use rustc_hash::FxHashMap;
use stir_db::SourceFile;
use stir_script::rhai::{Array, Dynamic, Engine, INT, ImmutableString, Map};
use stir_syntax::SyntaxKind::*;
use stir_syntax::{NodeOrToken, SyntaxKind, SyntaxNode, TextRange};

/// The kinds of node scripts can ask for and see. These names are what's stable.
const KINDS: &[(&str, &[SyntaxKind])] = &[
    ("array", &[ARRAY_EXPR]),
    ("array_access", &[INDEX_EXPR]),
    ("anonymous_class", &[ANON_CLASS]),
    ("arrow_function", &[ARROW_FN_EXPR]),
    ("assign", &[ASSIGN_EXPR]),
    ("binary_op", &[BIN_EXPR]),
    ("break", &[BREAK_STMT]),
    ("cast", &[CAST_EXPR]),
    ("catch", &[CATCH_CLAUSE]),
    ("class", &[CLASS_DECL]),
    ("class_const", &[CLASS_CONST_DECL]),
    ("class_const_fetch", &[CLASS_CONST_FETCH_EXPR]),
    ("clone", &[CLONE_EXPR]),
    ("closure", &[CLOSURE_EXPR]),
    ("const", &[CONST_DECL]),
    ("const_fetch", &[NAME_EXPR]),
    ("continue", &[CONTINUE_STMT]),
    ("do_while", &[DO_WHILE_STMT]),
    ("echo", &[ECHO_STMT, ECHO_TAG_STMT]),
    ("empty", &[EMPTY_EXPR]),
    ("enum", &[ENUM_DECL]),
    ("error_suppress", &[ERROR_SUPPRESS_EXPR]),
    ("eval", &[EVAL_EXPR]),
    ("exit", &[EXIT_EXPR]),
    ("for", &[FOR_STMT]),
    ("foreach", &[FOREACH_STMT]),
    ("func_call", &[CALL_EXPR]),
    ("function", &[FUNCTION_DECL]),
    ("global", &[GLOBAL_STMT]),
    ("goto", &[GOTO_STMT]),
    ("if", &[IF_STMT]),
    ("include", &[INCLUDE_EXPR]),
    ("instanceof", &[INSTANCEOF_EXPR]),
    ("interface", &[INTERFACE_DECL]),
    ("isset", &[ISSET_EXPR]),
    (
        "literal",
        &[LITERAL, STRING_LITERAL, INTERP_STRING, HEREDOC],
    ),
    ("list", &[LIST_EXPR]),
    ("magic_const", &[MAGIC_CONST]),
    ("match", &[MATCH_EXPR]),
    ("method", &[METHOD_DECL]),
    (
        "method_call",
        &[METHOD_CALL_EXPR, NULLSAFE_METHOD_CALL_EXPR],
    ),
    ("name", &[NAME_REF]),
    ("namespace", &[NAMESPACE_DECL]),
    ("new", &[NEW_EXPR]),
    ("print", &[PRINT_EXPR]),
    ("property", &[PROPERTY_DECL]),
    (
        "property_fetch",
        &[PROPERTY_FETCH_EXPR, NULLSAFE_PROPERTY_FETCH_EXPR],
    ),
    ("return", &[RETURN_STMT]),
    ("shell_exec", &[SHELL_EXEC]),
    ("static_call", &[STATIC_CALL_EXPR]),
    ("static_property_fetch", &[STATIC_PROPERTY_FETCH_EXPR]),
    ("static_var", &[STATIC_VAR_STMT]),
    ("switch", &[SWITCH_STMT]),
    ("ternary", &[TERNARY_EXPR, SHORT_TERNARY_EXPR]),
    ("throw", &[THROW_EXPR]),
    ("trait", &[TRAIT_DECL]),
    ("try", &[TRY_STMT]),
    ("unary_op", &[PREFIX_EXPR, POSTFIX_EXPR]),
    ("unset", &[UNSET_STMT]),
    ("use", &[USE_DECL]),
    ("variable", &[VARIABLE_EXPR, VARIABLE_VARIABLE]),
    ("while", &[WHILE_STMT]),
    ("yield", &[YIELD_EXPR, YIELD_FROM_EXPR]),
];

static BY_SYNTAX: LazyLock<FxHashMap<SyntaxKind, &'static str>> = LazyLock::new(|| {
    KINDS
        .iter()
        .flat_map(|(name, kinds)| kinds.iter().map(move |k| (*k, *name)))
        .collect()
});

/// The syntax kinds a kind of node scripts can ask for may be, or `None` if there's no such kind.
pub(super) fn syntax_kinds(name: &str) -> Option<&'static [SyntaxKind]> {
    KINDS.iter().find(|(n, _)| *n == name).map(|(_, k)| *k)
}

/// All the kinds of node scripts can ask for.
pub(super) fn kind_names() -> impl Iterator<Item = &'static str> {
    KINDS.iter().map(|(name, _)| *name)
}

/// What `node` is to a script: nothing, if it's only a part of what's around it. `$a->b()` isn't
/// a fetch of `b`, `Foo::bar()` isn't a constant and neither is the `foo` of `foo()`.
pub(super) fn php_kind(node: &SyntaxNode) -> Option<&'static str> {
    let kind = BY_SYNTAX.get(&node.kind()).copied()?;
    let Some(parent) = node.parent() else {
        return Some(kind);
    };
    match node.kind() {
        PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR
            if matches!(parent.kind(), METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR)
                && is_first_child(&parent, node) =>
        {
            None
        }
        CLASS_CONST_FETCH_EXPR
            if parent.kind() == STATIC_CALL_EXPR && is_first_child(&parent, node) =>
        {
            None
        }
        // it names something, and the name is what's interesting
        NAME_EXPR
            if matches!(
                parent.kind(),
                CALL_EXPR
                    | NEW_EXPR
                    | STATIC_CALL_EXPR
                    | CLASS_CONST_FETCH_EXPR
                    | STATIC_PROPERTY_FETCH_EXPR
                    | INSTANCEOF_EXPR
            ) =>
        {
            None
        }
        _ => Some(kind),
    }
}

fn is_first_child(parent: &SyntaxNode, node: &SyntaxNode) -> bool {
    parent.first_child().as_ref() == Some(node)
}

/// A node of a PHP file, as a script has it: it can be kept across calls within one run, and goes
/// back to the tree for what it needs from it.
#[derive(Clone)]
pub struct Node {
    green: stir_syntax::GreenNode,
    file: SourceFile,
    syntax_kind: SyntaxKind,
    kind: &'static str,
    range: TextRange,
    name: Option<ImmutableString>,
}

impl Node {
    /// `node` of `file`, whose tree is `green`. `None` if scripts don't see such a node.
    pub(super) fn new(
        green: &stir_syntax::GreenNode,
        file: SourceFile,
        node: &SyntaxNode,
    ) -> Option<Node> {
        Some(Node {
            green: green.clone(),
            file,
            syntax_kind: node.kind(),
            kind: php_kind(node)?,
            range: node.text_range(),
            name: name_of(node).map(ImmutableString::from),
        })
    }

    pub(super) fn range(&self) -> TextRange {
        self.range
    }

    /// Back in the tree.
    fn syntax(&self) -> Option<SyntaxNode> {
        let root = SyntaxNode::new_root(self.green.clone());
        let element = root.covering_element(self.range);
        let start = match element {
            NodeOrToken::Node(n) => n,
            NodeOrToken::Token(t) => t.parent()?,
        };
        start
            .ancestors()
            .find(|n| n.kind() == self.syntax_kind && n.text_range() == self.range)
    }

    fn wrap(&self, node: &SyntaxNode) -> Option<Node> {
        Node::new(&self.green, self.file, node)
    }

    /// The nearest nodes below this one that scripts see.
    fn children(&self) -> Array {
        let Some(node) = self.syntax() else {
            return Array::new();
        };
        let mut out = Array::new();
        self.collect_children(&node, &mut out);
        out
    }

    fn collect_children(&self, node: &SyntaxNode, out: &mut Array) {
        for child in node.children() {
            match self.wrap(&child) {
                Some(n) => out.push(Dynamic::from(n)),
                None => self.collect_children(&child, out),
            }
        }
    }

    /// The expressions passed to a call, in order.
    fn arguments(&self) -> Array {
        let Some(node) = self.syntax() else {
            return Array::new();
        };
        let Some(list) = node.children().find(|c| c.kind() == ARG_LIST) else {
            return Array::new();
        };
        list.children()
            .filter(|c| matches!(c.kind(), ARG | NAMED_ARG))
            .filter_map(|arg| arg.last_child())
            .filter_map(|expr| self.wrap(&without_parens(expr)))
            .map(Dynamic::from)
            .collect()
    }

    /// What a member is looked up on: `$a` of `$a->b()`, `Foo` of `Foo::bar()`.
    fn receiver(&self) -> Option<Node> {
        let node = self.syntax()?;
        let base = without_parens(receiver_of(&node)?);
        self.wrap(&base).or_else(|| {
            // the class of `Foo::bar()` is only a name
            let name = base.children().find(|c| c.kind() == NAME_REF)?;
            self.wrap(&name).map(|mut n| {
                n.range = base.text_range();
                n.syntax_kind = base.kind();
                n
            })
        })
    }

    fn line_and_column(&self) -> (INT, INT) {
        let offset = usize::from(self.range.start());
        let at = stir_db::attached(|db| {
            let text = self.file.text(db);
            let before = &text.as_bytes()[..offset.min(text.len())];
            let line = before.iter().filter(|b| **b == b'\n').count();
            let start = before
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |i| i + 1);
            (line, offset.saturating_sub(start))
        });
        let (line, column) = at.unwrap_or_default();
        (line as INT + 1, column as INT + 1)
    }
}

/// What's in the parentheses, which scripts don't see: they only group.
pub(super) fn without_parens(mut node: SyntaxNode) -> SyntaxNode {
    while node.kind() == PAREN_EXPR {
        let Some(inner) = node.first_child() else {
            break;
        };
        node = inner;
    }
    node
}

/// The expression that a member access in `node` is on.
pub(super) fn receiver_of(node: &SyntaxNode) -> Option<SyntaxNode> {
    match node.kind() {
        METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR => {
            node.first_child()?.first_child()
        }
        PROPERTY_FETCH_EXPR
        | NULLSAFE_PROPERTY_FETCH_EXPR
        | STATIC_PROPERTY_FETCH_EXPR
        | CLASS_CONST_FETCH_EXPR => node.first_child(),
        _ => None,
    }
}

/// What a node is named, the way it's written: a function called, a method, a class. No `$` on
/// a variable, no leading `\` on a name.
fn name_of(node: &SyntaxNode) -> Option<String> {
    let child_name = |n: &SyntaxNode| {
        n.children()
            .find(|c| matches!(c.kind(), NAME | NAME_REF))
            .map(|c| text_of(&c))
    };
    let written = match node.kind() {
        CALL_EXPR | NEW_EXPR => {
            let callee = node.children().find(|c| c.kind() == NAME_EXPR)?;
            Some(text_of(&callee))
        }
        METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR => {
            child_name(&node.first_child()?)
        }
        STATIC_PROPERTY_FETCH_EXPR => node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == VARIABLE)
            .map(|t| t.text().to_owned()),
        VARIABLE_EXPR => Some(text_of(node)),
        PROPERTY_DECL => node
            .children()
            .find(|c| c.kind() == PROPERTY_ELEM)
            .and_then(|e| {
                e.children_with_tokens()
                    .filter_map(|x| x.into_token())
                    .find(|t| t.kind() == VARIABLE)
                    .map(|t| t.text().to_owned())
            }),
        CLASS_CONST_DECL | CONST_DECL => node
            .children()
            .find(|c| c.kind() == CONST_ELEM)
            .and_then(|e| child_name(&e)),
        NAME_EXPR | NAME_REF => Some(text_of(node)),
        _ => child_name(node),
    }?;
    Some(
        written
            .trim_start_matches('\\')
            .trim_start_matches('$')
            .to_owned(),
    )
}

fn text_of(node: &SyntaxNode) -> String {
    node.text().to_string().trim().to_owned()
}

fn optional<T: Clone + Send + Sync + 'static>(value: Option<T>) -> Dynamic {
    value.map_or(Dynamic::UNIT, Dynamic::from)
}

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_type_with_name::<Node>("Node")
        .register_get("kind", |n: &mut Node| n.kind)
        .register_get("name", |n: &mut Node| optional(n.name.clone()))
        .register_get("text", |n: &mut Node| {
            n.syntax().map_or_else(String::new, |s| text_of(&s))
        })
        .register_get("range", |n: &mut Node| {
            let (line, column) = n.line_and_column();
            let mut range = Map::new();
            range.insert("start".into(), (u32::from(n.range.start()) as INT).into());
            range.insert("end".into(), (u32::from(n.range.end()) as INT).into());
            range.insert("line".into(), line.into());
            range.insert("column".into(), column.into());
            range
        })
        .register_get("arguments", |n: &mut Node| n.arguments())
        .register_get("children", |n: &mut Node| n.children())
        .register_get("receiver", |n: &mut Node| optional(n.receiver()))
        .register_fn("to_string", |n: &mut Node| {
            n.syntax().map_or_else(String::new, |s| text_of(&s))
        });
}
