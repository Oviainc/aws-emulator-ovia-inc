// SPDX-License-Identifier: CC0-1.0

//! Every `TagList` this service stores is read by ONE rule, whichever member and
//! whichever operation carried it.
//!
//! The rule: a `TagList` is merged onto the tag set the resource holds — an entry with
//! no `Key` names no tag and is skipped, an entry with no `Value` stores the empty
//! value, a key repeated in the list keeps the last value, and a key already stored
//! takes the new value (`Tag.Key` and `Tag.Value` are both optional in the model,
//! `smithy_query(query="shape",shape="Tag")`).
//!
//! Each row is driven through the four members that carry caller tags — the create's
//! top-level `Tags`, the create's `ManagedInstance.TagSpecifications`, `TagResource`,
//! and `CreateVolume`'s `TagSpecifications` — and every path must store the SAME set,
//! read back out of the store (`ListTagsForResource` for an instance, the service's own
//! state for a volume, which no operation reports yet).
//!
//! The wire helpers come from the `workspace_instance_lifecycle` suite.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{
    create, harness, launchable_managed_instance, ok, stored_tags,
};

/// A `TagList` body. Each entry carries only the members the row names, so a row can
/// send a tag with no key, or none with no value: both members are optional.
fn tag_entries(entries: &[(Option<&str>, Option<&str>)]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|(key, value)| {
                let mut entry = json!({});
                if let Some(key) = key {
                    entry["Key"] = json!(key);
                }
                if let Some(value) = value {
                    entry["Value"] = json!(value);
                }
                entry
            })
            .collect(),
    )
}

/// The tag map a row expects, from pairs.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One `TagList` a request can carry, and the tag set the resource must hold after it.
struct Row {
    case: &'static str,
    tags: Value,
    expected: BTreeMap<String, String>,
}

/// Rows spanning what the rule generalises over: how many entries the list carries
/// (none, one, several, the modelled maximum), which optional members each entry fills,
/// a key repeated inside the list, and the character range a key and a value may use.
fn rows() -> Vec<Row> {
    // The modelled maximum one `TagList` can carry: `Tags` is `TagList len 0..50`
    // (`wsinst::state::MAX_TAGS_PER_INSTANCE`,
    // `smithy_query(query="operation",operation="TagResource")`).
    let limit = wsinst::state::MAX_TAGS_PER_INSTANCE;
    let full: Vec<(String, String)> = (0..limit)
        .map(|i| (format!("key-{i:02}"), format!("value-{i:02}")))
        .collect();
    let full_pairs: Vec<(&str, &str)> = full
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let full_entries: Vec<(Option<&str>, Option<&str>)> = full_pairs
        .iter()
        .map(|(key, value)| (Some(*key), Some(*value)))
        .collect();

    vec![
        Row {
            case: "an empty list",
            tags: tag_entries(&[]),
            expected: BTreeMap::new(),
        },
        Row {
            case: "one key and value",
            tags: tag_entries(&[(Some("env"), Some("dev"))]),
            expected: expected(&[("env", "dev")]),
        },
        Row {
            case: "several keys at once",
            tags: tag_entries(&[
                (Some("env"), Some("prod")),
                (Some("owner"), Some("alice")),
                (Some("tier"), Some("gold")),
            ]),
            expected: expected(&[("env", "prod"), ("owner", "alice"), ("tier", "gold")]),
        },
        Row {
            case: "a key repeated in the list keeps the last value",
            tags: tag_entries(&[
                (Some("env"), Some("first")),
                (Some("other"), Some("kept")),
                (Some("env"), Some("last")),
            ]),
            expected: expected(&[("env", "last"), ("other", "kept")]),
        },
        Row {
            case: "an entry with no value stores the empty value",
            tags: tag_entries(&[(Some("marker"), None), (Some("env"), Some("dev"))]),
            expected: expected(&[("marker", ""), ("env", "dev")]),
        },
        Row {
            case: "an entry with an explicitly empty value",
            tags: tag_entries(&[(Some("cost-centre"), Some(""))]),
            expected: expected(&[("cost-centre", "")]),
        },
        Row {
            case: "an entry with no key names no tag and is skipped",
            tags: tag_entries(&[(None, Some("orphan")), (Some("env"), Some("dev"))]),
            expected: expected(&[("env", "dev")]),
        },
        Row {
            case: "an entry with neither member",
            tags: tag_entries(&[(None, None)]),
            expected: BTreeMap::new(),
        },
        Row {
            case: "a keyless entry repeated after the key it does not name",
            tags: tag_entries(&[(Some("env"), Some("kept")), (None, Some("ignored"))]),
            expected: expected(&[("env", "kept")]),
        },
        Row {
            case: "keys and values outside ASCII",
            tags: tag_entries(&[
                (Some("Ünïcode.Key"), Some("valeur/accentuée")),
                (Some("日本語"), Some("テスト")),
            ]),
            expected: expected(&[("Ünïcode.Key", "valeur/accentuée"), ("日本語", "テスト")]),
        },
        Row {
            case: "the full punctuation set a key and a value may use",
            tags: tag_entries(&[(
                Some("a_b.c:d/e=f+g-h@i"),
                Some("j_k.l:m/n=o+p-q@r"),
            )]),
            expected: expected(&[("a_b.c:d/e=f+g-h@i", "j_k.l:m/n=o+p-q@r")]),
        },
        Row {
            case: "the modelled maximum number of entries",
            tags: tag_entries(&full_entries),
            expected: expected(&full_pairs),
        },
    ]
}

/// `CreateVolume`, returning the minted id.
async fn create_volume(router: &MultiServiceRouter, body: Value) -> String {
    let payload = ok(router, "CreateVolume", body).await;
    payload["VolumeId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateVolume returned no VolumeId: {payload}"))
        .to_string()
}

#[tokio::test]
async fn every_tag_list_member_is_read_by_the_same_merge_rule() {
    let (router, engine) = harness();
    let store = wsinst::state::service_state(&*engine).expect("the service's own store");

    for row in rows() {
        // The create's top-level `Tags`.
        let instance = create(
            &router,
            json!({
                "ManagedInstance": launchable_managed_instance(),
                "Tags": row.tags.clone(),
            }),
        )
        .await;
        let via_create_tags = stored_tags(&router, &instance).await;

        // The create's `ManagedInstance.TagSpecifications`.
        let mut managed_instance = launchable_managed_instance();
        managed_instance["TagSpecifications"] =
            json!([{ "ResourceType": "instance", "Tags": row.tags.clone() }]);
        let specified = create(&router, json!({ "ManagedInstance": managed_instance })).await;
        let via_create_specifications = stored_tags(&router, &specified).await;

        // `TagResource` on an instance created with no tags at all.
        let tagged = create(&router, json!({ "ManagedInstance": launchable_managed_instance() })).await;
        ok(
            &router,
            "TagResource",
            json!({ "WorkspaceInstanceId": tagged, "Tags": row.tags.clone() }),
        )
        .await;
        let via_tag_resource = stored_tags(&router, &tagged).await;

        // `CreateVolume`'s `TagSpecifications`.
        let volume = create_volume(
            &router,
            json!({
                "AvailabilityZone": "us-east-1a",
                "SizeInGB": 4,
                "TagSpecifications": [{ "ResourceType": "volume", "Tags": row.tags.clone() }],
            }),
        )
        .await;
        // No operation reports a volume's tags yet, so the volume path is read out of
        // the service's own state.
        let via_create_volume = store
            .lock()
            .volumes
            .get(&volume)
            .unwrap_or_else(|| panic!("the create stored no volume {volume:?}"))
            .tags
            .clone();

        for (path, actual) in [
            ("the create's Tags member", &via_create_tags),
            ("the create's TagSpecifications", &via_create_specifications),
            ("TagResource", &via_tag_resource),
            ("CreateVolume's TagSpecifications", &via_create_volume),
        ] {
            assert_eq!(
                *actual, row.expected,
                "case {:?} through {path}: expected the stored tags {:?}, got {actual:?}",
                row.case, row.expected
            );
        }
    }
}

#[tokio::test]
async fn a_tag_list_merges_onto_a_tag_set_the_resource_already_holds() {
    let (router, _engine) = harness();

    // The same rows, applied to a resource that already holds tags: a key the list does
    // not name is left alone, and a key it does name takes the new value.
    let base = [("env", "before"), ("keep", "untouched")];
    for row in rows() {
        let id = create(
            &router,
            json!({
                "ManagedInstance": launchable_managed_instance(),
                "Tags": tag_entries(
                    &base
                        .iter()
                        .map(|(key, value)| (Some(*key), Some(*value)))
                        .collect::<Vec<_>>(),
                ),
            }),
        )
        .await;

        let mut want = expected(&base);
        want.extend(row.expected.clone());
        // A row at the modelled maximum plus the base set would pass the bound the
        // resource's reported `TagList` can carry, which is its own refusal rule.
        if want.len() > wsinst::state::MAX_TAGS_PER_INSTANCE {
            continue;
        }

        ok(
            &router,
            "TagResource",
            json!({ "WorkspaceInstanceId": id, "Tags": row.tags.clone() }),
        )
        .await;

        let actual = stored_tags(&router, &id).await;
        assert_eq!(
            actual, want,
            "case {:?} merged onto a tagged instance: expected {want:?}, got {actual:?}",
            row.case
        );
    }
}
