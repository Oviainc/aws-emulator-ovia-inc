// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListInsightsData`, over the shared
//! [`super::emulator::Emulator`] harness.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `InsightSource` is "the ARN of the trail for which you want to retrieve
//!   Insights events": it must be the ARN of a trail this account holds and the
//!   calling Region can see, and anything else — a bare name, another account's
//!   or partition's ARN, a non-trail ARN, an ARN naming no trail — is refused
//!   with HTTP 400 `ValidationException`;
//! * a `Dimensions` key is one of the model's `ListInsightsDataDimensionKey`
//!   values, and a key outside that set is refused in the validation envelope;
//! * "If the specified end time is before the specified start time, an error is
//!   returned", and a window may only start within the documented 90-day
//!   lookback;
//! * a trail that exists answers an empty `Events` page with no `NextToken`,
//!   because nothing in this emulator records CloudTrail events or runs the
//!   Insights analysis that mints an Insights event — so a `NextToken` is never
//!   one this emulator issued;
//! * a boundary refusal names the member and the requirement it failed, worded by
//!   this service's `override_refusal` (`src/service.rs`): the classes it rewords
//!   value-free state the member and the bound only, and every other class keeps
//!   the framework's own sentence, value and all.
//!
//! Which operation an `X-Amz-Target` names is asserted in
//! `tests/all/target_prefix_dispatch.rs`; every case here goes through the
//! harness, which sends the prefix an SDK sends.

use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::list_insights_data::INSIGHTS_LOOKBACK_DAYS;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::LIST_INSIGHTS_DATA_DIMENSION_KEY_VALUES;

use super::emulator::{Emulator, Reply, HOME_REGION};

// The error message a reply carries is read through the shared harness's
// `Reply::message()`, which also reads the capitalised `Message` member a
// modelled CloudTrail error shape carries.

/// Another Region, for the cases about which trails a Region can see.
const OTHER_REGION: &str = "us-west-2";

/// The account every emulator ARN carries (`emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// Assert `reply` is an empty `ListInsightsData` page.
fn expect_empty_page(reply: &Reply, case: &str) {
    let body = reply.expect_ok(case);
    assert_eq!(
        body["Events"],
        json!([]),
        "case {case}: expected an empty Events list, got {body:?}"
    );
    assert_eq!(
        body.get("NextToken"),
        None,
        "case {case}: a page that is not full carries no NextToken, got {body:?}"
    );
}

/// The trail ARN of `name` homed in `region`, as `CreateTrail` mints it.
fn trail_arn(region: &str, name: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:{ACCOUNT}:trail/{name}")
}

/// Seconds since the epoch, as an `awsJson1_1` timestamp is carried.
fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("a clock after the epoch")
            .as_secs(),
    )
    .expect("an instant that fits an i64")
}

/// A minimal `ListInsightsData` request for `source`.
fn request_for(source: &str) -> Value {
    json!({"InsightSource": source, "DataType": "InsightsEvents"})
}

/// An emulator holding one single-Region trail in [`HOME_REGION`] and one
/// multi-Region trail homed in [`OTHER_REGION`].
async fn with_two_trails() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .call_in(
            "CreateTrail",
            HOME_REGION,
            &json!({"Name": "insights-home", "S3BucketName": "bucket-home"}),
        )
        .await
        .expect_ok("CreateTrail insights-home");
    emulator
        .call_in(
            "CreateTrail",
            OTHER_REGION,
            &json!({
                "Name": "insights-everywhere",
                "S3BucketName": "bucket-multi",
                "IsMultiRegionTrail": true,
            }),
        )
        .await
        .expect_ok("CreateTrail insights-everywhere");
    emulator
}

/// `InsightSource` must be the ARN of a trail this account holds and the calling
/// Region can see; every other value is refused the same way.
#[tokio::test]
async fn insight_source_must_be_the_arn_of_a_visible_trail() {
    let emulator = with_two_trails().await;
    let home = trail_arn(HOME_REGION, "insights-home");
    let everywhere = trail_arn(OTHER_REGION, "insights-everywhere");

    // (case, calling Region, InsightSource, is it accepted)
    let rows: Vec<(&str, &str, String, bool)> = vec![
        (
            "the ARN of a trail homed in the calling Region",
            HOME_REGION,
            home.clone(),
            true,
        ),
        (
            "the ARN of a multi-Region trail, from its home Region",
            OTHER_REGION,
            everywhere.clone(),
            true,
        ),
        (
            "the ARN of a multi-Region trail, from a Region holding its shadow",
            HOME_REGION,
            everywhere.clone(),
            true,
        ),
        (
            "the ARN of a single-Region trail, from another Region",
            OTHER_REGION,
            home.clone(),
            false,
        ),
        (
            "the bare name of an existing trail",
            HOME_REGION,
            "insights-home".to_string(),
            false,
        ),
        (
            "a well-formed trail ARN naming no trail",
            HOME_REGION,
            trail_arn(HOME_REGION, "no-such-trail"),
            false,
        ),
        (
            "the right trail name in the wrong Region",
            HOME_REGION,
            trail_arn("eu-west-1", "insights-home"),
            false,
        ),
        (
            "another account's ARN for the same trail name",
            HOME_REGION,
            format!("arn:aws:cloudtrail:{HOME_REGION}:123456789012:trail/insights-home"),
            false,
        ),
        (
            "another partition's ARN for the same trail name",
            HOME_REGION,
            format!("arn:aws-cn:cloudtrail:{HOME_REGION}:{ACCOUNT}:trail/insights-home"),
            false,
        ),
        (
            "an event data store ARN",
            HOME_REGION,
            format!("arn:aws:cloudtrail:{HOME_REGION}:{ACCOUNT}:eventdatastore/insights-home"),
            false,
        ),
        (
            "a channel ARN",
            HOME_REGION,
            format!("arn:aws:cloudtrail:{HOME_REGION}:{ACCOUNT}:channel/insights-home"),
            false,
        ),
        (
            "another service's ARN",
            HOME_REGION,
            format!("arn:aws:s3:{HOME_REGION}:{ACCOUNT}:trail/insights-home"),
            false,
        ),
        (
            "a trail ARN with no trail name",
            HOME_REGION,
            format!("arn:aws:cloudtrail:{HOME_REGION}:{ACCOUNT}:trail/"),
            false,
        ),
        (
            "a value that is not an ARN at all",
            HOME_REGION,
            "not-an-arn".to_string(),
            false,
        ),
    ];

    for (case, region, source, accepted) in rows {
        let reply = emulator
            .call_in("ListInsightsData", region, &request_for(&source))
            .await;
        if accepted {
            expect_empty_page(&reply, case);
            continue;
        }
        reply.expect_error(case, 400, "ValidationException");
        assert_eq!(
            reply.message(),
            format!("The Insight source {source} is not a valid trail ARN."),
            "case {case}: the refusal does not name the source it refused, got {reply:?}"
        );
    }
}

/// A `Dimensions` key is one of the model's `ListInsightsDataDimensionKey`
/// values; a key outside that set is refused in the validation envelope.
#[tokio::test]
async fn dimension_keys_come_from_the_modelled_enum() {
    let emulator = with_two_trails().await;
    let source = trail_arn(HOME_REGION, "insights-home");
    // The envelope the service states a map-key constraint in, written out: the
    // member path, the constraint its keys must satisfy, and the value set in the
    // order the service lists it.
    let refusal = "1 validation error detected: Value at 'dimensions' failed to satisfy \
                   constraint: Map keys must satisfy constraint: [Member must satisfy \
                   enum value set: [EventName, EventSource, EventId]]";
    // The set that refusal names is the model's own, so a value the enum gains
    // cannot go unlisted.
    for key in LIST_INSIGHTS_DATA_DIMENSION_KEY_VALUES.iter().copied() {
        assert!(
            refusal.contains(key),
            "case the refusal text: the model declares the {key} dimension but the \
             refusal does not list it, got {refusal:?}"
        );
    }

    // Every value the model's enum declares is accepted — including the ones no
    // recorded request used.
    for key in LIST_INSIGHTS_DATA_DIMENSION_KEY_VALUES.iter().copied() {
        let case = format!("the {key} dimension");
        let mut request = request_for(&source);
        request["Dimensions"] = json!({key: "a-value"});
        let reply = emulator.call("ListInsightsData", &request).await;
        expect_empty_page(&reply, &case);
    }

    // A key outside the enum, however it differs from one inside it.
    for key in [
        "EventType",
        "eventName",
        "EVENTSOURCE",
        "EventIds",
        "EventI",
        "",
    ] {
        let case = format!("the {key:?} dimension");
        let mut request = request_for(&source);
        request["Dimensions"] = json!({key: "a-value"});
        let reply = emulator.call("ListInsightsData", &request).await;
        reply.expect_error(&case, 400, "ValidationException");
        assert_eq!(
            reply.message(),
            refusal,
            "case {case}: expected the map-key validation envelope, got {reply:?}"
        );
    }
}

/// The window a request asks for: an end before the start is refused, and a
/// start before the documented lookback is refused — at the published bound and
/// one past it.
#[tokio::test]
async fn the_requested_window_must_be_within_the_documented_lookback() {
    let emulator = with_two_trails().await;
    let source = trail_arn(HOME_REGION, "insights-home");
    let now = now_secs();
    let day = 24 * 60 * 60;
    let lookback = i64::try_from(INSIGHTS_LOOKBACK_DAYS).expect("a day count that fits") * day;
    // A minute of slack either side of a bound, so a case cannot flip on the
    // clock ticking between building the request and serving it.
    let minute = 60;

    // (case, StartTime, EndTime, the message expected, or "" for a success)
    let rows: Vec<(&str, Option<i64>, Option<i64>, &str)> = vec![
        ("no window at all", None, None, ""),
        ("an end with no start", None, Some(now), ""),
        ("a start with no end", Some(now - day), None, ""),
        ("an hour-long window", Some(now - 3600), Some(now), ""),
        ("a zero-length window", Some(now), Some(now), ""),
        (
            "an end one second before the start",
            Some(now),
            Some(now - 1),
            "The start time must be before the end time.",
        ),
        (
            "an end long before the start",
            Some(now - day),
            Some(now - 30 * day),
            "The start time must be before the end time.",
        ),
        (
            "a start just inside the lookback",
            Some(now - lookback + minute),
            Some(now),
            "",
        ),
        (
            "a window entirely at the far edge of the lookback",
            Some(now - lookback + minute),
            Some(now - lookback + 2 * minute),
            "",
        ),
        (
            "a start just outside the lookback",
            Some(now - lookback - minute),
            Some(now),
            "a start outside the lookback",
        ),
        (
            "a start far outside the lookback",
            Some(now - 10 * lookback),
            Some(now),
            "a start outside the lookback",
        ),
    ];

    for (case, start, end, expected) in rows {
        let mut request = request_for(&source);
        if let Some(start) = start {
            request["StartTime"] = json!(start);
        }
        if let Some(end) = end {
            request["EndTime"] = json!(end);
        }
        let reply = emulator.call("ListInsightsData", &request).await;
        if expected.is_empty() {
            expect_empty_page(&reply, case);
            continue;
        }
        reply.expect_error(case, 400, "ValidationException");
        if expected == "a start outside the lookback" {
            // The refusal names the member and the bound it broke; the instant it
            // quotes is the one the request sent.
            assert!(
                reply.message().starts_with("The StartTime ")
                    && reply
                        .message()
                        .contains(&format!("{INSIGHTS_LOOKBACK_DAYS} days")),
                "case {case}: expected a refusal naming StartTime and the \
                 {INSIGHTS_LOOKBACK_DAYS}-day lookback, got {reply:?}"
            );
            continue;
        }
        assert_eq!(
            reply.message(),
            expected,
            "case {case}: unexpected refusal text, got {reply:?}"
        );
    }
}

/// No page this operation answers is ever full, so no `NextToken` is ever
/// minted — and a token a caller sends is one this emulator did not issue.
#[tokio::test]
async fn a_page_is_never_full_and_no_token_is_ever_issued() {
    let emulator = with_two_trails().await;
    let source = trail_arn(HOME_REGION, "insights-home");

    // Every page size the model allows, at each published bound and in between.
    for max_results in [1, 2, 25, 49, 50] {
        let case = format!("MaxResults {max_results}");
        let mut request = request_for(&source);
        request["MaxResults"] = json!(max_results);
        let reply = emulator.call("ListInsightsData", &request).await;
        expect_empty_page(&reply, &case);
    }

    // `PaginationToken` is 4..1000 characters, so each of these reaches the
    // handler rather than the boundary validator.
    for token in ["abcd", "0".repeat(1000).as_str(), "eyJvZmZzZXQiOiAxfQ=="] {
        let case = format!("NextToken {token:.16}");
        let mut request = request_for(&source);
        request["NextToken"] = json!(token);
        let reply = emulator.call("ListInsightsData", &request).await;
        reply.expect_error(&case, 400, "InvalidParameterException");
        assert_eq!(
            reply.message(),
            format!("The NextToken {token} is not valid."),
            "case {case}: the refusal does not name the token it refused, got {reply:?}"
        );
    }
}

/// A boundary refusal names the member and the requirement it failed. For an
/// enum or a pattern constraint it does NOT echo the offending value back; every
/// other class still shows the value that broke it.
#[tokio::test]
async fn a_boundary_refusal_names_the_member_and_its_constraint() {
    let emulator = with_two_trails().await;
    let source = trail_arn(HOME_REGION, "insights-home");

    // (case, request, the message expected)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a DataType outside the enum",
            json!({"InsightSource": source, "DataType": "NotAValidType"}),
            // An enum violation is a class this service rewords value-free (see
            // `src/service.rs::is_value_free`), as the recorded answer to a
            // `DataType` outside its enum shows.
            "1 validation error detected: Value at 'dataType' failed to satisfy \
             constraint: Member must satisfy enum value set: [InsightsEvents]"
                .to_string(),
        ),
        (
            "an InsightSource outside the member's pattern",
            json!({"InsightSource": "a trail arn with spaces", "DataType": "InsightsEvents"}),
            "1 validation error detected: Value at 'insightSource' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: \
             ^[a-zA-Z0-9._/\\-:]+$"
                .to_string(),
        ),
        (
            "an InsightSource shorter than the member's minimum",
            json!({"InsightSource": "ab", "DataType": "InsightsEvents"}),
            "1 validation error detected: Value 'ab' at 'insightSource' failed to \
             satisfy constraint: Member must have length greater than or equal to 3"
                .to_string(),
        ),
        (
            // A `@range` violation IS reworded value-free here (the rule
            // `tests/all/range_refusal_wording.rs` owns), so these two name the
            // member and the bound and quote nothing of the value.
            "a MaxResults below the member's range",
            json!({"InsightSource": source, "DataType": "InsightsEvents", "MaxResults": 0}),
            "1 validation error detected: Value at 'maxResults' failed to satisfy \
             constraint: Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "a MaxResults above the member's range",
            json!({"InsightSource": source, "DataType": "InsightsEvents", "MaxResults": 51}),
            "1 validation error detected: Value at 'maxResults' failed to satisfy \
             constraint: Member must have value less than or equal to 50"
                .to_string(),
        ),
    ];

    for (case, request, expected) in rows {
        let reply = emulator.call("ListInsightsData", &request).await;
        reply.expect_error(case, 400, "ValidationException");
        assert_eq!(
            reply.message(),
            expected,
            "case {case}: unexpected refusal text, got {reply:?}"
        );
    }

    // A required member left out is refused too, and a refusal changes nothing:
    // the next well-formed request still answers.
    emulator
        .call("ListInsightsData", &json!({"InsightSource": source}))
        .await
        .expect_error("DataType absent", 400, "ValidationException");
    emulator
        .call("ListInsightsData", &json!({"DataType": "InsightsEvents"}))
        .await
        .expect_error("InsightSource absent", 400, "ValidationException");
    let reply = emulator
        .call("ListInsightsData", &request_for(&source))
        .await;
    expect_empty_page(&reply, "a well-formed request after the refusals");
}
