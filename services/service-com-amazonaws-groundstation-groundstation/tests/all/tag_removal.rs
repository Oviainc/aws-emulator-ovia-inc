// SPDX-License-Identifier: CC0-1.0

//! Ground Station TAG REMOVAL, end to end over HTTP: what deassigning tags does to the
//! resource an ARN names, and what a tag read reports afterwards.
//!
//! The RULE under test: `UntagResource` DROPS exactly the keys its `tagKeys` list names
//! from the tag map of the resource `resourceArn` resolves to, leaving every key the list
//! does not name with the value it had — a key the resource does not carry is not a
//! failure (the operation is `@idempotent`, so naming a key twice, or removing it twice,
//! leaves the same map as removing it once), and the map that is left is the ONE map
//! `ListTagsForResource` and the resource's own read report. An ARN that resolves to no
//! resource of this account is refused with the tagging plane's own not-found refusal — the
//! code `NotFoundException`, HTTP 434 and one fixed sentence
//! ([`crate::tagging_not_found_refusal`] asserts it across the whole plane) — a list
//! naming a key AWS's tagging rules do not let a caller hold is refused with
//! `InvalidParameterException` (HTTP 431) naming the `tagKeys` member, and neither
//! refusal removes anything.
//!
//! Each suite asserts that rule over a TABLE rather than one recorded call: every taggable
//! resource this service hands out (a config of each of the eight capability types, a
//! mission profile, a dataflow endpoint group, a contact, an ephemeris and an agent),
//! crossed with removals differing along what the rule generalises over — how many keys a
//! call names, whether the resource holds them, whether a key is repeated, what the key
//! spells, and down to the empty map. The key rules are asserted on each published bound
//! and one past it, in characters rather than bytes, and on both spellings of the reserved
//! namespace.
//!
//! The taggable-resource table, the tag plumbing and the response reader are
//! [`crate::resource_tagging`]'s and [`crate::scheduling_loop`]'s, reused rather than
//! rebuilt.

use std::collections::BTreeMap;

use serde_json::json;
use service_com_amazonaws_groundstation_groundstation::state::{
    RESERVED_TAG_KEY_PREFIX, TAG_KEYS_PARAMETER, TAG_KEY_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tagging::{
    agent_arn, contact_arn, fresh_config, label, list_tags, reported_tags, tag_resource,
    taggable_resources, tags_of, Taggable,
};
use crate::scheduling_loop::{
    create_config, harness, harness_serving, send, Reply, ABSENT_CONFIG_ARN,
    ABSENT_MISSION_PROFILE_ARN, ABSENT_UUID, REGION, STATION,
};
use crate::tag_mutation_no_content::TAG_MUTATION_SUCCESS_STATUS;

/// The longest `tagKeys` list the model admits (`smithy_query(query="shape",
/// shape="TagKeys")` — a list of `length` 0..500), and so the bound this suite asserts at
/// and one past.
const MAX_TAG_KEYS: usize = 500;

/// A multi-byte character, for the rows that assert a key's length bound is counted in
/// CHARACTERS and not in the bytes a UTF-8 encoding takes.
const WIDE_CHAR: char = 'é';

/// `value` as a query-string value: every byte outside the unreserved set is
/// percent-encoded, which is what an SDK sends and what the server decodes — a bare `+`
/// in a query string would arrive as a space.
///
/// `pub(crate)` so the suite that asserts the STATUS of a removal
/// ([`crate::tag_mutation_no_content`]) sends the same encoding through the same front
/// door rather than carrying a second copy of it.
pub(crate) fn encode_query_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `DELETE /tags/{resourceArn}?tagKeys=..&tagKeys=..` — `UntagResource`.
///
/// `tagKeys` is bound to `@httpQuery("tagKeys")` (`smithy_query(query="operation",
/// operation="UntagResource")`), so every key travels as its own query pair.
/// `DELETE /tags/{resourceArn}?tagKeys=..&tagKeys=..` — `UntagResource`.
///
/// `pub(crate)` so a suite asserting the not-found refusal of the tagging plane across all
/// three of its operations ([`crate::tagging_not_found_refusal`]) reaches the removal
/// through this one helper rather than building a second copy of it.
pub(crate) async fn untag_resource(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> Reply {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_query_value(key)))
        .collect();
    let path = if query.is_empty() {
        format!("/tags/{}", label(arn))
    } else {
        format!("/tags/{}?{}", label(arn), query.join("&"))
    };
    send(router, "DELETE", &path, None).await
}

/// `str` repeated `chars` times.
fn sized(fill: char, chars: usize) -> String {
    std::iter::repeat_n(fill, chars).collect()
}

/// A key spelled over the characters AWS documents for a tag — letters, digits, spaces and
/// `+ - = . _ : / @` — so a removal is asserted to address a key by its exact spelling
/// however the query string had to encode it.
const WIDE_KEY: &str = "cost center:dept/1 +=._-@";

/// The map every resource is seeded with before the removals below, so each row starts
/// from a resource that genuinely holds the keys it is asked to drop.
fn seeded() -> BTreeMap<String, String> {
    let mut tags = tags_of(&[
        ("mission", "aqua"),
        ("stage", "flight"),
        ("owner", "ops"),
        ("note", ""),
        (WIDE_KEY, "a b+c=d.e_f:g/h@i"),
    ]);
    for index in 0..4 {
        tags.insert(format!("bulk-{index:02}"), format!("value-{index:02}"));
    }
    tags
}

/// One `UntagResource` call of the removal table: the keys it names, and whether the row
/// is the one that drops whatever is left.
struct Removal {
    label: &'static str,
    keys: Vec<String>,
    /// `true` for the row that names every key the resource still holds, whose key list is
    /// computed from the expected map rather than written out.
    remove_everything: bool,
}

impl Removal {
    fn of(label: &'static str, keys: &[&str]) -> Self {
        Self {
            label,
            keys: keys.iter().map(|key| (*key).to_string()).collect(),
            remove_everything: false,
        }
    }
}

/// The removals every taggable resource is put through, in order, each asserted against
/// the map the test computes for itself.
///
/// The rows differ along what the rule generalises over: how many keys a call names,
/// whether the resource holds them, whether a key is named twice, what a key spells, and
/// the removal that empties the map.
fn removals() -> Vec<Removal> {
    vec![
        Removal::of("one key the resource holds", &["mission"]),
        Removal::of("the same removal again", &["mission"]),
        Removal::of("several keys at once", &["stage", "owner"]),
        Removal::of("a key the resource never held", &["never-assigned"]),
        Removal::of("a key named twice in one request", &["note", "note"]),
        Removal::of("a key over the documented tag character set", &[WIDE_KEY]),
        Removal::of(
            "held and absent keys in one request",
            &["bulk-00", "bulk-01", "absent-one", "absent-two"],
        ),
        Removal {
            label: "every key the resource still holds",
            keys: Vec::new(),
            remove_everything: true,
        },
        Removal::of("a key of a resource that now holds none", &["mission"]),
    ]
}

/// Seed `arn` with [`seeded`] and answer the map the resource then holds, created tags
/// included.
async fn seed(router: &MultiServiceRouter, case: &str, arn: &str) -> BTreeMap<String, String> {
    let assigned = tag_resource(router, arn, &seeded()).await;
    assert_eq!(
        assigned.status, TAG_MUTATION_SUCCESS_STATUS,
        "case {case}: expected the seeding TagResource {TAG_MUTATION_SUCCESS_STATUS}, got {} {}",
        assigned.status, assigned.body
    );
    let listed = list_tags(router, arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case}: expected ListTagsForResource 200, got {} {}",
        listed.status, listed.body
    );
    reported_tags(case, &listed)
}

#[tokio::test]
async fn untag_resource_drops_the_named_keys_from_the_resource_its_arn_names() {
    // The taggable resources include a reserved contact, so the account is served by the
    // station that reservation is held at.
    let router = harness_serving(STATION);

    for resource in taggable_resources(&router).await {
        let Taggable {
            label,
            arn,
            own_read,
            // What the create stored is part of the map the seeding below merges into,
            // which is `crate::resource_tagging`'s subject: the removals here start from
            // whatever the resource actually holds.
            created_with: _,
        } = resource;

        let mut expected = seed(&router, &label, &arn).await;

        for removal in removals() {
            let case = format!("{label}: {}", removal.label);
            let keys: Vec<String> = if removal.remove_everything {
                expected.keys().cloned().collect()
            } else {
                removal.keys.clone()
            };
            let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();

            let removed = untag_resource(&router, &arn, &borrowed).await;
            assert_eq!(
                removed.status, TAG_MUTATION_SUCCESS_STATUS,
                "case {case}: expected UntagResource {TAG_MUTATION_SUCCESS_STATUS}, got {} {}",
                removed.status, removed.body
            );

            // The removal the rule describes, computed here rather than read back from
            // the emulator: the keys the request named, gone; everything else as it was.
            for key in &keys {
                expected.remove(key);
            }

            let listed = list_tags(&router, &arn).await;
            assert_eq!(
                listed.status, 200,
                "case {case}: expected ListTagsForResource 200, got {} {}",
                listed.status, listed.body
            );
            assert_eq!(
                reported_tags(&case, &listed),
                expected,
                "case {case}: exactly the named keys must be dropped, got {}",
                listed.body
            );

            // The tags a tag read reports are the resource's own: the SAME map its own
            // read answers, not a second copy beside it.
            if let Some(route) = &own_read {
                let read = send(&router, "GET", route, None).await;
                assert_eq!(
                    read.status, 200,
                    "case {case}: expected {route} 200, got {} {}",
                    read.status, read.body
                );
                assert_eq!(
                    reported_tags(&case, &read),
                    expected,
                    "case {case}: the resource's own read must report the remaining tags, got {}",
                    read.body
                );
            }
        }

        assert!(
            expected.is_empty(),
            "case {label}: the table must end with the resource holding no tag, got {expected:?}"
        );
    }
}

#[tokio::test]
async fn a_removal_touches_only_the_resource_its_arn_names() {
    let router = harness_serving(STATION);
    let resources = taggable_resources(&router).await;

    // One key every resource holds, so removing it from one is observable on all.
    let shared = tags_of(&[("shared", "yes")]);
    let mut before: Vec<(String, String, BTreeMap<String, String>)> = Vec::new();
    for resource in &resources {
        let case = format!("{} (before)", resource.label);
        let assigned = tag_resource(&router, &resource.arn, &shared).await;
        assert_eq!(
            assigned.status, TAG_MUTATION_SUCCESS_STATUS,
            "case {case}: expected TagResource {TAG_MUTATION_SUCCESS_STATUS}, got {} {}",
            assigned.status, assigned.body
        );
        let listed = list_tags(&router, &resource.arn).await;
        before.push((
            resource.label.clone(),
            resource.arn.clone(),
            reported_tags(&case, &listed),
        ));
    }

    // Each resource in turn is the ONE the key is removed from; every other must keep it.
    for (index, (target_label, target_arn, _)) in before.iter().enumerate() {
        let removed = untag_resource(&router, target_arn, &["shared"]).await;
        assert_eq!(
            removed.status, TAG_MUTATION_SUCCESS_STATUS,
            "case {target_label}: expected UntagResource {TAG_MUTATION_SUCCESS_STATUS}, got {} {}",
            removed.status, removed.body
        );

        for (other_index, (other_label, other_arn, other_tags)) in before.iter().enumerate() {
            let case = format!("{target_label} untagged, reading {other_label}");
            let listed = list_tags(&router, other_arn).await;
            let mut expected = other_tags.clone();
            if other_index <= index {
                // Already had its own turn (or is the target): the key is gone.
                expected.remove("shared");
            }
            assert_eq!(
                reported_tags(&case, &listed),
                expected,
                "case {case}: a removal must change only the resource its ARN names, got {}",
                listed.body
            );
        }
    }
}

#[tokio::test]
async fn an_arn_that_names_no_resource_is_refused_by_the_removal() {
    let router = harness();

    // A config created only to be DELETED: an ARN whose resource is gone resolves to
    // nothing, exactly as one that never existed does.
    let (config_id, config_type, deleted_arn) = create_config(
        &router,
        "deleted before untagging",
        json!({"trackingConfig": {"autotrack": "PREFERRED"}}),
        Some(json!({"stage": "created"})),
    )
    .await;
    let deleted = send(
        &router,
        "DELETE",
        &format!("/config/{config_type}/{config_id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case delete first: expected DeleteConfig 200, got {} {}",
        deleted.status, deleted.body
    );

    // A live resource, so the suite also shows the refusals removed nothing anywhere.
    let live_arn = fresh_config(&router, "untouched by the refusals").await;
    let assigned = tag_resource(&router, &live_arn, &tags_of(&[("stage", "created")])).await;
    assert_eq!(
        assigned.status, TAG_MUTATION_SUCCESS_STATUS,
        "case the live resource: expected TagResource {TAG_MUTATION_SUCCESS_STATUS}, got {} {}",
        assigned.status, assigned.body
    );

    let account = engine::emulator_account_id();
    let absent: Vec<(&str, String)> = vec![
        (
            "a config that was never created",
            ABSENT_CONFIG_ARN.to_string(),
        ),
        (
            "a mission profile that was never created",
            ABSENT_MISSION_PROFILE_ARN.to_string(),
        ),
        ("a config that was deleted", deleted_arn),
        (
            "a contact that was never reserved",
            contact_arn(ABSENT_UUID),
        ),
        ("an agent that was never registered", agent_arn(ABSENT_UUID)),
        (
            "a dataflow endpoint group that was never created",
            format!(
                "arn:aws:groundstation:{REGION}:{account}:dataflow-endpoint-group/{ABSENT_UUID}"
            ),
        ),
        (
            "a resource of another service",
            "arn:aws:s3:::ground-station-downlink".to_string(),
        ),
    ];

    for (case, arn) in absent {
        let refused = untag_resource(&router, &arn, &["stage"]).await;
        assert_eq!(
            refused.status, 434,
            "case {case}: UntagResource must answer 434, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "NotFoundException",
            "case {case}: UntagResource must answer NotFoundException, got {}",
            refused.error_code()
        );
        let message = refused.field("message").as_str().unwrap_or_default();
        assert_eq!(
            message, "Resource was not found",
            "case {case}: the refusal states one fixed sentence, got {message:?}"
        );
    }

    // Nothing the refusals touched: the live resource still holds its own tag.
    let listed = list_tags(&router, &live_arn).await;
    assert_eq!(
        reported_tags("the live resource", &listed),
        tags_of(&[("stage", "created")]),
        "case the live resource: a refused removal must remove nothing, got {}",
        listed.body
    );
}

/// The key lists the rules refuse, each row differing in WHAT it breaks, paired with the
/// key of the same request that is the caller's to remove (so the refusal is shown to cost
/// the whole call).
fn illegal_key_lists() -> Vec<(String, Vec<String>)> {
    let (_, max_key) = TAG_KEY_CHARS;
    vec![
        (
            "a key in the reserved namespace".to_string(),
            vec![format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name")],
        ),
        (
            "the reserved prefix as the whole key".to_string(),
            vec![RESERVED_TAG_KEY_PREFIX.to_string()],
        ),
        (
            "the reserved prefix spelled in upper case".to_string(),
            vec![format!("{}owner", RESERVED_TAG_KEY_PREFIX.to_uppercase())],
        ),
        (
            "the reserved prefix spelled in mixed case".to_string(),
            vec!["Aws:createdBy".to_string()],
        ),
        (
            "a reserved key beside a legal one".to_string(),
            vec!["mission".to_string(), "aws:createdBy".to_string()],
        ),
        (
            format!("a key of {} characters, one past the ceiling", max_key + 1),
            vec![sized('k', max_key + 1)],
        ),
        (
            "a key far past the ceiling".to_string(),
            vec![sized('k', 300)],
        ),
        (
            format!(
                "a key of {} multi-byte characters, one past the ceiling",
                max_key + 1
            ),
            vec![sized(WIDE_CHAR, max_key + 1)],
        ),
    ]
}

#[tokio::test]
async fn a_removal_is_held_to_the_platform_tag_key_rules() {
    let router = harness();
    let target = fresh_config(&router, "tag key rules target").await;
    let held = seed(&router, "tag key rules", &target).await;

    for (row, keys) in illegal_key_lists() {
        let case = format!("UntagResource / {row}");
        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
        let refused = untag_resource(&router, &target, &borrowed).await;
        assert_eq!(
            refused.status, 431,
            "case {case:?}: expected 431, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {}",
            refused.body
        );
        assert_eq!(
            refused.field("parameterName"),
            &json!(TAG_KEYS_PARAMETER),
            "case {case:?}: the refusal must name the tagKeys member, got {}",
            refused.body
        );

        let after = list_tags(&router, &target).await;
        assert_eq!(
            reported_tags(&case, &after),
            held,
            "case {case:?}: a refused removal must remove nothing, got {}",
            after.body
        );
    }
}

#[tokio::test]
async fn a_key_the_rules_allow_is_removed_at_every_published_bound() {
    let router = harness();
    let (min_key, max_key) = TAG_KEY_CHARS;

    // Each row is a key the rules DO allow, including both ends of the published key
    // length bound, a length counted in characters rather than bytes, and the near miss of
    // the reserved namespace — assigned first, then removed, so the removal is asserted on
    // a key the resource genuinely holds.
    let legal: Vec<(String, String)> = vec![
        (
            format!("a key of {min_key} character, the floor"),
            sized('k', min_key),
        ),
        (
            format!("a key of {max_key} characters, the ceiling"),
            sized('k', max_key),
        ),
        (
            format!("a key of {max_key} multi-byte characters, the ceiling"),
            sized(WIDE_CHAR, max_key),
        ),
        (
            "a key that CONTAINS the reserved prefix without starting with it".to_string(),
            "x-aws:owner".to_string(),
        ),
        (
            "a key over the documented tag character set".to_string(),
            WIDE_KEY.to_string(),
        ),
    ];

    for (index, (row, key)) in legal.into_iter().enumerate() {
        let case = format!("UntagResource / {row}");
        let arn = fresh_config(&router, &format!("legal key {index}")).await;
        let mut assigned_map = BTreeMap::new();
        assigned_map.insert(key.clone(), "v".to_string());
        assigned_map.insert("kept".to_string(), "yes".to_string());
        let assigned = tag_resource(&router, &arn, &assigned_map).await;
        assert_eq!(
            assigned.status, TAG_MUTATION_SUCCESS_STATUS,
            "case {case:?}: expected TagResource {TAG_MUTATION_SUCCESS_STATUS}, got {} {}",
            assigned.status, assigned.body
        );

        let removed = untag_resource(&router, &arn, &[key.as_str()]).await;
        assert_eq!(
            removed.status, TAG_MUTATION_SUCCESS_STATUS,
            "case {case:?}: expected UntagResource {TAG_MUTATION_SUCCESS_STATUS}, got {} {}",
            removed.status, removed.body
        );

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            reported_tags(&case, &listed),
            tags_of(&[("kept", "yes")]),
            "case {case:?}: the named key must be gone and the other kept, got {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn the_modelled_shape_of_the_key_list_is_enforced_before_the_handler() {
    let router = harness();
    let target = fresh_config(&router, "modelled key list").await;
    let held = seed(&router, "modelled key list", &target).await;

    // `tagKeys` members have a modelled floor of one character
    // (`smithy_query(query="shape", shape="TagKeys")`), so a request naming an empty key
    // is refused by the model's own rules before the handler runs.
    for (case, keys) in [("an empty key", vec![String::new()])] {
        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
        let refused = untag_resource(&router, &target, &borrowed).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected the modelled refusal 400, got {} {}",
            refused.status, refused.body
        );
        let after = list_tags(&router, &target).await;
        assert_eq!(
            reported_tags(case, &after),
            held,
            "case {case}: a refused removal must remove nothing, got {}",
            after.body
        );
    }

    // The list's own length bound, at the ceiling and one past it: at the ceiling the keys
    // are dropped (the ones the resource holds) and past it the model refuses the request.
    let at_ceiling: Vec<String> = held
        .keys()
        .cloned()
        .chain((0..MAX_TAG_KEYS).map(|index| format!("filler-{index:04}")))
        .take(MAX_TAG_KEYS)
        .collect();
    assert_eq!(
        at_ceiling.len(),
        MAX_TAG_KEYS,
        "case at the key-list ceiling: the row must name {MAX_TAG_KEYS} keys, got {}",
        at_ceiling.len()
    );
    let borrowed: Vec<&str> = at_ceiling.iter().map(String::as_str).collect();
    let removed = untag_resource(&router, &target, &borrowed).await;
    assert_eq!(
        removed.status, TAG_MUTATION_SUCCESS_STATUS,
        "case at the key-list ceiling: expected UntagResource {TAG_MUTATION_SUCCESS_STATUS}, \
         got {} {}",
        removed.status, removed.body
    );
    let listed = list_tags(&router, &target).await;
    let mut expected = held.clone();
    for key in &at_ceiling {
        expected.remove(key);
    }
    assert_eq!(
        reported_tags("at the key-list ceiling", &listed),
        expected,
        "case at the key-list ceiling: the named keys must be dropped, got {}",
        listed.body
    );

    // One past the ceiling, on a resource seeded afresh so the refusal is observable.
    let past = fresh_config(&router, "past the key-list ceiling").await;
    let held = seed(&router, "past the key-list ceiling", &past).await;
    let too_many: Vec<String> = (0..=MAX_TAG_KEYS)
        .map(|index| format!("filler-{index:04}"))
        .collect();
    let borrowed: Vec<&str> = too_many.iter().map(String::as_str).collect();
    let refused = untag_resource(&router, &past, &borrowed).await;
    assert_eq!(
        refused.status, 400,
        "case one past the key-list ceiling: expected the modelled refusal 400, got {} {}",
        refused.status, refused.body
    );
    let after = list_tags(&router, &past).await;
    assert_eq!(
        reported_tags("one past the key-list ceiling", &after),
        held,
        "case one past the key-list ceiling: a refused removal must remove nothing, got {}",
        after.body
    );
}
