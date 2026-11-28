/// NAMI_DUMP_CONFIGS="a.nami b.nami" NAMI_DUMP_OUT=dir cargo test dump_configs -- --ignored:
/// each computer file as nami reads it, to see that a change to the
/// format leaves the computers it makes as they were.
#[test]
fn keys_only_a_computer_nami_made_acts_on() {
    assert_eq!(only_on_nami_os("hostname: pc\npackages: [ripgrep]\nenv: { A: b }\nusers: { ada: {} }\nwifi: {}\n").as_deref(), Some("hostname:, env: and users:"));
    assert_eq!(only_on_nami_os("settings: {}\n").as_deref(), Some("settings:"));
    assert_eq!(only_on_nami_os("packages: [ripgrep]\nfiles: {}\nservices: []\n"), None);
}

/// On another distribution, `nami check` says each key only namiOS acts
/// on, at its line, and each set for namiOS.
#[test]
fn what_another_distro_keeps_its_own() {
    let said = |text: &str, distro: Option<&str>| not_on_this_distro(text, distro, None).into_iter().map(|d| (d.line, d.severity, d.message)).collect::<Vec<_>>();
    let d = said("packages: [ripgrep]\nhostname: pc\nusers:\n  ada: {}\ntimezone: Europe/Copenhagen\nwifi: {}\nenv: { A: b }\n", Some("Ubuntu"));
    assert_eq!(
        d,
        [
            (2, Severity::Warning, "`hostname:` only works on namiOS. Ubuntu keeps its own computer name.".to_string()),
            (3, Severity::Warning, "`users:` only works on namiOS. Ubuntu keeps its own users.".to_string()),
            (5, Severity::Warning, "`timezone:` only works on namiOS. Ubuntu keeps its own time zone.".to_string()),
            (7, Severity::Warning, "`env:` only works on namiOS. Ubuntu keeps its own environment variables.".to_string()),
        ]
    );
    assert_eq!(said("keyboard: de\n", None)[0].2, "`keyboard:` only works on namiOS. This distribution keeps its own keyboard layout.");
    assert!(said("packages: [ripgrep]\nfiles: {}\nservices: []\n", Some("Fedora Linux")).is_empty());

    // A set for namiOS, at its line in packages:.
    let tmp = std::env::temp_dir().join(format!("nami-distro-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join("nix/var/nami")).unwrap();
    std::fs::create_dir_all(tmp.join("packages/desk")).unwrap();
    std::fs::write(tmp.join("packages/desk/package.nami"), "packages: [htop]\n").unwrap();
    std::fs::write(tmp.join("nix/var/nami/index.json"), "{\"htop\":{\"name\":\"htop-1\",\"outputs\":{\"out\":\"/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-htop-1\"}}}").unwrap();
    let store = crate::store::Store::open_direct_readonly(&tmp).unwrap();
    let index = crate::index::Index::load(&store).unwrap();
    let text = "packages:\n  - htop\n  - desk\n";
    load_text(text, SYSTEM_FILE, &tmp).unwrap();
    let d: Vec<_> = not_on_this_distro(text, Some("Ubuntu"), Some(&index)).into_iter().map(|d| (d.line, d.message, d.hint)).collect();
    assert_eq!(d, [(3, "desk is a set for namiOS; left out here".to_string(), Some("Ubuntu keeps its own system".to_string()))]);
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
#[ignore]
fn dump_configs() {
    let out = std::path::PathBuf::from(std::env::var("NAMI_DUMP_OUT").expect("NAMI_DUMP_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    for f in std::env::var("NAMI_DUMP_CONFIGS").expect("NAMI_DUMP_CONFIGS").split_whitespace() {
        let name = f.replace('/', "_");
        let text = match load(Path::new(f)) {
            Ok(c) => format!("{c:#?}\n"),
            Err(e) => format!("error: {e:#}\n"),
        };
        std::fs::write(out.join(name), text).unwrap();
    }
}
/// A config repo's computers are its folders named after them (shared/
/// isn't one): the one named, the only one, or this computer's; `nami
/// check` says when a folder's name isn't its hostname.
/// A package of your own: a folder with package.nami, its build and its
/// setup; one that builds nothing is the catalog's package, set up.
#[test]
fn own_package_folders() {
    let d = std::env::temp_dir().join(format!("nami-own-packages-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for p in ["packages/mytool", "packages/htop"] {
        std::fs::create_dir_all(d.join(p)).unwrap();
    }
    std::fs::write(d.join("packages/mytool/package.nami"), "name: mytool\ninstall: mkdir $out\nservices:\n  - mytool:\n      run: mytool serve\n      /etc/mytool.conf: x\n").unwrap();
    std::fs::write(d.join("packages/htop/package.nami"), "files:\n  /etc/htoprc: y\n").unwrap();
    let c = load_text("packages:\n  - ./packages/mytool\n  - ./packages/htop\nhostname: x\n", SYSTEM_FILE, &d).unwrap();
    assert!(c.packages.contains(&"./packages/mytool".to_string()) && c.packages.contains(&"htop".to_string()), "{:?}", c.packages);
    // What a package sets up is for a computer nami makes.
    let os = c.os.as_ref().unwrap();
    assert_eq!(os.files["/etc/htoprc"], Value::from("y"));
    assert_eq!(os.files["/etc/mytool.conf"], Value::from("x"));
    assert_eq!(os.services["mytool"]["run"], Value::from("mytool serve"));
    assert!(c.files.is_empty() && c.services.is_empty(), "{:?} {:?}", c.files, c.services);
    let e = load_text("recipes: ./packages\n", USER_FILE, &d).unwrap_err().to_string();
    assert!(e.contains("recipes: is gone: list your own packages by path"), "{e}");
    std::fs::remove_dir_all(&d).unwrap();
}

/// The example packages (examples/packages/<name>): name, its folder, its package.nami.
pub(crate) fn examples() -> Vec<(String, String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/packages");
    let mut out: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().filter_map(|e| {
        let text = std::fs::read_to_string(e.path().join("package.nami")).ok()?;
        Some((e.file_name().to_string_lossy().into_owned(), e.path().display().to_string(), text))
    }).collect();
    out.sort();
    out
}

/// An example package's folder, as a system.nami lists it.
pub(crate) fn ex(name: &str) -> String {
    format!("{}/examples/packages/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Whether namiOS's own example packages (namios, namios-*) are here: they
/// are in this repo, not in nami's public one, whose builds have none.
pub(crate) fn namios_examples() -> bool {
    crate::new::is_package("namios-bare")
}

/// A whole computer to test with: namiOS barebones, or niri where namiOS's
/// own packages aren't.
pub(crate) fn a_computer() -> &'static str {
    if namios_examples() { "namios-bare" } else { "niri" }
}

/// The example packages only add: none says what must be one.
#[test]
fn packages_only_add() {
    for (name, _, text) in examples() {
        let v: Value = serde_yaml::from_str(&text).unwrap();
        for k in SINGLE_KEYS {
            assert!(v.get(*k).is_none(), "packages/{name}/package.nami sets {k}");
        }
    }
}

#[test]
fn a_folder_per_computer() {
    let d = std::env::temp_dir().join(format!("nami-computers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("laptop")).unwrap();
    std::fs::create_dir_all(d.join("files")).unwrap();
    std::fs::create_dir_all(d.join(".git")).unwrap();
    // A repo that is one OS: its files at the top.
    std::fs::write(d.join(SYSTEM_FILE), "packages:\n  - htop\n").unwrap();
    assert_eq!(computer_dir(&d, None, None).unwrap(), d);
    std::fs::remove_file(d.join(SYSTEM_FILE)).unwrap();
    std::fs::create_dir_all(d.join(SHARED_DIR)).unwrap();
    std::fs::write(d.join(SHARED_DIR).join("me.nami"), "packages:\n  - jq\n").unwrap();
    std::fs::create_dir_all(d.join("laptop/ada")).unwrap();
    std::fs::write(person_file(&d.join("laptop"), "ada"), "packages:\n  - ../../shared/me.nami\n").unwrap();
    std::fs::write(d.join("laptop").join(SYSTEM_FILE), "hostname: laptop\n").unwrap();
    assert_eq!(people(&d.join("laptop")), ["ada"]);
    assert_eq!(computers(&d), ["laptop"]);
    assert_eq!(computer_dir(&d, None, None).unwrap(), d.join("laptop"));
    std::fs::create_dir_all(d.join("desktop")).unwrap();
    std::fs::write(d.join("desktop").join(SYSTEM_FILE), "hostname: pc\n").unwrap();
    // A computer with only people's files (nami doesn't manage the computer).
    std::fs::create_dir_all(d.join("work/ada")).unwrap();
    std::fs::write(person_file(&d.join("work"), "ada"), "packages:\n  - jq\n").unwrap();
    assert_eq!(computers(&d), ["desktop", "laptop", "work"]);
    assert_eq!(computer_dir(&d, None, Some("laptop")).unwrap(), d.join("laptop"));
    assert_eq!(computer_dir(&d, Some("desktop"), Some("laptop")).unwrap(), d.join("desktop"));
    let e = computer_dir(&d, None, Some("other")).unwrap_err().to_string();
    assert!(e.contains("several computers (desktop, laptop, work): none is named after this one (other)"), "{e}");
    let e = computer_dir(&d, Some("server"), None).unwrap_err().to_string();
    assert!(e.contains("no computer server there"), "{e}");
    let user = check("packages:\n  - ../../shared/me.nami\n", USER_FILE, &d.join("laptop/ada"), None);
    assert!(user.iter().all(|x| x.severity != Severity::Error), "{user:?}");
    let at = |c: &str| check(&std::fs::read_to_string(d.join(c).join(SYSTEM_FILE)).unwrap(), SYSTEM_FILE, &d.join(c), None);
    assert!(at("laptop").iter().all(|x| x.severity != Severity::Error), "{:?}", at("laptop"));
    assert!(at("desktop").iter().any(|x| x.message.contains("desktop/system.nami says hostname: pc: a computer's folder is named after it")), "{:?}", at("desktop"));
    // A USB drive of laptop has what ada's file lists too.
    let used = used(&d.join("laptop").join(SYSTEM_FILE)).unwrap();
    assert!(used.folders.contains(&d.join(SHARED_DIR).canonicalize().unwrap()), "{:?}", used.folders);
    std::fs::remove_dir_all(&d).unwrap();
}

use super::*;

#[test]
fn updates_manual_keeps_the_daily_timer_off() {
    let base = Path::new("/");
    let mask = |text: &str| load_text(text, SYSTEM_FILE, base).unwrap().os.map(|o| o.mask).unwrap_or_default();
    assert_eq!(mask("packages: []\nupdates: manual\n"), [UPDATE_TIMER]);
    assert!(mask("packages: []\nupdates: auto\n").is_empty());
    assert!(mask("packages: []\n").is_empty());
    // Said both ways: once.
    assert_eq!(mask("updates: manual\nwithout: [nami-update.timer]\n"), [UPDATE_TIMER]);
    // Anything else is said plainly, at its line.
    let e = load_text("updates: sometimes\n", SYSTEM_FILE, base).unwrap_err();
    assert!(format!("{e:#}").contains("updates: auto or manual, not sometimes"), "{e:#}");
    let d = check("packages: []\nupdates: false\n", SYSTEM_FILE, base, None);
    assert!(d.iter().any(|d| d.line == 2 && d.severity == Severity::Error && d.message.contains("auto or manual")), "{d:?}");
    assert!(check("packages: []\nupdates: manual\n", SYSTEM_FILE, base, None).iter().all(|d| d.severity != Severity::Error));
    // The computer's alone, and it works on another distribution too: no "only on namiOS".
    assert!(load_text("updates: manual\n", USER_FILE, base).is_err());
    assert!(not_on_this_distro("packages: []\nupdates: manual\n", Some("Ubuntu"), None).is_empty());
    assert_eq!(only_on_nami_os("updates: manual\nhostname: a\n").as_deref(), Some("hostname:"));
}

#[test]
fn whether_the_computer_updates_itself() {
    assert!(updates_itself(""));
    assert!(updates_itself("packages: [vlc]\n"));
    assert!(updates_itself("updates: auto\n"));
    assert!(!updates_itself("updates: manual\n"));
    // The way earlier versions turned it off still counts.
    assert!(!updates_itself("without:\n  - nami-update.timer\n"));
    assert!(updates_itself("without: [printing]\n"));
    // A file that can't be read leaves it on.
    assert!(updates_itself("updates: [\n"));
}

#[test]
fn edits_versioned_entries() {
    let edit = |t: &str, ops: Vec<crate::config_edit::Op>| ops.iter().fold(t.to_string(), |t, op| crate::config_edit::apply(&t, op).unwrap());
    let text = "packages:\n  - firefox: 150   # for work\n  - jq\n";
    let listed = listed_packages(text);
    assert_eq!(listed.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["firefox@150", "jq"]);
    let removed = edit(text, remove_package(text, "firefox"));
    assert_eq!(removed, "packages:\n  - jq\n");
    let added = edit(&removed, add_package(&removed, "firefox@150").unwrap());
    assert_eq!(added, "packages:\n  - jq\n  - firefox: 150\n");
    // Another version: in place of the one there.
    let moved = edit(&added, add_package(&added, "firefox@151").unwrap());
    assert_eq!(listed_packages(&moved).iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["jq", "firefox@151"], "{moved}");
    // A package with its own setup under it: listed, and its version is changed there.
    let t = "packages:\n  - git:\n      version: 2.50\n      ~/.gitconfig: { user: { name: A } }\n  - jq\n";
    assert_eq!(listed_packages(t)[0].0, "git@2.50");
    assert!(add_package(t, "git@2.51").is_err());
    assert_eq!(edit(t, remove_package(t, "git")), "packages:\n  - jq\n");
}

/// A package folder is the same package by its path or its name: `nami
/// remove nami-gui` takes out `./packages/nami-gui` (as the installer used to
/// write it), and `nami install nami-gui` doesn't list it twice.
#[test]
fn a_package_folder_by_its_path_or_its_name() {
    let edit = |t: &str, ops: Vec<crate::config_edit::Op>| ops.iter().fold(t.to_string(), |t, op| crate::config_edit::apply(&t, op).unwrap());
    let dir = std::env::temp_dir().join(format!("nami-same-package-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("packages/nami-gui")).unwrap();
    std::fs::write(dir.join("packages/nami-gui/package.nami"), "name: nami-gui\n").unwrap();
    let by_path = "packages:\n  - jq\n  - ./packages/nami-gui\n";
    assert!(same_package("./packages/nami-gui", "nami-gui", &dir));
    assert!(same_package("nami-gui", "./packages/nami-gui", &dir));
    assert!(same_package(&dir.join("packages/nami-gui").display().to_string(), "nami-gui", &dir));
    assert_eq!(edit(by_path, remove_package_in(by_path, "nami-gui", &dir)), "packages:\n  - jq\n");
    let by_name = "packages:\n  - jq\n  - nami-gui\n";
    assert_eq!(edit(by_name, remove_package_in(by_name, "./packages/nami-gui", &dir)), "packages:\n  - jq\n");
    // Another version of a catalog's package is another entry to install (in its place).
    assert!(!same_package("firefox@150", "firefox@151", &dir) && same_package("firefox@150", "firefox@150", &dir));
    let fx = "packages:\n  - firefox: 150\n";
    assert_eq!(remove_package_in(fx, "firefox", &dir).len(), 1);
    // Without a folder of that name, a name is the catalog's alone.
    assert!(!same_package("./packages/nami-gui", "jq", &dir));
    assert!(remove_package_in(by_path, "gui", &dir).is_empty());
    std::fs::remove_dir_all(dir.join("packages/nami-gui")).unwrap();
    assert!(!same_package("./packages/nami-gui", "nami-gui", &dir));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn versions_are_the_text_written_however_written() {
    let here = Path::new(".");
    for text in [
        "packages: [{python3: 3.10}]\n",
        "packages:\n  - \"python3\": 3.10\n",
        "files:\n  ~/notes: { python3: 3.12 }\npackages:\n  - python3: 3.10\n",
        "packages:\n  - python3:\n      version: 3.10\n",
    ] {
        let c = load_text(text, "user.nami", here).unwrap();
        assert_eq!(c.packages, ["python3@3.10"], "{text}");
    }
    // Two versions of one: said as that, not as one listed twice.
    let said = check("packages:\n  - python3: 3.10\n  - python3: 3.12\n", "user.nami", here, None);
    assert_eq!(said.len(), 1, "{:?}", said.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert!(said[0].message.contains("two versions, 3.10 and 3.12"), "{}", said[0].message);
}

#[test]
fn a_recipes_version_is_the_text_written() {
    let text = "name: hello\nversion: 1.10\nsource: { url: https://example.org/hello-1.10.tar.gz, sha256: ab }\n";
    let (recipe, _) = crate::build::split_package(text).unwrap();
    let recipe: crate::build::Recipe = serde_yaml::from_value(Value::Mapping(recipe.unwrap())).unwrap();
    assert_eq!(recipe.version, "1.10");
    assert!(check(text, "packages/hello/package.nami", Path::new("."), None).is_empty());
}

#[test]
fn env_is_the_text_written_in_every_file() {
    let env = "env:\n  PORT: 8080\n  V: 3.10\n  DEBUG: true\n";
    let want = [("DEBUG", "true"), ("PORT", "8080"), ("V", "3.10")];
    let pairs = |m: &std::collections::BTreeMap<String, String>| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>();
    let want: Vec<(String, String)> = want.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let user = load_text(env, "user.nami", Path::new(".")).unwrap();
    assert_eq!(pairs(&user.home.unwrap().env), want);
    let computer = load_text(env, "system.nami", Path::new(".")).unwrap();
    assert_eq!(pairs(&computer.os.unwrap().environment), want);
    let dir = std::env::temp_dir().join(format!("nami-env-text-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.nami"), format!("packages: [jq]\n{env}")).unwrap();
    let project = crate::project::read(&dir.join("project.nami")).unwrap();
    assert_eq!(pairs(&project.env), want);
    for name in ["user.nami", "system.nami"] {
        assert!(check(env, name, Path::new("."), None).is_empty(), "{name}");
    }
    assert!(check(&format!("packages: [jq]\n{env}tasks: {{ t: echo }}\n"), "project.nami", &dir, None).is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn config_edits_keep_versions_and_env_as_text() {
    use crate::config_edit::{apply, Op};
    let p = |s: &str| s.split('.').map(String::from).collect::<Vec<_>>();
    let v = |path: &str, item: bool, t: &str| crate::yaml_text::at(&p(path), item, t).unwrap();
    let t = apply("env:\n  A: b\n", &Op::Set { path: p("env.V"), value: v("env.V", false, "3.10") }).unwrap();
    assert_eq!(load_text(&t, "user.nami", Path::new(".")).unwrap().home.unwrap().env["V"], "3.10");
    let t = apply("env:\n  A: b\n", &Op::Set { path: p("env.PORT"), value: v("env.PORT", false, "8080") }).unwrap();
    assert!(t.contains("PORT:"), "{t}");
    let flow = "packages: [jq, {python3: 3.10}]\n";
    let t = apply(flow, &Op::Add { path: p("packages"), value: Value::from("git") }).unwrap();
    assert_eq!(load_text(&t, "user.nami", Path::new(".")).unwrap().packages, ["jq", "python3@3.10", "git"]);
    let t = apply("packages:\n  - jq\n  - python3: 3.10\n", &Op::Remove { path: p("packages"), value: v("packages", true, "python3: 3.10") }).unwrap();
    assert_eq!(t, "packages:\n  - jq\n");
}

#[test]
fn versioned_packages() {
    let c = load_text("packages:\n  - firefox: 150\n  - python3: 3.10   # as written, not 3.1\n  - jq@1.7\n  - git\n  - htop: \"3.4\"\n", "t", Path::new(".")).unwrap();
    assert_eq!(c.packages, ["firefox@150", "python3@3.10", "jq@1.7", "git", "htop@3.4"]);
    assert_eq!(c.os.unwrap().packages[4], "htop@3.4");
    assert!(load_text("packages:\n  - firefox: [1]\n", "t", Path::new(".")).is_err());
}

#[test]
fn a_python_with_its_packages() {
    let c = load_text("packages:\n  - jq\n  - python3:\n      version: 3.13\n      with: [requests, numpy]\n  - python314: { with: [django] }\n  - python3: { with: [] }\n", "t", Path::new(".")).unwrap();
    assert_eq!(c.packages, ["jq", "python313 with numpy, requests", "python314 with django", "python3"]);
    let err = |text: &str| load_text(text, "t", Path::new(".")).unwrap_err().to_string();
    assert!(err("packages:\n  - python3: 3.13\n    with: [numpy]\n").contains("with: goes under it, with its version: `- python3: { version: 3.13, with: [numpy] }`"));
    assert!(err("packages:\n  - nodejs: { with: [typescript] }\n").contains("with: is for a Python"));
    assert!(err("packages:\n  - python3: { with: { numpy: 1 } }\n").contains("a list of its packages' names"));
    // Taken out by its name, as any package is.
    let text = "packages:\n  - jq\n  - python3:\n      version: 3.13\n      with: [numpy]\n";
    assert_eq!(listed_packages(text).iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["jq", "python3@3.13"]);
    assert_eq!(remove_package(text, "python3").len(), 1);

    // nami check: a package its set doesn't have, with what it might be.
    let tmp = std::env::temp_dir().join(format!("nami-python-check-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join("nix/var/nami")).unwrap();
    let out = |n: &str| format!("{{\"name\":\"{n}-1\",\"outputs\":{{\"out\":\"/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-{n}-1\"}}}}");
    let names = ["python3", "python313", "python312", "python313Packages.numpy", "python313Packages.requests", "python3Packages.numpy", "ripgrep"];
    let json = names.iter().map(|n| format!("\"{n}\":{}", out(n))).collect::<Vec<_>>().join(",");
    std::fs::write(tmp.join("nix/var/nami/index.json"), format!("{{{json}}}")).unwrap();
    let store = crate::store::Store::open_direct_readonly(&tmp).unwrap();
    let index = crate::index::Index::load(&store).unwrap();
    let said = |text: &str| check(text, "user.nami", &tmp, Some(&index)).into_iter().map(|d| (d.line, d.message, d.hint.unwrap_or_default())).collect::<Vec<_>>();
    assert!(said("packages:\n  - python3:\n      version: 3.13\n      with: [numpy, requests]\n").is_empty());
    let d = said("packages:\n  - jq\n  - python3:\n      version: 3.13\n      with: [numpyy, ripgrep, nothing-like-it]\n");
    assert!(d.contains(&(3, "numpyy isn't in python313Packages".into(), "did you mean `numpy`?".into())), "{d:?}");
    assert!(d.iter().any(|(_, m, h)| m == "ripgrep isn't in python313Packages" && h.contains("a program, not a Python package")), "{d:?}");
    assert!(d.iter().any(|(_, m, h)| m == "nothing-like-it isn't in python313Packages" && h.contains("nami search nothing-like-it --all")), "{d:?}");
    let d = said("packages:\n  - python3: { version: 3.12, with: [numpy] }\n");
    assert_eq!(d, [(2, "the catalog has no packages for python312 (python312Packages)".into(), "the catalog has packages for python3, python313".into())]);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn check_catches_the_typos_a_file_can_have() {
    let tmp = std::env::temp_dir().join(format!("nami-typos-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join("nix/var/nami")).unwrap();
    let out = |n: &str| format!("{{\"name\":\"{n}-1\",\"outputs\":{{\"out\":\"/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-{n}-1\"}}");
    std::fs::write(tmp.join("nix/var/nami/index.json"), format!("{{\"htop\":{}}},\"steam\":{},\"unfree\":true}}}}", out("htop"), out("steam"))).unwrap();
    let store = crate::store::Store::open_direct_readonly(&tmp).unwrap();
    let index = crate::index::Index::load(&store).unwrap();
    let said = |text: &str, name: &str| check(text, name, &tmp, Some(&index)).into_iter().map(|d| (d.line, d.message)).collect::<Vec<_>>();
    // Software that isn't free, where the file doesn't allow it.
    let d = said("unfree: false\npackages: [htop, steam]\n", "system.nami");
    assert!(d.iter().any(|(l, m)| *l == 2 && m == "steam isn't free software, and this file says unfree: false"), "{d:?}");
    // A package left out that there isn't.
    let d = said("packages: [htop]\nwithout: [plymuth]\n", "system.nami");
    assert!(d.iter().any(|(l, m)| *l == 2 && m == "without: plymuth isn't a package here"), "{d:?}");
    // A person's service runs as them, in any user.nami.
    let d = said("services:\n  - backup:\n      run: restic backup\n      user: root\n", "user.nami");
    assert!(d.iter().any(|(_, m)| m.contains("your services run as you")), "{d:?}");
    // namiOS's settings, by name.
    let d = said("namios:\n  dok: { size: small }\n", "user.nami");
    assert!(d.iter().any(|(l, m)| *l == 2 && m == "namios: dok: not a setting namiOS's desktop has"), "{d:?}");
    // A language, a time zone and a keyboard there are.
    let d = said("language: dansk\n", "system.nami");
    assert!(d.iter().any(|(l, m)| *l == 1 && m.starts_with("language: dansk isn't")), "{d:?}");
    if time_zones().is_some() {
        let d = said("timezone: Europe/Copenhagn\n", "system.nami");
        assert!(d.iter().any(|(l, m)| *l == 1 && m == "timezone: Europe/Copenhagn isn't a time zone"), "{d:?}");
        assert!(said("timezone: Europe/Copenhagen\n", "system.nami").iter().all(|(_, m)| !m.starts_with("timezone")));
        assert!(said("timezone: auto\n", "system.nami").iter().all(|(_, m)| !m.starts_with("timezone")));
    }
    if keyboard_layouts().is_some() {
        let d = said("keyboard: dkk\n", "system.nami");
        assert!(d.iter().any(|(_, m)| m == "keyboard: dkk isn't a keyboard layout"), "{d:?}");
        assert!(said("keyboard: us(dvorak)\n", "system.nami").iter().all(|(_, m)| !m.starts_with("keyboard")));
    }
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn messages_are_in_the_files_words() {
    assert_eq!(plain_message("os.users.ada.hours: x"), "users.ada.hours: x");
    assert_eq!(plain_message("home.env.TOKEN: can't open"), "env.TOKEN: can't open");
    assert_eq!(plain_message("user.nami: home: invalid type: boolean `true`, expected a string"), "user.nami: text, not true");
    assert_eq!(plain_message("history: invalid type: string \"ten\", expected usize"), "history: a whole number, not the text \"ten\"");
    assert_eq!(plain_message("unknown variant `sometimes`, expected one of `no`, `always`"), "sometimes isn't one of no, always");
    assert_eq!(plain_message("missing field `run`"), "run: is needed");
    // One yes/no rule: true or false, and a hint for the words YAML reads as text.
    assert_eq!(plain_message("unfree: invalid type: string \"no\", expected a boolean"), "unfree: true or false: write false (in YAML, no is text)");
    let d = check("unfree: no\n", "system.nami", Path::new("."), None);
    assert_eq!((d.len(), d[0].line), (1, 1), "{:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert!(d[0].message.contains("write false"), "{}", d[0].message);
    let d = check("namios:\n  dock:\n    on: yes\n", "user.nami", Path::new("."), None);
    assert!(d.iter().any(|d| d.message == "namios: dock.on: yes is text, not true or false"), "{:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    // Words that only start so stay.
    assert_eq!(plain_message("namios.local and photos.app"), "namios.local and photos.app");
    // A message about no one line names no line.
    let d = Diagnostic { line: 0, column: 0, severity: Severity::Note, message: String::new(), hint: None };
    assert_eq!(d.at("user.nami"), "user.nami");
}

#[test]
fn packages_no_cache_has() {
    let tmp = std::env::temp_dir().join(format!("nami-uncached-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join("nix/var/nami")).unwrap();
    let out = |n: &str| format!("{{\"name\":\"{n}-1\",\"outputs\":{{\"out\":\"/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-{n}-1\"}}");
    let index = format!(
        "{{\"htop\":{}}},\"steam\":{},\"unfree\":true,\"closure\":\"x\"}},\"steam-run\":{},\"uncached\":true}},\"lutris\":{},\"uncached\":true,\"closure\":\"y\"}}}}",
        out("htop"), out("steam"), out("steam-run"), out("lutris")
    );
    std::fs::write(tmp.join("nix/var/nami/index.json"), index).unwrap();
    let store = crate::store::Store::open_direct_readonly(&tmp).unwrap();
    let index = crate::index::Index::load(&store).unwrap();
    let said = check("packages: [htop, steam, steam-run, lutris]\n", SYSTEM_FILE, &tmp, Some(&index));
    let said: Vec<String> = said.iter().map(|d| format!("{:?} {}", d.severity, d.message)).collect();
    // A Mac builds nothing: what the catalog marks as not in the cache is left out.
    if crate::index::MAC {
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[0].starts_with("Warning steam-run: it'll be left out: it isn't ready-built for Mac"), "{said:?}");
        assert!(said[1].starts_with("Warning lutris: it'll be left out"), "{said:?}");
        std::fs::remove_dir_all(&tmp).unwrap();
        return;
    }
    // Built here, or left out: said before anything is built.
    assert_eq!(said.len(), 3, "{said:?}");
    assert!(said[0].starts_with("Note steam: built on this computer"), "{said:?}");
    assert!(said[1].starts_with("Warning steam-run: it'll be left out"), "{said:?}");
    assert!(said[2].starts_with("Note lutris: built on this computer"), "{said:?}");
    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn package_nami_keys() {
    let dir = std::env::temp_dir().join(format!("nami-package-keys-{}", std::process::id()));
    let pkg = dir.join("packages/mydesk");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        pkg.join("package.nami"),
        "packages: [gdm, htop]\nenv: { A: b }\nwithout: [systemd-networkd.service, nano]\nlogin:\n  mytool: su\n  all: [auth optional pam_x.so]\nsetuid: [bin/mytool]\n\
             boot:\n  microcode: [intel-ucode.img]\n  initrd-modules: [dm_crypt]\n  kernel-options: [pcie_aspm=off]\ndrivers:\n  firmware: [linux-firmware]\n  graphics-32bit: [pkgsi686Linux.mesa]\n\
             files:\n  /etc/mytool.conf: x\n  /etc/systemd/system/mytool.service: |\n    [Service]\n  /etc/systemd/user/me.path: p\n  /etc/mytool/made:\n    tools: [glib.dev]\n    script: mkdir $out\n",
    )
    .unwrap();
    std::fs::write(pkg.join("checks.nami"), "\"it runs\":\n  run: mytool --version\n  expect: mytool\n").unwrap();
    std::fs::create_dir_all(dir.join("packages/nvme-fix")).unwrap();
    std::fs::write(dir.join("packages/nvme-fix/package.nami"), "boot:\n  kernel-options: [nvme_core.default_ps_max_latency_us=0, pcie_aspm=off]\n").unwrap();
    let c = load_text("packages: [./packages/mydesk, ./packages/nvme-fix]\nusers: { a: { admin: true } }\n", SYSTEM_FILE, &dir).unwrap();
    // What a package sets up is for a computer nami makes: its files too.
    let os = c.os.unwrap();
    // The kernel's options: each package's, joined.
    assert!(os.kernel_options.contains(&"pcie_aspm=off".to_string()) && os.kernel_options.contains(&"nvme_core.default_ps_max_latency_us=0".to_string()), "{:?}", os.kernel_options);
    assert_eq!(os.graphics_32bit, ["pkgsi686Linux.mesa"]);
    for bad in ["tray.root=UUID=x", "root=/dev/sda1", "\"a b\""] {
        std::fs::write(dir.join("packages/nvme-fix/package.nami"), format!("boot:\n  kernel-options: [{bad}]\n")).unwrap();
        let e = load_text("packages: [./packages/nvme-fix]\n", SYSTEM_FILE, &dir).unwrap_err();
        assert!(format!("{e:#}").contains("kernel-options"), "{bad}: {e:#}");
    }
    assert!(os.files.iter().any(|(k, _)| k == "/etc/mytool.conf") && c.files.is_empty());
    assert_eq!(os.environment["A"], "b");
    assert_eq!(os.mask, ["systemd-networkd.service"]);
    assert_eq!(os.without, ["nano"]);
    assert_eq!(os.login_services["mytool"], "su");
    assert_eq!(os.login, ["auth optional pam_x.so"]);
    assert_eq!(os.wrappers, ["4755 bin/mytool"]);
    assert_eq!((os.early_boot.clone(), os.initrd_modules.clone(), os.firmware.clone()), (vec!["intel-ucode.img".to_string()], vec!["dm_crypt".to_string()], vec!["linux-firmware".to_string()]));
    // (gdm, which it lists, brings a drop-in of its own.)
    assert!(os.units.contains_key("mytool.service") && os.units.contains_key("user/me.path"));
    assert_eq!(os.build["/etc/mytool/made"], "mkdir $out");
    assert_eq!(os.build_tools, ["glib.dev"]);
    assert!(!os.files.contains_key("/etc/systemd/system/mytool.service"));
    assert_eq!(os.checks["it runs"].run, "mytool --version");
    // It lists a login screen: it's a desktop.
    assert_eq!(os.desktop, ["mydesk"]);
    // What's not for a file says what it is now, or where it goes.
    let e = |text: &str, name: &str| load_text(text, name, &dir).unwrap_err().to_string();
    assert!(e("units: { a.service: x }\n", SYSTEM_FILE).contains("files: /etc/systemd/system/<name>"));
    assert!(e("setuid: [bin/sudo]\n", SYSTEM_FILE).contains("is a package's"));
    assert!(e("desktop: [x]\n", "packages/x/package.nami").contains("is a desktop by itself"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn option_examples_are_valid() {
    // The files they name, next to them.
    let dir = std::env::temp_dir().join(format!("nami-options-{}/laptop", std::process::id()));
    std::fs::create_dir_all(dir.join("nami")).unwrap();
    // The example packages they list, copied next to it as `nami new` does.
    let copied: Vec<String> = ["namios", "tailscale"].into_iter().filter(|p| crate::new::is_package(p)).map(String::from).collect();
    crate::new::copy_packages(&dir, &copied).unwrap();
    std::fs::create_dir_all(dir.join("recipes/my-tool")).unwrap();
    std::fs::create_dir_all(dir.join("dotfiles/nvim")).unwrap();
    std::fs::write(dir.join("recipes/my-tool/package.nami"), "name: my-tool\ninstall: mkdir $out\n").unwrap();
    std::fs::write(dir.join("work.nami"), "packages: [jq]\n").unwrap();
    std::fs::write(dir.join("nami/common.nami"), "env: { A: b }\n").unwrap();
    std::fs::create_dir_all(dir.join("services")).unwrap();
    for s in ["postgres", "redis", "mailpit"] {
        std::fs::copy(format!("{}/examples/services/postgres.nami", env!("CARGO_MANIFEST_DIR")), dir.join("services/testdb.nami")).unwrap();
        std::fs::copy(format!("{}/examples/services/{s}.nami", env!("CARGO_MANIFEST_DIR")), dir.join(format!("services/{s}.nami"))).unwrap();
    }
    for o in OPTIONS {
        // One that lists namiOS's packages, where they aren't.
        if !namios_examples() && o.example.contains("namios") {
            continue;
        }
        if o.key.contains("project") {
            let example = o.example.replace("  - github:acme/dev/node-service.nami\n", "");
            let d = check_project(&example, &dir, None);
            assert!(d.is_empty(), "{}: {:?}", o.key, d.iter().map(|d| &d.message).collect::<Vec<_>>());
            continue;
        }
        // A recipe's keys: a package.nami's, read as `nami check` reads one.
        if o.key.contains("a recipe") {
            let d = check(o.example, "packages/example/package.nami", &dir, None);
            assert!(d.is_empty(), "{}: {:?}", o.key, d.iter().map(|d| &d.message).collect::<Vec<_>>());
            continue;
        }
        if o.key.starts_with("checks.nami") {
            let checks: std::collections::BTreeMap<String, crate::config::computer::Check> = serde_yaml::from_str(o.example).unwrap();
            assert!(!checks.is_empty(), "{}", o.key);
            continue;
        }
        let name = if o.key.contains("user.nami") { USER_FILE } else if o.key.contains("package.nami") { "packages/example/package.nami" } else { SYSTEM_FILE };
        if let Err(e) = load_text(o.example, name, &dir) {
            panic!("{}: {e}", o.key);
        }
        // (Off namiOS, an example of its desktop says it does nothing here: that's right.)
        let d: Vec<_> = check(o.example, name, &dir, None).into_iter().filter(|d| !d.message.starts_with("namios: is namiOS's desktop;")).collect();
        assert!(d.is_empty(),
 "{}: {:?}", o.key, d.iter().map(|d| &d.message).collect::<Vec<_>>());
    }
    std::fs::remove_dir_all(dir.parent().unwrap()).ok();
}

#[test]
fn without_leaves_out_a_folder_and_its_setup() {
    let ex = |p: &str| format!("{}/examples/packages/{p}", env!("CARGO_MANIFEST_DIR"));
    let without = load_text(&format!("packages: [{}]\nhostname: x\nwithout: [plymouth, printing]\n", ex(a_computer())), "system.nami", Path::new(".")).unwrap();
    let with = load_text(&format!("packages: [{}]\nhostname: x\n", ex(a_computer())), "system.nami", Path::new(".")).unwrap();
    let (a, b) = (with.os.unwrap(), without.os.unwrap());
    assert!(a.packages.iter().any(|p| p == "plymouth") && a.packages.iter().any(|p| p == "cups"));
    // printing's folder isn't loaded (cups is its); the names go at build time.
    assert!(!b.packages.iter().any(|p| p == "cups" || p == "hplip"), "{:?}", b.packages);
    assert!(b.packages.iter().any(|p| p.ends_with("/niri")));
}

#[test]
fn a_missing_package_folder_is_said() {
    let e = format!("{:#}", load_text("packages: [../nowhere/namios, htop]\nhostname: x\n", "system.nami", Path::new(".")).unwrap_err());
    assert!(e.contains("no package at ../nowhere/namios"), "{e}");
}

#[test]
fn a_versioned_package_is_said_on_its_line() {
    let mut out = Vec::new();
    let p = ["nodejs@22".to_string(), "nodejs@22".to_string()];
    check_packages("packages:\n  - ripgrep\n  - nodejs: 22\n  - nodejs: 22\n", &p, Path::new("."), None, &mut out);
    assert_eq!(out.iter().map(|d| d.line).collect::<Vec<_>>(), [4], "{out:?}");
}

#[test]
fn flathub_apps_are_a_persons() {
    let text = "packages:\n  - htop\n  - flathub:org.gnome.Calculator\n";
    let errors = |tier| check_tier(text, Path::new("."), None, tier).into_iter().filter(|d| d.severity == Severity::Error).collect::<Vec<_>>();
    assert!(errors(Tier::User).is_empty(), "{:?}", errors(Tier::User));
    let d = errors(Tier::System);
    assert_eq!((d.len(), d[0].line), (1, 3), "{d:?}");
    assert!(d[0].message.contains("each person's own"), "{d:?}");
    // Named by its id, as Flatpak names it.
    let mut out = Vec::new();
    check_packages("packages:\n  - flathub:spotify\n", &["flathub:spotify".to_string()], Path::new("."), None, &mut out);
    assert_eq!(out.iter().map(|d| (d.line, d.severity)).collect::<Vec<_>>(), [(2, Severity::Error)], "{out:?}");
    // Not a project's tool.
    let p = check_project("packages:\n  - flathub:org.gnome.Calculator\n", Path::new("."), None);
    assert!(p.iter().any(|d| d.severity == Severity::Error && d.message.contains("not a project's tool")), "{p:?}");
    // A file with one loads, the entry as it's written.
    let c = load_text(text, "user.nami", Path::new(".")).unwrap();
    assert_eq!(c.packages, ["htop", "flathub:org.gnome.Calculator"]);
}

#[test]
fn casks_are_a_macs_and_never_a_projects() {
    // Anyone's (a person's or the computer's) on a Mac; on Linux, said to be left out.
    let text = "packages:\n  - htop\n  - cask:rectangle\n";
    for tier in [Tier::User, Tier::System] {
        let d = check_tier(text, Path::new("."), None, tier);
        assert!(!d.iter().any(|d| d.severity == Severity::Error), "{d:?}");
        if !crate::index::MAC {
            assert!(d.iter().any(|d| d.line == 3 && d.severity == Severity::Note && d.message == "cask:rectangle: a Mac app from Homebrew's casks, left out on Linux"), "{d:?}");
        }
    }
    // Named by its token, on Linux too (one file for both).
    let mut out = Vec::new();
    check_packages("packages:\n  - cask:Bad_Name\n", &["cask:Bad_Name".to_string()], Path::new("."), None, &mut out);
    assert_eq!(out.iter().map(|d| (d.line, d.severity)).collect::<Vec<_>>(), [(2, Severity::Error)], "{out:?}");
    let p = check_project("packages:\n  - cask:rectangle\n", Path::new("."), None);
    assert!(p.iter().any(|d| d.severity == Severity::Error && d.message.contains("not a project's tool")), "{p:?}");
    let c = load_text(text, "user.nami", Path::new(".")).unwrap();
    assert_eq!(c.packages, ["htop", "cask:rectangle"]);
    assert!(!is_plain_entry("cask:rectangle"));
}

#[test]
fn export_reads_back() {
    let m: Manifest = serde_json::from_value(serde_json::json!({ "generation": 3, "created": 0, "packages": [
        { "attr": "ripgrep", "name": "ripgrep-15", "paths": [] }, { "attr": "python3@3.12", "name": "python3-3.12.4", "paths": [] } ] })).unwrap();
    let c = load_text(&export(&m), "export", Path::new(".")).unwrap();
    assert_eq!(c.packages, ["ripgrep", "python3@3.12"]);
}

#[test]
fn wifi_in_a_system_file() {
    let text = concat!("packages: [htop]\nhostname: x\nwifi:\n  home: { password: !secret \"", sealed_example!(), "\" }\n  cafe: {}\nfiles:\n  /etc/x.conf: hi\n");
    let c = load_text(text, "t", Path::new(".")).unwrap();
    let home = &c.files["/run/NetworkManager/system-connections/home.nmconnection"];
    assert!(crate::secrets::contains(home) && home["reload"] == "nmcli connection reload" && home["mode"] == "0600", "{home:?}");
    assert!(!crate::secrets::contains(&c.files["/run/NetworkManager/system-connections/cafe.nmconnection"]));
    assert_eq!(c.files["/etc/x.conf"], "hi");
    // nami check knows it too, and says where a mistake is.
    assert!(check(text, "t", Path::new("."), None).is_empty(), "{:?}", check(text, "t", Path::new("."), None).iter().map(|d| &d.message).collect::<Vec<_>>());
    let d = check("hostname: x\n\nwifi:\n  home: { pasword: x }\n", "t", Path::new("."), None);
    assert!(d.len() == 1 && d[0].line == 3 && d[0].message.contains("wifi.home: pasword: did you mean password?"), "{:?}", d.iter().map(|d| (d.line, &d.message)).collect::<Vec<_>>());
    // A secret is a network's password, nothing else of it.
    let bad = concat!("hostname: x\nwifi:\n  home: { identity: !secret \"", sealed_example!(), "\" }\n");
    assert!(load_text(bad, "t", Path::new(".")).unwrap_err().to_string().contains("wifi.home.identity: a secret can't go here"));
}

/// Layers from a git repo: pinned the first time, kept at that commit
/// after the repo moves on, refused unpinned with --locked, moved by an
/// update; names in a layer are its repo's first, then nami's.
#[test]
fn layers_from_a_repo() {
    use crate::layers::{set_mode, Mode};
    use std::process::Command;
    if std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).all(|d| !d.join("git").is_file()) {
        return;
    }
    let d = std::env::temp_dir().join(format!("nami-layers-repo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("me")).unwrap();
    let d = d.canonicalize().unwrap();
    std::env::set_var("NAMI_LAYERS_DIR", d.join("cache"));
    let git = |dir: &Path, args: &[&str]| {
        let o = Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@x", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"]).args(args).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    git(&d, &["init", "-q", "--bare", "os.git"]);
    git(&d, &["clone", "-q", "os.git", "work"]);
    let work = d.join("work");
    std::fs::write(work.join("default.nami"), "packages: [./extra.nami, jq]\n").unwrap();
    std::fs::write(work.join("extra.nami"), "packages: [./gaming.nami, ripgrep]\n").unwrap();
    std::fs::write(work.join("gaming.nami"), "packages: [./packages/printing, steam]\n").unwrap();
    // Package folders of the repo's own: a set, and a copy of the tailscale example.
    std::fs::create_dir_all(work.join("packages/printing")).unwrap();
    std::fs::write(work.join("packages/printing/package.nami"), "packages: [cups]\n").unwrap();
    std::fs::create_dir_all(work.join("packages/tailscale")).unwrap();
    std::fs::copy(format!("{}/package.nami", ex("tailscale")), work.join("packages/tailscale/package.nami")).unwrap();
    std::fs::write(work.join("escape.nami"), "packages: [../../me/secret.nami]\n").unwrap();
    std::fs::write(work.join("net.nami"), "packages: [./packages/tailscale]\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-qm", "one"]);
    git(&work, &["push", "-q", "origin", "HEAD:main"]);
    let first = git(&work, &["rev-parse", "HEAD"]);
    let url = format!("git+file://{}", d.join("os.git").display());
    let me = d.join("me");
    let file = format!("packages: [{url}#default.nami, git]\n");
    // It builds on one file: two side by side is a mistake.
    let two = load_text(&format!("packages: [{url}#default.nami, {url}#gaming.nami]\n"), "system.nami", &d.join("me")).unwrap_err().to_string();
    assert!(two.contains("it builds on one file"), "{two}");
    let load = |text: &str| load_text(text, "system.nami", &me);
    let c = load(&file).unwrap();
    assert_eq!(c.packages, ["printing", "steam", "ripgrep", "jq", "git"]);
    assert!(c.os.is_some_and(|o| o.packages.iter().any(|p| p == "cups")), "printing is the repo's own package");
    let lock = crate::lock::load(&me.join("nami.lock")).unwrap();
    assert_eq!(lock.layers.len(), 1, "{:?}", lock.layers);
    assert_eq!(lock.layers[&url].rev, first);
    // The flat file says each package's layer, at its commit.
    let flat = flat_text(&file, "system.nami", &me).unwrap();
    assert!(flat.contains("  - git  # system.nami\n"), "{flat}");
    assert!(flat.contains(&format!("  - ripgrep  # {url}/extra.nami at {}\n", &first[..12])), "{flat}");
    assert!(flat.contains(&format!("  - steam  # {url}/gaming.nami at {}\n", &first[..12])), "{flat}");
    // The repo moves on; the computer doesn't, until it's asked to.
    std::fs::write(work.join("default.nami"), "packages: [./gaming.nami, fd]\n").unwrap();
    git(&work, &["commit", "-qam", "two"]);
    git(&work, &["push", "-q", "origin", "HEAD:main"]);
    assert_eq!(load(&file).unwrap().packages, ["printing", "steam", "ripgrep", "jq", "git"]);
    set_mode(Mode::Locked);
    let err = load(&format!("packages: [{url}#default.nami@main]\n")).unwrap_err().to_string();
    assert!(err.contains("isn't pinned"), "{err}");
    assert_eq!(load(&file).unwrap().packages, ["printing", "steam", "ripgrep", "jq", "git"], "pinned ones still work, offline too");
    set_mode(Mode::Update);
    assert_eq!(load(&file).unwrap().packages, ["printing", "steam", "fd", "git"]);
    set_mode(Mode::Pin);
    assert_eq!(crate::lock::load(&me.join("nami.lock")).unwrap().layers[&url].rev, git(&work, &["rev-parse", "HEAD"]));
    // A layer that brings nami's tailscale, and the file's own wifi:
    // and Tailscale settings, with secrets: no mistakes.
    let text = format!(concat!("packages: [{}#net.nami]\nhostname: x\nwifi:\n  home: {{ password: !secret \"", sealed_example!(), "\" }}\nfiles:\n  /etc/tailscale/up.conf: {{ set: {{ ssh: true }} }}\n  /run/nami/secrets/tailscale-auth-key: !secret \"", sealed_example!(), "\"\n"), url);
    let errs: Vec<_> = check_tier(&text, &me, None, Tier::System).into_iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:?}");
    // What goes wrong says what to do.
    let err = format!("{:#}", load(&format!("packages: [{url}#nope.nami]\n")).unwrap_err());
    assert!(err.contains("has no nope.nami") && err.contains("default, escape, extra, gaming, net"), "{err}");
    let err = format!("{:#}", load(&format!("packages: [git+file://{}/missing.git#default.nami]\n", d.display())).unwrap_err());
    assert!(err.contains("can't reach"), "{err}");
    std::fs::write(me.join("secret.nami"), "packages: [x]\n").unwrap();
    let err = format!("{:#}", load(&format!("packages: [{url}#escape.nami]\n")).unwrap_err());
    assert!(err.contains("files of that repo"), "{err}");

    // A project builds on a team's layer too (in the same test: the
    // pinning mode is the whole program's).
    std::fs::write(
        work.join("node.nami"),
        "packages: [./svc.nami, jq, ./tools/lint]\nenv: { MODE: layer, FROM_LAYER: \"yes\" }\ntasks: { test: layer-test, lint: lint . }\nservices:\n  - ./services/redis.nami: { version: 7 }\n  - worker: { run: sleep 1000, port: 9000 }\n",
    )
    .unwrap();
    std::fs::write(work.join("svc.nami"), "services:\n  - ./services/postgres.nami\n").unwrap();
    std::fs::create_dir_all(work.join("services")).unwrap();
    for s in ["postgres", "redis"] {
        std::fs::copy(format!("{}/examples/services/{s}.nami", env!("CARGO_MANIFEST_DIR")), work.join(format!("services/{s}.nami"))).unwrap();
    }
    std::fs::create_dir_all(work.join("tools/lint")).unwrap();
    std::fs::write(work.join("tools/lint/package.nami"), "name: lint\ninstall: mkdir $out\n").unwrap();
    std::fs::write(work.join("sealed.nami"), concat!("env:\n  TOKEN: !secret \"", sealed_example!(), "\"\n")).unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-qm", "three"]);
    git(&work, &["push", "-q", "origin", "HEAD:main"]);
    let third = git(&work, &["rev-parse", "HEAD"]);
    let proj = d.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("common.nami"), format!(concat!("packages: [{}#node.nami]\nenv:\n  COMMON: \"1\"\n  SHARED: !secret \"", sealed_example!(), "\"\n"), url)).unwrap();
    let pfile = proj.join("project.nami");
    std::fs::write(&pfile, format!("packages: [./common.nami, nodejs]\nenv: {{ MODE: mine }}\ntasks: {{ test: own-test }}\nservices:\n  - worker: {{ port: 9001 }}\n")).unwrap();
    set_mode(Mode::Pin);
    let (p, used) = crate::project::read_with_layers(&pfile).unwrap();
    assert_eq!(used.len(), 1);
    assert_eq!((p.packages[0].as_str(), p.packages[2].as_str()), ("jq", "nodejs"), "the set's, then the project's own");
    let lint = &p.packages[1];
    assert!(lint.starts_with(&d.join("cache").display().to_string()) && lint.ends_with("/tools/lint"), "the layer's recipe, in its repo: {lint}");
    assert_eq!((p.env["MODE"].as_str(), p.env["FROM_LAYER"].as_str(), p.env["COMMON"].as_str()), ("mine", "yes", "1"), "the project's own wins");
    assert!(crate::secrets::marked(&p.env["SHARED"]).is_some(), "a file of the project's may have its secrets");
    assert_eq!((p.tasks["test"].run.as_deref(), p.tasks["lint"].run.as_deref()), (Some("own-test"), Some("lint .")));
    let services = crate::up::resolve(&p.services, &proj).unwrap();
    let names: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["postgres", "redis", "worker"]);
    assert_eq!(p.services["worker"]["port"], 9001);
    assert_eq!(p.services["worker"]["run"], "sleep 1000");
    assert_eq!(crate::lock::load(&proj.join("nami.lock")).unwrap().layers[&url].rev, third, "pinned in the project's nami.lock");
    assert!(check_project(&std::fs::read_to_string(&pfile).unwrap(), &proj, None).is_empty());
    // The hook: what's pinned and fetched, never the network.
    set_mode(Mode::Offline);
    assert_eq!(crate::project::read(&pfile).unwrap().tasks["lint"].run.as_deref(), Some("lint ."));
    std::fs::write(proj.join("other.nami"), format!("packages: [{url}#default.nami@main]\n")).unwrap();
    std::fs::write(&pfile, format!("packages: [{url}#svc.nami@main]\n")).unwrap();
    let err = format!("{:#}", crate::project::read(&pfile).unwrap_err());
    assert!(err.contains("isn't pinned") && err.contains("`nami layers` (or `nami shell`)"), "{err}");
    std::fs::write(&pfile, format!("packages: [{url}#node.nami]\n")).unwrap();
    std::fs::rename(d.join("cache"), d.join("cache.away")).unwrap();
    let err = format!("{:#}", crate::project::read(&pfile).unwrap_err());
    assert!(err.contains("isn't fetched here yet"), "{err}");
    std::fs::rename(d.join("cache.away"), d.join("cache")).unwrap();
    set_mode(Mode::Pin);
    // No secrets from someone's repo.
    std::fs::write(&pfile, format!("packages: [{url}#sealed.nami]\n")).unwrap();
    let err = format!("{:#}", crate::project::read(&pfile).unwrap_err());
    assert!(err.contains("can't bring secrets into a project"), "{err}");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_value_of_the_wrong_kind_says_where() {
    let d = check("hostname: pc\nusers:\n  - ada\npackages:\n  - htop\n", "system.nami", Path::new("."), None);
    assert!(d.len() == 1 && d[0].line == 2 && d[0].message == "users: a map, not a list", "{:?}", d.iter().map(|d| (d.line, &d.message)).collect::<Vec<_>>());
}

#[test]
fn known_formats_are_settings() {
    let d = check("packages:\n  - git:\n      ~/.gitconfig: |\n        [user]\n        name = A\n", USER_FILE, Path::new("."), None);
    assert!(d.len() == 1 && d[0].message.contains("written as text") && d[0].hint.as_deref().is_some_and(|h| h.contains("source")), "{:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert!(check("files:\n  ~/.vimrc: |\n    set number\n  ~/.config/app.conf: |\n    a b\n", USER_FILE, Path::new("."), None).is_empty());
    // The example packages too.
    for (name, dir, text) in examples() {
        let d: Vec<_> = check(&text, &format!("{dir}/package.nami"), Path::new(&dir), None).into_iter().filter(|d| d.message.contains("written as text")).collect();
        assert!(d.is_empty(), "{name}: {:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    }
}

#[test]
fn a_packages_own_setup_is_under_it() {
    let text = "packages:\n  - ripgrep\n  - firefox: 150\n  - git:\n      ~/.gitconfig:\n        user: { name: A }\n  - gnome-text-editor:\n      org.gnome.TextEditor: { show-line-numbers: true }\n  - syncthing\n  - zsh:\n      login-shell: true\n      version: 5.9\nenv: { EDITOR: vi }\nservices:\n  - syncthing\n";
    let c = load_text(text, USER_FILE, Path::new(".")).unwrap();
    assert_eq!(c.packages, ["ripgrep", "firefox@150", "git", "gnome-text-editor", "syncthing", "zsh@5.9"]);
    let home = c.home.unwrap();
    assert_eq!(home.files[".gitconfig"]["user"]["name"], Value::from("A"));
    assert_eq!(home.login_shell.as_deref(), Some("zsh"));
    assert_eq!(c.settings.unwrap()["org.gnome.TextEditor"]["show-line-numbers"], Value::Bool(true));
    assert_eq!(c.services["syncthing"], Value::Bool(true));
    let errors: Vec<_> = check(text, USER_FILE, Path::new("."), None).into_iter().filter(|d| d.severity == Severity::Error).map(|d| d.message).collect();
    assert!(errors.is_empty(), "{errors:?}");
    // One place for each thing, and a list is the old way.
    let e = load_text("packages:\n  - git:\n      ~/.gitconfig: a\nfiles:\n  ~/.gitconfig: b\n", USER_FILE, Path::new(".")).unwrap_err().to_string();
    assert!(e.contains("~/.gitconfig is given twice"), "{e}");
    let e = load_text("packages:\n  jq:\n", USER_FILE, Path::new(".")).unwrap_err().to_string();
    assert!(e.contains("packages: a list, `- name` a line"), "{e}");
    let e = load_text("packages:\n  - git:\n      colour: auto\n", USER_FILE, Path::new(".")).unwrap_err().to_string();
    assert!(e.contains("git: colour: under a package go its files"), "{e}");
    // A service's own files are under it, and restart it.
    let c = load_text("services:\n  - adguardhome:\n      /etc/adguardhome.yaml:\n        dns: { port: 53 }\n  - syncthing\n", SYSTEM_FILE, Path::new(".")).unwrap();
    assert_eq!(c.files["/etc/adguardhome.yaml"]["dns"]["port"], Value::from(53));
    assert_eq!(c.services["adguardhome"]["restart-on"][0], Value::from("/etc/adguardhome.yaml"));
    let e = load_text("services:\n  - a:\n      /etc/x: hi\nfiles:\n  /etc/x: ho\n", SYSTEM_FILE, Path::new(".")).unwrap_err().to_string();
    assert!(e.contains("/etc/x is given twice (also under services: a)"), "{e}");
    let e = load_text("packages:\n  - syncthing:\n      service: true\n", USER_FILE, Path::new(".")).unwrap_err().to_string();
    assert!(e.contains("syncthing: services go under services: (`- syncthing`)"), "{e}");
}

#[test]
fn everything_is_a_package() {
    let d = std::env::temp_dir().join(format!("nami-sets-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::create_dir_all(d.join("desk")).unwrap();
    std::fs::write(d.join("desk/package.nami"), format!("packages: [{}, {}, nautilus]\n", ex("desktop"), ex("gdm"))).unwrap();
    std::fs::write(d.join("mine.nami"), "packages: [./desk, ./more.nami, htop]\nsettings:\n  a.b: { x: 1 }\n").unwrap();
    std::fs::write(d.join("more.nami"), "packages: [jq]\n").unwrap();
    // A set brings what it lists, and their sets theirs: desk (a
    // desktop of someone's own) brings desktop, which brings base.
    let c = load_text("packages: [./mine.nami, git]\nhostname: x\n", "system.nami", &d).unwrap();
    let os = c.os.unwrap();
    for p in ["systemd", "pipewire", "gdm", "nautilus", "jq", "htop", "git"] {
        assert!(os.packages.iter().any(|x| x == p), "no {p}");
    }
    let at = |p: &str| os.packages.iter().position(|x| x == p).unwrap();
    assert!(at("systemd") < at("gdm") && at("gdm") < at("htop"), "base's first, as its files win in sw/");
    assert_eq!(os.desktop, ["desk"]);
    assert_eq!(os.settings.unwrap()["a.b"]["x"], Value::from(1));
    // The file's own apps (what a computer nami didn't make installs):
    // its sets', not nami's.
    assert_eq!(c.packages, ["jq", "desk", "htop", "git"]);
    // Nothing comes in unlisted.
    let c = load_text("hostname: x\n", "system.nami", &d).unwrap();
    assert!(c.os.unwrap().packages.is_empty());
    // Each key in its file: a person's in user.nami, the computer's in system.nami.
    let e = load_text("hostname: x\n", USER_FILE, &d).unwrap_err().to_string();
    assert!(e.contains("hostname: is the computer's, in system.nami"), "{e}");
    let e = load_text("aliases: { ll: ls -l }\n", SYSTEM_FILE, &d).unwrap_err().to_string();
    assert!(e.contains("aliases: is a person's, in their ~/user.nami"), "{e}");
    let e = load_text("files: { /etc/x: hi }\n", USER_FILE, &d).unwrap_err().to_string();
    assert!(e.contains("a person's files are in their home, ~/"), "{e}");
    let c = load_text("files: { ~/.gitconfig: { user: { name: A } } }\nenv: { EDITOR: vi }\n", USER_FILE, &d).unwrap();
    let home = c.home.unwrap();
    assert!(home.files.contains_key(".gitconfig") && home.env["EDITOR"] == "vi");
    let e = load_text("pakages: [jq]\n", SYSTEM_FILE, &d).unwrap_err().to_string();
    assert!(e.contains("unknown key `pakages` (did you mean `packages`? or this file needs a newer nami (nami self-update))"), "{e}");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn tailscale_in_a_system_file() {
    let text = concat!(
        "packages: [TS, TSR]\nhostname: x\nfiles:\n  /etc/tailscale/up.conf:\n    up: { login-server: http://hs }\n    set:\n      hostname: laptop\n      advertise-routes: [10.0.0.0/24, 10.1.0.0/24]\n  /run/nami/secrets/tailscale-auth-key: !secret \"",
        sealed_example!(),
        "\"\n"
    );
    let text = &text.replace("[TS, TSR]", &format!("[{}, {}]", ex("tailscale"), ex("tailscale-router")));
    let c = load_text(text, "t", Path::new(".")).unwrap();
    let os = c.os.as_ref().unwrap();
    assert!(os.packages.iter().any(|p| p == "tailscale") && os.start.contains(&"tailscaled.service".to_string()) && os.start.contains(&"tailscale-up.service".to_string()), "{:?}", os.start);
    // The layer's file has the options, the system file's is its content.
    let files = merge_files(os.files.clone(), c.files.clone()).unwrap();
    let conf = &files["/etc/tailscale/up.conf"];
    assert_eq!(conf["restarts"], "tailscale-up.service");
    // INI repeats the routes; up.sh joins them.
    assert_eq!(crate::home::render("up.conf", &conf["content"]).unwrap(), "[up]\nlogin-server = http://hs\n[set]\nhostname = laptop\nadvertise-routes = 10.0.0.0/24\nadvertise-routes = 10.1.0.0/24\n");
    // A list before any section repeats its key too (backup.conf's leave-out).
    let v: serde_yaml::Value = serde_yaml::from_str("to: disk:Backups\nleave-out: [~/Downloads, ~/Videos]\nkeep: { daily: 7 }").unwrap();
    assert_eq!(crate::home::render("backup.conf", &v).unwrap(), "to = disk:Backups\nleave-out = ~/Downloads\nleave-out = ~/Videos\n[keep]\ndaily = 7\n");
    assert!(crate::secrets::contains(&files["/run/nami/secrets/tailscale-auth-key"]));
    // A subnet router forwards.
    assert!(files.contains_key("/etc/sysctl.d/60-tailscale.conf"));
    assert!(check(text, "t", Path::new("."), None).is_empty(), "{:?}", check(text, "t", Path::new("."), None).iter().map(|d| &d.message).collect::<Vec<_>>());
    // Just the package and its daemon: a login in the browser, no settings.
    let c = load_text(&format!("packages: [{}]\nhostname: x\n", ex("tailscale")), "t", Path::new(".")).unwrap();
    let os = c.os.unwrap();
    assert_eq!(os.files["/etc/tailscale/up.conf"]["content"], Value::Mapping(Default::default()));
    assert!(!os.files.contains_key("/etc/sysctl.d/60-tailscale.conf"));
    // (Package files are for a computer nami makes: elsewhere, nothing of theirs.)
    assert!(c.files.is_empty());
}

#[test]
fn files_and_lists_merge() {
    let y = |s: &str| serde_yaml::from_str::<Value>(s).unwrap();
    // Options from one file, the content from another: either way round.
    let under = y("files: { /etc/a.conf: { content: { s: { x: 1, y: 2 } }, restart: a.service }, /etc/b: { mode: dark } }");
    let over = y("files: { /etc/a.conf: { s: { y: 3 } }, /etc/b: { content: hi, mode: \"0600\" } }");
    let m = merge(under, over).unwrap();
    assert_eq!(m["files"]["/etc/a.conf"], y("{ content: { s: { x: 1, y: 3 } }, restart: a.service }"));
    assert_eq!(m["files"]["/etc/b"], y("{ content: hi, mode: \"0600\" }"), "text replaces settings");
    let m = merge(y("files: { /etc/c: { content: a, restart: c.service } }"), y("files: { /etc/c: { source: ./c, mode: \"0600\" } }")).unwrap();
    assert_eq!(m["files"]["/etc/c"], y("{ restart: c.service, source: ./c, mode: \"0600\" }"));
    // Elsewhere a map is just a map.
    assert_eq!(merge(y("x: { content: a, restart: b }"), y("x: { k: 1 }")).unwrap(), y("x: { content: a, restart: b, k: 1 }"));
    // A list and a single value: a mistake, said where.
    let e = merge(y("without: [a]"), y("without: b")).unwrap_err().to_string();
    assert!(e.contains("without: a list in one file, a single value in another"), "{e}");
    // `desktop: niri` is a list of one, and goes with niri's own.
    let c = load_text(&format!("packages: [{}]\ndesktop: niri\nhostname: x\n", ex(a_computer())), "t", Path::new(".")).unwrap();
    assert_eq!(c.os.unwrap().desktop, ["niri"]);
    // The example packages go together, each with everything it brings.
    for (name, dir, _) in examples() {
        if let Err(e) = load_text(&format!("packages: [{dir}]\nhostname: x\n"), "t", Path::new(".")) {
            panic!("{name}: {e:#}");
        }
    }
}

#[test]
fn docker_in_a_system_file() {
    // Plain keys: the docker group as os.users' groups, daemon.json as a file.
    let text = &"packages: [docker]\nhostname: x\nusers: { ada: { groups: [docker] }, bob: {} }\nfiles:\n  /etc/docker/daemon.json:\n    content: { registry-mirrors: [https://m.example.org], log-opts: { max-size: 10m } }\n    restart: docker.service\n".replacen("[docker]", &format!("[{}]", ex("docker")), 1);
    let c = load_text(text, "t", Path::new(".")).unwrap();
    let os = c.os.as_ref().unwrap();
    // The package, its daemon (packages/docker/package.nami), its group.
    assert!(os.packages.iter().any(|p| p == "docker") && os.start.contains(&"docker.socket".to_string()) && os.start.contains(&"docker.service".to_string()), "{:?}", os.start);
    assert!(os.files.contains_key("/etc/sysusers.d/docker.conf") && !c.files.contains_key("/etc/sysusers.d/docker.conf") && os.units.contains_key("docker.service.d/nami.conf"));
    assert_eq!(os.users["ada"].groups, ["docker"]);
    let json = &c.files["/etc/docker/daemon.json"];
    assert_eq!(crate::home::render("daemon.json", &json["content"]).unwrap(), "{\n  \"registry-mirrors\": [\n    \"https://m.example.org\"\n  ],\n  \"log-opts\": {\n    \"max-size\": \"10m\"\n  }\n}\n");
    assert!(check(text, "t", Path::new("."), None).is_empty(), "{:?}", check(text, "t", Path::new("."), None).iter().map(|d| &d.message).collect::<Vec<_>>());
    let d = check("hostname: x\nusers: { ada: { groups: [Docker] } }\n", "t", Path::new("."), None);
    assert!(d.iter().any(|d| d.message.contains("users.ada.groups: Docker: a group's name")), "{:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
}

#[test]
fn a_set_is_the_including_files() {
    let d = std::env::temp_dir().join(format!("nami-personal-set-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    // A person's set: env, aliases and their files, as in user.nami.
    let work = "env:\n  WORK: yes\naliases:\n  w: echo work\nfiles:\n  ~/.work: hi\n";
    std::fs::write(d.join("work.nami"), work).unwrap();
    let c = load_text("packages: [./work.nami, git]\n", USER_FILE, &d).unwrap();
    let home = format!("{:?}", c.home.unwrap());
    assert!(home.contains("WORK") && home.contains("echo work") && home.contains(".work"), "{home}");
    assert!(check(work, "work.nami", &d, None).is_empty(), "checked alone, it's a person's by what it says");
    // The computer's: env is everyone's.
    std::fs::write(d.join("office.nami"), "env:\n  OFFICE: yes\n").unwrap();
    let c = load_text("packages: [./office.nami, git]\n", SYSTEM_FILE, &d).unwrap();
    assert_eq!(c.os.unwrap().environment["OFFICE"], "yes");
    // Whatever its name, a set is the file's that lists it: from a user.nami, a person's.
    let c = load_text("packages: [./office.nami]\n", USER_FILE, &d).unwrap();
    assert_eq!(c.home.unwrap().env["OFFICE"], "yes");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_sets_paths_are_from_its_folder() {
    assert_eq!(relative_to(Path::new("/a/b/c"), Path::new("/a")), "./b/c");
    assert_eq!(relative_to(Path::new("/a/x/y"), Path::new("/a/b/c")), "../../x/y");
    assert_eq!(tidy(Path::new("/a/b/../c/./d")), Path::new("/a/c/d"));
    let d = std::env::temp_dir().join(format!("nami-set-paths-{}", std::process::id()));
    std::fs::create_dir_all(d.join("parts/deeper")).unwrap();
    std::fs::write(d.join("parts/work.nami"), "packages: [./deeper/more.nami]\nfiles:\n  ~/.note: { source: ./note.txt }\n").unwrap();
    std::fs::write(d.join("parts/deeper/more.nami"), "files:\n  ~/.more: { source: ../../top.txt }\n").unwrap();
    let c = load_text("packages: [./parts/work.nami]\n", USER_FILE, &d).unwrap();
    let files = &c.home.unwrap().files;
    assert_eq!(files[".note"]["source"], "./parts/note.txt");
    assert_eq!(files[".more"]["source"], "./top.txt");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn includes_merge() {
    let d = std::env::temp_dir().join(format!("nami-include-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("work.nami"), "packages: [slack, git]\nsettings:\n  a.b: { x: [1], y: 2 }\n").unwrap();
    let c = load_text("packages: [./work.nami, git, vim]\nsettings:\n  a.b: { x: [3] }\n", USER_FILE, &d).unwrap();
    assert_eq!(c.packages, ["slack", "git", "vim"]);
    let ab = &c.settings.unwrap()["a.b"];
    assert_eq!(ab["x"], serde_yaml::from_str::<Value>("[3]").unwrap(), "a setting is replaced, not joined");
    assert_eq!(ab["y"], Value::from(2));
    assert!(load_text("packages: [./nope.nami]\n", "t", &d).is_err());
    std::fs::write(d.join("loop.nami"), "packages: [./loop.nami]\n").unwrap();
    assert!(load_text("packages: [./loop.nami]\n", "t", &d).is_err());
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn typo_in_key_has_line_and_hint() {
    let d = check("packages: [jq]\nhistroy: 3\n", "t", Path::new("."), None);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].line, 2);
    assert_eq!(d[0].hint.as_deref(), Some("did you mean `history`? or this file needs a newer nami (nami self-update)"));
}

#[test]
fn finds_problems_with_lines() {
    let text = "packages:\n  - jq\n  - ./nope\nenv:\n  2BAD: x\nfiles:\n  ~/../escape: hi\n  ~/.bashrc:\n    a: 1\n";
    let d = check(text, USER_FILE, Path::new("/nonexistent"), None);
    let got: Vec<(usize, &str)> = d.iter().map(|d| (d.line, d.message.as_str())).collect();
    assert_eq!(got.len(), 4, "{got:?}");
    assert_eq!(got[0].0, 3); // missing recipe
    assert_eq!(got[1].0, 5); // bad env name
    assert_eq!(got[2].0, 7); // escapes home
    assert_eq!(got[3].0, 8); // unknown format
    // A name twice: said where.
    let d = check("packages:\n  - jq\n  - jq\n", USER_FILE, Path::new("/nonexistent"), None);
    assert!(d.len() == 1 && d[0].message.contains("listed twice"), "{:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
}

#[test]
fn services_and_files_are_checked() {
    let text = "services:\n  - myapp:\n      run: x\n      restart: sometimes\n  - ok\nfiles:\n  /etc/passwd: x\n  /etc/app.conf: hi\n  /etc/app.json: { a: 1 }\n  /etc/app.weird: { a: 1 }\n";
    let d = check(text, "t", Path::new("/nonexistent"), None);
    let got: Vec<(usize, &str)> = d.iter().map(|d| (d.line, d.message.as_str())).collect();
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(got[0].0, 2);
    assert!(got[0].1.contains("restart"));
    assert_eq!(got[1].0, 7);
    assert!(got[1].1.contains("user accounts"));
    assert_eq!(got[2].0, 10);
}

#[test]
fn a_services_own_files() {
    // Its own files restart it; restart-on isn't written.
    let d = check("services:\n  - myapp:\n      run: x\n      restart-on: [/etc/x]\n", "t", Path::new("."), None);
    assert!(d.iter().any(|d| d.message.contains("myapp: its own files go under it")), "{d:?}");
    let d = check("services:\n  - myapp:\n      run: x\n      /etc/myapp/app.conf: hi\n", "t", Path::new("."), None);
    assert!(d.iter().all(|d| d.severity != Severity::Error), "{d:?}");
    let e = load_text("services:\n  jq: true\n", SYSTEM_FILE, Path::new(".")).unwrap_err().to_string();
    assert!(e.contains("services: a list, `- name` a line"), "{e}");
}

#[test]
fn tier_rules() {
    let text = "services:\n  - myapp:\n      run: x\n      user: myapp\n";
    assert!(check_tier(text, Path::new("."), None, Tier::System).is_empty());
    let d = check_tier(text, Path::new("."), None, Tier::User);
    let got: Vec<(usize, &str)> = d.iter().map(|d| (d.line, d.message.as_str())).collect();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].0, 4);
    assert!(got[0].1.contains("your services run as you"));
    let d = check_tier("files:\n  /etc/x.conf: hi\n", Path::new("."), None, Tier::User);
    assert!(d.len() == 1 && d[0].line == 2 && d[0].message.contains("a person's files are in their home"), "{:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    // A project's file: its own services, and no /etc files.
    let d = check("packages: [nodejs]\ntasks: { dev: npm run dev }\nfiles:\n  /etc/x: y\n", "t", Path::new("."), None);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].line, 3);
    assert!(d[0].message.contains("not a project's"), "{}", d[0].message);
}

#[test]
fn an_output_written_with_a_caret_is_a_notice_not_an_error() {
    let d = check("packages:\n  - jq\n  - util-linux^bin\n", SYSTEM_FILE, Path::new("/nowhere"), None);
    let d: Vec<_> = d.iter().filter(|d| d.message.contains('^')).collect();
    assert!(d.len() == 1 && d[0].severity == Severity::Warning && d[0].line == 3 && d[0].message.contains("util-linux.bin"), "{d:?}");
    let mut v: Value = serde_yaml::from_str("packages: [util-linux^bin, jq]").unwrap();
    old_outputs(&mut v, "t");
    assert_eq!(v["packages"][0].as_str(), Some("util-linux.bin"));
    let d = check("name: x\nversion: \"1\"\nsource: { path: . }\ndeps: [libgcc^lib]\n", "package.nami", Path::new("/nowhere"), None);
    assert!(d.iter().any(|d| d.severity == Severity::Warning && d.line == 4 && d.message.contains("libgcc.lib")), "{d:?}");
}

#[test]
fn a_task_of_the_wrong_kind_is_said_at_its_line_and_doesnt_hang() {
    // These used to look for an empty word, forever.
    let d = check_project("packages: [jq]\ntasks:\n  test: [a, b]\n", Path::new("/p"), None);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].line, 3);
    assert!(d[0].message.starts_with("tasks: test: a task is a command") && d[0].message.contains("&&"), "{}", d[0].message);
    let d = check_project("tasks: { test: { run: x, decsription: y } }\n", Path::new("/p"), None);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].message.contains("did you mean `description`"), "{}", d[0].message);
    assert_eq!(d[0].line, 1);
    let d = check_project("tasks: [a, b]\n", Path::new("/p"), None);
    assert_eq!((d.len(), d[0].line), (1, 1), "{d:?}");
    // The map form is fine, and what it depends on is checked.
    assert!(check_project("tasks:\n  build: { run: make }\n  test: { run: x, description: Runs the tests, depends: [build] }\n", Path::new("/p"), None).is_empty());
    let d = check_project("tasks:\n  test: { run: x, depends: [biuld] }\n", Path::new("/p"), None);
    assert!(d.len() == 1 && d[0].message.contains("depends on biuld, which isn't a task here") && d[0].line == 2, "{d:?}");
    let d = check_project("tasks:\n  a: { run: x, depends: [b] }\n  b: { depends: [a] }\n", Path::new("/p"), None);
    assert!(d.len() == 1 && d[0].message.contains("circle (a -> b -> a)"), "{d:?}");
    // Nothing to find is no line, at once.
    assert_eq!(find_line("a: b\n", None, ""), 0);
}

#[test]
fn project_services_are_checked() {
    let ex = |s: &str| format!("{}/examples/services/{s}.nami", env!("CARGO_MANIFEST_DIR"));
    let ok = &format!("packages: [nodejs]\ntasks: {{ dev: npm run dev }}\nservices:\n  - {}\n  - {}: {{ version: 7 }}\n  - worker: {{ run: npm run worker }}\n", ex("postgres"), ex("redis"));
    assert!(check(ok, "t", Path::new("/p"), None).is_empty(), "{:?}", check(ok, "t", Path::new("/p"), None).iter().map(|d| &d.message).collect::<Vec<_>>());
    let d = check(&format!("tasks: {{ dev: x }}\nservices:\n  - {}\n  - {}: {{ prot: 7 }}\n", ex("postgres"), ex("redis")), "t", Path::new("/p"), None);
    assert_eq!(d.len(), 1, "{:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert_eq!(d[0].line, 4);
    assert!(d[0].message.contains("redis.nami: no key `prot` (did you mean `port`?)"), "{}", d[0].message);
    let d = check_project("packages: [jq]\nservics:\n  postgres: true\n", Path::new("/p"), None);
    assert_eq!((d.len(), d[0].line, d[0].hint.as_deref()), (1, 2, Some("did you mean `services`? or this file needs a newer nami (nami self-update)")));
    let d = check_project("services:\n  - postgres\n", Path::new("/p"), None);
    assert!(d[0].message.contains("list a service file by its path (./services/postgres.nami"), "{}", d[0].message);
    let d = check_project("services:\n  - ./services/postgress.nami\n", Path::new("/p"), None);
    assert!(d[0].message.contains("postgress.nami: No such file"), "{}", d[0].message);
}

#[test]
fn a_computer_uses_its_folder_and_what_it_lists() {
    let d = std::env::temp_dir().join(format!("nami-used-{}", std::process::id()));
    for f in ["laptop/files", "packages/greet", "packages/base", "other"] {
        std::fs::create_dir_all(d.join(f)).unwrap();
    }
    std::fs::write(d.join("laptop/system.nami"), "hostname: laptop\npackages:\n  - ../packages/greet\n").unwrap();
    std::fs::write(d.join("packages/greet/package.nami"), "packages: [../base]\n").unwrap();
    std::fs::write(d.join("packages/base/package.nami"), "packages: [bash]\n").unwrap();
    let u = used(&d.join("laptop/system.nami")).unwrap();
    let d = d.canonicalize().unwrap();
    assert_eq!(u.top, d);
    assert_eq!(u.folders, [d.join("laptop"), d.join("packages/base"), d.join("packages/greet")]);
    assert_eq!(u.rel(&u.computer), Path::new("laptop"));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn names_are_looked_up_in_the_nearest_packages_folder() {
    let d = std::env::temp_dir().join(format!("nami-names-{}", std::process::id()));
    for f in ["laptop/ada", "packages/greet", "packages/base", ".git"] {
        std::fs::create_dir_all(d.join(f)).unwrap();
    }
    std::fs::write(d.join("packages/greet/package.nami"), "packages: [base]\nenv: { GREETING: mine }\n").unwrap();
    std::fs::write(d.join("packages/base/package.nami"), "packages: [base, bash]\n").unwrap();
    let laptop = d.join("laptop");
    // A folder there is the folder (and what it lists, its neighbours
    // by name); a name with none is the catalog's.
    let c = load_text("hostname: laptop\npackages: [greet, htop]\n", SYSTEM_FILE, &laptop).unwrap();
    assert!(c.packages.contains(&"htop".to_string()), "{:?}", c.packages);
    // (base's package.nami listing base: the catalog's, not itself.)
    assert!(c.os.as_ref().unwrap().packages.contains(&"base".to_string()), "{:?}", c.os);
    assert_eq!(c.os.unwrap().environment["GREETING"], "mine");
    // From a person's file, one deeper, the same folder.
    let mut v: Value = serde_yaml::from_str("packages: [greet]").unwrap();
    resolve_names(&mut v, &d.join("laptop/ada"));
    assert_eq!(v["packages"][0].as_str(), Some("../../packages/greet"));
    // from: is gone, and says where names are looked up now.
    let e = format!("{:#}", load_text("from: ../packages\npackages: [htop]\n", SYSTEM_FILE, &laptop).unwrap_err());
    assert!(e.contains("nearest packages/ folder"), "{e}");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_persons_file_gets_what_is_theirs_of_a_folder() {
    let d = std::env::temp_dir().join(format!("nami-personal-{}", std::process::id()));
    for f in ["laptop/ann", "packages/zsh", ".git"] {
        std::fs::create_dir_all(d.join(f)).unwrap();
    }
    std::fs::write(d.join("packages/zsh/package.nami"), "files:\n  /etc/zshrc: |\n    setopt autocd\n  ~/.zshrc: |\n    bindkey -e\n").unwrap();
    let c = load_text("packages:\n  - zsh: { login-shell: true }\n", USER_FILE, &d.join("laptop/ann")).unwrap();
    assert!(c.packages.contains(&"zsh".to_string()), "{:?}", c.packages);
    let home = c.home.unwrap();
    let keys: Vec<&str> = home.files.keys().filter_map(Value::as_str).collect();
    assert!(keys == [".zshrc"], "{keys:?}");
    // Each person's services are theirs; the computer's stay behind.
    std::fs::create_dir_all(d.join("packages/sync")).unwrap();
    std::fs::write(d.join("packages/sync/package.nami"), "services:\n  - syncthing.service: { per-person: true }\n  - cupsd.service\n").unwrap();
    let c = load_text("packages:\n  - sync\n", USER_FILE, &d.join("laptop/ann")).unwrap();
    let services: Vec<&str> = c.services.keys().filter_map(Value::as_str).collect();
    assert_eq!(services, ["syncthing.service"]);
    for (k, v) in &c.services {
        crate::services::parse(k.as_str().unwrap(), v).unwrap();
    }
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn the_user_file_may_be_in_config_nami() {
    let home = std::env::temp_dir().join(format!("nami-user-file-{}", std::process::id()));
    std::fs::create_dir_all(home.join(".config/nami")).unwrap();
    assert_eq!(user_file_in(&home), home.join("user.nami"), "~/user.nami by default");
    std::fs::write(home.join(".config/nami/user.nami"), "packages: []\n").unwrap();
    assert_eq!(user_file_in(&home), home.join(".config/nami/user.nami"));
    std::fs::write(home.join("user.nami"), "packages: []\n").unwrap();
    assert_eq!(user_file_in(&home), home.join("user.nami"), "~/user.nami wins");
    std::fs::remove_file(home.join("user.nami")).unwrap();
    std::os::unix::fs::symlink(home.join(".config/nami/user.nami"), home.join("user.nami")).unwrap();
    assert_eq!(user_file_in(&home), home.join("user.nami"), "linked to it, it's the one file");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn clean_config_is_clean() {
    let text = "packages: [jq]\nhistory: 5\nfiles:\n  ~/.gitconfig:\n    user: { name: A }\n";
    assert!(check(text, USER_FILE, Path::new("."), None).is_empty());
}

#[test]
fn cleanup_in_either_file() {
    let here = Path::new(".");
    let p = |text: &str, file: &str| load_text(text, file, here).map(|c| c.cleanup);
    assert_eq!(p("packages: [jq]\n", USER_FILE).unwrap(), None);
    assert_eq!(p("cleanup: 7d\n", USER_FILE).unwrap(), Some(crate::cleanup::Policy { after: Some(7 * 86400), keep: 3 }));
    assert_eq!(p("cleanup: off\n", SYSTEM_FILE).unwrap(), Some(crate::cleanup::Policy { after: None, keep: 3 }));
    assert_eq!(p("cleanup:\n  older-than: 90d\n  keep: 5\n", SYSTEM_FILE).unwrap(), Some(crate::cleanup::Policy { after: Some(90 * 86400), keep: 5 }));
    // Written wrong: said, and where.
    let e = p("cleanup: soon\n", USER_FILE).unwrap_err().to_string();
    assert!(e.contains("cleanup") && e.contains("30d"), "{e}");
    let d = check("cleanup: { keep: 0 }\n", USER_FILE, here, None);
    assert!(d.iter().any(|d| d.message.contains("1 or more")), "{:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert!(check("cleanup: 30d\n", USER_FILE, here, None).is_empty());
}

#[test]
fn old_names_work_and_say_the_new_one() {
    let here = Path::new(".");
    let text = "keep-generations: 5\nunfree: false\nshell:\n  zsh: echo hi\nsettings:\n  org.gnome.desktop.interface: { color-scheme: prefer-dark }\n";
    let c = load_text(text, USER_FILE, here).unwrap();
    assert_eq!((c.history, c.unfree), (Some(5), Some(false)));
    assert_eq!(c.home.unwrap().shell["zsh"], "echo hi");
    assert!(c.settings.is_some());
    // (Whether this computer has GNOME's schemas isn't what's tested here.)
    let said: Vec<(usize, String)> = check(text, USER_FILE, here, None).into_iter().filter(|d| !d.message.contains("no schema")).map(|d| (d.line, d.message)).collect();
    // settings:, shell: and unfree: keep their names.
    assert_eq!(said, [(1, "keep-generations is now history".to_string())]);
    // A longer name for a short key says which it is.
    let e = load_text("desktop-settings: {}\n", USER_FILE, here).unwrap_err().to_string();
    assert!(e.contains("did you mean `settings`?"), "{e}");
    let e = load_text("keep-generations: 5\nhistory: 6\n", USER_FILE, here).unwrap_err().to_string();
    assert!(e.contains("keep-generations: and history: both say it"), "{e}");
    // What nami writes is the new name, the old one renamed in place.
    let t = crate::config_edit::apply("keep-generations: 3 # mine\npackages: [jq]\n", &crate::config_edit::Op::Set { path: vec!["keep-generations".into()], value: 4.into() }).unwrap();
    assert_eq!(t, "history: 4 # mine\npackages: [jq]\n");
}

#[test]
fn a_newer_format_is_refused_plainly() {
    let here = Path::new(".");
    assert!(load_text("format: 1\npackages: [jq]\n", USER_FILE, here).is_ok());
    let e = load_text("format: 2\npackages: [jq]\n", USER_FILE, here).unwrap_err().to_string();
    assert!(e.ends_with("format: 2: this file needs a newer nami: nami self-update"), "{e}");
    let d = check("format: 2\npackages: [jq]\n", "system.nami", here, None);
    assert_eq!((d.len(), d[0].line), (1, 1), "{:?}", d.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert!(load_text("format: one\n", USER_FILE, here).is_err());
    let project = check("format: 9\npackages: [jq]\ntasks: { t: echo }\n", "project.nami", here, None);
    assert!(project[0].message.contains("needs a newer nami"), "{}", project[0].message);
}

#[test]
fn flat_file_says_where_each_value_came_from() {
    let d = std::env::temp_dir().join(format!("nami-flat-{}", std::process::id()));
    std::fs::create_dir_all(d.join("parts")).unwrap();
    std::fs::write(d.join("parts/work.nami"), "timezone: Europe/Copenhagen\nhostname: work\npackages: [jq, git]\n").unwrap();
    let top = &format!("hostname: laptop\nusers: {{ ada: {{ admin: true }} }}\npackages: [{}, ./parts/work.nami, git, htop]\n", ex(a_computer()));
    let flat = flat_text(top, "system.nami", &d).unwrap();
    // Overridden by the file itself, or the include's.
    assert!(flat.contains("hostname: laptop  # system.nami\n"), "{flat}");
    assert!(flat.contains("timezone: Europe/Copenhagen  # parts/work.nami\n"), "{flat}");
    // A list joined from two files: each item its own file's.
    assert!(flat.contains("\n  - git  # system.nami\n"), "{flat}");
    assert!(flat.contains("\n  - htop  # system.nami\n"), "{flat}");
    assert!(flat.contains("\n  - jq  # parts/work.nami\n"), "{flat}");
    // The examples' files by where they are.
    let files = if namios_examples() { ["namios-bare", "niri", "base", "namios-greeter"] } else { ["niri", "desktop", "base", "gdm"] };
    assert!(files.iter().all(|p| flat.contains(&format!("examples/packages/{p}/package.nami\n"))), "{flat}");
    // Your file's keys come first.
    assert!(flat.starts_with("hostname: laptop  # system.nami\n"), "{flat}");
    // It reads back as the computer it describes, and the same each time.
    let mut back: Value = serde_yaml::from_str(&flat).unwrap();
    package_map(&mut back, "flat").unwrap();
    // (`- fuse: 2` is fuse@2, as a file is read.)
    crate::config::versioned_packages(&mut back, "flat").unwrap();
    // (In the keys files are written with: a view, as a package would write it.)
    let whole = |text: &str| {
        let (v, sources) = crate::flat::recorded(|| load_whole(text, "system.nami", &d));
        written_keys(v.unwrap().0, &sources)
    };
    assert_eq!(back, whole(top));
    assert_eq!(flat, flat_text(top, "system.nami", &d).unwrap());
    for (name, dir, _) in examples() {
        let text = format!("packages: [{dir}]\nhostname: x\n");
        let mut back: Value = serde_yaml::from_str(&flat_text(&text, "system.nami", &d).unwrap()).unwrap();
        package_map(&mut back, "flat").unwrap();
        crate::config::versioned_packages(&mut back, "flat").unwrap();
        assert_eq!(back, whole(&text), "{name}");
    }
    std::fs::remove_dir_all(&d).ok();
}

/// `add_package` and `remove_package`, through config_edit's edits (here, as config_edit.rs
/// is also the namiOS Installer's, which has no config/).
mod edits {
    use super::super::{add_package, listed_packages, remove_package};
    use crate::config_edit::{apply, parse_path, Op};
    use serde_yaml::Value;

    fn s(v: &str) -> Value {
        Value::String(v.into())
    }
    fn p(path: &str) -> Vec<String> {
        parse_path(path).unwrap()
    }

    const CONFIG: &str = "# my laptop\npackages:\n  - firefox   # browser\n  - jq\n\nkeep-generations: 5  # enough\nhome:\n  env:\n    EDITOR: nvim\n  files:\n    .gitconfig:\n      user: { name: Ada }\n";

    #[test]
    fn add_and_remove_packages_keep_comments() {
        let edit = |t: &str, ops: Vec<Op>| ops.iter().fold(t.to_string(), |t, op| apply(&t, op).unwrap());
        let t = edit(CONFIG, add_package(CONFIG, "ripgrep").unwrap());
        assert!(t.contains("  - jq\n  - ripgrep\n\nkeep-generations"), "{t}");
        assert!(t.contains("# my laptop") && t.contains("# browser") && t.contains("# enough"));
        let t = edit(&t, remove_package(&t, "firefox"));
        assert!(!t.contains("firefox") && t.contains("  - jq\n  - ripgrep"), "{t}");
        // Adding what's there changes nothing.
        assert!(add_package(&t, "jq").unwrap().is_empty());
        let t2 = edit(&t, add_package(&t, "python3@3.10").unwrap());
        assert!(t2.contains("  - python3: '3.10'\n") || t2.contains("  - python3: \"3.10\"\n"), "{t2}");
    }


    /// `nami install` then `nami remove` of a package from a repo, in
    /// every way it can be written: the file comes back as it was, and
    /// taking out the last one leaves the file's comments.
    #[test]
    fn repo_packages_install_and_remove() {
        let edit = |t: &str, ops: Vec<Op>| ops.iter().try_fold(t.to_string(), |t, op| apply(&t, op));
        let zen = "github:0xc000022070/zen-browser-flake";
        for (file, name) in [
            ("# my laptop\npackages: []\n", zen),
            ("# my laptop\npackages:\n", zen),
            ("packages: [jq]\n", zen),
            ("# head\npackages:\n  - jq   # json\n\nenv:\n  A: b\n", "github:acme/tools#greet"),
            ("packages:\n  - jq\n", "git+https://codeberg.org/bob/os#gaming.nami@v2"),
            ("packages:\n  - jq\n", "github:acme/tools@v2"),
        ] {
            let added = edit(file, add_package(file, name).unwrap()).unwrap_or_else(|e| panic!("adding {name} to {file:?}: {e}"));
            assert!(listed_packages(&added).iter().any(|(h, _)| h == name), "{added}");
            let back = edit(&added, remove_package(&added, name)).unwrap_or_else(|e| panic!("removing {name} from {added:?}: {e}"));
            // An empty `packages:` taken out with its last item is the same file.
            let bare = |t: &str| t.replace("packages: []\n", "").replace("packages:\n", "");
            assert!(back == file || bare(&back) == bare(file), "{name}: {file:?} came back as {back:?}");
        }
        // Each way an entry can be written: first, last, and the only one.
        for entry in [
            "github:acme/tools#greet",
            "'github:acme/tools#greet'",
            "\"github:acme/tools#greet\"  # mine",
            "github:acme/tools#greet # mine",
            "git+https://codeberg.org/bob/os#gaming.nami@v2",
            "github:acme/tools#greet:\n      version: 1.0",
        ] {
            for file in [format!("# c\npackages:\n  - {entry}\n  - jq\n"), format!("packages:\n  - jq\n  - {entry}\n"), format!("# c\npackages:\n  - {entry}\n")] {
                let (have, _) = listed_packages(&file).into_iter().find(|(h, _)| h != "jq").unwrap();
                let out = edit(&file, remove_package(&file, &have)).unwrap_or_else(|e| panic!("removing {have} from {file:?}: {e}"));
                assert!(!out.contains("acme") && !out.contains("codeberg"), "{out}");
                assert_eq!(out.contains("jq"), file.contains("jq"), "{out}");
                assert_eq!(out.starts_with("# c\n"), file.starts_with("# c\n"), "{out}");
            }
        }
        // A file that's only comments reads as one with no keys.
        assert_eq!(apply("# just this\n", &Op::Add { path: p("packages"), value: s("jq") }).unwrap(), "# just this\npackages:\n  - jq\n");
    }
}
