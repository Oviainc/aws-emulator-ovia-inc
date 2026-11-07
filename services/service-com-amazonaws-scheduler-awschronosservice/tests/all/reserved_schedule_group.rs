// SPDX-License-Identifier: CC0-1.0

//! The schedule-group rules this service enforces on the WIRE: the reserved
//! built-in group, the deletion of an ordinary one, and the sentence a
//! constraint refusal is worded with.
//!
//! Three rules, each asserted over a SET of inputs rather than one request:
//!
//! 1. **A reserved, service-owned group name is a request-validation refusal.**
//!    EventBridge Scheduler owns the group named `default`: no client creates it
//!    and none deletes it, and either request is refused with HTTP 400
//!    `ValidationException` BEFORE anything is looked up — never as a collision
//!    with (or a successful change to) the resource that name already denotes.
//!    The rows run every operation that addresses a group by name against the
//!    reserved name, and the same operations against ordinary names (including
//!    `Default`, which only differs in case and so is NOT reserved).
//!
//! 2. **Deleting a group removes it, and the schedules it held, once the clock
//!    makes that removal due.** `DeleteScheduleGroup` answers while the group is
//!    still `DELETING` ("it remains in a DELETING state until all of its
//!    associated schedules are deleted … This operation is eventually
//!    consistent"), so a caller acting straight afterwards still sees the group;
//!    past the due instant the group and its schedules are gone and every
//!    operation naming them answers 404 `ResourceNotFoundException`. The rows
//!    vary how many schedules the group held (none, one, several) and the form
//!    of its name.
//!
//! 3. **A `@pattern` requirement is quoted without the model's anchors.** AWS
//!    builds a constraint message from the request value, the member path and
//!    the modelled constraint, and renders a pattern with the leading `^` and
//!    trailing `$` stripped. The rows span every pattern-constrained member this
//!    service's surface reaches — path labels and body members, two different
//!    pattern texts, an implemented operation and a still-scaffolded one (the
//!    refusal happens before any handler) — plus the other violation classes,
//!    which keep their own wording, and a request with two violations at once.

use serde_json::json;

use crate::wire_harness::{account_id, call, encode_label, error_code, group_arn, scheduler_router, REGION};

/// The group name EventBridge Scheduler reserves for the account's built-in
/// group (`crate::state::DEFAULT_SCHEDULE_GROUP`).
const RESERVED_GROUP: &str = "default";

/// The message AWS is recorded answering for an operation on the reserved group.
fn reserved_group_message(name: &str) -> String {
    format!("This operation cannot be performed on schedule group {name}.")
}

/// A `CreateSchedule` body naming `role` as the target's execution role. Only
/// this crate's own rules are under test here, so the body carries the required
/// members and nothing else.
fn schedule_body(group: Option<&str>) -> serde_json::Value {
    let mut body = json!({
        "ScheduleExpression": "rate(1 day)",
        "FlexibleTimeWindow": { "Mode": "OFF" },
        "Target": {
            "Arn": format!("arn:aws:sqs:{REGION}:{}:q", account_id()),
            "RoleArn": format!("arn:aws:iam::{}:role/scheduler-role", account_id()),
        }
    });
    if let Some(group) = group {
        body["GroupName"] = json!(group);
    }
    body
}

// ---------------------------------------------------------------------------
// 1. the reserved group name
// ---------------------------------------------------------------------------

/// One request addressed at a schedule group by name.
struct GroupCall {
    case: &'static str,
    method: &'static str,
    /// The group name in the URI.
    name: &'static str,
}

/// Every operation that addresses a group by name, against the reserved name.
fn reserved_group_calls() -> Vec<GroupCall> {
    vec![
        GroupCall {
            case: "CreateScheduleGroup on the reserved name",
            method: "POST",
            name: RESERVED_GROUP,
        },
        GroupCall {
            case: "DeleteScheduleGroup on the reserved name",
            method: "DELETE",
            name: RESERVED_GROUP,
        },
    ]
}

/// Names that are NOT reserved, spanning the accepted alphabet of
/// `ScheduleGroupName` (`^[0-9a-zA-Z-_.]+$`): a plain name, one differing from
/// the reserved name only in case, one carrying every punctuation character the
/// pattern allows, a digit-led one, and one at the modelled 64-character
/// maximum.
fn ordinary_group_names() -> Vec<(&'static str, String)> {
    vec![
        ("a plain name", "ordinary".to_string()),
        ("the reserved name in another case", "Default".to_string()),
        ("every allowed punctuation", "grp-1_2.3".to_string()),
        ("a digit-led name", "9lives".to_string()),
        ("the 64-character maximum", "g".repeat(64)),
    ]
}

#[tokio::test]
async fn the_reserved_group_name_is_refused_as_a_request_validation_failure() {
    let router = scheduler_router();

    // The reserved group's refusal message, which the modelled
    // `ValidationException` carries as `Message`; a refusal from the boundary
    // validator carries it as `message`, so both spellings are read.
    for GroupCall { case, method, name } in reserved_group_calls() {
        let (status, body) = call(&router, method, &format!("/schedule-groups/{name}"), None).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a reserved name must be refused as a validation failure, \
             got {status} {body}"
        );
        assert_eq!(
            body.get("Message")
                .or_else(|| body.get("message"))
                .and_then(serde_json::Value::as_str),
            Some(reserved_group_message(name).as_str()),
            "case {case:?}: expected the reserved-name refusal message, got {body}"
        );
    }

    // The refused delete changed nothing: the reserved group is still ACTIVE and
    // still holds schedules.
    let (status, body) = call(&router, "GET", "/schedule-groups/default", None).await;
    assert_eq!(
        (status, body.get("State").and_then(serde_json::Value::as_str)),
        (200, Some("ACTIVE")),
        "a refused delete must leave the reserved group ACTIVE, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        "POST",
        "/schedules/after-the-refusal",
        Some(schedule_body(None)),
    )
    .await;
    assert_eq!(
        status, 200,
        "the reserved group must survive a refused delete, got {status} {body}"
    );

    // An ordinary name is not reserved: it is created, and deleted, normally.
    for (case, name) in ordinary_group_names() {
        let uri = format!("/schedule-groups/{name}");
        let (status, body) = call(&router, "POST", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an ordinary group name must be created, got {status} {body}"
        );
        let (status, body) = call(&router, "DELETE", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an ordinary group must be deletable, got {status} {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. deleting an ordinary group
// ---------------------------------------------------------------------------

/// One row of the deletion table: a group name and the schedules it holds when
/// the delete arrives.
struct DeletionCase {
    case: &'static str,
    group: &'static str,
    schedules: &'static [&'static str],
}

/// The rows differ along what the rule generalises over: how many schedules the
/// group held when it was deleted (none, one, several) and the form of the group
/// name.
fn deletion_cases() -> Vec<DeletionCase> {
    vec![
        DeletionCase {
            case: "an empty group",
            group: "empty-group",
            schedules: &[],
        },
        DeletionCase {
            case: "a group holding one schedule",
            group: "one.schedule_group",
            schedules: &["only"],
        },
        DeletionCase {
            case: "a group holding several schedules",
            group: "Busy-9",
            schedules: &["a", "b.c", "d_e-1"],
        },
    ]
}

#[tokio::test]
async fn deleting_a_group_removes_it_and_its_schedules_when_the_clock_makes_that_due() {
    let router = scheduler_router();

    // A schedule in the implicit default group: it must still be there after
    // every row, so a delete never reaches past the group it names.
    let (status, body) = call(&router, "POST", "/schedules/bystander", Some(schedule_body(None))).await;
    assert_eq!(status, 200, "setup: the bystander schedule: {status} {body}");

    for DeletionCase {
        case,
        group,
        schedules,
    } in deletion_cases()
    {
        let group_uri = format!("/schedule-groups/{group}");
        let (status, body) = call(&router, "POST", &group_uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateScheduleGroup: got {status} {body}"
        );
        for name in schedules {
            let (status, body) = call(
                &router,
                "POST",
                &format!("/schedules/{name}"),
                Some(schedule_body(Some(group))),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateSchedule {name}: got {status} {body}"
            );
        }

        let (status, body) = call(&router, "DELETE", &group_uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteScheduleGroup: got {status} {body}"
        );

        // The call answered while the group is still DELETING, which a caller
        // acting straight afterwards must be able to observe: the group reads
        // back in that state, its schedules still read back, and it takes no new
        // schedule.
        let (status, body) = call(&router, "GET", &group_uri, None).await;
        assert_eq!(
            (status, body.get("State").and_then(serde_json::Value::as_str)),
            (200, Some("DELETING")),
            "case {case:?}: a deleted group is observable in DELETING, got {status} {body}"
        );
        for name in schedules {
            let (status, body) = call(
                &router,
                "GET",
                &format!("/schedules/{name}?groupName={group}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a schedule in a DELETING group is still readable, \
                 got {status} {body}"
            );
        }
        let (status, body) = call(
            &router,
            "POST",
            "/schedules/during-deletion",
            Some(schedule_body(Some(group))),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a DELETING group must take no new schedule, got {status} {body}"
        );

        // Past the due instant the removal the delete owed has happened.
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;

        let (status, body) = call(&router, "GET", &group_uri, None).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: GetScheduleGroup must report the group gone, got {status} {body}"
        );
        let (status, body) = call(
            &router,
            "GET",
            &format!("/schedules?ScheduleGroup={group}"),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the deleted group must be gone, got {status} {body}"
        );
        let (status, body) = call(&router, "DELETE", &group_uri, None).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: deleting an already-deleted group must answer 404, \
             got {status} {body}"
        );
        assert_eq!(
            body.get("Message")
                .or_else(|| body.get("message"))
                .and_then(serde_json::Value::as_str),
            Some(format!("Schedule group {group} does not exist.").as_str()),
            "case {case:?}: expected the group-does-not-exist message, got {body}"
        );
        for name in schedules {
            let (status, body) = call(
                &router,
                "GET",
                &format!("/schedules/{name}?groupName={group}"),
                None,
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: the schedules of a deleted group go with it, \
                 got {status} {body}"
            );
        }
        let (status, body) = call(
            &router,
            "POST",
            "/schedules/after-deletion",
            Some(schedule_body(Some(group))),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a deleted group holds no new schedule, got {status} {body}"
        );

        // The group's name is free again: a create under it succeeds.
        let (status, body) = call(&router, "POST", &group_uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the name of a removed group is free again, got {status} {body}"
        );
    }

    // Nothing outside the deleted groups was touched.
    let (status, body) = call(&router, "GET", "/schedules/bystander", None).await;
    assert_eq!(
        status, 200,
        "a schedule in another group must survive every delete, got {status} {body}"
    );
}

/// Every operation that addresses a group, with the group named `group` already
/// deleted: each must report it gone. A row is driven as the FIRST call after
/// the due instant, against its OWN group, so the reader under test is the one
/// that materialises the removal rather than reading what another call already
/// settled.
fn group_reader_probes() -> Vec<&'static str> {
    vec![
        "ListScheduleGroups",
        "ListTagsForResource",
        "TagResource",
        "UntagResource",
        "GetScheduleGroup",
    ]
}

#[tokio::test]
async fn every_group_reader_reports_a_removal_the_clock_has_made_due() {
    for probe in group_reader_probes() {
        let router = scheduler_router();
        let group = "settling-group";
        let group_uri = format!("/schedule-groups/{group}");
        let arn = group_arn(REGION, group);
        let tags_uri = format!("/tags/{}", encode_label(&arn));

        let (status, body) = call(&router, "POST", &group_uri, None).await;
        assert_eq!(
            status, 200,
            "probe {probe:?}: CreateScheduleGroup: got {status} {body}"
        );
        let (status, body) = call(&router, "DELETE", &group_uri, None).await;
        assert_eq!(
            status, 200,
            "probe {probe:?}: DeleteScheduleGroup: got {status} {body}"
        );

        tokio::time::sleep(std::time::Duration::from_millis(900)).await;

        // The one call this row makes past the due instant.
        match probe {
            "ListScheduleGroups" => {
                let (status, body) = call(&router, "GET", "/schedule-groups", None).await;
                assert_eq!(
                    status, 200,
                    "probe {probe:?}: ListScheduleGroups: got {status} {body}"
                );
                let listed: Vec<&str> = body
                    .get("ScheduleGroups")
                    .and_then(serde_json::Value::as_array)
                    .map(|groups| {
                        groups
                            .iter()
                            .filter_map(|entry| {
                                entry.get("Name").and_then(serde_json::Value::as_str)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                assert!(
                    !listed.contains(&group),
                    "probe {probe:?}: a removed group must not be listed, got {listed:?}"
                );
            }
            "ListTagsForResource" => {
                let (status, body) = call(&router, "GET", &tags_uri, None).await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (404, "ResourceNotFoundException"),
                    "probe {probe:?}: a removed group holds no tags to report, got \
                     {status} {body}"
                );
            }
            "TagResource" => {
                let (status, body) = call(
                    &router,
                    "POST",
                    &tags_uri,
                    Some(json!({ "Tags": [{ "Key": "env", "Value": "dev" }] })),
                )
                .await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (404, "ResourceNotFoundException"),
                    "probe {probe:?}: a removed group cannot be tagged, got {status} {body}"
                );
            }
            "UntagResource" => {
                let (status, body) =
                    call(&router, "DELETE", &format!("{tags_uri}?TagKeys=env"), None).await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (404, "ResourceNotFoundException"),
                    "probe {probe:?}: a removed group cannot be untagged, got {status} {body}"
                );
            }
            _ => {
                let (status, body) = call(&router, "GET", &group_uri, None).await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (404, "ResourceNotFoundException"),
                    "probe {probe:?}: a removed group must read back gone, got {status} {body}"
                );
            }
        }

        // Whatever the probe was, the removal it materialised is the one the
        // delete owed: the group is gone for every other reader too.
        let (status, body) = call(&router, "GET", &group_uri, None).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "probe {probe:?}: GetScheduleGroup after the probe, got {status} {body}"
        );
        let (status, body) = call(&router, "GET", "/schedule-groups", None).await;
        let listed = body.get("ScheduleGroups").map(ToString::to_string).unwrap_or_default();
        assert!(
            !listed.contains(group),
            "probe {probe:?}: ListScheduleGroups after the probe, got {status} {listed}"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. the constraint-refusal sentence
// ---------------------------------------------------------------------------
/// One pattern-constrained member, with the pattern the MODEL declares for it.
struct PatternCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<serde_json::Value>,
    /// The value the request carries for the member under test.
    value: String,
    /// The `@pattern` as the model spells it, anchors included.
    modelled: &'static str,
    /// The expression a message must quote: the same one, as AWS publishes it.
    published: &'static str,
}

/// `ScheduleGroupName`'s `@pattern`, which `CreateScheduleGroup$Name`,
/// `DeleteScheduleGroup$Name`, `GetScheduleGroup$Name` and
/// `CreateSchedule$GroupName` all carry
/// (`smithy_query(query="shape",shape="ScheduleGroupName")`).
const GROUP_NAME_PATTERN: &str = "^[0-9a-zA-Z-_.]+$";

/// The same expression as AWS publishes it in a message: the model's, without
/// its anchors. Spelled out rather than derived, so a wrong anchor-stripping
/// rule cannot be asserted against itself.
const GROUP_NAME_PUBLISHED: &str = "[0-9a-zA-Z-_.]+";

/// `Name`'s `@pattern`, carried by `CreateSchedule$Name`, and the form a message
/// publishes it in.
const SCHEDULE_NAME_PATTERN: &str = "^[0-9a-zA-Z-_.]+$";
const SCHEDULE_NAME_PUBLISHED: &str = "[0-9a-zA-Z-_.]+";

/// `ClientToken`'s `@pattern` — a DIFFERENT expression (no `.`), so the rows
/// below do not all render one text — and its published form.
const CLIENT_TOKEN_PATTERN: &str = "^[a-zA-Z0-9-_]+$";
const CLIENT_TOKEN_PUBLISHED: &str = "[a-zA-Z0-9-_]+";

/// A value outside every pattern above: `@` is in none of their character
/// classes.
const BAD_VALUE: &str = "sched@invalid";

/// Each pattern-constrained member this service's surface reaches, with the
/// shape of request that carries it.
fn pattern_cases() -> Vec<PatternCase> {
    vec![
        PatternCase {
            case: "CreateScheduleGroup Name (a path label)",
            method: "POST",
            uri: format!("/schedule-groups/{BAD_VALUE}"),
            body: None,
            value: BAD_VALUE.to_string(),
            modelled: GROUP_NAME_PATTERN,
            published: GROUP_NAME_PUBLISHED,
        },
        PatternCase {
            case: "DeleteScheduleGroup Name (a path label)",
            method: "DELETE",
            uri: format!("/schedule-groups/{BAD_VALUE}"),
            body: None,
            value: BAD_VALUE.to_string(),
            modelled: GROUP_NAME_PATTERN,
            published: GROUP_NAME_PUBLISHED,
        },
        PatternCase {
            // A read, refused before its handler runs: the wording is the
            // service's, not any one operation's.
            case: "GetScheduleGroup Name (a read)",
            method: "GET",
            uri: format!("/schedule-groups/{BAD_VALUE}"),
            body: None,
            value: BAD_VALUE.to_string(),
            modelled: GROUP_NAME_PATTERN,
            published: GROUP_NAME_PUBLISHED,
        },
        PatternCase {
            case: "CreateSchedule Name (a path label)",
            method: "POST",
            uri: format!("/schedules/{BAD_VALUE}"),
            body: Some(schedule_body(None)),
            value: BAD_VALUE.to_string(),
            modelled: SCHEDULE_NAME_PATTERN,
            published: SCHEDULE_NAME_PUBLISHED,
        },
        PatternCase {
            case: "CreateSchedule GroupName (a body member)",
            method: "POST",
            uri: "/schedules/well-formed".to_string(),
            body: Some(schedule_body(Some(BAD_VALUE))),
            value: BAD_VALUE.to_string(),
            modelled: GROUP_NAME_PATTERN,
            published: GROUP_NAME_PUBLISHED,
        },
        PatternCase {
            case: "CreateSchedule ClientToken (another pattern)",
            method: "POST",
            uri: "/schedules/well-formed".to_string(),
            body: Some({
                let mut body = schedule_body(None);
                body["ClientToken"] = json!("token.with.dots");
                body
            }),
            value: "token.with.dots".to_string(),
            modelled: CLIENT_TOKEN_PATTERN,
            published: CLIENT_TOKEN_PUBLISHED,
        },
        PatternCase {
            case: "UpdateSchedule Name (a path label on a PUT)",
            method: "PUT",
            uri: format!("/schedules/{BAD_VALUE}"),
            body: Some(schedule_body(None)),
            value: BAD_VALUE.to_string(),
            modelled: SCHEDULE_NAME_PATTERN,
            published: SCHEDULE_NAME_PUBLISHED,
        },
    ]
}

#[tokio::test]
async fn a_pattern_requirement_is_quoted_without_the_models_anchors() {
    let router = scheduler_router();

    for case in pattern_cases() {
        let (status, body) = call(&router, case.method, &case.uri, case.body.clone()).await;
        let message = body
            .get("Message")
            .or_else(|| body.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {:?}: a pattern violation must be refused 400 ValidationException, \
             got {status} {body}",
            case.case
        );
        let expected_tail = format!(
            "failed to satisfy constraint: Member must satisfy regular expression pattern: {}",
            case.published
        );
        assert!(
            message.ends_with(&expected_tail),
            "case {:?}: expected the message to end with {expected_tail:?}, got {message:?}",
            case.case
        );
        assert!(
            message.starts_with(&format!(
                "1 validation error detected: Value '{}' at '",
                case.value
            )),
            "case {:?}: expected the AWS envelope naming the rejected value, got {message:?}",
            case.case
        );
        assert!(
            !message.contains(case.modelled),
            "case {:?}: the anchored pattern must not be quoted, got {message:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn the_recorded_group_name_refusal_is_worded_exactly_as_aws_words_it() {
    let router = scheduler_router();
    // The one case a recording pins end to end: the member path is the lower
    // camel form of the member name, and the pattern is unanchored.
    let value = "sim2real-sched-grp-bad@invalid";
    let (status, body) = call(&router, "POST", &format!("/schedule-groups/{value}"), None).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "expected 400 ValidationException, got {status} {body}"
    );
    let expected = format!(
        "1 validation error detected: Value '{value}' at 'name' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z-_.]+"
    );
    assert_eq!(
        body.get("Message")
            .or_else(|| body.get("message"))
            .and_then(serde_json::Value::as_str),
        Some(expected.as_str()),
        "expected {expected:?}, got {body}"
    );
}

/// One row of the other-classes table: a request, and the requirement clause the
/// answer must carry.
struct ClassCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<serde_json::Value>,
    /// Substrings the message must contain, in no particular order.
    contains: Vec<String>,
}

/// Every other violation class this surface can produce keeps the framework's
/// own wording: only the pattern clause is reworded.
fn other_class_cases() -> Vec<ClassCase> {
    vec![
        ClassCase {
            case: "a name past the modelled 64-character maximum",
            method: "POST",
            uri: format!("/schedule-groups/{}", "g".repeat(65)),
            body: None,
            contains: vec!["Member must have length less than or equal to 64".to_string()],
        },
        ClassCase {
            case: "CreateSchedule with no Target",
            method: "POST",
            uri: "/schedules/missing-members".to_string(),
            body: Some(json!({
                "ScheduleExpression": "rate(1 day)",
                "FlexibleTimeWindow": { "Mode": "OFF" }
            })),
            contains: vec![
                "1 validation error detected".to_string(),
                "Member must not be null".to_string(),
            ],
        },
        ClassCase {
            case: "ListSchedules with MaxResults above the modelled range",
            method: "GET",
            uri: "/schedules?MaxResults=101".to_string(),
            body: None,
            contains: vec!["Member must have value less than or equal to 100".to_string()],
        },
    ]
}

#[tokio::test]
async fn every_other_violation_class_keeps_its_own_requirement_clause() {
    let router = scheduler_router();

    for case in other_class_cases() {
        let (status, body) = call(&router, case.method, &case.uri, case.body.clone()).await;
        let message = body
            .get("Message")
            .or_else(|| body.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {:?}: expected 400 ValidationException, got {status} {body}",
            case.case
        );
        for needle in &case.contains {
            assert!(
                message.contains(needle),
                "case {:?}: expected the message to carry {needle:?}, got {message:?}",
                case.case
            );
        }
    }
}
