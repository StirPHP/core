//! The sandbox project scripts run in: a locked down Rhai engine and the limits it enforces.
//!
//! A script in a repository may run as soon as someone opens it, so it gets nothing it could harm
//! anything with, or depend on anything with. There is no file, network or process access, no
//! `import`, no `eval`, no clock, no randomness, and no state kept between calls: each call gets a
//! fresh scope, and only functions run, never a script's top level. What it can't be stopped from
//! doing is taking long or asking for a lot, and that's capped: exceeding a cap fails the call.
//!
//! What a script can see of a project is registered on top of [`sandbox`] by the checker.

use std::sync::Arc;

use rhai::{AST, CallFnOptions, Dynamic, Engine, EvalAltResult, ParseError, Position, Scope};

pub use rhai;

/// How many operations one call may take. Keeps a loop that never ends to a fraction of a second.
pub const MAX_OPERATIONS: u64 = 1_000_000;
/// How deep calls may go in a script: enough to walk any PHP tree recursively.
pub const MAX_CALL_LEVELS: usize = if cfg!(target_family = "wasm") { 16 } else { 64 };
/// How deep expressions and blocks may nest, at a script's top level and in its functions.
pub const MAX_EXPR_DEPTH: usize = 64;
pub const MAX_FN_EXPR_DEPTH: usize = 32;
/// The largest string, in bytes.
pub const MAX_STRING_SIZE: usize = 4 * 1024 * 1024;
/// The most elements of an array, or of a value made of them.
pub const MAX_ARRAY_SIZE: usize = 64 * 1024;
/// The most properties of a map.
pub const MAX_MAP_SIZE: usize = 16 * 1024;
/// The most variables one call may have.
pub const MAX_VARIABLES: usize = 1024;
/// The most functions a script may declare.
pub const MAX_FUNCTIONS: usize = 1024;
/// The largest script, in bytes.
pub const MAX_SCRIPT_SIZE: usize = 1024 * 1024;

/// The stack a script is sure of when it starts, whatever is below it: its deepest calls with its
/// deepest expressions take less than this. Debug builds use far more per call, but they don't get
/// lower limits, because what a script answers mustn't depend on the build.
#[cfg(not(target_family = "wasm"))]
const SCRIPT_STACK: usize = if cfg!(debug_assertions) {
    24 * 1024 * 1024
} else {
    4 * 1024 * 1024
};

/// Why a script can't be used, or a call of it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    pub message: String,
    /// 1-based, if the error has a place in the script.
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl ScriptError {
    pub fn new(message: impl Into<String>) -> Self {
        ScriptError {
            message: message.into(),
            line: None,
            column: None,
        }
    }

    fn at(message: impl Into<String>, position: Position) -> Self {
        ScriptError {
            message: message.into(),
            line: position.line().map(|n| n as u32),
            column: position.position().map(|n| n as u32),
        }
    }

    /// The byte offset of the error in `text`, the script.
    pub fn offset_in(&self, text: &str) -> u32 {
        let Some(line) = self.line else { return 0 };
        let start = text
            .split_inclusive('\n')
            .take(line.saturating_sub(1) as usize)
            .map(str::len)
            .sum::<usize>();
        let column = self.column.unwrap_or(1).saturating_sub(1) as usize;
        let rest = text.get(start..).unwrap_or("");
        let within: usize = rest.chars().take(column).map(char::len_utf8).sum();
        u32::try_from(start + within).unwrap_or(0)
    }
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

fn refused(what: &'static str) -> impl Fn() -> Box<EvalAltResult> + Clone {
    move || format!("`{what}` isn't available to scripts").into()
}

/// A new engine with nothing but the language and its pure library, and every cap set.
pub fn sandbox() -> Engine {
    let mut engine = Engine::new();
    engine
        .set_max_operations(MAX_OPERATIONS)
        .set_max_call_levels(MAX_CALL_LEVELS)
        .set_max_expr_depths(MAX_EXPR_DEPTH, MAX_FN_EXPR_DEPTH)
        .set_max_string_size(MAX_STRING_SIZE)
        .set_max_array_size(MAX_ARRAY_SIZE)
        .set_max_map_size(MAX_MAP_SIZE)
        .set_max_variables(MAX_VARIABLES)
        .set_max_functions(MAX_FUNCTIONS)
        .set_strict_variables(true);
    // what scripts print is for whoever debugs them
    engine.on_print(|text| tracing::debug!(target: "stirphp::script", "{text}"));
    engine.on_debug(|text, source, position| {
        tracing::debug!(target: "stirphp::script", "{}:{position}: {text}", source.unwrap_or(""));
    });
    engine.disable_symbol("eval");
    // These are in the library, and `Fn("sleep").call(1)` doesn't go past the parser. What's
    // registered on the engine is found first.
    let sleep = refused("sleep");
    let again = sleep.clone();
    engine.register_fn("sleep", move |_: i64| -> Result<(), _> { Err(sleep()) });
    engine.register_fn("sleep", move |_: f64| -> Result<(), _> { Err(again()) });
    let exit = refused("exit");
    let again = exit.clone();
    engine.register_fn("exit", move || -> Result<(), _> { Err(exit()) });
    engine.register_fn("exit", move |_: Dynamic| -> Result<(), _> { Err(again()) });
    engine
}

/// Compiles a script. Only functions ever run, so a top level with anything else in it is a
/// mistake that's better told than left to do nothing.
pub fn compile(engine: &Engine, name: &str, text: &str) -> Result<Arc<AST>, ScriptError> {
    if text.len() > MAX_SCRIPT_SIZE {
        return Err(ScriptError::new(format!(
            "it's larger than the limit of {MAX_SCRIPT_SIZE} bytes"
        )));
    }
    let mut ast = with_stack(|| engine.compile(text)).map_err(from_parse)?;
    if let Some(first) = ast.statements().first() {
        return Err(ScriptError::at(
            "only functions run: move what's at the top level into a function",
            first.position(),
        ));
    }
    ast.set_source(name);
    Ok(Arc::new(ast))
}

fn from_parse(error: ParseError) -> ScriptError {
    let ParseError(kind, position) = error;
    ScriptError::at(format!("syntax error: {kind}"), position)
}

/// Calls a function of a compiled script: a fresh scope, and only what `args` holds to go on.
pub fn call(
    engine: &Engine,
    ast: &AST,
    function: &str,
    args: Vec<Dynamic>,
) -> Result<Dynamic, ScriptError> {
    let options = CallFnOptions::new().eval_ast(false).rewind_scope(false);
    let mut scope = Scope::new();
    with_stack(|| engine.call_fn_with_options(options, &mut scope, ast, function, args))
        .map_err(|e| from_eval(&e))
}

fn from_eval(error: &EvalAltResult) -> ScriptError {
    let inner = innermost(error);
    // past the limit of calls, the last one has no place of its own
    let position = Some(inner.position())
        .filter(|p| !p.is_none())
        .unwrap_or_else(|| error.position());
    let message = match inner {
        EvalAltResult::ErrorTooManyOperations(_) => {
            format!("it took more than the limit of {MAX_OPERATIONS} operations")
        }
        EvalAltResult::ErrorStackOverflow(_) => {
            format!("it called too deep, past the limit of {MAX_CALL_LEVELS} calls")
        }
        EvalAltResult::ErrorDataTooLarge(what, _) => {
            format!("it made a value too large: {what}")
        }
        EvalAltResult::ErrorTooManyVariables(_) => {
            format!("it used more than the limit of {MAX_VARIABLES} variables")
        }
        other => {
            // the place is said separately
            let text = other.to_string();
            match text.rfind(" (line ") {
                Some(end) if text.ends_with(')') => text[..end].to_owned(),
                _ => text,
            }
        }
    };
    ScriptError::at(message, position)
}

/// The error behind the calls it came through.
fn innermost(error: &EvalAltResult) -> &EvalAltResult {
    match error {
        EvalAltResult::ErrorInFunctionCall(_, _, inner, _) => innermost(inner),
        other => other,
    }
}

/// Runs `f` with the stack a script needs, on a new one if this one hasn't got that much left. The
/// same thread, so that what's attached to it stays attached. A new one has room for many scripts
/// on top of what `f` itself uses, so around a run of many calls, this makes it once.
#[cfg(not(target_family = "wasm"))]
pub fn with_stack<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(SCRIPT_STACK, SCRIPT_STACK * 3, f)
}

/// A browser's stack can't be grown; [`MAX_CALL_LEVELS`] is lower there instead.
#[cfg(target_family = "wasm")]
pub fn with_stack<R>(f: impl FnOnce() -> R) -> R {
    f()
}

#[cfg(test)]
mod tests;
