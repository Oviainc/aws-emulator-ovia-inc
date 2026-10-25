// SPDX-License-Identifier: CC0-1.0

//! The form of a request-derived refusal message.
//!
//! The rule under test: a refusal message names what the request named, by the
//! names the service reports — the modelled member and enum for a cross-member
//! conflict, the schedule for a schedule that is absent — and adds no field path
//! or enclosing resource of the emulator's own invention.
//!
//! For the `FlexibleTimeWindow` conflict that means naming the member
//! `MaximumWindowInMinutes` of `com.amazonaws.scheduler#FlexibleTimeWindow` and
//! the enum `com.amazonaws.scheduler#FlexibleTimeWindowMode` carrying the mode
//! the request sent, not a dotted path the model does not define. The table
//! spans both halves of that conflict (provided-when-forbidden and
//! absent-when-required), every `FlexibleTimeWindowMode` value the model lists
//! (`OFF | FLEXIBLE`), and `MaximumWindowInMinutes` at each published bound of
//! its modelled range (1..1440) plus a value in between.
//!
//! For a schedule that is absent it means naming only the schedule, across every
//! operation that addresses one by name and whether the group was named or left
//! implicit.

use crate::wire_harness::{call, error_code, scheduler_router};

/// One row: the `FlexibleTimeWindow` sent, and the whole message expected back.
struct WindowCase {
    case: &'static str,
    window: serde_json::Value,
    /// `None` means the window is well formed and the request is served.
    message: Option<String>,
}

/// The target every request below carries; the window is what varies.
fn target() -> serde_json::Value {
    serde_json::json!({
        "Arn": "arn:aws:sqs:eu-west-2:000000000000:q",
        "RoleArn": "arn:aws:iam::000000000000:role/r"
    })
}

/// The refusal the rule builds for `mode` when the member is forbidden there.
fn forbidden(mode: &str) -> Option<String> {
    Some(format!(
        "MaximumWindowInMinutes must not be provided when FlexibleTimeWindowMode is set to {mode}."
    ))
}

/// The refusal the rule builds for `mode` when the member is required there.
fn required(mode: &str) -> Option<String> {
    Some(format!(
        "MaximumWindowInMinutes must be provided when FlexibleTimeWindowMode is set to {mode}."
    ))
}

/// `FlexibleTimeWindowMode` is modelled `OFF | FLEXIBLE`
/// (`com.amazonaws.scheduler#FlexibleTimeWindowMode`) and
/// `FlexibleTimeWindow$MaximumWindowInMinutes` is modelled `range 1..1440`.
fn window_cases() -> Vec<WindowCase> {
    vec![
        WindowCase {
            case: "OFF, no window: served",
            window: serde_json::json!({ "Mode": "OFF" }),
            message: None,
        },
        WindowCase {
            case: "OFF with the window at the lower bound",
            window: serde_json::json!({ "Mode": "OFF", "MaximumWindowInMinutes": 1 }),
            message: forbidden("OFF"),
        },
        WindowCase {
            case: "OFF with a window inside the range",
            window: serde_json::json!({ "Mode": "OFF", "MaximumWindowInMinutes": 15 }),
            message: forbidden("OFF"),
        },
        WindowCase {
            case: "OFF with the window at the upper bound",
            window: serde_json::json!({ "Mode": "OFF", "MaximumWindowInMinutes": 1440 }),
            message: forbidden("OFF"),
        },
        WindowCase {
            case: "FLEXIBLE with no window",
            window: serde_json::json!({ "Mode": "FLEXIBLE" }),
            message: required("FLEXIBLE"),
        },
        WindowCase {
            case: "FLEXIBLE with the window at the lower bound: served",
            window: serde_json::json!({ "Mode": "FLEXIBLE", "MaximumWindowInMinutes": 1 }),
            message: None,
        },
        WindowCase {
            case: "FLEXIBLE with the window at the upper bound: served",
            window: serde_json::json!({ "Mode": "FLEXIBLE", "MaximumWindowInMinutes": 1440 }),
            message: None,
        },
    ]
}

/// The `Message` a restJson1 error carries, or an empty string when it carries
/// none.
fn error_message(body: &serde_json::Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn create_schedule_reports_the_window_conflict_with_the_modelled_member_names() {
    let router = scheduler_router();

    for (index, WindowCase { case, window, message }) in window_cases().into_iter().enumerate() {
        let name = format!("ftw-msg-{index}");
        let (status, got) = call(
            &router,
            "POST",
            &format!("/schedules/{name}"),
            Some(serde_json::json!({
                "ScheduleExpression": "rate(5 minutes)",
                "FlexibleTimeWindow": window,
                "Target": target()
            })),
        )
        .await;

        match message {
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected the request to be served, got {status} {got}"
            ),
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&got).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: expected a 400 ValidationException, got {status} {got}"
                );
                let actual = error_message(&got);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected message {expected:?}, got {actual:?}"
                );
                // Nothing was persisted, so the schedule is still absent.
                let (status, got) = call(
                    &router,
                    "GET",
                    &format!("/schedules/{name}?groupName=default"),
                    None,
                )
                .await;
                assert_eq!(
                    (status, error_code(&got).as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {case:?}: a refused create must persist nothing, got {status} {got}"
                );
                let expected = format!("Schedule {name} does not exist.");
                let actual = error_message(&got);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected message {expected:?}, got {actual:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_absent_schedule_is_reported_by_its_own_name_alone() {
    let router = scheduler_router();
    let (status, got) = call(&router, "POST", "/schedule-groups/named", None).await;
    assert_eq!(status, 200, "the group to address: got {status} {got}");

    // Both readers that address a schedule by name through the URI, the group
    // left implicit and named, and names holding each character class the
    // modelled `Name` pattern (^[0-9a-zA-Z-_.]+$) allows.
    for name in ["absent", "Ab-sent_9.0"] {
        for group in [None, Some("named")] {
            let query = group.map_or_else(String::new, |group| format!("?groupName={group}"));
            for method in ["GET", "DELETE"] {
                let uri = format!("/schedules/{name}{query}");
                let case = format!("{method} {uri}");
                let (status, got) = call(&router, method, &uri, None).await;
                assert_eq!(
                    (status, error_code(&got).as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {case:?}: expected a 404 ResourceNotFoundException, got {status} {got}"
                );
                let expected = format!("Schedule {name} does not exist.");
                let actual = error_message(&got);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected message {expected:?}, got {actual:?}"
                );
            }

            // `UpdateSchedule` addresses the same schedule through its body.
            let mut update = serde_json::json!({
                "ScheduleExpression": "rate(5 minutes)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "Target": target()
            });
            if let Some(group) = group {
                update["GroupName"] = serde_json::json!(group);
            }
            let case = format!("PUT /schedules/{name} in group {group:?}");
            let (status, got) =
                call(&router, "PUT", &format!("/schedules/{name}"), Some(update)).await;
            assert_eq!(
                (status, error_code(&got).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected a 404 ResourceNotFoundException, got {status} {got}"
            );
            let expected = format!("Schedule {name} does not exist.");
            let actual = error_message(&got);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected message {expected:?}, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn update_schedule_reports_the_same_window_conflict_and_leaves_the_schedule_alone() {
    let router = scheduler_router();

    // One schedule to update, created with a window the rule accepts.
    let held = serde_json::json!({ "Mode": "FLEXIBLE", "MaximumWindowInMinutes": 30 });
    let (status, got) = call(
        &router,
        "POST",
        "/schedules/held",
        Some(serde_json::json!({
            "ScheduleExpression": "rate(5 minutes)",
            "FlexibleTimeWindow": held.clone(),
            "Target": target()
        })),
    )
    .await;
    assert_eq!(status, 200, "the schedule to update: got {status} {got}");

    for WindowCase { case, window, message } in window_cases() {
        let (status, got) = call(
            &router,
            "PUT",
            "/schedules/held",
            Some(serde_json::json!({
                "ScheduleExpression": "rate(5 minutes)",
                "FlexibleTimeWindow": window,
                "Target": target()
            })),
        )
        .await;

        let Some(expected) = message else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be served, got {status} {got}"
            );
            continue;
        };
        assert_eq!(
            (status, error_code(&got).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} {got}"
        );
        let actual = error_message(&got);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected message {expected:?}, got {actual:?}"
        );
    }

    // The last served row left its own window behind; put the held one back and
    // check a refused update changes nothing a later reader sees.
    let (status, got) = call(
        &router,
        "PUT",
        "/schedules/held",
        Some(serde_json::json!({
            "ScheduleExpression": "rate(5 minutes)",
            "FlexibleTimeWindow": held.clone(),
            "Target": target()
        })),
    )
    .await;
    assert_eq!(status, 200, "restoring the held window: got {status} {got}");
    let (status, got) = call(
        &router,
        "PUT",
        "/schedules/held",
        Some(serde_json::json!({
            "ScheduleExpression": "rate(5 minutes)",
            "FlexibleTimeWindow": { "Mode": "OFF", "MaximumWindowInMinutes": 15 },
            "Target": target()
        })),
    )
    .await;
    assert_eq!(
        (status, error_code(&got).as_str()),
        (400, "ValidationException"),
        "a refused update: got {status} {got}"
    );
    let (status, back) = call(&router, "GET", "/schedules/held?groupName=default", None).await;
    assert_eq!(status, 200, "reading the held schedule back: got {status} {back}");
    assert_eq!(
        back.get("FlexibleTimeWindow"),
        Some(&held),
        "a refused update must leave the stored window alone, got {:?}",
        back.get("FlexibleTimeWindow")
    );
}
