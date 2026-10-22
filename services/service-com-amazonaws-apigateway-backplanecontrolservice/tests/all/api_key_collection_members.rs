// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the collection members of the `ApiKey` body: `stageKeys` and
//! `tags`.
//!
//! The RULE under test: the `ApiKey` body reports every collection the stored record
//! owns, and reports it from the record whatever it holds — a key with no stage
//! associations reports `stageKeys: []` and an untagged key reports `tags: {}`. Member
//! presence is never decided by emptiness, so a client can tell "no stages / no tags"
//! from "not reported". It holds on every path that renders the shape: `CreateApiKey`
//! (POST /apikeys, 201) and `GetApiKey` (GET /apikeys/{apiKey}, 200), whose output
//! shape is the same `ApiKey` (`smithy_query(query="operation",
//! operation="CreateApiKey,GetApiKey")`: both carry `stageKeys` (ListOfString) and
//! `tags` (MapOfStringToString)).
//!
//! The table's rows differ along what the rule generalises over: how many tags the key
//! carries (none, one, several), how many stage associations it has (none, one, several,
//! across one API and two) and the two combined — the recorded case (no tags, no stages)
//! is one row of it.
//!
//! [`Gateway`], [`Answer`], [`create_api`] and [`string_member`] are this crate's one
//! wire-test harness, reused rather than standing a second emulator up another way.

use serde_json::{json, Map, Value};

use crate::rest_api_lifecycle::{create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::create_deployed_stage;

/// The members of an `ApiKey` body this rule governs, with the JSON shape each one is
/// reported as.
const COLLECTION_MEMBERS: &[(&str, &str)] = &[("stageKeys", "array"), ("tags", "object")];

/// Assert `body` reports each collection member as PRESENT and of the modelled JSON
/// shape — the heart of the rule, checked before any value comparison so an omitted
/// member is never read as an empty one.
fn assert_collections_present(body: &Value, case: &str) {
    let object = body
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a JSON object body, got {body:?}"));
    for (member, shape) in COLLECTION_MEMBERS {
        let value = object.get(*member).unwrap_or_else(|| {
            panic!("case {case}: expected the {member} member to be present, got {body:?}")
        });
        let present = match *shape {
            "array" => value.is_array(),
            _ => value.is_object(),
        };
        assert!(
            present,
            "case {case}: expected {member} to be reported as a JSON {shape}, got {value:?}"
        );
    }
}

/// One row of the table: the case name, how many tags the key carries, and the
/// `(restApiId, stageName)` pairs it is associated with.
type Row = (&'static str, usize, Vec<(String, &'static str)>);

/// A tag map of `count` entries, each entry distinct.
fn tag_map(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|n| (format!("k{n}"), Value::String(format!("v{n}"))))
            .collect::<Map<String, Value>>(),
    )
}

/// `GetApiKey` for `id`.
async fn get_key(gateway: &Gateway, id: &str) -> Answer {
    gateway.call("GET", &format!("/apikeys/{id}"), None).await
}

#[tokio::test]
async fn an_api_key_body_reports_its_stage_keys_and_tags_however_many_it_holds() {
    let gateway = Gateway::new();
    let (first, _) = create_api(&gateway, "collection-members-one").await;
    let (second, _) = create_api(&gateway, "collection-members-two").await;

    // A `stageKeys` entry must name a stage that exists, so every stage this table
    // associates a key with is deployed first — over the wire, through
    // `CreateDeployment`.
    for stage in ["prod", "beta", "canary", "gamma"] {
        create_deployed_stage(&gateway, &first, stage).await;
    }
    create_deployed_stage(&gateway, &second, "prod").await;

    // Each row: the case name, the tag count, and the (restApiId, stageName) pairs the
    // key is created with. Row 1 is the recorded case — no tags, no stage associations.
    let cases: Vec<Row> = vec![
        ("no tags and no stages", 0, vec![]),
        ("no tags, one stage", 0, vec![(first.clone(), "prod")]),
        ("one tag, no stages", 1, vec![]),
        ("one tag, one stage", 1, vec![(first.clone(), "beta")]),
        (
            "three tags, two stages of one api",
            3,
            vec![(first.clone(), "prod"), (first.clone(), "canary")],
        ),
        (
            "ten tags, stages of two apis",
            10,
            vec![(first.clone(), "gamma"), (second.clone(), "prod")],
        ),
    ];

    for (case, tag_count, stages) in &cases {
        let tags = tag_map(*tag_count);
        let request = json!({
            "name": format!("key-{case}"),
            "tags": tags.clone(),
            "stageKeys": stages
                .iter()
                .map(|(api, stage)| json!({ "restApiId": api, "stageName": stage }))
                .collect::<Vec<Value>>(),
        });
        let created = gateway.call("POST", "/apikeys", Some(request)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateApiKey, got {} with {:?}",
            created.status, created.body
        );
        assert_collections_present(&created.body, &format!("CreateApiKey / {case}"));

        let expected_stage_keys = Value::Array(
            stages
                .iter()
                .map(|(api, stage)| Value::String(format!("{api}/{stage}")))
                .collect(),
        );
        assert_eq!(
            created.body["stageKeys"], expected_stage_keys,
            "case CreateApiKey / {case}: expected {expected_stage_keys:?}, got {:?}",
            created.body["stageKeys"]
        );
        assert_eq!(
            created.body["tags"], tags,
            "case CreateApiKey / {case}: expected {tags:?}, got {:?}",
            created.body["tags"]
        );

        // The same rule on the read path, over the persisted record.
        let id = string_member(&created.body, "id", case);
        let read = get_key(&gateway, &id).await;
        assert_eq!(
            read.status, 200,
            "case GetApiKey / {case}: expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_collections_present(&read.body, &format!("GetApiKey / {case}"));
        assert_eq!(
            read.body["stageKeys"], expected_stage_keys,
            "case GetApiKey / {case}: expected {expected_stage_keys:?}, got {:?}",
            read.body["stageKeys"]
        );
        assert_eq!(
            read.body["tags"], tags,
            "case GetApiKey / {case}: expected {tags:?}, got {:?}",
            read.body["tags"]
        );
    }
}

#[tokio::test]
async fn an_empty_collection_is_reported_the_same_whether_the_request_omitted_it_or_sent_it_empty() {
    // Rows differ in the FORM the request takes for a collection it sets to nothing:
    // absent, explicitly empty, and explicitly null. All three store nothing, so all
    // three must be reported identically — as the empty collection.
    let cases: Vec<(&str, Value)> = vec![
        ("both members absent", json!({ "name": "absent" })),
        (
            "both members explicitly empty",
            json!({ "name": "empty", "stageKeys": [], "tags": {} }),
        ),
        (
            "tags empty, stageKeys absent",
            json!({ "name": "half-empty", "tags": {} }),
        ),
        (
            "both members null",
            json!({ "name": "null", "stageKeys": Value::Null, "tags": Value::Null }),
        ),
    ];

    let gateway = Gateway::new();
    for (case, request) in &cases {
        let created = gateway.call("POST", "/apikeys", Some(request.clone())).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateApiKey, got {} with {:?}",
            created.status, created.body
        );
        assert_collections_present(&created.body, &format!("CreateApiKey / {case}"));
        assert_eq!(
            created.body["stageKeys"],
            json!([]),
            "case CreateApiKey / {case}: expected an empty list, got {:?}",
            created.body["stageKeys"]
        );
        assert_eq!(
            created.body["tags"],
            json!({}),
            "case CreateApiKey / {case}: expected an empty map, got {:?}",
            created.body["tags"]
        );

        let id = string_member(&created.body, "id", case);
        // Read twice: the empty collections are reported from the record every time, and
        // reading one does not change what the next caller sees.
        for round in 0..2 {
            let read = get_key(&gateway, &id).await;
            let case = format!("GetApiKey(round {round}) / {case}");
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                read.status, read.body
            );
            assert_collections_present(&read.body, &case);
            assert_eq!(
                read.body["stageKeys"],
                json!([]),
                "case {case}: expected an empty list, got {:?}",
                read.body["stageKeys"]
            );
            assert_eq!(
                read.body["tags"],
                json!({}),
                "case {case}: expected an empty map, got {:?}",
                read.body["tags"]
            );
        }
    }
}
