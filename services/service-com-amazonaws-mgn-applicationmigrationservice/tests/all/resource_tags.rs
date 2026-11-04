// SPDX-License-Identifier: CC0-1.0

//! The mgn tag API, end to end over HTTP: `TagResource`, `UntagResource` and
//! `ListTagsForResource`, the three operations bound to the ONE `/tags/{resourceArn}`
//! route (`smithy_query(query="operation",operation="TagResource,UntagResource,
//! ListTagsForResource")`: `POST`, `DELETE` and `GET /tags/{resourceArn}`, each with
//! `resourceArn` as a non-greedy `@httpLabel`).
//!
//! The rule under test, over a TABLE of resource kinds and tag sets rather than one
//! recorded request: a tag write names a resource by the ARN that resource was created
//! with and stores the pairs it carries on THAT resource — adding or overwriting exactly
//! the keys it names and leaving every other key as it was; a read reports exactly the
//! stored set, through the tag reader AND through the resource's own reader; a removal
//! deletes exactly the keys it names and is a no-op for a key the resource never had;
//! and an ARN no resource of the account carries is refused 404 by all three.
//!
//! Every assertion goes through the router's `dispatch`, so it exercises the same
//! restJson1 deserialize → handle → serialize path a client drives, and the harness
//! (`Mgn`, `error_code`, the region and account constants) is the one
//! `application_lifecycle.rs` defines.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_mgn_applicationmigrationservice::state::{
    source_server_arn, AggregatedStatus, Application, SourceServer,
};

use crate::application_lifecycle::{error_code, Mgn, EMULATOR_ACCOUNT, OTHER_ACCOUNT, REGION};

/// The modelled `TagKey` / `TagValue` maximum length
/// (`smithy_query(query="shape",shape="TagsMap")`: key and value `@length` 0..256).
const MAX_TAG_TEXT_LEN: usize = 256;

/// The most tags one resource may carry, spelled out from the SPEC rather than read
/// from the implementation: `TagResource`'s documentation ("Each resource can have a
/// maximum of 50 tags") and the model's `TagsMap` `@length` 0..50
/// (`smithy_query(query="shape",shape="TagsMap")`).
const MAX_TAGS: usize = 50;

/// The message Application Migration Service words a not-found refusal of a resource
/// addressed by ARN with.
const NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// `value` as ONE URI path segment or query value.
///
/// `resourceArn` is bound to a NON-GREEDY `@httpLabel`, so the two characters an ARN
/// carries that a URI reads as structure — the field separator `:` and the resource
/// separator `/` — travel escaped, exactly as an SDK sends them. Every other character
/// the values below use is unreserved, so nothing else needs escaping here.
pub(crate) fn label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// `GET /tags/{resourceArn}` — `ListTagsForResource`.
pub(crate) async fn list_tags(mgn: &Mgn, resource_arn: &str) -> (u16, Value) {
    let (status, _, body) = mgn
        .call_http("GET", &format!("/tags/{}", label(resource_arn)), None)
        .await;
    (status, body)
}

/// `POST /tags/{resourceArn}` with the pairs in the body — `TagResource`.
pub(crate) async fn tag_resource(
    mgn: &Mgn,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    let (status, _, body) = mgn
        .call_http(
            "POST",
            &format!("/tags/{}", label(resource_arn)),
            Some(&json!({"tags": tags})),
        )
        .await;
    (status, body)
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`. `tagKeys` is an
/// `@httpQuery` list, so one parameter per key.
pub(crate) async fn untag_resource(
    mgn: &Mgn,
    resource_arn: &str,
    tag_keys: &[String],
) -> (u16, Value) {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", label(key)))
        .collect::<Vec<_>>()
        .join("&");
    let (status, _, body) = mgn
        .call_http(
            "DELETE",
            &format!("/tags/{}?{query}", label(resource_arn)),
            None,
        )
        .await;
    (status, body)
}

/// The `tags` member of `body` as a map; an absent member is NO tags.
fn tags_of(body: &Value) -> BTreeMap<String, String> {
    match &body["tags"] {
        Value::Null => BTreeMap::new(),
        Value::Object(pairs) => pairs
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("tag {key:?} is not a string in {body:?}"))
                        .to_string(),
                )
            })
            .collect(),
        other => panic!("tags is neither absent nor an object: {other:?}"),
    }
}

/// The tags `ListTagsForResource` reports for `resource_arn`, asserting the 200.
async fn reported_tags(mgn: &Mgn, case: &str, resource_arn: &str) -> BTreeMap<String, String> {
    let (status, body) = list_tags(mgn, resource_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {status} with {body:?}"
    );
    assert!(
        body["tags"].is_object(),
        "case {case:?}: a resource that exists must report a tags MAP (empty when it \
         carries none), got {body:?}"
    );
    tags_of(&body)
}

/// A tag map from `(key, value)` pairs.
pub(crate) fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Which kind of resource a row addresses — the three kinds this service mints an ARN
/// for, so the ARN a tag operation names can resolve to any of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Application,
    SourceServer,
    NetworkMigrationDefinition,
}

/// One taggable resource: its kind, its ARN and the id its own reader takes.
#[derive(Debug, Clone)]
pub(crate) struct Resource {
    pub(crate) kind: Kind,
    pub(crate) arn: String,
    pub(crate) id: String,
}

/// An emulator holding one resource of every taggable kind, each created (or, for a
/// source server, registered) the way this service's own operations do.
pub(crate) async fn emulator_with_one_of_each_kind() -> (Mgn, Vec<Resource>) {
    let mgn = Mgn::new();
    mgn.initialize().await;

    // An application created WITH tags, so a row starts from a non-empty set.
    let (status, application) = mgn
        .call(
            "CreateApplication",
            &json!({"name": "billing-app", "tags": {"env": "dev"}}),
        )
        .await;
    assert_eq!(
        status, 201,
        "CreateApplication expected 201, got {status} with {application:?}"
    );

    // A definition created with no tags, so another row starts from an empty set.
    let (status, definition) = mgn
        .call(
            "network-migration/CreateNetworkMigrationDefinition",
            &json!({
                "name": "isolated-vpc-plan",
                "targetS3Configuration": {
                    "s3Bucket": "migration-artifacts",
                    "s3BucketOwner": EMULATOR_ACCOUNT,
                },
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        )
        .await;
    assert_eq!(
        status, 201,
        "CreateNetworkMigrationDefinition expected 201, got {status} with {definition:?}"
    );

    // A source server has no create operation: an agent installation registers it, so
    // the record goes into the store the way the discovery suite seeds one.
    let server_id = "s-0123456789abcdefg".to_string();
    let server_arn = source_server_arn(REGION, &server_id);
    let stored = mgn.store().lock().insert_source_server(SourceServer {
        source_server_id: server_id.clone(),
        arn: server_arn.clone(),
        account_id: EMULATOR_ACCOUNT.to_string(),
        tags: tags(&[("team", "migration")]),
        ..SourceServer::default()
    });
    assert!(stored, "seeding the source server must store the record");

    let resources = vec![
        Resource {
            kind: Kind::Application,
            arn: application["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("no arn in {application:?}"))
                .to_string(),
            id: application["applicationID"]
                .as_str()
                .unwrap_or_else(|| panic!("no applicationID in {application:?}"))
                .to_string(),
        },
        Resource {
            kind: Kind::SourceServer,
            arn: server_arn,
            id: server_id,
        },
        Resource {
            kind: Kind::NetworkMigrationDefinition,
            arn: definition["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("no arn in {definition:?}"))
                .to_string(),
            id: definition["networkMigrationDefinitionID"]
                .as_str()
                .unwrap_or_else(|| panic!("no networkMigrationDefinitionID in {definition:?}"))
                .to_string(),
        },
    ];
    (mgn, resources)
}

/// The tags the resource's OWN reader reports for it: `ListApplications`,
/// `DescribeSourceServers` and `GetNetworkMigrationDefinition` each carry a `tags`
/// member, so a tag write must be visible through them too.
async fn tags_through_the_resource_reader(
    mgn: &Mgn,
    case: &str,
    resource: &Resource,
) -> BTreeMap<String, String> {
    let (operation, request) = match resource.kind {
        Kind::Application => ("ListApplications", json!({})),
        Kind::SourceServer => ("DescribeSourceServers", json!({})),
        Kind::NetworkMigrationDefinition => (
            "network-migration/GetNetworkMigrationDefinition",
            json!({"networkMigrationDefinitionID": resource.id}),
        ),
    };
    let (status, body) = mgn.call(operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} expected 200, got {status} with {body:?}"
    );
    let record = match resource.kind {
        Kind::NetworkMigrationDefinition => body,
        _ => body["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no items array in {body:?}"))
            .iter()
            .find(|item| item["arn"].as_str() == Some(resource.arn.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: {operation} reported no record for {:?}", resource.arn)
            })
            .clone(),
    };
    tags_of(&record)
}

/// The class a tag WRITE spans: the pair count (none, one, several), a value at the
/// modelled minimum length, a key and value at the modelled maximum, and values
/// carrying spaces and punctuation. The bounds are the model's
/// (`smithy_query(query="shape",shape="TagsMap")`: `@length` 0..50 entries, key and
/// value `@length` 0..256).
pub(crate) fn write_cases() -> Vec<(&'static str, BTreeMap<String, String>)> {
    vec![
        ("one pair", tags(&[("stage", "test")])),
        (
            "several pairs",
            tags(&[
                ("stage", "prod"),
                ("team", "network"),
                ("cost-center", "4711"),
            ]),
        ),
        (
            "an empty value — the modelled TagValue length minimum of 0",
            tags(&[("owner", "")]),
        ),
        (
            "a key and value at the modelled maximum length",
            BTreeMap::from([(
                "k".repeat(MAX_TAG_TEXT_LEN),
                "v".repeat(MAX_TAG_TEXT_LEN),
            )]),
        ),
        (
            "spaces and punctuation in the value",
            tags(&[("note", "wave 1, phase 2 (pilot)")]),
        ),
        (
            "no pairs — the modelled TagsMap minimum of 0 entries",
            BTreeMap::new(),
        ),
    ]
}

/// A tag write stores its pairs on the resource its ARN names, a read reports exactly
/// the stored set — through the tag reader and through the resource's own reader — a
/// second write overwrites only the keys it names, and a removal deletes only the keys
/// it names.
#[tokio::test]
async fn tags_written_by_arn_round_trip_through_both_readers() {
    for (write_case, write) in write_cases() {
        let (mgn, resources) = emulator_with_one_of_each_kind().await;
        for resource in &resources {
            let case = format!("{:?} / {write_case}", resource.kind);
            let before = reported_tags(&mgn, &case, &resource.arn).await;

            let (status, body) = tag_resource(&mgn, &resource.arn, &write).await;
            assert_eq!(
                status, 204,
                "case {case:?}: TagResource expected 204, got {status} with {body:?}"
            );

            let mut expected = before.clone();
            expected.extend(write.clone());
            let reported = reported_tags(&mgn, &case, &resource.arn).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: ListTagsForResource must report the merged set \
                 {expected:?}, got {reported:?}"
            );
            let through_reader = tags_through_the_resource_reader(&mgn, &case, resource).await;
            assert_eq!(
                through_reader, expected,
                "case {case:?}: the resource's own reader must report the same tags \
                 {expected:?}, got {through_reader:?}"
            );

            // A second write: every key of the first, with a different value, plus one
            // key neither the create nor the first write carried. Only those keys may
            // move.
            let mut overwrite: BTreeMap<String, String> = write
                .keys()
                .map(|key| (key.clone(), "rewritten".to_string()))
                .collect();
            overwrite.insert("added-later".to_string(), "yes".to_string());
            let (status, body) = tag_resource(&mgn, &resource.arn, &overwrite).await;
            assert_eq!(
                status, 204,
                "case {case:?}: the second TagResource expected 204, got {status} with \
                 {body:?}"
            );
            expected.extend(overwrite.clone());
            let reported = reported_tags(&mgn, &case, &resource.arn).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: a second write must overwrite only the keys it names, \
                 leaving {expected:?}, got {reported:?}"
            );

            // A removal of one stored key and one key the resource never carried: the
            // first goes, the second is a no-op, and a repeat answers the same.
            let removed = expected
                .keys()
                .next()
                .cloned()
                .expect("the second write left at least one pair");
            let keys = vec![removed.clone(), "never-set".to_string()];
            for attempt in 1..=2 {
                let (status, body) = untag_resource(&mgn, &resource.arn, &keys).await;
                assert_eq!(
                    status, 204,
                    "case {case:?}: UntagResource attempt {attempt} expected 204, got \
                     {status} with {body:?}"
                );
            }
            expected.remove(&removed);
            let reported = reported_tags(&mgn, &case, &resource.arn).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: after removing {removed:?} twice the resource must carry \
                 {expected:?}, got {reported:?}"
            );
            let through_reader = tags_through_the_resource_reader(&mgn, &case, resource).await;
            assert_eq!(
                through_reader, expected,
                "case {case:?}: the resource's own reader must report the removal too: \
                 {expected:?}, got {through_reader:?}"
            );
        }
    }
}

/// A tag write moves the tags of the resource its ARN names and of NO other resource,
/// whatever kind the others are.
#[tokio::test]
async fn a_tag_write_touches_only_the_resource_its_arn_names() {
    let (mgn, resources) = emulator_with_one_of_each_kind().await;
    for target in &resources {
        let case = format!("writing to {:?}", target.kind);
        let mut before = BTreeMap::new();
        for resource in &resources {
            before.insert(
                resource.arn.clone(),
                reported_tags(&mgn, &case, &resource.arn).await,
            );
        }

        let write = tags(&[("touched", &format!("{:?}", target.kind))]);
        let (status, body) = tag_resource(&mgn, &target.arn, &write).await;
        assert_eq!(
            status, 204,
            "case {case:?}: TagResource expected 204, got {status} with {body:?}"
        );

        for resource in &resources {
            let mut expected = before[&resource.arn].clone();
            if resource.arn == target.arn {
                expected.extend(write.clone());
            }
            let reported = reported_tags(&mgn, &case, &resource.arn).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: {:?} must carry {expected:?}, got {reported:?}",
                resource.kind
            );
        }
    }
}

/// Every form of `resourceArn` that names no resource of the calling account, each
/// refused 404 `ResourceNotFoundException` by all three tag operations — and a refused
/// write stores nothing.
#[tokio::test]
async fn an_arn_naming_no_resource_of_the_account_is_refused_not_found() {
    let (mgn, resources) = emulator_with_one_of_each_kind().await;
    let present = resources
        .iter()
        .find(|resource| resource.kind == Kind::Application)
        .expect("the fixture holds an application");

    // An application of ANOTHER account, stored under its own account's ARN: an ARN
    // this caller's account holds no resource under.
    let other_id = "app-00000000000000002".to_string();
    let other_arn = format!("arn:aws:mgn:{REGION}:{OTHER_ACCOUNT}:application/{other_id}");
    let now = "2026-10-29T22:44:56.000000000Z".to_string();
    let stored = mgn.store().lock().insert_application(Application {
        application_id: other_id,
        arn: other_arn.clone(),
        name: "another-account-app".to_string(),
        aggregated_status: AggregatedStatus {
            last_update_date_time: now.clone(),
            ..AggregatedStatus::default()
        },
        creation_date_time: now.clone(),
        last_modified_date_time: now,
        account_id: OTHER_ACCOUNT.to_string(),
        ..Application::default()
    });
    assert!(stored, "seeding the other account's application must store it");

    let absent = vec![
        (
            "an application ARN nothing created",
            format!("arn:aws:mgn:{REGION}:{EMULATOR_ACCOUNT}:application/app-00000000000000000"),
        ),
        (
            "a source-server ARN nothing registered",
            format!("arn:aws:mgn:{REGION}:{EMULATOR_ACCOUNT}:source-server/s-00000000000000000"),
        ),
        (
            "a network-migration-definition ARN nothing created",
            format!(
                "arn:aws:mgn:{REGION}:{EMULATOR_ACCOUNT}:network-migration-definition/\
                 nmd-00000000000000000"
            ),
        ),
        (
            "the ARN of a resource that exists, in another Region",
            present.arn.replace(REGION, "eu-central-1"),
        ),
        (
            "an ARN of another vendor",
            "arn:aws:s3:::migration-artifacts-bucket".to_string(),
        ),
        (
            "a value that is not an ARN at all",
            "not-an-arn-but-long-enough".to_string(),
        ),
        ("another account's resource", other_arn),
    ];

    for (case, resource_arn) in absent {
        let write = tags(&[("stage", "test")]);
        let answers = vec![
            ("TagResource", tag_resource(&mgn, &resource_arn, &write).await),
            ("ListTagsForResource", list_tags(&mgn, &resource_arn).await),
            (
                "UntagResource",
                untag_resource(&mgn, &resource_arn, &["stage".to_string()]).await,
            ),
        ];
        for (operation, (status, body)) in answers {
            assert_eq!(
                status, 404,
                "case {case:?}: {operation} expected 404, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: {operation} must refuse with the modelled not-found \
                 error, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(NOT_FOUND_MESSAGE),
                "case {case:?}: {operation} must word the refusal {NOT_FOUND_MESSAGE:?}, \
                 got {body:?}"
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(resource_arn.as_str()),
                "case {case:?}: {operation} must report the submitted ARN, got {body:?}"
            );
        }
    }

    // The refused writes left the resource that DOES exist untouched.
    let reported = reported_tags(&mgn, "the present application", &present.arn).await;
    assert_eq!(
        reported,
        tags(&[("env", "dev")]),
        "a refused tag write must store nothing: the present application must still \
         carry only its creation tags, got {reported:?}"
    );
}

/// The documented per-resource maximum: a write that would leave the resource at the
/// bound is accepted, one past it is refused with the modelled `ValidationException`
/// and stores nothing, and a write that only overwrites keys the resource already
/// carries is accepted at the bound.
#[tokio::test]
async fn a_tag_write_past_the_per_resource_maximum_is_refused() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    // One short of the bound, so the next single pair lands exactly on it.
    let created: BTreeMap<String, String> = (0..MAX_TAGS - 1)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let (status, application) = mgn
        .call(
            "CreateApplication",
            &json!({"name": "nearly-full", "tags": created}),
        )
        .await;
    assert_eq!(
        status, 201,
        "CreateApplication expected 201, got {status} with {application:?}"
    );
    let arn = application["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("no arn in {application:?}"))
        .to_string();

    let case = "the pair that reaches the bound";
    let last = BTreeMap::from([(
        format!("key-{:02}", MAX_TAGS - 1),
        format!("value-{:02}", MAX_TAGS - 1),
    )]);
    let (status, body) = tag_resource(&mgn, &arn, &last).await;
    assert_eq!(
        status, 204,
        "case {case:?}: a write reaching the bound expected 204, got {status} with {body:?}"
    );
    let mut at_the_bound = created.clone();
    at_the_bound.extend(last);
    let reported = reported_tags(&mgn, case, &arn).await;
    assert_eq!(
        reported.len(),
        MAX_TAGS,
        "case {case:?}: the resource must carry exactly {MAX_TAGS} tags, got {reported:?}"
    );

    let case = "one pair past the bound";
    let past = BTreeMap::from([(
        format!("key-{MAX_TAGS:02}"),
        format!("value-{MAX_TAGS:02}"),
    )]);
    let (status, body) = tag_resource(&mgn, &arn, &past).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: must refuse with the modelled ValidationException, got {body:?}"
    );
    assert_eq!(
        body["fieldList"][0]["name"].as_str(),
        Some("tags"),
        "case {case:?}: the refusal must name the member the pairs arrived in, got {body:?}"
    );
    let reported = reported_tags(&mgn, case, &arn).await;
    assert_eq!(
        reported, at_the_bound,
        "case {case:?}: a refused write must store none of its pairs, got {reported:?}"
    );

    let case = "overwriting a key the resource already carries, at the bound";
    let (status, body) = tag_resource(&mgn, &arn, &tags(&[("key-00", "rewritten")])).await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected 204 — an overwrite grows nothing — got {status} with {body:?}"
    );
    at_the_bound.insert("key-00".to_string(), "rewritten".to_string());
    let reported = reported_tags(&mgn, case, &arn).await;
    assert_eq!(
        reported, at_the_bound,
        "case {case:?}: the resource must carry {at_the_bound:?}, got {reported:?}"
    );
}
