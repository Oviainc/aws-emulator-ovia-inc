// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service default-application setting:
//! `PutDefaultApplicationSetting` and `GetDefaultApplicationSetting`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it, only the two
//! paths it drives.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the setting round-trips whatever ARN is written to it, over a table of ARNs
//!   differing in partition, Region and account fields, resource form and length —
//!   including each published length bound — with the read repeated to show it
//!   changes nothing and the next write replacing the previous value;
//! * while nothing is default the read is refused with the modelled
//!   `ResourceNotFoundException`, and a write that clears the setting answers with
//!   NO `applicationArn` member rather than an empty one;
//! * clearing is asked for by naming the ARN in effect: a clear naming any other
//!   application leaves the setting exactly as it was;
//! * a value that is not an ARN, and one past each published length bound, are
//!   refused with `ValidationException` and leave the stored setting untouched.

use serde_json::{json, Value};

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX};

/// `GetDefaultApplicationSetting`: `GET /opensearch/defaultApplicationSetting`.
async fn get(emulator: &Emulator) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/defaultApplicationSetting"),
            None,
        )
        .await
}

/// `PutDefaultApplicationSetting`: `PUT /opensearch/defaultApplicationSetting`.
async fn put(emulator: &Emulator, application_arn: &str, set_as_default: bool) -> Answer {
    put_body(
        emulator,
        json!({ "applicationArn": application_arn, "setAsDefault": set_as_default }),
    )
    .await
}

/// `PutDefaultApplicationSetting` with a body the caller composes, so a case can
/// leave a member out.
async fn put_body(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/defaultApplicationSetting"),
            Some(body),
        )
        .await
}

/// The `applicationArn` an answer reports, with the case named on failure.
fn reported_arn(answer: &Answer, case: &str) -> String {
    answer.body["applicationArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a string applicationArn, got {:?}",
                answer.body
            )
        })
        .to_owned()
}

/// A case label for a value that may be thousands of characters long: its first
/// characters, which is what tells the rows apart.
fn label(value: &str) -> String {
    value.chars().take(24).collect()
}

/// An ARN exactly as long as `ARN`'s published maximum (2048), built by padding the
/// resource of a well-formed application ARN.
fn longest_arn() -> String {
    let prefix = "arn:aws:es:us-east-1:123456789012:application/";
    format!("{prefix}{}", "a".repeat(2048 - prefix.len()))
}

/// The ARN table: one row per way one accepted ARN differs from another — the
/// partition, whether the Region and account fields are the usual ones, the resource
/// form (with a slash, with colons) and the length at each published bound.
///
/// `ARN` is `@length(min: 20, max: 2048)` with pattern `.*`, from this service's
/// Smithy model (`smithy_query(query="operation",operation="PutDefaultApplicationSetting")`),
/// so every row below is inside the modelled constraint and the service itself is
/// what accepts or refuses it.
fn arn_rows() -> Vec<String> {
    vec![
        // The ordinary form an application ARN takes.
        "arn:aws:es:us-east-1:123456789012:application/abc123".to_string(),
        // Another partition, Region and account.
        "arn:aws-cn:es:cn-north-1:210987654321:application/d4t4".to_string(),
        // A resource carrying colons: the resource is the ARN's last field, so the
        // colons belong to it.
        "arn:aws:es:eu-west-1:123456789012:application:with:colons".to_string(),
        // Exactly the shortest ARN the constraint allows (20 characters).
        "arn:aws:es:a:1:a/abc".to_string(),
        // Exactly the longest (2048 characters).
        longest_arn(),
    ]
}

#[tokio::test]
async fn the_default_application_setting_round_trips_whatever_arn_is_written_to_it() {
    let emulator = Emulator::new();

    let mut previous: Option<String> = None;
    for arn in arn_rows() {
        let case = format!("PutDefaultApplicationSetting({}..)", label(&arn));

        let written = put(&emulator, &arn, true).await;
        assert_eq!(
            written.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            written.status, written.body
        );
        assert_eq!(
            reported_arn(&written, &case),
            arn,
            "case {case}: the write must report the setting it put in effect, got {:?}",
            written.body
        );

        // Read back through the implemented reader: the setting holds what was
        // written, and the previous row's value is gone — the setting is ONE value,
        // so the last write wins.
        let read = get(&emulator).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from the read, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            reported_arn(&read, &case),
            arn,
            "case {case}: expected the written ARN back, got {:?}",
            read.body
        );
        if let Some(previous) = previous.as_deref() {
            assert_ne!(
                reported_arn(&read, &case),
                previous,
                "case {case}: the write must replace the previous default, got {:?}",
                read.body
            );
        }

        // The read reported the setting and changed nothing: a second one answers
        // exactly the same.
        let again = get(&emulator).await;
        assert_eq!(
            again.body, read.body,
            "case {case}: a read must not change what the next caller sees, got {:?} then {:?}",
            read.body, again.body
        );

        previous = Some(arn);
    }
}

#[tokio::test]
async fn an_unset_default_application_setting_is_not_found_and_clearing_reports_no_arn() {
    for arn in arn_rows() {
        let case = format!("clear({}..)", label(&arn));
        let emulator = Emulator::new();

        // Nothing is default yet, so the setting resolves to no resource.
        let unset = get(&emulator).await;
        assert_eq!(
            unset.status, 409,
            "case {case}: expected 409 while nothing is default, got {} with {:?}",
            unset.status, unset.body
        );
        assert_eq!(
            error_code(&unset),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {:?}",
            unset.body
        );

        put(&emulator, &arn, true).await;

        // Clearing names the ARN in effect, as the operation documents.
        let cleared = put(&emulator, &arn, false).await;
        assert_eq!(
            cleared.status, 200,
            "case {case}: expected 200 from the clear, got {} with {:?}",
            cleared.status, cleared.body
        );
        // Nothing is default any more, so the answer carries NO `applicationArn`
        // member — not the member with an empty value.
        assert!(
            cleared.body.get("applicationArn").is_none(),
            "case {case}: a cleared setting must report no applicationArn member, got {:?}",
            cleared.body
        );

        let after = get(&emulator).await;
        assert_eq!(
            after.status, 409,
            "case {case}: expected 409 after the clear, got {} with {:?}",
            after.status, after.body
        );
        assert_eq!(
            error_code(&after),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException after the clear, got {:?}",
            after.body
        );

        // The setting can be put back: clearing released the value, not the setting.
        let reset = put(&emulator, &arn, true).await;
        assert_eq!(
            reported_arn(&reset, &case),
            arn,
            "case {case}: the setting must be writable again after a clear, got {:?}",
            reset.body
        );
    }
}

#[tokio::test]
async fn clearing_an_application_that_is_not_the_default_leaves_the_setting_in_effect() {
    let emulator = Emulator::new();
    let rows = arn_rows();
    let in_effect = &rows[0];

    put(&emulator, in_effect, true).await;

    for other in rows.iter().skip(1) {
        let case = format!("clear-other({}..)", label(other));
        let answer = put(&emulator, other, false).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        // The answer reports the setting in effect after the call, which the request
        // did not name and so did not change.
        assert_eq!(
            &reported_arn(&answer, &case),
            in_effect,
            "case {case}: expected the untouched default back, got {:?}",
            answer.body
        );
        let read = get(&emulator).await;
        assert_eq!(
            &reported_arn(&read, &case),
            in_effect,
            "case {case}: clearing another application must leave the default alone, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_value_that_is_not_an_arn_is_refused_and_the_stored_setting_stands() {
    let emulator = Emulator::new();
    let in_effect = arn_rows()[0].clone();
    put(&emulator, &in_effect, true).await;

    // Inside `ARN`'s length bound (20..2048) and so past the boundary validator, yet
    // not an ARN: no `arn` literal, too few fields, or an empty partition, service or
    // resource field.
    let too_long = format!("{}x", longest_arn());
    let malformed: Vec<&str> = vec![
        "not-an-arn-at-all-xx",
        "arn:aws:es:us-east-1:application/abc",
        "arn:aws:es:us-east-1:123456789012:",
        "arn::es:us-east-1:123456789012:application/abc",
        "arn:aws::us-east-1:123456789012:application/abc",
        "ARN:aws:es:us-east-1:123456789012:application/abc",
        // One past each published length bound: refused at the request boundary.
        "arn:aws:es:a:1:a/ab",
        too_long.as_str(),
    ];

    for value in malformed {
        let case = format!("PutDefaultApplicationSetting({:?}..)", label(value));
        let answer = put(&emulator, value, true).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400 for a value that is not an ARN, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            answer.body
        );

        // A refused write stored nothing: the setting still holds what the accepted
        // write put in effect.
        let read = get(&emulator).await;
        assert_eq!(
            reported_arn(&read, &case),
            in_effect,
            "case {case}: a refused write must leave the setting as it was, got {:?}",
            read.body
        );
    }

    // `setAsDefault` is required, so a write that leaves it out is refused too, and
    // changes nothing either.
    let incomplete = put_body(&emulator, json!({ "applicationArn": in_effect })).await;
    assert_eq!(
        incomplete.status, 400,
        "case PutDefaultApplicationSetting(no setAsDefault): expected 400, got {} with {:?}",
        incomplete.status, incomplete.body
    );
    let read = get(&emulator).await;
    assert_eq!(
        reported_arn(&read, "no setAsDefault"),
        in_effect,
        "case PutDefaultApplicationSetting(no setAsDefault): the setting must stand, got {:?}",
        read.body
    );
}
