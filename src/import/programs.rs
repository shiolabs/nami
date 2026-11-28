//! Programs whose home-manager (or NixOS) options are their own config file,
//! written out the way the module writes it: tmux (tmux.conf), ssh
//! (~/.ssh/config), fzf (its environment variables).

use std::collections::BTreeMap;

use serde_yaml::Value;

use super::{Kind, Reader, Tier};
use crate::nix::expr::{Thunk, Val};

impl Reader {
    /// The profile the target file's packages are in, as a path a config
    /// file can name.
    fn profile_dir(&self) -> String {
        if self.want == Some(Tier::System) {
            // (A file goes from computer to computer: by the place every one has.)
            format!("/{}/profiles/default", crate::migrate::VAR)
        } else {
            "~/.local/state/nami/profile".to_string()
        }
    }

    fn scalar(&self, v: &Val) -> Option<String> {
        match v {
            Val::Str(s, true) => Some(s.clone()),
            Val::Int(n) => Some(n.to_string()),
            Val::Bool(b) => Some(if *b { "yes" } else { "no" }.to_string()),
            Val::Path(p) => Some(p.display().to_string()),
            _ => None,
        }
    }

    /// `programs.tmux`: tmux.conf, as home-manager / NixOS write it.
    pub(super) fn tmux(&mut self, m: &BTreeMap<String, Thunk>, kind: Kind) {
        let mut conf: Vec<String> = Vec::new();
        let mut tail: Vec<String> = Vec::new();
        let get = |r: &Self, k: &str| m.get(k).map(|t| r.force(t)).filter(|v| !matches!(v, Val::Null));
        let on = |r: &Self, k: &str| matches!(get(r, k), Some(Val::Bool(true)));
        let what = |k: &str| format!("programs.tmux.{k}");
        if let Some(Val::Int(n)) = get(self, "baseIndex") {
            conf.push(format!("set -g base-index {n}\nsetw -g pane-base-index {n}"));
        }
        if on(self, "clock24") {
            conf.push("setw -g clock-mode-style 24".into());
        }
        if let Some(Val::Int(n)) = get(self, "escapeTime") {
            conf.push(format!("set -s escape-time {n}"));
        }
        if let Some(Val::Int(n)) = get(self, "historyLimit") {
            conf.push(format!("set -g history-limit {n}"));
        }
        if let Some(k) = get(self, "keyMode").and_then(|v| v.str().map(String::from)) {
            conf.push(format!("set -g status-keys {k}\nset -g mode-keys {k}"));
        }
        if on(self, "mouse") {
            conf.push("set -g mouse on".into());
        }
        if on(self, "focusEvents") {
            conf.push("set -g focus-events on".into());
        }
        if on(self, "aggressiveResize") {
            conf.push("setw -g aggressive-resize on".into());
        }
        let prefix = get(self, "prefix").and_then(|v| v.str().map(String::from)).or_else(|| get(self, "shortcut").and_then(|v| v.str().map(|s| format!("C-{s}"))));
        if let Some(p) = prefix {
            conf.push(format!("unbind C-b\nset -g prefix {p}\nbind {p} send-prefix"));
        }
        if let Some(t) = get(self, "terminal").and_then(|v| v.str().map(String::from)) {
            conf.push(format!("set -g default-terminal \"{t}\""));
        }
        match get(self, "shell") {
            Some(Val::Str(s, true)) => conf.push(format!("set -g default-shell \"{s}\"")),
            Some(Val::Pkg(p)) => conf.push(format!("set -g default-shell \"{}/bin/{}\"", self.profile_dir(), p.rsplit('.').next().unwrap_or(&p))),
            Some(_) => self.found.skip(what("shell"), "refers to values only Nix knows"),
            None => {}
        }
        if on(self, "customPaneNavigationAndResize") {
            let amount = match get(self, "resizeAmount") {
                Some(Val::Int(n)) => n,
                _ => 5,
            };
            conf.push(format!(
                "bind -N \"Select pane to the left\" h select-pane -L\nbind -N \"Select pane below\" j select-pane -D\nbind -N \"Select pane above\" k select-pane -U\nbind -N \"Select pane to the right\" l select-pane -R\nbind -r H resize-pane -L {amount}\nbind -r J resize-pane -D {amount}\nbind -r K resize-pane -U {amount}\nbind -r L resize-pane -R {amount}"
            ));
        }
        if on(self, "disableConfirmationPrompt") {
            conf.push("bind-key & kill-window\nbind-key x kill-pane".into());
        }
        if on(self, "newSession") {
            conf.push("new-session".into());
        }
        if on(self, "reverseSplit") {
            conf.push("bind v split-window -h\nbind s split-window -v".into());
        }
        // Plugins: their packages, and each one's .tmux scripts run, with its
        // own settings before.
        let mut plugins = Vec::new();
        if on(self, "sensibleOnTop") {
            plugins.push(("tmuxPlugins.sensible".to_string(), None));
        }
        for item in get(self, "plugins").map(|p| self.ev.list(&p)).unwrap_or_default() {
            let (pkg, extra) = match &item {
                Val::Pkg(p) => (Some(p.clone()), None),
                Val::Attrs(_) => (
                    match self.ev.select(&item, "plugin") {
                        Some(Val::Pkg(p)) => Some(p),
                        _ => None,
                    },
                    self.ev.select(&item, "extraConfig").and_then(|e| e.str().map(String::from)),
                ),
                _ => (None, None),
            };
            match pkg {
                Some(p) => plugins.push((p, extra)),
                None => self.found.skip(what("plugins"), "a plugin that isn't a nixpkgs package"),
            }
        }
        for (pkg, extra) in plugins {
            self.found.package(&pkg, &what("plugins"));
            let short = pkg.rsplit('.').next().unwrap_or(&pkg).trim_start_matches("tmuxplugin-").to_string();
            if let Some(e) = extra {
                tail.push(e.trim_end().to_string());
            }
            tail.push(format!("run-shell 'for f in {}/share/tmux-plugins/{short}/*.tmux; do \"$f\"; done'", self.profile_dir()));
        }
        match get(self, "extraConfig") {
            Some(Val::Str(e, true)) => tail.push(e.trim_end().to_string()),
            Some(_) => self.found.skip(what("extraConfig"), "refers to values only Nix knows: copy it into the tmux.conf"),
            None => {}
        }
        let known = [
            "enable", "package", "baseIndex", "clock24", "escapeTime", "historyLimit", "keyMode", "mouse", "focusEvents", "aggressiveResize", "prefix", "shortcut", "terminal", "shell",
            "customPaneNavigationAndResize", "resizeAmount", "disableConfirmationPrompt", "newSession", "reverseSplit", "sensibleOnTop", "plugins", "extraConfig", "secureSocket",
            "tmuxp", "tmuxinator",
        ];
        for (k, t) in m {
            let v = self.force(t);
            let empty = matches!(v, Val::Null | Val::Bool(false)) || matches!(&v, Val::List(l) if l.is_empty());
            if !known.contains(&k.as_str()) && !empty {
                self.found.skip(what(k), "no nami equivalent: add it to the tmux.conf");
            } else if (k == "tmuxp" || k == "tmuxinator") && self.ev.select(&v, "enable").is_some_and(|e| matches!(e, Val::Bool(true))) {
                self.found.package(k, &what(k));
            }
        }
        conf.extend(tail);
        if conf.is_empty() {
            return;
        }
        let text = conf.join("\n") + "\n";
        if kind == Kind::Nixos {
            self.found.etc_files.entry("/etc/tmux.conf".into()).or_insert(Value::String(text));
        } else {
            self.found.files.entry(".config/tmux/tmux.conf".into()).or_insert(Value::String(text));
        }
    }

    /// home-manager's `programs.ssh`: ~/.ssh/config.
    pub(super) fn ssh_config(&mut self, m: &BTreeMap<String, Thunk>) {
        const NAMES: &[(&str, &str)] = &[
            ("hostname", "HostName"), ("user", "User"), ("port", "Port"), ("identityFile", "IdentityFile"), ("identitiesOnly", "IdentitiesOnly"),
            ("identityAgent", "IdentityAgent"), ("forwardAgent", "ForwardAgent"), ("forwardX11", "ForwardX11"), ("forwardX11Trusted", "ForwardX11Trusted"),
            ("proxyJump", "ProxyJump"), ("proxyCommand", "ProxyCommand"), ("serverAliveInterval", "ServerAliveInterval"),
            ("serverAliveCountMax", "ServerAliveCountMax"), ("compression", "Compression"), ("certificateFile", "CertificateFile"),
            ("addKeysToAgent", "AddKeysToAgent"), ("sendEnv", "SendEnv"), ("checkHostIP", "CheckHostIP"), ("addressFamily", "AddressFamily"),
            ("controlMaster", "ControlMaster"), ("controlPath", "ControlPath"), ("controlPersist", "ControlPersist"), ("hashKnownHosts", "HashKnownHosts"),
            ("userKnownHostsFile", "UserKnownHostsFile"), ("kexAlgorithms", "KexAlgorithms"),
        ];
        let mut lines = Vec::new();
        let options = |r: &mut Self, block: &Val, what: &str, out: &mut Vec<String>| {
            for (k, t) in r.attrs(block) {
                let v = r.force(&t);
                if matches!(v, Val::Null) || matches!(&v, Val::List(l) if l.is_empty()) {
                    continue;
                }
                let name = NAMES.iter().find(|(n, _)| *n == k).map(|(_, s)| *s);
                match (k.as_str(), name) {
                    ("host" | "match" | "enable" | "data" | "dagOrder", _) => {}
                    ("extraOptions", _) => {
                        for (ek, et) in r.attrs(&v) {
                            match r.scalar(&r.force(&et)) {
                                Some(ev) => out.push(format!("  {ek} {ev}")),
                                None => r.found.skip(format!("{what}.extraOptions.{ek}"), "refers to values only Nix knows"),
                            }
                        }
                    }
                    ("setEnv", _) => {
                        let pairs: Vec<String> = r.attrs(&v).into_iter().filter_map(|(ek, et)| r.scalar(&r.force(&et)).map(|ev| format!("{ek}={ev}"))).collect();
                        if !pairs.is_empty() {
                            out.push(format!("  SetEnv {}", pairs.join(" ")));
                        }
                    }
                    ("localForwards" | "remoteForwards", _) => {
                        let key = if k == "localForwards" { "LocalForward" } else { "RemoteForward" };
                        for f in r.ev.list(&v) {
                            let part = |side: &str| {
                                let s = r.ev.select(&f, side)?;
                                let port = r.ev.select(&s, "port").and_then(|p| r.scalar(&p));
                                let addr = r.ev.select(&s, "address").and_then(|a| a.str().map(String::from));
                                Some(match (addr, port) {
                                    (Some(a), Some(p)) => format!("{a}:{p}"),
                                    (Some(a), None) => a,
                                    (None, Some(p)) => p,
                                    (None, None) => return None,
                                })
                            };
                            match (part("bind"), part("host")) {
                                (Some(b), Some(h)) => out.push(format!("  {key} {b} {h}")),
                                _ => r.found.skip(format!("{what}.{k}"), "a forward nami can't read"),
                            }
                        }
                    }
                    ("dynamicForwards", _) => {
                        for f in r.ev.list(&v) {
                            if let Some(p) = r.ev.select(&f, "port").and_then(|p| r.scalar(&p)) {
                                out.push(format!("  DynamicForward {p}"));
                            }
                        }
                    }
                    (_, Some(n)) => {
                        let vals: Vec<String> = match &v {
                            Val::List(_) => r.ev.list(&v).iter().filter_map(|x| r.scalar(x)).collect(),
                            x => r.scalar(x).into_iter().collect(),
                        };
                        if vals.is_empty() {
                            r.found.skip(format!("{what}.{k}"), "refers to values only Nix knows");
                        } else if n == "IdentityFile" || n == "CertificateFile" {
                            out.extend(vals.iter().map(|x| format!("  {n} {x}")));
                        } else {
                            out.push(format!("  {n} {}", vals.join(" ")));
                        }
                    }
                    _ => r.found.skip(format!("{what}.{k}"), "no nami equivalent: add it to ~/.ssh/config"),
                }
            }
        };
        let get = |r: &Self, k: &str| m.get(k).map(|t| r.force(t)).filter(|v| !matches!(v, Val::Null));
        for inc in get(self, "includes").map(|i| self.strs(&i).unwrap_or_default()).unwrap_or_default() {
            lines.push(format!("Include {inc}"));
        }
        let mut blocks: Vec<(String, Val)> = get(self, "matchBlocks").map(|b| self.attrs(&b).into_iter().map(|(n, t)| (n, self.force(&t))).collect()).unwrap_or_default();
        // Specific hosts first, `*` itself last: ssh takes the first value
        // it finds.
        blocks.sort_by_key(|(n, _)| (n == "*", n.contains('*')));
        // home-manager's ordered entries (`dag.entryAfter [..] { ... }`):
        // the block is their `data`, placed after/before the blocks named.
        let mut order: Vec<(String, Val, Vec<String>, Vec<String>)> = Vec::new();
        for (name, block) in blocks {
            let names = |r: &Self, k: &str| r.ev.select(&block, k).and_then(|v| r.strs(&v)).unwrap_or_default();
            match self.ev.select(&block, "data") {
                Some(data) if matches!(data, Val::Attrs(_)) => {
                    let (after, before) = (names(self, "after"), names(self, "before"));
                    order.push((name, data, after, before));
                }
                _ => order.push((name, block, Vec::new(), Vec::new())),
            }
        }
        let mut blocks = Vec::new();
        while !order.is_empty() {
            let waiting = |i: usize| {
                let (name, _, after, _) = &order[i];
                after.iter().any(|a| order.iter().any(|o| &o.0 == a)) || order.iter().any(|o| o.3.contains(name))
            };
            // The first that waits for nothing left (a cycle: the first).
            let i = (0..order.len()).find(|&i| !waiting(i)).unwrap_or(0);
            let (name, block, _, _) = order.remove(i);
            blocks.push((name, block));
        }
        for (name, block) in blocks {
            if matches!(self.ev.select(&block, "enable"), Some(Val::Bool(false))) {
                continue;
            }
            let head = match (self.ev.select(&block, "match").and_then(|x| x.str().map(String::from)), self.ev.select(&block, "host").and_then(|x| x.str().map(String::from))) {
                (Some(mt), _) => format!("Match {mt}"),
                (None, Some(h)) => format!("Host {h}"),
                (None, None) => format!("Host {name}"),
            };
            let mut out = vec![String::new(), head];
            options(self, &block, &format!("programs.ssh.matchBlocks.{name}"), &mut out);
            lines.extend(out);
        }
        // What applies to every host, last (`Host *`).
        let top: BTreeMap<String, Thunk> = m.iter().filter(|(k, _)| NAMES.iter().any(|(n, _)| n == k)).map(|(k, t)| (k.clone(), t.clone())).collect();
        let mut all = Vec::new();
        options(self, &Val::Attrs(std::rc::Rc::new(top)), "programs.ssh", &mut all);
        if let Some(Val::Str(e, true)) = get(self, "extraConfig") {
            all.extend(e.lines().map(|l| format!("  {}", l.trim())));
        } else if get(self, "extraConfig").is_some() {
            self.found.skip("programs.ssh.extraConfig", "refers to values only Nix knows: copy it into ~/.ssh/config");
        }
        if !all.is_empty() {
            lines.push(String::new());
            lines.push("Host *".into());
            lines.extend(all);
        }
        for k in m.keys() {
            let handled = ["enable", "package", "includes", "matchBlocks", "extraConfig", "enableDefaultConfig", "startAgent"].contains(&k.as_str()) || NAMES.iter().any(|(n, _)| n == k);
            if !handled && !matches!(self.force(&m[k]), Val::Null | Val::Bool(false)) {
                self.found.skip(format!("programs.ssh.{k}"), "no nami equivalent: add it to ~/.ssh/config");
            }
        }
        let text = lines.join("\n").trim_start().to_string();
        if !text.is_empty() {
            self.found.files.entry(".ssh/config".into()).or_insert(Value::String(text + "\n"));
        }
    }

    /// `programs.fzf.<k>`: fzf's environment variables. True: handled.
    pub(super) fn fzf_option(&mut self, k: &str, v: &Val) -> bool {
        let var = match k {
            "defaultCommand" => "FZF_DEFAULT_COMMAND",
            "defaultOptions" => "FZF_DEFAULT_OPTS",
            "fileWidgetCommand" => "FZF_CTRL_T_COMMAND",
            "fileWidgetOptions" => "FZF_CTRL_T_OPTS",
            "changeDirWidgetCommand" => "FZF_ALT_C_COMMAND",
            "changeDirWidgetOptions" => "FZF_ALT_C_OPTS",
            "historyWidgetOptions" => "FZF_CTRL_R_OPTS",
            "colors" => {
                let pairs: Vec<String> = self.attrs(v).into_iter().filter_map(|(c, t)| self.force(&t).str().map(|x| format!("{c}:{x}"))).collect();
                if !pairs.is_empty() {
                    let opts = self.found.env.entry("FZF_DEFAULT_OPTS".into()).or_default();
                    *opts = format!("{opts} --color {}", pairs.join(",")).trim().to_string();
                }
                return true;
            }
            "tmux" => {
                if self.ev.select(v, "enableShellIntegration").is_some_and(|e| matches!(e, Val::Bool(true))) {
                    self.found.env.entry("FZF_TMUX".into()).or_insert("1".into());
                }
                return true;
            }
            _ => return false,
        };
        let value = match v {
            Val::Str(s, true) => Some(s.clone()),
            Val::List(_) => self.strs(v).map(|l| l.join(" ")),
            _ => None,
        };
        match value {
            Some(x) => {
                let e = self.found.env.entry(var.into()).or_default();
                *e = if e.is_empty() { x } else { format!("{x} {e}") };
            }
            None => self.found.skip(format!("programs.fzf.{k}"), "refers to values only Nix knows"),
        }
        true
    }
}
