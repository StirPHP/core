//! What scripts can ask about a Symfony project's compiled container (see `DataFile`): the class
//! of a service by its id (`symfony_service`), and the value of a parameter (`symfony_parameter`).
//! The container is only read, as phpstan-symfony reads it: it's whatever was compiled last.

use std::collections::BTreeMap;

use stir_db::{Db, Workspace};
use stir_script::rhai::{Array, Dynamic, Engine, INT, Map};

/// A parameter's value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    /// `type="collection"`: by key, in order.
    Collection(Vec<(Option<String>, Value)>),
}

impl Eq for Value {}

/// What the compiled container says.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Container {
    /// By id: the class, or for an alias, the id it stands for.
    services: BTreeMap<String, Service>,
    parameters: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Service {
    Class(String),
    Alias(String),
}

impl Container {
    /// The class of the service `id`, through any aliases.
    pub fn class_of(&self, id: &str) -> Option<&str> {
        let mut id = id;
        for _ in 0..16 {
            match self.services.get(id)? {
                Service::Class(class) => return Some(class),
                Service::Alias(to) => id = to,
            }
        }
        None
    }
}

/// The container of the workspace's first data file that's one.
#[salsa::tracked(returns(ref))]
pub fn symfony_container(db: &dyn Db, ws: Workspace) -> Container {
    ws.data_files(db)
        .iter()
        .find(|f| f.path(db).extension().is_some_and(|e| e == "xml"))
        .map(|f| parse(f.text(db)))
        .unwrap_or_default()
}

/// A tag of the XML: its name, attributes, and whether it closes itself, or a closing tag.
struct Tag<'a> {
    name: &'a str,
    closing: bool,
    empty: bool,
    attributes: Vec<(&'a str, String)>,
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The tags of `text`, each with the text that follows it up to the next tag.
fn tags(text: &str) -> Vec<(Tag<'_>, &str)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        // comments, declarations and `<?xml ..?>`
        if let Some(after) = rest.strip_prefix("!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        if rest.starts_with('?') || rest.starts_with('!') {
            rest = rest.find('>').map_or("", |end| &rest[end + 1..]);
            continue;
        }
        let Some(end) = rest.find('>') else { break };
        let inside = &rest[..end];
        rest = &rest[end + 1..];
        let closing = inside.starts_with('/');
        let empty = inside.ends_with('/');
        let inside = inside.trim_start_matches('/').trim_end_matches('/');
        let name_end = inside
            .find(|c: char| c.is_whitespace())
            .unwrap_or(inside.len());
        let name = &inside[..name_end];
        let mut attributes = Vec::new();
        let mut attrs = &inside[name_end..];
        while let Some(eq) = attrs.find('=') {
            let key = attrs[..eq].trim();
            let after = attrs[eq + 1..].trim_start();
            let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                break;
            };
            let Some(close) = after[1..].find(quote) else {
                break;
            };
            attributes.push((key, unescape(&after[1..1 + close])));
            attrs = &after[close + 2..];
        }
        let content = rest.find('<').map_or(rest, |next| &rest[..next]);
        out.push((
            Tag {
                name,
                closing,
                empty,
                attributes,
            },
            content,
        ));
    }
    out
}

fn attribute<'a>(tag: &'a Tag<'_>, key: &str) -> Option<&'a str> {
    tag.attributes
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.as_str())
}

/// A value of `type`, written as `text`.
fn scalar(kind: Option<&str>, text: &str) -> Value {
    let text = unescape(text.trim());
    match kind {
        Some("string") => return Value::String(text),
        Some("constant" | "binary") => return Value::Null,
        _ => {}
    }
    match text.as_str() {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" => Value::Null,
        _ => {
            if let Ok(i) = text.parse::<i64>() {
                Value::Int(i)
            } else if let Ok(f) = text.parse::<f64>()
                && text.contains('.')
            {
                Value::Float(f)
            } else {
                Value::String(text)
            }
        }
    }
}

/// A collection's items, by key where they have one.
type Items = Vec<(Option<String>, Value)>;

/// What Symfony's `XmlDumper` writes, read back: services under `<services>`, parameters under
/// `<parameters>`, collections nested.
fn parse(text: &str) -> Container {
    let mut container = Container::default();
    let mut in_parameters = false;
    // the collections being read, innermost last, each with its key
    let mut open: Vec<(Option<String>, Items)> = Vec::new();
    // for each `<parameter>` not closed yet, whether it's a collection
    let mut unclosed: Vec<bool> = Vec::new();
    for (tag, content) in tags(text) {
        match (tag.name, tag.closing) {
            ("parameters", closing) if open.is_empty() => in_parameters = !closing,
            ("service", false) if !in_parameters => {
                let Some(id) = attribute(&tag, "id") else {
                    continue;
                };
                let service = match (attribute(&tag, "alias"), attribute(&tag, "class")) {
                    (Some(alias), _) => Service::Alias(alias.to_owned()),
                    (None, Some(class)) => {
                        Service::Class(class.trim_start_matches('\\').to_owned())
                    }
                    // a service named after its class
                    (None, None) if id.contains('\\') => Service::Class(id.to_owned()),
                    (None, None) => continue,
                };
                container.services.entry(id.to_owned()).or_insert(service);
            }
            ("parameter", false) if in_parameters => {
                let key = attribute(&tag, "key").map(str::to_owned);
                if attribute(&tag, "type") == Some("collection") && !tag.empty {
                    open.push((key, Vec::new()));
                    unclosed.push(true);
                    continue;
                }
                if !tag.empty {
                    unclosed.push(false);
                }
                let value = if tag.empty {
                    if attribute(&tag, "type") == Some("collection") {
                        Value::Collection(Vec::new())
                    } else {
                        Value::Null
                    }
                } else {
                    scalar(attribute(&tag, "type"), content)
                };
                match open.last_mut() {
                    Some((_, items)) => items.push((key, value)),
                    None => {
                        if let Some(key) = key {
                            container.parameters.insert(key, value);
                        }
                    }
                }
            }
            ("parameter", true) if in_parameters => {
                // the end of a collection, or of a value that was read already
                if unclosed.pop() == Some(true)
                    && let Some((key, items)) = open.pop()
                {
                    let value = Value::Collection(items);
                    match open.last_mut() {
                        Some((_, outer)) => outer.push((key, value)),
                        None => {
                            if let Some(key) = key {
                                container.parameters.insert(key, value);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    container
}

fn to_dynamic(value: &Value) -> Dynamic {
    match value {
        Value::Null => Dynamic::UNIT,
        Value::Bool(b) => (*b).into(),
        Value::Int(i) => (*i as INT).into(),
        Value::Float(f) => (*f).into(),
        Value::String(s) => s.as_str().into(),
        Value::Collection(items) => {
            if items.iter().all(|(k, _)| k.is_none()) {
                items
                    .iter()
                    .map(|(_, v)| to_dynamic(v))
                    .collect::<Array>()
                    .into()
            } else {
                let mut map = Map::new();
                for (i, (k, v)) in items.iter().enumerate() {
                    let key = k.clone().unwrap_or_else(|| i.to_string());
                    map.insert(key.into(), to_dynamic(v));
                }
                map.into()
            }
        }
    }
}

fn with_container<R>(f: impl FnOnce(&Container) -> R) -> Option<R> {
    stir_db::attached(|db| {
        let ws = Workspace::try_get(db)?;
        Some(f(symfony_container(db, ws)))
    })
    .flatten()
}

/// `symfony_parameter(name)`: `#{ found: bool, value }`, as it's not the same to be `null` and
/// not to be there.
fn parameter(name: &str) -> Map {
    let found = with_container(|c| c.parameters.get(name).cloned()).flatten();
    let mut out = Map::new();
    out.insert("found".into(), found.is_some().into());
    out.insert(
        "value".into(),
        found.as_ref().map_or(Dynamic::UNIT, to_dynamic),
    );
    out
}

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn("symfony_service", |id: &str| {
            with_container(|c| c.class_of(id).map(str::to_owned))
                .flatten()
                .map_or(Dynamic::UNIT, Dynamic::from)
        })
        .register_fn("symfony_parameter", parameter);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_xml_dumper_writes() {
        let c = parse(
            r#"<?xml version="1.0" encoding="utf-8"?>
<container xmlns="http://symfony.com/schema/dic/services">
  <parameters>
    <parameter key="kernel.environment">dev</parameter>
    <parameter key="kernel.debug">true</parameter>
    <parameter key="per_page">25</parameter>
    <parameter key="nothing"/>
    <parameter key="locales" type="collection">
      <parameter>en</parameter>
      <parameter>de</parameter>
    </parameter>
    <parameter key="text" type="string">&lt;b&gt; 1</parameter>
  </parameters>
  <!-- <service id="commented" class="Nope"/> -->
  <services>
    <service id="service_container" class="Symfony\Component\DependencyInjection\ContainerInterface" public="true" synthetic="true"/>
    <service id="App\Mailer" class="App\Mailer" public="true">
      <argument type="service" id="logger"/>
    </service>
    <service id="app.mailer" alias="App\Mailer" public="true"/>
    <service id="App\Other"/>
  </services>
</container>"#,
        );
        assert_eq!(c.class_of("app.mailer"), Some("App\\Mailer"));
        assert_eq!(c.class_of("App\\Other"), Some("App\\Other"));
        assert_eq!(c.class_of("commented"), None);
        assert_eq!(c.class_of("logger"), None);
        assert_eq!(c.parameters["kernel.debug"], Value::Bool(true));
        assert_eq!(c.parameters["per_page"], Value::Int(25));
        assert_eq!(c.parameters["nothing"], Value::Null);
        assert_eq!(c.parameters["text"], Value::String("<b> 1".into()));
        assert_eq!(
            c.parameters["locales"],
            Value::Collection(vec![
                (None, Value::String("en".into())),
                (None, Value::String("de".into()))
            ])
        );
    }
}
