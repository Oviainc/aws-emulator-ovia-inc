// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for reading a schedule group back: `GetScheduleGroup`.
//!
//! The rule under test: `GetScheduleGroup` reports the persisted group a name
//! addresses — its ARN, name, `ScheduleGroupState` and the two timestamps —
//! including the implicit `default` group every account has without creating it,
//! and answers `ResourceNotFoundException` for a name no group has. These cases
//! vary the name over the whole modelled `ScheduleGroupName` class and the
//! request over several regions; the recorded case is one row.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::wire_harness::{account_id, call, call_in, error_code, scheduler_router, REGION};

/// Seconds since the epoch, the unit restJson1 renders a `timestamp` member in.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// A timestamp member of a `GetScheduleGroup` response, as epoch seconds.
fn timestamp(body: &serde_json::Value, member: &str) -> f64 {
    body.get(member)
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_else(|| panic!("member {member:?} must be a numeric timestamp, got {body}"))
}

/// The names `ScheduleGroupName` accepts: `^[0-9a-zA-Z-_.]+$`, length 1..64.
/// One character, every punctuation character the pattern allows, digits only,
/// mixed case, and the 64-character maximum.
const NAME_CASES: &[(&str, &str)] = &[
    ("single character", "g"),
    ("dots, dashes and underscores", "a.b-c_d"),
    ("digits only", "9876543210"),
    ("mixed case", "GroupWithMixedCase"),
    (
        "at the 64-character maximum",
        "g0123456789012345678901234567890123456789012345678901234567890a",
    ),
];

#[tokio::test]
async fn get_schedule_group_reports_every_created_group() {
    let router = scheduler_router();

    for (case, name) in NAME_CASES {
        assert!(
            !name.is_empty() && name.len() <= 64,
            "case {case:?}: the test's own name is outside the modelled bound"
        );

        let before = now_epoch_seconds();
        let (status, created) =
            call(&router, "POST", &format!("/schedule-groups/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateScheduleGroup got {status} {created}"
        );
        let created_arn = created
            .get("ScheduleGroupArn")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        let (status, got) = call(&router, "GET", &format!("/schedule-groups/{name}"), None).await;
        let after = now_epoch_seconds();
        assert_eq!(
            status, 200,
            "case {case:?}: GetScheduleGroup got {status} {got}"
        );

        let expected_arn = format!(
            "arn:aws:scheduler:{REGION}:{}:schedule-group/{name}",
            account_id()
        );
        assert_eq!(
            got.get("Arn").and_then(serde_json::Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected Arn {expected_arn}, got {got}"
        );
        assert_eq!(
            got.get("Arn").and_then(serde_json::Value::as_str),
            Some(created_arn.as_str()),
            "case {case:?}: the ARN read back must be the one CreateScheduleGroup \
             answered ({created_arn}), got {got}"
        );
        assert_eq!(
            got.get("Name").and_then(serde_json::Value::as_str),
            Some(*name),
            "case {case:?}: expected Name {name}, got {got}"
        );
        // `ScheduleGroupState` is modelled `ACTIVE | DELETING`
        // (smithy_query shape="ScheduleGroupState"); a group AWS created and did
        // not delete is ACTIVE.
        assert_eq!(
            got.get("State").and_then(serde_json::Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: expected State ACTIVE, got {got}"
        );

        let creation = timestamp(&got, "CreationDate");
        let modification = timestamp(&got, "LastModificationDate");
        assert!(
            creation >= before - 1.0 && creation <= after + 1.0,
            "case {case:?}: CreationDate {creation} must be the instant of the \
             create call (between {before} and {after})"
        );
        assert_eq!(
            creation, modification,
            "case {case:?}: a group nothing has modified must report \
             LastModificationDate {creation}, got {modification}"
        );

        // A read reports state; it never rewrites it. Reading again answers the
        // same record, timestamps included.
        let (status, again) = call(&router, "GET", &format!("/schedule-groups/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the second GetScheduleGroup got {status} {again}"
        );
        assert_eq!(
            got, again,
            "case {case:?}: a read must not change what the next reader sees; \
             first {got}, then {again}"
        );
    }
}

#[tokio::test]
async fn group_arn_read_back_carries_the_region_the_group_was_created_in() {
    // One rule, several regions: the ARN's region segment is the request's
    // credential-scope region, not a constant.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-2", "sa-east-1"] {
        let router = scheduler_router();
        let name = format!("grp-{}", region.replace('-', "_"));
        let (status, body) = call_in(
            &router,
            "POST",
            &format!("/schedule-groups/{name}"),
            region,
            None,
        )
        .await;
        assert_eq!(status, 200, "region {region}: create got {status} {body}");

        let (status, got) = call_in(
            &router,
            "GET",
            &format!("/schedule-groups/{name}"),
            region,
            None,
        )
        .await;
        assert_eq!(status, 200, "region {region}: get got {status} {got}");
        let expected = format!(
            "arn:aws:scheduler:{region}:{}:schedule-group/{name}",
            account_id()
        );
        assert_eq!(
            got.get("Arn").and_then(serde_json::Value::as_str),
            Some(expected.as_str()),
            "region {region}: expected Arn {expected}, got {got}"
        );
    }
}

#[tokio::test]
async fn the_implicit_default_group_is_readable_without_being_created() {
    // The `default` group exists for every account in every region without a
    // create call, so a fresh store answers for it.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-2"] {
        let router = scheduler_router();
        let (status, got) = call_in(&router, "GET", "/schedule-groups/default", region, None).await;
        assert_eq!(
            status, 200,
            "region {region}: the default group must be readable, got {status} {got}"
        );
        let expected = format!(
            "arn:aws:scheduler:{region}:{}:schedule-group/default",
            account_id()
        );
        assert_eq!(
            got.get("Arn").and_then(serde_json::Value::as_str),
            Some(expected.as_str()),
            "region {region}: expected Arn {expected}, got {got}"
        );
        assert_eq!(
            got.get("Name").and_then(serde_json::Value::as_str),
            Some("default"),
            "region {region}: expected Name default, got {got}"
        );
        assert_eq!(
            got.get("State").and_then(serde_json::Value::as_str),
            Some("ACTIVE"),
            "region {region}: expected State ACTIVE, got {got}"
        );
        let creation = timestamp(&got, "CreationDate");
        assert!(
            creation > 0.0 && creation <= now_epoch_seconds() + 1.0,
            "region {region}: CreationDate {creation} must be a past instant"
        );

        // And it is the same record on every later read.
        let (status, again) =
            call_in(&router, "GET", "/schedule-groups/default", region, None).await;
        assert_eq!(status, 200, "region {region}: second read got {status}");
        assert_eq!(
            got, again,
            "region {region}: the default group must not be re-minted per read; \
             first {got}, then {again}"
        );
    }
}

#[tokio::test]
async fn getting_a_group_no_name_addresses_is_a_404() {
    let router = scheduler_router();

    // A group and a schedule inside it, so the misses below are misses for a
    // reason other than an empty store.
    let (status, body) = call(&router, "POST", "/schedule-groups/alpha-group", None).await;
    assert_eq!(status, 200, "setup: create group got {status} {body}");
    let (status, body) = call(
        &router,
        "POST",
        "/schedules/lonely?groupName=alpha-group",
        Some(serde_json::json!({
            "GroupName": "alpha-group",
            "ScheduleExpression": "rate(5 minutes)",
            "FlexibleTimeWindow": { "Mode": "OFF" },
            "Target": {
                "Arn": "arn:aws:sqs:eu-west-2:000000000000:q",
                "RoleArn": "arn:aws:iam::000000000000:role/r"
            }
        })),
    )
    .await;
    assert_eq!(status, 200, "setup: create schedule got {status} {body}");

    // Every way a name can fail to address a group, while still satisfying the
    // modelled `ScheduleGroupName` constraints.
    let misses: &[(&str, &str)] = &[
        ("never created", "never-created"),
        ("a prefix of an existing group", "alpha"),
        ("an existing group with a suffix", "alpha-group-2"),
        ("differing only in case", "ALPHA-GROUP"),
        ("a schedule's name, not a group's", "lonely"),
        ("the default group's name with a suffix", "default-2"),
    ];
    for (case, name) in misses {
        let (status, body) = call(&router, "GET", &format!("/schedule-groups/{name}"), None).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a 404 ResourceNotFoundException for {name}, \
             got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_name_outside_the_modelled_constraints_is_refused() {
    let router = scheduler_router();

    // `ScheduleGroupName` is length 1..64, pattern `^[0-9a-zA-Z-_.]+$`: one past
    // the published maximum, and a character the pattern excludes.
    let refused: &[(&str, String)] = &[
        ("one past the 64-character maximum", "g".repeat(65)),
        ("a character the pattern excludes", "bad~name".to_string()),
    ];
    for (case, name) in refused {
        let (status, body) = call(&router, "GET", &format!("/schedule-groups/{name}"), None).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} {body}"
        );
    }
}
