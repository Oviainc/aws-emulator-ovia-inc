// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, over every operation that stores caller tags.
//!
//! The rule: a tag key a caller supplies may not begin with the prefix AWS keeps for
//! the tags it generates itself (`crate`'s `state::RESERVED_TAG_KEY_PREFIX`), in any
//! mix of upper and lower case; a request carrying one is refused with the modelled
//! `ValidationException` (HTTP 400) naming the input member the keys arrived in, and
//! nothing is stored for it. Only the PREFIX is reserved, so a key that merely
//! contains it, or stops short of the colon, is an ordinary key and persists.
//!
//! Every row goes through the router and is read back out of the store — through
//! `ListTagsForResource` for an instance, and through the service's own state for a
//! volume, which no operation reports yet.
//!
//! The wire helpers come from the sibling `workspace_instance_lifecycle` suite.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;

use crate::workspace_instance_lifecycle::{
    call, create, error_code, harness, launchable_managed_instance, list_all_ids, minimal_create,
    ok, router, stored_tags,
};

/// Keys inside the reserved namespace, varying the case of the prefix, what follows
/// it, and the script of the rest. Each one satisfies the modelled `TagKey`
/// (`len 1..128`, `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`,
/// `smithy_query(query="shape",shape="TagKey")`), so only a handler rule can refuse it.
fn reserved_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a system tag key", "aws:cloudformation:stack-name"),
        ("the prefix alone", "aws:"),
        ("the prefix upper case", "AWS:Region"),
        ("the prefix in mixed case", "Aws:CreatedBy"),
        ("the prefix in alternating case", "aWs:x"),
        ("non-ASCII after the prefix", "aws:étiquette"),
        ("the prefix with a long tail", "aws:servicecatalog/provisioning-principal-arn"),
    ]
}

/// Keys that are NOT in the reserved namespace, each differing from one of the rows
/// above in exactly what the rule turns on: the prefix must be the key's start, and
/// must be the whole four characters.
fn ordinary_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("the prefix without its colon", "aws"),
        ("a longer word starting with the letters", "awsome:team"),
        ("the letters then a dash", "aws-managed:owner"),
        ("the prefix after another segment", "my:aws:key"),
        ("the prefix with a letter before it", "paws:cat"),
        ("the letters then a dot", "aws.region"),
        ("a leading space before the prefix", " aws:not-reserved"),
        ("the letters upper case without a colon", "AWS"),
    ]
}

/// A `Tags` list body for `TagResource` / `CreateWorkspaceInstance`.
fn tags(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// The refusal every row expects: HTTP 400, the modelled `ValidationException`, and
/// `FieldList` naming the member the tags arrived in — with none of the submitted key
/// echoed back.
fn assert_refused(case: &str, member: &str, status: u16, payload: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a reserved tag key, got {status} with {payload}"
    );
    assert_eq!(
        error_code(payload),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {payload}"
    );
    let named: Vec<&str> = payload["FieldList"]
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field["Name"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        named.contains(&member),
        "case {case:?}: expected FieldList to name {member:?}, got {named:?} in {payload}"
    );
}

#[tokio::test]
async fn tag_resource_refuses_a_reserved_key_and_leaves_the_tag_set_unchanged() {
    let router = router();
    let id = create(
        &router,
        json!({
            "ManagedInstance": launchable_managed_instance(),
            "Tags": tags(&[("env", "dev")]),
        }),
    )
    .await;
    let before: BTreeMap<String, String> = stored_tags(&router, &id).await;

    for (case, key) in reserved_keys() {
        let (status, payload) = call(
            &router,
            "TagResource",
            json!({ "WorkspaceInstanceId": &id, "Tags": tags(&[(key, "mine")]) }),
        )
        .await;
        assert_refused(case, "Tags", status, &payload);
        // The refusal states the rule, not what the caller sent: the submitted key is
        // not echoed back. (The row that IS the bare prefix is excepted — the rule's
        // own wording names the prefix.)
        assert!(
            key == wsinst::state::RESERVED_TAG_KEY_PREFIX || !payload.to_string().contains(key),
            "case {case:?}: the refusal must not echo the submitted key, got {payload}"
        );

        let after = stored_tags(&router, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused TagResource changed the stored tags, got {after:?}"
        );
    }

    // A list that mixes an ordinary key with a reserved one is refused WHOLE: the
    // ordinary key of that same request is not stored either.
    for (case, key) in reserved_keys() {
        let (status, payload) = call(
            &router,
            "TagResource",
            json!({
                "WorkspaceInstanceId": &id,
                "Tags": tags(&[("team", "euc"), (key, "mine")]),
            }),
        )
        .await;
        assert_refused(case, "Tags", status, &payload);
        let after = stored_tags(&router, &id).await;
        assert_eq!(
            after, before,
            "case {case:?} mixed with an ordinary key: expected nothing stored, got {after:?}"
        );
    }
}

#[tokio::test]
async fn tag_resource_stores_a_key_outside_the_reserved_namespace() {
    let router = router();
    let id = create(&router, minimal_create()).await;
    let mut expected: BTreeMap<String, String> = BTreeMap::new();

    for (case, key) in ordinary_keys() {
        ok(
            &router,
            "TagResource",
            json!({ "WorkspaceInstanceId": &id, "Tags": tags(&[(key, "mine")]) }),
        )
        .await;
        expected.insert(key.to_string(), "mine".to_string());

        let after = stored_tags(&router, &id).await;
        assert_eq!(
            after, expected,
            "case {case:?} ({key:?}): expected the key to persist, got {after:?}"
        );
    }
}

#[tokio::test]
async fn create_refuses_a_reserved_key_and_creates_no_instance() {
    let router = router();
    let existing = create(&router, minimal_create()).await;

    for (case, key) in reserved_keys() {
        let (status, payload) = call(
            &router,
            "CreateWorkspaceInstance",
            json!({
                "ManagedInstance": launchable_managed_instance(),
                "Tags": tags(&[(key, "mine")]),
            }),
        )
        .await;
        assert_refused(case, "Tags", status, &payload);

        let listed = list_all_ids(&router, json!({})).await;
        assert_eq!(
            listed,
            vec![existing.clone()],
            "case {case:?}: a refused create left an instance behind, got {listed:?}"
        );
    }

    // The refusal happens before the store is reached, so the `ClientToken` of a
    // refused create is not retired: the same token still creates once it is sent
    // with keys this service accepts.
    let token = "tok-reserved";
    let (status, payload) = call(
        &router,
        "CreateWorkspaceInstance",
        json!({
            "ClientToken": token,
            "ManagedInstance": launchable_managed_instance(),
            "Tags": tags(&[("aws:created-by", "me")]),
        }),
    )
    .await;
    assert_refused("a refused create carrying a token", "Tags", status, &payload);
    let minted = create(
        &router,
        json!({
            "ClientToken": token,
            "ManagedInstance": launchable_managed_instance(),
            "Tags": tags(&[("env", "dev")]),
        }),
    )
    .await;
    let tagged = stored_tags(&router, &minted).await;
    assert_eq!(
        tagged,
        [("env".to_string(), "dev".to_string())]
            .into_iter()
            .collect::<BTreeMap<String, String>>(),
        "case a token reused after a refusal: expected only the accepted tag, got {tagged:?}"
    );
}

#[tokio::test]
async fn create_instance_stores_a_key_outside_the_reserved_namespace() {
    let router = router();

    for (case, key) in ordinary_keys() {
        let id = create(
            &router,
            json!({
                "ManagedInstance": launchable_managed_instance(),
                "Tags": tags(&[(key, "mine")]),
            }),
        )
        .await;
        let got = stored_tags(&router, &id).await;
        assert_eq!(
            got,
            [(key.to_string(), "mine".to_string())]
                .into_iter()
                .collect::<BTreeMap<String, String>>(),
            "case {case:?} ({key:?}): expected the key to persist, got {got:?}"
        );
    }
}

#[tokio::test]
async fn create_volume_refuses_a_reserved_key_in_any_tag_specification() {
    let (router, engine) = harness();
    let store = wsinst::state::service_state(&*engine).expect("the service's own store");

    // Every `ResourceTypeEnum` value the model declares, plus a specification that
    // names none: the prefix is reserved whichever resource the tags are for.
    let resource_types = wsinst::ops::types::RESOURCE_TYPE_ENUM_VALUES;
    assert!(
        resource_types.contains(&"volume"),
        "case catalog: the generated ResourceTypeEnum list must carry the modelled values, got {resource_types:?}"
    );
    let mut specification_kinds: Vec<Option<&str>> = vec![None];
    specification_kinds.extend(resource_types.iter().map(|kind| Some(*kind)));

    for (case, key) in reserved_keys() {
        for kind in &specification_kinds {
            let mut specification = json!({ "Tags": tags(&[(key, "mine")]) });
            if let Some(kind) = kind {
                specification["ResourceType"] = json!(kind);
            }
            let held = store.lock().volumes.len();

            let (status, payload) = call(
                &router,
                "CreateVolume",
                json!({
                    "AvailabilityZone": "us-east-1a",
                    "SizeInGB": 4,
                    "TagSpecifications": [specification],
                }),
            )
            .await;
            assert_refused(case, "TagSpecifications", status, &payload);
            let after = store.lock().volumes.len();
            assert_eq!(
                after, held,
                "case {case:?} for resource type {kind:?}: a refused create stored a volume, got {after} volumes"
            );
        }
    }

    // The same call with a key outside the reserved namespace creates the volume and
    // the key is on the stored record.
    for (case, key) in ordinary_keys() {
        let payload = ok(
            &router,
            "CreateVolume",
            json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 4,
                "TagSpecifications": [{ "ResourceType": "volume", "Tags": tags(&[(key, "mine")]) }],
            }),
        )
        .await;
        let id = payload["VolumeId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateVolume returned no VolumeId: {payload}"))
            .to_string();
        let stored = store
            .lock()
            .volumes
            .get(&id)
            .unwrap_or_else(|| panic!("case {case:?}: the create stored no volume {id:?}"))
            .tags
            .clone();
        assert_eq!(
            stored.get(key).map(String::as_str),
            Some("mine"),
            "case {case:?} ({key:?}): expected the key on the stored volume, got {stored:?}"
        );
    }
}

#[test]
fn the_reserved_prefix_rule_is_the_services_own_predicate() {
    // The helper the handlers share, exercised directly on the same rows: the wire
    // tests above assert the refusals, this pins the predicate they all read.
    for (case, key) in reserved_keys() {
        assert!(
            wsinst::state::is_reserved_tag_key(key),
            "case {case:?}: expected {key:?} to be reserved, got not reserved"
        );
    }
    for (case, key) in ordinary_keys() {
        assert!(
            !wsinst::state::is_reserved_tag_key(key),
            "case {case:?}: expected {key:?} to be an ordinary key, got reserved"
        );
    }
    // A key shorter than the prefix, and one whose first character is multi-byte,
    // are ordinary: neither can start with it.
    for key in ["a", "aw", "日本語:key", "ü"] {
        assert!(
            !wsinst::state::is_reserved_tag_key(key),
            "case a key that cannot carry the prefix: expected {key:?} to be ordinary, got reserved"
        );
    }
}
