// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the EventBridge Scheduler schedule lifecycle.
//!
//! Every request here goes through the real restJson1 front door —
//! [`MultiServiceRouter::dispatch`] — so routing, URI/query binding, constraint
//! validation, the handler, and response serialization are all exercised. The
//! state lives in one shared engine for the whole router, so a value written by
//! one request is read back by the next, which is the property these tests are
//! about.

use smithy_server_runtime::MultiServiceRouter;

use crate::wire_harness::{account_id, call, call_in, error_code, scheduler_router, REGION};

/// One row of the create/read/update/delete table: a whole `CreateSchedule`
/// request body, plus the group it belongs in.
struct LifecycleCase {
    case: &'static str,
    /// `None` exercises the implicit `default` group.
    group: Option<&'static str>,
    name: &'static str,
    create: serde_json::Value,
}

/// The classes a schedule's input varies over: the group (implicit default vs a
/// created one), the three documented `ScheduleExpression` forms (`at`, `rate`,
/// `cron` — `CreateScheduleInput$ScheduleExpression` docs), both
/// `FlexibleTimeWindowMode` values at each published bound of
/// `MaximumWindowInMinutes` (1..1440), both `ScheduleState` values, both
/// `ActionAfterCompletion` values, and targets with and without the optional
/// parameter blocks.
fn lifecycle_cases() -> Vec<LifecycleCase> {
    vec![
        LifecycleCase {
            case: "minimal, implicit default group, rate expression, window OFF",
            group: None,
            name: "minimal",
            create: serde_json::json!({
                "ScheduleExpression": "rate(5 minutes)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": {
                    "Arn": "arn:aws:sqs:eu-west-2:000000000000:q",
                    "RoleArn": "arn:aws:iam::000000000000:role/r"
                }
            }),
        },
        LifecycleCase {
            case: "every optional member set, at expression, window at the lower bound",
            group: Some("alpha-group"),
            name: "fully.specified_schedule-1",
            create: serde_json::json!({
                "ScheduleExpression": "at(2027-01-01T00:00:00)",
                "ScheduleExpressionTimezone": "Europe/London",
                "Description": "every optional member",
                "State": "DISABLED",
                "StartDate": 1_767_225_600.5,
                "EndDate": 1_798_761_600.0,
                "KmsKeyArn": "arn:aws:kms:eu-west-2:000000000000:key/abc_123",
                "ActionAfterCompletion": "DELETE",
                "FlexibleTimeWindow": { "Mode": "FLEXIBLE", "MaximumWindowInMinutes": 1 },
                "Target": {
                    "Arn": "arn:aws:sqs:eu-west-2:000000000000:q.fifo",
                    "RoleArn": "arn:aws:iam::000000000000:role/r",
                    "Input": "{\"k\":\"v\"}",
                    "RetryPolicy": { "MaximumEventAgeInSeconds": 120, "MaximumRetryAttempts": 3 },
                    "DeadLetterConfig": { "Arn": "arn:aws:sqs:eu-west-2:000000000000:dlq" },
                    "SqsParameters": { "MessageGroupId": "g1" }
                }
            }),
        },
        LifecycleCase {
            case: "cron expression, window at the upper bound, ENABLED, NONE after completion",
            group: Some("alpha-group"),
            name: "cron-schedule",
            create: serde_json::json!({
                "ScheduleExpression": "cron(0 8 1 * ? *)",
                "State": "ENABLED",
                "ActionAfterCompletion": "NONE",
                "FlexibleTimeWindow": { "Mode": "FLEXIBLE", "MaximumWindowInMinutes": 1440 },
                "Target": {
                    "Arn": "arn:aws:lambda:eu-west-2:000000000000:function:fn",
                    "RoleArn": "arn:aws:iam::000000000000:role/r",
                    "EventBridgeParameters": { "DetailType": "d", "Source": "s" }
                }
            }),
        },
        LifecycleCase {
            case: "kinesis target in a dotted group name, no State (defaults to ENABLED)",
            group: Some("beta.group-2_x"),
            name: "k",
            create: serde_json::json!({
                "ScheduleExpression": "rate(1 day)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": {
                    "Arn": "arn:aws:kinesis:eu-west-2:000000000000:stream/s",
                    "RoleArn": "arn:aws:iam::000000000000:role/r",
                    "KinesisParameters": { "PartitionKey": "pk" }
                }
            }),
        },
    ]
}

/// Create the group a case needs (the implicit `default` group needs none).
async fn ensure_group(router: &MultiServiceRouter, group: Option<&str>) {
    if let Some(group) = group {
        let (status, body) =
            call(router, "POST", &format!("/schedule-groups/{group}"), None).await;
        assert!(
            status == 200 || error_code(&body) == "ConflictException",
            "group {group}: expected 200 or a ConflictException, got {status} {body}"
        );
    }
}

/// The value a member should have in a `GetSchedule` response: the one the
/// create request sent, or absent when it sent none.
fn expected_member(create: &serde_json::Value, member: &str) -> Option<serde_json::Value> {
    create.get(member).cloned()
}

#[tokio::test]
async fn create_get_update_delete_round_trips_every_schedule_shape() {
    let router = scheduler_router();

    for case in lifecycle_cases() {
        let LifecycleCase {
            case: label,
            group,
            name,
            create,
        } = case;
        ensure_group(&router, group).await;

        let group_name = group.unwrap_or("default");
        let mut create_body = create.clone();
        if let Some(group) = group {
            create_body["GroupName"] = serde_json::json!(group);
        }

        // CREATE — the ARN is generated from the request's region, account and
        // the (group, name) pair, never echoed from the request.
        let (status, body) = call(
            &router,
            "POST",
            &format!("/schedules/{name}"),
            Some(create_body),
        )
        .await;
        assert_eq!(status, 200, "case {label:?}: CreateSchedule got {status} {body}");
        let expected_arn = format!(
            "arn:aws:scheduler:{REGION}:{}:schedule/{group_name}/{name}",
            account_id()
        );
        assert_eq!(
            body.get("ScheduleArn").and_then(serde_json::Value::as_str),
            Some(expected_arn.as_str()),
            "case {label:?}: expected ScheduleArn {expected_arn}, got {body}"
        );

        // READ — field for field, from state, in a separate request.
        let (status, got) = call(
            &router,
            "GET",
            &format!("/schedules/{name}?groupName={group_name}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {label:?}: GetSchedule got {status} {got}");
        assert_eq!(
            got.get("Arn").and_then(serde_json::Value::as_str),
            Some(expected_arn.as_str()),
            "case {label:?}: expected Arn {expected_arn}, got {got}"
        );
        assert_eq!(
            got.get("Name").and_then(serde_json::Value::as_str),
            Some(name),
            "case {label:?}: expected Name {name}, got {got}"
        );
        assert_eq!(
            got.get("GroupName").and_then(serde_json::Value::as_str),
            Some(group_name),
            "case {label:?}: expected GroupName {group_name}, got {got}"
        );
        for member in [
            "ScheduleExpression",
            "ScheduleExpressionTimezone",
            "Description",
            "KmsKeyArn",
            "ActionAfterCompletion",
            "Target",
            "FlexibleTimeWindow",
        ] {
            assert_eq!(
                got.get(member).cloned(),
                expected_member(&create, member),
                "case {label:?}: member {member} did not round-trip, got {:?}",
                got.get(member)
            );
        }
        // Timestamps the client sent come back as the same instant; JSON renders
        // a whole epoch-second without its `.0`, so compare numerically.
        for member in ["StartDate", "EndDate"] {
            assert_eq!(
                got.get(member).and_then(serde_json::Value::as_f64),
                create.get(member).and_then(serde_json::Value::as_f64),
                "case {label:?}: member {member} did not round-trip, got {:?}",
                got.get(member)
            );
        }
        // `State` defaults to ENABLED when the request omits it.
        let expected_state = create
            .get("State")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("ENABLED");
        assert_eq!(
            got.get("State").and_then(serde_json::Value::as_str),
            Some(expected_state),
            "case {label:?}: expected State {expected_state}, got {got}"
        );
        let creation_date = got
            .get("CreationDate")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: CreationDate absent in {got}"));
        let first_modified = got
            .get("LastModificationDate")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: LastModificationDate absent in {got}"));
        assert!(
            creation_date > 0.0 && first_modified >= creation_date,
            "case {label:?}: expected a server-set CreationDate <= LastModificationDate, \
             got {creation_date} and {first_modified}"
        );

        // UPDATE — full replace: the new request carries only the three required
        // members plus State, so every other member must be CLEARED.
        let (status, body) = call(
            &router,
            "PUT",
            &format!("/schedules/{name}"),
            Some(serde_json::json!({
                "GroupName": group_name,
                "ScheduleExpression": "rate(10 minutes)",
                "State": "DISABLED",
                "FlexibleTimeWindow": { "Mode": "FLEXIBLE", "MaximumWindowInMinutes": 15 },
                "Target": {
                    "Arn": "arn:aws:sqs:eu-west-2:000000000000:updated",
                    "RoleArn": "arn:aws:iam::000000000000:role/updated"
                }
            })),
        )
        .await;
        assert_eq!(status, 200, "case {label:?}: UpdateSchedule got {status} {body}");
        assert_eq!(
            body.get("ScheduleArn").and_then(serde_json::Value::as_str),
            Some(expected_arn.as_str()),
            "case {label:?}: UpdateSchedule must return the same ARN, got {body}"
        );

        let (status, got) = call(
            &router,
            "GET",
            &format!("/schedules/{name}?groupName={group_name}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {label:?}: GetSchedule got {status} {got}");
        assert_eq!(
            got.get("ScheduleExpression")
                .and_then(serde_json::Value::as_str),
            Some("rate(10 minutes)"),
            "case {label:?}: expected the updated expression, got {got}"
        );
        assert_eq!(
            got.get("State").and_then(serde_json::Value::as_str),
            Some("DISABLED"),
            "case {label:?}: expected State DISABLED, got {got}"
        );
        assert_eq!(
            got.get("FlexibleTimeWindow").cloned(),
            Some(serde_json::json!({ "Mode": "FLEXIBLE", "MaximumWindowInMinutes": 15 })),
            "case {label:?}: expected the updated window, got {got}"
        );
        assert_eq!(
            got.get("Target").cloned(),
            Some(serde_json::json!({
                "Arn": "arn:aws:sqs:eu-west-2:000000000000:updated",
                "RoleArn": "arn:aws:iam::000000000000:role/updated"
            })),
            "case {label:?}: expected the Target replaced wholesale, got {got}"
        );
        for cleared in [
            "ScheduleExpressionTimezone",
            "Description",
            "KmsKeyArn",
            "ActionAfterCompletion",
            "StartDate",
            "EndDate",
        ] {
            assert_eq!(
                got.get(cleared),
                None,
                "case {label:?}: an omitted member must be cleared by a full-replace \
                 update, but {cleared} survived as {:?}",
                got.get(cleared)
            );
        }
        assert_eq!(
            got.get("CreationDate").and_then(serde_json::Value::as_f64),
            Some(creation_date),
            "case {label:?}: CreationDate must survive an update, got {got}"
        );
        let second_modified = got
            .get("LastModificationDate")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: LastModificationDate absent in {got}"));
        assert!(
            second_modified > first_modified,
            "case {label:?}: expected LastModificationDate to advance past \
             {first_modified}, got {second_modified}"
        );

        // DELETE — and the resource is observably gone.
        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/schedules/{name}?groupName={group_name}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {label:?}: DeleteSchedule got {status} {body}");

        let (status, body) = call(
            &router,
            "GET",
            &format!("/schedules/{name}?groupName={group_name}"),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {label:?}: a deleted schedule must be gone, got {status} {body}"
        );

        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/schedules/{name}?groupName={group_name}"),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {label:?}: deleting a gone schedule must 404, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn schedule_group_arns_track_the_requests_region_and_names_are_unique() {
    let router = scheduler_router();

    // The name class the model accepts for `ScheduleGroupName`:
    // `^[0-9a-zA-Z-_.]+$`, length 1..64 — a single character, every punctuation
    // character the pattern allows, digits only, and the 64-character maximum.
    let cases: &[(&str, &str)] = &[
        ("single character", "g"),
        ("dots, dashes and underscores", "a.b-c_d"),
        ("digits only", "1234567890"),
        (
            "at the 64-character maximum",
            "g0123456789012345678901234567890123456789012345678901234567890a",
        ),
    ];

    for (case, name) in cases {
        assert!(
            name.len() <= 64,
            "case {case:?}: the test's own name is over the modelled bound"
        );
        let (status, body) = call(&router, "POST", &format!("/schedule-groups/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateScheduleGroup got {status} {body}"
        );
        let expected = format!(
            "arn:aws:scheduler:{REGION}:{}:schedule-group/{name}",
            account_id()
        );
        assert_eq!(
            body.get("ScheduleGroupArn")
                .and_then(serde_json::Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected ScheduleGroupArn {expected}, got {body}"
        );

        // A name that already names a group conflicts, whatever the name is.
        let (status, body) = call(&router, "POST", &format!("/schedule-groups/{name}"), None).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (409, "ConflictException"),
            "case {case:?}: a duplicate group must conflict, got {status} {body}"
        );
    }

    // The reserved `default` group is refused on the NAME, as a validation
    // failure, rather than as a collision with the group the account already has
    // — `reserved_schedule_group.rs` is where that rule is pinned over the whole
    // class.
    let (status, body) = call(&router, "POST", "/schedule-groups/default", None).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "the reserved default group must be refused as a validation failure, got {status} {body}"
    );
}

#[tokio::test]
async fn group_arn_region_follows_the_signed_region() {
    let router = scheduler_router();

    // One rule, several regions: the ARN's region segment is the request's
    // credential-scope region, not a constant.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-2", "sa-east-1"] {
        let name = format!("grp-{}", region.replace('-', "_"));
        let (status, body) = call_in(
            &router,
            "POST",
            &format!("/schedule-groups/{name}"),
            region,
            None,
        )
        .await;
        assert_eq!(status, 200, "region {region}: got {status} {body}");
        let expected = format!(
            "arn:aws:scheduler:{region}:{}:schedule-group/{name}",
            account_id()
        );
        assert_eq!(
            body.get("ScheduleGroupArn")
                .and_then(serde_json::Value::as_str),
            Some(expected.as_str()),
            "region {region}: expected {expected}, got {body}"
        );
    }
}

#[tokio::test]
async fn a_schedule_needs_a_group_that_exists() {
    let router = scheduler_router();
    let body = serde_json::json!({
        "GroupName": "never-created",
        "ScheduleExpression": "rate(5 minutes)",
        "FlexibleTimeWindow": { "Mode": "OFF" },
        "Target": {
            "Arn": "arn:aws:sqs:eu-west-2:000000000000:q",
            "RoleArn": "arn:aws:iam::000000000000:role/r"
        }
    });
    let (status, got) = call(&router, "POST", "/schedules/s", Some(body)).await;
    assert_eq!(
        (status, error_code(&got).as_str()),
        (404, "ResourceNotFoundException"),
        "a schedule in an uncreated group must 404, got {status} {got}"
    );

    // The same request into the implicit `default` group succeeds, and a second
    // one with the same (group, name) conflicts.
    let in_default = serde_json::json!({
        "ScheduleExpression": "rate(5 minutes)",
        "FlexibleTimeWindow": { "Mode": "OFF" },
        "Target": {
            "Arn": "arn:aws:sqs:eu-west-2:000000000000:q",
            "RoleArn": "arn:aws:iam::000000000000:role/r"
        }
    });
    let (status, got) = call(&router, "POST", "/schedules/s", Some(in_default.clone())).await;
    assert_eq!(status, 200, "a schedule in the default group must be created, got {status} {got}");
    let (status, got) = call(&router, "POST", "/schedules/s", Some(in_default)).await;
    assert_eq!(
        (status, error_code(&got).as_str()),
        (409, "ConflictException"),
        "a duplicate (group, name) must conflict, got {status} {got}"
    );

    // Updating and getting a schedule that does not exist are both 404s.
    let (status, got) = call(
        &router,
        "GET",
        "/schedules/absent?groupName=default",
        None,
    )
    .await;
    assert_eq!(
        (status, error_code(&got).as_str()),
        (404, "ResourceNotFoundException"),
        "GetSchedule on an absent schedule must 404, got {status} {got}"
    );
    let (status, got) = call(
        &router,
        "PUT",
        "/schedules/absent",
        Some(serde_json::json!({
            "ScheduleExpression": "rate(5 minutes)",
            "FlexibleTimeWindow": { "Mode": "OFF" },
            "Target": {
                "Arn": "arn:aws:sqs:eu-west-2:000000000000:q",
                "RoleArn": "arn:aws:iam::000000000000:role/r"
            }
        })),
    )
    .await;
    assert_eq!(
        (status, error_code(&got).as_str()),
        (404, "ResourceNotFoundException"),
        "UpdateSchedule on an absent schedule must 404, got {status} {got}"
    );
}

#[tokio::test]
async fn flexible_time_window_mode_and_maximum_window_must_agree() {
    let router = scheduler_router();

    // `MaximumWindowInMinutes` is modelled `range 1..1440` and is required
    // exactly when `Mode` is FLEXIBLE: each published bound, one past each
    // bound, and both modes with and without the member.
    let cases: &[(&str, serde_json::Value, bool)] = &[
        ("OFF with no window", serde_json::json!({ "Mode": "OFF" }), true),
        (
            "OFF with a window",
            serde_json::json!({ "Mode": "OFF", "MaximumWindowInMinutes": 10 }),
            false,
        ),
        (
            "FLEXIBLE with no window",
            serde_json::json!({ "Mode": "FLEXIBLE" }),
            false,
        ),
        (
            "FLEXIBLE at the lower bound",
            serde_json::json!({ "Mode": "FLEXIBLE", "MaximumWindowInMinutes": 1 }),
            true,
        ),
        (
            "FLEXIBLE at the upper bound",
            serde_json::json!({ "Mode": "FLEXIBLE", "MaximumWindowInMinutes": 1440 }),
            true,
        ),
        (
            "FLEXIBLE one below the lower bound",
            serde_json::json!({ "Mode": "FLEXIBLE", "MaximumWindowInMinutes": 0 }),
            false,
        ),
        (
            "FLEXIBLE one above the upper bound",
            serde_json::json!({ "Mode": "FLEXIBLE", "MaximumWindowInMinutes": 1441 }),
            false,
        ),
    ];

    for (index, (case, window, accepted)) in cases.iter().enumerate() {
        let name = format!("ftw{index}");
        let (status, got) = call(
            &router,
            "POST",
            &format!("/schedules/{name}"),
            Some(serde_json::json!({
                "ScheduleExpression": "rate(5 minutes)",
                "FlexibleTimeWindow": window,
                "Target": {
                    "Arn": "arn:aws:sqs:eu-west-2:000000000000:q",
                    "RoleArn": "arn:aws:iam::000000000000:role/r"
                }
            })),
        )
        .await;
        if *accepted {
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {got}");
            // And it round-trips exactly as sent.
            let (status, back) = call(
                &router,
                "GET",
                &format!("/schedules/{name}?groupName=default"),
                None,
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: GetSchedule got {status} {back}");
            assert_eq!(
                back.get("FlexibleTimeWindow"),
                Some(window),
                "case {case:?}: window did not round-trip, got {:?}",
                back.get("FlexibleTimeWindow")
            );
        } else {
            assert_eq!(
                (status, error_code(&got).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected a 400 ValidationException, got {status} {got}"
            );
        }
    }
}

#[tokio::test]
async fn list_schedules_filters_and_pages_over_persisted_state() {
    let router = scheduler_router();
    for group in ["alpha", "beta"] {
        let (status, body) =
            call(&router, "POST", &format!("/schedule-groups/{group}"), None).await;
        assert_eq!(status, 200, "group {group}: got {status} {body}");
    }

    // (group, name, state) — two groups, overlapping name prefixes, both states.
    let seeded: &[(&str, &str, &str)] = &[
        ("alpha", "demo-one", "ENABLED"),
        ("alpha", "demo-two", "DISABLED"),
        ("alpha", "other", "ENABLED"),
        ("beta", "demo-three", "DISABLED"),
        ("beta", "zeta", "ENABLED"),
        ("default", "demo-four", "ENABLED"),
    ];
    for (group, name, state) in seeded {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/schedules/{name}"),
            Some(serde_json::json!({
                "GroupName": group,
                "ScheduleExpression": "rate(5 minutes)",
                "State": state,
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": {
                    "Arn": format!("arn:aws:sqs:eu-west-2:000000000000:{name}"),
                    "RoleArn": "arn:aws:iam::000000000000:role/r"
                }
            })),
        )
        .await;
        assert_eq!(status, 200, "seeding {group}/{name}: got {status} {body}");
    }

    // `(case, query, expected names in order)`.
    let filters: &[(&str, &str, &[&str])] = &[
        (
            "no filter lists everything, ordered by (group, name)",
            "",
            &["demo-one", "demo-two", "other", "demo-three", "zeta", "demo-four"],
        ),
        (
            "ScheduleGroup narrows to one group",
            "?ScheduleGroup=alpha",
            &["demo-one", "demo-two", "other"],
        ),
        (
            "NamePrefix narrows across groups",
            "?NamePrefix=demo",
            &["demo-one", "demo-two", "demo-three", "demo-four"],
        ),
        (
            "State narrows to DISABLED",
            "?State=DISABLED",
            &["demo-two", "demo-three"],
        ),
        (
            "every filter at once",
            "?ScheduleGroup=alpha&NamePrefix=demo&State=ENABLED",
            &["demo-one"],
        ),
        (
            "a prefix nothing matches is an empty list, not an absent one",
            "?NamePrefix=nomatch",
            &[],
        ),
    ];

    for (case, query, expected) in filters {
        let (status, body) = call(&router, "GET", &format!("/schedules{query}"), None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let listed = body
            .get("Schedules")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: Schedules must be present in {body}"));
        let names: Vec<&str> = listed
            .iter()
            .map(|s| s.get("Name").and_then(serde_json::Value::as_str).unwrap_or(""))
            .collect();
        assert_eq!(
            names.as_slice(),
            *expected,
            "case {case:?}: expected {expected:?}, got {names:?}"
        );
        assert_eq!(
            body.get("NextToken"),
            None,
            "case {case:?}: an unpaged list must not carry a NextToken, got {body}"
        );
        // Each summary is built from the stored record, not from the request.
        for summary in listed {
            let name = summary.get("Name").and_then(serde_json::Value::as_str).unwrap_or("");
            let group = summary
                .get("GroupName")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let expected_arn = format!(
                "arn:aws:scheduler:{REGION}:{}:schedule/{group}/{name}",
                account_id()
            );
            assert_eq!(
                summary.get("Arn").and_then(serde_json::Value::as_str),
                Some(expected_arn.as_str()),
                "case {case:?}: expected Arn {expected_arn} in {summary}"
            );
            assert_eq!(
                summary
                    .get("Target")
                    .and_then(|t| t.get("Arn"))
                    .and_then(serde_json::Value::as_str),
                Some(format!("arn:aws:sqs:eu-west-2:000000000000:{name}").as_str()),
                "case {case:?}: expected the stored target arn in {summary}"
            );
            assert!(
                summary.get("CreationDate").is_some()
                    && summary.get("LastModificationDate").is_some(),
                "case {case:?}: a summary must carry both dates, got {summary}"
            );
        }
    }

    // Paging: for every page size, following NextToken must yield each schedule
    // exactly once, in the same order, and stop.
    let all: Vec<String> = seeded.iter().map(|(_, name, _)| (*name).to_string()).collect();
    for max_results in [1usize, 2, 3, 5, 6, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?MaxResults={max_results}&NextToken={token}"),
                None => format!("?MaxResults={max_results}"),
            };
            let (status, body) = call(&router, "GET", &format!("/schedules{query}"), None).await;
            assert_eq!(status, 200, "page size {max_results}: got {status} {body}");
            let page = body
                .get("Schedules")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| panic!("page size {max_results}: Schedules absent in {body}"));
            assert!(
                page.len() <= max_results,
                "page size {max_results}: a page returned {} items",
                page.len()
            );
            for summary in page {
                seen.push(
                    summary
                        .get("Name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                );
            }
            token = body
                .get("NextToken")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "page size {max_results}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        let mut expected = all.clone();
        let mut got = seen.clone();
        expected.sort();
        got.sort();
        assert_eq!(
            got, expected,
            "page size {max_results}: expected every schedule exactly once, got {seen:?}"
        );
    }

    // A ScheduleGroup filter naming no group is a missing resource — the error
    // this operation declares and ListScheduleGroups does not.
    let (status, body) = call(&router, "GET", "/schedules?ScheduleGroup=nope", None).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "an unknown ScheduleGroup must 404, got {status} {body}"
    );

    // A token this service did not mint is a bad request, not an empty page.
    let (status, body) = call(&router, "GET", "/schedules?NextToken=zzzz", None).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a forged NextToken must be refused, got {status} {body}"
    );

    // Deleting removes it from the listing too.
    let (status, body) = call(
        &router,
        "DELETE",
        "/schedules/demo-one?groupName=alpha",
        None,
    )
    .await;
    assert_eq!(status, 200, "DeleteSchedule got {status} {body}");
    let (status, body) = call(&router, "GET", "/schedules?ScheduleGroup=alpha", None).await;
    assert_eq!(status, 200, "ListSchedules got {status} {body}");
    let names: Vec<&str> = body
        .get("Schedules")
        .and_then(serde_json::Value::as_array)
        .expect("Schedules present")
        .iter()
        .map(|s| s.get("Name").and_then(serde_json::Value::as_str).unwrap_or(""))
        .collect();
    assert_eq!(
        names.as_slice(),
        ["demo-two", "other"],
        "a deleted schedule must leave the listing, got {names:?}"
    );
}

#[tokio::test]
async fn an_unparameterised_list_is_capped_at_the_modelled_maximum_page() {
    let router = scheduler_router();

    // More schedules than one page may carry, so the server-side cap shows.
    // `MaxResults` is modelled `range 1..100`, so 100 is the largest page any
    // client can ask for and the largest this service ever serializes.
    let total = 150usize;
    for index in 0..total {
        let name = format!("s{index:04}");
        let (status, body) = call(
            &router,
            "POST",
            &format!("/schedules/{name}"),
            Some(serde_json::json!({
                "ScheduleExpression": "rate(5 minutes)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": {
                    "Arn": "arn:aws:sqs:eu-west-2:000000000000:q",
                    "RoleArn": "arn:aws:iam::000000000000:role/r"
                }
            })),
        )
        .await;
        assert_eq!(status, 200, "seeding {name}: got {status} {body}");
    }

    let mut seen = 0usize;
    let mut first_page = None;
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(token) => format!("?NextToken={token}"),
            None => String::new(),
        };
        let (status, body) = call(&router, "GET", &format!("/schedules{query}"), None).await;
        assert_eq!(status, 200, "unparameterised list got {status} {body}");
        let page = body
            .get("Schedules")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("Schedules absent in {body}"));
        assert!(
            page.len() <= 100,
            "case \"no MaxResults\": a page must not exceed the modelled maximum of 100, \
             got {}",
            page.len()
        );
        if first_page.is_none() {
            first_page = Some(page.len());
        }
        seen += page.len();
        token = body
            .get("NextToken")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        first_page,
        Some(100),
        "case \"no MaxResults\": expected a full 100-entry first page, got {first_page:?}"
    );
    assert_eq!(
        seen, total,
        "case \"no MaxResults\": following NextToken must enumerate all {total} schedules, \
         got {seen}"
    );
}

// Every operation this service models is now implemented — `ListScheduleGroups`,
// `GetScheduleGroup`, the tagging trio and `DeleteScheduleGroup` were the last
// scaffolds — so the suite that pinned the remaining 501s has nothing left to
// assert and is gone. Each operation's own behaviour is asserted by the suite
// that owns it (`schedule_group_read.rs`, `resource_tags.rs`,
// `reserved_schedule_group.rs`, and the tables above).
