// SPDX-License-Identifier: CC0-1.0

//! The MediaLive monitor-template UPDATE operations, end to end over the
//! restJson1 wire.
//!
//! Every test drives the service the way a client does — real HTTP requests
//! dispatched through `MultiServiceRouter` — so what is asserted is the
//! emulator's wire behaviour, not an internal function's return value.
//!
//! The rule these suites pin is one rule for three resources: an update writes
//! the members its request supplies onto the STORED resource, so the operation's
//! own response and every later read report the updated values. The recorded
//! case in the issue (one PATCH of one description) is ONE row of the tables
//! below, which span the three resources
//! (`UpdateCloudWatchAlarmTemplateGroup`, `UpdateEventBridgeRuleTemplateGroup`,
//! `UpdateEventBridgeRuleTemplate`), both forms of `Identifier` ("Can be either
//! be its id or current name"), and the value, size, type and count of the
//! members a request may carry.

use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{epoch_seconds, member, timestamp, unix_seconds};
use crate::wire::{call_in_region, error_code, medialive, ACCOUNT, DEFAULT_REGION as REGION};

/// The collection each resource is bound to, from
/// `smithy_query(query="operation", operation="UpdateCloudWatchAlarmTemplateGroup")`
/// (PATCH /prod/cloudwatch-alarm-template-groups/{Identifier}),
/// `operation="UpdateEventBridgeRuleTemplateGroup"` and
/// `operation="UpdateEventBridgeRuleTemplate"`.
const ALARM_GROUPS: &str = "/prod/cloudwatch-alarm-template-groups";
const RULE_GROUPS: &str = "/prod/eventbridge-rule-template-groups";
const RULE_TEMPLATES: &str = "/prod/eventbridge-rule-templates";

/// The three resources whose update this suite exercises: a slug for the case
/// label, and the collection its PATCH and its GET are bound to.
const RESOURCES: [(&str, &str); 3] = [
    ("alarm-group", ALARM_GROUPS),
    ("rule-group", RULE_GROUPS),
    ("rule-template", RULE_TEMPLATES),
];

/// The bounds the input members' target shapes name: `Description` is
/// `__stringMin0Max1024` and `Name` is `__stringMin1Max255PatternS`
/// (`smithy_query(query="operation", operation="UpdateEventBridgeRuleTemplate")`).
const MAX_DESCRIPTION: usize = 1024;
const MAX_NAME: usize = 255;

/// The most targets a template may carry:
/// `EventBridgeRuleTemplateSummary$EventTargetCount` ("The number of targets
/// configured to send matching events") targets `__integerMax5`
/// (`smithy_query(query="shape", shape="EventBridgeRuleTemplateSummary")`).
const MAX_TARGETS: usize = 5;

/// Every `EventBridgeRuleTemplateEventType` the model declares
/// (`smithy_query(query="shape", shape="EventBridgeRuleTemplateEventType")`).
/// The rows below patch values the issue did not cite, not only the recorded
/// one.
const EVENT_TYPES: [&str; 13] = [
    "MEDIALIVE_MULTIPLEX_ALERT",
    "MEDIALIVE_MULTIPLEX_STATE_CHANGE",
    "MEDIALIVE_CHANNEL_ALERT",
    "MEDIALIVE_CHANNEL_INPUT_CHANGE",
    "MEDIALIVE_CHANNEL_STATE_CHANGE",
    "MEDIAPACKAGE_INPUT_NOTIFICATION",
    "MEDIAPACKAGE_KEY_PROVIDER_NOTIFICATION",
    "MEDIAPACKAGE_HARVEST_JOB_NOTIFICATION",
    "SIGNAL_MAP_ACTIVE_ALARM",
    "MEDIACONNECT_ALERT",
    "MEDIACONNECT_SOURCE_HEALTH",
    "MEDIACONNECT_OUTPUT_HEALTH",
    "MEDIACONNECT_FLOW_STATUS_CHANGE",
];

/// Every member an eventbridge rule template's response shape carries besides
/// `ModifiedAt` (`smithy_query(query="operation",
/// operation="UpdateEventBridgeRuleTemplate")`), which is what an assertion walks
/// to check each one either took the patched value or kept the stored one. The
/// group responses carry a subset of these names, and `member` reads an absent
/// member as null, so one list serves all three resources.
const REPORTED_MEMBERS: [&str; 9] = [
    "arn",
    "createdAt",
    "description",
    "eventTargets",
    "eventType",
    "groupId",
    "id",
    "name",
    "tags",
];

/// The description every resource in these tables is created with, so a patched
/// value can be told from the stored one.
const INITIAL_DESCRIPTION: &str = "initial description";

async fn create(router: &MultiServiceRouter, collection: &str, body: Value) -> (u16, Value) {
    call_in_region(router, REGION, "POST", collection, Some(body)).await
}

async fn patch_in(
    router: &MultiServiceRouter,
    region: &str,
    collection: &str,
    identifier: &str,
    body: Value,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "PATCH",
        &format!("{collection}/{identifier}"),
        Some(body),
    )
    .await
}

async fn patch(
    router: &MultiServiceRouter,
    collection: &str,
    identifier: &str,
    body: Value,
) -> (u16, Value) {
    patch_in(router, REGION, collection, identifier, body).await
}

async fn get_in(
    router: &MultiServiceRouter,
    region: &str,
    collection: &str,
    identifier: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("{collection}/{identifier}"),
        None,
    )
    .await
}

async fn get(router: &MultiServiceRouter, collection: &str, identifier: &str) -> (u16, Value) {
    get_in(router, REGION, collection, identifier).await
}

/// An eventbridge rule template group to create templates inside, answered by
/// its minted id.
async fn host_group(router: &MultiServiceRouter, name: &str) -> String {
    let (status, group) = create(router, RULE_GROUPS, json!({ "name": name })).await;
    assert_eq!(
        status, 201,
        "setup: expected the host group {name:?} to be created, got {status}: {group}"
    );
    group["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the created group carries no id: {group}"))
        .to_string()
}

/// Create one resource of `collection` named `name`, described
/// [`INITIAL_DESCRIPTION`], and answer the body the create reported. A rule
/// template also needs the group it lives in and the event type it matches,
/// which its model makes required.
async fn create_resource(
    router: &MultiServiceRouter,
    collection: &str,
    group: &str,
    name: &str,
) -> Value {
    let body = if collection == RULE_TEMPLATES {
        json!({
            "name": name,
            "description": INITIAL_DESCRIPTION,
            "groupIdentifier": group,
            "eventType": EVENT_TYPES[2],
        })
    } else {
        json!({ "name": name, "description": INITIAL_DESCRIPTION })
    };
    let (status, created) = create(router, collection, body).await;
    assert_eq!(
        status, 201,
        "setup: expected {name:?} in {collection} to be created, got {status}: {created}"
    );
    created
}

/// An SNS topic ARN, which is one of the two ARN kinds
/// `EventBridgeRuleTemplateTarget$Arn` documents ("Target ARNs must be either an
/// SNS topic or CloudWatch log group").
fn sns_arn(index: usize) -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT}:topic-{index}")
}

/// A CloudWatch Logs log group ARN, the other kind that member documents.
fn log_group_arn(index: usize) -> String {
    format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:/aws/medialive/{index}")
}

/// Assert `modified_at` falls in the window the call ran in, widened by a second
/// each way because a wire timestamp may be rendered at second resolution.
///
/// The member's FORM is pinned too: every timestamp of this service targets
/// `__timestampIso8601`, which carries `@timestampFormat("date-time")`, so an
/// update reports it as an RFC 3339 string exactly as the create and the read
/// beside it do.
fn stamped_in_window(case: &str, body: &Value, before: SystemTime, after: SystemTime) {
    assert!(
        member(body, "modifiedAt").is_string(),
        "case {case:?}: expected modifiedAt as an RFC 3339 date-time string, got {:?}",
        member(body, "modifiedAt")
    );
    let stamped = epoch_seconds(timestamp(case, "modifiedAt", body));
    let (opened, closed) = (unix_seconds(before) - 1.0, unix_seconds(after) + 1.0);
    assert!(
        (opened..=closed).contains(&stamped),
        "case {case:?}: expected modifiedAt within the update's window [{opened}, {closed}], got {stamped} ({body})"
    );
}

/// Rule: an update writes the members its request supplies onto the stored
/// resource, so its own response and EVERY later read of that resource report
/// the updated values, while the members the request omitted keep theirs and
/// `ModifiedAt` becomes the instant of the call.
///
/// The table spans what the rule generalises over: the three resources whose
/// update carries a `Description`, the form of `Identifier` the PATCH addresses
/// the resource by ("Can be either be its id or current name"), and the value
/// and size of the description — including both published bounds of
/// `__stringMin0Max1024` and a non-ASCII value. The recorded case in the issue
/// is the "plain text" row.
#[tokio::test]
async fn a_patched_description_is_what_every_later_read_reports() {
    let longest = "d".repeat(MAX_DESCRIPTION);
    // (case, the description the PATCH supplies)
    let rows: [(&str, &str); 6] = [
        ("plain text", "updated description"),
        ("single character", "u"),
        // `__stringMin0Max1024` admits the empty string.
        ("empty", ""),
        ("at 1024 characters", longest.as_str()),
        ("non-ascii", "更新された説明"),
        ("the value it already had", INITIAL_DESCRIPTION),
    ];

    let router = medialive();
    let group = host_group(&router, "patched-description-group").await;

    for (resource, collection) in RESOURCES {
        for (index, (row, description)) in rows.iter().enumerate() {
            for form in ["id", "name"] {
                let case = format!("{resource} {row} patched by {form}");
                let name = format!("{resource}-described-{index}-{form}");
                let created = create_resource(&router, collection, &group, &name).await;
                let id = created["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: the create reported no id"))
                    .to_string();
                let identifier = if form == "id" { id.as_str() } else { name.as_str() };

                let before = SystemTime::now();
                let (status, patched) = patch(
                    &router,
                    collection,
                    identifier,
                    json!({ "description": description }),
                )
                .await;
                let after = SystemTime::now();
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status}: {patched}"
                );
                assert_eq!(
                    patched["description"],
                    json!(description),
                    "case {case:?}: expected the update to report the patched description {description:?}, got {:?}",
                    patched["description"]
                );

                // Everything the request did not carry keeps its stored value:
                // an update is a patch, not a replacement of the record.
                for kept in REPORTED_MEMBERS.iter().filter(|name| **name != "description") {
                    assert_eq!(
                        member(&patched, kept),
                        member(&created, kept),
                        "case {case:?}: expected {kept} to keep its stored value {:?}, got {:?}",
                        member(&created, kept),
                        member(&patched, kept)
                    );
                }
                stamped_in_window(&case, &patched, before, after);

                // ROUND TRIP: the reader answers the STORED resource, for either
                // form of identifier the model documents — this is the read the
                // issue saw answer the pre-update value.
                for (read_form, read_identifier) in [("id", id.as_str()), ("name", name.as_str())] {
                    let (status, read) = get(&router, collection, read_identifier).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?} read by {read_form}: expected 200, got {status}: {read}"
                    );
                    assert_eq!(
                        read, patched,
                        "case {case:?} read by {read_form}: expected the read to report the updated resource {patched}, got {read}"
                    );
                }
            }
        }
    }
}

/// Rule: every member an update supplies replaces the stored one — whatever its
/// type — and a `GroupIdentifier` is resolved to the group's id before it is
/// stored, so the template keeps reporting the group out of state.
///
/// The rows differ along WHICH members the request carries and along their type
/// and count: free text, a closed enum (values the issue did not cite), a list
/// of none, one and the five targets the model can count, a rename, a move to
/// another group by either form of its identifier, and every member at once.
#[tokio::test]
async fn every_member_a_rule_template_update_supplies_replaces_the_stored_one() {
    let longest_name = "n".repeat(MAX_NAME);
    let five_targets: Vec<Value> = (0..MAX_TARGETS)
        .map(|index| {
            if index.is_multiple_of(2) {
                json!({ "arn": sns_arn(index) })
            } else {
                json!({ "arn": log_group_arn(index) })
            }
        })
        .collect();

    let router = medialive();
    let group = host_group(&router, "rule-template-update-group").await;
    let other_group = host_group(&router, "rule-template-move-group").await;

    // (case, the PATCH body, the response members it must change)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "description only",
            json!({ "description": "updated channel alert rule" }),
            json!({ "description": "updated channel alert rule" }),
        ),
        (
            "event type only",
            json!({ "eventType": EVENT_TYPES[4] }),
            json!({ "eventType": EVENT_TYPES[4] }),
        ),
        (
            "another event type the issue did not cite",
            json!({ "eventType": EVENT_TYPES[12] }),
            json!({ "eventType": EVENT_TYPES[12] }),
        ),
        (
            "description and event type together",
            json!({ "description": "both", "eventType": EVENT_TYPES[8] }),
            json!({ "description": "both", "eventType": EVENT_TYPES[8] }),
        ),
        (
            "one target",
            json!({ "eventTargets": [{ "arn": sns_arn(1) }] }),
            json!({ "eventTargets": [{ "arn": sns_arn(1) }] }),
        ),
        (
            "as many targets as the model counts",
            json!({ "eventTargets": five_targets.clone() }),
            json!({ "eventTargets": five_targets.clone() }),
        ),
        (
            "no targets at all",
            // An empty list is a value the request supplied, so it replaces the
            // stored targets; the projection reports a template with none as
            // carrying no `EventTargets` member.
            json!({ "eventTargets": [] }),
            json!({ "eventTargets": null }),
        ),
        (
            "renamed",
            json!({ "name": "renamed-rule-template" }),
            json!({ "name": "renamed-rule-template" }),
        ),
        (
            "renamed to the longest name the model allows",
            json!({ "name": longest_name.clone() }),
            json!({ "name": longest_name.clone() }),
        ),
        (
            "moved to another group by its id",
            json!({ "groupIdentifier": other_group.clone() }),
            json!({ "groupId": other_group.clone() }),
        ),
        (
            "moved to another group by its name",
            json!({ "groupIdentifier": "rule-template-move-group" }),
            json!({ "groupId": other_group.clone() }),
        ),
        (
            "every member at once",
            json!({
                "description": "everything",
                "eventType": EVENT_TYPES[0],
                "eventTargets": [{ "arn": log_group_arn(7) }],
                "name": "everything-rule-template",
                "groupIdentifier": "rule-template-move-group",
            }),
            json!({
                "description": "everything",
                "eventType": EVENT_TYPES[0],
                "eventTargets": [{ "arn": log_group_arn(7) }],
                "name": "everything-rule-template",
                "groupId": other_group.clone(),
            }),
        ),
    ];

    for (index, (case, body, changed)) in rows.into_iter().enumerate() {
        let name = format!("rule-template-{index}");
        let created = create_resource(&router, RULE_TEMPLATES, &group, &name).await;
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create reported no id"))
            .to_string();

        let before = SystemTime::now();
        let (status, patched) = patch(&router, RULE_TEMPLATES, &id, body).await;
        let after = SystemTime::now();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {patched}"
        );

        // Each reported member is either the value this request supplied or the
        // value the store already held — never a value the response invented.
        for name in REPORTED_MEMBERS {
            let expected = changed
                .get(name)
                .cloned()
                .unwrap_or_else(|| member(&created, name));
            assert_eq!(
                member(&patched, name),
                expected,
                "case {case:?}: expected {name} to be {expected:?}, got {:?}",
                member(&patched, name)
            );
        }
        stamped_in_window(case, &patched, before, after);

        // ROUND TRIP: the GET reports the stored template, reached by the id the
        // update did not change.
        let (status, read) = get(&router, RULE_TEMPLATES, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated template to read back, got {status}: {read}"
        );
        assert_eq!(
            read, patched,
            "case {case:?}: expected the read to report the updated template {patched}, got {read}"
        );
    }
}

/// Rule: successive updates each replace the previous value, so the resource
/// reports the LAST write and its `ModifiedAt` never moves backwards.
#[tokio::test]
async fn successive_updates_each_replace_the_previous_value() {
    let router = medialive();
    let group = host_group(&router, "successive-group").await;

    for (resource, collection) in RESOURCES {
        let name = format!("{resource}-successive");
        let created = create_resource(&router, collection, &group, &name).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let mut previous = epoch_seconds(timestamp(resource, "createdAt", &created));

        for round in 0..3 {
            let description = format!("description round {round}");
            let case = format!("{resource} round {round}");
            let (status, patched) = patch(
                &router,
                collection,
                &id,
                json!({ "description": description }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {patched}"
            );

            let (status, read) = get(&router, collection, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the read to answer, got {status}: {read}"
            );
            assert_eq!(
                read["description"],
                json!(description),
                "case {case:?}: expected the latest description {description:?}, got {:?}",
                read["description"]
            );

            let stamped = epoch_seconds(timestamp(&case, "modifiedAt", &read));
            assert!(
                stamped >= previous,
                "case {case:?}: expected modifiedAt not to move backwards from {previous}, got {stamped}"
            );
            previous = stamped;
            assert_eq!(
                read["createdAt"], created["createdAt"],
                "case {case:?}: expected createdAt to stay the creation instant {:?}, got {:?}",
                created["createdAt"], read["createdAt"]
            );
        }
    }
}

/// Rule: an update the service refuses writes NOTHING — the resource still reads
/// back exactly as it was — and each refusal carries the status and code its
/// model declares: 404 `NotFoundException` for an identifier the request's region
/// has no resource for, 409 `ConflictException` for a name the region already
/// uses for another resource of the type, and a client error for a member outside
/// the constraint its model carries.
#[tokio::test]
async fn a_refused_update_changes_nothing() {
    let past_description_bound = "d".repeat(MAX_DESCRIPTION + 1);
    let six_targets: Vec<Value> = (0..=MAX_TARGETS)
        .map(|index| json!({ "arn": sns_arn(index) }))
        .collect();

    let router = medialive();
    let group = host_group(&router, "refusal-group").await;
    let mut stored: Vec<(&str, &str, Value)> = Vec::new();
    for (resource, collection) in RESOURCES {
        let name = format!("{resource}-refused");
        let created = create_resource(&router, collection, &group, &name).await;
        stored.push((resource, collection, created));
    }
    // A second template, so a rename onto a name the region already uses is a
    // collision with another template rather than with the template itself.
    let neighbour = create_resource(&router, RULE_TEMPLATES, &group, "rule-template-neighbour").await;

    // (case, collection, identifier to patch, body, expected status, expected code)
    let unminted = "9999999";
    let rows: Vec<(&str, &str, String, Value, u16, &str)> = vec![
        (
            "alarm group: identifier nothing minted",
            ALARM_GROUPS,
            unminted.to_string(),
            json!({ "description": "nowhere" }),
            404,
            "NotFoundException",
        ),
        (
            "rule group: name nothing uses",
            RULE_GROUPS,
            "no-such-group".to_string(),
            json!({ "description": "nowhere" }),
            404,
            "NotFoundException",
        ),
        (
            "rule template: identifier nothing minted",
            RULE_TEMPLATES,
            unminted.to_string(),
            json!({ "description": "nowhere" }),
            404,
            "NotFoundException",
        ),
        (
            "rule template: moved into a group nothing holds",
            RULE_TEMPLATES,
            "rule-template-refused".to_string(),
            json!({ "groupIdentifier": "no-such-group" }),
            404,
            "NotFoundException",
        ),
        (
            "rule template: renamed onto another template's name",
            RULE_TEMPLATES,
            "rule-template-refused".to_string(),
            json!({ "name": "rule-template-neighbour" }),
            409,
            "ConflictException",
        ),
        (
            "rule template: an event type the enum does not declare",
            RULE_TEMPLATES,
            "rule-template-refused".to_string(),
            json!({ "eventType": "NOT_AN_EVENT_TYPE" }),
            400,
            "client error",
        ),
        (
            "rule template: one target past what the model counts",
            RULE_TEMPLATES,
            "rule-template-refused".to_string(),
            json!({ "eventTargets": six_targets.clone() }),
            400,
            "client error",
        ),
        (
            "rule template: a target that is neither an SNS topic nor a log group",
            RULE_TEMPLATES,
            "rule-template-refused".to_string(),
            json!({ "eventTargets": [{ "arn": format!("arn:aws:s3:::bucket-{ACCOUNT}") }] }),
            400,
            "client error",
        ),
        (
            "alarm group: a description past the bound its shape names",
            ALARM_GROUPS,
            "alarm-group-refused".to_string(),
            json!({ "description": past_description_bound.clone() }),
            400,
            "client error",
        ),
        (
            "rule group: a description past the bound its shape names",
            RULE_GROUPS,
            "rule-group-refused".to_string(),
            json!({ "description": past_description_bound.clone() }),
            400,
            "client error",
        ),
    ];

    for (case, collection, identifier, body, expected, expected_code) in rows {
        let (status, refused) = patch(&router, collection, &identifier, body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}: {refused}"
        );
        let code = error_code(status, &refused);
        if expected_code == "client error" {
            assert!(
                code == "BadRequestException" || code == "ValidationException",
                "case {case:?}: expected the update to be refused as a client error, got {code} ({refused})"
            );
        } else {
            assert_eq!(
                code, expected_code,
                "case {case:?}: expected {expected_code}, got {refused}"
            );
        }

        // Nothing moved: every resource this suite created still reads back
        // exactly as the create reported it.
        for (resource, collection, created) in &stored {
            let id = created["id"].as_str().unwrap_or_default();
            let (status, read) = get(&router, collection, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the {resource} to still read back, got {status}: {read}"
            );
            assert_eq!(
                &read, created,
                "case {case:?}: expected the refused update to leave the {resource} as it was {created}, got {read}"
            );
        }
        let neighbour_id = neighbour["id"].as_str().unwrap_or_default();
        let (status, read) = get(&router, RULE_TEMPLATES, neighbour_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the neighbour template to still read back, got {status}: {read}"
        );
        assert_eq!(
            read, neighbour,
            "case {case:?}: expected the refused update to leave the neighbour as it was {neighbour}, got {read}"
        );
    }
}

/// Rule: a resource belongs to the region it was created in — `Name` is unique
/// "within the scope of a resource type in a specific region" — so an update
/// signed for another region finds nothing to write and leaves the resource
/// alone.
#[tokio::test]
async fn an_update_signed_for_another_region_finds_nothing() {
    let elsewhere = "eu-west-1";
    let router = medialive();
    let group = host_group(&router, "region-scoped-group").await;

    for (resource, collection) in RESOURCES {
        let name = format!("{resource}-region-scoped");
        let created = create_resource(&router, collection, &group, &name).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        for (form, identifier) in [("id", id.as_str()), ("name", name.as_str())] {
            let case = format!("{resource} patched in {elsewhere} by {form}");
            let (status, refused) = patch_in(
                &router,
                elsewhere,
                collection,
                identifier,
                json!({ "description": "written from the wrong region" }),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status}: {refused}"
            );
            assert_eq!(
                error_code(status, &refused),
                "NotFoundException",
                "case {case:?}: expected NotFoundException, got {refused}"
            );
        }

        let (status, read) = get(&router, collection, &id).await;
        assert_eq!(
            status, 200,
            "case {resource:?}: expected the resource to still read back, got {status}: {read}"
        );
        assert_eq!(
            read, created,
            "case {resource:?}: expected the other region's update to change nothing, got {read}"
        );
    }
}
