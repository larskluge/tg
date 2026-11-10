//! AGENTS.md is the one instruction file for agents in this repository, and it stays small
//! (tg-eib).
//!
//! Every agent turn here loads it whole: Codex's as AGENTS.md, Claude's through CLAUDE.md, a link
//! to it. When each feature appended its description to it, it grew to 59 KB. How a feature works
//! belongs in docs/architecture/, the notes behind the real-TDLib tests in docs/build-and-test.md
//! (AGENTS.md, "Docs"). The smoke runs these: `cargo test -- cli::tests agents_md`.

mod agents_md {
    use std::fs;
    use std::path::{Path, PathBuf};

    const MOST_BYTES: usize = 24_000;
    /// A line longer than this is a feature's description pasted in: owl's generated block stays
    /// under it.
    const MOST_LINE: usize = 400;
    const MOVE: &str = "move the detail to docs/architecture/<area>.md or docs/build-and-test.md \
                        (AGENTS.md, \"Docs\")";

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// The relative targets of a page's Markdown links, without their `#fragment`.
    fn local_links(page: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = page;
        while let Some(at) = rest.find("](") {
            rest = &rest[at + 2..];
            let Some(end) = rest.find(')') else { break };
            let target = rest[..end].split('#').next().unwrap_or_default();
            let scheme = target
                .split_once(':')
                .is_some_and(|(s, _)| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase()));
            if !target.is_empty() && !scheme && !target.contains(char::is_whitespace) {
                out.push(target.to_string());
            }
            rest = &rest[end..];
        }
        out
    }

    #[test]
    fn claude_md_is_a_relative_link_to_it() {
        let link = root().join("CLAUDE.md");
        let target = fs::read_link(&link).unwrap_or_else(|e| {
            panic!("CLAUDE.md is not a link ({e}): make it one, ln -sf AGENTS.md CLAUDE.md")
        });
        assert_eq!(
            target,
            Path::new("AGENTS.md"),
            "CLAUDE.md links to {target:?}, not AGENTS.md"
        );
    }

    #[test]
    fn stays_small() {
        let size = read(&root().join("AGENTS.md")).len();
        assert!(
            size <= MOST_BYTES,
            "AGENTS.md is {size} bytes, over {MOST_BYTES}: {MOVE}"
        );
    }

    #[test]
    fn no_line_holds_a_feature() {
        let long: Vec<usize> = read(&root().join("AGENTS.md"))
            .lines()
            .enumerate()
            .filter(|(_, line)| line.chars().count() > MOST_LINE)
            .map(|(n, _)| n + 1)
            .collect();
        assert!(
            long.is_empty(),
            "AGENTS.md lines over {MOST_LINE} characters, {long:?}: {MOVE}"
        );
    }

    #[test]
    fn its_links_and_the_architecture_index_resolve() {
        for page in [
            root().join("AGENTS.md"),
            root().join("docs/architecture/README.md"),
        ] {
            let dir = page.parent().expect("a page has a folder");
            for target in local_links(&read(&page)) {
                assert!(
                    dir.join(&target).exists(),
                    "{} links to {target}, which is missing",
                    page.display()
                );
            }
        }
    }

    #[test]
    fn local_links_reads_relative_targets_only() {
        let page = "[a](docs/x.md#part) [b](https://example.com) [c](mailto:a@b.c) [d](../y.md)";
        assert_eq!(local_links(page), ["docs/x.md", "../y.md"]);
    }
}
