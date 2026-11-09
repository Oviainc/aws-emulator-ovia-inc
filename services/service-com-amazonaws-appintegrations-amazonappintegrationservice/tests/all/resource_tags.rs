// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AppIntegrations tagging operations
//! (`TagResource` + `ListTagsForResource` + `UntagResource`).
//!
//! The rule under test: a tag set is part of the record of the resource it is on, so
//! `TagResource` merges the request's keys into that resource's tag set,
//! `UntagResource` removes the keys it names from it, and `ListTagsForResource` reports
//! exactly what is stored there — the same copy the resource's own `Get` reports — while
//! an ARN this account holds no taggable resource under is refused with
//! `ResourceNotFoundException`.
//!
//! The suite is a table: rows differ in the resource type the ARN names (every resource
//! of this service whose create takes `Tags`), in tag count, in key and value form and
//! length (both published bounds of `TagKey` and `TagValue`), and in which keys an untag
//! names (a subset, a key the resource does not carry, all of them). The recorded
//! tag/list/untag round trip is one row.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts (see `harness.rs`); nothing reaches into the service's state struct, so a
//! response that is not computed from persisted state cannot pass.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::harness::{encode_label, Emulator, Reply, REGION};

/// The canonical account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// `TagKey` is `len 1..128`, `TagValue` is `len 0..256`
/// (`smithy_query(query="shape", shape="TagKey,TagValue")`).
const TAG_KEY_MAX: usize = 128;
const TAG_VALUE_MAX: usize = 256;

/// The most tags one resource carries: the maximum the modelled `TagMap` admits
/// (`smithy_query(query="shape", shape="TagMap")`: `len 1..200`).
const MAX_TAGS_PER_RESOURCE: usize = 200;

/// One taggable resource of this service, as the wire reports it: the ARN the tagging
/// operations address it by, and the URI its own reader answers on.
#[derive(Debug, Clone)]
struct Taggable {
    /// What names this resource in an assertion.
    kind: &'static str,
    arn: String,
    /// `GET <get_uri>` reports this resource, `Tags` included.
    get_uri: String,
}

/// A `@httpQuery` list travels one `tagKeys=` pair per key, and a key can carry `+`,
/// `=`, `:` and `/` (`TagKey` pattern `^(?!aws:)[a-zA-Z+-=._:/]+$`), so every key is
/// percent-encoded rather than left for the query parser to read as a separator.
fn tag_keys_query(keys: &[&str]) -> String {
    keys.iter()
        .map(|key| format!("tagKeys={}", urlencoding::encode(key)))
        .collect::<Vec<_>>()
        .join("&")
}

/// `GET /tags/{resourceArn}`.
async fn list_tags(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("/tags/{}", encode_label(arn)),
            REGION,
            None,
        )
        .await
}

/// `POST /tags/{resourceArn}` with the request's `tags`.
async fn tag(emulator: &Emulator, arn: &str, tags: Value) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/tags/{}", encode_label(arn)),
            REGION,
            Some(json!({ "tags": tags })),
        )
        .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=...`.
async fn untag(emulator: &Emulator, arn: &str, keys: &[&str]) -> Reply {
    emulator
        .send(
            "DELETE",
            &format!(
                "/tags/{}?{}",
                encode_label(arn),
                tag_keys_query(keys)
            ),
            REGION,
            None,
        )
        .await
}

/// The `tags`/`Tags` member of `reply`, as a comparable map.
fn tag_map(case: &str, reply: &Reply, member: &str) -> BTreeMap<String, String> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200 reporting {member}, got {} / {}",
        reply.status, reply.body
    );
    let value = reply.field(member);
    assert!(
        value.is_some_and(Value::is_object),
        "case {case:?}: expected a {member} map, got {:?}",
        reply.body
    );
    value
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tag set `ListTagsForResource` reports for `resource`.
async fn reported_tags(
    emulator: &Emulator,
    case: &str,
    resource: &Taggable,
) -> BTreeMap<String, String> {
    let reply = list_tags(emulator, &resource.arn).await;
    tag_map(case, &reply, "tags")
}

/// The tag set the resource's OWN reader reports — the same stored copy, read through a
/// different operation, so a tag written into a second place beside the record would
/// show up as a difference.
async fn tags_from_own_reader(
    emulator: &Emulator,
    case: &str,
    resource: &Taggable,
) -> BTreeMap<String, String> {
    let reply = emulator.send("GET", &resource.get_uri, REGION, None).await;
    tag_map(case, &reply, "Tags")
}

/// Create one application, optionally with `Tags` on the create.
async fn application(emulator: &Emulator, name: &str, tags: Option<Value>) -> Taggable {
    let mut body = json!({
        "Name": name,
        "Namespace": name,
        "ApplicationSourceConfig": {
            "ExternalUrlConfig": { "AccessUrl": "https://example.com/app" }
        },
    });
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    let created = emulator.send("POST", "/applications", REGION, Some(body)).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: the application under test must be created, got {} / {}",
        created.status, created.body
    );
    let arn = created
        .string("Arn")
        .expect("CreateApplication reports the minted Arn");
    Taggable {
        kind: "application",
        get_uri: format!("/applications/{}", encode_label(&arn)),
        arn,
    }
}

/// Create one event integration, optionally with `Tags` on the create.
async fn event_integration(emulator: &Emulator, name: &str, tags: Option<Value>) -> Taggable {
    let mut body = json!({
        "Name": name,
        "EventBridgeBus": "default",
        "EventFilter": { "Source": "aws.partner/demo" },
    });
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    let created = emulator
        .send("POST", "/eventIntegrations", REGION, Some(body))
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: the event integration under test must be created, got {} / {}",
        created.status, created.body
    );
    let arn = created
        .string("EventIntegrationArn")
        .expect("CreateEventIntegration reports the minted ARN");
    Taggable {
        kind: "event integration",
        get_uri: format!("/eventIntegrations/{}", encode_label(name)),
        arn,
    }
}

/// Create one data integration, optionally with `Tags` on the create.
async fn data_integration(emulator: &Emulator, name: &str, tags: Option<Value>) -> Taggable {
    let kms_key = emulator.create_kms_key(REGION).await;
    let mut body = json!({ "Name": name, "KmsKey": kms_key });
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    let created = emulator
        .send("POST", "/dataIntegrations", REGION, Some(body))
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: the data integration under test must be created, got {} / {}",
        created.status, created.body
    );
    let arn = created
        .string("Arn")
        .expect("CreateDataIntegration reports the minted Arn");
    let id = created
        .string("Id")
        .expect("CreateDataIntegration reports the minted Id");
    Taggable {
        kind: "data integration",
        get_uri: format!("/dataIntegrations/{}", encode_label(&id)),
        arn,
    }
}

/// Every resource type of this service whose create takes `Tags`
/// (`smithy_query(query="operation",
/// operation="CreateApplication,CreateEventIntegration,CreateDataIntegration")`), each
/// created untagged under a name derived from `row`.
async fn taggable_resources(emulator: &Emulator, row: usize) -> Vec<Taggable> {
    vec![
        application(emulator, &format!("tagged.application-{row}"), None).await,
        event_integration(emulator, &format!("tagged.event-integration-{row}"), None).await,
        data_integration(emulator, &format!("tagged.data-integration-{row}"), None).await,
    ]
}

/// One tag set a request can carry: the label that names it in an assertion, and the
/// entries themselves.
fn tag_cases() -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        // The shape the recorded round trip carries: one row of many.
        (
            "one tag",
            vec![("owner".to_string(), "team-a".to_string())],
        ),
        (
            "several tags",
            vec![
                ("owner".to_string(), "team-a".to_string()),
                ("env".to_string(), "prod".to_string()),
                ("cost-center".to_string(), "4711".to_string()),
            ],
        ),
        (
            // `TagValue` is len 0..256, so the empty value is a legal value and not an
            // absent tag.
            "an empty value",
            vec![("retired".to_string(), String::new())],
        ),
        (
            // Both published maxima: `TagKey` len 128, `TagValue` len 256.
            "key and value at their maximum length",
            vec![("k".repeat(TAG_KEY_MAX), "v".repeat(TAG_VALUE_MAX))],
        ),
        (
            // Every punctuation class `TagKey`'s pattern admits, plus digits, which the
            // pattern's `+-=` range covers.
            "punctuation and digits in the key",
            vec![(
                "a+b-c=d.e_f:g/h0".to_string(),
                "unicode value: ünïcödé".to_string(),
            )],
        ),
    ]
}

/// The entries as the JSON a request carries.
fn tags_body(entries: &[(String, String)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// The entries as the map a response must report.
fn expected_map(entries: &[(String, String)]) -> BTreeMap<String, String> {
    entries.iter().cloned().collect()
}

#[tokio::test]
async fn the_tag_set_tag_resource_wrote_is_what_every_reader_reports() {
    let emulator = Emulator::new();

    for (row, (label, entries)) in tag_cases().into_iter().enumerate() {
        for resource in taggable_resources(&emulator, row).await {
            let case = format!("{label} on a {}", resource.kind);

            // An untagged resource reports the EMPTY tag set, never an absent member.
            assert_eq!(
                reported_tags(&emulator, &case, &resource).await,
                BTreeMap::new(),
                "case {case:?}: an untagged resource must report an empty tag set"
            );

            let tagged = tag(&emulator, &resource.arn, tags_body(&entries)).await;
            assert_eq!(
                tagged.status, 200,
                "case {case:?}: expected TagResource to answer HTTP 200, got {} / {}",
                tagged.status, tagged.body
            );

            let expected = expected_map(&entries);
            let listed = reported_tags(&emulator, &case, &resource).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: expected ListTagsForResource to report {expected:?}, got {listed:?}"
            );

            // The same stored copy, through the resource's own reader.
            let own = tags_from_own_reader(&emulator, &case, &resource).await;
            assert_eq!(
                own, expected,
                "case {case:?}: expected the resource's own reader to report {expected:?}, \
                 got {own:?}"
            );
        }
    }
}

#[tokio::test]
async fn tagging_merges_keys_and_overwrites_the_value_of_a_key_already_carried() {
    let emulator = Emulator::new();

    // Rows: the tags the create carried, then the tags one `TagResource` adds, then what
    // the resource must carry — a key the resource already has takes the new value, a key
    // it does not have is added, and a key the request does not name is left alone.
    let created_tags = json!({ "owner": "team-a", "env": "dev" });
    for resource in [
        application(&emulator, "merged.application", Some(created_tags.clone())).await,
        event_integration(
            &emulator,
            "merged.event-integration",
            Some(created_tags.clone()),
        )
        .await,
        data_integration(
            &emulator,
            "merged.data-integration",
            Some(created_tags.clone()),
        )
        .await,
    ] {
        let case = format!("merge on a {}", resource.kind);

        // What the create persisted is readable before any TagResource call.
        let listed = reported_tags(&emulator, &case, &resource).await;
        let expected: BTreeMap<String, String> = [("owner", "team-a"), ("env", "dev")]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the create's tags {expected:?}, got {listed:?}"
        );

        let tagged = tag(
            &emulator,
            &resource.arn,
            json!({ "env": "prod", "tier": "gold" }),
        )
        .await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected TagResource to answer HTTP 200, got {} / {}",
            tagged.status, tagged.body
        );

        let expected: BTreeMap<String, String> =
            [("owner", "team-a"), ("env", "prod"), ("tier", "gold")]
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
        let listed = reported_tags(&emulator, &case, &resource).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?} after the merge, got {listed:?}"
        );

        // Tagging twice with the same request leaves one entry per key.
        tag(
            &emulator,
            &resource.arn,
            json!({ "env": "prod", "tier": "gold" }),
        )
        .await;
        let listed = reported_tags(&emulator, &case, &resource).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected a repeated TagResource to change nothing, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn untagging_removes_only_the_keys_it_names() {
    let emulator = Emulator::new();

    // Rows differ in which keys the untag names: one of them, several, a key the
    // resource does not carry, a mix of carried and absent, and every key at once.
    let carried = [("owner", "team-a"), ("env", "prod"), ("tier", "gold")];
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("one carried key", vec!["env"], vec!["owner", "tier"]),
        (
            "several carried keys",
            vec!["env", "tier"],
            vec!["owner"],
        ),
        (
            "a key the resource does not carry",
            vec!["absent"],
            vec!["env", "owner", "tier"],
        ),
        (
            "a carried key beside an absent one",
            vec!["owner", "absent"],
            vec!["env", "tier"],
        ),
        ("every key at once", vec!["owner", "env", "tier"], vec![]),
    ];

    for (row, (label, keys, remaining)) in rows.into_iter().enumerate() {
        let tags = Value::Object(
            carried
                .iter()
                .map(|(key, value)| (key.to_string(), Value::String(value.to_string())))
                .collect(),
        );
        for resource in [
            application(
                &emulator,
                &format!("untagged.application-{row}"),
                Some(tags.clone()),
            )
            .await,
            event_integration(
                &emulator,
                &format!("untagged.event-integration-{row}"),
                Some(tags.clone()),
            )
            .await,
            data_integration(
                &emulator,
                &format!("untagged.data-integration-{row}"),
                Some(tags.clone()),
            )
            .await,
        ] {
            let case = format!("{label} on a {}", resource.kind);
            let expected: BTreeMap<String, String> = remaining
                .iter()
                .map(|key| {
                    let value = carried
                        .iter()
                        .find(|(carried_key, _)| carried_key == key)
                        .map(|(_, value)| value.to_string())
                        .unwrap_or_default();
                    (key.to_string(), value)
                })
                .collect();

            let untagged = untag(&emulator, &resource.arn, &keys).await;
            assert_eq!(
                untagged.status, 200,
                "case {case:?}: expected UntagResource to answer HTTP 200, got {} / {}",
                untagged.status, untagged.body
            );

            let listed = reported_tags(&emulator, &case, &resource).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: expected {expected:?} after removing {keys:?}, got {listed:?}"
            );

            // Removing the same keys again removes nothing more.
            untag(&emulator, &resource.arn, &keys).await;
            let listed = reported_tags(&emulator, &case, &resource).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: expected a repeated UntagResource to change nothing, \
                 got {listed:?}"
            );

            // And the resource's own reader reports the same remainder.
            let own = tags_from_own_reader(&emulator, &case, &resource).await;
            assert_eq!(
                own, expected,
                "case {case:?}: expected the resource's own reader to report {expected:?}, \
                 got {own:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_tag_set_belongs_to_one_resource_only() {
    let emulator = Emulator::new();

    // Three resources of three types, stored side by side: tagging one must leave the
    // others exactly as they were.
    let resources = taggable_resources(&emulator, 0).await;
    let subject = resources[0].clone();
    tag(&emulator, &subject.arn, json!({ "owner": "team-a" })).await;

    for resource in &resources[1..] {
        let case = format!("neighbour {} of a tagged application", resource.kind);
        let listed = reported_tags(&emulator, &case, resource).await;
        assert_eq!(
            listed,
            BTreeMap::new(),
            "case {case:?}: expected an untouched neighbour to report no tags, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_no_taggable_resource_carries_is_reported_as_not_found() {
    let emulator = Emulator::new();
    // Stored neighbours of each type, so a 404 is about the ARN the request named and
    // not about an empty store.
    let stored = taggable_resources(&emulator, 0).await;
    assert_eq!(
        stored.len(),
        3,
        "the neighbours of every taggable type must be stored, got {}",
        stored.len()
    );

    // Rows: an ARN of each resource type this service mints one for, carrying an id
    // nothing holds; an ARN of a resource type of this service that is not taggable; an
    // ARN of the right type in another Region and in another account; and an ARN of
    // another vendor entirely. The expected resource word is the type the ARN claims.
    let absent_id = "00000000-0000-4000-8000-000000000000";
    let rows: Vec<(String, &str)> = vec![
        (
            format!("arn:aws:app-integrations:{REGION}:{ACCOUNT}:application/{absent_id}"),
            "Application",
        ),
        (
            format!("arn:aws:app-integrations:{REGION}:{ACCOUNT}:event-integration/absent"),
            "EventIntegration",
        ),
        (
            format!("arn:aws:app-integrations:{REGION}:{ACCOUNT}:data-integration/{absent_id}"),
            "DataIntegration",
        ),
        (
            format!(
                "arn:aws:app-integrations:{REGION}:{ACCOUNT}:data-integration-association/\
                 {absent_id}/{absent_id}"
            ),
            "DataIntegrationAssociation",
        ),
        (
            format!(
                "arn:aws:app-integrations:{REGION}:{ACCOUNT}:application-association/\
                 {absent_id}/{absent_id}"
            ),
            "ApplicationAssociation",
        ),
        (
            // The right resource type in another Region: an ARN of this service that
            // names nothing this store holds.
            format!("arn:aws:app-integrations:eu-west-1:{ACCOUNT}:application/{absent_id}"),
            "Application",
        ),
        (
            // And in another account.
            format!("arn:aws:app-integrations:{REGION}:000000000000:application/{absent_id}"),
            "Application",
        ),
        (
            // Another vendor's ARN claims no resource type of this service.
            "arn:aws:s3:::some-bucket".to_string(),
            "Resource",
        ),
    ];

    for (arn, resource_type) in rows {
        let expected = format!("Could not find {resource_type} with ARN: {arn}.");
        for (operation, reply) in [
            (
                "ListTagsForResource",
                list_tags(&emulator, &arn).await,
            ),
            (
                "TagResource",
                tag(&emulator, &arn, json!({ "owner": "team-a" })).await,
            ),
            ("UntagResource", untag(&emulator, &arn, &["owner"]).await),
        ] {
            let case = format!("{operation} on {arn:?}");
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected HTTP 404, got {} / {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                reply.error_type
            );
            assert_eq!(
                reply.message().as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {:?}",
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn a_resource_cannot_be_tagged_past_the_tag_maps_maximum() {
    let emulator = Emulator::new();
    let resource = application(&emulator, "capped.application", None).await;

    // `TagMap` is len 1..200 and is the shape a tag set is both written and reported as
    // (`smithy_query(query="shape", shape="TagMap")`), so a resource carries at most
    // 200 tags. Rows: exactly the maximum, a repeat of keys already carried at the
    // maximum (no growth, so still fine), and one key past it.
    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let at_maximum = tag(&emulator, &resource.arn, tags_body(&full)).await;
    assert_eq!(
        at_maximum.status, 200,
        "case \"exactly the maximum\": expected HTTP 200, got {} / {}",
        at_maximum.status, at_maximum.body
    );
    let listed = reported_tags(&emulator, "exactly the maximum", &resource).await;
    assert_eq!(
        listed.len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"exactly the maximum\": expected {MAX_TAGS_PER_RESOURCE} tags, got {}",
        listed.len()
    );

    let repeated = tag(
        &emulator,
        &resource.arn,
        tags_body(&[("key-0".to_string(), "replaced".to_string())]),
    )
    .await;
    assert_eq!(
        repeated.status, 200,
        "case \"a key already carried, at the maximum\": expected HTTP 200, got {} / {}",
        repeated.status, repeated.body
    );

    let past = tag(
        &emulator,
        &resource.arn,
        tags_body(&[("one-too-many".to_string(), "value".to_string())]),
    )
    .await;
    assert_eq!(
        past.status, 400,
        "case \"one key past the maximum\": expected HTTP 400, got {} / {}",
        past.status, past.body
    );
    assert_eq!(
        past.error_type, "InvalidRequestException",
        "case \"one key past the maximum\": expected InvalidRequestException, got {:?}",
        past.error_type
    );
    let listed = reported_tags(&emulator, "one key past the maximum", &resource).await;
    assert_eq!(
        listed.len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"one key past the maximum\": expected the refused tag to be stored nowhere, \
         got {} tags",
        listed.len()
    );
}

#[tokio::test]
async fn tags_survive_a_read_and_a_delete_removes_them_with_the_resource() {
    let emulator = Emulator::new();

    // A read must not change what a later caller sees: listing the tags twice, with the
    // resource's own reader in between, reports the same set every time.
    let resource = application(&emulator, "stable.application", None).await;
    tag(&emulator, &resource.arn, json!({ "owner": "team-a" })).await;
    let expected: BTreeMap<String, String> =
        [("owner".to_string(), "team-a".to_string())].into_iter().collect();
    for round in 0..3 {
        let case = format!("read {round} of a tagged application");
        let listed = reported_tags(&emulator, &case, &resource).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected a read to report {expected:?} unchanged, got {listed:?}"
        );
        let own = tags_from_own_reader(&emulator, &case, &resource).await;
        assert_eq!(
            own, expected,
            "case {case:?}: expected the resource's own reader to report {expected:?}, got {own:?}"
        );
    }

    // The tag set is part of the resource's record, so the delete that removes the
    // resource takes it with it: the ARN then names nothing to tag.
    let deleted = emulator
        .send(
            "DELETE",
            &format!("/applications/{}", encode_label(&resource.arn)),
            REGION,
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "the application under test must be deleted, got {} / {}",
        deleted.status, deleted.body
    );
    let listed = list_tags(&emulator, &resource.arn).await;
    assert_eq!(
        listed.status, 404,
        "case \"ListTagsForResource after the delete\": expected HTTP 404, got {} / {}",
        listed.status, listed.body
    );
}
