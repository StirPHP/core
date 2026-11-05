//! The single kind enum shared by the lexer, the parser and the rowan tree.

/// Every token and node kind in a PHP syntax tree.
///
/// Tokens come first, then nodes. The `#[repr(u16)]` lets the kind round-trip
/// through `rowan::SyntaxKind`.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, strum::FromRepr)]
#[repr(u16)]
pub enum SyntaxKind {
    // ---- trivia ----
    WHITESPACE = 0,
    COMMENT,
    DOC_COMMENT,

    // ---- file structure ----
    INLINE_HTML,
    OPEN_TAG,
    OPEN_TAG_WITH_ECHO,
    CLOSE_TAG,
    /// Raw data after `__halt_compiler();`.
    HALT_DATA,
    ERROR_TOKEN,

    // ---- literals and names ----
    INT_NUMBER,
    FLOAT_NUMBER,
    /// `$name`
    VARIABLE,
    /// A lone `$` (variable variables).
    DOLLAR,
    IDENT,
    /// `Foo\Bar`
    NAME_QUALIFIED,
    /// `\Foo\Bar`
    NAME_FULLY_QUALIFIED,
    /// `namespace\Foo`
    NAME_RELATIVE,
    /// A complete `'...'` string.
    STRING_SINGLE,
    /// A `"..."` string without any interpolation.
    STRING_DOUBLE,
    /// Opening or closing `"` of an interpolated string.
    DQUOTE,
    /// Opening or closing `` ` `` of a shell-exec string.
    BACKTICK,
    /// Literal text inside an interpolated string or heredoc.
    STRING_CONTENT,
    /// `<<<LABEL\n`, `<<<"LABEL"\n` or `<<<'LABEL'\n` (nowdoc).
    HEREDOC_START,
    /// The closing label of a heredoc, including leading indentation.
    HEREDOC_END,
    /// `{` directly followed by `$` inside a string.
    CURLY_OPEN,
    /// `${` inside a string.
    DOLLAR_CURLY_OPEN,

    // ---- casts ----
    INT_CAST,
    FLOAT_CAST,
    STRING_CAST,
    BOOL_CAST,
    ARRAY_CAST,
    OBJECT_CAST,
    UNSET_CAST,
    /// PHP 8.5 `(void)`
    VOID_CAST,

    // ---- punctuation ----
    SEMI,
    COMMA,
    L_PAREN,
    R_PAREN,
    L_BRACK,
    R_BRACK,
    L_CURLY,
    R_CURLY,
    COLON,
    COLON2,
    QUESTION,
    QUESTION2,
    QUESTION2_EQ,
    NULLSAFE_ARROW,
    ARROW,
    FAT_ARROW,
    ELLIPSIS,
    DOT,
    DOT_EQ,
    EQ,
    EQ2,
    EQ3,
    NEQ,
    NEQ2,
    L_ANGLE,
    R_ANGLE,
    GTEQ,
    SHR,
    SHR_EQ,
    LTEQ,
    SPACESHIP,
    PLUS,
    MINUS,
    STAR,
    SLASH,
    PERCENT,
    POW,
    PLUS_EQ,
    MINUS_EQ,
    STAR_EQ,
    SLASH_EQ,
    PERCENT_EQ,
    POW_EQ,
    INC,
    DEC,
    AMP,
    PIPE,
    CARET,
    TILDE,
    SHL,
    AMP_EQ,
    PIPE_EQ,
    CARET_EQ,
    SHL_EQ,
    AMP2,
    PIPE2,
    /// PHP 8.5 pipe `|>`
    PIPE_GT,
    BANG,
    AT,
    /// `#[`
    ATTR_OPEN,
    BACKSLASH,

    // ---- keywords (PHP keywords are case-insensitive) ----
    ABSTRACT_KW,
    AND_KW,
    ARRAY_KW,
    AS_KW,
    BREAK_KW,
    CALLABLE_KW,
    CASE_KW,
    CATCH_KW,
    CLASS_KW,
    CLONE_KW,
    CONST_KW,
    CONTINUE_KW,
    DECLARE_KW,
    DEFAULT_KW,
    DO_KW,
    ECHO_KW,
    ELSE_KW,
    ELSEIF_KW,
    EMPTY_KW,
    ENDDECLARE_KW,
    ENDFOR_KW,
    ENDFOREACH_KW,
    ENDIF_KW,
    ENDSWITCH_KW,
    ENDWHILE_KW,
    EVAL_KW,
    EXIT_KW,
    EXTENDS_KW,
    FINAL_KW,
    FINALLY_KW,
    FN_KW,
    FOR_KW,
    FOREACH_KW,
    FUNCTION_KW,
    GLOBAL_KW,
    GOTO_KW,
    HALT_COMPILER_KW,
    IF_KW,
    IMPLEMENTS_KW,
    INCLUDE_KW,
    INCLUDE_ONCE_KW,
    INSTANCEOF_KW,
    INSTEADOF_KW,
    INTERFACE_KW,
    ISSET_KW,
    LIST_KW,
    MATCH_KW,
    NAMESPACE_KW,
    NEW_KW,
    OR_KW,
    PRINT_KW,
    PRIVATE_KW,
    PROTECTED_KW,
    PUBLIC_KW,
    READONLY_KW,
    REQUIRE_KW,
    REQUIRE_ONCE_KW,
    RETURN_KW,
    STATIC_KW,
    SWITCH_KW,
    THROW_KW,
    TRAIT_KW,
    TRY_KW,
    UNSET_KW,
    USE_KW,
    VAR_KW,
    WHILE_KW,
    XOR_KW,
    YIELD_KW,
    // magic constants
    CLASS_C_KW,
    DIR_KW,
    FILE_KW,
    FUNCTION_C_KW,
    LINE_KW,
    METHOD_C_KW,
    NAMESPACE_C_KW,
    TRAIT_C_KW,
    PROPERTY_C_KW,

    EOF,

    // ---- nodes ----
    SOURCE_FILE,
    ERROR,

    // names
    NAME,
    NAME_REF,

    // top-level & statements
    INLINE_HTML_STMT,
    ECHO_TAG_STMT,
    NAMESPACE_DECL,
    USE_DECL,
    USE_CLAUSE,
    USE_GROUP,
    CONST_DECL,
    CONST_ELEM,
    DECLARE_STMT,
    DECLARE_ITEM,
    FUNCTION_DECL,
    CLASS_DECL,
    INTERFACE_DECL,
    TRAIT_DECL,
    ENUM_DECL,
    BLOCK,
    EXPR_STMT,
    ECHO_STMT,
    RETURN_STMT,
    IF_STMT,
    ELSEIF_CLAUSE,
    ELSE_CLAUSE,
    WHILE_STMT,
    DO_WHILE_STMT,
    FOR_STMT,
    FOR_INIT,
    FOR_COND,
    FOR_STEP,
    FOREACH_STMT,
    SWITCH_STMT,
    SWITCH_CASE,
    BREAK_STMT,
    CONTINUE_STMT,
    TRY_STMT,
    CATCH_CLAUSE,
    FINALLY_CLAUSE,
    GLOBAL_STMT,
    STATIC_VAR_STMT,
    STATIC_VAR,
    UNSET_STMT,
    GOTO_STMT,
    LABEL_STMT,
    HALT_COMPILER_STMT,
    EMPTY_STMT,

    // declarations: parts
    MODIFIERS,
    ATTRIBUTE_LIST,
    ATTRIBUTE_GROUP,
    ATTRIBUTE,
    PARAM_LIST,
    PARAM,
    RETURN_TYPE,
    EXTENDS_CLAUSE,
    IMPLEMENTS_CLAUSE,
    CLASS_BODY,
    PROPERTY_DECL,
    PROPERTY_ELEM,
    PROPERTY_HOOKS,
    PROPERTY_HOOK,
    CLASS_CONST_DECL,
    METHOD_DECL,
    TRAIT_USE,
    TRAIT_ADAPTATIONS,
    TRAIT_ALIAS,
    TRAIT_PRECEDENCE,
    ENUM_CASE,
    CLOSURE_USE,
    CLOSURE_USE_VAR,

    // types
    NAMED_TYPE,
    NULLABLE_TYPE,
    UNION_TYPE,
    INTERSECTION_TYPE,
    PAREN_TYPE,

    // expressions
    LITERAL,
    STRING_LITERAL,
    INTERP_STRING,
    HEREDOC,
    SHELL_EXEC,
    INTERP_VAR,
    INTERP_EXPR,
    VARIABLE_EXPR,
    VARIABLE_VARIABLE,
    NAME_EXPR,
    ARRAY_EXPR,
    ARRAY_ELEM,
    LIST_EXPR,
    PAREN_EXPR,
    BIN_EXPR,
    PREFIX_EXPR,
    POSTFIX_EXPR,
    ASSIGN_EXPR,
    TERNARY_EXPR,
    /// `a ?: b`
    SHORT_TERNARY_EXPR,
    CAST_EXPR,
    INSTANCEOF_EXPR,
    CALL_EXPR,
    ARG_LIST,
    ARG,
    NAMED_ARG,
    METHOD_CALL_EXPR,
    NULLSAFE_METHOD_CALL_EXPR,
    PROPERTY_FETCH_EXPR,
    NULLSAFE_PROPERTY_FETCH_EXPR,
    STATIC_CALL_EXPR,
    STATIC_PROPERTY_FETCH_EXPR,
    CLASS_CONST_FETCH_EXPR,
    INDEX_EXPR,
    NEW_EXPR,
    ANON_CLASS,
    CLONE_EXPR,
    CLOSURE_EXPR,
    ARROW_FN_EXPR,
    MATCH_EXPR,
    MATCH_ARM,
    MATCH_CONDITIONS,
    ISSET_EXPR,
    EMPTY_EXPR,
    EVAL_EXPR,
    EXIT_EXPR,
    PRINT_EXPR,
    INCLUDE_EXPR,
    YIELD_EXPR,
    YIELD_FROM_EXPR,
    THROW_EXPR,
    ERROR_SUPPRESS_EXPR,
    MAGIC_CONST,
    /// First-class callable syntax `foo(...)`.
    CALLABLE_PLACEHOLDER,

    #[doc(hidden)]
    __LAST,
}

use SyntaxKind::*;

impl SyntaxKind {
    pub fn is_trivia(self) -> bool {
        matches!(self, WHITESPACE | COMMENT | DOC_COMMENT)
    }

    pub fn is_keyword(self) -> bool {
        (ABSTRACT_KW as u16..=PROPERTY_C_KW as u16).contains(&(self as u16))
    }

    pub fn is_token(self) -> bool {
        (self as u16) < (EOF as u16)
    }

    /// Maps a PHP keyword (case-insensitive) to its kind.
    pub fn from_keyword(ident: &str) -> Option<SyntaxKind> {
        let lower = ident.to_ascii_lowercase();
        let kind = match lower.as_str() {
            "abstract" => ABSTRACT_KW,
            "and" => AND_KW,
            "array" => ARRAY_KW,
            "as" => AS_KW,
            "break" => BREAK_KW,
            "callable" => CALLABLE_KW,
            "case" => CASE_KW,
            "catch" => CATCH_KW,
            "class" => CLASS_KW,
            "clone" => CLONE_KW,
            "const" => CONST_KW,
            "continue" => CONTINUE_KW,
            "declare" => DECLARE_KW,
            "default" => DEFAULT_KW,
            "do" => DO_KW,
            "echo" => ECHO_KW,
            "else" => ELSE_KW,
            "elseif" => ELSEIF_KW,
            "empty" => EMPTY_KW,
            "enddeclare" => ENDDECLARE_KW,
            "endfor" => ENDFOR_KW,
            "endforeach" => ENDFOREACH_KW,
            "endif" => ENDIF_KW,
            "endswitch" => ENDSWITCH_KW,
            "endwhile" => ENDWHILE_KW,
            "eval" => EVAL_KW,
            "exit" | "die" => EXIT_KW,
            "extends" => EXTENDS_KW,
            "final" => FINAL_KW,
            "finally" => FINALLY_KW,
            "fn" => FN_KW,
            "for" => FOR_KW,
            "foreach" => FOREACH_KW,
            "function" => FUNCTION_KW,
            "global" => GLOBAL_KW,
            "goto" => GOTO_KW,
            "__halt_compiler" => HALT_COMPILER_KW,
            "if" => IF_KW,
            "implements" => IMPLEMENTS_KW,
            "include" => INCLUDE_KW,
            "include_once" => INCLUDE_ONCE_KW,
            "instanceof" => INSTANCEOF_KW,
            "insteadof" => INSTEADOF_KW,
            "interface" => INTERFACE_KW,
            "isset" => ISSET_KW,
            "list" => LIST_KW,
            "match" => MATCH_KW,
            "namespace" => NAMESPACE_KW,
            "new" => NEW_KW,
            "or" => OR_KW,
            "print" => PRINT_KW,
            "private" => PRIVATE_KW,
            "protected" => PROTECTED_KW,
            "public" => PUBLIC_KW,
            "readonly" => READONLY_KW,
            "require" => REQUIRE_KW,
            "require_once" => REQUIRE_ONCE_KW,
            "return" => RETURN_KW,
            "static" => STATIC_KW,
            "switch" => SWITCH_KW,
            "throw" => THROW_KW,
            "trait" => TRAIT_KW,
            "try" => TRY_KW,
            "unset" => UNSET_KW,
            "use" => USE_KW,
            "var" => VAR_KW,
            "while" => WHILE_KW,
            "xor" => XOR_KW,
            "yield" => YIELD_KW,
            "__class__" => CLASS_C_KW,
            "__dir__" => DIR_KW,
            "__file__" => FILE_KW,
            "__function__" => FUNCTION_C_KW,
            "__line__" => LINE_KW,
            "__method__" => METHOD_C_KW,
            "__namespace__" => NAMESPACE_C_KW,
            "__trait__" => TRAIT_C_KW,
            "__property__" => PROPERTY_C_KW,
            _ => return None,
        };
        Some(kind)
    }
}

impl From<SyntaxKind> for rowan::SyntaxKind {
    fn from(kind: SyntaxKind) -> Self {
        rowan::SyntaxKind(kind as u16)
    }
}

/// The rowan language tag for PHP trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PhpLanguage {}

impl rowan::Language for PhpLanguage {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        SyntaxKind::from_repr(raw.0).expect("invalid raw SyntaxKind")
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        kind.into()
    }
}

pub type SyntaxNode = rowan::SyntaxNode<PhpLanguage>;
pub type SyntaxToken = rowan::SyntaxToken<PhpLanguage>;
pub type SyntaxElement = rowan::SyntaxElement<PhpLanguage>;
