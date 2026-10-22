// SPDX-License-Identifier: CC0-1.0

//! The TAG slice of a Performance Insights resource, end to end over the wire:
//! `TagResource`, `ListTagsForResource` and `UntagResource`.
//!
//! RULE under test: a Performance Insights resource is tagged by its `ResourceARN`,
//! and the tags a resource carries are ONE set — whatever `CreatePerformanceAnalysisReport`
//! persisted, as `TagResource` and `UntagResource` have since left it — which
//! `ListTagsForResource` and a listing's `ListTags` summary both report. An ARN
//! inside the modelled `AmazonResourceName` pattern (`^arn:.*:pi:.*$`) that names no
//! resource this account holds is refused at the authorization stage with the
//! modelled `NotAuthorizedException` (HTTP 400) under the one message every refusal
//! in this service carries; an ARN outside that pattern is refused earlier, by the
//! boundary validator, so a client can tell a malformed ARN from an absent resource.
//!
//! The rows span what the rule generalises over: how many tags a resource starts
//! with, whether a write adds a key or overwrites one, which keys are removed
//! (present, absent, all of them), and the forms a non-existent ARN can take.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator, Answer, REGION};

use service_com_amazonaws_pi_performanceinsightsv20180227::state;

/// 2026-01-01T00:00:00Z. `awsJson1_1` carries an `ISOTimestamp` as epoch seconds.
const T0: f64 = 1_767_225_600.0;

/// A data source of the form the authorization gate serves.
const IDENTIFIER: &str = "db-ABCDEFGHIJKLMNOPQRSTU1VW2X";

/// The `ServiceType` every request below names.
const SERVICE_TYPE: &str = "RDS";

/// One row of the tag table: the tags the report is created with, the tags a
/// `TagResource` then writes, the keys an `UntagResource` then removes, and the set
/// that must be left.
struct TagCase {
    case: &'static str,
    created_with: &'static [(&'static str, &'static str)],
    tagged_with: &'static [(&'static str, &'static str)],
    untagged_keys: &'static [&'static str],
    expected: &'static [(&'static str, &'static str)],
}

const TAG_CASES: &[TagCase] = &[
    TagCase {
        case: "a report created with no tags, one added",
        created_with: &[],
        tagged_with: &[("team", "db")],
        untagged_keys: &[],
        expected: &[("team", "db")],
    },
    TagCase {
        case: "a tag added beside the two Create persisted",
        created_with: &[("env", "prod"), ("owner", "sre")],
        tagged_with: &[("tier", "gold")],
        untagged_keys: &[],
        expected: &[("env", "prod"), ("owner", "sre"), ("tier", "gold")],
    },
    TagCase {
        case: "an existing key overwritten rather than duplicated",
        created_with: &[("env", "prod")],
        tagged_with: &[("env", "staging")],
        untagged_keys: &[],
        expected: &[("env", "staging")],
    },
    TagCase {
        case: "one key removed, the others kept",
        created_with: &[("env", "prod"), ("owner", "sre")],
        tagged_with: &[("tier", "gold")],
        untagged_keys: &["owner"],
        expected: &[("env", "prod"), ("tier", "gold")],
    },
    TagCase {
        case: "a key the resource never carried removes nothing",
        created_with: &[("env", "prod")],
        tagged_with: &[],
        untagged_keys: &["absent-key"],
        expected: &[("env", "prod")],
    },
    TagCase {
        case: "every key removed leaves an empty set",
        created_with: &[("env", "prod"), ("owner", "sre")],
        tagged_with: &[("tier", "gold")],
        untagged_keys: &["env", "owner", "tier"],
        expected: &[],
    },
    TagCase {
        case: "an empty value is a value, and a key with it survives",
        created_with: &[("note", "")],
        tagged_with: &[("marker", "")],
        untagged_keys: &[],
        expected: &[("note", ""), ("marker", "")],
    },
];

/// The tags of an answer that carries a `Tags` list, as (key, value) pairs in the
/// order the service reported them.
fn tags_of(value: &Value) -> Vec<(String, String)> {
    value
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag.get("Key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        tag.get("Value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The same pairs, as a sorted set, for an assertion that is about membership rather
/// than order.
fn tag_set(value: &Value) -> Vec<(String, String)> {
    let mut pairs = tags_of(value);
    pairs.sort();
    pairs
}

/// `expected`, as the same sorted set.
fn expected_set(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = pairs_in_order(pairs);
    pairs.sort();
    pairs
}

/// The same pairs, owned, in the order they are written here.
fn pairs_in_order(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// A `Tags` request member built from (key, value) pairs.
fn tags_member(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// Create one report and answer with its id.
async fn create_report(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    tags: &[(&str, &str)],
) -> String {
    let mut input = json!({
        "ServiceType": SERVICE_TYPE,
        "Identifier": IDENTIFIER,
        "StartTime": T0,
        "EndTime": T0 + 3_600.0,
    });
    if !tags.is_empty() {
        input["Tags"] = tags_member(tags);
    }
    let created = call(emulator, "CreatePerformanceAnalysisReport", input).await;
    created
        .member("AnalysisReportId")
        .as_str()
        .unwrap_or_else(|| panic!("Create answers with a minted id, got {:?}", created.body))
        .to_string()
}

/// The ARN of one report, the handle the tag operations address it by. Minted by the
/// service itself from the region the request was signed for, so the test asks the
/// service's own ARN rule rather than restating it.
fn report_arn(report_id: &str) -> String {
    state::report_arn(REGION, SERVICE_TYPE, IDENTIFIER, report_id)
}

/// RULE: the tags a resource carries are one set, written by Create and the two tag
/// writers and reported identically by `ListTagsForResource` and by a listing's
/// `ListTags` summary.
#[tokio::test]
async fn a_resources_tags_are_one_set_every_reader_agrees_on() {
    let emulator = emulator();

    for case in TAG_CASES {
        let label = case.case;
        let report_id = create_report(&emulator, case.created_with).await;
        let arn = report_arn(&report_id);

        // What Create persisted is what ListTagsForResource reports, in the order
        // the caller sent it.
        let listed = call(
            &emulator,
            "ListTagsForResource",
            json!({"ServiceType": SERVICE_TYPE, "ResourceARN": arn}),
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: expected 200 from ListTagsForResource, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            tags_of(listed.member("Tags")),
            pairs_in_order(case.created_with),
            "case {label:?}: expected the tags Create persisted, in the order it was sent them, \
             got {:?}",
            listed.body
        );

        if !case.tagged_with.is_empty() {
            let tagged = call(
                &emulator,
                "TagResource",
                json!({
                    "ServiceType": SERVICE_TYPE,
                    "ResourceARN": arn,
                    "Tags": tags_member(case.tagged_with),
                }),
            )
            .await;
            assert_eq!(
                tagged.status, 200,
                "case {label:?}: expected 200 from TagResource, got {} {:?}",
                tagged.status, tagged.body
            );
        }

        if !case.untagged_keys.is_empty() {
            let untagged = call(
                &emulator,
                "UntagResource",
                json!({
                    "ServiceType": SERVICE_TYPE,
                    "ResourceARN": arn,
                    "TagKeys": case.untagged_keys,
                }),
            )
            .await;
            assert_eq!(
                untagged.status, 200,
                "case {label:?}: expected 200 from UntagResource, got {} {:?}",
                untagged.status, untagged.body
            );
        }

        let listed = call(
            &emulator,
            "ListTagsForResource",
            json!({"ServiceType": SERVICE_TYPE, "ResourceARN": arn}),
        )
        .await;
        assert_eq!(
            tag_set(listed.member("Tags")),
            expected_set(case.expected),
            "case {label:?}: expected the tag set the writes leave, got {:?}",
            listed.body
        );

        // The SAME set is what the report's own listing reports, because the two
        // readers read one home.
        let reports = call(
            &emulator,
            "ListPerformanceAnalysisReports",
            json!({
                "ServiceType": SERVICE_TYPE,
                "Identifier": IDENTIFIER,
                "ListTags": true,
            }),
        )
        .await;
        let summary = reports
            .member("AnalysisReports")
            .as_array()
            .and_then(|summaries| {
                summaries
                    .iter()
                    .find(|summary| {
                        summary.get("AnalysisReportId").and_then(Value::as_str)
                            == Some(report_id.as_str())
                    })
                    .cloned()
            })
            .unwrap_or_else(|| panic!("case {label:?}: the report is listed, got {:?}", reports.body));
        assert_eq!(
            tag_set(summary.get("Tags").unwrap_or(&Value::Null)),
            expected_set(case.expected),
            "case {label:?}: a listing's summary reports the same set as ListTagsForResource, \
             got {summary:?}"
        );

        // Clean up, so each row starts from a resource of its own.
        call(
            &emulator,
            "DeletePerformanceAnalysisReport",
            json!({
                "ServiceType": SERVICE_TYPE,
                "Identifier": IDENTIFIER,
                "AnalysisReportId": report_id,
            }),
        )
        .await;
    }
}

/// RULE: a well-formed ARN that names no resource this account holds is refused at
/// the authorization stage — `NotAuthorizedException`, HTTP 400, under the one
/// message that discloses nothing — on every tag operation.
#[tokio::test]
async fn a_well_formed_arn_naming_no_resource_is_not_authorized() {
    let emulator = emulator();

    // A real report exists, so the refusals below are about the ARN and not about an
    // empty store.
    let report_id = create_report(&emulator, &[("env", "prod")]).await;
    let live_arn = report_arn(&report_id);

    let deleted_id = create_report(&emulator, &[("env", "gone")]).await;
    let deleted_arn = report_arn(&deleted_id);
    call(
        &emulator,
        "DeletePerformanceAnalysisReport",
        json!({
            "ServiceType": SERVICE_TYPE,
            "Identifier": IDENTIFIER,
            "AnalysisReportId": deleted_id,
        }),
    )
    .await;

    // Every row satisfies the modelled `^arn:.*:pi:.*$` pattern and names no report
    // this account holds: an id never minted, a report deleted, another region,
    // another account, another resource kind.
    let absent: Vec<(&str, String)> = vec![
        (
            "an AnalysisReportId no Create ever minted",
            report_arn("report-0123456789abcdef0"),
        ),
        ("a report that was deleted", deleted_arn),
        (
            "the live report in another region",
            state::report_arn("eu-west-1", SERVICE_TYPE, IDENTIFIER, &report_id),
        ),
        (
            "the live report under another account",
            live_arn.replace(
                &format!(":{}:", ::emulator_server::EMULATOR_ACCOUNT_ID),
                ":123456789012:",
            ),
        ),
        (
            "a resource kind this service issues no ARN for",
            format!(
                "arn:aws:pi:{REGION}:{}:dashboard/my-dashboard",
                ::emulator_server::EMULATOR_ACCOUNT_ID
            ),
        ),
    ];

    for (label, arn) in &absent {
        let calls: Vec<(&str, Value)> = vec![
            (
                "ListTagsForResource",
                json!({"ServiceType": SERVICE_TYPE, "ResourceARN": arn}),
            ),
            (
                "TagResource",
                json!({
                    "ServiceType": SERVICE_TYPE,
                    "ResourceARN": arn,
                    "Tags": [{"Key": "team", "Value": "db"}],
                }),
            ),
            (
                "UntagResource",
                json!({
                    "ServiceType": SERVICE_TYPE,
                    "ResourceARN": arn,
                    "TagKeys": ["team"],
                }),
            ),
        ];
        for (operation, input) in calls {
            let case = format!("{label} / {operation}");
            let answer = call(&emulator, operation, input).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "NotAuthorizedException",
                "case {case}: expected NotAuthorizedException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_message(),
                state::NOT_AUTHORIZED_MESSAGE,
                "case {case}: expected the service's uniform authorization message, got {:?}",
                answer.body
            );
            assert!(
                !answer.error_message().contains(arn.as_str()),
                "case {case}: expected a message naming no resource, got {:?}",
                answer.body
            );
        }
    }

    // None of those refusals touched the resource that does exist.
    let listed = call(
        &emulator,
        "ListTagsForResource",
        json!({"ServiceType": SERVICE_TYPE, "ResourceARN": live_arn}),
    )
    .await;
    assert_eq!(
        tag_set(listed.member("Tags")),
        expected_set(&[("env", "prod")]),
        "case the live resource after the refusals: expected its tags untouched, got {:?}",
        listed.body
    );
}

/// RULE: the two stages stay distinguishable — an ARN outside the modelled pattern is
/// refused by the boundary validator (NOT as an authorization outcome), and only an
/// ARN that passes it reaches the existence check above.
#[tokio::test]
async fn a_malformed_arn_is_refused_before_the_authorization_stage() {
    let emulator = emulator();

    let malformed: &[(&str, &str)] = &[
        ("not an ARN at all", "not-a-pi-arn-at-all"),
        (
            "an ARN of another service",
            "arn:aws:rds:us-west-2:000000000000:db:my-instance",
        ),
        ("the empty string", ""),
    ];

    for (label, arn) in malformed {
        for operation in ["ListTagsForResource", "TagResource", "UntagResource"] {
            let mut input = json!({"ServiceType": SERVICE_TYPE, "ResourceARN": arn});
            if operation == "TagResource" {
                input["Tags"] = json!([{"Key": "team", "Value": "db"}]);
            }
            if operation == "UntagResource" {
                input["TagKeys"] = json!(["team"]);
            }
            let case = format!("{label} ({arn:?}) / {operation}");
            let answer = call(&emulator, operation, input).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_ne!(
                answer.error_code(),
                "NotAuthorizedException",
                "case {case}: a malformed ARN is an argument refusal, not an authorization one, \
                 got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: a resource holds at most the modelled `TagList` maximum, so a write that
/// would pass it is refused with `InvalidArgumentException` and adds no tag at all.
#[tokio::test]
async fn a_write_past_the_tag_maximum_is_refused_and_adds_nothing() {
    let emulator = emulator();
    let report_id = create_report(&emulator, &[]).await;
    let arn = report_arn(&report_id);

    let maximum = state::MAX_TAGS_PER_REPORT;
    let full: Vec<Value> = (0..maximum)
        .map(|index| json!({"Key": format!("key-{index}"), "Value": "v"}))
        .collect();
    let filled = call(
        &emulator,
        "TagResource",
        json!({"ServiceType": SERVICE_TYPE, "ResourceARN": arn, "Tags": full}),
    )
    .await;
    assert_eq!(
        filled.status, 200,
        "case a write of exactly the maximum: expected 200, got {} {:?}",
        filled.status, filled.body
    );

    // One more KEY does not fit …
    let past = call(
        &emulator,
        "TagResource",
        json!({
            "ServiceType": SERVICE_TYPE,
            "ResourceARN": arn,
            "Tags": [{"Key": "one-too-many", "Value": "v"}],
        }),
    )
    .await;
    assert_eq!(
        (past.status, past.error_code().as_str()),
        (400, "InvalidArgumentException"),
        "case one key past the maximum: expected InvalidArgumentException, got {} {:?}",
        past.status,
        past.body
    );

    // … while overwriting a key the resource already holds still does, because it
    // adds no key.
    let overwrite = call(
        &emulator,
        "TagResource",
        json!({
            "ServiceType": SERVICE_TYPE,
            "ResourceARN": arn,
            "Tags": [{"Key": "key-0", "Value": "overwritten"}],
        }),
    )
    .await;
    assert_eq!(
        overwrite.status, 200,
        "case an overwrite at the maximum: expected 200, got {} {:?}",
        overwrite.status, overwrite.body
    );

    let listed = call(
        &emulator,
        "ListTagsForResource",
        json!({"ServiceType": SERVICE_TYPE, "ResourceARN": arn}),
    )
    .await;
    let pairs = tags_of(listed.member("Tags"));
    assert_eq!(
        pairs.len(),
        maximum,
        "case the resource after the refusal: expected exactly {maximum} tags, got {:?}",
        listed.body
    );
    assert!(
        pairs.contains(&("key-0".to_string(), "overwritten".to_string()))
            && !pairs.iter().any(|(key, _)| key == "one-too-many"),
        "case the resource after the refusal: expected the overwrite applied and the refused key \
         absent, got {:?}",
        listed.body
    );
}

/// RULE: the tags go with the resource — deleting the report drops the tags filed
/// under its ARN, so a later read of that ARN is the uniform refusal and a NEW report
/// of the same data source starts from its own set.
#[tokio::test]
async fn tags_are_dropped_with_the_resource_they_belong_to() {
    let emulator = emulator();

    let first_id = create_report(&emulator, &[("env", "prod")]).await;
    let first_arn = report_arn(&first_id);
    call(
        &emulator,
        "DeletePerformanceAnalysisReport",
        json!({
            "ServiceType": SERVICE_TYPE,
            "Identifier": IDENTIFIER,
            "AnalysisReportId": first_id,
        }),
    )
    .await;

    let gone: Answer = call(
        &emulator,
        "ListTagsForResource",
        json!({"ServiceType": SERVICE_TYPE, "ResourceARN": first_arn}),
    )
    .await;
    assert_eq!(
        (gone.status, gone.error_code().as_str()),
        (400, "NotAuthorizedException"),
        "case a deleted resource's ARN: expected the uniform refusal, got {} {:?}",
        gone.status,
        gone.body
    );

    let second_id = create_report(&emulator, &[]).await;
    let second_arn = report_arn(&second_id);
    let listed = call(
        &emulator,
        "ListTagsForResource",
        json!({"ServiceType": SERVICE_TYPE, "ResourceARN": second_arn}),
    )
    .await;
    assert_eq!(
        tags_of(listed.member("Tags")),
        Vec::<(String, String)>::new(),
        "case a new report of the same data source: expected its own empty set, got {:?}",
        listed.body
    );
}

/// RULE: an ARN a client BUILDS for a report it created addresses that report — no
/// response member of this model carries a report's ARN, so the caller assembles it
/// from the `ServiceType` and `Identifier` it created the report with and the
/// `AnalysisReportId` it got back, and the case it writes the service type in does not
/// decide whether the resource is found. Every OTHER segment still does: a different
/// region, account, data source or report id addresses nothing.
#[tokio::test]
async fn a_client_built_arn_addresses_the_report_it_names() {
    let emulator = emulator();
    let report_id = create_report(&emulator, &[("env", "prod")]).await;
    let minted = report_arn(&report_id);

    // Rows that name the SAME report: the service type as the model spells it, and
    // the spellings a client holding the value in another case writes.
    let same: Vec<(&str, String)> = vec![
        ("the service type as the model spells it", minted.clone()),
        (
            "the service type lowercased",
            minted.replace("/RDS/", "/rds/"),
        ),
        (
            "the service type in mixed case",
            minted.replace("/RDS/", "/Rds/"),
        ),
    ];
    for (label, arn) in &same {
        let listed = call(
            &emulator,
            "ListTagsForResource",
            json!({"ServiceType": SERVICE_TYPE, "ResourceARN": arn}),
        )
        .await;
        assert_eq!(
            (listed.status, tag_set(listed.member("Tags"))),
            (200, expected_set(&[("env", "prod")])),
            "case {label}: expected the report's tags, got {} {:?}",
            listed.status,
            listed.body
        );
    }

    // Rows that differ in any OTHER segment name no report this account holds.
    let other: Vec<(&str, String)> = vec![
        (
            "another data source",
            state::report_arn(REGION, SERVICE_TYPE, "db-OTHERSOURCE00001", &report_id),
        ),
        (
            "another report of the same data source",
            report_arn("report-aaaaaaaaaaaaaaaaa"),
        ),
        (
            "a resource part with an extra segment",
            format!("{minted}/extra"),
        ),
        (
            "a resource part with the report id missing",
            minted
                .rsplit_once('/')
                .map(|(head, _)| head.to_string())
                .expect("the minted ARN has a report-id segment"),
        ),
    ];
    for (label, arn) in &other {
        let answer = call(
            &emulator,
            "ListTagsForResource",
            json!({"ServiceType": SERVICE_TYPE, "ResourceARN": arn}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, "NotAuthorizedException"),
            "case {label}: expected the uniform refusal, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}
