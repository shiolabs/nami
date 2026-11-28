//! JSON Schemas of nami's files, for editors (`nami options --schema
//! computer`): the YAML language server then completes keys, shows each
//! one's docs, and marks a key nami would refuse. Made from the same lists
//! nami reads files with (config/'s keys, the recipe's, the project's)
//! and described by its options (`nami options`), so they can't drift.
//! Values are typed only where nami is sure of them: a schema mustn't mark
//! what nami takes.

use serde_json::{json, Map, Value};

/// The files there's a schema for, by the name nami gives them.
/// (A `set` is a nami file another lists in its packages, `- ./work.nami`;
/// a `service` is a project's service file, `services/postgres.nami`.)
pub const KINDS: &[&str] = &["computer", "user", "package", "project", "checks", "set", "service"];

/// A recipe's keys (how a package.nami says it's built), for a typo's hint.
pub fn recipe_keys() -> Vec<&'static str> {
    RECIPE.iter().map(|(k, _)| *k).collect()
}

/// A package.nami's keys that say how it's built, and what each is.
const RECIPE: &[(&str, &str)] = &[
    ("extends", "Rebuild this nixpkgs package with the changes here instead of starting from scratch: lists (patches, deps, flags) are added to its own."),
    ("name", "The package's name, for a build of its own."),
    ("version", "Its version."),
    ("source", "Where it's built from: `{ url, sha256 }`, `{ path }` (in the recipe's folder), several Debian packages unpacked together, or one of those for each kind of computer (`x86_64-linux:`, `aarch64-linux:`)."),
    ("bundle", "Debian packages whose libraries go in its lib, for libraries nixpkgs has no fitting version of."),
    ("build-system", "How it's built, when its files shouldn't decide: cargo, go, python, node, meson, cmake, autotools or make. Without it, nami works it out from the source."),
    ("deps", "Libraries and what it needs when it runs (nixpkgs' buildInputs)."),
    ("build-deps", "Tools needed only while building (nixpkgs' nativeBuildInputs)."),
    ("runtime-deps", "Libraries a prebuilt program loads itself (dlopen): added to its RPATH."),
    ("patches", "Patches to apply, in the recipe's folder."),
    ("configure-flags", "Flags for ./configure."),
    ("cmake-flags", "Flags for cmake."),
    ("meson-flags", "Flags for meson."),
    ("make-flags", "Flags for make."),
    ("cargo-flags", "Flags for cargo build (`--features`, `--bin`)."),
    ("go-flags", "Flags for go install (`-tags`)."),
    ("unpack", "Shell commands in place of the usual unpacking."),
    ("configure", "Shell commands in place of the usual configure step."),
    ("build", "Shell commands in place of the usual build step."),
    ("check", "Shell commands that test the build."),
    ("install", "Shell commands that put the result in $out."),
    ("build-env", "Environment variables for the build."),
];

/// Keys no nami file has, though they look like one would.
const REFUSED: &[(&str, &str)] = &[
    ("login-shell", "login-shell: goes under its package, `- zsh: { login-shell: true }`"),
    ("with", "with: goes under its Python, `- python3: { version: 3.13, with: [numpy] }`"),
    ("replace-in", "replace-in: isn't a key: what's replaced in one package goes under it, `- firefox: { replace: { libpng: ./libpng-fix } }`"),
    ("recipes", "recipes: is gone: list your own packages by path (`- ./packages/foo`)"),
];

/// A project.nami's keys the options don't describe.
const PROJECT: &[(&str, &str)] = &[
    ("develop", "A full build environment, like `nix develop`: a compiler, make, pkg-config, and the packages' headers and libraries. Unset: on when the project lists a compiler, a build tool or a library."),
    ("tasks", "Named commands: `nami run test` runs `tasks.test` with the project's tools. A task is a command, or `{ run, description, depends }`: `depends:` are tasks run first, each once; `description:` is what `nami project tasks` shows."),
    ("bundle", "How `nami bundle .` makes the project (Rust, Go, C or C++, Python, Node.js, Electron, Zig, Flutter, .NET) one file that runs on any Linux distribution, where its own files don't say: `name`, `version`, `program` (which of its programs runs), `icon` (.png or .svg), `desktop` (its .desktop file), or a build of its own: `build` (a command, run with the project's tools; install into $DESTDIR$PREFIX) and `output` (instead, the program or folder it makes). `developer` is your name, shown with the key that signs its updates; `sandbox` runs it in bubblewrap where the computer has it, with only what you list: `{ network: true, files: [documents, downloads], devices: [gpu, audio] }`."),
    ("provides", "Tools this project shares, a name and its recipe in the repo: anyone installs one with `nami install github:owner/repo#name`."),
];

/// A project's service file's keys (services/postgres.nami), and what each is.
const SERVICE: &[(&str, &str)] = &[
    ("version", "Which version of its packages: `16` is postgresql@16. A project changes it with `- ./services/postgres.nami: { version: 16 }`."),
    ("about", "What it is, in a few words: `nami up` shows it."),
    ("packages", "What it runs with, from the catalog: on the project's PATH and in its nami.lock like its other tools."),
    ("port", "Its port, `{port}` in its commands and variables."),
    ("ports", "More ports by name, `{port.<name>}`: `ports: { web: 8025 }`."),
    ("init", "Commands run once, before its first start (initdb...): again only when its folder in .nami is gone."),
    ("run", "Its command: it runs in the foreground, and nami restarts it when it ends."),
    ("args", "More arguments after `run:`'s, for a project to add its own."),
    ("ready", "A command that succeeds once it's ready: services listed after it wait for it."),
    ("ready-timeout", "How long it may take to be ready: `300`, `300s` or `5m` (3 minutes unless it says)."),
    ("restart", "Whether nami starts it again when it ends."),
    ("as-root", "Whether it may run as root: when `nami up` runs as root, one that won't runs as nobody."),
    ("stop-signal", "The signal that stops it: TERM (the default), INT, QUIT, HUP, KILL, USR1 or USR2."),
    ("dir", "The folder it runs in, from the project's (its top by default)."),
    ("env", "Variables for it alone. Each value is the text as written (`PORT: 8080`, `V: 3.10`, `DEBUG: true`)."),
    ("exports", "Variables it gives the project (DATABASE_URL...): in the project's environment, running or not."),
];

/// The schema of one kind of file (one of [`KINDS`]).
pub fn schema(kind: &str) -> Option<Value> {
    let (title, keys): (&str, Vec<&str>) = match kind {
        "computer" => ("system.nami: a computer", system_keys()),
        "user" => ("user.nami: a person's own", user_keys()),
        "package" => ("package.nami: a package, how it's built or set up", package_keys()),
        "project" => ("project.nami: a project's tools, tasks and services", crate::project::KEYS.iter().copied().filter(|k| *k != "replace-in").collect()),
        "set" => ("A set: packages and settings another nami file lists (`- ./work.nami`)", set_keys()),
        "service" => ("A project's service (services/postgres.nami)", ["format"].into_iter().chain(crate::up::KEYS.iter().copied()).collect()),
        "checks" => return Some(checks()),
        _ => return None,
    };
    let mut props = Map::new();
    for k in keys {
        props.insert(k.to_string(), property(kind, k));
    }
    // Keys nami refuses, saying where what they said goes.
    let former = crate::config::FORMER_KEYS.iter().filter(|_| kind == "computer" || kind == "package").map(|(k, now)| (*k, format!("{k}: is {now}")));
    // What must be one: a package only adds.
    let old_single = crate::format::RENAMED_KEYS.iter().filter(|(_, new)| crate::config::SINGLE_KEYS.contains(new)).map(|(old, _)| old);
    let single = crate::config::SINGLE_KEYS.iter().chain(old_single).filter(|_| kind == "package").map(|k| (*k, format!("{k}: a package only adds; {k}: goes in the file that lists it")));
    let refused = REFUSED.iter().filter(|_| kind != "service").map(|(k, m)| (*k, m.to_string()));
    // Keys with a new name: still read, until nami 1.0.
    if kind != "service" {
        for (old, new) in crate::format::RENAMED_KEYS {
            if let Some(Value::Object(p)) = props.get(*new).cloned() {
                let says = format!("{old} is now {new}; {old} works until nami 1.0");
                let mut p = p;
                p.insert("deprecationMessage".into(), says.clone().into());
                p.insert("description".into(), says.into());
                p.remove("markdownDescription");
                props.insert(old.to_string(), Value::Object(p));
            }
        }
    }
    for (k, says) in refused.chain(former).chain(single) {
        if !props.contains_key(k) {
            let docs = docs(kind, k).map(|d| format!("{says}\n\n---\n\n{d}")).unwrap_or_else(|| says.clone());
            props.insert(k.to_string(), json!({ "not": {}, "errorMessage": says, "deprecationMessage": says, "description": says, "markdownDescription": docs }));
        }
    }
    Some(json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": crate::site::schema_url(kind),
        "title": title,
        "type": ["object", "null"],
        "properties": props,
        "additionalProperties": false,
    }))
}

/// What a system.nami may say: nami's keys for it, as written (not the
/// ones it reads them into).
fn system_keys() -> Vec<&'static str> {
    let mut keys: Vec<&str> = crate::config::TOP_KEYS.iter().copied().filter(|k| *k != "replace-in").collect();
    keys.extend(["desktop", "login-screen", "hostname", "timezone", "language", "keyboard", "users", "without", "wifi", "env", "updates"]);
    keys
}

fn user_keys() -> Vec<&'static str> {
    let mut keys: Vec<&str> = crate::config::TOP_KEYS.iter().copied().filter(|k| *k != "replace-in").collect();
    keys.extend(crate::config::USER_KEYS.iter().filter(|k| **k != "login-shell"));
    keys
}

/// A set's: a computer's or a person's, whichever the set is for.
fn set_keys() -> Vec<&'static str> {
    let mut keys = system_keys();
    for k in user_keys() {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    keys
}

/// A package.nami's: a computer's (but its desktops, and what must be one:
/// the computer's name, its time zone...), a package's own, and how it's built.
fn package_keys() -> Vec<&'static str> {
    let mut keys: Vec<&str> = system_keys().into_iter().filter(|k| *k != "desktop" && !crate::config::SINGLE_KEYS.contains(k)).collect();
    for k in crate::config::PACKAGE_KEYS.iter().chain(RECIPE.iter().map(|(k, _)| k)) {
        if !keys.contains(k) {
            keys.push(k);
        }
    }
    keys
}

/// An option's key (`files (user.nami: text)`): the keys it's about, and
/// which kind of file it's for (None: any).
fn about(option: &str) -> (Vec<&str>, Option<&'static str>) {
    let (names, scope) = match option.split_once(" (") {
        Some((n, s)) => (n, s),
        None => (option, ""),
    };
    let kind = if scope.starts_with("user.nami") {
        Some("user")
    } else if scope.starts_with("package.nami") || scope.starts_with("next to a package.nami") {
        Some("package")
    } else if scope.starts_with("project") {
        Some("project")
    } else {
        None
    };
    (names.split(" / ").map(str::trim).collect(), kind)
}

/// Whether an option for `kind` (None: any) describes a key of a `file`.
fn for_file(kind: Option<&str>, file: &str) -> bool {
    match kind {
        None => true,
        // What a computer can say, a package can too.
        Some("package") => file == "package",
        Some(k) => k == file,
    }
}

/// Key `k` of a `kind` file: its value's type where nami is sure of it,
/// and its docs.
fn property(kind: &str, k: &str) -> Value {
    let mut p = if kind == "service" { service_type(k) } else { value_type(kind, k) };
    if let Some(docs) = docs(kind, k) {
        let plain: String = docs.lines().next().unwrap_or_default().replace('`', "");
        p.insert("description".into(), plain.into());
        p.insert("markdownDescription".into(), docs.into());
    }
    Value::Object(p)
}

/// What the options (and the lists here) say of key `k` in a `kind` file.
fn docs(kind: &str, k: &str) -> Option<String> {
    let mut text = Vec::new();
    for o in crate::config::OPTIONS {
        let (names, scope) = about(o.key);
        if names.contains(&k) && for_file(scope, kind) {
            text.push(format!("{}\n\n```yaml\n{}\n```", o.summary, o.example));
        }
    }
    let own = match kind {
        "package" => RECIPE,
        "project" => PROJECT,
        "service" => SERVICE,
        _ => &[],
    };
    if let Some((_, what)) = own.iter().find(|(r, _)| *r == k) {
        text.push(what.to_string());
    }
    (!text.is_empty()).then(|| text.join("\n\n---\n\n"))
}

/// An entry of a dash list: a name, or a name with its settings under it
/// (`- zsh: { login-shell: true }`, `- nodejs: 22`).
fn entries() -> Value {
    json!({ "type": "array", "items": { "anyOf": [
        { "type": "string" },
        { "type": "object", "minProperties": 1, "maxProperties": 1 },
    ] } })
}

fn strings() -> Value {
    json!({ "type": "array", "items": { "type": "string" } })
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn value_type(kind: &str, k: &str) -> Map<String, Value> {
    let scalar = json!({ "type": ["string", "number", "boolean"] });
    obj(match (kind, k) {
        (_, "packages") => entries(),
        (_, "services") => json!({ "type": ["array", "object"] }),
        (_, "tasks") => json!({ "type": "object", "additionalProperties": { "anyOf": [
            { "type": "string" },
            { "type": "object", "properties": {
                "run": { "type": "string", "description": "Its command." },
                "description": { "type": "string", "description": "What it does, in a few words: nami project tasks shows it." },
                "depends": { "type": "array", "items": { "type": "string" }, "description": "Tasks run first, each once, in this order." },
            }, "additionalProperties": false },
        ] } }),
        (_, "files" | "namios" | "settings" | "replace" | "aliases" | "login" | "groups" | "provides" | "build-env" | "bundle") => json!({ "type": "object" }),
        (_, "env") => json!({ "type": "object", "additionalProperties": scalar }),
        (_, "unfree" | "develop") => json!({ "type": "boolean" }),
        (_, "history") => json!({ "type": "integer", "minimum": 0 }),
        (_, "cleanup") => {
            let age = json!({ "type": "string", "pattern": "^(off|never|[0-9]+ *[smhdw]?)$" });
            json!({ "anyOf": [age.clone(), { "type": "object", "properties": {
                "older-than": { "description": "How old a change gets before it goes (30d), or off.", "anyOf": [age] },
                "keep": { "type": "integer", "minimum": 1, "description": "The newest changes that always stay, whatever their age (3)." },
            }, "additionalProperties": false }] })
        }
        (_, "updates") => json!({ "enum": crate::config::UPDATES }),
        (_, "format") => json!({ "type": "integer", "minimum": 1, "maximum": crate::config::FORMAT }),
        (_, "caches") => json!({ "type": "array" }),
        (_, "hostname" | "timezone" | "language" | "keyboard" | "login-screen" | "name") => json!({ "type": "string" }),
        (_, "version") => json!({ "type": ["string", "number"] }),
        (_, "desktop") => json!({ "anyOf": [{ "type": "string" }, strings()] }),
        (_, "without" | "setuid" | "name-lookup" | "libraries" | "deps" | "build-deps" | "runtime-deps" | "patches" | "configure-flags" | "cmake-flags" | "meson-flags" | "make-flags" | "cargo-flags" | "go-flags") => strings(),
        ("package", "build-system") => json!({ "enum": crate::build_kinds::KINDS.iter().map(|(_, n, _)| *n).collect::<Vec<_>>() }),
        (_, "users") => json!({ "type": "object", "additionalProperties": {
            "type": "object",
            "properties": {
                "admin": { "type": "boolean", "description": "May change the computer (sudo, settings for everyone)." },
                "name": { "type": "string", "description": "The full name the login screen shows." },
                "password": { "type": "string", "description": "A password hash ($6$..., from mkpasswd); without one, nami usb asks." },
                "groups": { "anyOf": [{ "type": "string" }, strings()], "description": "More groups they're in (docker)." },
                "child": { "type": "boolean", "description": "A child: no sudo, no apps of their own, and the limits hours and without (parental controls)." },
                "hours": { "type": "string", "pattern": "^[0-2][0-9]:[0-5][0-9]-[0-2][0-9]:[0-5][0-9]$", "description": "A child's hours, 07:00-20:00: they log in only then, and their session ends at the end." },
                "without": { "type": "array", "items": { "type": "string" }, "description": "Apps a child can't use: off their menu, and their desktop won't start them." },
            },
            "additionalProperties": false,
        } }),
        (_, "wifi") => json!({ "type": "object", "additionalProperties": { "type": ["object", "null"] } }),
        (_, "boot") => parts(&["kernel", "microcode", "initrd-programs", "initrd-modules", "kernel-options"], &["kernel"]),
        (_, "drivers") => parts(&["firmware", "graphics", "graphics-32bit", "kernel-modules"], &[]),
        (_, "unpack" | "configure" | "build" | "check" | "install") => json!({ "type": "string" }),
        _ => json!({}),
    })
}

/// A service file's key `k`: its value's type where nami is sure of it.
fn service_type(k: &str) -> Map<String, Value> {
    let scalar = json!({ "type": ["string", "number", "boolean"] });
    let port = json!({ "type": "integer", "minimum": 1, "maximum": 65535 });
    obj(match k {
        "packages" => entries(),
        "port" => port,
        "ports" => json!({ "type": "object", "additionalProperties": port }),
        "version" => json!({ "type": ["string", "number"] }),
        "ready-timeout" => json!({ "type": ["string", "integer"] }),
        "env" | "exports" => json!({ "type": "object", "additionalProperties": scalar }),
        "about" | "init" | "run" | "args" | "ready" | "stop-signal" | "dir" => json!({ "type": "string" }),
        "format" => json!({ "type": "integer", "minimum": 1, "maximum": crate::config::FORMAT, "description": "The nami file format it's written in: 1. A newer nami may know a higher one." }),
        "as-root" => json!({ "type": "boolean" }),
        _ => json!({}),
    })
}

/// `boot:` and `drivers:`: lists of names under each part (`scalars` may
/// be one name).
fn parts(names: &[&str], scalars: &[&str]) -> Value {
    let props: Map<String, Value> = names.iter().map(|n| (n.to_string(), if scalars.contains(n) { json!({ "type": "string" }) } else { strings() })).collect();
    json!({ "type": "object", "properties": props, "additionalProperties": false })
}

/// checks.nami: a name, and its command and what its output has.
fn checks() -> Value {
    let o = crate::config::OPTIONS.iter().find(|o| o.key.starts_with("checks.nami"));
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": crate::site::schema_url("checks"),
        "title": "checks.nami: how to tell a package works",
        "description": o.map(|o| o.summary).unwrap_or_default(),
        "type": ["object", "null"],
        "additionalProperties": {
            "type": "object",
            "properties": {
                "run": { "type": "string", "description": "The command, run on the computer." },
                "expect": { "type": "string", "description": "What its output has: a regular expression." },
            },
            "required": ["run", "expect"],
            "additionalProperties": false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Enough of JSON Schema for nami's own: the errors at `at`.
    fn check(s: &Value, v: &Value, at: &str, errors: &mut Vec<String>) {
        if s.get("not").is_some() {
            errors.push(format!("{at}: {}", s["errorMessage"].as_str().unwrap_or("not allowed")));
            return;
        }
        if let Some(any) = s.get("anyOf").and_then(Value::as_array) {
            if !any.iter().any(|a| {
                let mut e = Vec::new();
                check(a, v, at, &mut e);
                e.is_empty()
            }) {
                errors.push(format!("{at}: none of its forms"));
            }
        }
        if let Some(t) = s.get("type") {
            let types: Vec<&str> = match t {
                Value::String(t) => vec![t.as_str()],
                Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            let is = match v {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };
            if !types.iter().any(|t| *t == is || (*t == "number" && is == "integer")) {
                errors.push(format!("{at}: a {is}, not {types:?}"));
                return;
            }
        }
        if let Value::Object(m) = v {
            let props = s.get("properties").and_then(Value::as_object);
            for (k, val) in m {
                match props.and_then(|p| p.get(k)) {
                    Some(p) => check(p, val, &format!("{at}.{k}"), errors),
                    None => match s.get("additionalProperties") {
                        Some(Value::Bool(false)) => errors.push(format!("{at}: {k}: not a key here")),
                        Some(a @ Value::Object(_)) => check(a, val, &format!("{at}.{k}"), errors),
                        _ => {}
                    },
                }
            }
            for r in s.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                if !m.contains_key(r) {
                    errors.push(format!("{at}: {r}: missing"));
                }
            }
            let n = m.len() as u64;
            if s.get("minProperties").and_then(Value::as_u64).is_some_and(|min| n < min) || s.get("maxProperties").and_then(Value::as_u64).is_some_and(|max| n > max) {
                errors.push(format!("{at}: {n} keys"));
            }
        }
        if let (Value::Array(items), Some(each)) = (v, s.get("items")) {
            for (i, item) in items.iter().enumerate() {
                check(each, item, &format!("{at}[{i}]"), errors);
            }
        }
    }

    /// A file as the YAML language server sees it: JSON, `!secret ...` a string.
    fn as_json(text: &str) -> Value {
        let v: serde_yaml::Value = serde_yaml::from_str(text).unwrap();
        fn plain(v: serde_yaml::Value) -> serde_yaml::Value {
            match v {
                serde_yaml::Value::Tagged(t) => plain(t.value),
                serde_yaml::Value::Mapping(m) => serde_yaml::Value::Mapping(m.into_iter().map(|(k, v)| (k, plain(v))).collect()),
                serde_yaml::Value::Sequence(s) => serde_yaml::Value::Sequence(s.into_iter().map(plain).collect()),
                v => v,
            }
        }
        serde_json::to_value(plain(v)).unwrap()
    }

    fn kind_of(file: &Path) -> Option<&'static str> {
        match file.file_name()?.to_str()? {
            "system.nami" | "computer.nami" => Some("computer"),
            "user.nami" => Some("user"),
            "package.nami" => Some("package"),
            "project.nami" => Some("project"),
            "checks.nami" => Some("checks"),
            // A project's service file (examples/services/postgres.nami).
            _ if file.parent()?.file_name()? == "services" => Some("service"),
            _ => None,
        }
    }

    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "nami") {
                out.push(p);
            }
        }
    }

    #[test]
    fn every_option_is_in_a_schema() {
        let all: Vec<Value> = KINDS.iter().map(|k| schema(k).unwrap()).collect();
        let text = serde_json::to_string(&all).unwrap();
        for o in crate::config::OPTIONS {
            let (names, scope) = about(o.key);
            // (tailscale is a package and a file, not a key of its own.)
            if names == ["tailscale"] || o.key.starts_with("checks.nami") {
                continue;
            }
            let kinds: Vec<&str> = KINDS.iter().copied().filter(|k| *k != "checks" && for_file(scope, k)).collect();
            for n in names {
                assert!(kinds.iter().any(|k| schema(k).unwrap()["properties"].get(n).is_some()), "{}: {n} is in no schema", o.key);
            }
            assert!(text.contains(&o.summary.replace('\\', "\\\\").replace('"', "\\\"")[..40.min(o.summary.len())]), "{}: its docs", o.key);
        }
        assert!(schema("nothing").is_none());
    }

    #[test]
    fn the_examples_fit() {
        let top = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        walk(&top.join("examples"), &mut files);
        walk(&top.join("tests"), &mut files);
        let mut checked = 0;
        let mut errors = Vec::new();
        for f in &files {
            let Some(kind) = kind_of(f) else { continue };
            let s = schema(kind).unwrap();
            let v = as_json(&std::fs::read_to_string(f).unwrap());
            let mut e = Vec::new();
            check(&s, &v, "", &mut e);
            errors.extend(e.into_iter().map(|e| format!("{}: {e}", f.strip_prefix(top).unwrap().display())));
            checked += 1;
        }
        assert!(checked > 50, "{checked} files");
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    #[test]
    fn it_marks_what_nami_refuses() {
        let mut e = Vec::new();
        check(&schema("computer").unwrap(), &as_json("pakages: [htop]\nfrom: x\nboot: { kernel: linux_zen }\n"), "", &mut e);
        assert_eq!(e.len(), 3, "{e:?}");
        assert!(e.iter().any(|e| e.contains("from: is gone")), "{e:?}");
        let mut e = Vec::new();
        check(&schema("package").unwrap(), &as_json("boot: { kernel-option: [x] }\ndesktop: niri\n"), "", &mut e);
        assert_eq!(e.len(), 2, "{e:?}");
        let mut e = Vec::new();
        check(&schema("user").unwrap(), &as_json("hostname: x\nlogin-shell: zsh\n"), "", &mut e);
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(e.iter().any(|e| e.contains("goes under its package")), "{e:?}");
        let mut e = Vec::new();
        check(&schema("checks").unwrap(), &as_json("\"it runs\": { run: x }\n"), "", &mut e);
        assert_eq!(e.len(), 1, "{e:?}");
    }

    #[test]
    fn old_names_are_read_and_marked_and_format_is_known() {
        for kind in ["computer", "user", "package", "set"] {
            let s = schema(kind).unwrap();
            let p = &s["properties"];
            assert_eq!(p["format"]["maximum"], crate::config::FORMAT, "{kind}");
            if kind != "package" {
                assert!(p["keep-generations"]["deprecationMessage"].as_str().unwrap().contains("is now history"), "{kind}");
                assert!(p["keep-generations"].get("not").is_none(), "{kind}: still read");
            }
            assert!(p["settings"].get("deprecationMessage").is_none(), "{kind}: settings: keeps its name");
        }
        let p = schema("project").unwrap();
        assert!(p["properties"]["unfree"].get("deprecationMessage").is_none());
        let task = &p["properties"]["tasks"]["additionalProperties"]["anyOf"][1]["properties"];
        assert!(task["depends"].is_object() && task["run"].is_object());
        assert!(schema("service").unwrap()["properties"]["format"].is_object());
    }

    #[test]
    fn a_package_only_adds() {
        let s = schema("package").unwrap();
        for k in crate::config::SINGLE_KEYS {
            let mut e = Vec::new();
            check(&s, &as_json(&format!("{k}: x\n")), "", &mut e);
            assert_eq!(e.len(), 1, "{k}: {e:?}");
            assert!(e[0].contains("a package only adds"), "{k}: {e:?}");
        }
        // The computer's own file says them.
        let mut e = Vec::new();
        check(&schema("computer").unwrap(), &as_json("hostname: laptop\ntimezone: Europe/Copenhagen\n"), "", &mut e);
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn sets_and_services_have_schemas() {
        // A set is a computer's or a person's.
        let mut e = Vec::new();
        check(&schema("set").unwrap(), &as_json("packages: [htop]\naliases: { ll: ls -l }\nwithout: [plymouth]\n"), "", &mut e);
        assert!(e.is_empty(), "{e:?}");
        let mut e = Vec::new();
        check(&schema("set").unwrap(), &as_json("pakages: [htop]\n"), "", &mut e);
        assert_eq!(e.len(), 1, "{e:?}");
        // A service file.
        let s = schema("service").unwrap();
        assert_eq!(s["$id"], "https://namios.dev/schema/service.json");
        let mut e = Vec::new();
        check(&s, &as_json("packages: [redis]\nport: 6379\nrun: redis-server\nenv: { V: 3.10, DEBUG: true, N: 2, NAME: x }\n"), "", &mut e);
        assert!(e.is_empty(), "{e:?}");
        let mut e = Vec::new();
        check(&s, &as_json("port: web\nrunn: x\nenv: { X: [1] }\n"), "", &mut e);
        assert_eq!(e.len(), 3, "{e:?}");
        assert!(s["properties"]["ready-timeout"]["markdownDescription"].as_str().unwrap().contains("5m"));
    }

    #[test]
    fn env_values_are_any_scalar() {
        for kind in ["computer", "user", "project", "set"] {
            let mut e = Vec::new();
            check(&schema(kind).unwrap(), &as_json("env: { PORT: 8080, V: 3.10, DEBUG: true, NAME: ada }\n"), "", &mut e);
            assert!(e.is_empty(), "{kind}: {e:?}");
            let mut e = Vec::new();
            check(&schema(kind).unwrap(), &as_json("env: { PATHS: [a, b] }\n"), "", &mut e);
            assert_eq!(e.len(), 1, "{kind}: {e:?}");
        }
    }

    #[test]
    fn schemas_live_at_the_public_address() {
        for kind in KINDS {
            assert_eq!(schema(kind).unwrap()["$id"], format!("{}/schema/{kind}.json", crate::site::PUBLIC), "{kind}");
        }
        // And the docs tell editors the same.
        let docs = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/guide/editor.md")).unwrap();
        for kind in KINDS {
            assert!(docs.contains(&crate::site::schema_url(kind)), "editors.md: {kind}");
        }
        assert!(!docs.contains("workers.dev"), "editors.md names the public address only");
    }
}
