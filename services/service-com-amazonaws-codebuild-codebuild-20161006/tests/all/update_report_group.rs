// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateReportGroup`, driven over the wire the router serves:
//! `POST /` with `X-Amz-Target: CodeBuild_20161006.UpdateReportGroup` on awsJson1_1,
//! through the harness the project -> build suite owns
//! (`crate::project_build_lifecycle`) and the `CreateReportGroup` body builder
//! `crate::report_group_lifecycle` owns.
//!
//! RULE under test: `UpdateReportGroup` resolves the report group its `arn` addresses
//! and replaces exactly the settings the request carries — each member whole, a
//! structure never merged field by field — leaving every omitted setting and the group's
//! identity (`name`, `type`, `arn`, `created`, the read-only `status`) as stored, and
//! moving `lastModified`. A value that is not a report-group ARN of this service is
//! refused with `InvalidInputException`, a well-formed ARN this account holds no group
//! for with `ResourceNotFoundException`, and a merged export configuration that names no
//! destination with `InvalidInputException`; every refusal persists nothing.
//!
//! Every update is read back through `BatchGetReportGroups`, so what is asserted is the
//! store rather than the updating call's own answer.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::project_build_lifecycle::{call, emulator, error_type, expected_arn, REGION};
use crate::report_group_lifecycle::create_report_group_body;
use crate::update_project::{all_members, instant};

/// The export configuration every group below is created with, so "a member the request
/// did not carry is unchanged" has something to hold on to, and so a structure replaced
/// WHOLE is visible: every optional member of `S3ReportExportConfig` is set here
/// (`smithy_query(query="shape",shape="S3ReportExportConfig")`).
fn stored_export_config() -> Value {
    json!({
        "exportConfigType": "S3",
        "s3Destination": {
            "bucket": "stored-bucket",
            "bucketOwner": "111122223333",
            "path": "stored/",
            "packaging": "ZIP",
            "encryptionKey": "alias/aws/s3",
            "encryptionDisabled": false,
        },
    })
}

/// The tag list every group below is created with.
fn stored_tags() -> Value {
    json!([{"key": "team", "value": "platform"}])
}

/// Send one `UpdateReportGroup` for `arn` with `extra`'s members merged in.
async fn update_report_group(
    router: &MultiServiceRouter,
    arn: &str,
    extra: Value,
) -> (u16, Value) {
    let mut body = json!({"arn": arn});
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extra) {
        target.extend(members);
    }
    call(router, "UpdateReportGroup", body).await
}

/// `UpdateReportGroup`, asserting it succeeded, handing back the group it returned.
async fn update_ok(router: &MultiServiceRouter, case: &str, arn: &str, extra: Value) -> Value {
    let (status, body) = update_report_group(router, arn, extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UpdateReportGroup to answer 200, got {status} with {body}"
    );
    body["reportGroup"].clone()
}

/// The report group `arn` names, as the read path reports it.
async fn fetch_report_group(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(
        router,
        "BatchGetReportGroups",
        json!({"reportGroupArns": [arn]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetReportGroups {arn:?}: expected 200, got {status} with {body}"
    );
    body["reportGroups"][0].clone()
}

/// Create one report group carrying [`stored_export_config`] and [`stored_tags`], and
/// hand back `(its ARN, its record)`.
async fn seeded_report_group(
    router: &MultiServiceRouter,
    name: &str,
    report_type: &str,
) -> (String, Value) {
    let (status, body) = call(
        router,
        "CreateReportGroup",
        create_report_group_body(
            name,
            report_type,
            stored_export_config(),
            Some(stored_tags()),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateReportGroup {name:?}: expected 200, got {status} with {body}"
    );
    let created = body["reportGroup"].clone();
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a created report group carries an arn, got {created}"))
        .to_string();
    (arn, created)
}

/// Assert every member of `after` except the one a successful update always moves
/// (`lastModified`) and the ones `changed` names reads exactly as in `before`.
fn only_the_requested_members_changed(case: &str, before: &Value, after: &Value, changed: &Value) {
    let changed = changed.as_object().expect("the expectation is an object");
    for (member, want) in changed {
        assert_eq!(
            &after[member], want,
            "case {case:?}: expected {member} = {want}, got {after}"
        );
    }
    for member in all_members(before, after) {
        if member == "lastModified" || changed.contains_key(&member) {
            continue;
        }
        assert_eq!(
            after[&member], before[&member],
            "case {case:?}: {member} was not in the request, so it must stay {:?}, got {:?}",
            before[&member], after[&member]
        );
    }
}

/// RULE: a member the request carries replaces the stored setting whole; every member it
/// omits is reported exactly as stored, and the read path hands back the same record. The
/// table varies the member, its value, its type (structure, list), its entry count and
/// its form (replaced, emptied, filled to the modelled maximum) — the `ReportType` and
/// `ReportExportConfigType` values are the model's
/// (`smithy_query(query="shape",shape="ReportType,ReportExportConfigType,ReportPackagingType")`).
#[tokio::test]
async fn update_report_group_replaces_the_members_it_was_given_and_keeps_the_rest() {
    let router = emulator();

    // `TagList`'s `@length(min: 0, max: 50)` ceiling, as one request's worth of tags.
    let fifty_tags = Value::Array(
        (0..50)
            .map(|index| json!({"key": format!("k{index}"), "value": format!("v{index}")}))
            .collect(),
    );

    // (case, the report type the group was created with, the members the request carries,
    // the members that must change and to what)
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        ("nothing but the arn", "TEST", json!({}), json!({})),
        (
            "an export config that exports nothing",
            "TEST",
            json!({"exportConfig": {"exportConfigType": "NO_EXPORT"}}),
            json!({"exportConfig": {"exportConfigType": "NO_EXPORT"}}),
        ),
        // A structure replaces WHOLE: the stored path, packaging, owner and key are
        // gone, because the destination the request carried declares none of them.
        (
            "an s3Destination replacing the stored one whole",
            "TEST",
            json!({"exportConfig": {
                "exportConfigType": "S3",
                "s3Destination": {"bucket": "replacement-bucket"},
            }}),
            json!({"exportConfig": {
                "exportConfigType": "S3",
                "s3Destination": {"bucket": "replacement-bucket"},
            }}),
        ),
        (
            "an unpackaged, unencrypted export under another path",
            "CODE_COVERAGE",
            json!({"exportConfig": {
                "exportConfigType": "S3",
                "s3Destination": {
                    "bucket": "replacement-bucket",
                    "path": "coverage/",
                    "packaging": "NONE",
                    "encryptionDisabled": true,
                },
            }}),
            json!({"exportConfig": {
                "exportConfigType": "S3",
                "s3Destination": {
                    "bucket": "replacement-bucket",
                    "path": "coverage/",
                    "packaging": "NONE",
                    "encryptionDisabled": true,
                },
            }}),
        ),
        (
            "two tags replacing one",
            "CODE_COVERAGE",
            json!({"tags": [{"key": "a", "value": "1"}, {"key": "b", "value": "2"}]}),
            json!({"tags": [{"key": "a", "value": "1"}, {"key": "b", "value": "2"}]}),
        ),
        ("tags emptied", "TEST", json!({"tags": []}), json!({"tags": []})),
        (
            "tags at the modelled maximum of 50",
            "TEST",
            json!({"tags": fifty_tags}),
            json!({"tags": fifty_tags}),
        ),
        (
            "both members at once",
            "CODE_COVERAGE",
            json!({
                "exportConfig": {"exportConfigType": "NO_EXPORT"},
                "tags": [{"key": "only", "value": "tag"}],
            }),
            json!({
                "exportConfig": {"exportConfigType": "NO_EXPORT"},
                "tags": [{"key": "only", "value": "tag"}],
            }),
        ),
    ];

    for (index, (case, report_type, request, expected)) in cases.iter().enumerate() {
        let name = format!("replace-{index:02}");
        let (arn, stored) = seeded_report_group(&router, &name, report_type).await;

        let updated = update_ok(&router, case, &arn, request.clone()).await;

        only_the_requested_members_changed(case, &stored, &updated, expected);
        assert!(
            instant(&updated, "lastModified") > instant(&stored, "lastModified"),
            "case {case:?}: a successful update must move lastModified past {}, got {}",
            instant(&stored, "lastModified"),
            instant(&updated, "lastModified")
        );

        // The read path reports the record this call persisted, member for member.
        let fetched = fetch_report_group(&router, &arn).await;
        assert_eq!(
            fetched, updated,
            "case {case:?}: BatchGetReportGroups must report the updated record, got {fetched}"
        );
    }
}

/// What one row of the addressing table expects back.
enum Addressed {
    /// The value is not a report-group ARN of this service at all.
    NotAnArn,
    /// A well-formed report-group ARN this account holds no group for.
    NotHeld,
}

/// RULE: an ARN addresses exactly the report group whose STORED ARN it is, so a value
/// that is not a report-group ARN of this service is refused with the modelled
/// `InvalidInputException` and a well-formed one this account holds no group for with the
/// modelled `ResourceNotFoundException` — both HTTP 400, each with CodeBuild's own fixed
/// sentence, and neither touching the group the account does hold. The rows vary the way
/// the value misses: the resource type, the service, the partition, the account, the
/// region and the name.
#[tokio::test]
async fn an_arn_addressing_no_report_group_is_refused_and_changes_nothing() {
    let router = emulator();
    let (held, created) = seeded_report_group(&router, "held-group", "TEST").await;

    let cases: Vec<(&str, String, Addressed)> = vec![
        (
            "a bare name rather than an ARN",
            "held-group".to_string(),
            Addressed::NotAnArn,
        ),
        (
            "an ARN of another resource type",
            expected_arn("project/held-group"),
            Addressed::NotAnArn,
        ),
        (
            "an ARN of another service",
            format!("arn:aws:codepipeline:{REGION}:000000000000:report-group/held-group"),
            Addressed::NotAnArn,
        ),
        (
            "a report-group ARN naming no group",
            expected_arn("report-group/"),
            Addressed::NotAnArn,
        ),
        (
            "this group's ARN in another partition",
            held.replace("arn:aws:", "arn:aws-cn:"),
            Addressed::NotHeld,
        ),
        (
            "this group's ARN in another account",
            held.replace(engine::EMULATOR_ACCOUNT_ID, "000000000001"),
            Addressed::NotHeld,
        ),
        (
            "this group's ARN in another region",
            held.replace(REGION, "eu-west-1"),
            Addressed::NotHeld,
        ),
        (
            "a name the account holds no group for",
            expected_arn("report-group/never-created"),
            Addressed::NotHeld,
        ),
    ];

    for (case, arn, addressed) in cases {
        let (status, body) =
            update_report_group(&router, &arn, json!({"tags": [{"key": "x", "value": "y"}]})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        let (expected_error, expected_message) = match addressed {
            Addressed::NotAnArn => (
                "InvalidInputException",
                state::INVALID_REPORT_GROUP_ARN_MESSAGE,
            ),
            Addressed::NotHeld => (
                "ResourceNotFoundException",
                state::REPORT_GROUP_NOT_FOUND_MESSAGE,
            ),
        };
        assert_eq!(
            error_type(&body),
            expected_error,
            "case {case:?}: expected {expected_error}, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(expected_message),
            "case {case:?}: expected the refusal {expected_message:?}, got {body}"
        );

        // The group the account does hold is untouched: nothing was resolved, so nothing
        // was written.
        let fetched = fetch_report_group(&router, &held).await;
        assert_eq!(
            fetched, created,
            "case {case:?}: a refused update must leave the stored record as it was, \
             got {fetched}"
        );
    }
}

/// RULE: the export configuration the report group CARRIES once the request has been
/// applied — not the request alone — is what has to name the bucket an `S3` export sends
/// the raw data to, so an update is refused with the modelled `InvalidInputException`
/// (HTTP 400) exactly when the merge of request and stored record leaves an `S3` export
/// with no destination, and persists nothing. The rows change one side of the merge each,
/// so a rule read off the request alone fails here.
#[tokio::test]
async fn the_merged_export_config_is_what_the_export_rule_is_applied_to() {
    let router = emulator();

    // (case, the export config the group was created with, the members the request
    // carries, whether the call is accepted)
    let cases: Vec<(&str, Value, Value, bool)> = vec![
        (
            "an S3 export naming no destination at all",
            stored_export_config(),
            json!({"exportConfig": {"exportConfigType": "S3"}}),
            false,
        ),
        (
            "an S3 export whose destination names no bucket",
            stored_export_config(),
            json!({"exportConfig": {
                "exportConfigType": "S3",
                "s3Destination": {"path": "junit/"},
            }}),
            false,
        ),
        (
            "an S3 export whose bucket name is empty",
            stored_export_config(),
            json!({"exportConfig": {
                "exportConfigType": "S3",
                "s3Destination": {"bucket": ""},
            }}),
            false,
        ),
        (
            "an S3 export naming a bucket",
            json!({"exportConfigType": "NO_EXPORT"}),
            json!({"exportConfig": {
                "exportConfigType": "S3",
                "s3Destination": {"bucket": "accepted-bucket"},
            }}),
            true,
        ),
        (
            "no export at all",
            stored_export_config(),
            json!({"exportConfig": {"exportConfigType": "NO_EXPORT"}}),
            true,
        ),
        // The STORED side of the merge is what carries the destination here: a request
        // that touches only the tags leaves a valid `S3` export in place.
        (
            "tags alone, over a stored S3 export",
            stored_export_config(),
            json!({"tags": []}),
            true,
        ),
        // ...and the same request over a stored `NO_EXPORT` group is equally fine.
        (
            "tags alone, over a stored NO_EXPORT config",
            json!({"exportConfigType": "NO_EXPORT"}),
            json!({"tags": []}),
            true,
        ),
    ];

    for (index, (case, export_config, request, accepted)) in cases.into_iter().enumerate() {
        let name = format!("merged-{index:02}");
        let (status, body) = call(
            &router,
            "CreateReportGroup",
            create_report_group_body(&name, "TEST", export_config, Some(stored_tags())),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: seeding expected 200, got {status} with {body}"
        );
        let created = body["reportGroup"].clone();
        let arn = created["arn"].as_str().unwrap_or_default().to_string();

        let (status, body) = update_report_group(&router, &arn, request).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("exportConfig.s3Destination.bucket"),
            "case {case:?}: the refusal must name the member at fault, got {message:?}"
        );

        // A refused update leaves the report group exactly as it was.
        let fetched = fetch_report_group(&router, &arn).await;
        assert_eq!(
            fetched, created,
            "case {case:?}: a refused update must persist nothing, got {fetched}"
        );
    }
}

/// RULE (invariant): `lastModified` is strictly increasing across the successful updates
/// of one report group, while its identity — `name`, `type`, `arn`, `created` and the
/// read-only `status` — never moves, whatever the update carried.
#[tokio::test]
async fn repeated_updates_keep_the_report_group_identity_and_advance_last_modified() {
    let router = emulator();
    let (arn, created) = seeded_report_group(&router, "repeated-updates", "TEST").await;

    let requests = [
        json!({}),
        json!({"tags": []}),
        json!({"exportConfig": {"exportConfigType": "NO_EXPORT"}}),
        json!({"tags": [{"key": "team", "value": "platform"}]}),
        json!({"exportConfig": {
            "exportConfigType": "S3",
            "s3Destination": {"bucket": "final-bucket"},
        }}),
    ];

    let mut previous = created.clone();
    for (index, request) in requests.iter().enumerate() {
        let case = format!("update {index}");
        let updated = update_ok(&router, &case, &arn, request.clone()).await;
        for member in ["name", "type", "arn", "created", "status"] {
            assert_eq!(
                updated[member], created[member],
                "case {case:?}: {member} must never move, got {updated}"
            );
        }
        assert!(
            instant(&updated, "lastModified") > instant(&previous, "lastModified"),
            "case {case:?}: lastModified must advance past {}, got {}",
            instant(&previous, "lastModified"),
            instant(&updated, "lastModified")
        );
        previous = updated;
    }

    // What the last update stored is what the store holds, read twice: a read never
    // changes what a later caller sees.
    for read in ["first", "second"] {
        let fetched = fetch_report_group(&router, &arn).await;
        assert_eq!(
            fetched, previous,
            "the {read} read must report the record the last update stored, got {fetched}"
        );
    }
}
