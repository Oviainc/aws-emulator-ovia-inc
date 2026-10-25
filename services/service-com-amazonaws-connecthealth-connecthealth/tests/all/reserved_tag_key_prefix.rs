// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, over EVERY operation that accepts a `TagMap`.
//!
//! The RULE under test, asserted over a table rather than one recorded request: a tag KEY
//! that begins with the namespace the service reserves for the tags it sets itself
//! (`state::RESERVED_TAG_KEY_PREFIX`, matched without regard to ASCII case) is one no
//! caller may set, so any request whose `tags` map carries such a key is refused with HTTP
//! 400 `ValidationException` naming the key, and NOTHING the request carried is persisted
//! — no domain is minted by `CreateDomain`, no entry is merged by `TagResource`. The rule
//! is on the start of the KEY only: a key that carries the prefix anywhere else, a key
//! that merely resembles it, and a VALUE that spells it are all ordinary input.
//!
//! The rule belongs to the `TagMap` shape rather than to one operation, so every operation
//! that accepts one enforces it identically, and both readers of a resource's tag set
//! (`GetDomain` and `ListTagsForResource`) report the same set afterwards.
//!
//! The rows differ along what the rule generalises over: the CASE the prefix is spelled in,
//! what follows the prefix (nothing, one segment, several segments, a key at the modelled
//! maximum length), WHERE in the key the prefix sits (start, middle, end), how a key
//! resembles the prefix without being in the namespace (no colon, a different separator, a
//! leading space), whether the prefix is in the key or in the VALUE, and how many entries
//! one map carries (so a legal entry beside a reserved one is not persisted either).
//!
//! Sources for every value written by hand here:
//!
//! * `smithy_query(query="shape",shape="TagMap,TagKey,TagValue",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `TagMap`'s key targets `TagKey` (`@length` 1..128, and NO `@pattern`) and its value
//!   targets `TagValue` (`@length` 0..256), which is why the namespace rule is the
//!   service's own and is applied by the handlers rather than by the request boundary. The
//!   prefix and the lengths are stated once in the crate
//!   (`state::RESERVED_TAG_KEY_PREFIX`, `state::MAX_TAG_KEY_LEN`,
//!   `state::MAX_TAG_VALUE_LEN`), which is what this suite imports rather than restating.
//! * `smithy_query(query="digest")` — the operations whose input carries a `TagMap` are
//!   `CreateDomain` (`POST /domain`, 201) and `TagResource` (`POST /tags/{resourceArn}`,
//!   204); `UntagResource` carries a `TagKeyList` of keys to REMOVE and is still a 501
//!   stub, so it has no row here.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::service::tag_bounds::violation as tag_entry_violation;
use service_com_amazonaws_connecthealth_connecthealth::state::{
    MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN, RESERVED_TAG_KEY_PREFIX,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::list_tags_for_resource::list_tags;
use crate::resource_tagging::{domain_tags, entries, tags_path};

/// The ids `ListDomains` reports, so a refused `CreateDomain` can be shown to have
/// persisted nothing at all.
async fn listed_domain_ids(emulator: &Emulator) -> Vec<String> {
    let (status, listed) = emulator.send("GET", "/domain", None).await;
    assert_eq!(status, 200, "ListDomains got {status} {listed}");
    listed
        .get("domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListDomains must report a domains list, got {listed}"))
        .iter()
        .map(|summary| string_at(summary, "domainId"))
        .collect()
}

/// A refusal of a reserved key: 400, the service's `ValidationException`, and a message
/// that names the key the caller asked for and the prefix no caller may use. The sentence
/// itself is not the requirement — the status, the code, and the fact that it is built
/// from the request are.
fn assert_refused(case: &str, reserved_key: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: got {body}"
    );
    let message = body
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a refusal must carry a Message, got {body}"))
        .to_string();
    assert!(
        message.contains(reserved_key) && message.contains(RESERVED_TAG_KEY_PREFIX),
        "case {case:?}: the refusal must name the offending key {reserved_key:?} and the \
         reserved prefix, got {message:?}"
    );
    message
}

/// Every row of the table: a label, the `tags` map one call carries, whether the rule
/// accepts it, and (when it does not) the key that makes it reserved.
fn rows() -> Vec<(String, Value, bool, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    // A reserved key at the modelled maximum key length: the namespace rule is
    // independent of the length bound, so a key inside the length is still refused.
    let longest_reserved_key = format!(
        "{prefix}{}",
        "k".repeat(MAX_TAG_KEY_LEN - prefix.chars().count())
    );
    // A value at the modelled maximum length, to show the rule never looks at the value.
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);

    let mut rows: Vec<(String, Value, bool, String)> = Vec::new();

    // --- Refused: the reserved namespace, in each case the caller might spell it in and
    // with each shape of key that can follow it.
    for spelling in ["aws:", "AWS:", "Aws:", "aWs:", "awS:"] {
        for (shape, suffix) in [
            ("nothing after the prefix", ""),
            ("one segment", "owner"),
            ("several segments", "cloudformation:stack-name"),
            ("a segment that is only a digit", "1"),
            ("a non-ASCII segment", "propriétaire"),
        ] {
            let key = format!("{spelling}{suffix}");
            rows.push((
                format!("the reserved prefix spelled {spelling:?} with {shape}"),
                json!({ key.clone(): "mine" }),
                false,
                key,
            ));
        }
    }
    rows.push((
        "a reserved key at the modelled maximum key length".to_string(),
        json!({ longest_reserved_key.clone(): "mine" }),
        false,
        longest_reserved_key,
    ));
    rows.push((
        "a reserved key carrying a value at the modelled maximum value length".to_string(),
        json!({ format!("{prefix}owner"): longest_value.clone() }),
        false,
        format!("{prefix}owner"),
    ));
    rows.push((
        "a reserved key with an empty value".to_string(),
        json!({ format!("{prefix}owner"): "" }),
        false,
        format!("{prefix}owner"),
    ));
    rows.push((
        "a legal entry beside a reserved one".to_string(),
        json!({ "env": "prod", format!("{prefix}owner"): "mine" }),
        false,
        format!("{prefix}owner"),
    ));
    rows.push((
        "several legal entries beside one reserved key".to_string(),
        json!({
            "env": "prod",
            "team": "cardiology",
            format!("{prefix}cloudformation:stack-name"): "mine",
        }),
        false,
        format!("{prefix}cloudformation:stack-name"),
    ));

    // --- Accepted: a key that resembles the namespace without being in it, and a value
    // that spells it (the rule is on the start of the KEY).
    for (shape, key) in [
        ("the prefix without its colon", "aws".to_string()),
        ("the prefix's letters inside a word", "awsome".to_string()),
        (
            "a different separator after the prefix's letters",
            "aws-region".to_string(),
        ),
        (
            "the prefix in the middle of the key",
            format!("team{prefix}owner"),
        ),
        ("the prefix at the end of the key", format!("team{prefix}")),
        (
            "the prefix after a leading space",
            format!(" {prefix}owner"),
        ),
        ("a key one letter longer than the prefix's letters", "awsx:owner".to_string()),
        ("an ordinary key", "env".to_string()),
    ] {
        rows.push((
            format!("{shape} ({key:?})"),
            json!({ key.clone(): "mine" }),
            true,
            String::new(),
        ));
    }
    rows.push((
        "a value that spells the reserved prefix".to_string(),
        json!({ "stack": format!("{prefix}cloudformation") }),
        true,
        String::new(),
    ));
    rows.push((
        "several ordinary entries".to_string(),
        json!({ "env": "prod", "team": "cardiology", "awsome": "yes" }),
        true,
        String::new(),
    ));

    rows
}

#[test]
fn the_shared_tag_entry_check_refuses_exactly_the_keys_in_the_reserved_namespace() {
    // The one check both handlers run, over the same table the end-to-end suite below
    // drives through HTTP — so the rule is pinned at the seam the two operations share as
    // well as on the wire.
    for (case, tags, accepted, _) in rows() {
        let pairs: Vec<(String, String)> = entries(&tags);
        let borrowed: Vec<(&String, &String)> =
            pairs.iter().map(|(key, value)| (key, value)).collect();
        let violation = tag_entry_violation(borrowed);
        assert_eq!(
            violation.is_none(),
            accepted,
            "case {case:?}: expected accepted={accepted} for {tags}, got {violation:?}"
        );
    }
}

#[tokio::test]
async fn every_operation_that_accepts_a_tag_map_refuses_a_reserved_key_and_writes_nothing() {
    for (case, tags, accepted, reserved_key) in rows() {
        // A fresh emulator per row: one row's domains and tag sets can then never be what
        // a later row reads, and no row is near the account-wide domain cap.
        let emulator = Emulator::new("us-east-1");
        let expected: BTreeMap<String, String> = entries(&tags).into_iter().collect();

        // --- CreateDomain carries the map on the create itself.
        let before = listed_domain_ids(&emulator).await;
        let (status, created) = emulator
            .send(
                "POST",
                "/domain",
                Some(&json!({ "name": "clinic", "tags": tags })),
            )
            .await;
        let create_refusal = if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: CreateDomain must accept {tags}, got {status} with {created}"
            );
            let domain_id = string_at(&created, "domainId");
            let arn = string_at(&created, "arn");
            // Both readers of a domain's tags report exactly what the create stored.
            let reported = domain_tags(&emulator, &domain_id).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: GetDomain must report the stored set, got {reported:?}"
            );
            let listed = list_tags(&emulator, &case, &arn).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: ListTagsForResource must report the same set, got {listed:?}"
            );
            None
        } else {
            let message = assert_refused(
                &format!("{case} (CreateDomain)"),
                &reserved_key,
                status,
                &created,
            );
            // Nothing the refused create carried reached the store: no domain was minted,
            // so no reader can report the entry it carried.
            let after = listed_domain_ids(&emulator).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused CreateDomain must persist no domain, got {after:?}"
            );
            Some(message)
        };

        // --- TagResource carries the same map against a domain that already exists.
        let created = emulator
            .create_domain(&json!({ "name": "clinic-tagged" }))
            .await;
        let domain_id = string_at(&created, "domainId");
        let arn = string_at(&created, "arn");
        let (status, body) = emulator
            .send("POST", &tags_path(&arn), Some(&json!({ "tags": tags })))
            .await;
        if accepted {
            assert_eq!(
                status, 204,
                "case {case:?}: TagResource must accept {tags}, got {status} with {body}"
            );
            let reported = domain_tags(&emulator, &domain_id).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: the merged set must be exactly the accepted entries, \
                 got {reported:?}"
            );
        } else {
            let message = assert_refused(
                &format!("{case} (TagResource)"),
                &reserved_key,
                status,
                &body,
            );
            let reported = domain_tags(&emulator, &domain_id).await;
            assert!(
                reported.is_empty(),
                "case {case:?}: a refused TagResource must write nothing, got {reported:?}"
            );
            let listed = list_tags(&emulator, &case, &arn).await;
            assert!(
                listed.is_empty(),
                "case {case:?}: no reader may report a refused entry, got {listed:?}"
            );
            // The rule belongs to the shape, so the two operations cannot word the same
            // refusal differently.
            assert_eq!(
                create_refusal.as_deref(),
                Some(message.as_str()),
                "case {case:?}: both operations must refuse the entry identically"
            );
        }
    }
}

#[tokio::test]
async fn a_reserved_key_cannot_overwrite_or_join_a_tag_set_that_already_exists() {
    let emulator = Emulator::new("eu-central-1");
    // The domain starts with a tag set of its own, and with a key whose NAME carries the
    // reserved prefix somewhere other than its start — which is ordinary input.
    let ordinary = format!("team{RESERVED_TAG_KEY_PREFIX}owner");
    let created = emulator
        .create_domain(&json!({
            "name": "clinic",
            "tags": { "env": "prod", ordinary.clone(): "cardiology" },
        }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let domain_arn = string_at(&created, "arn");
    let (status, subscription) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(status, 201, "CreateSubscription got {subscription}");
    let subscription_arn = string_at(&subscription, "arn");

    let established: BTreeMap<String, String> = [
        ("env".to_string(), "prod".to_string()),
        (ordinary.clone(), "cardiology".to_string()),
    ]
    .into_iter()
    .collect();
    let reported = domain_tags(&emulator, &domain_id).await;
    assert_eq!(
        reported, established,
        "the create must have stored the ordinary entries, got {reported:?}"
    );

    // A reserved key is refused on either resource kind, and a refused merge leaves the
    // set it would have joined exactly as it was — including when the request also
    // carries an overwrite of a key the resource already holds.
    for (case, arn) in [
        ("the domain's ARN", &domain_arn),
        ("the subscription's ARN", &subscription_arn),
    ] {
        for (shape, tags, reserved_key) in [
            (
                "a reserved key on its own",
                json!({ format!("{RESERVED_TAG_KEY_PREFIX}owner"): "mine" }),
                format!("{RESERVED_TAG_KEY_PREFIX}owner"),
            ),
            (
                "a reserved key beside an overwrite of an existing key",
                json!({ "env": "staging", format!("{RESERVED_TAG_KEY_PREFIX}owner"): "mine" }),
                format!("{RESERVED_TAG_KEY_PREFIX}owner"),
            ),
        ] {
            let label = format!("{shape} against {case}");
            let (status, body) = emulator
                .send("POST", &tags_path(arn), Some(&json!({ "tags": tags })))
                .await;
            assert_refused(&label, &reserved_key, status, &body);
            let listed = list_tags(&emulator, &label, arn).await;
            let expected = if arn == &domain_arn {
                established.clone()
            } else {
                BTreeMap::new()
            };
            assert_eq!(
                listed, expected,
                "case {label:?}: the refused merge must change nothing, got {listed:?}"
            );
        }
    }

    // And the set is still readable and still mergeable afterwards: a refusal left no
    // half-applied state behind.
    let (status, body) = emulator
        .send(
            "POST",
            &tags_path(&domain_arn),
            Some(&json!({ "tags": { "tier": "gold" } })),
        )
        .await;
    assert_eq!(status, 204, "an ordinary merge after a refusal got {body}");
    let mut expected = established.clone();
    expected.insert("tier".to_string(), "gold".to_string());
    let reported = domain_tags(&emulator, &domain_id).await;
    assert_eq!(
        reported, expected,
        "the ordinary merge must apply to the untouched set, got {reported:?}"
    );
}
