// SPDX-License-Identifier: CC0-1.0

//! What a suite-definition read SERVES, and what a tag mutation ANSWERS, over the wire.
//!
//! Every case drives real HTTP requests through the same `MultiServiceRouter` the emulator
//! serves on a socket (the harness `suite_lifecycle` owns), so what is asserted is the
//! service's wire behaviour.
//!
//! The rules under test, each asserted over a TABLE rather than over the one recorded
//! request:
//!
//! 1. A suite definition is stored RESOLVED, so a read answers every member of its
//!    configuration with the value the suite will actually run with: the optional list as an
//!    empty list, the optional booleans as false, the protocol as the service's default, and
//!    the `rootGroup` of a qualification suite that named an empty one as the default
//!    qualification root group — never as the empty string the caller sent. The caller's own
//!    values are never overridden.
//! 2. A read that reports a resource's tags always serializes the member: `TagMap` is
//!    `len 0..50`, so an untagged resource reads as an EMPTY map, on every operation that
//!    reports tags and for both taggable resource kinds.
//! 3. An operation whose output declares no members answers with no content: both tag
//!    mutations answer 204, and the write they made is observable through the readers.
//! 4. `UpdateSuiteDefinition` does not serialize `suiteDefinitionArn`; the operations that
//!    report the definition (and the create that minted it) do.

use serde_json::{json, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use super::endpoint::percent_encode;
use super::suite_lifecycle::{definition_rows, thing_arn, Answer, Emulator, ROLE_ARN};

/// The region every case below signs for.
const REGION: &str = "us-east-1";

/// The protocol a configuration that named none reads back with: plain MQTT 3.1.1, the
/// `Protocol` value AWS answers a read of a definition stored without a protocol with.
/// Written out here rather than read from the service, so a wrong resolved default fails.
const DEFAULT_PROTOCOL: &str = "MqttV3_1_1";

/// The status a successful tag mutation answers with: 204 No Content, as AWS answers an
/// empty-output tag mutation. Written out here rather than read from the service, so the
/// status regressing to the protocol default fails these cases.
const TAG_MUTATION_STATUS: u16 = 204;

/// `GetSuiteDefinition`, optionally naming a version.
async fn get_definition(emulator: &Emulator, id: &str, version: Option<&str>) -> Answer {
    let path = match version {
        Some(version) => format!("/suiteDefinitions/{id}?suiteDefinitionVersion={version}"),
        None => format!("/suiteDefinitions/{id}"),
    };
    emulator.call("GET", &path, REGION, None).await
}

/// `UpdateSuiteDefinition`.
async fn update_definition(emulator: &Emulator, id: &str, configuration: &Value) -> Answer {
    emulator
        .call(
            "PATCH",
            &format!("/suiteDefinitions/{id}"),
            REGION,
            Some(json!({ "suiteDefinitionConfiguration": configuration })),
        )
        .await
}

/// `StartSuiteRun`, with the tags the row carries.
async fn start_run(emulator: &Emulator, definition_id: &str, tags: Option<&Value>) -> Answer {
    let mut body = json!({
        "suiteRunConfiguration": { "primaryDevice": { "thingArn": thing_arn(1) } },
    });
    if let Some(tags) = tags {
        body["tags"] = tags.clone();
    }
    emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            REGION,
            Some(body),
        )
        .await
}

/// `ListTagsForResource` for `arn`.
async fn list_tags(emulator: &Emulator, arn: &str) -> Answer {
    emulator
        .call("GET", &format!("/tags/{}", percent_encode(arn)), REGION, None)
        .await
}

/// `TagResource` for `arn`.
async fn tag_resource(emulator: &Emulator, arn: &str, tags: &Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/tags/{}", percent_encode(arn)),
            REGION,
            Some(json!({ "tags": tags })),
        )
        .await
}

/// `UntagResource` for `arn`, with one `tagKeys` query entry per key.
async fn untag_resource(emulator: &Emulator, arn: &str, keys: &[&str]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encode(key)))
        .collect::<Vec<_>>()
        .join("&");
    emulator
        .call(
            "DELETE",
            &format!("/tags/{}?{query}", percent_encode(arn)),
            REGION,
            None,
        )
        .await
}

/// The configurations rule 1 is asserted over: rows differing in WHICH optional members the
/// caller sent (none, each one alone, all of them), in the device-list size (0, 1 and the
/// modelled maximum of 2), in both `rootGroup` forms, and in the protocol named — one row
/// per `Protocol` the model enumerates (source: the `Protocol` enum, re-exported as
/// `PROTOCOL_VALUES`), so a resolved default is never mistaken for a value a row sent.
fn sparse_and_full_rows() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        (
            "no optional member at all".to_string(),
            json!({
                "suiteDefinitionName": "resolve-none",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
        ),
        (
            "devices alone".to_string(),
            json!({
                "suiteDefinitionName": "resolve-devices",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "devices": [{ "thingArn": thing_arn(2) }],
            }),
        ),
        (
            "an empty device list alone".to_string(),
            json!({
                "suiteDefinitionName": "resolve-empty-devices",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "devices": [],
            }),
        ),
        (
            "isLongDurationTest alone, true".to_string(),
            json!({
                "suiteDefinitionName": "resolve-long",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "isLongDurationTest": true,
            }),
        ),
        (
            "isLongDurationTest alone, false".to_string(),
            json!({
                "suiteDefinitionName": "resolve-short",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "isLongDurationTest": false,
            }),
        ),
        (
            "a qualification suite that named an empty rootGroup".to_string(),
            json!({
                "suiteDefinitionName": "resolve-qualification",
                "rootGroup": "",
                "intendedForQualification": true,
                "devicePermissionRoleArn": ROLE_ARN,
            }),
        ),
        (
            "a qualification suite that named its own rootGroup".to_string(),
            json!({
                "suiteDefinitionName": "resolve-qualification-named",
                "rootGroup": "root/group/of/its/own",
                "intendedForQualification": true,
                "devicePermissionRoleArn": ROLE_ARN,
            }),
        ),
        (
            // `DeviceUnderTestList` is `len 0..2`, so two devices is the modelled maximum.
            "every optional member at once".to_string(),
            json!({
                "suiteDefinitionName": "resolve-all",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "devices": [
                    { "thingArn": thing_arn(3) },
                    { "deviceRoleArn": ROLE_ARN },
                ],
                "intendedForQualification": false,
                "isLongDurationTest": true,
                "protocol": "MqttV5",
            }),
        ),
    ];
    for (index, protocol) in iotdeviceadvisor::ops::types::PROTOCOL_VALUES
        .iter()
        .enumerate()
    {
        rows.push((
            format!("the caller named protocol {protocol}"),
            json!({
                "suiteDefinitionName": format!("resolve-protocol-{index}"),
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "protocol": protocol,
            }),
        ));
    }
    rows
}

/// Assert rule 1 of one read: every member of the served configuration is the resolved one.
fn assert_resolved(case: &str, served: &Value, sent: &Value) {
    assert_eq!(
        served["suiteDefinitionName"], sent["suiteDefinitionName"],
        "case {case:?}: the caller's suiteDefinitionName must be served, got {served}"
    );
    assert_eq!(
        served["devicePermissionRoleArn"], sent["devicePermissionRoleArn"],
        "case {case:?}: the caller's devicePermissionRoleArn must be served, got {served}"
    );

    // The optional list: present always, and the caller's own when it sent one.
    let expected_devices = sent.get("devices").cloned().unwrap_or_else(|| json!([]));
    assert_eq!(
        served["devices"], expected_devices,
        "case {case:?}: expected devices {expected_devices}, got {}",
        served["devices"]
    );

    // The optional booleans: present always, false where the caller sent nothing.
    for member in ["intendedForQualification", "isLongDurationTest"] {
        let expected = sent.get(member).cloned().unwrap_or(Value::Bool(false));
        assert_eq!(
            served[member], expected,
            "case {case:?}: expected {member} {expected}, got {} in {served}",
            served[member]
        );
    }

    // The protocol: the caller's, or the service's default.
    let expected_protocol = sent
        .get("protocol")
        .cloned()
        .unwrap_or_else(|| json!(DEFAULT_PROTOCOL));
    assert_eq!(
        served["protocol"], expected_protocol,
        "case {case:?}: expected protocol {expected_protocol}, got {}",
        served["protocol"]
    );

    // The root group: the caller's where it named one, and the default qualification root
    // group — non-empty — where it named an empty one.
    let served_root_group = served["rootGroup"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: rootGroup must be a string, got {served}"));
    assert!(
        !served_root_group.is_empty(),
        "case {case:?}: a stored rootGroup is never empty, got {served}"
    );
    assert!(
        (1..=2048).contains(&served_root_group.len()),
        "case {case:?}: rootGroup must stay inside the modelled len 0..2048, got {} bytes",
        served_root_group.len()
    );
    let sent_root_group = sent["rootGroup"].as_str().unwrap_or_default();
    if sent_root_group.is_empty() {
        // What the service contract fixes is that the empty string the caller sent is NOT
        // what a qualification suite reads back: the stored root group is a non-empty one
        // (asserted above), and it is the same one on every read, so re-submitting it is
        // not refused. The document's internal shape is the emulator's own choice, which is
        // why it is not spelled out here.
        assert_ne!(
            served_root_group, sent_root_group,
            "case {case:?}: a qualification suite that named no rootGroup must read back a \
             materialized one, got {served_root_group:?}"
        );
    } else {
        assert_eq!(
            served_root_group, sent_root_group,
            "case {case:?}: the caller's rootGroup must be served unchanged, got \
             {served_root_group:?}"
        );
    }
}

/// Rule 1 on both write paths: whatever subset of the optional members a create or an
/// update carried, every later read serves the resolved configuration.
#[tokio::test]
async fn a_definition_read_serves_the_resolved_configuration() {
    let emulator = Emulator::new();
    let rows = sparse_and_full_rows();

    for (case, configuration) in &rows {
        let created = emulator.create_definition(configuration, REGION).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateSuiteDefinition, got {} {}",
            created.status, created.body
        );
        let id = created.string("suiteDefinitionId");

        let read = get_definition(&emulator, &id, None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetSuiteDefinition, got {} {}",
            read.status, read.body
        );
        assert_resolved(
            &format!("{case} (created, unqualified read)"),
            read.member("suiteDefinitionConfiguration"),
            configuration,
        );

        // The version-qualified read answers the same stored configuration.
        let by_version = get_definition(&emulator, &id, Some("v1")).await;
        assert_resolved(
            &format!("{case} (created, read by version)"),
            by_version.member("suiteDefinitionConfiguration"),
            configuration,
        );
        assert_eq!(
            by_version.member("suiteDefinitionConfiguration"),
            read.member("suiteDefinitionConfiguration"),
            "case {case:?}: the resolved configuration is stable across reads, got {} and {}",
            by_version.body,
            read.body
        );

        // And the update path stores the configuration exactly as resolved, for every row
        // used as the replacement.
        for (replacement_case, replacement) in &rows {
            let updated = update_definition(&emulator, &id, replacement).await;
            assert_eq!(
                updated.status, 200,
                "case {case:?} -> {replacement_case:?}: expected HTTP 200 from \
                 UpdateSuiteDefinition, got {} {}",
                updated.status, updated.body
            );
            let read = get_definition(&emulator, &id, None).await;
            assert_resolved(
                &format!("{case} -> {replacement_case} (updated)"),
                read.member("suiteDefinitionConfiguration"),
                replacement,
            );
        }
    }
}

/// A configuration a read served is one the service accepts back: re-submitting what a
/// qualification suite reads as its `rootGroup` is not refused, which an empty one would be
/// for a suite that is no longer intended for qualification.
#[tokio::test]
async fn a_served_configuration_can_be_submitted_back() {
    let emulator = Emulator::new();
    let configuration = json!({
        "suiteDefinitionName": "round-trip-qualification",
        "rootGroup": "",
        "intendedForQualification": true,
        "devicePermissionRoleArn": ROLE_ARN,
    });
    let id = emulator
        .create_definition(&configuration, REGION)
        .await
        .string("suiteDefinitionId");
    let served = get_definition(&emulator, &id, None)
        .await
        .member("suiteDefinitionConfiguration")
        .clone();

    // (case, the configuration re-submitted)
    let mut as_non_qualification = served.clone();
    as_non_qualification["intendedForQualification"] = json!(false);
    as_non_qualification["suiteDefinitionName"] = json!("round-trip-non-qualification");
    for (case, resubmitted) in [
        ("the configuration exactly as it was read", served.clone()),
        (
            "the same configuration, no longer for qualification",
            as_non_qualification,
        ),
    ] {
        let updated = update_definition(&emulator, &id, &resubmitted).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: a configuration the read served must be accepted back, got {} {}",
            updated.status, updated.body
        );
        let read = get_definition(&emulator, &id, None).await;
        assert_eq!(
            read.member("suiteDefinitionConfiguration"),
            &resubmitted,
            "case {case:?}: re-submitting a served configuration must change nothing in it, \
             got {}",
            read.body
        );
    }
}

/// Rule 4: the update reply identifies the definition without the ARN member, while the
/// create that minted it and the reads that report it carry it. Asserted over the whole
/// configuration table, so the rule is about the OPERATION and not about one request.
#[tokio::test]
async fn the_update_reply_carries_no_definition_arn() {
    let emulator = Emulator::new();

    for (case, configuration) in definition_rows() {
        let created = emulator.create_definition(&configuration, REGION).await;
        let id = created.string("suiteDefinitionId");
        let arn = created.string("suiteDefinitionArn");
        assert_eq!(
            arn,
            format!("arn:aws:iotdeviceadvisor:{REGION}:000000000000:suitedefinition/{id}"),
            "case {case:?}: the create must report the ARN it minted, got {}",
            created.body
        );

        let updated = update_definition(&emulator, &id, &configuration).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateSuiteDefinition, got {} {}",
            updated.status, updated.body
        );
        assert!(
            updated.member("suiteDefinitionArn").is_null(),
            "case {case:?}: an update reply must carry no suiteDefinitionArn, got {}",
            updated.body
        );
        // The members it DOES identify the definition by.
        assert_eq!(
            updated.string("suiteDefinitionId"),
            id,
            "case {case:?}: an update reply names the definition by id, got {}",
            updated.body
        );
        assert_eq!(
            updated.member("suiteDefinitionName"),
            &configuration["suiteDefinitionName"],
            "case {case:?}: an update reply carries the stored name, got {}",
            updated.body
        );
        assert_eq!(
            updated.string("suiteDefinitionVersion"),
            "v2",
            "case {case:?}: an update reply carries the version it handed out, got {}",
            updated.body
        );
        for member in ["createdAt", "lastUpdatedAt"] {
            assert!(
                updated.member(member).is_number(),
                "case {case:?}: an update reply carries {member}, got {}",
                updated.body
            );
        }

        // The ARN is the readers' to report, and it did not move.
        let read = get_definition(&emulator, &id, None).await;
        assert_eq!(
            read.string("suiteDefinitionArn"),
            arn,
            "case {case:?}: GetSuiteDefinition must report the definition's ARN, got {}",
            read.body
        );
    }
}

/// The tag maps rules 2 and 3 are asserted over: rows differing in SIZE (none, one, several)
/// and in key/value LENGTH (the modelled `String128`/`String256` maxima).
fn tag_rows() -> Vec<(String, Value)> {
    // `TagMap`'s key target is `String128` (`len 1..128`) and its value target `String256`
    // (`len 1..256`): the bounds the last row sits on.
    let mut longest = serde_json::Map::new();
    longest.insert("k".repeat(128), json!("v".repeat(256)));
    vec![
        ("no tags at all".to_string(), json!({})),
        ("one tag".to_string(), json!({ "team": "iot" })),
        (
            "three tags".to_string(),
            json!({ "team": "iot", "env": "test", "owner": "advisor" }),
        ),
        (
            "a key and a value at the modelled maxima".to_string(),
            Value::Object(longest),
        ),
    ]
}

/// Rules 2 and 3 together, for both taggable resource kinds: an untagged resource reports an
/// EMPTY tag map on every reader, a tag mutation answers no content, and what it wrote is
/// what the readers then report — so the no-content status is never an excuse for doing
/// nothing.
#[tokio::test]
async fn tags_read_as_a_map_and_tag_mutations_answer_no_content() {
    let emulator = Emulator::new();
    let configuration = json!({
        "suiteDefinitionName": "tag-response-suite",
        "rootGroup": "g",
        "devicePermissionRoleArn": ROLE_ARN,
    });

    for (index, (case, tags)) in tag_rows().into_iter().enumerate() {
        let created = emulator.create_definition(&configuration, REGION).await;
        let definition_id = created.string("suiteDefinitionId");
        let definition_arn = created.string("suiteDefinitionArn");
        let started = start_run(&emulator, &definition_id, None).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartSuiteRun, got {} {}",
            started.status, started.body
        );
        let run_id = started.string("suiteRunId");
        let run_arn = started.string("suiteRunArn");

        // Rule 2 before anything is tagged: every reader reports an empty map.
        let empty = json!({});
        for (reader, answer) in [
            (
                "GetSuiteDefinition",
                get_definition(&emulator, &definition_id, None).await,
            ),
            (
                "GetSuiteRun",
                emulator
                    .call(
                        "GET",
                        &format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}"),
                        REGION,
                        None,
                    )
                    .await,
            ),
            (
                "ListTagsForResource on the definition",
                list_tags(&emulator, &definition_arn).await,
            ),
            (
                "ListTagsForResource on the run",
                list_tags(&emulator, &run_arn).await,
            ),
        ] {
            assert_eq!(
                answer.member("tags"),
                &empty,
                "case {case:?} on {reader}: an untagged resource reports an empty tag map, \
                 got {}",
                answer.body
            );
        }

        // Rule 3 on both resource kinds, and rule 2 on what the mutation left behind.
        for (kind, arn) in [
            ("suite definition", definition_arn.clone()),
            ("suite run", run_arn.clone()),
        ] {
            let tagged = tag_resource(&emulator, &arn, &tags).await;
            assert_eq!(
                tagged.status, TAG_MUTATION_STATUS,
                "case {case:?} on a {kind}: expected HTTP {TAG_MUTATION_STATUS} from \
                 TagResource, got {} {}",
                tagged.status, tagged.body
            );
            assert!(
                tagged
                    .body
                    .as_object()
                    .is_none_or(serde_json::Map::is_empty),
                "case {case:?} on a {kind}: a no-content answer reports nothing, got {}",
                tagged.body
            );
            assert!(
                tagged.error_code.is_none(),
                "case {case:?} on a {kind}: a success carries no error code, got {:?}",
                tagged.error_code
            );

            let listed = list_tags(&emulator, &arn).await;
            assert_eq!(
                listed.member("tags"),
                &tags,
                "case {case:?} on a {kind}: the tags the mutation wrote must be readable, \
                 got {}",
                listed.body
            );

            // Removing every key it named leaves the resource reading as untagged — again
            // an empty map, and again a no-content answer. `tagKeys` is a required member,
            // so the row that named no tags at all has nothing to remove.
            let keys: Vec<String> = tags
                .as_object()
                .expect("a tag map is a JSON object")
                .keys()
                .cloned()
                .collect();
            if !keys.is_empty() {
                let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
                let untagged = untag_resource(&emulator, &arn, &borrowed).await;
                assert_eq!(
                    untagged.status, TAG_MUTATION_STATUS,
                    "case {case:?} on a {kind}: expected HTTP {TAG_MUTATION_STATUS} from \
                     UntagResource, got {} {}",
                    untagged.status, untagged.body
                );
                assert!(
                    untagged
                        .body
                        .as_object()
                        .is_none_or(serde_json::Map::is_empty),
                    "case {case:?} on a {kind}: a no-content answer reports nothing, got {}",
                    untagged.body
                );
            }
            let listed = list_tags(&emulator, &arn).await;
            assert_eq!(
                listed.member("tags"),
                &empty,
                "case {case:?} on a {kind}: a resource untagged down to nothing reports an \
                 empty map, got {}",
                listed.body
            );
        }

        // A tagged definition reads its tags back through the resource read too, which is
        // the same projection of the same row.
        let tagged = tag_resource(&emulator, &definition_arn, &tags).await;
        assert_eq!(
            tagged.status, TAG_MUTATION_STATUS,
            "case {case:?}: expected HTTP {TAG_MUTATION_STATUS} re-tagging definition {index}, \
             got {} {}",
            tagged.status,
            tagged.body
        );
        let read = get_definition(&emulator, &definition_id, None).await;
        assert_eq!(
            read.member("tags"),
            &tags,
            "case {case:?}: GetSuiteDefinition must report the tags the mutation wrote, got {}",
            read.body
        );
    }
}

/// Resolution happens before the create's idempotency fingerprint, so two creates that
/// resolve to the SAME stored configuration are one request: a `clientToken` offered for the
/// omitted form and for the explicit-default form answers the definition the first call
/// created, while a token offered for a configuration that resolves differently is still the
/// client mistake it was.
#[tokio::test]
async fn one_client_token_covers_the_forms_that_resolve_alike() {
    let emulator = Emulator::new();

    // (case, the configuration the retry sends, whether it is the same request)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "the omitted device list sent explicitly as empty",
            json!({
                "suiteDefinitionName": "token-suite",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "devices": [],
            }),
            true,
        ),
        (
            "the resolved booleans sent explicitly as false",
            json!({
                "suiteDefinitionName": "token-suite",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "intendedForQualification": false,
                "isLongDurationTest": false,
            }),
            true,
        ),
        (
            "the resolved protocol sent explicitly",
            json!({
                "suiteDefinitionName": "token-suite",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "protocol": DEFAULT_PROTOCOL,
            }),
            true,
        ),
        (
            "another protocol, which resolves differently",
            json!({
                "suiteDefinitionName": "token-suite",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "protocol": "MqttV5",
            }),
            false,
        ),
        (
            "a device the first request did not name",
            json!({
                "suiteDefinitionName": "token-suite",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "devices": [{ "thingArn": thing_arn(4) }],
            }),
            false,
        ),
    ];

    for (index, (case, retried, same_request)) in rows.into_iter().enumerate() {
        let token = format!("token-{index:04}");
        // The first call names NONE of the optional members.
        let first = emulator
            .call(
                "POST",
                "/suiteDefinitions",
                REGION,
                Some(json!({
                    "suiteDefinitionConfiguration": {
                        "suiteDefinitionName": "token-suite",
                        "rootGroup": "g",
                        "devicePermissionRoleArn": ROLE_ARN,
                    },
                    "clientToken": token,
                })),
            )
            .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: setup create failed with {} {}",
            first.status, first.body
        );

        let retry = emulator
            .call(
                "POST",
                "/suiteDefinitions",
                REGION,
                Some(json!({
                    "suiteDefinitionConfiguration": retried,
                    "clientToken": token,
                })),
            )
            .await;
        if same_request {
            assert_eq!(
                retry.status, 200,
                "case {case:?}: a form that resolves alike is a retry, got {} {}",
                retry.status, retry.body
            );
            assert_eq!(
                retry.string("suiteDefinitionId"),
                first.string("suiteDefinitionId"),
                "case {case:?}: a retry must answer the definition the first call created, \
                 got {}",
                retry.body
            );
        } else {
            assert_eq!(
                (retry.status, retry.error_code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: a configuration that resolves differently under the same \
                 token is refused, got {} {:?} {}",
                retry.status,
                retry.error_code,
                retry.body
            );
        }
    }
}

/// The no-content status is a SUCCESS status: a refused tag mutation keeps the refusal's own
/// status and error code, for each way a `resourceArn` is refused.
#[tokio::test]
async fn a_refused_tag_mutation_keeps_its_own_status() {
    let emulator = Emulator::new();
    let configuration = json!({
        "suiteDefinitionName": "refusal-status-suite",
        "rootGroup": "g",
        "devicePermissionRoleArn": ROLE_ARN,
    });
    emulator.create_definition(&configuration, REGION).await;

    // (case, the resourceArn the call names, expected status, expected error code)
    let unknown = "11111111-2222-3333-4444-555555555555";
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a suite definition the store does not hold",
            format!("arn:aws:iotdeviceadvisor:{REGION}:000000000000:suitedefinition/{unknown}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a suite run the store does not hold",
            format!("arn:aws:iotdeviceadvisor:{REGION}:000000000000:suiterun/{unknown}/{unknown}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a value that is not one of this service's ARNs",
            "this-value-is-not-an-arn-at-all".to_string(),
            400,
            "ValidationException",
        ),
    ];

    for (case, arn, status, code) in rows {
        for (operation, answer) in [
            (
                "TagResource",
                tag_resource(&emulator, &arn, &json!({ "env": "test" })).await,
            ),
            (
                "UntagResource",
                untag_resource(&emulator, &arn, &["env"]).await,
            ),
        ] {
            assert_eq!(
                (answer.status, answer.error_code.as_deref()),
                (status, Some(code)),
                "case {case:?} on {operation}: expected HTTP {status} {code}, got {} {:?} {}",
                answer.status,
                answer.error_code,
                answer.body
            );
        }
    }
}
