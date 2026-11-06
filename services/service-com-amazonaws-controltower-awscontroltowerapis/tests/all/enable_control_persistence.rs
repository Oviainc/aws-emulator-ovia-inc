// SPDX-License-Identifier: CC0-1.0

//! `EnableControl` as a WRITER, end to end over HTTP.
//!
//! The sibling suite `tests/all/enabled_control_lifecycle.rs` pins what the two
//! enabled-control readers report. These cases pin the rest of what the call persists —
//! the parts a reader other than those two resolves, and the bounds of the store it
//! writes into — so no part of `EnableControl`'s write is unasserted:
//!
//! * the tag map the call carries is persisted onto the resource it mints, so
//!   `ListTagsForResource` reports exactly that map, for any map inside `TagMap`'s
//!   bounds, and the empty map when the member is absent;
//! * every call mints its OWN enabled-control ARN and its own `operationIdentifier`, and
//!   that identifier is the one the readers report as the enablement's last operation,
//!   before and after the clock settles it;
//! * the enabled-control store is BOUNDED, and a call past the bound is refused with the
//!   operation's own modelled quota error and persists nothing.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches
//! with, so what these tests exercise is the wire behaviour a client sees.
//!
//! Every bound written out by hand here names its source. `tags` is a `TagMap`,
//! `@length(0..200)` over a `TagKey` of `@length(1..128)` and a `TagValue` of
//! `@length(0..256)`; `operationIdentifier` is `@length(36..36)`, pattern
//! `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`; and
//! `ServiceQuotaExceededException` is HTTP 402 — from
//! `smithy_query(query="operation", operation="EnableControl,ListTagsForResource")` and
//! `smithy_query(query="shape", shape="TagMap,TagKey,TagValue")` on this crate's
//! `smithy-model/`.

use std::collections::{BTreeMap, BTreeSet};

use crate::enabled_control_lifecycle::{
    control, details, enable, enabled_control_prefix, entries, ou,
};
use crate::landing_zone_lifecycle::{call, is_operation_identifier, past_the_settle_span, set_up_emulator};
use crate::resource_identifier_form::is_resource_id;
use crate::resource_tags::{read_tags, reported_tags, tags};

/// `ListEnabledControls`' entry for `arn`, asserting the listing answered, or
/// `Value::Null` when the listing holds no such entry.
async fn listed_summary(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
) -> serde_json::Value {
    let listed = call(router, "/list-enabled-controls", serde_json::json!({})).await;
    assert_eq!(
        listed.status, 200,
        "list-enabled-controls: expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    entries(listed.get("enabledControls"))
        .into_iter()
        .find(|entry| entry.get("arn").and_then(|value| value.as_str()) == Some(arn))
        .unwrap_or(serde_json::Value::Null)
}

/// RULE: a create persists the tag map it was given onto the resource it mints, so a
/// later read of that resource's tags reports exactly that map — and a resource created
/// without the member, or with an empty map, is an untagged resource rather than one
/// whose tags are unknown.
///
/// The rows differ along what the rule generalises over: whether the member is sent at
/// all, the number of keys (none, one, several, `TagMap`'s maximum count) and the key and
/// value forms at both published length bounds.
#[tokio::test]
async fn enable_control_persists_the_tag_map_it_was_given() {
    // `TagMap`'s `@length` max, which the crate already names for this bound.
    use service_com_amazonaws_controltower_awscontroltowerapis::state::MAX_TAGS_PER_RESOURCE;

    // `TagKey` is @length(1..128) and `TagValue` @length(0..256), so these are the
    // longest key and value a caller may send.
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    // The most entries one tag map may hold.
    let full: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("Key{index:04}"), format!("value-{index}")))
        .collect();

    let cases: Vec<(&str, Option<BTreeMap<String, String>>)> = vec![
        ("no tags member at all", None),
        ("an empty tag map", Some(BTreeMap::new())),
        (
            "one tag at both length minima: a 1-character key and an empty value",
            Some(tags(&[("K", "")])),
        ),
        (
            "one tag at both length maxima",
            Some(tags(&[(longest_key.as_str(), longest_value.as_str())])),
        ),
        (
            "several tags",
            Some(tags(&[
                ("env", "test"),
                ("owner", "platform"),
                ("cost-centre", "42"),
            ])),
        ),
        (
            "a tag map at TagMap's maximum entry count",
            Some(full),
        ),
    ];

    // One emulator for the whole table, so the last loop also proves one resource's tags
    // are its own: a later enablement must not disturb an earlier one's map.
    let router = set_up_emulator().await;
    let mut persisted: Vec<(&str, String, BTreeMap<String, String>)> = Vec::new();

    for (index, (case, tags)) in cases.into_iter().enumerate() {
        let mut body = serde_json::json!({
            "controlIdentifier": control("AWS-GR_ENCRYPTED_VOLUMES"),
            "targetIdentifier": ou(&format!("7ab{index:01}")),
        });
        if let Some(tags) = &tags {
            body["tags"] = serde_json::json!(tags);
        }
        let (arn, _) = enable(&router, body).await;
        let expected = tags.unwrap_or_default();

        let read = read_tags(&router, &arn).await;
        assert_eq!(
            (read.status, reported_tags(&read)),
            (200, expected.clone()),
            "case {case:?}: expected 200 and the tag map the enablement carried, got {} \
             and {:?}",
            read.status,
            reported_tags(&read)
        );

        // The enablement itself is persisted alongside its tags, so the resource the tag
        // map hangs off resolves through its own reader too.
        let detail = details(&router, &arn).await;
        assert_eq!(
            detail.get("arn").and_then(|value| value.as_str()),
            Some(arn.as_str()),
            "case {case:?}: expected the stored enablement, got {detail:?}"
        );

        persisted.push((case, arn, expected));
    }

    for (case, arn, expected) in &persisted {
        let read = read_tags(&router, arn).await;
        assert_eq!(
            &reported_tags(&read),
            expected,
            "case {case:?}: the later enablements must not change this resource's tags, \
             got {:?}",
            reported_tags(&read)
        );
    }
}

/// RULE: each enablement is its own resource with its own asynchronous operation, so
/// every call mints an identifier pair no other call returns, and the identifier it
/// returns is the one both readers report as that enablement's last operation — at the
/// non-terminal status and after the clock settles it.
///
/// The rows span the ways two requests can differ: the same control on two targets,
/// another control on a target already carrying one, and a control the catalog publishes
/// with parameters beside ones that declare none.
#[tokio::test]
async fn every_enablement_mints_its_own_arn_and_operation_identifier() {
    let encrypted = control("AWS-GR_ENCRYPTED_VOLUMES");
    let root_user = control("AWS-GR_RESTRICT_ROOT_USER");
    let first_ou = ou("4aa1");
    let second_ou = ou("4aa2");
    let third_ou = ou("4aa3");

    let cases: Vec<(&str, &String, &String)> = vec![
        ("one control on an OU target", &encrypted, &first_ou),
        ("the same control on another OU", &encrypted, &second_ou),
        ("the same control on a third OU", &encrypted, &third_ou),
        ("another control on a target already carrying one", &root_user, &first_ou),
        ("another control on the third OU", &root_user, &third_ou),
    ];

    let router = set_up_emulator().await;
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut operations: BTreeSet<String> = BTreeSet::new();
    let mut enabled: Vec<(&str, String, String)> = Vec::new();

    for (case, control_identifier, target_identifier) in cases {
        let (arn, operation) = enable(
            &router,
            serde_json::json!({
                "controlIdentifier": control_identifier,
                "targetIdentifier": target_identifier,
            }),
        )
        .await;

        let prefix = enabled_control_prefix();
        let resource_id = arn.strip_prefix(&prefix).unwrap_or_default();
        assert!(
            // The resource segment carries this service's resource-id form — 16
            // characters of `[A-Z0-9]`, pinned in
            // `tests/all/resource_identifier_form.rs` — not an operation identifier's.
            is_resource_id(resource_id),
            "case {case:?}: expected an ARN {prefix:?} plus a 16-character [A-Z0-9] \
             resource id, got {arn:?}"
        );
        assert!(
            is_operation_identifier(&operation),
            "case {case:?}: expected a 36-character lowercase-hex operationIdentifier, \
             got {operation:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: ARN {arn:?} was already minted by an earlier enablement"
        );
        assert!(
            operations.insert(operation.clone()),
            "case {case:?}: operationIdentifier {operation:?} was already returned by an \
             earlier enablement"
        );

        // The non-terminal phase is asserted HERE, on the enablement this iteration just
        // started, rather than after the whole table: the settle span is short on
        // purpose, so a case that enabled every row first could race the clock.
        let deploying = serde_json::json!({
            "status": "UNDER_CHANGE",
            "lastOperationIdentifier": operation,
        });
        let detail = details(&router, &arn).await;
        assert_eq!(
            detail.get("statusSummary").cloned().unwrap_or_default(),
            deploying,
            "case {case:?} while deploying: get expected {deploying:?}, got {:?}",
            detail.get("statusSummary")
        );
        let summary = listed_summary(&router, &arn).await;
        assert_eq!(
            summary.get("statusSummary").cloned().unwrap_or_default(),
            deploying,
            "case {case:?} while deploying: the listing expected {deploying:?}, got \
             {summary:?}"
        );

        enabled.push((case, arn, operation));
    }

    // Once the clock has settled every enablement: the operation identifier a call
    // returned keeps naming that enablement's last operation, and the two readers still
    // agree because they read one record.
    tokio::time::sleep(past_the_settle_span()).await;

    for (case, arn, operation) in &enabled {
        let settled = serde_json::json!({
            "status": "SUCCEEDED",
            "lastOperationIdentifier": operation,
        });

        let detail = details(&router, arn).await;
        assert_eq!(
            detail.get("statusSummary").cloned().unwrap_or_default(),
            settled,
            "case {case:?} once settled: get expected {settled:?}, got {:?}",
            detail.get("statusSummary")
        );

        let summary = listed_summary(&router, arn).await;
        assert_eq!(
            summary.get("statusSummary").cloned().unwrap_or_default(),
            settled,
            "case {case:?} once settled: the listing expected {settled:?}, got {summary:?}"
        );
    }
}

/// RULE: the enabled-control store is BOUNDED — the enablements an account may hold at
/// once is capped, and a call that would grow the store past the cap is refused with the
/// operation's own modelled quota error (HTTP 402) and persists nothing, while every
/// enablement already stored keeps resolving.
///
/// Reaching the cap is the only way to observe it, so this case issues
/// `MAX_ENABLED_CONTROLS` enablements and is the longest-running one in this suite.
#[tokio::test]
async fn the_enabled_control_store_is_bounded_by_the_modeled_quota() {
    use service_com_amazonaws_controltower_awscontroltowerapis::state::MAX_ENABLED_CONTROLS;

    let router = set_up_emulator().await;
    let encrypted = control("AWS-GR_ENCRYPTED_VOLUMES");

    // Fill the store to its cap. Each control/target pair is one entry, so one control on
    // distinct organizational units reaches the cap.
    let mut last_stored = String::new();
    for index in 0..MAX_ENABLED_CONTROLS {
        let (arn, _) = enable(
            &router,
            serde_json::json!({
                "controlIdentifier": encrypted,
                "targetIdentifier": ou(&format!("q{index:04}")),
            }),
        )
        .await;
        last_stored = arn;
    }

    // Past the cap, on a pair nothing has enabled: the refusal is the quota, not the
    // once-per-target rule.
    let past_the_cap = ou("qpast");
    let refused = call(
        &router,
        "/enable-control",
        serde_json::json!({
            "controlIdentifier": encrypted,
            "targetIdentifier": past_the_cap,
        }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (402, Some("ServiceQuotaExceededException")),
        "one enablement past the cap of {MAX_ENABLED_CONTROLS}: expected 402 \
         ServiceQuotaExceededException, got {:?} body {:?}",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );

    // The refusal wrote nothing: the target it named carries no enablement.
    let listed = call(
        &router,
        "/list-enabled-controls",
        serde_json::json!({ "targetIdentifier": past_the_cap }),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "after the refusal: list expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        entries(listed.get("enabledControls")),
        Vec::<serde_json::Value>::new(),
        "a refused enablement must persist nothing, got {:?}",
        listed.body
    );

    // And the enablements inside the cap are untouched by the refusal.
    let detail = details(&router, &last_stored).await;
    assert_eq!(
        detail.get("arn").and_then(|value| value.as_str()),
        Some(last_stored.as_str()),
        "after the refusal: expected the last stored enablement, got {detail:?}"
    );
}
