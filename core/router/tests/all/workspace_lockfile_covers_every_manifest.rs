// SPDX-License-Identifier: CC0-1.0

//! The committed `Cargo.lock` covers every dependency the workspace's manifests declare.
//!
//! The release build resolves with `--locked`, which forbids cargo from writing
//! `Cargo.lock`: a dependency some manifest declares that the lock does not carry at an
//! accepted version stops the build before a single crate compiles, with
//! `cannot update the lock file ... because --locked was passed`. This suite reads the
//! committed manifests and the committed lock and pairs them, so a dependency added
//! without refreshing the lock fails here — in the workspace's own tests — instead of in
//! a platform build nobody can rerun locally.
//!
//! It reads files only; it resolves nothing, spawns nothing and reaches no network.
//!
//! # Why it reads the two files itself
//!
//! The suite adds no dependency to the router, so it reads the manifests and the lock —
//! both generated — line by line rather than through a TOML or semver crate, joining a
//! declaration that runs over several lines back into one. It is conservative by
//! construction: a requirement form it does not recognise counts as satisfied, so it can
//! only fail on a dependency the lock genuinely omits, never on manifest syntax it has not
//! met. `an_unlocked_dependency_is_reported` pins that it does fail on one, and
//! `no_manifest_declares_a_dependency_in_a_form_the_suite_cannot_read` pins that the
//! declaration forms it does not read — a `[dependencies.<name>]` sub-table and a dotted
//! key — are absent from the tree rather than silently skipped.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A dependency as a manifest declares it: `name = "<req>"`, or an inline table with any
/// of `version`, `package` (the crate the lock knows, when the key is an alias) and
/// `workspace = true` (the requirement lives in the root `[workspace.dependencies]`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Declared {
    package: Option<String>,
    req: Option<String>,
    from_workspace: bool,
}

/// The crate names the lock carries, each with the versions locked for it.
type Locked = BTreeMap<String, Vec<String>>;

/// The workspace root: this crate's manifest sits at `<root>/core/router`.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the router crate sits two directories below the workspace root")
        .to_path_buf()
}

fn unquote(value: &str) -> &str {
    value.trim().trim_matches('"')
}

/// Every `[[package]]` in the lock, as crate name -> the versions locked for it.
fn locked_packages(lock: &str) -> Locked {
    let mut locked: Locked = BTreeMap::new();
    let mut name: Option<String> = None;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            name = None;
        } else if let Some(value) = line.strip_prefix("name = ") {
            name = Some(unquote(value).to_string());
        } else if let Some(value) = line.strip_prefix("version = ") {
            if let Some(name) = name.take() {
                locked
                    .entry(name)
                    .or_default()
                    .push(unquote(value).to_string());
            }
        }
    }
    locked
}

/// The directories listed in the root manifest's `members` array.
fn workspace_members(root_manifest: &str) -> Vec<String> {
    let mut members = Vec::new();
    let mut inside = false;
    for line in root_manifest.lines() {
        let line = line.trim();
        if !inside {
            inside = line.starts_with("members") && line.ends_with('[');
            continue;
        }
        if line.starts_with(']') {
            break;
        }
        let entry = unquote(line.trim_end_matches(','));
        if !entry.is_empty() && !entry.starts_with('#') {
            members.push(entry.to_string());
        }
    }
    members
}

/// The `name` of the `[package]` a manifest defines.
fn package_name(manifest: &str) -> Option<String> {
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[package]";
        } else if inside {
            if let Some(value) = line.strip_prefix("name = ") {
                return Some(unquote(value).to_string());
            }
        }
    }
    None
}

fn parse_declared(value: &str) -> Declared {
    let Some(table) = value.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) else {
        return Declared {
            req: Some(unquote(value).to_string()),
            ..Declared::default()
        };
    };
    let mut declared = Declared::default();
    for field in table.split(',') {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        match key.trim() {
            "package" => declared.package = Some(unquote(value).to_string()),
            "version" => declared.req = Some(unquote(value).to_string()),
            "workspace" => declared.from_workspace = unquote(value) == "true",
            _ => {}
        }
    }
    declared
}

/// Whether a line closes every brace and bracket it opens, counting only those outside a
/// quoted string: a brace inside a value (a `description` with one in it) opens nothing.
fn is_balanced(line: &str) -> bool {
    let mut depth = 0i32;
    let mut quoted = false;
    for character in line.chars() {
        match character {
            '"' => quoted = !quoted,
            '{' | '[' if !quoted => depth += 1,
            '}' | ']' if !quoted => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

/// A manifest's lines, with a declaration spread over several of them joined back into one:
/// cargo lets an inline table's feature array run over lines, and a declaration read as its
/// first line alone would hide the version requirement the rest of it carries.
fn logical_lines(manifest: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut open: Option<String> = None;
    for line in manifest.lines() {
        let line = line.trim();
        let joined = match open.take() {
            Some(mut pending) => {
                pending.push(' ');
                pending.push_str(line);
                pending
            }
            None if line.starts_with('#') => {
                lines.push(line.to_string());
                continue;
            }
            None => line.to_string(),
        };
        if is_balanced(&joined) {
            lines.push(joined);
        } else {
            open = Some(joined);
        }
    }
    lines.extend(open);
    lines
}

/// Every dependency a manifest declares in the tables `wanted` accepts, as the table's
/// header, the name the manifest uses and the declaration. A name may be declared once per
/// table — `[dependencies]` and `[dev-dependencies]` can require different versions of one
/// crate — so these are a sequence, not a map keyed by name.
fn dependencies_in(manifest: &str, wanted: fn(&str) -> bool) -> Vec<(String, String, Declared)> {
    let mut declared = Vec::new();
    let mut table = String::new();
    let mut inside = false;
    for line in logical_lines(manifest) {
        let line = line.trim();
        if line.starts_with('[') {
            inside = wanted(line);
            if inside {
                table = line.to_string();
            }
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        // `version.workspace = true` and its siblings are package fields, not dependencies.
        if key.is_empty() || key.contains('.') {
            continue;
        }
        declared.push((table.clone(), key.to_string(), parse_declared(value.trim())));
    }
    declared
}

/// The root `[workspace.dependencies]` table a member's `workspace = true` resolves through,
/// keyed by the name a member declares. It is one table, so no name appears twice.
fn workspace_dependency_table(root_manifest: &str) -> BTreeMap<String, Declared> {
    dependencies_in(root_manifest, is_workspace_dependency_table)
        .into_iter()
        .map(|(_table, name, declared)| (name, declared))
        .collect()
}

fn is_member_dependency_table(section: &str) -> bool {
    matches!(
        section,
        "[dependencies]" | "[dev-dependencies]" | "[build-dependencies]"
    ) || (section.starts_with("[target.") && section.ends_with("dependencies]"))
}

fn is_workspace_dependency_table(section: &str) -> bool {
    section == "[workspace.dependencies]"
}

/// One report per dependency `manifest` declares in a form `dependencies_in` reads as
/// nothing, and so would leave unchecked: a `[dependencies.<name>]` sub-table, whose header
/// `is_member_dependency_table` does not accept, and a dotted key inside a dependency table
/// (`serde.workspace = true`), which that reader skips because the same spelling in
/// `[package]` is a package field rather than a dependency. Both are failed here rather than
/// passed over in silence.
fn unreadable_dependency_forms(manifest_path: &str, manifest: &str) -> Vec<String> {
    let mut reports = Vec::new();
    let mut inside = false;
    for (number, line) in manifest.lines().enumerate() {
        let line = line.trim();
        if line.starts_with('[') {
            // `[workspace.dependencies]` and `[target.'…'.dependencies]` put their dot
            // BEFORE the word; a sub-table is the only header with one after it.
            if line.contains("dependencies.") {
                reports.push(format!(
                    "case {manifest_path:?} line {}: expected the dependency in a \
                     `[…dependencies]` table, got the sub-table header {line:?}",
                    number + 1
                ));
            }
            inside = is_member_dependency_table(line) || is_workspace_dependency_table(line);
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line
            .split_once('=')
            .is_some_and(|(key, _)| key.trim().contains('.'))
        {
            reports.push(format!(
                "case {manifest_path:?} line {}: expected the whole declaration on the \
                 dependency's own key, got the dotted key {line:?}",
                number + 1
            ));
        }
    }
    reports
}

/// `major.minor.patch` and how many of the three the text actually spelled out.
fn parse_version(value: &str) -> Option<([u64; 3], usize)> {
    let mut parsed = [0u64; 3];
    let mut spelled = 0usize;
    for part in value.trim().split('.') {
        if spelled == 3 {
            return None;
        }
        parsed[spelled] = part.parse().ok()?;
        spelled += 1;
    }
    (spelled > 0).then_some((parsed, spelled))
}

/// Whether `version` falls inside the requirement `req`, for the two forms this workspace's
/// manifests use: cargo's default caret requirement (bare or `^`-prefixed), which admits
/// versions from the one spelled out up to the next increment of its leftmost non-zero
/// component, and the `=` pin, which admits only the components it spells out. Any other
/// form — a comparison, a tilde, a wildcard, a union, a prerelease — is reported accepted:
/// this suite pins lock COVERAGE, and cargo stays the authority on the requirement forms it
/// does not read here.
fn accepts(req: &str, version: &str) -> bool {
    let Some((actual, _)) = parse_version(version) else {
        return true;
    };
    let req = req.trim();
    if let Some(pinned) = req.strip_prefix('=') {
        let Some((wanted, spelled)) = parse_version(pinned) else {
            return true;
        };
        return wanted[..spelled] == actual[..spelled];
    }
    let Some((lower, spelled)) = parse_version(req.strip_prefix('^').unwrap_or(req)) else {
        return true;
    };
    let bump = (0..spelled)
        .find(|&slot| lower[slot] != 0)
        .unwrap_or(spelled - 1);
    let mut upper = lower;
    upper[bump] += 1;
    for slot in upper.iter_mut().skip(bump + 1) {
        *slot = 0;
    }
    actual >= lower && actual < upper
}

/// How many dependencies `manifest` declares, and one report per dependency the lock does
/// not carry at a version the manifest's requirement accepts.
fn unlocked_dependencies(
    manifest_path: &str,
    manifest: &str,
    workspace_deps: &BTreeMap<String, Declared>,
    locked: &Locked,
) -> (usize, Vec<String>) {
    let declared = dependencies_in(manifest, is_member_dependency_table);
    let mut reports = Vec::new();
    for (table, dependency, spec) in &declared {
        let resolved = if spec.from_workspace {
            match workspace_deps.get(dependency) {
                Some(root_spec) => root_spec.clone(),
                None => {
                    reports.push(format!(
                        "case {manifest_path:?} {table} -> {dependency:?}: declared \
                         `workspace = true`, expected the root [workspace.dependencies] to \
                         declare it, got nothing"
                    ));
                    continue;
                }
            }
        } else {
            spec.clone()
        };
        let crate_name = resolved.package.clone().unwrap_or_else(|| dependency.clone());
        let Some(versions) = locked.get(&crate_name) else {
            reports.push(format!(
                "case {manifest_path:?} {table} -> {dependency:?}: expected Cargo.lock to \
                 carry the crate {crate_name:?}, got no [[package]] with that name"
            ));
            continue;
        };
        if let Some(req) = &resolved.req {
            if !versions.iter().any(|version| accepts(req, version)) {
                reports.push(format!(
                    "case {manifest_path:?} {table} -> {dependency:?}: expected a locked \
                     version of {crate_name:?} that {req:?} accepts, got {versions:?}"
                ));
            }
        }
    }
    (declared.len(), reports)
}

/// The root manifest, the lock's packages, and every member's manifest text.
fn workspace() -> (String, Locked, Vec<(String, String)>) {
    let root = workspace_root();
    let root_manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml");
    let locked = locked_packages(&fs::read_to_string(root.join("Cargo.lock")).expect("read lock"));
    let members = workspace_members(&root_manifest);
    assert!(
        members.len() > 1,
        "the root manifest must list the workspace members, read {} of them",
        members.len()
    );
    let manifests = members
        .into_iter()
        .map(|member| {
            let path = root.join(&member).join("Cargo.toml");
            let manifest = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            (format!("{member}/Cargo.toml"), manifest)
        })
        .collect();
    (root_manifest, locked, manifests)
}

#[test]
fn every_workspace_member_is_a_locked_package() {
    let (_, locked, manifests) = workspace();
    for (manifest, text) in &manifests {
        let name = package_name(text)
            .unwrap_or_else(|| panic!("case {manifest:?}: the manifest names no [package]"));
        assert!(
            locked.contains_key(&name),
            "case {manifest:?}: expected Cargo.lock to carry the member package {name:?}, \
             got no [[package]] with that name"
        );
    }
}

#[test]
fn every_dependency_a_workspace_manifest_declares_is_locked() {
    let (root_manifest, locked, manifests) = workspace();
    let workspace_deps = workspace_dependency_table(&root_manifest);
    let mut checked = 0usize;
    let mut reports = Vec::new();
    for (manifest, text) in &manifests {
        let (declared, unlocked) =
            unlocked_dependencies(manifest, text, &workspace_deps, &locked);
        checked += declared;
        reports.extend(unlocked);
    }
    assert!(
        reports.is_empty(),
        "expected every declared dependency to be locked, got {} that are not, so the \
         release build cannot resolve with --locked: {reports:#?}",
        reports.len()
    );
    assert!(
        checked > 100,
        "expected the workspace's manifests to declare dependencies to check, got {checked}"
    );
}

#[test]
fn no_manifest_declares_a_dependency_in_a_form_the_suite_cannot_read() {
    let (root_manifest, _, manifests) = workspace();
    let mut reports = unreadable_dependency_forms("Cargo.toml", &root_manifest);
    for (manifest, text) in &manifests {
        reports.extend(unreadable_dependency_forms(manifest, text));
    }
    assert!(
        reports.is_empty(),
        "expected every dependency to be declared on its own key in a `[…dependencies]` \
         table, which is what the coverage check above reads, got {} that are not: \
         {reports:#?}",
        reports.len()
    );
    // Every form that would hide a dependency from the coverage check, so this guard is
    // known to fail on each rather than to pass over it, and the two spellings next to them
    // that are NOT dependencies and must stay accepted.
    let cases = [
        ("a sub-table", "[dependencies.absent-crate]\nversion = \"1\"\n", 1),
        ("a dev-dependency sub-table", "[dev-dependencies.absent-crate]\nversion = \"1\"\n", 1),
        ("a dotted key in a dependency table", "[dependencies]\nabsent-crate.workspace = true\n", 1),
        ("a dotted key in a target dependency table", "[target.'cfg(unix)'.dependencies]\nabsent-crate.version = \"1\"\n", 1),
        ("a dotted package field", "[package]\nversion.workspace = true\n", 0),
        ("a dependency on its own key", "[dependencies]\nlocked-crate = { workspace = true }\n", 0),
    ];
    for case in cases {
        let (name, manifest, expected) = case;
        let reports = unreadable_dependency_forms(name, manifest);
        assert_eq!(
            reports.len(),
            expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}

#[test]
fn an_unlocked_dependency_is_reported() {
    // One synthetic manifest per way a declaration reaches the lock, against a lock that
    // carries `locked-crate 1.2.3` and the renamed `renamed-crate 0.4.0` only.
    let locked: Locked = [
        ("locked-crate".to_string(), vec!["1.2.3".to_string()]),
        ("renamed-crate".to_string(), vec!["0.4.0".to_string()]),
    ]
    .into_iter()
    .collect();
    let workspace_deps = workspace_dependency_table(
        "[workspace.dependencies]\n\
         locked-crate = \"1\"\n\
         alias = { path = \"vendor/renamed\", package = \"renamed-crate\" }\n",
    );
    let cases = [
        ("a locked crate at an accepted version", "[dependencies]\nlocked-crate = \"1.2\"\n", 0),
        ("a crate the lock does not carry", "[dependencies]\nabsent-crate = \"1\"\n", 1),
        ("a locked crate below the requirement", "[dependencies]\nlocked-crate = \"1.3\"\n", 1),
        ("a locked crate in another major", "[dependencies]\nlocked-crate = \"2\"\n", 1),
        ("an exact pin on the locked version", "[dependencies]\nlocked-crate = \"=1.2.3\"\n", 0),
        ("an exact pin on another version", "[dependencies]\nlocked-crate = \"=1.2.4\"\n", 1),
        ("a dev-dependency the lock does not carry", "[dev-dependencies]\nabsent-crate = \"1\"\n", 1),
        ("a build-dependency the lock carries", "[build-dependencies]\nlocked-crate = \"1\"\n", 0),
        ("a target dependency the lock does not carry", "[target.'cfg(unix)'.dependencies]\nabsent-crate = \"1\"\n", 1),
        ("a workspace dependency the root table declares", "[dependencies]\nlocked-crate = { workspace = true }\n", 0),
        ("a renamed workspace dependency", "[dependencies]\nalias = { workspace = true }\n", 0),
        ("a workspace dependency the root table omits", "[dependencies]\nabsent-crate = { workspace = true }\n", 1),
        ("a path dependency with no requirement", "[dependencies]\nlocked-crate = { path = \"../locked\" }\n", 0),
        ("a package field, not a dependency", "[package]\nname = \"absent-crate\"\nversion.workspace = true\n", 0),
        ("a multi-line declaration of a locked crate", "[dependencies]\nlocked-crate = { version = \"1\", features = [\n    \"one\",\n] }\n", 0),
        ("a multi-line declaration below the requirement", "[dependencies]\nlocked-crate = { version = \"1.3\", features = [\n    \"one\",\n] }\n", 1),
        ("a multi-line declaration of an absent crate", "[dependencies]\nabsent-crate = { version = \"1\", features = [\n    \"one\",\n] }\n", 1),
        ("one name required differently by two tables", "[dependencies]\nlocked-crate = \"1.3\"\n\n[dev-dependencies]\nlocked-crate = \"1\"\n", 1),
        ("a brace inside a value, before a dependency table", "[package]\ndescription = \"takes a { brace\"\n\n[dependencies]\nabsent-crate = \"1\"\n", 1),
    ];
    for case in cases {
        let (name, manifest, expected) = case;
        let (_, reports) = unlocked_dependencies(name, manifest, &workspace_deps, &locked);
        assert_eq!(
            reports.len(),
            expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}

#[test]
fn caret_and_exact_requirements_accept_what_cargo_accepts() {
    // Rows from cargo's documented default (caret) and `=` requirement semantics: the
    // upper bound follows the leftmost non-zero component spelled out. The last two rows
    // are forms this suite deliberately does not read, and so reports accepted.
    let cases = [
        ("1", "1.0.0", true),
        ("1", "1.9.2", true),
        ("1", "2.0.0", false),
        ("1", "0.9.9", false),
        ("^1.2.3", "1.2.3", true),
        ("^1.2.3", "1.2.2", false),
        ("0.22", "0.22.0", true),
        ("0.22", "0.22.9", true),
        ("0.22", "0.23.0", false),
        ("0.22", "0.21.9", false),
        ("0.0.3", "0.0.3", true),
        ("0.0.3", "0.0.4", false),
        ("0", "0.9.1", true),
        ("0", "1.0.0", false),
        ("=3.20.3", "3.20.3", true),
        ("=3.20.3", "3.20.4", false),
        ("=3.20.3", "3.21.0", false),
        ("*", "1.2.3", true),
        (">=1, <3", "2.0.0", true),
    ];
    for case in cases {
        let (req, version, expected) = case;
        let actual = accepts(req, version);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the requirement to accept the version = {expected}, \
             got {actual}"
        );
    }
}
