//! home-manager's shells and desktop look, in nami's terms: `programs.zsh`,
//! `programs.bash`, `programs.fish` and the tools that hook into them become
//! `home.shell`; `home.pointerCursor`, `gtk` become desktop settings, GTK's
//! settings.ini and env.
//!
//! Lines are collected with an order, like home-manager's own (mkOrder), so
//! the file comes out the way home-manager would have written the rc: first
//! lines, keymap, completion, history, oh-my-zsh, plugins, your init, tools'
//! hooks, syntax highlighting last.

use std::collections::BTreeMap;

use serde_yaml::{Mapping, Value};

use super::{Kind, Reader};
use crate::nix::expr::Val;

const FIRST: u32 = 100;
const KEYMAP: u32 = 120;
const BEFORE_COMPINIT: u32 = 150;
const COMPINIT: u32 = 200;
const HISTORY: u32 = 250;
const FUNCTIONS: u32 = 280;
const OH_MY_ZSH: u32 = 300;
const PLUGINS: u32 = 400;
const GLOBAL_ALIASES: u32 = 450;
const INIT: u32 = 500;
const HOOKS: u32 = 600;
const LAST: u32 = 900;

/// Programs that hook into shells (home-manager's `enable<Shell>Integration`,
/// on by default when the program and the shell are both enabled): the line
/// each shell runs. In home-manager's order of how commonly they're used.
const SHELL_HOOKS: &[(&str, &str, &str, &str)] = &[
    // program, bash, zsh, fish
    ("direnv", r#"eval "$(direnv hook bash)""#, r#"eval "$(direnv hook zsh)""#, "direnv hook fish | source"),
    ("zoxide", r#"eval "$(zoxide init bash)""#, r#"eval "$(zoxide init zsh)""#, "zoxide init fish | source"),
    ("fzf", r#"eval "$(fzf --bash)""#, "source <(fzf --zsh)", "fzf --fish | source"),
    ("atuin", r#"eval "$(atuin init bash)""#, r#"eval "$(atuin init zsh)""#, "atuin init fish | source"),
    ("mcfly", r#"eval "$(mcfly init bash)""#, r#"eval "$(mcfly init zsh)""#, "mcfly init fish | source"),
    ("mise", r#"eval "$(mise activate bash)""#, r#"eval "$(mise activate zsh)""#, "mise activate fish | source"),
    ("carapace", "source <(carapace _carapace bash)", "source <(carapace _carapace zsh)", "carapace _carapace fish | source"),
    ("thefuck", r#"eval "$(thefuck --alias)""#, r#"eval "$(thefuck --alias)""#, "thefuck --alias | source"),
    ("pay-respects", r#"eval "$(pay-respects bash --alias)""#, r#"eval "$(pay-respects zsh --alias)""#, "pay-respects fish --alias | source"),
    ("navi", r#"eval "$(navi widget bash)""#, r#"eval "$(navi widget zsh)""#, "navi widget fish | source"),
    ("television", r#"eval "$(tv init bash)""#, r#"eval "$(tv init zsh)""#, "tv init fish | source"),
    ("keychain", r#"eval "$(keychain --eval --quiet)""#, r#"eval "$(keychain --eval --quiet)""#, "keychain --eval --quiet | source"),
    // Prompts last, so they win over a theme set earlier (oh-my-zsh's).
    ("oh-my-posh", r#"eval "$(oh-my-posh init bash)""#, r#"eval "$(oh-my-posh init zsh)""#, "oh-my-posh init fish | source"),
    ("starship", r#"eval "$(starship init bash)""#, r#"eval "$(starship init zsh)""#, "starship init fish | source"),
];

/// Programs whose shell integration is aliases (the same in every shell).
const ALIAS_HOOKS: &[(&str, &[(&str, &str)])] = &[
    ("eza", &[("ls", "eza"), ("ll", "eza -l"), ("la", "eza -a"), ("lt", "eza --tree"), ("lla", "eza -la")]),
    ("lsd", &[("ls", "lsd"), ("ll", "lsd -l"), ("la", "lsd -A"), ("lt", "lsd --tree"), ("lla", "lsd -lA")]),
];

/// Integrations nami's shells don't have.
const OTHER_SHELLS: &[&str] = &["enableNushellIntegration", "enableIonIntegration", "enableXonshIntegration"];

/// Where zsh plugins are in their packages (nixpkgs moved them under
/// share/zsh/plugins; nami_share takes the first that exists).
const ZSH_AUTOSUGGESTIONS: &str = "zsh/plugins/zsh-autosuggestions/zsh-autosuggestions.zsh zsh-autosuggestions/zsh-autosuggestions.zsh";
const ZSH_SYNTAX_HIGHLIGHTING: &str = "zsh-syntax-highlighting/zsh-syntax-highlighting.zsh zsh/plugins/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh";
const ZSH_HISTORY_SUBSTRING_SEARCH: &str =
    "zsh/plugins/zsh-history-substring-search/zsh-history-substring-search.zsh zsh-history-substring-search/zsh-history-substring-search.zsh";

/// `'text'` for sh, bash, zsh and fish.
fn quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:,+=@%".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// What the shells in a home-manager configuration add up to, decided once
/// everything is read (an integration is on by default only if its shell is).
#[derive(Debug, Default)]
pub(super) struct Shells {
    /// bash/zsh/fish -> (order, lines).
    pub lines: BTreeMap<String, Vec<(u32, String)>>,
    /// Shells home-manager would set up (`programs.<shell>.enable`).
    on: Vec<String>,
    /// Programs with a shell hook that are on -> shell -> what
    /// `enable<Shell>Integration` said, if it said.
    hooks: BTreeMap<String, BTreeMap<String, bool>>,
    zsh_completion: Option<bool>,
    /// `programs.direnv.nix-direnv`: per-project environments when you cd,
    /// which nami's own hook does for a project's project.nami.
    project_hook: bool,
    zsh_completion_init: Option<String>,
    oh_my_zsh: bool,
    /// zsh plugins with sources Nix fetched: (name, file to source).
    fetched: Vec<(String, String)>,
}

impl Shells {
    fn add(&mut self, sh: &str, order: u32, text: impl Into<String>) {
        let text = text.into();
        let lines = self.lines.entry(sh.to_string()).or_default();
        if !lines.iter().any(|(_, t)| *t == text) {
            lines.push((order, text));
        }
    }

    pub fn has(&self, sh: &str) -> bool {
        self.lines.get(sh).is_some_and(|l| !l.is_empty())
    }

    pub fn fetched_plugins(&self) -> Vec<(String, String)> {
        self.fetched.clone()
    }

    /// A fetched plugin that home-manager's files have a copy of.
    pub fn plugin_from_home(&mut self, name: &str, file: &str) {
        self.add("zsh", PLUGINS, format!("source ~/.zsh/plugins/{name}/{}", quote(file)));
    }

    /// `home.shell`: each shell's lines in order.
    pub fn text(&self) -> BTreeMap<String, String> {
        self.lines
            .iter()
            .filter(|(_, l)| !l.is_empty())
            .map(|(sh, lines)| {
                let mut lines = lines.clone();
                lines.sort_by_key(|(o, _)| *o); // stable: same order keeps reading order
                let text: String = lines.iter().map(|(_, t)| format!("{}\n", t.trim_end_matches('\n'))).collect();
                (sh.clone(), text)
            })
            .collect()
    }
}

impl Reader {
    /// A string of shell code, with `${pkgs.x}/share/...` as nami_share.
    fn shell_code(&self, v: &Val) -> Option<String> {
        match v {
            Val::Str(s, true) => Some(s.clone()),
            _ => None,
        }
    }

    /// Before `programs.<name>`'s options are read: shell code in them may
    /// name package files (`${pkgs.x}/share/...`).
    pub(super) fn shell_program_start(&mut self, name: &str, enabled: Option<bool>, kind: Kind) {
        if kind != Kind::HomeManager || enabled != Some(true) {
            return;
        }
        if crate::home::SHELLS.contains(&name) {
            self.shells.on.push(name.to_string());
        }
        if SHELL_HOOKS.iter().any(|(p, ..)| *p == name) || ALIAS_HOOKS.iter().any(|(p, _)| *p == name) {
            self.shells.hooks.entry(name.to_string()).or_default();
        }
    }

    /// An option of `programs.<name>` that's about shells, read whatever
    /// its value (false says something too). True: handled.
    pub(super) fn shell_option(&mut self, name: &str, k: &str, v: &Val, kind: Kind) -> bool {
        if k.starts_with("enable") && k.ends_with("Integration") {
            let shell = match k {
                "enableBashIntegration" => "bash",
                "enableZshIntegration" => "zsh",
                "enableFishIntegration" => "fish",
                _ if OTHER_SHELLS.contains(&k) => {
                    if matches!(v, Val::Bool(true)) {
                        self.found.skip(format!("programs.{name}.{k}"), "nami's shell: is for bash, zsh and fish");
                    }
                    return true;
                }
                _ => return false,
            };
            let known = SHELL_HOOKS.iter().any(|(p, ..)| *p == name) || ALIAS_HOOKS.iter().any(|(p, _)| *p == name);
            match v {
                Val::Bool(b) if known && kind == Kind::HomeManager => {
                    self.shells.hooks.entry(name.to_string()).or_default().insert(shell.to_string(), *b);
                }
                Val::Bool(false) => {}
                _ => self.found.skip(format!("programs.{name}.{k}"), format!("{name}'s {shell} integration: add its init line to shell.{shell}")),
            }
            return true;
        }
        if kind != Kind::HomeManager {
            return false;
        }
        match (name, k) {
            ("zsh", "enableCompletion") => {
                self.shells.zsh_completion = Some(matches!(v, Val::Bool(true)));
                true
            }
            ("bash", "enableCompletion") => true, // distros load bash-completion themselves
            ("direnv", "nix-direnv") => {
                if matches!(v, Val::Bool(true)) || self.ev.select(v, "enable").is_some_and(|e| matches!(e, Val::Bool(true))) {
                    self.shells.project_hook = true;
                    self.found.warn("programs.direnv.nix-direnv: projects' `use flake` needs Nix; `nami hook` (added to shell:) loads a project's project.nami when you cd into it instead: `nami import` each project's flake");
                }
                true
            }
            _ => false,
        }
    }

    /// `programs.zsh.<k>`. True: handled.
    pub(super) fn zsh_option(&mut self, k: &str, v: &Val) -> bool {
        let what = format!("programs.zsh.{k}");
        let on = |v: &Val| matches!(v, Val::Bool(true));
        let enabled = |r: &Self, v: &Val| on(v) || r.ev.select(v, "enable").is_some_and(|e| on(&e));
        match k {
            "initContent" | "initExtra" | "initExtraFirst" | "initExtraBeforeCompInit" | "envExtra" | "completionInit" => {
                let Some(code) = self.shell_code(v) else {
                    self.found.skip(what, "refers to values only Nix knows: copy it into shell.zsh");
                    return true;
                };
                match k {
                    "completionInit" => self.shells.zsh_completion_init = Some(code),
                    "initExtraFirst" => self.shells.add("zsh", FIRST, code),
                    "initExtraBeforeCompInit" => self.shells.add("zsh", BEFORE_COMPINIT, code),
                    "envExtra" => {
                        self.found.warn("programs.zsh.envExtra: shell.zsh runs in interactive shells only (not scripts); variables for everything go in env:");
                        self.shells.add("zsh", FIRST, code);
                    }
                    _ => self.shells.add("zsh", INIT, code),
                }
            }
            "autosuggestion" | "enableAutosuggestions" => {
                if enabled(self, v) {
                    self.found.package("zsh-autosuggestions", &what);
                    self.shells.add("zsh", PLUGINS, format!("source \"$(nami_share {ZSH_AUTOSUGGESTIONS})\""));
                    if let Some(h) = self.ev.select(v, "highlight").and_then(|h| h.str().map(String::from)) {
                        self.shells.add("zsh", PLUGINS - 1, format!("ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE={}", quote(&h)));
                    }
                    if let Some(s) = self.ev.select(v, "strategy").and_then(|s| self.strs(&s)) {
                        self.shells.add("zsh", PLUGINS - 1, format!("ZSH_AUTOSUGGEST_STRATEGY=({})", s.join(" ")));
                    }
                }
            }
            "syntaxHighlighting" | "enableSyntaxHighlighting" => {
                if enabled(self, v) {
                    self.found.package("zsh-syntax-highlighting", &what);
                    if let Some(h) = self.ev.select(v, "highlighters").and_then(|h| self.strs(&h)) {
                        self.shells.add("zsh", LAST + 1, format!("ZSH_HIGHLIGHT_HIGHLIGHTERS+=({})", h.join(" ")));
                    }
                    for (style, t) in self.ev.select(v, "styles").map(|s| self.attrs(&s)).unwrap_or_default() {
                        if let Some(s) = self.force(&t).str() {
                            self.shells.add("zsh", LAST + 1, format!("ZSH_HIGHLIGHT_STYLES[{}]={}", quote(&style), quote(s)));
                        }
                    }
                    // Must come after everything else that adds widgets.
                    self.shells.add("zsh", LAST, format!("source \"$(nami_share {ZSH_SYNTAX_HIGHLIGHTING})\""));
                }
            }
            "historySubstringSearch" => {
                if enabled(self, v) {
                    self.found.package("zsh-history-substring-search", &what);
                    self.shells.add("zsh", LAST + 2, format!("source \"$(nami_share {ZSH_HISTORY_SUBSTRING_SEARCH})\""));
                    for (key, widget, default) in [("searchUpKey", "history-substring-search-up", "^[[A"), ("searchDownKey", "history-substring-search-down", "^[[B")] {
                        let keys = self.ev.select(v, key).and_then(|k| self.strs(&k)).unwrap_or_else(|| vec![default.to_string()]);
                        for key in keys {
                            self.shells.add("zsh", LAST + 3, format!("bindkey {} {widget}", quote(&key)));
                        }
                    }
                }
            }
            "oh-my-zsh" => {
                if !enabled(self, v) {
                    return true;
                }
                self.shells.oh_my_zsh = true;
                self.found.package("oh-my-zsh", &what);
                let mut lines = vec![
                    "export ZSH=\"$(nami_share oh-my-zsh)\"".to_string(),
                    "ZSH_CACHE_DIR=\"${XDG_CACHE_HOME:-$HOME/.cache}/oh-my-zsh\"; mkdir -p \"$ZSH_CACHE_DIR\"".to_string(),
                ];
                if let Some(theme) = self.ev.select(v, "theme").and_then(|t| t.str().map(String::from)) {
                    lines.push(format!("ZSH_THEME={}", quote(&theme)));
                }
                if let Some(p) = self.ev.select(v, "plugins").and_then(|p| self.strs(&p)) {
                    lines.push(format!("plugins=({})", p.join(" ")));
                }
                match self.ev.select(v, "custom") {
                    Some(Val::Str(c, true)) => lines.push(format!("ZSH_CUSTOM={}", quote(&c))),
                    Some(Val::Path(p)) => lines.push(format!("ZSH_CUSTOM={}", quote(&p.display().to_string()))),
                    Some(Val::Null) | None => {}
                    Some(_) => self.found.skip(format!("{what}.custom"), "refers to values only Nix knows"),
                }
                match self.ev.select(v, "extraConfig") {
                    Some(Val::Str(c, true)) => lines.push(c.trim_end().to_string()),
                    Some(Val::Null) | None => {}
                    Some(_) => self.found.skip(format!("{what}.extraConfig"), "refers to values only Nix knows: copy it into shell.zsh"),
                }
                lines.push("source \"$ZSH/oh-my-zsh.sh\"".into());
                self.shells.add("zsh", OH_MY_ZSH, lines.join("\n"));
            }
            "plugins" => {
                for item in self.ev.list(v) {
                    let name = self.ev.select(&item, "name").and_then(|n| n.str().map(String::from)).unwrap_or_default();
                    let file = self.ev.select(&item, "file").and_then(|f| f.str().map(String::from)).unwrap_or_else(|| format!("{name}.plugin.zsh"));
                    match self.ev.select(&item, "src") {
                        Some(Val::Str(src, true)) => self.shells.add("zsh", PLUGINS, format!("source {src}/{}", quote(&file))),
                        Some(Val::Path(p)) => self.shells.add("zsh", PLUGINS, format!("source {}", quote(&p.join(&file).display().to_string()))),
                        _ => {
                            self.shells.fetched.push((name.clone(), file.clone()));
                            self.found.skip(
                                format!("{what}.{name}"),
                                "its source is fetched by Nix: install the plugin's package and source it in shell.zsh (`source \"$(nami_share <path>)\"`)",
                            )
                        }
                    }
                }
            }
            "history" => {
                let mut lines = Vec::new();
                for (hk, t) in self.attrs(v) {
                    let hv = self.force(&t);
                    let line = match (hk.as_str(), &hv) {
                        ("size", Val::Int(n)) => format!("HISTSIZE={n}"),
                        ("save", Val::Int(n)) => format!("SAVEHIST={n}"),
                        ("path", Val::Str(p, _)) => format!("HISTFILE={}", p.replace("${config.xdg.dataHome}", "$HOME/.local/share")),
                        ("ignorePatterns", _) => match self.strs(&hv) {
                            Some(p) if !p.is_empty() => format!("HISTORY_IGNORE={}", quote(&format!("({})", p.join("|")))),
                            _ => continue,
                        },
                        (opt, Val::Bool(b)) => {
                            let name = match opt {
                                "ignoreDups" => "HIST_IGNORE_DUPS",
                                "ignoreAllDups" => "HIST_IGNORE_ALL_DUPS",
                                "ignoreSpace" => "HIST_IGNORE_SPACE",
                                "expireDuplicatesFirst" => "HIST_EXPIRE_DUPS_FIRST",
                                "share" => "SHARE_HISTORY",
                                "extended" => "EXTENDED_HISTORY",
                                "append" => "APPEND_HISTORY",
                                "saveNoDups" => "HIST_SAVE_NO_DUPS",
                                "findNoDups" => "HIST_FIND_NO_DUPS",
                                _ => {
                                    self.found.skip(format!("{what}.{opt}"), "no nami equivalent");
                                    continue;
                                }
                            };
                            format!("{} {name}", if *b { "setopt" } else { "unsetopt" })
                        }
                        (_, Val::Null) => continue,
                        _ => {
                            self.found.skip(format!("{what}.{hk}"), "refers to values only Nix knows");
                            continue;
                        }
                    };
                    lines.push(line);
                }
                if !lines.is_empty() {
                    self.shells.add("zsh", HISTORY, lines.join("\n"));
                }
            }
            "defaultKeymap" => match v.str() {
                Some("emacs") => self.shells.add("zsh", KEYMAP, "bindkey -e"),
                Some("viins") => self.shells.add("zsh", KEYMAP, "bindkey -v"),
                Some("vicmd") => self.shells.add("zsh", KEYMAP, "bindkey -a"),
                _ => self.found.skip(what, "not emacs, viins or vicmd"),
            },
            "autocd" => self.shells.add("zsh", HISTORY, if on(v) { "setopt autocd" } else { "unsetopt autocd" }),
            "setOptions" => {
                for o in self.strs(v).unwrap_or_default() {
                    self.shells.add("zsh", HISTORY, format!("setopt {o}"));
                }
            }
            "shellGlobalAliases" => {
                for (n, t) in self.attrs(v) {
                    match self.force(&t) {
                        Val::Str(s, true) => self.shells.add("zsh", GLOBAL_ALIASES, format!("alias -g {n}={}", quote(&s))),
                        _ => self.found.skip(format!("{what}.{n}"), "refers to values only Nix knows"),
                    }
                }
            }
            "dirHashes" => {
                for (n, t) in self.attrs(v) {
                    if let Some(d) = self.force(&t).str() {
                        self.shells.add("zsh", GLOBAL_ALIASES, format!("hash -d {n}={}", quote(d)));
                    }
                }
            }
            "cdpath" => {
                if let Some(d) = self.strs(v) {
                    self.shells.add("zsh", HISTORY, format!("cdpath=({})", d.iter().map(|d| quote(d)).collect::<Vec<_>>().join(" ")));
                }
            }
            "localVariables" => {
                for (n, t) in self.attrs(v) {
                    match self.force(&t) {
                        Val::Str(s, true) => self.shells.add("zsh", FIRST, format!("typeset -g {n}={}", quote(&s))),
                        Val::Int(i) => self.shells.add("zsh", FIRST, format!("typeset -g {n}={i}")),
                        _ => self.found.skip(format!("{what}.{n}"), "not text or a number"),
                    }
                }
            }
            "loginExtra" | "logoutExtra" | "profileExtra" => self.found.skip(what, "for login shells only: nami's shell.zsh runs in every interactive zsh"),
            "antidote" | "zplug" | "prezto" | "zprof" => {
                if enabled(self, v) {
                    self.found.skip(what, "a plugin manager: install the plugins' packages and source them in shell.zsh instead")
                }
            }
            "dotDir" => self.found.skip(what, "nami hooks into ~/.zshrc"),
            _ => return false,
        }
        true
    }

    /// `programs.bash.<k>`. True: handled.
    pub(super) fn bash_option(&mut self, k: &str, v: &Val) -> bool {
        let what = format!("programs.bash.{k}");
        match k {
            "initExtra" | "bashrcExtra" => match self.shell_code(v) {
                Some(code) => self.shells.add("bash", if k == "bashrcExtra" { FIRST } else { INIT }, code),
                None => self.found.skip(what, "refers to values only Nix knows: copy it into shell.bash"),
            },
            "historySize" | "historyFileSize" => match v {
                Val::Int(n) => self.shells.add("bash", HISTORY, format!("{}={n}", if k == "historySize" { "HISTSIZE" } else { "HISTFILESIZE" })),
                _ => self.found.skip(what, "not a number"),
            },
            "historyFile" => match v.str() {
                Some(f) => self.shells.add("bash", HISTORY, format!("HISTFILE={}", quote(f))),
                None => self.found.skip(what, "refers to values only Nix knows"),
            },
            "historyControl" | "historyIgnore" => match self.strs(v) {
                Some(l) => self.shells.add("bash", HISTORY, format!("{}={}", if k == "historyControl" { "HISTCONTROL" } else { "HISTIGNORE" }, quote(&l.join(":")))),
                None => self.found.skip(what, "refers to values only Nix knows"),
            },
            "shellOptions" => {
                for o in self.strs(v).unwrap_or_default() {
                    match o.strip_prefix('-') {
                        Some(off) => self.shells.add("bash", HISTORY, format!("shopt -u {off}")),
                        None => self.shells.add("bash", HISTORY, format!("shopt -s {o}")),
                    }
                }
            }
            "profileExtra" | "logoutExtra" => self.found.skip(what, "for login shells only: nami's shell.bash runs in every interactive bash"),
            "enableVteIntegration" => {}
            _ => return false,
        }
        true
    }

    /// `programs.fish.<k>`. True: handled.
    pub(super) fn fish_option(&mut self, k: &str, v: &Val) -> bool {
        let what = format!("programs.fish.{k}");
        match k {
            "interactiveShellInit" | "shellInit" | "shellInitLast" => match self.shell_code(v) {
                Some(code) => {
                    if k == "shellInit" {
                        self.found.warn("programs.fish.shellInit: shell.fish runs in interactive shells only; variables for everything go in env:");
                    }
                    let order = match k {
                        "shellInit" => FIRST,
                        "shellInitLast" => LAST,
                        _ => INIT,
                    };
                    self.shells.add("fish", order, code);
                }
                None => self.found.skip(what, "refers to values only Nix knows: copy it into shell.fish"),
            },
            "functions" => {
                for (name, t) in self.attrs(v) {
                    let f = self.force(&t);
                    let (body, args) = match &f {
                        Val::Str(b, true) => (Some(b.clone()), String::new()),
                        Val::Attrs(_) => {
                            let body = self.ev.select(&f, "body").and_then(|b| match b {
                                Val::Str(s, true) => Some(s),
                                _ => None,
                            });
                            let mut args = String::new();
                            if let Some(d) = self.ev.select(&f, "description").and_then(|d| d.str().map(String::from)) {
                                args += &format!(" --description {}", quote(&d));
                            }
                            if let Some(a) = self.ev.select(&f, "argumentNames").and_then(|a| self.strs(&a)) {
                                args += &format!(" --argument-names {}", a.join(" "));
                            }
                            for (opt, flag) in [("onEvent", "--on-event"), ("onVariable", "--on-variable")] {
                                if let Some(e) = self.ev.select(&f, opt).and_then(|e| self.strs(&e)) {
                                    for e in e {
                                        args += &format!(" {flag} {e}");
                                    }
                                }
                            }
                            (body, args)
                        }
                        _ => (None, String::new()),
                    };
                    match body {
                        Some(b) => self.shells.add("fish", FUNCTIONS, format!("function {name}{args}\n{}\nend", b.trim_end())),
                        None => self.found.skip(format!("{what}.{name}"), "refers to values only Nix knows"),
                    }
                }
            }
            "plugins" => {
                if !self.ev.list(v).is_empty() {
                    self.found.skip(what, "fetched by Nix: install the plugins' packages; fish finds what they put in share/fish")
                }
            }
            "loginShellInit" => self.found.skip(what, "for login shells only: nami's shell.fish runs in every interactive fish"),
            "generateCompletions" => {}
            _ => return false,
        }
        true
    }

    /// After everything is read: the hooks that are on, and zsh's completion
    /// (home-manager runs compinit unless told not to, and oh-my-zsh does it itself).
    pub(super) fn finish_shells(&mut self) {
        let hooks = std::mem::take(&mut self.shells.hooks);
        for (prog, said) in &hooks {
            let on: Vec<&str> = ["bash", "zsh", "fish"].into_iter().filter(|sh| said.get(*sh).copied().unwrap_or_else(|| self.shells.on.iter().any(|s| s == sh))).collect();
            if let Some((_, aliases)) = ALIAS_HOOKS.iter().find(|(p, _)| p == prog) {
                if !on.is_empty() {
                    for (a, cmd) in *aliases {
                        self.found.aliases.entry(a.to_string()).or_insert(cmd.to_string());
                    }
                }
                continue;
            }
            let Some((_, bash, zsh, fish)) = SHELL_HOOKS.iter().find(|(p, ..)| p == prog) else { continue };
            let order = HOOKS + SHELL_HOOKS.iter().position(|(p, ..)| p == prog).unwrap_or(0) as u32;
            for (sh, line) in [("bash", bash), ("zsh", zsh), ("fish", fish)] {
                if on.contains(&sh) {
                    self.shells.add(sh, order, *line);
                }
            }
        }
        if self.shells.project_hook {
            for (sh, line) in [("bash", r#"eval "$(nami hook bash)""#), ("zsh", r#"eval "$(nami hook zsh)""#), ("fish", "nami hook fish | source")] {
                if self.shells.on.iter().any(|s| s == sh) {
                    self.shells.add(sh, HOOKS + 50, line);
                }
            }
        }
        let zsh_on = self.shells.on.iter().any(|s| s == "zsh");
        if zsh_on && !self.shells.oh_my_zsh && self.shells.zsh_completion.unwrap_or(true) {
            let init = self.shells.zsh_completion_init.take().unwrap_or_else(|| "autoload -U compinit && compinit".into());
            self.shells.add("zsh", COMPINIT, init);
        }
        for (sh, text) in self.shells.text() {
            self.found.shell.entry(sh).or_insert(text);
        }
    }

    // ---------- the desktop's look ----------

    /// A GTK setting, in settings.ini for GTK 3 and 4 (what home-manager's
    /// `gtk` writes), and the matching GNOME setting GTK 4 apps read.
    fn gtk_setting(&mut self, ini_key: &str, value: Value, gnome_key: Option<&str>) {
        for rel in [".config/gtk-3.0/settings.ini", ".config/gtk-4.0/settings.ini"] {
            let file = self.found.files.entry(rel.into()).or_insert_with(|| Value::Mapping(Mapping::new()));
            if let Value::Mapping(m) = file {
                let section = m.entry(Value::String("Settings".into())).or_insert_with(|| Value::Mapping(Mapping::new()));
                if let Value::Mapping(s) = section {
                    s.entry(Value::String(ini_key.into())).or_insert(value.clone());
                }
            }
        }
        if let Some(g) = gnome_key {
            self.found.settings.entry("org.gnome.desktop.interface".into()).or_default().entry(g.into()).or_insert(value);
        }
    }

    /// A `{ name, package, size }` theme: its package installed, (name, size).
    fn theme(&mut self, v: &Val, what: &str) -> Option<(String, Option<i64>)> {
        match self.ev.select(v, "package") {
            Some(Val::Pkg(p)) if !p.is_empty() => self.found.package(&p, what),
            Some(Val::Null) | None => {}
            Some(_) => self.found.skip(format!("{what}.package"), "not a nixpkgs package"),
        }
        let name = self.ev.select(v, "name").and_then(|n| n.str().map(String::from))?;
        let size = match self.ev.select(v, "size") {
            Some(Val::Int(n)) => Some(n),
            _ => None,
        };
        Some((name, size))
    }

    /// `home.pointerCursor`: the cursor theme for X11, Wayland and GTK.
    pub(super) fn pointer_cursor(&mut self, v: &Val) {
        let what = "home.pointerCursor";
        if matches!(self.ev.select(v, "enable"), Some(Val::Bool(false))) {
            return;
        }
        let Some((name, size)) = self.theme(v, what) else {
            return self.found.skip(what, "no cursor theme name nami can read");
        };
        // What X11 and Wayland toolkits read, and the fallback theme for apps
        // that only look at ~/.icons/default.
        self.found.env.entry("XCURSOR_THEME".into()).or_insert(name.clone());
        if let Some(s) = size {
            self.found.env.entry("XCURSOR_SIZE".into()).or_insert(s.to_string());
        }
        self.found
            .files
            .entry(".icons/default/index.theme".into())
            .or_insert(Value::String(format!("[Icon Theme]\nName=Default\nComment=Default Cursor Theme\nInherits={name}\n")));
        let gtk = self.ev.select(v, "gtk").and_then(|g| self.ev.select(&g, "enable"));
        if matches!(gtk, Some(Val::Bool(true))) {
            self.gtk_setting("gtk-cursor-theme-name", Value::String(name), Some("cursor-theme"));
            if let Some(s) = size {
                self.gtk_setting("gtk-cursor-theme-size", Value::Number(s.into()), Some("cursor-size"));
            }
        }
        for (k, t) in self.attrs(v) {
            let on = self.ev.select(&self.force(&t), "enable");
            match k.as_str() {
                "name" | "package" | "size" | "gtk" | "x11" | "enable" => {}
                _ if matches!(on, Some(Val::Bool(true))) => self.found.skip(format!("{what}.{k}"), format!("{k}'s cursor setting: set it in {k}'s own config")),
                _ => {}
            }
        }
    }

    /// home-manager's `gtk`: themes, icons, cursor, font, extra settings.
    pub(super) fn gtk(&mut self, v: &Val) {
        if !matches!(self.ev.select(v, "enable"), Some(Val::Bool(true))) {
            return;
        }
        for (k, t) in self.attrs(v) {
            let gv = self.force(&t);
            if matches!(gv, Val::Null) {
                continue;
            }
            let what = format!("gtk.{k}");
            match k.as_str() {
                "enable" => {}
                "theme" => {
                    if let Some((name, _)) = self.theme(&gv, &what) {
                        self.gtk_setting("gtk-theme-name", Value::String(name), Some("gtk-theme"));
                    }
                }
                "iconTheme" => {
                    if let Some((name, _)) = self.theme(&gv, &what) {
                        self.gtk_setting("gtk-icon-theme-name", Value::String(name), Some("icon-theme"));
                    }
                }
                "cursorTheme" => {
                    if let Some((name, size)) = self.theme(&gv, &what) {
                        self.gtk_setting("gtk-cursor-theme-name", Value::String(name), Some("cursor-theme"));
                        if let Some(s) = size {
                            self.gtk_setting("gtk-cursor-theme-size", Value::Number(s.into()), Some("cursor-size"));
                        }
                    }
                }
                "font" => {
                    if let Some((name, size)) = self.theme(&gv, &what) {
                        let font = match size {
                            Some(s) => format!("{name} {s}"),
                            None => name,
                        };
                        self.gtk_setting("gtk-font-name", Value::String(font), Some("font-name"));
                    }
                }
                "colorScheme" => match gv.str() {
                    Some("dark") => {
                        self.gtk_setting("gtk-application-prefer-dark-theme", Value::Bool(true), None);
                        self.found.settings.entry("org.gnome.desktop.interface".into()).or_default().entry("color-scheme".into()).or_insert(Value::String("prefer-dark".into()));
                    }
                    Some("light") => {
                        self.found.settings.entry("org.gnome.desktop.interface".into()).or_default().entry("color-scheme".into()).or_insert(Value::String("prefer-light".into()));
                    }
                    _ => self.found.skip(what, "not dark or light"),
                },
                "gtk3" | "gtk4" => {
                    let n = &k[3..];
                    for (gk, t) in self.attrs(&gv) {
                        let x = self.force(&t);
                        match (gk.as_str(), &x) {
                            (_, Val::Null) => {}
                            ("extraConfig", _) => {
                                let rel = format!(".config/gtk-{n}.0/settings.ini");
                                for (sk, t) in self.attrs(&x) {
                                    match self.ev.to_yaml(&self.force(&t)) {
                                        Ok(y) => {
                                            let file = self.found.files.entry(rel.clone()).or_insert_with(|| Value::Mapping(Mapping::new()));
                                            if let Value::Mapping(m) = file {
                                                if let Value::Mapping(s) = m.entry(Value::String("Settings".into())).or_insert_with(|| Value::Mapping(Mapping::new())) {
                                                    s.entry(Value::String(sk)).or_insert(y);
                                                }
                                            }
                                        }
                                        Err(why) => self.found.skip(format!("{what}.extraConfig.{sk}"), why),
                                    }
                                }
                            }
                            ("extraCss", Val::Str(css, true)) => {
                                self.found.files.entry(format!(".config/gtk-{n}.0/gtk.css")).or_insert(Value::String(css.clone()));
                            }
                            ("bookmarks", _) => match self.strs(&x) {
                                Some(b) => {
                                    self.found.files.entry(".config/gtk-3.0/bookmarks".into()).or_insert(Value::String(b.join("\n") + "\n"));
                                }
                                None => self.found.skip(format!("{what}.bookmarks"), "refers to values only Nix knows"),
                            },
                            _ => self.found.skip(format!("{what}.{gk}"), "no nami equivalent"),
                        }
                    }
                }
                "gtk2" => self.found.skip(what, "GTK 2 apps: copy its settings into home.files.\".gtkrc-2.0\""),
                _ => self.found.skip(what, "no nami equivalent"),
            }
        }
    }
}
