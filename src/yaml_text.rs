//! A nami file's text as YAML, its versions and env values as written.
//!
//! YAML reads `python3: 3.10` as the number 3.1, `PORT: 8080` as a number
//! and `DEBUG: true` as yes. In a nami file those are text: a version is
//! the version written, and an env value is what the program gets. So
//! before the file is read, each plain value in such a place is marked as
//! text (`!!str`), from where the YAML parser itself finds it: in block
//! or flow style, quoted name or not, wherever it is in the file.
//!
//! The places: `packages:` entries' versions (`- python3: 3.10`, and
//! `version:` under one), `env:` (and `exports:`, and the former
//! `environment:`), a recipe's or service file's own `version:`, a
//! project's `bundle: version:`, and the same under each of `services:`.

use std::borrow::Cow;
use std::mem::MaybeUninit;

use serde_yaml::Value;
use unsafe_libyaml as sys;

/// A nami file's text read as YAML, every version and env value as the
/// text written.
pub fn from_str(text: &str) -> Result<Value, serde_yaml::Error> {
    serde_yaml::from_str(&as_written(text))
}

/// `text` with each plain version and env value marked as text. Unchanged
/// when there are none, or when it isn't YAML (serde_yaml then says why).
/// Lines stay where they are: only `!!str ` is added before a value.
pub fn as_written(text: &str) -> Cow<'_, str> {
    let spots = match spots(text) {
        Some(s) if !s.is_empty() => s,
        _ => return Cow::Borrowed(text),
    };
    let mut out = String::with_capacity(text.len() + spots.len() * 6);
    let mut last = 0;
    for at in spots {
        if at < last || at > text.len() || !text.is_char_boundary(at) {
            return Cow::Borrowed(text);
        }
        out.push_str(&text[last..at]);
        out.push_str("!!str ");
        last = at;
    }
    out.push_str(&text[last..]);
    Cow::Owned(out)
}

/// `text` read as the value at `path` in a nami file (`env.PORT`, or an
/// item of `packages` when `item`): a version or env value is the text
/// written there (`nami config set env.V 3.10` sets "3.10").
pub fn at(path: &[String], item: bool, text: &str) -> Option<Value> {
    if path.is_empty() {
        return from_str(text).ok();
    }
    let key = |k: &str| serde_yaml::to_string(&Value::from(k)).ok().map(|s| s.trim_end().to_string());
    let mut doc = String::new();
    for (d, k) in path.iter().enumerate() {
        doc += &format!("{}{}:\n", "  ".repeat(d), key(k)?);
    }
    let ind = "  ".repeat(path.len());
    for (i, line) in text.lines().enumerate() {
        let lead = match (i, item) {
            (0, true) => "- ",
            (_, true) => "  ",
            _ => "",
        };
        doc += &format!("{ind}{lead}{line}\n");
    }
    let mut v = &from_str(&doc).ok()?;
    for k in path {
        v = v.get(k.as_str())?;
    }
    Some(if item { v.as_sequence()?.first()?.clone() } else { v.clone() })
}

/// The text of an env value: what's written, a number or true/false
/// alike (a value nami made itself, not read from a file, can be one).
pub fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Tagged(t) => scalar_text(&t.value),
        _ => None,
    }
}

/// An `env:` map (`deserialize_with`): each value its text, whichever
/// file it's in.
pub fn text_map<'de, D: serde::Deserializer<'de>>(d: D) -> Result<std::collections::BTreeMap<String, String>, D::Error> {
    use serde::Deserialize;
    let raw = std::collections::BTreeMap::<String, Value>::deserialize(d)?;
    raw.into_iter()
        .map(|(k, v)| scalar_text(&v).map(|s| (k.clone(), s)).ok_or_else(|| serde::de::Error::custom(format!("env: {k}: a value is text, a number or true/false"))))
        .collect()
}

#[derive(Debug)]
enum Seg {
    Key(String),
    Item,
}

struct Frame {
    map: bool,
    /// A map's key being read (None: the next scalar is a key).
    key: Option<String>,
    /// This collection is itself a key (YAML's complex keys).
    is_key: bool,
}

/// Whether the value at `path` is text as written.
fn wants_text(path: &[Seg]) -> bool {
    use Seg::*;
    let k = |s: &Seg, name: &str| matches!(s, Key(k) if k == name);
    // A service's own (in `services:`, a list or a map): as a file's.
    let inner = match path {
        [s, Item, Key(_), rest @ ..] | [s, Key(_), rest @ ..] if k(s, "services") => rest,
        _ => path,
    };
    let own = |p: &[Seg]| match p {
        [v] => k(v, "version"),
        [e, Key(_)] => k(e, "env") || k(e, "exports") || k(e, "environment"),
        [p, Item, Key(_)] => k(p, "packages"),
        [p, Item, Key(_), v] => k(p, "packages") && k(v, "version"),
        _ => false,
    };
    own(inner) || matches!(path, [b, v] if k(b, "bundle") && k(v, "version"))
}

fn is_null(s: &str) -> bool {
    matches!(s, "" | "~" | "null" | "Null" | "NULL")
}

/// Where in `text` each plain value that is text as written starts.
fn spots(text: &str) -> Option<Vec<usize>> {
    let mut stack: Vec<Frame> = Vec::new();
    let mut spots = Vec::new();
    let mut ok = true;
    // A value was read: in a map, the next scalar is a key again.
    fn done(stack: &mut [Frame]) {
        if let Some(f) = stack.last_mut().filter(|f| f.map) {
            f.key = None;
        }
    }
    fn wants_key(stack: &[Frame]) -> bool {
        stack.last().is_some_and(|f| f.map && f.key.is_none())
    }
    fn path(stack: &[Frame]) -> Vec<Seg> {
        stack.iter().map(|f| if f.map { Seg::Key(f.key.clone().unwrap_or_default()) } else { Seg::Item }).collect()
    }
    // SAFETY: the parser stays where it is (it points at itself while it
    // reads a string), reads `text` while it lives, and is deleted once;
    // each event is read only as its type says, and deleted once.
    unsafe {
        let mut parser = MaybeUninit::<sys::yaml_parser_t>::uninit();
        let parser = parser.as_mut_ptr();
        if sys::yaml_parser_initialize(parser).fail {
            return None;
        }
        sys::yaml_parser_set_encoding(parser, sys::YAML_UTF8_ENCODING);
        sys::yaml_parser_set_input_string(parser, text.as_ptr(), text.len() as u64);
        loop {
            let mut event = MaybeUninit::<sys::yaml_event_t>::uninit();
            let event = event.as_mut_ptr();
            if sys::yaml_parser_parse(parser, event).fail {
                ok = false;
                break;
            }
            let kind = (*event).type_;
            match kind {
                sys::YAML_STREAM_END_EVENT => {
                    sys::yaml_event_delete(event);
                    break;
                }
                sys::YAML_SCALAR_EVENT => {
                    let s = &(*event).data.scalar;
                    let value = String::from_utf8_lossy(std::slice::from_raw_parts(s.value, s.length as usize)).into_owned();
                    if wants_key(&stack) {
                        if let Some(f) = stack.last_mut() {
                            f.key = Some(value);
                        }
                    } else {
                        let plain = s.style == sys::YAML_PLAIN_SCALAR_STYLE && s.tag.is_null();
                        if plain && !is_null(&value) && wants_text(&path(&stack)) {
                            spots.push((*event).start_mark.index as usize);
                        }
                        done(&mut stack);
                    }
                }
                sys::YAML_ALIAS_EVENT => {
                    if wants_key(&stack) {
                        if let Some(f) = stack.last_mut() {
                            f.key = Some(String::new());
                        }
                    } else {
                        done(&mut stack);
                    }
                }
                sys::YAML_SEQUENCE_START_EVENT | sys::YAML_MAPPING_START_EVENT => {
                    let is_key = wants_key(&stack);
                    stack.push(Frame { map: kind == sys::YAML_MAPPING_START_EVENT, key: None, is_key });
                }
                sys::YAML_SEQUENCE_END_EVENT | sys::YAML_MAPPING_END_EVENT => {
                    let f = stack.pop();
                    if f.is_some_and(|f| f.is_key) {
                        if let Some(p) = stack.last_mut() {
                            p.key = Some(String::new());
                        }
                    } else {
                        done(&mut stack);
                    }
                }
                sys::YAML_DOCUMENT_START_EVENT => stack.clear(),
                _ => {}
            }
            sys::yaml_event_delete(event);
        }
        sys::yaml_parser_delete(parser);
    }
    spots.sort_unstable();
    ok.then_some(spots)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(v: &'a Value, path: &[&str]) -> &'a Value {
        path.iter().fold(v, |v, k| match k.parse::<usize>() {
            Ok(i) => &v[i],
            Err(_) => &v[*k],
        })
    }

    #[test]
    fn versions_are_as_written_in_any_style() {
        for text in [
            "packages:\n  - python3: 3.10\n",
            "packages: [{python3: 3.10}]\n",
            "packages:\n  - \"python3\": 3.10\n",
            "packages:\n  - python3:\n      version: 3.10\n",
            "packages: [ { python3: { version: 3.10 } } ]\n",
        ] {
            let v = from_str(text).unwrap();
            let item = &v["packages"][0]["python3"];
            let got = item.as_str().or_else(|| item["version"].as_str());
            assert_eq!(got, Some("3.10"), "{text}");
        }
    }

    #[test]
    fn only_versions_and_env_are_text() {
        let text = "files:\n  ~/notes: { python3: 3.12 }\n  ~/a.yaml: { env: { A: 1.10 } }\npackages:\n  - python3: 3.10\n  - git:\n      ~/.gitconfig: { core: { x: 1.5 } }\nhistory: 5\nsettings:\n  org.gnome.desktop.interface: { text-scaling-factor: 1.25 }\n";
        let v = from_str(text).unwrap();
        assert_eq!(get(&v, &["packages", "0", "python3"]).as_str(), Some("3.10"));
        assert!(get(&v, &["files", "~/notes", "python3"]).is_number());
        assert!(get(&v, &["files", "~/a.yaml", "env", "A"]).is_number());
        assert!(get(&v, &["packages", "1", "git", "~/.gitconfig", "core", "x"]).is_number());
        assert!(v["history"].is_number());
        assert!(get(&v, &["settings", "org.gnome.desktop.interface", "text-scaling-factor"]).is_f64());
    }

    #[test]
    fn env_values_are_the_text_written() {
        let text = "env:\n  PORT: 8080\n  V: 3.10\n  DEBUG: true\n  OFF: no\n  EMPTY:\n  Q: \"x\"\nservices:\n  - api:\n      env: { WORKERS: 4 }\n      version: 1.10\nexports: { URL: 5 }\nbundle: { version: 2.0 }\n";
        let v = from_str(text).unwrap();
        assert_eq!(v["env"]["PORT"].as_str(), Some("8080"));
        assert_eq!(v["env"]["V"].as_str(), Some("3.10"));
        assert_eq!(v["env"]["DEBUG"].as_str(), Some("true"));
        assert_eq!(v["env"]["OFF"].as_str(), Some("no"));
        assert!(v["env"]["EMPTY"].is_null());
        assert_eq!(v["env"]["Q"].as_str(), Some("x"));
        assert_eq!(get(&v, &["services", "0", "api", "env", "WORKERS"]).as_str(), Some("4"));
        assert_eq!(get(&v, &["services", "0", "api", "version"]).as_str(), Some("1.10"));
        let v2 = from_str("services:\n  db: { env: { X: 1.0 } }\n").unwrap();
        assert_eq!(get(&v2, &["services", "db", "env", "X"]).as_str(), Some("1.0"));
        assert_eq!(v["exports"]["URL"].as_str(), Some("5"));
        assert_eq!(v["bundle"]["version"].as_str(), Some("2.0"));
    }

    #[test]
    fn a_value_in_its_place() {
        let p = |s: &str| s.split('.').map(String::from).collect::<Vec<_>>();
        assert_eq!(at(&p("env.V"), false, "3.10"), Some(Value::from("3.10")));
        assert_eq!(at(&p("env.DEBUG"), false, "true"), Some(Value::from("true")));
        assert_eq!(at(&p("history"), false, "5"), Some(Value::from(5)));
        let item = at(&p("packages"), true, "python3: 3.10").unwrap();
        assert_eq!(item["python3"].as_str(), Some("3.10"));
        let whole = at(&p("packages"), true, "vim:\n  version: 9.10").unwrap();
        assert_eq!(whole["vim"]["version"].as_str(), Some("9.10"));
    }

    #[test]
    fn a_recipes_version_is_text() {
        let v = from_str("name: hello\nversion: 1.10\n").unwrap();
        assert_eq!(v["version"].as_str(), Some("1.10"));
    }

    #[test]
    fn lines_stay_and_bad_yaml_is_left_to_serde() {
        let text = "packages:\n  - a: 1.0\n  - b: [\n";
        assert_eq!(as_written(text), text);
        assert!(from_str(text).is_err());
        let fine = "packages:\n  - a: 1.0\n";
        assert_eq!(as_written(fine).lines().count(), fine.lines().count());
    }

    #[test]
    fn anchors_and_complex_keys_dont_confuse_it() {
        let text = "x: &v 3.10\npackages:\n  - ? [a, b]\n    : 1\n  - c: 2.50\nenv: { A: *v }\n";
        let v = from_str(text).unwrap();
        assert_eq!(get(&v, &["packages", "1", "c"]).as_str(), Some("2.50"));
    }
}
