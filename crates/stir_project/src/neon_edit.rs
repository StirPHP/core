//! Adding to a NEON file by its lines, so that comments and everything else in it stay as written.

use std::path::Path;

use stir_neon::{Key, Value};

use crate::error::{Error, Result};

/// `text` with `key: value` in `parameters.stirphp`, and with what PHPStan needs to accept that
/// section if `for_phpstan`. An existing `key` stays as it is. `value` is NEON already.
pub fn with_stirphp_setting(
    path: &Path,
    text: &str,
    key: &str,
    value: &str,
    for_phpstan: bool,
) -> Result<String> {
    let cant = |reason: String| Error::CantEditConfig {
        path: path.to_path_buf(),
        reason,
    };
    let before = stir_neon::parse(text).map_err(|e| cant(e.to_string()))?;
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let unit = indent_unit(&lines);

    let section = |lines: &[String], name: &str, indent: &str| -> Result<Option<usize>> {
        for (i, line) in lines.iter().enumerate() {
            let Some(rest) = line.strip_prefix(indent) else {
                continue;
            };
            let Some(rest) = rest.strip_prefix(name) else {
                continue;
            };
            let Some(rest) = rest.strip_prefix(':') else {
                continue;
            };
            let rest = rest.trim_start();
            if rest.is_empty() || rest.starts_with('#') {
                return Ok(Some(i));
            }
            return Err(cant(format!(
                "`{name}` is written on one line, which needs editing by hand"
            )));
        }
        Ok(None)
    };

    match section(&lines, "parameters", "")? {
        Some(parameters) => match section(&lines, "stirphp", &unit)? {
            Some(own) => {
                let set = before
                    .get("parameters")
                    .and_then(|p| p.get("stirphp"))
                    .and_then(|s| s.get(key))
                    .is_some();
                if !set {
                    let at = block_end(&lines, own, indent_of(&lines[own]));
                    lines.insert(at, format!("{unit}{unit}{key}: {value}"));
                }
            }
            None => {
                let at = block_end(&lines, parameters, 0);
                lines.insert(at, format!("{unit}stirphp:"));
                lines.insert(at + 1, format!("{unit}{unit}{key}: {value}"));
            }
        },
        None => {
            lines.push("parameters:".to_owned());
            lines.push(format!("{unit}stirphp:"));
            lines.push(format!("{unit}{unit}{key}: {value}"));
        }
    }
    if for_phpstan {
        let declared = before
            .get("parametersSchema")
            .is_some_and(|s| s.get("?stirphp").is_some() || s.get("stirphp").is_some());
        if !declared {
            let declaration = format!("{unit}?stirphp: mixed()");
            match section(&lines, "parametersSchema", "")? {
                Some(schema) => {
                    let at = block_end(&lines, schema, 0);
                    lines.insert(at, declaration);
                }
                None => {
                    lines.push("parametersSchema:".to_owned());
                    lines.push(declaration);
                }
            }
        }
    }

    let mut out = lines.join(eol);
    out.push_str(eol);
    let after = stir_neon::parse(&out).map_err(|e| cant(e.to_string()))?;
    if without_ours(after) != without_ours(before) {
        return Err(cant("it's laid out in a way this can't follow".to_owned()));
    }
    Ok(out)
}

/// `value` as it was before `parameters.stirphp` and its schema.
fn without_ours(mut value: Value) -> Value {
    if let Value::Mapping(top) = &mut value {
        for (section, name) in [("parameters", "stirphp"), ("parametersSchema", "?stirphp")] {
            let key = Key::from(section);
            if let Some(Value::Mapping(entries)) = top.get_mut(&key) {
                entries.shift_remove(&Key::from(name));
            }
            let empty = match top.get(&key) {
                Some(Value::Mapping(entries)) => entries.is_empty(),
                Some(Value::Sequence(items)) => items.is_empty(),
                Some(Value::Null) => true,
                _ => false,
            };
            if empty {
                top.shift_remove(&key);
            }
        }
        if top.is_empty() {
            return Value::Null;
        }
    }
    value
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// The indent the file uses: tabs or spaces, as its first indented line does.
fn indent_unit(lines: &[String]) -> String {
    lines
        .iter()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| &l[..indent_of(l)])
        .find(|indent| !indent.is_empty())
        .map_or_else(|| "\t".to_owned(), str::to_owned)
}

/// The line after the last one in the block under `start`, which is at `indent`.
fn block_end(lines: &[String], start: usize, indent: usize) -> usize {
    let mut end = start + 1;
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if indent_of(line) <= indent {
            break;
        }
        end = i + 1;
    }
    end
}
