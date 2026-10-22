// SPDX-License-Identifier: CC0-1.0

//! `UpdateRepositoryDescription` over the wire, driven as awsJson1_1 requests through
//! the router that serves the real emulator.
//!
//! The rule under test: `UpdateRepositoryDescription` sets the named repository's
//! stored description to exactly the value the request carries — and leaves it with
//! none when the request carries none — so a later `GetRepository` reports that
//! value, the repository's `lastModifiedDate` advances to the instant of the change,
//! and nothing else about the repository (or any other repository) moves.
//!
//! The table below spans the class that rule covers: the description present or
//! absent, at both modelled length bounds and past the upper one, and over the
//! character content the operation's documentation says the field accepts (all HTML
//! characters and all valid Unicode characters) — rather than one recorded request.

use std::time::Duration;

use serde_json::{json, Value};

use crate::harness::{metadata_of, seconds_at};
use crate::repository_lifecycle::{emulator, error, ok};

/// A `repositoryName` plus an optional `repositoryDescription` — the input shape
/// `CreateRepository` and `UpdateRepositoryDescription` share.
fn with_description(name: &str, description: Option<&str>) -> Value {
    let mut input = json!({ "repositoryName": name });
    if let Some(description) = description {
        input["repositoryDescription"] = json!(description);
    }
    input
}

/// Metadata members that a description change must not touch.
const INVARIANT_MEMBERS: [&str; 7] = [
    "accountId",
    "repositoryId",
    "repositoryName",
    "creationDate",
    "cloneUrlHttp",
    "cloneUrlSsh",
    "Arn",
];

/// The table. Rows differ along what the rule generalises over: the value of
/// `repositoryDescription` — absent, both modelled length bounds (0 and 1000, from
/// `smithy_query(query="shape",shape="RepositoryDescription")`: `length` 0..1000),
/// and the character content the operation's documentation says the field accepts
/// ("accepts all HTML characters and all valid Unicode characters"). The rows are
/// applied in sequence to one repository, so each also exercises replacing the
/// previous value — including setting one after it was cleared.
fn description_cases() -> Vec<Option<String>> {
    vec![
        // Ordinary text.
        Some("the team's build tooling".to_string()),
        // The modelled minimum length.
        Some(String::new()),
        // "accepts all HTML characters".
        Some("<b>build</b> &amp; test <script>alert(\"x\")</script>".to_string()),
        // "all valid Unicode characters".
        Some("説明 — ünïcode ✅ 🚀 \u{1F1E6}\u{1F1FA}".to_string()),
        // The modelled maximum length.
        Some("z".repeat(1000)),
        // A request that carries no description leaves the repository without one.
        None,
        // Set again after the clear.
        Some("restored after clearing".to_string()),
        // A single character: the shortest non-empty value.
        Some("x".to_string()),
    ]
}

#[tokio::test]
async fn an_updated_description_is_read_back_from_state() {
    let router = emulator();
    // Two starting states — a repository created WITH a description and one created
    // without — plus a third that is never updated, so the write is shown to reach
    // only the repository the request names.
    ok(
        &router,
        "CreateRepository",
        &with_description("with-description", Some("created with this")),
    )
    .await;
    ok(
        &router,
        "CreateRepository",
        &with_description("without-description", None),
    )
    .await;
    ok(
        &router,
        "CreateRepository",
        &with_description("untouched", Some("never updated")),
    )
    .await;
    let untouched_before = metadata_of(&router, "untouched").await;
    // The table is built once; every target walks the same rows.
    let cases = description_cases();

    for target in ["with-description", "without-description"] {
        let identity = metadata_of(&router, target).await;
        let created = seconds_at(&identity, "creationDate");
        let mut previous_modified = seconds_at(&identity, "lastModifiedDate");

        for case in &cases {
            let label = case
                .as_deref()
                .map(|text| format!("{target}: {} chars", text.chars().count()))
                .unwrap_or_else(|| format!("{target}: absent"));
            // Separate the modification instants so lastModifiedDate strictly orders.
            tokio::time::sleep(Duration::from_millis(5)).await;

            let response = ok(
                &router,
                "UpdateRepositoryDescription",
                &with_description(target, case.as_deref()),
            )
            .await;
            assert!(
                response.is_null() || response.as_object().is_some_and(|members| members.is_empty()),
                "case {label:?}: the operation is modelled with a Unit output, got {response}"
            );

            let after = metadata_of(&router, target).await;
            assert_eq!(
                after.get("repositoryDescription").and_then(Value::as_str),
                case.as_deref(),
                "case {label:?}: the stored description must be the value the request carried, got {after}"
            );
            // A read does not change what the next caller sees.
            let again = metadata_of(&router, target).await;
            assert_eq!(
                again, after,
                "case {label:?}: reading the repository must not change it, got {again}"
            );

            for member in INVARIANT_MEMBERS {
                assert_eq!(
                    after.get(member),
                    identity.get(member),
                    "case {label:?}: {member} must not change, expected {:?}, got {:?}",
                    identity.get(member),
                    after.get(member)
                );
            }
            let modified = seconds_at(&after, "lastModifiedDate");
            assert!(
                modified > previous_modified,
                "case {label:?}: lastModifiedDate must advance to the instant of the change, expected later than {previous_modified}, got {modified}"
            );
            assert!(
                modified > created,
                "case {label:?}: lastModifiedDate must be later than creationDate {created}, got {modified}"
            );
            previous_modified = modified;
        }
    }

    let untouched_after = metadata_of(&router, "untouched").await;
    assert_eq!(
        untouched_after, untouched_before,
        "updating one repository's description must not touch another, expected {untouched_before}, got {untouched_after}"
    );
}

#[tokio::test]
async fn a_description_update_is_refused_with_the_modeled_error() {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &with_description("guarded", Some("the stored description")),
    )
    .await;

    // Each row is a request outside the operation's contract and the error the model
    // declares for it: a repository that does not exist, a `repositoryName` outside
    // its modelled `length` 1..100 / `pattern ^[\w\.-]+$` or missing altogether, and
    // a `repositoryDescription` one character past its modelled maximum of 1000.
    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "a repository that was never created",
            with_description("absent-repository", Some("anything")),
            "RepositoryDoesNotExistException",
        ),
        (
            "a repository deleted before the update",
            with_description("deleted", Some("anything")),
            "RepositoryDoesNotExistException",
        ),
        (
            "no repositoryName at all",
            json!({ "repositoryDescription": "anything" }),
            "RepositoryNameRequiredException",
        ),
        (
            "a repositoryName outside the modelled pattern",
            with_description("not a valid name!", Some("anything")),
            "InvalidRepositoryNameException",
        ),
        (
            "a repositoryName shorter than the modelled minimum of 1",
            with_description("", Some("anything")),
            "InvalidRepositoryNameException",
        ),
        (
            "a repositoryName longer than the modelled maximum of 100",
            with_description(&"n".repeat(101), Some("anything")),
            "InvalidRepositoryNameException",
        ),
        (
            "a description one character past the modelled maximum of 1000",
            with_description("guarded", Some(&"z".repeat(1001))),
            "InvalidRepositoryDescriptionException",
        ),
    ];

    // The one row that needs a repository to have existed first.
    ok(
        &router,
        "CreateRepository",
        &with_description("deleted", None),
    )
    .await;
    ok(
        &router,
        "DeleteRepository",
        &json!({ "repositoryName": "deleted" }),
    )
    .await;

    for (label, input, expected) in refusals {
        let code = error(&router, "UpdateRepositoryDescription", &input, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code}"
        );
    }

    // A refused update persists nothing: the guarded repository keeps the
    // description it was created with.
    let after = metadata_of(&router, "guarded").await;
    assert_eq!(
        after.get("repositoryDescription").and_then(Value::as_str),
        Some("the stored description"),
        "a refused update must leave the stored description in place, got {after}"
    );
}
