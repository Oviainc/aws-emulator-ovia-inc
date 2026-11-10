// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the uniqueness of a `TemplateName` within the account.
//!
//! The rules pinned here, each over a TABLE of spellings rather than one recorded
//! request:
//!
//! 1. A review template's name identifies it within the account, so `CreateReviewTemplate`
//!    naming one a stored template already holds is a modelled `409 ConflictException`
//!    (`smithy_query(query="operation", operation="CreateReviewTemplate")` lists it with
//!    `httpError 409`, carrying `Message`, `ResourceId` and `ResourceType`) instead of a
//!    second template under the same name. The comparison is the one this service applies
//!    to every name it meters for uniqueness — "spaces and capitalization are ignored when
//!    checking for uniqueness" (`smithy_query(query="shape", shape="WorkloadName")`) — so a
//!    spelling that differs from a stored template's only in either collides with it, and a
//!    refused create stores nothing.
//! 2. `UpdateReviewTemplate` renaming a template to a name ANOTHER template holds is
//!    refused the same way; renaming a template to the name it already holds (in any
//!    spelling the comparison treats as that name) is not a collision with itself.
//! 3. The name is held by the TEMPLATE, not reserved forever: once the template holding it
//!    is deleted, a create under that name is accepted again.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{created_arn, member, minimal_template, Answer, Client};

/// `TemplateName` is `@length(min: 3, max: 100)` (`smithy_query(query="shape",
/// shape="TemplateName")`), so every spelling a row submits stays inside that bound.
const TEMPLATE_NAME_RANGE: (usize, usize) = (3, 100);

/// The stored spellings the tables below are run for: a plain name, one carrying inner
/// spaces and one mixing capitalization, so the rows differ in what the loose comparison
/// has to drop. Every one satisfies `TemplateName`'s pattern
/// `^[A-Za-z0-9-_.,:/()@!&?#+'’\s]+$`.
const STORED_NAMES: [&str; 3] = ["tpl-plain", "tpl with inner spaces", "MiXeD Case Tpl"];

/// How a duplicate respells a stored name: the label and the transformation. Each row is a
/// spelling the service's comparison must treat as the SAME name.
fn collisions(stored_name: &str) -> Vec<(&'static str, String)> {
    vec![
        ("identical", stored_name.to_string()),
        ("upper", stored_name.to_uppercase()),
        ("lower", stored_name.to_lowercase()),
        ("padded", format!("  {stored_name} ")),
        ("whitespace-removed", stored_name.replace(' ', "")),
        ("tab-separated", stored_name.replace(' ', "\t")),
    ]
}

async fn list_templates(client: &Client) -> Vec<Value> {
    let answer = client.list_review_templates("").await;
    assert_eq!(
        answer.status, 200,
        "expected ListReviewTemplates 200, got {answer:?}"
    );
    answer
        .body
        .get("ReviewTemplates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The refusal a duplicate name gets: 409 `ConflictException`, `ResourceType`
/// `REVIEW_TEMPLATE`, the `ResourceId` of the template already holding the name, and the
/// fixed message quoting the name AS SUBMITTED by the refused request.
fn assert_name_conflict(case: &str, answer: &Answer, submitted_name: &str, stored_arn: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected HTTP 409, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ConflictException",
        "case {case:?}: expected ConflictException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceType"),
        "REVIEW_TEMPLATE",
        "case {case:?}: expected ResourceType REVIEW_TEMPLATE, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceId"),
        stored_arn,
        "case {case:?}: ResourceId must name the template holding the name, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        format!("[Conflict] A review template with name {submitted_name} already exists."),
        "case {case:?}: the refusal must quote the submitted name, got {answer:?}"
    );
}

/// Rule 1. The rows differ along the stored spelling and, for each, along how the
/// duplicate respells it; every refused create leaves the store with the one template.
#[tokio::test]
async fn a_review_template_name_is_unique_in_the_account_ignoring_case_and_whitespace() {
    for (index, stored_name) in STORED_NAMES.iter().enumerate() {
        // One client per row, so the names of one row cannot occupy another's account.
        let client = Client::new();
        let stored_arn = created_arn(
            &client
                .create_review_template(minimal_template(stored_name, &format!("tok-{index}")))
                .await,
            stored_name,
        );

        for (spelling, submitted) in collisions(stored_name) {
            let case = format!("{stored_name:?}/{spelling}");
            assert!(
                (TEMPLATE_NAME_RANGE.0..=TEMPLATE_NAME_RANGE.1).contains(&submitted.len()),
                "case {case:?}: the row must stay inside \
                 TemplateName @length{TEMPLATE_NAME_RANGE:?}, got {submitted:?}"
            );
            // A FRESH token every time: the refusal is about the name, not about a
            // replayed idempotency token.
            let refused = client
                .create_review_template(minimal_template(
                    &submitted,
                    &format!("tok-dup-{index}-{spelling}"),
                ))
                .await;
            assert_name_conflict(&case, &refused, &submitted, &stored_arn);
        }

        // A genuinely different name is still accepted, so the rows above were refused on
        // the name they collided with and not on a store that stopped creating.
        let accepted = client
            .create_review_template(minimal_template(
                &format!("{stored_name}-second"),
                &format!("tok-second-{index}"),
            ))
            .await;
        let second_arn = created_arn(&accepted, "a-free-name");
        assert_ne!(
            second_arn, stored_arn,
            "case {stored_name:?}: a free name must mint a distinct template"
        );

        let listed = list_templates(&client).await;
        assert_eq!(
            listed.len(),
            2,
            "case {stored_name:?}: only the two accepted templates must be stored, \
             got {listed:?}"
        );
    }
}

/// Rule 2. The rows differ along the spelling the rename submits: one that collides with
/// ANOTHER template is refused, and one that resolves to the renamed template's OWN name
/// is accepted and leaves the name as sent.
#[tokio::test]
async fn a_rename_may_not_take_a_name_another_review_template_holds() {
    for (index, stored_name) in STORED_NAMES.iter().enumerate() {
        let client = Client::new();
        let stored_arn = created_arn(
            &client
                .create_review_template(minimal_template(stored_name, &format!("tok-held-{index}")))
                .await,
            stored_name,
        );
        let other_name = format!("{stored_name}-other");
        let other_arn = created_arn(
            &client
                .create_review_template(minimal_template(
                    &other_name,
                    &format!("tok-other-{index}"),
                ))
                .await,
            &other_name,
        );

        for (spelling, submitted) in collisions(stored_name) {
            let case = format!("{stored_name:?}/rename-to-{spelling}");
            let refused = client
                .update_review_template(&other_arn, json!({ "TemplateName": submitted }))
                .await;
            assert_name_conflict(&case, &refused, &submitted, &stored_arn);
        }

        // Renaming a template to its OWN name, in any spelling the comparison treats as
        // that name, collides with nothing: the write lands and the template reports the
        // name AS SENT.
        for (spelling, submitted) in collisions(&other_name) {
            let case = format!("{other_name:?}/self-rename-{spelling}");
            let answer = client
                .update_review_template(&other_arn, json!({ "TemplateName": submitted.clone() }))
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: renaming a template to its own name must be accepted, \
                 got {answer:?}"
            );
            let fetched = client.get_review_template(&other_arn).await;
            assert_eq!(
                fetched
                    .body
                    .get("ReviewTemplate")
                    .and_then(|template| template.get("TemplateName"))
                    .and_then(Value::as_str),
                Some(submitted.as_str()),
                "case {case:?}: the stored name must be the spelling as sent, got {fetched:?}"
            );
        }
    }
}

/// Rule 3. The name belongs to the template holding it: once that template is deleted the
/// name is free, in every spelling the comparison treats as it. The rows differ along the
/// spelling the second create submits.
#[tokio::test]
async fn deleting_a_review_template_frees_its_name() {
    for (index, (spelling, submitted)) in collisions(STORED_NAMES[1]).into_iter().enumerate() {
        let client = Client::new();
        let stored_name = STORED_NAMES[1];
        let stored_arn = created_arn(
            &client
                .create_review_template(minimal_template(stored_name, &format!("tok-del-{index}")))
                .await,
            stored_name,
        );

        let deleted = client
            .delete_review_template(&stored_arn, &format!("tok-delete-{index}"))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {spelling:?}: DeleteReviewTemplate must succeed, got {deleted:?}"
        );

        let recreated = client
            .create_review_template(minimal_template(
                &submitted,
                &format!("tok-recreate-{index}"),
            ))
            .await;
        let recreated_arn = created_arn(&recreated, spelling);
        assert_ne!(
            recreated_arn, stored_arn,
            "case {spelling:?}: the re-created template must be a new template"
        );
    }
}
