//! Ticket 25: the repository is a Claude Code plugin marketplace. A cheap
//! check of what `claude plugin validate --strict` checks in full: both
//! manifests parse, names match, sources exist, and every skill has
//! frontmatter with a name and a description.

use serde_json::Value;
use std::path::{Path, PathBuf};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn text(v: &Value, key: &str, of: &str) -> String {
    match v[key].as_str() {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => panic!("{of}: no {key}"),
    }
}

/// The `key: value` lines between a SKILL.md's opening and closing `---`.
fn frontmatter(path: &Path) -> Vec<(String, String)> {
    let body = std::fs::read_to_string(path).unwrap();
    let mut lines = body.lines();
    assert_eq!(
        lines.next(),
        Some("---"),
        "{}: no frontmatter",
        path.display()
    );
    let fields: Vec<&str> = lines.by_ref().take_while(|l| *l != "---").collect();
    assert!(
        lines.next().is_some(),
        "{}: frontmatter not closed, or nothing after it",
        path.display()
    );
    fields
        .iter()
        .map(|l| {
            let (k, v) = l
                .split_once(':')
                .unwrap_or_else(|| panic!("{}: bad line {l:?}", path.display()));
            (k.trim().to_string(), v.trim().to_string())
        })
        .collect()
}

#[test]
fn the_marketplace_lists_the_paddock_plugin_and_every_skill_has_frontmatter() {
    let root = Path::new(ROOT);
    let market = read_json(&root.join(".claude-plugin/marketplace.json"));
    assert_eq!(text(&market, "name", "marketplace"), "paddock");
    text(&market["owner"], "name", "marketplace owner");
    let plugins = market["plugins"].as_array().expect("plugins");
    assert_eq!(plugins.len(), 1);
    for entry in plugins {
        let name = text(entry, "name", "plugin entry");
        let source = text(entry, "source", &name);
        assert!(
            source.starts_with("./") && !source.contains(".."),
            "{source}"
        );
        let dir: PathBuf = root.join(&source);
        let manifest = read_json(&dir.join(".claude-plugin/plugin.json"));
        assert_eq!(
            text(&manifest, "name", "plugin.json"),
            name,
            "entry and manifest names"
        );
        text(&manifest, "description", "plugin.json");
        text(&manifest, "version", "plugin.json");

        let mut skills: Vec<String> = std::fs::read_dir(dir.join("skills"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        skills.sort();
        assert_eq!(
            skills,
            ["paddock-worktrees", "worktree-list", "worktree-new"]
        );
        for skill in &skills {
            let path = dir.join("skills").join(skill).join("SKILL.md");
            let fields = frontmatter(&path);
            let get = |k: &str| {
                fields
                    .iter()
                    .find(|(key, _)| key == k)
                    .map(|(_, v)| v.as_str())
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| panic!("{}: no {k}", path.display()))
            };
            assert_eq!(get("name"), skill, "name matches its directory");
            get("description");
            // `rm` is never pre-approved: the user confirms each delete.
            let allowed = fields
                .iter()
                .find(|(k, _)| k == "allowed-tools")
                .map_or("", |(_, v)| v.as_str());
            assert!(!allowed.contains("rm"), "{skill}: {allowed}");
            assert!(!allowed.contains("worktree *"), "{skill}: {allowed}");
        }
    }
}
