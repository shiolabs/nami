//! Where nami is on the web: two addresses, kept apart on purpose.

/// nami's public address. It's written into people's files and stays
/// there: a schema's `$id` and the editor's first line
/// (`# yaml-language-server: $schema=...`). Nothing else may bake a host
/// into a file (nami.lock names its catalog's channel and release, not
/// where they came from), so moving nami to another domain never breaks a
/// file someone wrote.
pub const PUBLIC: &str = "https://namios.dev";

/// Where nami fetches from today: the catalog, the installer, new versions
/// of nami itself, and the docs it links to. It's the public address once
/// that one's DNS is live: then this becomes [`PUBLIC`] (computers keep
/// using the catalog address they were installed with, so both have to be
/// served for a while).
pub const SERVED_FROM: &str = "https://namios.dev";

/// The address of one kind of file's schema (`computer`: .../schema/computer.json).
pub fn schema_url(kind: &str) -> String {
    format!("{PUBLIC}/schema/{kind}.json")
}

/// A page of the docs, where they're served today (`nami/recipes`).
pub fn docs_url(page: &str) -> String {
    format!("{SERVED_FROM}/docs/{page}")
}

/// The file in nami's folder that marks a test computer: the installer
/// leaves it when run with `NAMI_TEST=1` (packaging/install-web.sh).
pub const TEST_MARK: &str = "test-computer";

/// A test computer (a VM, a container, CI): [`TEST_MARK`] in nami's folder,
/// or `NAMI_TEST=1` where nami runs.
pub fn is_test(var_dir: &std::path::Path) -> bool {
    test_said(std::env::var("NAMI_TEST").ok().as_deref()) || var_dir.join(TEST_MARK).exists()
}

fn test_said(v: Option<&str>) -> bool {
    v.is_some_and(|v| !matches!(v.trim(), "" | "0" | "no" | "false"))
}

/// `url` as nami asks its site for it: on a test computer with `via=test`,
/// so the site counts the update check or download as a test's, not a
/// person's (site/worker.js, /privacy). A file, or anyone else's computer: as it is.
pub fn counted(store: &crate::store::Store, url: &str) -> String {
    if (url.starts_with("https://") || url.starts_with("http://")) && is_test(&store.var_dir()) {
        with_test(url)
    } else {
        url.to_string()
    }
}

fn with_test(url: &str) -> String {
    format!("{url}{}via=test", if url.contains('?') { '&' } else { '?' })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_computer_says_so() {
        assert_eq!(with_test("https://namios.dev/menu/nixos-unstable"), "https://namios.dev/menu/nixos-unstable?via=test");
        assert_eq!(with_test("https://namios.dev/releases/1.0/nami-1.0-x86_64-linux.tar.gz?via=update"), "https://namios.dev/releases/1.0/nami-1.0-x86_64-linux.tar.gz?via=update&via=test");
        assert!(test_said(Some("1")) && test_said(Some("yes")));
        assert!(!test_said(None) && !test_said(Some("")) && !test_said(Some("0")) && !test_said(Some("false")));
        let dir = std::env::temp_dir().join(format!("nami-test-mark-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(TEST_MARK), "").unwrap();
        assert!(is_test(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn files_get_the_public_address() {
        assert_eq!(schema_url("computer"), "https://namios.dev/schema/computer.json");
        assert!(docs_url("nami/recipes").starts_with(SERVED_FROM));
        assert!(!PUBLIC.ends_with('/') && !SERVED_FROM.ends_with('/'));
    }
}
