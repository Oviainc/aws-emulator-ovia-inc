// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the PILLARS a workload's `PillarPriorities` may name.
//!
//! The rule under test, over TABLES of inputs rather than the one reported request: an
//! entry of the pillar order a workload is reviewed in is a pillar IDENTIFIER — "Each
//! pillar is represented by its `PillarReviewSummary$PillarId`"
//! (`smithy_query(query="shape", shape="WorkloadPillarPriorities")`), and "a pillar is
//! identified by its `PillarReviewSummary$PillarId`"
//! (`smithy_query(query="shape", shape="PillarId")`) — so it has to RESOLVE to a pillar
//! the workload has: one of the framework's own pillars, or one a lens the workload
//! reviews defines. An entry that resolves to neither is refused with the modelled
//! `ValidationException` (HTTP 400) naming `PillarPriorities`, and the refused request
//! stores nothing, on both paths that write the member — `CreateWorkload` and
//! `UpdateWorkload`, because the rule belongs to the member rather than to one request.
//!
//! What each test adds:
//!
//! 1. Every published pillar id is accepted, read back as sent and replaceable by another
//!    on both write paths; so are the whole set and the whole set reversed.
//! 2. A create naming a pillar the workload has none of is refused and records no
//!    workload, so the name it asked for is still free.
//! 3. The same update is refused and applies NOTHING — neither the order nor the other
//!    members the request carried.
//! 4. A pillar a CUSTOM lens the workload reviews defines is accepted, while the same
//!    identifier is refused on a workload that does not review that lens: the set the rule
//!    judges against is the workload's own pillars, not any string.
//! 5. `PillarId`'s modelled `@length(1, 64)` and this rule are different rules, and each
//!    row names the one that refused it.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, the boundary constraint walk and the handler all run exactly as they
//! do in production. The HTTP client and the workload helpers are the shared
//! `super::workload_harness`; the lens import and publish helpers are the ones
//! `super::custom_lens_import` and `super::custom_lens_versions` already spell.

use serde_json::{json, Value};

use super::custom_lens_import::{import, imported_arn, lens_document, wait_for_the_import_to_land};
use super::custom_lens_versions::create_version;
use super::workload_harness::{
    created_id, minimal_create, stored_workload, stored_workload_count, Answer, Client,
};

/// Every `PillarId` the AWS Well-Architected Framework publishes, in the order the
/// framework presents them.
///
/// SOURCE: the pillar ids the framework publishes, named on the wire as
/// `PillarReviewSummary$PillarId` ("The ID used to identify a pillar, for example,
/// `security`" — `smithy_query(query="shape", shape="PillarId")`). Spelled out here rather
/// than read off the code under test, so the set the emulator holds to is pinned against
/// the framework's own pillars and not against itself.
const FRAMEWORK_PILLARS: &[&str] = &[
    "operationalExcellence",
    "security",
    "reliability",
    "performance",
    "costOptimization",
    "sustainability",
];

/// The published `@length` bounds of `PillarId`
/// (`smithy_query(query="shape", shape="PillarId")`): the only bounds the model itself puts
/// on one entry of the order.
const PILLAR_ID_MIN: usize = 1;
const PILLAR_ID_MAX: usize = 64;

/// The sentence the refusal states for an order naming pillars the workload has none of:
/// the member, and the ids that resolved to no pillar, sorted and deduplicated so the
/// refusal does not depend on the order the request listed them in.
fn expected_sentence(undefined: &[&str]) -> String {
    let mut ids: Vec<&str> = undefined.to_vec();
    ids.sort_unstable();
    ids.dedup();
    format!(
        "Value at PillarPriorities failed to satisfy constraint: every pillar must be a \
         pillar the workload is reviewed in, which {} is not",
        ids.join(", ")
    )
}

/// A `CreateWorkload` request carrying `priorities` as its `PillarPriorities`.
fn create_request(name: &str, priorities: &Value) -> Value {
    let mut request = minimal_create(name, &format!("tok-{name}"));
    request["PillarPriorities"] = priorities.clone();
    request
}

/// The `PillarPriorities` a stored workload reports through `GetWorkload`.
async fn reported_priorities(client: &Client, workload_id: &str, case: &str) -> Value {
    stored_workload(client, workload_id, case)
        .await
        .get("PillarPriorities")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Assert one refused write: the modelled 400 `ValidationException` naming
/// `PillarPriorities`, with the sentence the offending ids state.
fn assert_refused(answer: &Answer, case: &str, undefined: &[&str]) {
    let sentence = expected_sentence(undefined);
    assert_eq!(
        answer.status, 400,
        "case {case:?}: an order naming {undefined:?} must be refused, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Reason").and_then(Value::as_str),
        Some("FIELD_VALIDATION_FAILED"),
        "case {case:?}: the rule is about the member's value, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Fields"),
        Some(&json!([{ "Name": "PillarPriorities", "Message": sentence }])),
        "case {case:?}: the refusal must report the offending member, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Message").and_then(Value::as_str),
        Some(format!("[Validation] {sentence}").as_str()),
        "case {case:?}: Message must state the rule once, got {answer:?}"
    );
}

/// The orders no workload reviewing only the framework lens may be left in, each the kind
/// of value a caller plausibly sends.
///
/// The rows differ along WHAT does not resolve: a word no pillar carries (the instance the
/// report cites), a published id in another case, a near miss of one, a published id with
/// padding, a pillar's NAME rather than its id, an undefined id among published ones, two
/// undefined ids sent twice (so the refusal's sorting and deduplication show), a pillar
/// only a custom lens could define, and an id AT the published length maximum.
fn undefined_orders() -> Vec<(String, Vec<String>, Vec<String>)> {
    vec![
        (
            "a-word-no-pillar-carries",
            vec!["not-a-pillar"],
            vec!["not-a-pillar"],
        ),
        (
            "a-published-id-uppercased",
            vec!["SECURITY"],
            vec!["SECURITY"],
        ),
        (
            "a-published-id-capitalised",
            vec!["Sustainability"],
            vec!["Sustainability"],
        ),
        (
            "a-near-miss-of-a-published-id",
            vec!["costOptimisation"],
            vec!["costOptimisation"],
        ),
        (
            "a-published-id-with-padding",
            vec![" performance"],
            vec![" performance"],
        ),
        (
            "a-pillar-name-rather-than-its-id",
            vec!["Operational Excellence"],
            vec!["Operational Excellence"],
        ),
        (
            "one-undefined-among-published",
            vec!["security", "not-a-pillar", "reliability"],
            vec!["not-a-pillar"],
        ),
        (
            "two-undefined-listed-twice",
            vec!["zz-no-pillar", "not-a-pillar", "zz-no-pillar", "security"],
            vec!["not-a-pillar", "zz-no-pillar"],
        ),
        (
            "a-pillar-only-a-custom-lens-defines",
            vec!["governance"],
            vec!["governance"],
        ),
    ]
    .into_iter()
    .map(|(case, order, undefined)| {
        (
            case.to_string(),
            order.into_iter().map(str::to_string).collect(),
            undefined.into_iter().map(str::to_string).collect(),
        )
    })
    .chain(std::iter::once((
        "at-the-length-maximum-and-undefined".to_string(),
        vec!["p".repeat(PILLAR_ID_MAX)],
        vec!["p".repeat(PILLAR_ID_MAX)],
    )))
    .collect()
}

/// Rule 1. EVERY published pillar id is accepted by both write paths, read back as sent,
/// and replaceable by another published id.
///
/// The rows are every published pillar on its own — including the ones the report never
/// cited — plus the whole set and the whole set reversed, so the rule is exercised over the
/// value, the count and the ordering it generalises along.
#[tokio::test]
async fn every_published_pillar_is_accepted_on_both_write_paths() {
    let reversed: Vec<&str> = FRAMEWORK_PILLARS.iter().rev().copied().collect();
    let mut cases: Vec<(String, Vec<&str>)> = FRAMEWORK_PILLARS
        .iter()
        .enumerate()
        .map(|(index, pillar_id)| (format!("single-{index}-{pillar_id}"), vec![*pillar_id]))
        .collect();
    cases.push(("every-pillar".to_string(), FRAMEWORK_PILLARS.to_vec()));
    cases.push(("every-pillar-reversed".to_string(), reversed));

    let client = Client::new();
    for (case, order) in cases {
        let created = client.create(create_request(&case, &json!(order))).await;
        let workload_id = created_id(&created, &case);
        assert_eq!(
            reported_priorities(&client, &workload_id, &case).await,
            json!(order),
            "case {case:?}: the stored workload must report the order {order:?} as sent"
        );

        // A DIFFERENT published order for every row: the row's own order rotated by one,
        // so the update is a real write rather than a rewrite of the same list.
        let mut rotated = order.clone();
        rotated.rotate_left(1);
        let updated = client
            .update(&workload_id, json!({ "PillarPriorities": rotated }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateWorkload must accept the published order {rotated:?}, got \
             {updated:?}"
        );
        assert_eq!(
            reported_priorities(&client, &workload_id, &case).await,
            json!(rotated),
            "case {case:?}: the stored workload must report the updated order {rotated:?}"
        );
    }
}

/// Rule 2. A `CreateWorkload` whose order names a pillar the workload has none of is
/// refused and records no workload — and the name it asked for is still free afterwards.
#[tokio::test]
async fn a_create_naming_an_undefined_pillar_is_refused_and_stores_nothing() {
    let client = Client::new();

    for (case, order, undefined) in undefined_orders() {
        let before = stored_workload_count(&client);
        let answer = client.create(create_request(&case, &json!(order))).await;

        let undefined: Vec<&str> = undefined.iter().map(String::as_str).collect();
        assert_refused(&answer, &case, &undefined);
        assert_eq!(
            stored_workload_count(&client),
            before,
            "case {case:?}: a refused CreateWorkload must store no workload"
        );

        // The refused create took nothing: the name it asked for is still available, and a
        // create naming a published pillar takes it.
        let accepted = client
            .create(create_request(&case, &json!(["security"])))
            .await;
        let workload_id = created_id(&accepted, &format!("{case}-retried-with-a-published-id"));
        assert_eq!(
            reported_priorities(&client, &workload_id, &case).await,
            json!(["security"]),
            "case {case:?}: the retried create must store the order it named"
        );
    }
}

/// Rule 3. An `UpdateWorkload` whose order names a pillar the workload has none of is
/// refused and applies NOTHING — not the order, and not the other members the same request
/// carried.
#[tokio::test]
async fn an_update_naming_an_undefined_pillar_changes_nothing() {
    let client = Client::new();
    // The workload every row updates starts from an order of its own, so a refused update
    // is observable as the order it did NOT replace.
    let stored_order = json!(["reliability", "security"]);

    for (case, order, undefined) in undefined_orders() {
        let created = client
            .create(create_request(&format!("update-{case}"), &stored_order))
            .await;
        let workload_id = created_id(&created, &case);

        let answer = client
            .update(
                &workload_id,
                json!({
                    "PillarPriorities": order,
                    "Description": "an update that names a pillar the workload has none of",
                }),
            )
            .await;
        let undefined: Vec<&str> = undefined.iter().map(String::as_str).collect();
        assert_refused(&answer, &case, &undefined);

        assert_eq!(
            reported_priorities(&client, &workload_id, &case).await,
            stored_order,
            "case {case:?}: a refused update must leave the stored order alone"
        );
        let stored = stored_workload(&client, &workload_id, &case).await;
        assert_eq!(
            stored.get("Description").and_then(Value::as_str),
            Some("a workload under test"),
            "case {case:?}: a refused update must apply none of its other members, got \
             {stored:?}"
        );
    }
}

/// Rule 4. The pillars the rule judges against are the WORKLOAD's own: a pillar a custom
/// lens the workload reviews defines is accepted on both write paths, while the same
/// identifier is refused on a workload that does not review that lens.
///
/// The rows differ along which of the custom lens's pillars the order names and whether it
/// names a framework pillar beside them, so the union of the two pillar sets is exercised
/// rather than one entry of it.
#[tokio::test]
async fn a_pillar_a_reviewed_custom_lens_defines_is_accepted() {
    let client = Client::new();
    // A custom lens document names its own pillars, so these ids are pillars of THIS lens
    // and of nothing else (`lens_document` builds the document the lens JSON format
    // specification defines).
    let document = lens_document(
        "pillar-identity-lens",
        None,
        &[("governance", 1, 2), ("dataPrivacy", 2, 1)],
    )
    .to_string();
    let imported = import(&client, &document, None, "tok-import-pillar-identity", None).await;
    let lens_arn = imported_arn(&imported, "custom-lens");
    wait_for_the_import_to_land();
    let published = create_version(&client, &lens_arn, "1.0", Some(true), "tok-pub-identity").await;
    assert_eq!(
        published.status, 200,
        "case \"custom-lens\": expected CreateLensVersion 200, got {published:?}"
    );

    let orders: Vec<(&str, Vec<&str>)> = vec![
        ("one-custom-pillar", vec!["governance"]),
        ("the-other-custom-pillar", vec!["dataPrivacy"]),
        ("both-custom-pillars", vec!["dataPrivacy", "governance"]),
        (
            "a-custom-pillar-beside-framework-pillars",
            vec!["governance", "security", "dataPrivacy", "sustainability"],
        ),
    ];

    for (case, order) in orders {
        // The workload REVIEWS the custom lens, so the lens's own pillars are pillars it
        // has.
        let mut request = create_request(&format!("reviews-{case}"), &json!(order));
        request["Lenses"] = json!(["wellarchitected", lens_arn]);
        let created = client.create(request).await;
        let workload_id = created_id(&created, case);
        assert_eq!(
            reported_priorities(&client, &workload_id, case).await,
            json!(order),
            "case {case:?}: a pillar the reviewed lens defines must be stored as sent"
        );
        // The same order on the update path.
        let updated = client
            .update(&workload_id, json!({ "PillarPriorities": order }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateWorkload must accept the order {order:?}, got {updated:?}"
        );

        // A workload that does NOT review that lens has no such pillar, so the identical
        // order is refused: the judgement is of the workload's pillars, not of any string.
        let answer = client
            .create(create_request(
                &format!("reviews-nothing-{case}"),
                &json!(order),
            ))
            .await;
        let undefined: Vec<&str> = order
            .iter()
            .copied()
            .filter(|pillar_id| !FRAMEWORK_PILLARS.contains(pillar_id))
            .collect();
        assert_refused(&answer, case, &undefined);
    }
}

/// Rule 5. `PillarId`'s published `@length(1, 64)` and the pillar-identity rule are
/// DIFFERENT rules, and each row names the one that refused it: inside the bounds the
/// length is satisfied, so the refusal is the identity rule; outside them the boundary's
/// own constraint walk refuses the length before the handler's rule is reached.
#[tokio::test]
async fn the_length_bounds_and_the_pillar_identity_rule_are_refused_as_different_rules() {
    let client = Client::new();
    let at_maximum = "p".repeat(PILLAR_ID_MAX);
    let past_maximum = "p".repeat(PILLAR_ID_MAX + 1);
    let identity_sentence = expected_sentence(&[at_maximum.as_str()]);

    for (case, value, refused_by_the_identity_rule) in [
        ("at-the-length-minimum", "p".repeat(PILLAR_ID_MIN), true),
        ("at-the-length-maximum", at_maximum.clone(), true),
        ("below-the-length-minimum", String::new(), false),
        ("past-the-length-maximum", past_maximum.clone(), false),
    ] {
        let answer = client
            .create(create_request(case, &json!([value.clone()])))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a {}-character pillar id must be refused, got {answer:?}",
            value.len()
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        let message = answer
            .body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if refused_by_the_identity_rule {
            assert!(
                message.contains("every pillar must be a pillar the workload is reviewed in"),
                "case {case:?}: a length-satisfying id must be refused by the identity rule, \
                 got {message:?}"
            );
        } else {
            assert!(
                message.contains("length"),
                "case {case:?}: an out-of-bounds id must be refused by the length rule, got \
                 {message:?}"
            );
            assert!(
                !message.contains(identity_sentence.as_str()),
                "case {case:?}: a length failure must not be reported as the identity rule, \
                 got {message:?}"
            );
        }
        assert_eq!(
            stored_workload_count(&client),
            0,
            "case {case:?}: a refused CreateWorkload must store no workload"
        );
    }
}
