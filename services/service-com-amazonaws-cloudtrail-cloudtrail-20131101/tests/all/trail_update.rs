// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateTrail`.
//!
//! Every case goes over the wire through the shared `super::emulator` harness, so
//! what is asserted here is what a client sees — including the HTTP status and the
//! error code.
//!
//! The rule under test, from the model's `UpdateTrail` contract: every member of the
//! request besides `Name` is optional, the members the request CARRIES replace the
//! stored values, the members it LEAVES OUT keep theirs, and the response reports the
//! stored trail — which is why a repeated call changes nothing (`@idempotent`). The
//! trail is addressed as a bare name or a trail ARN like every trail operation, and
//! the call runs only in the Region the trail was created in ("UpdateTrail must be
//! called from the Region in which the trail was created; otherwise, an
//! InvalidHomeRegionException is thrown").

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state;

use super::emulator::{Emulator, HOME_REGION};

/// A member of `value`, with an absent member read as `null` — so "this member was
/// dropped" and "this member is unset" are the same assertion.
fn member(value: &Value, name: &str) -> Value {
    value.get(name).cloned().unwrap_or(Value::Null)
}

/// The response member a request member writes.
///
/// Three request members are named differently in the response: the model calls the
/// request's `EnableLogFileValidation` `LogFileValidationEnabled` on the trail, the
/// request's `SnsTopicName` is reported as the topic's ARN (`SnsTopicARN`, the
/// response's own `SnsTopicName` being deprecated), and `Name` addresses the trail
/// rather than changing anything.
fn response_member(request_member: &str) -> Option<&'static str> {
    match request_member {
        "Name" => None,
        "EnableLogFileValidation" => Some("LogFileValidationEnabled"),
        "SnsTopicName" => Some("SnsTopicARN"),
        "S3BucketName" => Some("S3BucketName"),
        "S3KeyPrefix" => Some("S3KeyPrefix"),
        "IncludeGlobalServiceEvents" => Some("IncludeGlobalServiceEvents"),
        "IsMultiRegionTrail" => Some("IsMultiRegionTrail"),
        "CloudWatchLogsLogGroupArn" => Some("CloudWatchLogsLogGroupArn"),
        "CloudWatchLogsRoleArn" => Some("CloudWatchLogsRoleArn"),
        "KmsKeyId" => Some("KmsKeyId"),
        "IsOrganizationTrail" => Some("IsOrganizationTrail"),
        "RecursiveLogging" => Some("RecursiveLogging"),
        other => panic!("no UpdateTrail request member {other}"),
    }
}

/// The trail every case in [`updating_a_trail_replaces_only_the_members_it_carries`]
/// starts from: every optional member set, so each row can be seen to change one
/// group of them and leave the rest alone.
fn full_trail(name: &str) -> Value {
    json!({
        "Name": name,
        "S3BucketName": "bucket-initial",
        "S3KeyPrefix": "logs/initial",
        "SnsTopicName": "InitialTopic",
        "IncludeGlobalServiceEvents": true,
        "IsMultiRegionTrail": true,
        "EnableLogFileValidation": true,
        "IsOrganizationTrail": true,
        "RecursiveLogging": true,
        "KmsKeyId": "alias/InitialKey",
        "CloudWatchLogsLogGroupArn": "arn:aws:logs:us-east-1:000000000000:log-group:initial:*",
        "CloudWatchLogsRoleArn": "arn:aws:iam::000000000000:role/initial",
    })
}

/// The members `UpdateTrail` reports, as the model's `UpdateTrailResponse` lists
/// them.
const RESPONSE_MEMBERS: [&str; 13] = [
    "Name",
    "S3BucketName",
    "S3KeyPrefix",
    "SnsTopicARN",
    "IncludeGlobalServiceEvents",
    "IsMultiRegionTrail",
    "TrailARN",
    "LogFileValidationEnabled",
    "CloudWatchLogsLogGroupArn",
    "CloudWatchLogsRoleArn",
    "KmsKeyId",
    "IsOrganizationTrail",
    "RecursiveLogging",
];

/// An update changes exactly the members the request carries, and the response is
/// the stored trail: the rows differ in which members are present, in their type
/// (string and boolean), in each boolean's direction, in the form of `SnsTopicName`
/// (bare name vs ARN vs the empty value that drops the topic), in a value at the
/// documented length bound, and in how many members one request carries — including
/// none at all.
#[tokio::test]
async fn updating_a_trail_replaces_only_the_members_it_carries() {
    let emulator = Emulator::new();
    let name = "update-me";
    let created = emulator.call("CreateTrail", &full_trail(name)).await;
    let mut previous = created.expect_ok("CreateTrail").clone();

    // (case, the members the request carries besides Name, the response members it
    // is expected to change)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "one string member",
            json!({"S3BucketName": "bucket-second"}),
            json!({"S3BucketName": "bucket-second"}),
        ),
        (
            "every boolean turned the other way",
            json!({
                "IncludeGlobalServiceEvents": false,
                "IsMultiRegionTrail": false,
                "EnableLogFileValidation": false,
                "IsOrganizationTrail": false,
                "RecursiveLogging": false,
            }),
            json!({
                "IncludeGlobalServiceEvents": false,
                "IsMultiRegionTrail": false,
                "LogFileValidationEnabled": false,
                "IsOrganizationTrail": false,
                "RecursiveLogging": false,
            }),
        ),
        (
            "the same booleans turned back on",
            json!({"IsMultiRegionTrail": true, "EnableLogFileValidation": true}),
            json!({"IsMultiRegionTrail": true, "LogFileValidationEnabled": true}),
        ),
        (
            "an SNS topic given by name is completed into an ARN",
            json!({"SnsTopicName": "SecondTopic"}),
            json!({"SnsTopicARN": "arn:aws:sns:us-east-1:000000000000:SecondTopic"}),
        ),
        (
            "an SNS topic given as an ARN is kept",
            json!({"SnsTopicName": "arn:aws:sns:eu-west-1:000000000000:OtherTopic"}),
            json!({"SnsTopicARN": "arn:aws:sns:eu-west-1:000000000000:OtherTopic"}),
        ),
        (
            "an empty SnsTopicName names no topic, so the trail keeps none",
            json!({"SnsTopicName": ""}),
            json!({"SnsTopicARN": null}),
        ),
        (
            "an empty KmsKeyId names no key, so the trail keeps none",
            json!({"KmsKeyId": ""}),
            json!({"KmsKeyId": null}),
        ),
        (
            "the CloudWatch Logs pair at once",
            json!({
                "CloudWatchLogsLogGroupArn":
                    "arn:aws:logs:us-east-1:000000000000:log-group:second:*",
                "CloudWatchLogsRoleArn": "arn:aws:iam::000000000000:role/second",
            }),
            json!({
                "CloudWatchLogsLogGroupArn":
                    "arn:aws:logs:us-east-1:000000000000:log-group:second:*",
                "CloudWatchLogsRoleArn": "arn:aws:iam::000000000000:role/second",
            }),
        ),
        (
            // The bound is `state::S3_KEY_PREFIX_MAX_LEN`, from the model doc for
            // `S3KeyPrefix`: "The maximum length is 200 characters."
            "an S3KeyPrefix at the documented maximum length",
            json!({"S3KeyPrefix": "p".repeat(state::S3_KEY_PREFIX_MAX_LEN)}),
            json!({"S3KeyPrefix": "p".repeat(state::S3_KEY_PREFIX_MAX_LEN)}),
        ),
        (
            "a request that carries nothing but Name changes nothing",
            json!({}),
            json!({}),
        ),
        (
            "the whole configuration at once",
            json!({
                "S3BucketName": "bucket-last",
                "S3KeyPrefix": "logs/last",
                "SnsTopicName": "LastTopic",
                "IncludeGlobalServiceEvents": false,
                "IsMultiRegionTrail": false,
                "EnableLogFileValidation": false,
                "IsOrganizationTrail": false,
                "RecursiveLogging": false,
                "KmsKeyId": "alias/LastKey",
                "CloudWatchLogsLogGroupArn":
                    "arn:aws:logs:us-east-1:000000000000:log-group:last:*",
                "CloudWatchLogsRoleArn": "arn:aws:iam::000000000000:role/last",
            }),
            json!({
                "S3BucketName": "bucket-last",
                "S3KeyPrefix": "logs/last",
                "SnsTopicARN": "arn:aws:sns:us-east-1:000000000000:LastTopic",
                "IncludeGlobalServiceEvents": false,
                "IsMultiRegionTrail": false,
                "LogFileValidationEnabled": false,
                "IsOrganizationTrail": false,
                "RecursiveLogging": false,
                "KmsKeyId": "alias/LastKey",
                "CloudWatchLogsLogGroupArn":
                    "arn:aws:logs:us-east-1:000000000000:log-group:last:*",
                "CloudWatchLogsRoleArn": "arn:aws:iam::000000000000:role/last",
            }),
        ),
    ];

    for (case, patch, expected) in rows {
        let mut request = patch.as_object().expect("an object of members").clone();
        request.insert("Name".to_string(), json!(name));
        let updated = emulator.call("UpdateTrail", &Value::Object(request)).await;
        let updated = updated.expect_ok(case).clone();

        // The members the request carries now read as the request wrote them.
        let expected = expected.as_object().expect("an object of expectations");
        for (response_name, want) in expected {
            assert_eq!(
                &member(&updated, response_name),
                want,
                "case {case}: expected {response_name} = {want}, got {updated:?}"
            );
        }

        // Every other member keeps the value it had before this call — including
        // the identity members, which this request cannot carry at all.
        let changed: Vec<&'static str> = patch
            .as_object()
            .expect("an object")
            .keys()
            .filter_map(|request_name| response_member(request_name))
            .collect();
        for response_name in RESPONSE_MEMBERS {
            if changed.contains(&response_name) {
                continue;
            }
            assert_eq!(
                member(&updated, response_name),
                member(&previous, response_name),
                "case {case}: {response_name} moved although the request did not \
                 carry it, got {updated:?}"
            );
        }
        assert_eq!(
            member(&updated, "TrailARN"),
            json!(format!(
                "arn:aws:cloudtrail:us-east-1:000000000000:trail/{name}"
            )),
            "case {case}: the trail ARN moved, got {updated:?}"
        );
        // The deprecated response member stays unreported ("This field is no longer
        // in use. Use SnsTopicARN.").
        assert_eq!(
            member(&updated, "SnsTopicName"),
            Value::Null,
            "case {case}: the deprecated SnsTopicName was reported, got {updated:?}"
        );

        // The update was PERSISTED: a read reports exactly what the update reported.
        let fetched = emulator.call("GetTrail", &json!({"Name": name})).await;
        let trail = fetched.expect_ok(case)["Trail"].clone();
        for response_name in RESPONSE_MEMBERS {
            assert_eq!(
                member(&trail, response_name),
                member(&updated, response_name),
                "case {case}: GetTrail reports {response_name} = {}, where UpdateTrail \
                 reported {}, got {trail:?}",
                member(&trail, response_name),
                member(&updated, response_name)
            );
        }
        assert_eq!(
            member(&trail, "HomeRegion"),
            json!(HOME_REGION),
            "case {case}: the home Region moved, got {trail:?}"
        );

        // `@idempotent`: the same request again answers the same trail.
        let mut again_request = patch.as_object().expect("an object").clone();
        again_request.insert("Name".to_string(), json!(name));
        let again = emulator
            .call("UpdateTrail", &Value::Object(again_request))
            .await;
        let again = again.expect_ok(&format!("{case}, repeated")).clone();
        assert_eq!(
            again, updated,
            "case {case}: repeating the update answered something else, got {again:?}"
        );

        previous = updated;
    }
}

/// An update is refused for the same reasons every trail operation refuses one — a
/// malformed identifier as the form it was written in, an unknown trail as a
/// not-found, a value past a documented bound as that bound's own error, a call from
/// outside the home Region as a home-Region error — and a refused update leaves the
/// stored trail exactly as it was.
#[tokio::test]
async fn an_update_is_refused_without_changing_the_trail() {
    let emulator = Emulator::new();
    let name = "refusals";
    let created = emulator.call("CreateTrail", &full_trail(name)).await;
    let stored = created.expect_ok("CreateTrail").clone();

    // (case, Region the call is made in, request, status, error code)
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        // The documented trail-name rules, one broken rule per row; `my--namespace`
        // is the model's own example of adjacent punctuation.
        (
            "adjacent dashes in the name",
            HOME_REGION,
            json!({"Name": "my--namespace"}),
            400,
            "InvalidTrailNameException",
        ),
        (
            "adjacent periods in the name",
            HOME_REGION,
            json!({"Name": "my..namespace"}),
            400,
            "InvalidTrailNameException",
        ),
        (
            "a name one character under the documented minimum",
            HOME_REGION,
            json!({"Name": "a".repeat(state::TRAIL_NAME_MIN_LEN - 1)}),
            400,
            "InvalidTrailNameException",
        ),
        (
            "a name one character over the documented maximum",
            HOME_REGION,
            json!({"Name": "a".repeat(state::TRAIL_NAME_MAX_LEN + 1)}),
            400,
            "InvalidTrailNameException",
        ),
        (
            "a name in IP address format",
            HOME_REGION,
            json!({"Name": "192.168.5.4"}),
            400,
            "InvalidTrailNameException",
        ),
        (
            "a name outside ASCII",
            HOME_REGION,
            json!({"Name": "hás-accent"}),
            400,
            "InvalidTrailNameException",
        ),
        (
            "an ARN that is not a trail ARN",
            HOME_REGION,
            json!({"Name": "arn:aws:cloudtrail:us-east-1:000000000000:channel/01234567890"}),
            400,
            "CloudTrailARNInvalidException",
        ),
        (
            "an ARN missing a field",
            HOME_REGION,
            json!({"Name": "arn:aws:cloudtrail:us-east-1:trail/refusals"}),
            400,
            "CloudTrailARNInvalidException",
        ),
        (
            "a well-formed name naming no trail",
            HOME_REGION,
            json!({"Name": "absent-trail"}),
            404,
            "TrailNotFoundException",
        ),
        (
            "a well-formed trail ARN naming no trail",
            HOME_REGION,
            json!({"Name": "arn:aws:cloudtrail:us-east-1:000000000000:trail/absent-trail"}),
            404,
            "TrailNotFoundException",
        ),
        (
            // `state::S3_KEY_PREFIX_MAX_LEN`, one character past the documented
            // maximum the accepted row above sits on.
            "an S3KeyPrefix one character over the documented maximum",
            HOME_REGION,
            json!({"Name": name, "S3KeyPrefix": "p".repeat(state::S3_KEY_PREFIX_MAX_LEN + 1)}),
            400,
            "InvalidS3PrefixException",
        ),
        (
            // `state::SNS_TOPIC_NAME_MAX_LEN`, from the model doc for
            // `SnsTopicName`: "The maximum length is 256 characters."
            "an SnsTopicName one character over the documented maximum",
            HOME_REGION,
            json!({"Name": name, "SnsTopicName": "t".repeat(state::SNS_TOPIC_NAME_MAX_LEN + 1)}),
            400,
            "InvalidSnsTopicNameException",
        ),
        (
            "the trail's own ARN, from another Region",
            "eu-west-1",
            json!({
                "Name": format!("arn:aws:cloudtrail:us-east-1:000000000000:trail/{name}"),
                "S3BucketName": "bucket-elsewhere",
            }),
            400,
            "InvalidHomeRegionException",
        ),
        (
            "the trail's bare name, from another Region",
            "eu-west-1",
            json!({"Name": name, "S3BucketName": "bucket-elsewhere"}),
            400,
            "InvalidHomeRegionException",
        ),
    ];

    for (case, region, request, status, code) in rows {
        emulator
            .call_in("UpdateTrail", region, &request)
            .await
            .expect_error(case, status, code);

        // Nothing the refusal touched: the trail still reads as it was created.
        let fetched = emulator.call("GetTrail", &json!({"Name": name})).await;
        let trail = fetched.expect_ok(case)["Trail"].clone();
        for response_name in RESPONSE_MEMBERS {
            assert_eq!(
                member(&trail, response_name),
                member(&stored, response_name),
                "case {case}: the refused update changed {response_name} to {}, got {trail:?}",
                member(&trail, response_name)
            );
        }
    }

    // A value at the documented maximum SNS topic name length is accepted, so the
    // row above refuses the bound and not the member.
    let at_bound = "t".repeat(state::SNS_TOPIC_NAME_MAX_LEN);
    let updated = emulator
        .call(
            "UpdateTrail",
            &json!({"Name": name, "SnsTopicName": at_bound}),
        )
        .await;
    assert_eq!(
        member(updated.expect_ok("an SnsTopicName at the documented maximum"), "SnsTopicARN"),
        json!(format!("arn:aws:sns:us-east-1:000000000000:{at_bound}")),
        "case an SnsTopicName at the documented maximum: expected the completed topic ARN, \
         got {updated:?}"
    );
}

/// A trail-name refusal names the ONE rule the name broke, and a length bound also
/// names the bound and the name's own length — the same text whichever operation the
/// name was given to, because the rule belongs to the identifier and not to the
/// operation.
#[tokio::test]
async fn a_refused_trail_name_names_the_rule_it_broke() {
    let emulator = Emulator::new();
    // (case, name, the fragments the refusal must carry) — one row per documented
    // rule, each name breaking that rule first.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "under the documented minimum length",
            "a".repeat(state::TRAIL_NAME_MIN_LEN - 1),
            vec![
                "too short".to_string(),
                format!("Minimum allowed length: {} characters", state::TRAIL_NAME_MIN_LEN),
                format!(
                    "Specified name length: {} characters",
                    state::TRAIL_NAME_MIN_LEN - 1
                ),
            ],
        ),
        (
            "over the documented maximum length",
            "a".repeat(state::TRAIL_NAME_MAX_LEN + 1),
            vec![
                "too long".to_string(),
                format!("Maximum allowed length: {} characters", state::TRAIL_NAME_MAX_LEN),
                format!(
                    "Specified name length: {} characters",
                    state::TRAIL_NAME_MAX_LEN + 1
                ),
            ],
        ),
        (
            "a character outside the allowed set",
            "has space".to_string(),
            vec!["can only contain".to_string()],
        ),
        (
            "punctuation at the start",
            "-leading".to_string(),
            vec!["must start with a letter or number".to_string()],
        ),
        (
            "punctuation at the end",
            "trailing-".to_string(),
            vec!["must end with a letter or number".to_string()],
        ),
        (
            "adjacent dashes",
            "my--namespace".to_string(),
            vec!["adjacent".to_string()],
        ),
        (
            "adjacent periods",
            "my..namespace".to_string(),
            vec!["adjacent".to_string()],
        ),
        (
            "IP address format",
            "192.168.5.4".to_string(),
            vec!["formatted as an IP address".to_string()],
        ),
    ];

    for (case, name, fragments) in rows {
        // The rule belongs to the identifier, so every operation that takes one
        // refuses the name the same way.
        for operation in ["UpdateTrail", "GetTrail", "CreateTrail", "DeleteTrail"] {
            let reply = emulator
                .call(
                    operation,
                    &json!({"Name": name, "S3BucketName": "bucket-messages"}),
                )
                .await;
            let case = format!("{operation} with a name {case}");
            reply.expect_error(&case, 400, "InvalidTrailNameException");
            let message = reply
                .body
                .get("Message")
                .or_else(|| reply.body.get("message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            for fragment in &fragments {
                assert!(
                    message.contains(fragment.as_str()),
                    "case {case}: expected the refusal to name {fragment:?}, got {message:?}"
                );
            }
        }
    }
}

/// A well-formed identifier naming no trail is refused with the identifier EXACTLY
/// as the caller wrote it — a bare name or a trail ARN — and the account the lookup
/// ran in, the same text whichever operation was called.
#[tokio::test]
async fn an_unknown_trail_is_refused_naming_the_identifier_as_given() {
    let emulator = Emulator::new();
    let identifiers = [
        "absent-trail".to_string(),
        format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/absent-trail"),
    ];
    for identifier in &identifiers {
        for operation in ["UpdateTrail", "GetTrail", "GetTrailStatus", "DeleteTrail"] {
            let reply = emulator.call(operation, &json!({"Name": identifier})).await;
            let case = format!("{operation} with the unknown trail {identifier}");
            reply.expect_error(&case, 404, "TrailNotFoundException");
            let message = reply
                .body
                .get("Message")
                .or_else(|| reply.body.get("message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let expected = format!("Unknown trail: {identifier} for the user: 000000000000");
            assert_eq!(
                message, expected,
                "case {case}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// A trail is addressed by its ARN as well as by its name, and an update leaves the
/// state other operations own — the logging flag `StartLogging` / `StopLogging` move
/// — alone.
#[tokio::test]
async fn an_update_addresses_a_trail_by_arn_and_leaves_other_state_alone() {
    let emulator = Emulator::new();
    let name = "by-arn-update";
    let region = "us-west-2";
    emulator
        .call_in(
            "CreateTrail",
            region,
            &json!({"Name": name, "S3BucketName": "bucket-arn"}),
        )
        .await
        .expect_ok("CreateTrail");
    let arn = format!("arn:aws:cloudtrail:{region}:000000000000:trail/{name}");

    emulator
        .call_in("StartLogging", region, &json!({"Name": name}))
        .await
        .expect_ok("StartLogging");

    let updated = emulator
        .call_in(
            "UpdateTrail",
            region,
            &json!({"Name": arn, "S3BucketName": "bucket-arn-2", "IsMultiRegionTrail": true}),
        )
        .await;
    let updated = updated.expect_ok("UpdateTrail by ARN").clone();
    let expected: Map<String, Value> = json!({
        "Name": name,
        "TrailARN": arn,
        "S3BucketName": "bucket-arn-2",
        "IsMultiRegionTrail": true,
    })
    .as_object()
    .expect("an object")
    .clone();
    for (response_name, want) in &expected {
        assert_eq!(
            &member(&updated, response_name),
            want,
            "case UpdateTrail by ARN: expected {response_name} = {want}, got {updated:?}"
        );
    }

    // The logging state belongs to Start/StopLogging: the update does not move it.
    let status = emulator
        .call_in("GetTrailStatus", region, &json!({"Name": name}))
        .await;
    let status = status.expect_ok("GetTrailStatus after UpdateTrail").clone();
    assert_eq!(
        member(&status, "IsLogging"),
        json!(true),
        "case GetTrailStatus after UpdateTrail: expected IsLogging true, got {status:?}"
    );
    let start_logging_time = member(&status, "StartLoggingTime");
    assert!(
        start_logging_time.as_f64().is_some_and(|t| t > 0.0),
        "case GetTrailStatus after UpdateTrail: expected the instant logging started, \
         got {status:?}"
    );
}
