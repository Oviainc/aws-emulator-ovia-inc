// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tag set a CREATE call stores, driven over the wire
//! (restJson1) through the same router the emulator boots.
//!
//! RULE under test: a tag list is a map of key-value pairs, so a create's `tags`
//! member is collapsed onto one entry per key before it is stored — a key the
//! list carries more than once is held ONCE, with the last value that list gave
//! it — and the stored set is therefore the same whichever call carried the
//! list, a create or `TagResource`. `TagList` carries no `@uniqueItems`
//! (`smithy_query(query="shape", shape="TagList")`), so a request may
//! legitimately repeat a key and every create has to answer for it.
//!
//! Each test runs a TABLE of tag lists over every resource kind a create call
//! can tag: app bundle, app authorization, ingestion and ingestion destination.
//!
//! The harness (the router, `Answer`, `encode`) is the one
//! `app_bundle_ingestion_lifecycle` builds, and the tag helpers (`tag_list`,
//! `expected`, `reported_tags`, `tag_resource`, `list_tags`) are
//! `resource_tags`'.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{encode, fixture_region, Answer, Emulator};
use crate::resource_tags::{expected, list_tags, reported_tags, tag_list, tag_resource, Tags};

/// One row of the table: the case, the tag list the create request carries, and
/// the tag set the resource must then hold.
type RepeatCase<'a> = (&'a str, Tags<'a>, Tags<'a>);

/// The table every test below runs: tag lists that differ in how a key repeats
/// (not at all, twice, three times, among other keys, to the empty value, with
/// the same value, and at the published key length), plus the empty list.
///
/// The key and value lengths are the published bounds of `Tag$key` (1..128) and
/// `Tag$value` (0..256), from `smithy_query(query="shape", shape="Tag")`.
fn repeat_cases<'a>(longest_key: &'a str, longest_value: &'a str) -> Vec<RepeatCase<'a>> {
    vec![
        ("an empty tag list", vec![], vec![]),
        (
            "distinct keys, no repeat",
            vec![("env", "prod"), ("team", "data")],
            vec![("env", "prod"), ("team", "data")],
        ),
        (
            "one key carried twice",
            vec![("env", "prod"), ("env", "staging")],
            vec![("env", "staging")],
        ),
        (
            "one key carried three times",
            vec![("env", "a"), ("env", "b"), ("env", "c")],
            vec![("env", "c")],
        ),
        (
            "a repeat among distinct keys",
            vec![("env", "prod"), ("team", "data"), ("env", "dev")],
            vec![("env", "dev"), ("team", "data")],
        ),
        (
            "two keys each carried twice",
            vec![
                ("env", "prod"),
                ("team", "data"),
                ("env", "dev"),
                ("team", "ops"),
            ],
            vec![("env", "dev"), ("team", "ops")],
        ),
        (
            "the same key and value twice",
            vec![("env", "prod"), ("env", "prod")],
            vec![("env", "prod")],
        ),
        (
            "a repeat whose last value is the empty one",
            vec![("marker", "set"), ("marker", "")],
            vec![("marker", "")],
        ),
        (
            "a key repeated at its maximum length, with values at theirs",
            vec![(longest_key, "first"), (longest_key, longest_value)],
            vec![(longest_key, longest_value)],
        ),
        (
            "keys differing only in case are different keys",
            vec![("Env", "prod"), ("env", "dev"), ("Env", "staging")],
            vec![("Env", "staging"), ("env", "dev")],
        ),
    ]
}

/// The app this service's authorizations and ingestions are created for, and the
/// tenant they name: one supported application and one tenant is all these
/// tables need, because the rule under test is about `tags` alone.
const APP: &str = "SLACK";
const TENANT: &str = "tenant-1";

/// The ARN member of a created resource: every create answers with its resource
/// under its own member name, and an app authorization names its ARN
/// `appAuthorizationArn` while the others name theirs `arn`.
fn created_arn(answer: &Answer, member: &str, arn_member: &str, case: &str) -> String {
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected HTTP 201 from the create of {member}, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .body
        .get(member)
        .and_then(|resource| resource.get(arn_member))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected {member}.{arn_member} in {}",
                answer.body
            )
        })
        .to_string()
}

/// Every resource kind a create call can tag, each created in `region` carrying
/// `tags`, as (kind, ARN): the rule holds whichever create received the list.
async fn resources_created_with(
    emulator: &Emulator,
    region: &str,
    tags: &Value,
    case: &str,
) -> Vec<(&'static str, String)> {
    let bundle = created_arn(
        &emulator
            .create_app_bundle_in_region(region, json!({"tags": tags}))
            .await,
        "appBundle",
        "arn",
        case,
    );

    let authorization = created_arn(
        &emulator
            .call_in_region(
                region,
                "POST",
                &format!("/appbundles/{}/appauthorizations", encode(&bundle)),
                Some(json!({
                    "app": APP,
                    "authType": "apiKey",
                    "credential": {"apiKeyCredential": {"apiKey": "placeholder-api-key"}},
                    "tenant": {"tenantIdentifier": TENANT, "tenantDisplayName": TENANT},
                    "tags": tags,
                })),
            )
            .await,
        "appAuthorization",
        "appAuthorizationArn",
        case,
    );
    // AppFabric's chain is authorize → connect → ingest, so the authorization
    // has to complete its handshake before the ingestion below is accepted.
    let connected = emulator
        .call_in_region(
            region,
            "POST",
            &format!(
                "/appbundles/{}/appauthorizations/{}/connect",
                encode(&bundle),
                encode(&authorization)
            ),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        connected.status, 200,
        "case {case:?}: expected HTTP 200 from ConnectAppAuthorization, got {} with body {}",
        connected.status, connected.body
    );

    let ingestion = created_arn(
        &emulator
            .call_in_region(
                region,
                "POST",
                &format!("/appbundles/{}/ingestions", encode(&bundle)),
                Some(json!({
                    "app": APP,
                    "tenantId": TENANT,
                    "ingestionType": "auditLog",
                    "tags": tags,
                })),
            )
            .await,
        "ingestion",
        "arn",
        case,
    );

    let destination = created_arn(
        &emulator
            .call_in_region(
                region,
                "POST",
                &format!(
                    "/appbundles/{}/ingestions/{}/ingestiondestinations",
                    encode(&bundle),
                    encode(&ingestion)
                ),
                Some(json!({
                    "processingConfiguration": {"auditLog": {"schema": "ocsf", "format": "json"}},
                    "destinationConfiguration":
                        {"auditLog": {"destination": {"s3Bucket": {"bucketName": "audit-logs-bucket"}}}},
                    "tags": tags,
                })),
            )
            .await,
        "ingestionDestination",
        "arn",
        case,
    );

    vec![
        ("app bundle", bundle),
        ("app authorization", authorization),
        ("ingestion", ingestion),
        ("ingestion destination", destination),
    ]
}

/// RULE: a create stores its `tags` as the map they are — one entry per key,
/// holding the last value the request's list gave that key — so
/// `ListTagsForResource` never reports a key twice for a resource a create
/// minted, whichever create minted it.
#[tokio::test]
async fn a_create_stores_its_tag_list_as_a_map() {
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);

    for (case, carried, held) in repeat_cases(&longest_key, &longest_value) {
        let emulator = Emulator::new();
        let list = tag_list(&carried);
        for (kind, arn) in resources_created_with(&emulator, &fixture_region(0), &list, case).await {
            let case = format!("{case} on an {kind}");
            let listed = list_tags(&emulator, &arn).await;
            let reported = reported_tags(&listed, &case);
            assert_eq!(
                reported,
                expected(&held),
                "case {case:?}: expected the create's tag list stored as a map, got {}",
                listed.body
            );
            assert_eq!(
                reported.len(),
                held.len(),
                "case {case:?}: expected {} tags held, got {}",
                held.len(),
                reported.len()
            );
        }
    }
}

/// RULE: ONE tag list yields ONE tag set, whichever operation carried it — the
/// set a create stores for a list is exactly the set `TagResource` writes for
/// the same list on a resource created without tags.
#[tokio::test]
async fn a_create_and_tag_resource_store_the_same_set() {
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);

    for (case, carried, _) in repeat_cases(&longest_key, &longest_value) {
        let emulator = Emulator::new();
        let list = tag_list(&carried);
        // The created-with-tags resources, and a second set of resources created
        // with none and tagged afterwards. Each app bundle lives in its own
        // Region, because a Region holds one.
        let created = resources_created_with(&emulator, &fixture_region(0), &list, case).await;
        let tagged = resources_created_with(&emulator, &fixture_region(1), &json!([]), case).await;

        for ((kind, created_arn), (_, tagged_arn)) in created.iter().zip(tagged.iter()) {
            let case = format!("{case} on an {kind}");
            let answer = tag_resource(&emulator, tagged_arn, tag_list(&carried)).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 from TagResource, got {} with body {}",
                answer.status, answer.body
            );
            let by_create = reported_tags(&list_tags(&emulator, created_arn).await, &case);
            let by_tagging = reported_tags(&list_tags(&emulator, tagged_arn).await, &case);
            assert_eq!(
                by_create, by_tagging,
                "case {case:?}: the create path stored {by_create:?} where the tagging path stored {by_tagging:?}"
            );
        }
    }
}

/// RULE: the tag set a create stored is reachable to every later tag write — a
/// key the create's list repeated holds ONE entry, so `TagResource` replaces its
/// value and `UntagResource` removes it, leaving nothing behind.
#[tokio::test]
async fn a_repeated_key_a_create_carried_stays_writable() {
    let emulator = Emulator::new();
    let carried = tag_list(&[("env", "prod"), ("env", "staging"), ("team", "data")]);
    let case = "a key the create repeated";

    for (kind, arn) in resources_created_with(&emulator, &fixture_region(0), &carried, case).await {
        let case = format!("{case} on an {kind}");
        let replaced = tag_resource(&emulator, &arn, tag_list(&[("env", "dev")])).await;
        assert_eq!(
            replaced.status, 200,
            "case {case:?}: expected HTTP 200 from TagResource, got {} with body {}",
            replaced.status, replaced.body
        );
        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            reported_tags(&listed, &case),
            expected(&[("env", "dev"), ("team", "data")]),
            "case {case:?}: a tag write must replace every copy of the key, got {}",
            listed.body
        );

        let removed = emulator
            .call(
                "DELETE",
                &format!("/tags/{}?tagKeys={}", encode(&arn), encode("env")),
                None,
            )
            .await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected HTTP 200 from UntagResource, got {} with body {}",
            removed.status, removed.body
        );
        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            reported_tags(&listed, &case),
            expected(&[("team", "data")]),
            "case {case:?}: the removed key must leave no copy behind, got {}",
            listed.body
        );
    }
}

/// RULE: an idempotency token is spent on the REQUEST, and a create's tag list
/// is the map it denotes — so a retry carrying the same token and a list that
/// denotes the same map replays the first call's resource (one resource, the
/// merged tag set) rather than being refused as a parameter mismatch.
#[tokio::test]
async fn a_retry_whose_tag_list_denotes_the_same_map_replays() {
    // (case, the first call's list, the retry's list)
    let cases: Vec<(&str, Tags, Tags)> = vec![
        (
            "the identical list",
            vec![("env", "prod")],
            vec![("env", "prod")],
        ),
        (
            "the same pairs in another order",
            vec![("env", "prod"), ("team", "data")],
            vec![("team", "data"), ("env", "prod")],
        ),
        (
            "a repeated key against the value it collapses to",
            vec![("env", "prod"), ("env", "staging")],
            vec![("env", "staging")],
        ),
    ];

    for (case, first_list, retry_list) in cases {
        let emulator = Emulator::new();
        // A token in the form the model's UUID pattern requires.
        let token = "11111111-0000-4000-8000-000000000000";
        let first = emulator
            .create_app_bundle(json!({"clientToken": token, "tags": tag_list(&first_list)}))
            .await;
        let first_arn = created_arn(&first, "appBundle", "arn", case);

        let retry = emulator
            .create_app_bundle(json!({"clientToken": token, "tags": tag_list(&retry_list)}))
            .await;
        let retry_arn = created_arn(&retry, "appBundle", "arn", case);
        assert_eq!(
            retry_arn, first_arn,
            "case {case:?}: the retry must replay the first call's bundle, got {}",
            retry.body
        );

        let listed = list_tags(&emulator, &first_arn).await;
        let reported = reported_tags(&listed, case);
        let collapsed: BTreeMap<String, String> = first_list
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported, collapsed,
            "case {case:?}: expected the first call's tag map to be held, got {}",
            listed.body
        );
    }
}
