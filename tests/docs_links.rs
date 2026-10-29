//! Ticket 56: the README is a front page and the details live in docs/.
//! Every relative link between README.md, CONTEXT.md and the pages under
//! docs/ resolves, and every `#anchor` names a heading of its target, by
//! GitHub's slug rules. docs/README.md lists every user page, and the
//! README repeats that list.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// The Markdown files checked: README.md, CONTEXT.md, docs/*.md and
/// docs/adr/*.md.
fn markdown_files() -> Vec<PathBuf> {
    let root = Path::new(ROOT);
    let mut files = vec![root.join("README.md"), root.join("CONTEXT.md")];
    for dir in ["docs", "docs/adr"] {
        let mut found: Vec<PathBuf> = std::fs::read_dir(root.join(dir))
            .unwrap_or_else(|e| panic!("{dir}: {e}"))
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "md"))
            .collect();
        found.sort();
        files.extend(found);
    }
    files
}

/// The lines outside fenced code blocks (``` or ~~~; a block ends at a
/// fence of the kind that opened it).
fn prose(text: &str) -> Vec<&str> {
    let mut open: Option<&str> = None;
    text.lines()
        .filter(|line| {
            let line = line.trim_start();
            let Some(fence) = ["```", "~~~"].into_iter().find(|f| line.starts_with(f)) else {
                return open.is_none();
            };
            match open {
                None => open = Some(fence),
                Some(o) if o == fence => open = None,
                Some(_) => {}
            }
            false
        })
        .collect()
}

/// `line` without its inline code spans.
fn without_code(line: &str) -> String {
    let mut out = String::new();
    let mut parts = line.split('`');
    while let Some(text) = parts.next() {
        out.push_str(text);
        if parts.next().is_none() {
            break;
        }
    }
    out
}

/// The targets of the `[text](target)` links in `text`.
fn links(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in prose(text) {
        let line = without_code(line);
        let mut rest = line.as_str();
        while let Some(at) = rest.find("](") {
            rest = &rest[at + 2..];
            let Some(end) = rest.find(')') else { break };
            let target = rest[..end].split_whitespace().next().unwrap_or("");
            out.push(target.to_string());
            rest = &rest[end..];
        }
    }
    out
}

/// GitHub's anchor for a heading's text: inline code and link markup
/// dropped, lowercase, letters, digits, `-` and `_` kept, spaces as `-`.
fn slug(heading: &str) -> String {
    let mut text = heading.replace('`', "");
    // `[text](url)` reads as `text`.
    while let (Some(open), Some(mid)) = (text.find('['), text.find("](")) {
        let Some(close) = text[mid..].find(')').map(|c| mid + c) else {
            break;
        };
        if open > mid {
            break;
        }
        text = format!(
            "{}{}{}",
            &text[..open],
            &text[open + 1..mid],
            &text[close + 1..]
        );
    }
    text.to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// The anchors of `text`'s headings, a repeated one numbered `-1`, `-2`.
fn anchors(text: &str) -> HashSet<String> {
    let mut seen = HashSet::new();
    for line in prose(text) {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&hashes) || !line[hashes..].starts_with(' ') {
            continue;
        }
        let base = slug(line[hashes..].trim());
        let mut anchor = base.clone();
        let mut n = 0;
        while seen.contains(&anchor) {
            n += 1;
            anchor = format!("{base}-{n}");
        }
        seen.insert(anchor);
    }
    seen
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn slugs_follow_github() {
    assert_eq!(slug("Connect Claude Code"), "connect-claude-code");
    assert_eq!(
        slug("Move from `hooks install` to the plugin"),
        "move-from-hooks-install-to-the-plugin"
    );
    assert_eq!(slug("From paddock 0.0.x (tmux)"), "from-paddock-00x-tmux");
    assert_eq!(slug("See [Upgrade](x.md)"), "see-upgrade");
    let a = anchors("# A\n## Notes\n```\n# not a heading\n```\n## Notes\n");
    assert!(a.contains("notes") && a.contains("notes-1"), "{a:?}");
    assert!(!a.contains("not-a-heading"), "{a:?}");
    let t = anchors("~~~\n```\n# in tildes\n~~~\n# Out\n");
    assert!(!t.contains("in-tildes") && t.contains("out"), "{t:?}");
}

#[test]
fn every_relative_link_and_anchor_resolves() {
    let mut broken = Vec::new();
    for file in markdown_files() {
        let text = read(&file);
        let dir = file.parent().unwrap();
        for target in links(&text) {
            if target.is_empty() || target.contains("://") || target.starts_with("mailto:") {
                continue;
            }
            let (path, anchor) = match target.split_once('#') {
                Some((p, a)) => (p, Some(a)),
                None => (target.as_str(), None),
            };
            let resolved = if path.is_empty() {
                file.clone()
            } else {
                dir.join(path)
            };
            if !resolved.exists() {
                broken.push(format!("{}: {target}: no such file", file.display()));
                continue;
            }
            let Some(anchor) = anchor else { continue };
            // Only a Markdown page's anchors are headings; `LICENSE#L10`
            // and the like are not checked.
            if resolved.extension().is_none_or(|x| x != "md") {
                continue;
            }
            if !anchors(&read(&resolved)).contains(anchor) {
                broken.push(format!("{}: {target}: no such heading", file.display()));
            }
        }
    }
    assert!(broken.is_empty(), "broken links:\n{}", broken.join("\n"));
}

/// The `- ` list items of `text`'s section under `heading`.
fn list_under<'a>(text: &'a str, heading: &str) -> Vec<&'a str> {
    text.lines()
        .skip_while(|l| *l != heading)
        .skip(1)
        .take_while(|l| !l.starts_with('#'))
        .filter(|l| l.starts_with("- "))
        .collect()
}

#[test]
fn the_docs_index_lists_every_user_page_and_the_readme_repeats_it() {
    let index = read(&Path::new(ROOT).join("docs/README.md"));
    let pages = list_under(&index, "## Using paddock");
    let listed: HashSet<String> = pages.iter().flat_map(|l| links(l)).collect();
    for entry in std::fs::read_dir(Path::new(ROOT).join("docs")).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if !name.ends_with(".md") || name == "README.md" || name == "design.md" {
            continue;
        }
        assert!(
            listed.contains(&name),
            "docs/README.md does not list {name}"
        );
    }
    // The README's list is the index's, with links from the root.
    let readme = read(&Path::new(ROOT).join("README.md"));
    let want: Vec<String> = pages.iter().map(|l| l.replace("](", "](docs/")).collect();
    assert_eq!(
        list_under(&readme, "## Docs"),
        want,
        "README's ## Docs list"
    );
}
