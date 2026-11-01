// SPDX-License-Identifier: CC0-1.0

//! What this service checks about a REQUEST before it acts on it, and how it words the
//! refusal when a check fails.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted here is what a client sees, including the HTTP
//! status, the error code and the refusal text.
//!
//! The rules under test:
//!
//! * a trail's log-delivery destination is a bucket of the account, so `CreateTrail`
//!   ("creates a trail that specifies the settings for delivery of log data to an
//!   Amazon S3 bucket") and `UpdateTrail` refuse a name outside the Amazon S3 naming
//!   rules with `InvalidS3BucketNameException`, and resolve a well-formed one in the
//!   one object store before they persist it, refusing a name no bucket of the account
//!   carries with `S3BucketDoesNotExistException`;
//! * `ListInsightsMetricData` serves a window bounded by the longest retention its
//!   periods carry ("data points with a period of 3600 seconds (1 hour) are available
//!   for 90 days"), so a `StartTime` further in the past is refused with
//!   `InvalidParameterException` rather than answered with a shortened series;
//! * `LookupEvents`' refusals carry CloudTrail's own fixed sentences — an inverted
//!   window and a marker the service cannot read each have ONE wording, with no part
//!   of the request interpolated into it;
//! * `PutEventConfiguration` judges the request's own parameters BEFORE it resolves the
//!   resource they are for, so a call that states no configuration is refused on the
//!   parameters whatever the registry holds.
//!
//! The event data store registry is seeded through the service's public `state` module
//! because `CreateEventDataStore` is still a scaffold stub (it answers 501); trails and
//! every call under test go over the wire.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{
    EVENT_CATEGORY_AGGREGATION_VALUES, TEMPLATE_VALUES, TYPE_VALUES,
};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, invalid_s3_bucket_name_message, s3_bucket_does_not_exist_message,
    EventDataStoreRecord, EventDataStoreState, MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE,
    MISSING_TRAIL_CONFIGURATION_MESSAGE,
};
// The naming rules the destination is held to live in the shared crate, so the rows
// below are written against the rule itself rather than against a copy of it.
use service_common::s3_bucket_name::{MAX_BUCKET_NAME_CHARS, RESERVED_PREFIXES, RESERVED_SUFFIXES};

use super::emulator::{Emulator, HOME_REGION};

/// The HTTP status this awsJson1_1 API answers a client refusal with.
const REFUSED_STATUS: u16 = 400;

/// Seconds in a day, the unit the Insights retention cutoffs are documented in.
const SECONDS_PER_DAY: i64 = 86_400;

/// The `Period` values the `ListInsightsMetricData` doc says CloudTrail serves:
/// "valid values are 60, 300, and 3600".
const SERVED_PERIODS: [i64; 3] = [60, 300, 3600];

/// How far into the past a `StartTime` may reach: the longest retention the operation
/// doc publishes ("data points with a period of 3600 seconds (1 hour) are available for
/// 90 days"), which is also `StartTime`'s own default.
const MAX_LOOKBACK_DAYS: i64 = 90;

/// The refusal text for a `StartTime` past that bound, as the recorded response words
/// it: entirely fixed, naming neither the start it was given nor the member.
const START_TIME_TOO_OLD: &str = "The start time cannot be greater than 90 days in the past.";

/// The refusal text for a lookup window that runs backwards: CloudTrail's fixed
/// sentence for `InvalidTimeRangeException`.
const INVERTED_WINDOW: &str = "The start time precedes the end time.";

/// The refusal text for a `NextToken` CloudTrail cannot read: its fixed sentence for
/// `InvalidNextTokenException`, which names no part of the request.
const UNREADABLE_MARKER: &str = "Can't decrypt marker";

/// The id of the event data store the configuration cases address.
const STORE_ID: &str = "55555555-5555-4555-8555-555555555555";

/// An id no store carries, for the cases about a target that does not exist.
const ABSENT_STORE_ID: &str = "66666666-6666-4666-8666-666666666666";

/// The epoch second this test is running at.
fn now_secs() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch");
    i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX)
}

/// The `S3BucketName` a trail reports, or `""`.
fn reported_bucket(body: &Value) -> &str {
    body.get("S3BucketName").and_then(Value::as_str).unwrap_or_default()
}

/// Bucket names spanning the forms S3 holds one in — the shortest name, a dotted name,
/// a dashed name, a digits-only name and a long one — so the destination rule is
/// asserted over the class of names and not over one of them.
fn destination_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest name", "abc".to_string()),
        ("a dashed name", "cloudtrail-logs-probe".to_string()),
        ("a dotted name", "logs.cloudtrail.probe".to_string()),
        ("a digits-only name", "20261030123456".to_string()),
        ("a long name", format!("bucket-{}", "x".repeat(50))),
    ]
}

/// `CreateTrail` resolves the destination bucket in the account's object store: a name
/// the store holds is accepted and read back, and one it does not hold is refused with
/// the modelled error and CloudTrail's own sentence.
#[tokio::test]
async fn a_trail_is_created_only_against_a_destination_bucket_the_account_holds() {
    // The harness provisions no destination here: each case states whether the bucket
    // it names exists.
    let emulator = Emulator::without_provisioned_destinations();

    for (case, bucket) in destination_names() {
        // Refused first, so the SAME bucket name is accepted below only because it was
        // created in between — the answer follows the object store, not the name.
        let name = format!("trail-{}", bucket.replace('.', "-"));
        let refused = emulator
            .call(
                "CreateTrail",
                &json!({ "Name": name, "S3BucketName": bucket }),
            )
            .await;
        refused.expect_error(case, REFUSED_STATUS, "S3BucketDoesNotExistException");
        assert_eq!(
            refused.message(),
            s3_bucket_does_not_exist_message(&bucket),
            "case {case:?}: expected CloudTrail's own sentence for a destination that \
             does not exist, got {:?}",
            refused.body
        );
        let absent = emulator.call("GetTrail", &json!({ "Name": name })).await;
        absent.expect_error(
            &format!("{case}: GetTrail after the refusal"),
            REFUSED_STATUS,
            "TrailNotFoundException",
        );

        emulator.provision_bucket(&bucket, HOME_REGION);
        let created = emulator
            .call(
                "CreateTrail",
                &json!({ "Name": name, "S3BucketName": bucket }),
            )
            .await;
        let created = created.expect_ok(case).clone();
        assert_eq!(
            reported_bucket(&created),
            bucket,
            "case {case:?}: the created trail must report the destination it was given, \
             got {created:?}"
        );
        // The round trip: the destination is read back through `GetTrail`.
        let read = emulator.call("GetTrail", &json!({ "Name": name })).await;
        let read = read.expect_ok(case).clone();
        assert_eq!(
            reported_bucket(&read["Trail"]),
            bucket,
            "case {case:?}: GetTrail must report the stored destination, got {read:?}"
        );
    }
}

/// `UpdateTrail` applies the same rule to a destination it REPLACES, and a refused
/// update leaves the trail pointing where it did.
#[tokio::test]
async fn a_trail_destination_is_replaced_only_by_a_bucket_the_account_holds() {
    let emulator = Emulator::without_provisioned_destinations();
    const NAME: &str = "destination-update";
    const FIRST: &str = "destination-first";

    emulator.provision_bucket(FIRST, HOME_REGION);
    emulator
        .call(
            "CreateTrail",
            &json!({ "Name": NAME, "S3BucketName": FIRST }),
        )
        .await
        .expect_ok("CreateTrail against the first destination");

    for (case, bucket) in destination_names() {
        let refused = emulator
            .call(
                "UpdateTrail",
                &json!({ "Name": NAME, "S3BucketName": bucket }),
            )
            .await;
        refused.expect_error(case, REFUSED_STATUS, "S3BucketDoesNotExistException");
        assert_eq!(
            refused.message(),
            s3_bucket_does_not_exist_message(&bucket),
            "case {case:?}: expected CloudTrail's own sentence for a destination that \
             does not exist, got {:?}",
            refused.body
        );
        let held = emulator.call("GetTrail", &json!({ "Name": NAME })).await;
        let held = held.expect_ok(case).clone();
        assert_eq!(
            reported_bucket(&held["Trail"]),
            FIRST,
            "case {case:?}: a refused update must leave the destination alone, got {held:?}"
        );

        emulator.provision_bucket(&bucket, HOME_REGION);
        let updated = emulator
            .call(
                "UpdateTrail",
                &json!({ "Name": NAME, "S3BucketName": bucket }),
            )
            .await;
        let updated = updated.expect_ok(case).clone();
        assert_eq!(
            reported_bucket(&updated),
            bucket,
            "case {case:?}: the update must report the destination it stored, got {updated:?}"
        );
        // Put the trail back on the first destination, so each row starts from it.
        emulator
            .call(
                "UpdateTrail",
                &json!({ "Name": NAME, "S3BucketName": FIRST }),
            )
            .await
            .expect_ok(&format!("{case}: back to the first destination"));
    }
}

/// A trail that could never deliver a log file is never created: a refused destination
/// leaves the registry as it was, whatever else the request carried.
#[tokio::test]
async fn a_refused_destination_persists_no_trail() {
    let emulator = Emulator::without_provisioned_destinations();

    // Each row carries members a trail accepts beside the destination, so the refusal
    // is the destination's and nothing of the request is persisted.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a multi-region trail",
            json!({
                "Name": "no-destination-multi",
                "S3BucketName": "never-created-multi",
                "IsMultiRegionTrail": true,
                "IncludeGlobalServiceEvents": true,
            }),
        ),
        (
            "a trail with a key prefix and tags",
            json!({
                "Name": "no-destination-tagged",
                "S3BucketName": "never-created-tagged",
                "S3KeyPrefix": "logs/cloudtrail",
                "TagsList": [{ "Key": "owner", "Value": "probe" }],
            }),
        ),
        (
            "a trail with log file validation",
            json!({
                "Name": "no-destination-validated",
                "S3BucketName": "never-created-validated",
                "EnableLogFileValidation": true,
            }),
        ),
    ];

    for (case, request) in rows {
        let reply = emulator.call("CreateTrail", &request).await;
        reply.expect_error(case, REFUSED_STATUS, "S3BucketDoesNotExistException");
        let trails = emulator
            .call("ListTrails", &json!({}))
            .await
            .expect_ok(case)
            .get("Trails")
            .cloned()
            .unwrap_or(json!([]));
        assert_eq!(
            trails,
            json!([]),
            "case {case:?}: a refused create must persist no trail, got {trails:?}"
        );
    }
}

/// A destination outside the Amazon S3 naming rules names no bucket anywhere, so it is
/// refused as the malformed parameter it is — by both operations that take one, and
/// before anything is asked about the account's buckets.
#[tokio::test]
async fn a_destination_outside_the_s3_naming_rules_is_refused_as_a_bad_name() {
    let emulator = Emulator::without_provisioned_destinations();
    const NAME: &str = "bad-destination";
    const HELD: &str = "destination-held";

    emulator.provision_bucket(HELD, HOME_REGION);
    emulator
        .call("CreateTrail", &json!({ "Name": NAME, "S3BucketName": HELD }))
        .await
        .expect_ok("CreateTrail against a held destination");

    // One row per rule of the published general purpose bucket naming rules, as
    // `service_common::s3_bucket_name` states them: the length bounds, the character
    // set, the first and last character, adjacent periods, the IP-address form, and the
    // reserved prefixes and suffixes.
    let rows: Vec<(&str, String)> = vec![
        ("the empty string", String::new()),
        ("one character short of the bound", "ab".to_string()),
        (
            "one character past the bound",
            "b".repeat(MAX_BUCKET_NAME_CHARS + 1),
        ),
        ("an upper-case letter", "Invalid-Bucket".to_string()),
        ("an underscore", "invalid_bucket_name".to_string()),
        ("a character outside the set", "bucket!!".to_string()),
        ("a leading hyphen", "-leading-hyphen".to_string()),
        ("a trailing dot", "trailing-dot.".to_string()),
        ("two adjacent dots", "two..dots".to_string()),
        ("the IP-address form", "192.168.5.4".to_string()),
        (
            "a reserved prefix",
            format!("{}bucket", RESERVED_PREFIXES[0]),
        ),
        (
            "a reserved suffix",
            format!("bucket{}", RESERVED_SUFFIXES[0]),
        ),
    ];

    for (case, bucket) in rows {
        for operation in ["CreateTrail", "UpdateTrail"] {
            let case = format!("{operation} with {case}");
            let name = if operation == "CreateTrail" {
                "trail-for-a-bad-destination"
            } else {
                NAME
            };
            let reply = emulator
                .call(operation, &json!({ "Name": name, "S3BucketName": bucket }))
                .await;
            reply.expect_error(&case, REFUSED_STATUS, "InvalidS3BucketNameException");
            assert_eq!(
                reply.message(),
                invalid_s3_bucket_name_message(&bucket),
                "case {case:?}: expected CloudTrail's own sentence for a name no bucket \
                 can carry, got {:?}",
                reply.body
            );
        }
    }

    // Neither refusal moved anything: the trail still points where it did, and the
    // create left no trail behind.
    let held = emulator.call("GetTrail", &json!({ "Name": NAME })).await;
    let held = held.expect_ok("the trail after the refusals").clone();
    assert_eq!(
        reported_bucket(&held["Trail"]),
        HELD,
        "a refused destination must leave the trail pointing where it did, got {held:?}"
    );
    emulator
        .call("GetTrail", &json!({ "Name": "trail-for-a-bad-destination" }))
        .await
        .expect_error(
            "GetTrail after the refused creates",
            REFUSED_STATUS,
            "TrailNotFoundException",
        );
}

/// The Insights metric window: a `StartTime` inside the retention bound is served, and
/// one further in the past is refused with CloudTrail's fixed sentence — for every
/// `Period` the service serves, because the bound is the service's and not the
/// period's.
#[tokio::test]
async fn an_insights_start_time_past_the_retention_bound_is_refused() {
    let emulator = Emulator::new();

    // (case, how far back StartTime reaches in seconds, whether it is served).
    // The bound is 90 days; the row AT it is kept a minute inside, because the clock
    // moves between this line and the handler reading its own "time of request".
    let rows: Vec<(String, i64, bool)> = vec![
        ("the time of request".to_string(), 0, true),
        ("an hour back".to_string(), 3600, true),
        ("a day back".to_string(), SECONDS_PER_DAY, true),
        ("15 days back".to_string(), 15 * SECONDS_PER_DAY, true),
        ("63 days back".to_string(), 63 * SECONDS_PER_DAY, true),
        (
            "at the bound".to_string(),
            MAX_LOOKBACK_DAYS * SECONDS_PER_DAY - 60,
            true,
        ),
        (
            "an hour past the bound".to_string(),
            MAX_LOOKBACK_DAYS * SECONDS_PER_DAY + 3600,
            false,
        ),
        (
            "a day past the bound".to_string(),
            (MAX_LOOKBACK_DAYS + 1) * SECONDS_PER_DAY,
            false,
        ),
        (
            "120 days back".to_string(),
            120 * SECONDS_PER_DAY,
            false,
        ),
        (
            "a year back".to_string(),
            365 * SECONDS_PER_DAY,
            false,
        ),
    ];

    for period in SERVED_PERIODS {
        for (row, age, served) in &rows {
            let case = format!("period {period}, {row}");
            let reply = emulator
                .call(
                    "ListInsightsMetricData",
                    &json!({
                        "EventSource": "s3.amazonaws.com",
                        "EventName": "PutObject",
                        "InsightType": "ApiCallRateInsight",
                        "Period": period,
                        "StartTime": now_secs() - age,
                    }),
                )
                .await;
            if *served {
                reply.expect_ok(&case);
            } else {
                reply.expect_error(&case, REFUSED_STATUS, "InvalidParameterException");
                assert_eq!(
                    reply.message(),
                    START_TIME_TOO_OLD,
                    "case {case:?}: expected CloudTrail's fixed sentence for a start \
                     outside the retention bound, got {:?}",
                    reply.body
                );
            }
        }
    }
}

/// An out-of-window start is refused however the rest of the request is written: the
/// bound is on `StartTime` alone.
#[tokio::test]
async fn the_retention_bound_holds_whatever_else_the_insights_request_carries() {
    let emulator = Emulator::new();
    let too_old = 120 * SECONDS_PER_DAY;

    let rows: Vec<(&str, Value)> = vec![
        (
            "with FillWithZeros",
            json!({
                "EventSource": "iam.amazonaws.com",
                "EventName": "CreateUser",
                "InsightType": "ApiCallRateInsight",
                "DataType": "FillWithZeros",
                "StartTime": now_secs() - too_old,
            }),
        ),
        (
            "with an end time and a page size",
            json!({
                "EventSource": "lambda.amazonaws.com",
                "EventName": "Invoke",
                "InsightType": "ApiCallRateInsight",
                "StartTime": now_secs() - too_old,
                "EndTime": now_secs(),
                "MaxResults": 5,
            }),
        ),
        (
            "the error-rate series",
            json!({
                "EventSource": "sts.amazonaws.com",
                "EventName": "AssumeRole",
                "InsightType": "ApiErrorRateInsight",
                "ErrorCode": "AccessDenied",
                "StartTime": now_secs() - too_old,
            }),
        ),
    ];

    for (case, request) in rows {
        let reply = emulator.call("ListInsightsMetricData", &request).await;
        reply.expect_error(case, REFUSED_STATUS, "InvalidParameterException");
        assert_eq!(
            reply.message(),
            START_TIME_TOO_OLD,
            "case {case:?}: expected CloudTrail's fixed sentence, got {:?}",
            reply.body
        );
    }
}

/// `LookupEvents` refuses an inverted window with ONE fixed sentence, whatever the two
/// timestamps are: the wording carries no part of the request.
#[tokio::test]
async fn an_inverted_lookup_window_carries_one_fixed_sentence() {
    let emulator = Emulator::new();
    let now = now_secs();

    // (case, StartTime, EndTime) — inverted by a second, an hour, a day and a year.
    let rows: [(&str, i64, i64); 4] = [
        ("a second inverted", now, now - 1),
        ("an hour inverted", now, now - 3600),
        ("a day inverted", now - SECONDS_PER_DAY, now - 2 * SECONDS_PER_DAY),
        ("a year inverted", now, now - 365 * SECONDS_PER_DAY),
    ];

    for (case, start, end) in rows {
        let reply = emulator
            .call(
                "LookupEvents",
                &json!({ "StartTime": start, "EndTime": end }),
            )
            .await;
        reply.expect_error(case, REFUSED_STATUS, "InvalidTimeRangeException");
        assert_eq!(
            reply.message(),
            INVERTED_WINDOW,
            "case {case:?}: expected CloudTrail's fixed sentence for an inverted \
             window, got {:?}",
            reply.body
        );
    }
}

/// A `NextToken` the service cannot read is refused with ONE fixed sentence, whichever
/// way it fails to belong to the lookup.
#[tokio::test]
async fn an_unreadable_lookup_marker_carries_one_fixed_sentence() {
    let emulator = Emulator::new();

    // Two served calls to record events, so a lookup has a page boundary to issue a
    // real token at.
    for name in ["marker-probe-one", "marker-probe-two"] {
        emulator
            .call(
                "CreateTrail",
                &json!({ "Name": name, "S3BucketName": "marker-probe-bucket" }),
            )
            .await
            .expect_ok("CreateTrail for the event history");
    }
    let page = emulator
        .call("LookupEvents", &json!({ "MaxResults": 1 }))
        .await;
    let issued = page
        .expect_ok("the first page")
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a page boundary issues a token")
        .to_string();

    // (case, the NextToken the request carries, the Region it is replayed in, the rest
    // of the request)
    let rows: Vec<(&str, String, &str, Value)> = vec![
        (
            "a token that is not a marker",
            "not-a-marker".to_string(),
            HOME_REGION,
            json!({}),
        ),
        (
            "a truncated marker",
            issued[..issued.len() / 2].to_string(),
            HOME_REGION,
            json!({}),
        ),
        (
            "a marker carrying a character a marker cannot carry",
            format!("zz{issued}"),
            HOME_REGION,
            json!({}),
        ),
        (
            "this lookup's marker replayed in another Region",
            issued.clone(),
            "us-west-2",
            json!({}),
        ),
        (
            "this lookup's marker replayed with a filter",
            issued.clone(),
            HOME_REGION,
            json!({ "LookupAttributes": [{ "AttributeKey": "EventName", "AttributeValue": "CreateTrail" }] }),
        ),
        (
            "this lookup's marker replayed with another category",
            issued.clone(),
            HOME_REGION,
            json!({ "EventCategory": "insight" }),
        ),
    ];

    for (case, token, region, rest) in rows {
        let mut request = json!({ "MaxResults": 1, "NextToken": token });
        if let Some(members) = rest.as_object() {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        let reply = emulator.call_in("LookupEvents", region, &request).await;
        reply.expect_error(case, REFUSED_STATUS, "InvalidNextTokenException");
        assert_eq!(
            reply.message(),
            UNREADABLE_MARKER,
            "case {case:?}: expected CloudTrail's fixed sentence for a marker it cannot \
             read, got {:?}",
            reply.body
        );
    }

    // The marker still resumes the lookup it was issued for.
    let resumed = emulator
        .call("LookupEvents", &json!({ "MaxResults": 1, "NextToken": issued }))
        .await;
    resumed.expect_ok("the marker's own lookup");
}

/// `PutEventConfiguration` judges the request's own parameters before it resolves the
/// resource they are for: the SAME call is refused on the parameters whether the target
/// exists or not, and the target's absence is reported only once the parameters hold.
#[tokio::test]
async fn an_event_configuration_call_is_judged_on_its_parameters_before_its_target() {
    let emulator = Emulator::new();
    let store = EventDataStoreRecord {
        id: STORE_ID.to_string(),
        name: format!("lake-{STORE_ID}"),
        region: HOME_REGION.to_string(),
        status: EventDataStoreState::Enabled,
        ..EventDataStoreRecord::default()
    };
    {
        let state = emulator.state();
        state.lock().event_data_stores.insert(STORE_ID.to_string(), store);
    }
    emulator
        .call(
            "CreateTrail",
            &json!({ "Name": "configured-trail", "S3BucketName": "configuration-probe" }),
        )
        .await
        .expect_ok("CreateTrail for the trail cases");

    let selectors = json!([{ "Type": TYPE_VALUES[0], "Equals": ["ctx:Key0"] }]);
    let aggregations = json!([{
        "Templates": [TEMPLATE_VALUES[0]],
        "EventCategory": EVENT_CATEGORY_AGGREGATION_VALUES[0],
    }]);
    let absent_store_arn = event_data_store_arn(HOME_REGION, ABSENT_STORE_ID);

    // (case, request, expected error code, expected message)
    let rows: Vec<(&str, Value, &str, String)> = vec![
        // The configuration is missing, so the call is refused on it — the store it
        // names exists.
        (
            "a store that exists, no configuration",
            json!({ "EventDataStore": STORE_ID }),
            "ValidationException",
            MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE.to_string(),
        ),
        // ... and the same, for a store that does not exist: the parameters decide.
        (
            "a store that does not exist, no configuration",
            json!({ "EventDataStore": ABSENT_STORE_ID }),
            "ValidationException",
            MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE.to_string(),
        ),
        (
            "a store that does not exist, only the size",
            json!({ "EventDataStore": ABSENT_STORE_ID, "MaxEventSize": "Large" }),
            "ValidationException",
            MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE.to_string(),
        ),
        (
            "a store that does not exist, only the selectors",
            json!({ "EventDataStore": ABSENT_STORE_ID, "ContextKeySelectors": selectors }),
            "ValidationException",
            MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE.to_string(),
        ),
        // An identifier that cannot be read at all is still a request the parameters
        // are judged on first.
        (
            "an unreadable store identifier, no configuration",
            json!({ "EventDataStore": format!("arn:aws:logs:{HOME_REGION}:000000000000:eventdatastore/{STORE_ID}") }),
            "ValidationException",
            MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE.to_string(),
        ),
        // Once the parameters hold, the store's absence is what is reported.
        (
            "a store that does not exist, a complete configuration",
            json!({
                "EventDataStore": ABSENT_STORE_ID,
                "MaxEventSize": "Large",
                "ContextKeySelectors": selectors,
            }),
            "EventDataStoreNotFoundException",
            format!("The event data store {absent_store_arn} was not found."),
        ),
        (
            "a trail that exists, no configuration",
            json!({ "TrailName": "configured-trail" }),
            "ValidationException",
            MISSING_TRAIL_CONFIGURATION_MESSAGE.to_string(),
        ),
        (
            "a trail that does not exist, no configuration",
            json!({ "TrailName": "absent-trail" }),
            "ValidationException",
            MISSING_TRAIL_CONFIGURATION_MESSAGE.to_string(),
        ),
        // A member of the other kind cannot hold with this target, whatever the
        // registry holds.
        (
            "a trail that does not exist, a store-level member",
            json!({
                "TrailName": "absent-trail",
                "AggregationConfigurations": aggregations,
                "MaxEventSize": "Large",
            }),
            "InvalidParameterCombinationException",
            "MaxEventSize and ContextKeySelectors configure an event data store, so \
             neither can be set with TrailName."
                .to_string(),
        ),
    ];

    for (case, request, code, expected) in rows {
        let reply = emulator.call("PutEventConfiguration", &request).await;
        reply.expect_error(case, REFUSED_STATUS, code);
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            reply.body
        );
    }

    // The trail's absence is reported once ITS parameters hold, which is the other
    // half of the same ordering.
    let absent_trail = emulator
        .call(
            "PutEventConfiguration",
            &json!({ "TrailName": "absent-trail", "AggregationConfigurations": aggregations }),
        )
        .await;
    absent_trail.expect_error(
        "a trail that does not exist, a complete configuration",
        REFUSED_STATUS,
        "TrailNotFoundException",
    );
}
