// SPDX-License-Identifier: CC0-1.0

//! `BatchGetRepositories` over the wire, driven as awsJson1_1 requests through the
//! router that serves the real emulator.
//!
//! Declared in `tests/all.rs`; the wire harness (`emulator`, `call`, `ok`, `error`,
//! `string_at`) comes from the sibling `repository_lifecycle` suite rather than a
//! second copy.
//!
//! The rule under test is the one every batch read follows: for each name the request
//! lists, the batch answers the SAME stored projection the single-resource read
//! (`GetRepository`) answers, names it holds no repository for are reported in
//! `repositoriesNotFound`, and the call changes nothing a later caller sees. The tables
//! below span what the rule generalises over — how many names a request carries, their
//! order, whether they are held or absent, repeated names, the empty array, and the
//! published limit on the number of names — rather than one recorded request.

use serde_json::{json, Value};

use super::harness::metadata_of;
use super::repository_lifecycle::{call, emulator, error, ok, string_at};

/// The repositories the batch tests seed, spanning `RepositoryName`'s modeled bounds
/// (`length` 1..100, pattern `^[\w\.-]+$`) and the description forms
/// `RepositoryDescription` allows (absent, empty, text).
fn seeded_repositories() -> Vec<(String, Option<String>)> {
    vec![
        ("alpha-repo".to_string(), Some("the first repo".to_string())),
        // The modeled minimum name length.
        ("a".to_string(), None),
        ("nested.repo_1".to_string(), Some(String::new())),
        // The modeled maximum name length, and the maximum description length.
        ("x".repeat(100), Some("y".repeat(1000))),
    ]
}

/// One row of the batch table: the names one request lists, and which of them the
/// service must report as found (in that order) and as not found.
#[derive(Debug)]
struct BatchCase {
    label: &'static str,
    requested: Vec<String>,
    found: Vec<String>,
    not_found: Vec<String>,
}

fn batch_cases(seeded: &[String]) -> Vec<BatchCase> {
    vec![
        BatchCase {
            label: "one held name",
            requested: vec![seeded[0].clone()],
            found: vec![seeded[0].clone()],
            not_found: Vec::new(),
        },
        BatchCase {
            label: "every held name, in request order rather than store order",
            requested: seeded.iter().rev().cloned().collect(),
            found: seeded.iter().rev().cloned().collect(),
            not_found: Vec::new(),
        },
        BatchCase {
            label: "held and absent names interleaved",
            requested: vec![
                "absent-one".to_string(),
                seeded[2].clone(),
                "absent.two".to_string(),
                seeded[1].clone(),
            ],
            found: vec![seeded[2].clone(), seeded[1].clone()],
            not_found: vec!["absent-one".to_string(), "absent.two".to_string()],
        },
        BatchCase {
            label: "a repeated name names one repository",
            requested: vec![
                seeded[0].clone(),
                seeded[0].clone(),
                "absent-one".to_string(),
                "absent-one".to_string(),
            ],
            found: vec![seeded[0].clone()],
            not_found: vec!["absent-one".to_string()],
        },
        BatchCase {
            label: "only absent names",
            requested: vec!["absent-one".to_string(), "absent-three".to_string()],
            found: Vec::new(),
            not_found: vec!["absent-one".to_string(), "absent-three".to_string()],
        },
        BatchCase {
            // `repositoryNames`'s own documentation: "The array itself can be empty."
            label: "the empty array",
            requested: Vec::new(),
            found: Vec::new(),
            not_found: Vec::new(),
        },
    ]
}

/// The list at `member`, with a missing member read as the empty list.
fn list_at(payload: &Value, member: &str) -> Vec<Value> {
    payload
        .get(member)
        .map(|value| {
            value
                .as_array()
                .unwrap_or_else(|| panic!("{member} must be a list in {payload}"))
                .clone()
        })
        .unwrap_or_default()
}

fn names_of(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| string_at(entry, "/repositoryName"))
        .collect()
}

fn strings_of(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("expected a string, got {entry}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_batch_read_reports_the_same_stored_metadata_as_the_single_read() {
    let router = emulator();

    let seeded = seeded_repositories();
    for (name, description) in &seeded {
        let mut input = json!({ "repositoryName": name });
        if let Some(description) = description {
            input["repositoryDescription"] = json!(description);
        }
        ok(&router, "CreateRepository", &input).await;
    }
    let names: Vec<String> = seeded.iter().map(|(name, _)| name.clone()).collect();

    // What the single-resource read answers for each held name: the batch must answer
    // exactly this, member for member.
    let mut single: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    for name in &names {
        single.insert(name.clone(), metadata_of(&router, name).await);
    }

    for case in batch_cases(&names) {
        let label = case.label;
        let input = json!({ "repositoryNames": case.requested });
        let payload = ok(&router, "BatchGetRepositories", &input).await;

        let repositories = list_at(&payload, "repositories");
        assert_eq!(
            names_of(&repositories),
            case.found,
            "case {label:?}: repositories must be the held names in request order, got {payload}"
        );
        for entry in &repositories {
            let name = string_at(entry, "/repositoryName");
            assert_eq!(
                entry,
                single
                    .get(&name)
                    .unwrap_or_else(|| panic!("case {label:?}: {name} was never seeded")),
                "case {label:?}: the batch metadata of {name} must equal GetRepository's, got {entry}"
            );
        }
        assert_eq!(
            strings_of(&list_at(&payload, "repositoriesNotFound")),
            case.not_found,
            "case {label:?}: repositoriesNotFound must be the absent names in request order, got {payload}"
        );
        assert!(
            list_at(&payload, "errors").is_empty(),
            "case {label:?}: no repository is in an unreadable state, got {payload}"
        );

        // The read changes nothing: the same request answers the same thing again, and
        // the single read still agrees.
        let again = ok(&router, "BatchGetRepositories", &input).await;
        assert_eq!(
            again, payload,
            "case {label:?}: a repeated batch read must answer the same payload"
        );
    }

    // Nothing the batch reads mutated the store: every seeded repository is still
    // listed, and its single read is unchanged.
    let listed = ok(&router, "ListRepositories", &json!({})).await;
    let mut listed_names = strings_of(
        &list_at(&listed, "repositories")
            .iter()
            .map(|entry| json!(string_at(entry, "/repositoryName")))
            .collect::<Vec<Value>>(),
    );
    listed_names.sort();
    let mut expected_names = names.clone();
    expected_names.sort();
    assert_eq!(
        listed_names, expected_names,
        "the batch reads must not add or remove a repository, got {listed}"
    );
    for name in &names {
        assert_eq!(
            Some(&metadata_of(&router, name).await),
            single.get(name),
            "the batch reads must not change the stored metadata of {name}"
        );
    }
}

#[tokio::test]
async fn the_batch_reports_what_a_later_write_persisted() {
    let router = emulator();
    let name = "written-repo";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": name }),
    )
    .await;

    let before = ok(
        &router,
        "BatchGetRepositories",
        &json!({ "repositoryNames": [name] }),
    )
    .await;
    let before_entry = list_at(&before, "repositories")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the held repository must be reported, got {before}"));
    assert_eq!(
        before_entry.get("defaultBranch"),
        None,
        "a repository with no commit has no defaultBranch, got {before_entry}"
    );
    let created_at = before_entry
        .get("creationDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no creationDate in {before_entry}"));
    assert_eq!(
        before_entry.get("lastModifiedDate").and_then(Value::as_f64),
        Some(created_at),
        "a fresh repository was last modified when it was created, got {before_entry}"
    );

    // Every write is a row of the same rule: whatever a write persists is what the
    // batch read reports afterwards.
    ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": name,
            "branchName": "main",
            "filePath": "README.md",
            "fileContent": aws_smithy_types::base64::encode(&b"# written\n"[..]),
            "commitMessage": "init",
            "name": "Test Author",
            "email": "author@example.com",
        }),
    )
    .await;

    let after = ok(
        &router,
        "BatchGetRepositories",
        &json!({ "repositoryNames": [name] }),
    )
    .await;
    let after_entry = list_at(&after, "repositories")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the held repository must be reported, got {after}"));
    assert_eq!(
        after_entry.get("defaultBranch").and_then(Value::as_str),
        Some("main"),
        "the branch the write created must be the reported defaultBranch, got {after_entry}"
    );
    let modified_at = after_entry
        .get("lastModifiedDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no lastModifiedDate in {after_entry}"));
    assert!(
        modified_at >= created_at,
        "the write must not move lastModifiedDate backwards: created {created_at}, modified {modified_at}"
    );
    let single = metadata_of(&router, name).await;
    assert_eq!(
        after_entry, single,
        "the batch and the single read must report one stored repository identically"
    );

    // A deleted repository is a name the store no longer holds.
    ok(
        &router,
        "DeleteRepository",
        &json!({ "repositoryName": name }),
    )
    .await;
    let deleted = ok(
        &router,
        "BatchGetRepositories",
        &json!({ "repositoryNames": [name] }),
    )
    .await;
    assert!(
        list_at(&deleted, "repositories").is_empty(),
        "a deleted repository must not be reported, got {deleted}"
    );
    assert_eq!(
        strings_of(&list_at(&deleted, "repositoriesNotFound")),
        vec![name.to_string()],
        "a deleted repository's name must be reported as not found, got {deleted}"
    );
}

#[tokio::test]
async fn the_number_of_names_is_bounded_by_the_published_limit() {
    let router = emulator();
    // "The maximum number of allowed repository names was exceeded. Currently, this
    // number is 100." — `MaximumRepositoryNamesExceededException`'s own documentation
    // (`smithy_query(query="shape",shape="MaximumRepositoryNamesExceededException")`).
    const MAX_REPOSITORY_NAMES: usize = 100;

    let names = |count: usize| -> Vec<String> {
        (0..count).map(|index| format!("repo-{index}")).collect()
    };
    // One held repository, so a bounded request is not only about absent names.
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "repo-0" }),
    )
    .await;

    // Each count at or below the limit is served; the first count past it is refused.
    for count in [1usize, MAX_REPOSITORY_NAMES - 1, MAX_REPOSITORY_NAMES] {
        let payload = ok(
            &router,
            "BatchGetRepositories",
            &json!({ "repositoryNames": names(count) }),
        )
        .await;
        let found = list_at(&payload, "repositories").len();
        let missing = list_at(&payload, "repositoriesNotFound").len();
        assert_eq!(
            found + missing,
            count,
            "case {count} names: every name must be reported exactly once, got {payload}"
        );
        assert_eq!(
            found, 1,
            "case {count} names: the one held repository must be reported, got {payload}"
        );
    }
    for count in [MAX_REPOSITORY_NAMES + 1, 2 * MAX_REPOSITORY_NAMES] {
        let code = error(
            &router,
            "BatchGetRepositories",
            &json!({ "repositoryNames": names(count) }),
            400,
        )
        .await;
        assert_eq!(
            code, "MaximumRepositoryNamesExceededException",
            "case {count} names: past the limit the call must be refused"
        );
    }

    // Refusing an over-long request persisted nothing.
    let (status, payload) = call(
        &router,
        "BatchGetRepositories",
        &json!({ "repositoryNames": ["repo-0"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the refused requests must leave the service serving reads, got {payload}"
    );
    assert_eq!(
        names_of(&list_at(&payload, "repositories")),
        vec!["repo-0".to_string()],
        "the refused requests must not have changed the store, got {payload}"
    );
}
