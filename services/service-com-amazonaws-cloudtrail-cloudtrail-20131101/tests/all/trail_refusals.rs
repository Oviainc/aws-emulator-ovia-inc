// SPDX-License-Identifier: CC0-1.0

//! How the trail surface refuses a caller-input error, end to end.
//!
//! Every case goes over the wire through the production router (see
//! `tests/all/emulator.rs`), so what is asserted is what a client sees: the HTTP
//! status, the error code it branches on, and the `Message` it reads.
//!
//! The three rules under test, each held by EVERY operation that takes a trail
//! identifier rather than by one of them:
//!
//! * a value written as an ARN that is not a trail ARN
//!   (`arn:<partition>:cloudtrail:<region>:<account>:trail/<name>`) is refused with
//!   the modelled `CloudTrailARNInvalidException` (HTTP 400), naming the ARN as the
//!   caller wrote it;
//! * a bare name outside the rules the model documents for `CreateTrailRequest$Name`
//!   is refused with `InvalidTrailNameException` (HTTP 400), naming the ONE rule the
//!   name broke;
//! * a well-formed identifier that names no stored trail is refused with
//!   `TrailNotFoundException`, naming the identifier and the account — with HTTP 400,
//!   the status CloudTrail answers with, not the `@httpError(404)` its model
//!   annotates (`state::TRAIL_NOT_FOUND_STATUS`).
//!
//! None of the three is a 501: a scaffold signal would tell an SDK that a caller
//! input error is a server fault.

use serde_json::json;

use super::emulator::{Emulator, HOME_REGION};

/// Every implemented operation whose single trail identifier is `Name`, which is
/// where each rule below has to hold. `CreateTrail` takes a bare name only, so it
/// joins the name table but not the ARN one.
const IDENTIFIER_OPERATIONS: [&str; 5] = [
    "GetTrail",
    "GetTrailStatus",
    "DeleteTrail",
    "StartLogging",
    "StopLogging",
];

/// The documented trail-name rules, from the model doc for `CreateTrailRequest$Name`
/// ("Contain only ASCII letters (a-z, A-Z), numbers (0-9), periods (.), underscores
/// (_), or dashes (-) / Start with a letter or number, and end with a letter or
/// number / Be between 3 and 128 characters / Have no adjacent periods, underscores
/// or dashes … / Not be in IP address format"): one row per rule, at the bound and
/// one past it where the rule is a bound, plus the two names the model itself gives
/// as invalid examples.
///
/// (case, the name, a phrase the refusal must name the broken rule with)
fn invalid_name_rows() -> Vec<(&'static str, String, &'static str)> {
    vec![
        (
            "one character under the documented minimum of 3",
            "ab".to_string(),
            "too short",
        ),
        (
            "one character over the documented maximum of 128",
            "a".repeat(129),
            "too long",
        ),
        (
            "a space, which is outside the documented character set",
            "probe trail".to_string(),
            "ASCII letters",
        ),
        (
            "a non-ASCII letter",
            "prôbe-trail".to_string(),
            "ASCII letters",
        ),
        (
            "a leading dash",
            "-probe-trail".to_string(),
            "start with a letter or number",
        ),
        (
            "a trailing period",
            "probe-trail.".to_string(),
            "end with a letter or number",
        ),
        (
            "the model's own adjacent-punctuation example my--namespace",
            "my--namespace".to_string(),
            "adjacent",
        ),
        (
            "the model's other adjacent-punctuation example my-_namespace",
            "my-_namespace".to_string(),
            "adjacent",
        ),
        (
            "the model's own IP-address example",
            "192.168.5.4".to_string(),
            "IP address",
        ),
        (
            "another dotted quad",
            "10.0.0.1".to_string(),
            "IP address",
        ),
    ]
}

/// A name outside the documented rules is refused by EVERY operation that takes a
/// trail identifier, with the modelled `InvalidTrailNameException` (HTTP 400) whose
/// text names the one rule the name broke.
#[tokio::test]
async fn an_invalid_trail_name_is_refused_with_the_rule_it_broke() {
    let emulator = Emulator::new();

    for (case, name, rule) in invalid_name_rows() {
        for operation in IDENTIFIER_OPERATIONS {
            let case = format!("{operation} with {case}");
            let reply = emulator.call(operation, &json!({"Name": name})).await;
            reply.expect_error(&case, 400, "InvalidTrailNameException");
            let actual = reply.message();
            assert!(
                actual.contains(rule),
                "case {case:?}: expected a refusal naming {rule:?}, got {actual:?}"
            );
        }

        // `CreateTrail` reads the same name through its own validation.
        let case = format!("CreateTrail with {case}");
        let reply = emulator
            .call(
                "CreateTrail",
                &json!({"Name": name, "S3BucketName": "probe-bucket"}),
            )
            .await;
        reply.expect_error(&case, 400, "InvalidTrailNameException");
        let actual = reply.message();
        assert!(
            actual.contains(rule),
            "case {case:?}: expected a refusal naming {rule:?}, got {actual:?}"
        );
    }

    // The bounds themselves are valid, so the rows above refuse a rule and not a
    // whole class: 3 and 128 characters are the documented minimum and maximum.
    for (case, name) in [
        ("the documented minimum of 3", "abc".to_string()),
        ("the documented maximum of 128", "a".repeat(128)),
    ] {
        emulator
            .call(
                "CreateTrail",
                &json!({"Name": name, "S3BucketName": "probe-bucket"}),
            )
            .await
            .expect_ok(&format!("CreateTrail with {case}"));
    }
}

/// A value written as an ARN that is not a trail ARN is refused with the modelled
/// `CloudTrailARNInvalidException` (HTTP 400), and the refusal names the ARN the
/// caller wrote — the rows differ in WHICH part of the ARN is wrong.
#[tokio::test]
async fn a_malformed_trail_arn_is_refused_naming_the_arn() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();

    // (case, the identifier) — each breaks one part of
    // `arn:<partition>:cloudtrail:<region>:<account>:trail/<name>`.
    let rows: Vec<(&str, String)> = vec![
        (
            "a resource part that is not trail/<name>",
            format!("arn:aws:cloudtrail:us-west-2:{account}:badtrail"),
        ),
        (
            "a resource part missing the separating slash",
            format!("arn:aws:cloudtrail:us-west-2:{account}:trailwrong"),
        ),
        (
            "another CloudTrail resource kind",
            format!("arn:aws:cloudtrail:us-west-2:{account}:eventdatastore/probe"),
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::probe-bucket".to_string(),
        ),
        (
            "too few fields",
            format!("arn:aws:cloudtrail:{account}:trail/probe"),
        ),
        (
            "an empty Region",
            format!("arn:aws:cloudtrail::{account}:trail/probe"),
        ),
        (
            "an empty account",
            "arn:aws:cloudtrail:us-west-2::trail/probe".to_string(),
        ),
        (
            "a trail name inside the ARN that breaks the name rules",
            format!("arn:aws:cloudtrail:us-west-2:{account}:trail/my--namespace"),
        ),
        ("nothing but the arn prefix", "arn:".to_string()),
    ];

    for (case, identifier) in rows {
        for operation in IDENTIFIER_OPERATIONS {
            let case = format!("{operation} with {case}");
            let reply = emulator.call(operation, &json!({"Name": identifier})).await;
            reply.expect_error(&case, 400, "CloudTrailARNInvalidException");
            let actual = reply.message();
            assert!(
                actual.contains(&identifier),
                "case {case:?}: expected a refusal naming {identifier:?}, got {actual:?}"
            );
        }
    }

    // A well-formed trail ARN is NOT refused as malformed: it resolves (and is a
    // not-found when it names no trail, which the next case pins).
    let well_formed = format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:trail/probe-trail");
    let reply = emulator
        .call("GetTrail", &json!({"Name": well_formed}))
        .await;
    reply.expect_error(
        "GetTrail with a well-formed trail ARN",
        400,
        "TrailNotFoundException",
    );
}

/// A well-formed identifier that names no stored trail is a not-found, on every
/// operation that takes one: HTTP 400 with `TrailNotFoundException`, naming the
/// trail and the account the lookup ran in. `GetTrail` names the identifier as the
/// caller wrote it; the four operations that resolve the identifier to one Region's
/// trail name that trail's ARN.
#[tokio::test]
async fn an_absent_trail_is_refused_as_not_found_naming_it() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    // One stored trail, so a not-found is "this trail" and not "no trails".
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": "probe-present", "S3BucketName": "probe-bucket"}),
        )
        .await
        .expect_ok("CreateTrail probe-present");

    // (case, the identifier, the Region it addresses, the trail name it names) —
    // a bare name and an ARN, at the length bounds, in the caller's Region and in
    // another one.
    let rows: Vec<(&str, String, &str, String)> = vec![
        (
            "a bare name never created",
            "probe-absent".to_string(),
            HOME_REGION,
            "probe-absent".to_string(),
        ),
        (
            "a bare name at the minimum length",
            "abc".to_string(),
            HOME_REGION,
            "abc".to_string(),
        ),
        (
            "a bare name at the maximum length",
            "z".repeat(128),
            HOME_REGION,
            "z".repeat(128),
        ),
        (
            "the stored trail's name in another Region",
            format!("arn:aws:cloudtrail:eu-west-1:{account}:trail/probe-present"),
            "eu-west-1",
            "probe-present".to_string(),
        ),
        (
            "a trail ARN never created",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:trail/probe-absent"),
            HOME_REGION,
            "probe-absent".to_string(),
        ),
    ];

    for (case, identifier, addressed, name) in rows {
        // The operations that resolve the identifier to one Region's trail name
        // that trail's ARN, whichever form the caller wrote.
        let arn = format!("arn:aws:cloudtrail:{addressed}:{account}:trail/{name}");
        for operation in ["GetTrailStatus", "StartLogging", "StopLogging", "DeleteTrail"] {
            // Logging and deletion run in the trail's home Region only, so they
            // read an ARN from another Region as a home-Region refusal instead.
            if addressed != HOME_REGION && operation != "GetTrailStatus" {
                continue;
            }
            let case = format!("{operation} with {case}");
            let reply = emulator.call(operation, &json!({"Name": identifier})).await;
            reply.expect_error(&case, 400, "TrailNotFoundException");
            let actual = reply.message();
            assert!(
                actual.contains(&arn) && actual.contains(account),
                "case {case:?}: expected a refusal naming the trail ARN {arn:?} and the \
                 account {account:?}, got {actual:?}"
            );
        }

        // `GetTrail` resolves no ARN: it names the identifier as given.
        let case = format!("GetTrail with {case}");
        let reply = emulator.call("GetTrail", &json!({"Name": identifier})).await;
        reply.expect_error(&case, 400, "TrailNotFoundException");
        let actual = reply.message();
        assert!(
            actual.contains(&identifier) && actual.contains(account),
            "case {case:?}: expected a refusal naming {identifier:?} and the account \
             {account:?}, got {actual:?}"
        );
    }

    // The stored trail is still there: a refused read changed nothing.
    let present = emulator
        .call("GetTrail", &json!({"Name": "probe-present"}))
        .await;
    assert_eq!(
        present.expect_ok("GetTrail probe-present")["Trail"]["Name"],
        json!("probe-present"),
        "case GetTrail probe-present: the stored trail did not survive the refusals, \
         got {present:?}"
    );
}
