// SPDX-License-Identifier: CC0-1.0

//! One channel's schedule, end to end over the restJson1 wire:
//! `BatchUpdateSchedule`, `DescribeSchedule` and `DeleteSchedule`.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables spanning what each rule generalises over: the three
//! start modes, the action types, the start instants that order a schedule, and
//! the request shapes the schedule rules reject. The recorded case in the issue is
//! ONE row.

use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    MAX_LIST_PAGE_SIZE, MAX_SCHEDULE_ACTIONS, SCHEDULE_ACTION_RETENTION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, medialive};

/// A created channel to hang a schedule on. A schedule operation does not depend
/// on the channel's lifecycle state, so the test does not wait for `CREATING` to
/// settle.
async fn channel(router: &MultiServiceRouter) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": "scheduled-channel" })),
    )
    .await;
    assert_eq!(status, 201, "CreateChannel: expected 201, got {status}: {created}");
    created["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The schedule sub-resource of a channel
/// (`smithy_query(query="operation", operation="BatchUpdateSchedule")`:
/// PUT|GET|DELETE /prod/channels/{ChannelId}/schedule).
fn schedule_uri(channel_id: &str) -> String {
    format!("/prod/channels/{channel_id}/schedule")
}

async fn update(router: &MultiServiceRouter, channel_id: &str, body: Value) -> (u16, Value) {
    call(router, "PUT", &schedule_uri(channel_id), Some(body)).await
}

async fn describe(router: &MultiServiceRouter, channel_id: &str) -> (u16, Value) {
    call(router, "GET", &schedule_uri(channel_id), None).await
}

async fn delete(router: &MultiServiceRouter, channel_id: &str) -> (u16, Value) {
    call(router, "DELETE", &schedule_uri(channel_id), None).await
}

/// One `ScheduleAction` as the wire carries it: the three members the model marks
/// required (`smithy_query(query="shape", shape="ScheduleAction")`).
fn action(name: &str, settings: Value, start: Value) -> Value {
    json!({
        "actionName": name,
        "scheduleActionSettings": settings,
        "scheduleActionStartSettings": start,
    })
}

/// A `creates`-only `BatchUpdateSchedule` body.
fn creates(actions: Vec<Value>) -> Value {
    json!({ "creates": { "scheduleActions": actions } })
}

/// The action names `DescribeSchedule` reported, in the order it reported them.
fn reported_names(body: &Value) -> Vec<String> {
    body["scheduleActions"]
        .as_array()
        .map(|actions| {
            actions
                .iter()
                .map(|action| action["actionName"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// An instant as `FixedModeScheduleActionStartSettings$Time` spells it: "UTC
/// format: yyyy-mm-ddThh:mm:ss.nnnZ", which is what `Format::DateTime` renders.
fn utc(at: SystemTime) -> String {
    DateTime::from(at)
        .fmt(Format::DateTime)
        .expect("render the instant in UTC format")
}

/// An immediate-mode start, which "occurs as soon as possible".
fn immediately() -> Value {
    json!({ "immediateModeScheduleActionStartSettings": {} })
}

/// A fixed-mode start at `time`.
fn at_time(time: &str) -> Value {
    json!({ "fixedModeScheduleActionStartSettings": { "time": time } })
}

/// Rule: `BatchUpdateSchedule` stores each created action in the named channel's
/// schedule, and a later `DescribeSchedule` reports it back whole — whichever of
/// the three start modes and whichever action type it carries.
///
/// The table spans `ScheduleActionStartSettings`'s three options
/// (`smithy_query(query="shape", shape="ScheduleActionStartSettings")`: fixed,
/// follow, immediate) against several of `ScheduleActionSettings`'s action types
/// (`smithy_query(query="shape", shape="ScheduleActionSettings")`), including one
/// whose settings structure has no members at all.
#[tokio::test]
async fn a_created_action_round_trips_through_the_schedule() {
    // (case, settings, start settings)
    let table: [(&str, Value, Value); 6] = [
        (
            "immediate, timed metadata",
            json!({ "timedMetadataSettings": { "id3": "QUJD" } }),
            immediately(),
        ),
        (
            "fixed to the second, scte-35 return to network",
            json!({ "scte35ReturnToNetworkSettings": { "spliceEventId": 7 } }),
            at_time("2030-01-01T00:00:00Z"),
        ),
        (
            "fixed to the millisecond, static image deactivate",
            json!({ "staticImageDeactivateSettings": { "layer": 3, "fadeOut": 250 } }),
            at_time("2030-06-15T12:30:45.250Z"),
        ),
        (
            "immediate, an action type whose settings carry no members",
            json!({ "motionGraphicsImageDeactivateSettings": {} }),
            immediately(),
        ),
        (
            "immediate, hls timed metadata",
            json!({ "hlsTimedMetadataSettings": { "id3": "RUZH" } }),
            immediately(),
        ),
        (
            "immediate, pause state",
            json!({ "pauseStateSettings": {} }),
            immediately(),
        ),
    ];

    for (case, settings, start) in table {
        let router = medialive();
        let channel_id = channel(&router).await;
        let created = action("the-action", settings, start);

        let (status, body) = update(&router, &channel_id, creates(vec![created.clone()])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchUpdateSchedule: expected 200, got {status}: {body}"
        );
        assert_eq!(
            body["creates"]["scheduleActions"],
            json!([created]),
            "case {case:?}: expected the created action reported back, got {:?}",
            body["creates"]
        );
        assert!(
            body.get("deletes").is_none_or(Value::is_null),
            "case {case:?}: expected no deletes member for a creates-only request, got {:?}",
            body["deletes"]
        );

        let (status, read) = describe(&router, &channel_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSchedule: expected 200, got {status}: {read}"
        );
        assert_eq!(
            read["scheduleActions"],
            json!([created]),
            "case {case:?}: expected the stored action read back whole, got {:?}",
            read["scheduleActions"]
        );
        assert!(
            read.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: expected no nextToken on a one-action schedule, got {:?}",
            read["nextToken"]
        );
    }
}

/// Rule: a schedule is a timeline, so `DescribeSchedule` reports its actions in
/// the order they start in the channel — a fixed-mode action at its `Time`, an
/// immediate-mode one at the instant it was added, a follow-mode one where the
/// action it refers to starts — and paginates over exactly that sequence.
///
/// The table is the page size, spanning one action per page, a page that splits
/// the schedule, and a page larger than the whole schedule.
#[tokio::test]
async fn describe_schedule_reports_the_schedule_in_start_order_and_pages_over_it() {
    let router = medialive();
    let channel_id = channel(&router).await;

    // Created out of start order on purpose: the request order is not the
    // schedule order.
    let built = vec![
        action(
            "third",
            json!({ "scte35ReturnToNetworkSettings": { "spliceEventId": 3 } }),
            at_time("2032-01-01T00:00:00Z"),
        ),
        action(
            "first",
            json!({ "scte35ReturnToNetworkSettings": { "spliceEventId": 1 } }),
            at_time("2030-01-01T00:00:00Z"),
        ),
        action(
            "second",
            json!({ "scte35ReturnToNetworkSettings": { "spliceEventId": 2 } }),
            at_time("2031-01-01T00:00:00Z"),
        ),
        action(
            "with-the-third",
            json!({ "timedMetadataSettings": { "id3": "QUJD" } }),
            json!({
                "followModeScheduleActionStartSettings": {
                    "followPoint": "END",
                    "referenceActionName": "third",
                }
            }),
        ),
        action(
            "right-now",
            json!({ "motionGraphicsImageDeactivateSettings": {} }),
            immediately(),
        ),
    ];
    let (status, body) = update(&router, &channel_id, creates(built)).await;
    assert_eq!(status, 200, "BatchUpdateSchedule: expected 200, got {status}: {body}");

    // "right-now" starts at the instant it was added, which is before every fixed
    // time in the table; "with-the-third" starts where "third" does, so it follows
    // it.
    let expected = ["right-now", "first", "second", "third", "with-the-third"];

    let (status, read) = describe(&router, &channel_id).await;
    assert_eq!(status, 200, "DescribeSchedule: expected 200, got {status}: {read}");
    assert_eq!(
        reported_names(&read),
        expected,
        "expected the schedule in start order, got {:?}",
        read["scheduleActions"]
    );

    for page_size in [1_usize, 2, 4, 50] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let uri = match token.as_deref() {
                Some(token) => format!(
                    "{}?maxResults={page_size}&nextToken={token}",
                    schedule_uri(&channel_id)
                ),
                None => format!("{}?maxResults={page_size}", schedule_uri(&channel_id)),
            };
            let (status, page) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size}: expected 200, got {status}: {page}"
            );
            let names = reported_names(&page);
            assert!(
                names.len() <= page_size,
                "case maxResults={page_size}: expected at most {page_size} action(s) in a page, got {names:?}"
            );
            walked.extend(names);
            token = page["nextToken"].as_str().map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case maxResults={page_size}: pagination did not terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case maxResults={page_size}: expected the pages to walk the schedule in start order, got {walked:?}"
        );
    }
}

/// Rule: `DescribeSchedule`'s page size must be a positive count and its
/// `NextToken` must be a position in the schedule it was handed out for;
/// anything else is the operation's own `BadRequestException`.
#[tokio::test]
async fn describe_schedule_refuses_a_page_it_cannot_read() {
    let router = medialive();
    let channel_id = channel(&router).await;
    let (status, body) = update(
        &router,
        &channel_id,
        creates(vec![action(
            "only",
            json!({ "timedMetadataSettings": { "id3": "QUJD" } }),
            immediately(),
        )]),
    )
    .await;
    assert_eq!(status, 200, "BatchUpdateSchedule: expected 200, got {status}: {body}");

    // (case, query string)
    let table: [(&str, &str); 4] = [
        ("a page of none", "maxResults=0"),
        ("a negative page", "maxResults=-3"),
        ("a token that is not a position", "nextToken=halfway"),
        ("a token that is not a whole number", "nextToken=1.5"),
    ];
    for (case, query) in table {
        let uri = format!("{}?{query}", schedule_uri(&channel_id));
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected the operation's own BadRequestException, got {body}"
        );
    }

    // A position past the end of the schedule is a page the schedule no longer
    // has, not a bad request: the schedule shrinks on its own, so a walk that
    // began while a later page existed ends rather than fails.
    for position in [1_usize, 2, 999] {
        let uri = format!("{}?nextToken={position}", schedule_uri(&channel_id));
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case nextToken={position}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            body["scheduleActions"],
            json!([]),
            "case nextToken={position}: expected a page past the end to report no action, got {:?}",
            body["scheduleActions"]
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case nextToken={position}: expected no further token, got {:?}",
            body["nextToken"]
        );
    }
}

/// Rule: a schedule holds a finite number of actions, so a request that would
/// leave it past [`MAX_SCHEDULE_ACTIONS`] is refused with the operation's own
/// `TooManyRequestsException` (429) and stores nothing.
///
/// The table is the bound: the request that exactly fills the schedule, and the
/// one action past it.
#[tokio::test]
async fn batch_update_schedule_refuses_a_schedule_past_its_ceiling() {
    let built = |count: usize| -> Vec<Value> {
        (0..count)
            .map(|index| {
                action(
                    &format!("action-{index}"),
                    json!({ "scte35ReturnToNetworkSettings": { "spliceEventId": index } }),
                    immediately(),
                )
            })
            .collect()
    };

    // (case, how many actions the one request creates, the status it gets)
    let table: [(&str, usize, u16); 2] = [
        ("exactly the ceiling", MAX_SCHEDULE_ACTIONS, 200),
        ("one action past the ceiling", MAX_SCHEDULE_ACTIONS + 1, 429),
    ];
    for (case, count, expected_status) in table {
        let router = medialive();
        let channel_id = channel(&router).await;
        let (status, body) = update(&router, &channel_id, creates(built(count))).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {body}"
        );
        if expected_status == 429 {
            assert_eq!(
                error_code(status, &body),
                "TooManyRequestsException",
                "case {case:?}: expected TooManyRequestsException, got {body}"
            );
        }

        let (read_status, read) = describe(&router, &channel_id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: DescribeSchedule: got {read_status}: {read}"
        );
        let stored = read["scheduleActions"].as_array().map_or(0, Vec::len);
        // The schedule reports at most one page, so a filled one is compared
        // against the page size rather than against the whole count.
        let expected_stored = if expected_status == 200 {
            count.min(MAX_LIST_PAGE_SIZE)
        } else {
            0
        };
        assert_eq!(
            stored, expected_stored,
            "case {case:?}: expected {expected_stored} action(s) in the first page, got {stored}"
        );
    }
}

/// Rule: a `BatchUpdateSchedule` request is ONE update of the schedule — it is
/// refused with `UnprocessableEntityException` (422) and changes nothing unless
/// every action in it is one the schedule can hold: a name unique within the
/// schedule, exactly one action type, exactly one start mode, a fixed `Time` in
/// UTC format, a follow reference naming another action that will exist, and a
/// delete naming an action the schedule holds.
///
/// Each row names the member path the refusal reports it at, which is what
/// `UnprocessableEntityException$ValidationErrors` carries ("Path to the source of
/// the error").
#[tokio::test]
async fn batch_update_schedule_refuses_a_request_the_schedule_rules_reject() {
    let keeper = action(
        "keeper",
        json!({ "timedMetadataSettings": { "id3": "QUJD" } }),
        immediately(),
    );
    let one = json!({ "timedMetadataSettings": { "id3": "QUJD" } });

    // (case, request body, the element path the refusal names)
    let table: [(&str, Value, &str); 14] = [
        (
            "a name already in the schedule",
            creates(vec![action("keeper", one.clone(), immediately())]),
            "creates/scheduleActions/0/actionName",
        ),
        (
            "the same name created twice in one request",
            creates(vec![
                action("twice", one.clone(), immediately()),
                action("twice", one.clone(), immediately()),
            ]),
            "creates/scheduleActions/1/actionName",
        ),
        (
            "an empty name",
            creates(vec![action("", one.clone(), immediately())]),
            "creates/scheduleActions/0/actionName",
        ),
        (
            "no start mode at all",
            creates(vec![action("no-start", one.clone(), json!({}))]),
            "creates/scheduleActions/0/scheduleActionStartSettings",
        ),
        (
            "two start modes",
            creates(vec![action(
                "two-starts",
                one.clone(),
                json!({
                    "immediateModeScheduleActionStartSettings": {},
                    "fixedModeScheduleActionStartSettings": { "time": "2030-01-01T00:00:00Z" },
                }),
            )]),
            "creates/scheduleActions/0/scheduleActionStartSettings",
        ),
        (
            "a fixed time with no UTC Z",
            creates(vec![action(
                "local-time",
                one.clone(),
                at_time("2030-01-01T00:00:00"),
            )]),
            "creates/scheduleActions/0/scheduleActionStartSettings/fixedModeScheduleActionStartSettings/time",
        ),
        (
            "a fixed time at a numeric offset rather than UTC",
            creates(vec![action(
                "offset-time",
                one.clone(),
                at_time("2030-01-01T00:00:00+01:00"),
            )]),
            "creates/scheduleActions/0/scheduleActionStartSettings/fixedModeScheduleActionStartSettings/time",
        ),
        (
            "a fixed time that is not a time",
            creates(vec![action("soon", one.clone(), at_time("soonZ"))]),
            "creates/scheduleActions/0/scheduleActionStartSettings/fixedModeScheduleActionStartSettings/time",
        ),
        (
            "a fixed time naming a month that does not exist",
            creates(vec![action(
                "month-13",
                one.clone(),
                at_time("2030-13-01T00:00:00Z"),
            )]),
            "creates/scheduleActions/0/scheduleActionStartSettings/fixedModeScheduleActionStartSettings/time",
        ),
        (
            "a follow reference that is not in the schedule",
            creates(vec![action(
                "follower",
                one.clone(),
                json!({
                    "followModeScheduleActionStartSettings": {
                        "followPoint": "START",
                        "referenceActionName": "ghost",
                    }
                }),
            )]),
            "creates/scheduleActions/0/scheduleActionStartSettings/followModeScheduleActionStartSettings/referenceActionName",
        ),
        (
            "a follow reference to the action itself",
            creates(vec![action(
                "ouroboros",
                one.clone(),
                json!({
                    "followModeScheduleActionStartSettings": {
                        "followPoint": "START",
                        "referenceActionName": "ouroboros",
                    }
                }),
            )]),
            "creates/scheduleActions/0/scheduleActionStartSettings/followModeScheduleActionStartSettings/referenceActionName",
        ),
        (
            "no action type selected",
            creates(vec![action("no-settings", json!({}), immediately())]),
            "creates/scheduleActions/0/scheduleActionSettings",
        ),
        (
            "two action types selected",
            creates(vec![action(
                "two-settings",
                json!({
                    "timedMetadataSettings": { "id3": "QUJD" },
                    "staticImageDeactivateSettings": { "layer": 1 },
                }),
                immediately(),
            )]),
            "creates/scheduleActions/0/scheduleActionSettings",
        ),
        (
            "a delete naming an action the schedule does not hold",
            json!({ "deletes": { "actionNames": ["ghost"] } }),
            "deletes/actionNames/0",
        ),
    ];

    for (case, request, element_path) in table {
        let router = medialive();
        let channel_id = channel(&router).await;
        let (status, body) = update(&router, &channel_id, creates(vec![keeper.clone()])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: seeding the schedule: expected 200, got {status}: {body}"
        );

        let (status, body) = update(&router, &channel_id, request).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {body}"
        );
        assert_eq!(
            body["validationErrors"][0]["elementPath"],
            json!(element_path),
            "case {case:?}: expected the refusal to name {element_path}, got {:?}",
            body["validationErrors"]
        );

        // One update: the refused request wrote nothing, so the schedule is still
        // the one action it was seeded with.
        let (status, read) = describe(&router, &channel_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSchedule: expected 200, got {status}: {read}"
        );
        assert_eq!(
            reported_names(&read),
            ["keeper"],
            "case {case:?}: expected the refused request to leave the schedule alone, got {:?}",
            read["scheduleActions"]
        );
    }
}

/// Rule: a delete reports the action it removed as the schedule held it (the
/// request names only names), and once removed the name is no longer in the
/// schedule, so the same request may reuse it.
#[tokio::test]
async fn a_delete_reports_the_stored_action_and_frees_its_name() {
    let router = medialive();
    let channel_id = channel(&router).await;
    let was = action(
        "swap",
        json!({ "timedMetadataSettings": { "id3": "QUJD" } }),
        immediately(),
    );
    let (status, body) = update(&router, &channel_id, creates(vec![was.clone()])).await;
    assert_eq!(status, 200, "seeding the schedule: got {status}: {body}");

    // One request that frees the name and creates a different action under it.
    let now = action(
        "swap",
        json!({ "scte35ReturnToNetworkSettings": { "spliceEventId": 9 } }),
        at_time("2030-01-01T00:00:00Z"),
    );
    let (status, body) = update(
        &router,
        &channel_id,
        json!({
            "deletes": { "actionNames": ["swap"] },
            "creates": { "scheduleActions": [now.clone()] },
        }),
    )
    .await;
    assert_eq!(status, 200, "reusing the name: expected 200, got {status}: {body}");
    assert_eq!(
        body["deletes"]["scheduleActions"],
        json!([was]),
        "expected the deleted action reported as the schedule held it, got {:?}",
        body["deletes"]
    );
    assert_eq!(
        body["creates"]["scheduleActions"],
        json!([now]),
        "expected the created action reported back, got {:?}",
        body["creates"]
    );

    let (status, read) = describe(&router, &channel_id).await;
    assert_eq!(status, 200, "DescribeSchedule: got {status}: {read}");
    assert_eq!(
        read["scheduleActions"],
        json!([now]),
        "expected the reused name to carry the new action, got {:?}",
        read["scheduleActions"]
    );

    // A deletes-only request reports only the deletes side.
    let (status, body) = update(
        &router,
        &channel_id,
        json!({ "deletes": { "actionNames": ["swap"] } }),
    )
    .await;
    assert_eq!(status, 200, "deleting the action: got {status}: {body}");
    assert_eq!(
        body["deletes"]["scheduleActions"],
        json!([now]),
        "expected the deleted action reported, got {:?}",
        body["deletes"]
    );
    assert!(
        body.get("creates").is_none_or(Value::is_null),
        "expected no creates member for a deletes-only request, got {:?}",
        body["creates"]
    );

    let (status, read) = describe(&router, &channel_id).await;
    assert_eq!(status, 200, "DescribeSchedule: got {status}: {read}");
    assert_eq!(
        read["scheduleActions"],
        json!([]),
        "expected an emptied schedule to report no action, got {:?}",
        read["scheduleActions"]
    );
}

/// Rule: a follow reference names another action that exists once the whole
/// request has been applied — the request is one update, so the reference may
/// name an action created later in the same `creates` list — and a follower
/// starts where its reference does, so it is reported after it.
#[tokio::test]
async fn a_follow_reference_may_name_an_action_the_same_request_creates_later() {
    let router = medialive();
    let channel_id = channel(&router).await;
    let follower = action(
        "follower",
        json!({ "timedMetadataSettings": { "id3": "QUJD" } }),
        json!({
            "followModeScheduleActionStartSettings": {
                "followPoint": "END",
                "referenceActionName": "leader",
            }
        }),
    );
    let leader = action(
        "leader",
        json!({ "scte35ReturnToNetworkSettings": { "spliceEventId": 4 } }),
        at_time("2030-01-01T00:00:00Z"),
    );
    // The follower is listed FIRST, before the action it refers to.
    let (status, body) = update(
        &router,
        &channel_id,
        creates(vec![follower, leader]),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected a forward follow reference to be accepted, got {status}: {body}"
    );

    let (status, read) = describe(&router, &channel_id).await;
    assert_eq!(status, 200, "DescribeSchedule: got {status}: {read}");
    assert_eq!(
        reported_names(&read),
        ["leader", "follower"],
        "expected the follower reported after the action it follows, got {:?}",
        read["scheduleActions"]
    );
}

/// Rule: a delete reports the actions it removed in the order the request named
/// them, and a name repeated in one request is the one deletion.
#[tokio::test]
async fn a_multi_delete_reports_the_actions_in_the_order_it_was_asked_for() {
    let router = medialive();
    let channel_id = channel(&router).await;
    let built: Vec<Value> = ["alpha", "beta", "gamma"]
        .into_iter()
        .map(|name| {
            action(
                name,
                json!({ "timedMetadataSettings": { "id3": "QUJD" } }),
                immediately(),
            )
        })
        .collect();
    let (status, body) = update(&router, &channel_id, creates(built)).await;
    assert_eq!(status, 200, "seeding the schedule: got {status}: {body}");

    // Named out of lexicographic order, with one name repeated.
    let (status, body) = update(
        &router,
        &channel_id,
        json!({ "deletes": { "actionNames": ["gamma", "alpha", "gamma"] } }),
    )
    .await;
    assert_eq!(status, 200, "deleting two actions: got {status}: {body}");
    let deleted: Vec<&str> = body["deletes"]["scheduleActions"]
        .as_array()
        .map(|actions| {
            actions
                .iter()
                .map(|action| action["actionName"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        deleted,
        ["gamma", "alpha"],
        "expected the deleted actions in the order the request named them, once each, got {deleted:?}"
    );

    let (status, read) = describe(&router, &channel_id).await;
    assert_eq!(status, 200, "DescribeSchedule: got {status}: {read}");
    assert_eq!(
        reported_names(&read),
        ["beta"],
        "expected only the action the request left alone, got {:?}",
        read["scheduleActions"]
    );
}

/// Rule: `DeleteSchedule` deletes ALL the schedule actions on a channel, whatever
/// they are and however many, and a schedule that is already empty is the same
/// answer.
///
/// The table is the schedule it is called on, spanning none, one and several
/// actions.
#[tokio::test]
async fn delete_schedule_deletes_every_action_on_the_channel() {
    for count in [0_usize, 1, 3] {
        let router = medialive();
        let channel_id = channel(&router).await;
        if count > 0 {
            let built: Vec<Value> = (0..count)
                .map(|index| {
                    action(
                        &format!("action-{index}"),
                        json!({ "scte35ReturnToNetworkSettings": { "spliceEventId": index } }),
                        at_time(&format!("203{index}-01-01T00:00:00Z")),
                    )
                })
                .collect();
            let (status, body) = update(&router, &channel_id, creates(built)).await;
            assert_eq!(
                status, 200,
                "case {count} action(s): seeding: got {status}: {body}"
            );
        }

        let (status, body) = delete(&router, &channel_id).await;
        assert_eq!(
            status, 200,
            "case {count} action(s): DeleteSchedule: expected 200, got {status}: {body}"
        );

        let (status, read) = describe(&router, &channel_id).await;
        assert_eq!(
            status, 200,
            "case {count} action(s): DescribeSchedule: got {status}: {read}"
        );
        assert_eq!(
            read["scheduleActions"],
            json!([]),
            "case {count} action(s): expected no action left, got {:?}",
            read["scheduleActions"]
        );

        // Idempotent: deleting an already empty schedule is the same answer.
        let (status, body) = delete(&router, &channel_id).await;
        assert_eq!(
            status, 200,
            "case {count} action(s): second DeleteSchedule: expected 200, got {status}: {body}"
        );
    }
}

/// Rule: the schedule is cleaned up of actions whose start time is more than
/// [`SCHEDULE_ACTION_RETENTION`] in the past (`ScheduleAction$ActionName`: "The
/// schedule is automatically cleaned up to remove actions with a start time of
/// more than 1 hour ago (approximately) so at that point a name can be reused"),
/// so such an action is no longer in the schedule and its name is free again.
///
/// The table spans the retention bound: well past it, just past it, just inside
/// it, and a start time still to come.
#[tokio::test]
async fn an_action_whose_start_time_has_long_passed_is_no_longer_in_the_schedule() {
    let minute = Duration::from_secs(60);
    // (case, how far before now the action starts, still in the schedule?)
    let table: [(&str, Duration, bool); 4] = [
        ("twice the retention ago", SCHEDULE_ACTION_RETENTION * 2, false),
        (
            "a minute past the retention",
            SCHEDULE_ACTION_RETENTION + minute,
            false,
        ),
        (
            "a minute inside the retention",
            SCHEDULE_ACTION_RETENTION - minute,
            true,
        ),
        ("one minute ago", minute, true),
    ];

    for (case, ago, retained) in table {
        let router = medialive();
        let channel_id = channel(&router).await;
        let started = SystemTime::now()
            .checked_sub(ago)
            .expect("an instant that far back is representable");
        let past = action(
            "already-ran",
            json!({ "timedMetadataSettings": { "id3": "QUJD" } }),
            at_time(&utc(started)),
        );
        let (status, body) = update(&router, &channel_id, creates(vec![past])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchUpdateSchedule: expected 200, got {status}: {body}"
        );

        let (status, read) = describe(&router, &channel_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSchedule: got {status}: {read}"
        );
        let expected: Vec<String> = if retained {
            vec!["already-ran".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported_names(&read),
            expected,
            "case {case:?}: expected retained={retained}, got {:?}",
            read["scheduleActions"]
        );

        // A name that is no longer in the schedule is reusable; one still in it is
        // not.
        let again = action(
            "already-ran",
            json!({ "motionGraphicsImageDeactivateSettings": {} }),
            immediately(),
        );
        let (status, body) = update(&router, &channel_id, creates(vec![again])).await;
        let expected_status = if retained { 422 } else { 200 };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected reusing the name to answer {expected_status}, got {status}: {body}"
        );
    }
}

/// Rule: every schedule operation is scoped to the channel its URI names, so a
/// channel that does not exist is `NotFoundException` (404) rather than an empty
/// schedule.
///
/// The table is the three operations bound to the schedule sub-resource.
#[tokio::test]
async fn every_schedule_operation_refuses_a_channel_that_does_not_exist() {
    // (case, method, body)
    let table: [(&str, &str, Option<Value>); 3] = [
        (
            "BatchUpdateSchedule",
            "PUT",
            Some(creates(vec![action(
                "orphan",
                json!({ "timedMetadataSettings": { "id3": "QUJD" } }),
                immediately(),
            )])),
        ),
        ("DescribeSchedule", "GET", None),
        ("DeleteSchedule", "DELETE", None),
    ];

    for (case, method, body) in table {
        let router = medialive();
        let (status, response) =
            call(&router, method, &schedule_uri("9999999"), body.clone()).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {response}"
        );
        assert_eq!(
            error_code(status, &response),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {response}"
        );
    }
}
