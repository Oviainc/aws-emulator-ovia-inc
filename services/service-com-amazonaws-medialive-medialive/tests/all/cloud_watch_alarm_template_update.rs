// SPDX-License-Identifier: CC0-1.0

//! `UpdateCloudWatchAlarmTemplate`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — real HTTP requests
//! dispatched through `MultiServiceRouter` — so what is asserted is the
//! emulator's wire behaviour, not an internal function's return value.
//!
//! The rule these suites pin: an update writes the members its request supplies
//! onto the STORED template, leaves every member it omits as it was, stamps
//! `ModifiedAt` with the instant of the call, and reports the stored record — so
//! the operation's own response and every later `GetCloudWatchAlarmTemplate`
//! agree. The recorded case in the issue (one PATCH of one template) is ONE row
//! of the tables below, which span every patchable member, both forms of
//! `Identifier` ("Can be either be its id or current name"), every value of each
//! closed enum the member targets, each published numeric bound and one past it.

use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{epoch_seconds, member, timestamp, unix_seconds};
use crate::wire::{call_in_region, error_code, medialive, DEFAULT_REGION as REGION};

/// The collections the alarm template and its group are bound to, from
/// `smithy_query(query="operation", operation="UpdateCloudWatchAlarmTemplate")`
/// (PATCH /prod/cloudwatch-alarm-templates/{Identifier}) and
/// `operation="CreateCloudWatchAlarmTemplateGroup"`.
const TEMPLATES: &str = "/prod/cloudwatch-alarm-templates";
const GROUPS: &str = "/prod/cloudwatch-alarm-template-groups";

/// The bounds the input members' target shapes name: `Name` is
/// `__stringMin1Max255PatternS`, `Description` is `__stringMin0Max1024` and
/// `MetricName` is `__stringMax64` (`smithy_query(query="operation",
/// operation="UpdateCloudWatchAlarmTemplate")`).
const MAX_NAME: usize = 255;
const MAX_DESCRIPTION: usize = 1024;
const MAX_METRIC_NAME: usize = 64;

/// The bounds `Period` carries, from its target shape
/// `__integerMin10Max86400`, and the minimum `EvaluationPeriods` and
/// `DatapointsToAlarm` share, from `__integerMin1`.
const MIN_PERIOD: i64 = 10;
const MAX_PERIOD: i64 = 86_400;
const MIN_DATAPOINTS: i64 = 1;

/// Every `CloudWatchAlarmTemplateComparisonOperator` the model declares
/// (`smithy_query(query="shape",
/// shape="CloudWatchAlarmTemplateComparisonOperator")`).
const COMPARISON_OPERATORS: [&str; 4] = [
    "GreaterThanOrEqualToThreshold",
    "GreaterThanThreshold",
    "LessThanOrEqualToThreshold",
    "LessThanThreshold",
];

/// Every `CloudWatchAlarmTemplateStatistic` the model declares
/// (`smithy_query(query="shape", shape="CloudWatchAlarmTemplateStatistic")`).
const STATISTICS: [&str; 5] = ["Average", "Maximum", "Minimum", "SampleCount", "Sum"];

/// Every `CloudWatchAlarmTemplateTargetResourceType` the model declares
/// (`smithy_query(query="shape",
/// shape="CloudWatchAlarmTemplateTargetResourceType")`).
const TARGET_RESOURCE_TYPES: [&str; 10] = [
    "CLOUDFRONT_DISTRIBUTION",
    "ELEMENTAL_INFERENCE_FEED",
    "MEDIACONNECT_FLOW",
    "MEDIALIVE_CHANNEL",
    "MEDIALIVE_INPUT_DEVICE",
    "MEDIALIVE_MULTIPLEX",
    "MEDIAPACKAGE_CHANNEL",
    "MEDIAPACKAGE_ORIGIN_ENDPOINT",
    "MEDIATAILOR_PLAYBACK_CONFIGURATION",
    "S3_BUCKET",
];

/// Every `CloudWatchAlarmTemplateTreatMissingData` the model declares
/// (`smithy_query(query="shape",
/// shape="CloudWatchAlarmTemplateTreatMissingData")`).
const TREAT_MISSING_DATA: [&str; 4] = ["breaching", "ignore", "missing", "notBreaching"];

/// Every member `UpdateCloudWatchAlarmTemplateResponse` carries besides
/// `ModifiedAt` (`smithy_query(query="operation",
/// operation="UpdateCloudWatchAlarmTemplate")`), which is what an assertion walks
/// to check each one either took the patched value or kept the stored one.
const REPORTED_MEMBERS: [&str; 16] = [
    "arn",
    "comparisonOperator",
    "createdAt",
    "datapointsToAlarm",
    "description",
    "evaluationPeriods",
    "groupId",
    "id",
    "metricName",
    "name",
    "period",
    "statistic",
    "tags",
    "targetResourceType",
    "threshold",
    "treatMissingData",
];

/// The description and metric every template in these tables is created with, so
/// a patched value can be told from the stored one.
const INITIAL_DESCRIPTION: &str = "initial description";
const INITIAL_METRIC: &str = "InputVideoFrameRate";

async fn create_group(router: &MultiServiceRouter, name: &str) -> String {
    let (status, group) =
        call_in_region(router, REGION, "POST", GROUPS, Some(json!({ "name": name }))).await;
    assert_eq!(
        status, 201,
        "setup: expected the group {name:?} to be created, got {status}: {group}"
    );
    group["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the created group carries no id: {group}"))
        .to_string()
}

/// One cloudwatch alarm template in `group`, created with every required member
/// the model names plus the optional ones these tables patch over, answering the
/// body the create reported.
async fn create_template(router: &MultiServiceRouter, group: &str, name: &str) -> Value {
    let body = json!({
        "name": name,
        "description": INITIAL_DESCRIPTION,
        "groupIdentifier": group,
        "comparisonOperator": COMPARISON_OPERATORS[1],
        "datapointsToAlarm": 2,
        "evaluationPeriods": 3,
        "metricName": INITIAL_METRIC,
        "period": 60,
        "statistic": STATISTICS[0],
        "targetResourceType": TARGET_RESOURCE_TYPES[3],
        "threshold": 29.97,
        "treatMissingData": TREAT_MISSING_DATA[3],
    });
    let (status, created) = call_in_region(router, REGION, "POST", TEMPLATES, Some(body)).await;
    assert_eq!(
        status, 201,
        "setup: expected the template {name:?} to be created, got {status}: {created}"
    );
    created
}

async fn patch_in(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    body: Value,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "PATCH",
        &format!("{TEMPLATES}/{identifier}"),
        Some(body),
    )
    .await
}

async fn patch(router: &MultiServiceRouter, identifier: &str, body: Value) -> (u16, Value) {
    patch_in(router, REGION, identifier, body).await
}

async fn get_in(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("{TEMPLATES}/{identifier}"),
        None,
    )
    .await
}

async fn get(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    get_in(router, REGION, identifier).await
}

/// The id the create reported.
fn minted_id(case: &str, created: &Value) -> String {
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create reported no id: {created}"))
        .to_string()
}

/// Two reported members are the same value.
///
/// `Threshold` is a `__double`, so the same number may arrive as `29.97` or as
/// `30` depending on what the request supplied; comparing numbers numerically is
/// what keeps an integral double from reading as a different value than itself.
fn same_value(actual: &Value, expected: &Value) -> bool {
    match (actual.as_f64(), expected.as_f64()) {
        (Some(actual), Some(expected)) => actual == expected,
        _ => actual == expected,
    }
}

/// Assert `modifiedAt` falls in the window the call ran in, widened by a second
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

/// Every reported member is either the value this request supplied (`changed`)
/// or the value the store already held (`created`) — never a value the response
/// invented.
fn reports_patch_over_stored(case: &str, patched: &Value, created: &Value, changed: &Value) {
    for name in REPORTED_MEMBERS {
        let expected = changed
            .get(name)
            .cloned()
            .unwrap_or_else(|| member(created, name));
        let actual = member(patched, name);
        assert!(
            same_value(&actual, &expected),
            "case {case:?}: expected {name} to be {expected:?}, got {actual:?}"
        );
    }
}

/// Rule: every member an update supplies replaces the stored one on the stored
/// template, every member it omits keeps its value, and a `GroupIdentifier` is
/// resolved to the group's id before it is stored, so the template keeps
/// reporting its group out of state.
///
/// The rows differ along WHICH members the request carries and along the value,
/// size and type of each: free text at both published bounds and non-ASCII, a
/// closed-enum value the issue did not cite, each published numeric bound of
/// `Period`, a negative and an integral `Threshold`, a rename, a move to another
/// group by either form of its identifier, and every member at once.
#[tokio::test]
async fn every_member_an_alarm_template_update_supplies_replaces_the_stored_one() {
    let longest_name = "n".repeat(MAX_NAME);
    let longest_description = "d".repeat(MAX_DESCRIPTION);
    let longest_metric = "m".repeat(MAX_METRIC_NAME);

    let router = medialive();
    let group = create_group(&router, "alarm-template-update-group").await;
    let other_group = create_group(&router, "alarm-template-move-group").await;

    // (case, the PATCH body, the response members it must change)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "description only",
            json!({ "description": "updated alarm" }),
            json!({ "description": "updated alarm" }),
        ),
        (
            // `__stringMin0Max1024` admits the empty string.
            "an empty description",
            json!({ "description": "" }),
            json!({ "description": "" }),
        ),
        (
            "a description at 1024 characters",
            json!({ "description": longest_description.clone() }),
            json!({ "description": longest_description.clone() }),
        ),
        (
            "a non-ascii description",
            json!({ "description": "更新された説明" }),
            json!({ "description": "更新された説明" }),
        ),
        (
            "a comparison operator the issue did not cite",
            json!({ "comparisonOperator": COMPARISON_OPERATORS[2] }),
            json!({ "comparisonOperator": COMPARISON_OPERATORS[2] }),
        ),
        (
            "a statistic the issue did not cite",
            json!({ "statistic": STATISTICS[4] }),
            json!({ "statistic": STATISTICS[4] }),
        ),
        (
            "a target resource type the issue did not cite",
            json!({ "targetResourceType": TARGET_RESOURCE_TYPES[9] }),
            json!({ "targetResourceType": TARGET_RESOURCE_TYPES[9] }),
        ),
        (
            "a treat-missing-data the issue did not cite",
            json!({ "treatMissingData": TREAT_MISSING_DATA[0] }),
            json!({ "treatMissingData": TREAT_MISSING_DATA[0] }),
        ),
        (
            "the shortest period the model admits",
            json!({ "period": MIN_PERIOD }),
            json!({ "period": MIN_PERIOD }),
        ),
        (
            "the longest period the model admits",
            json!({ "period": MAX_PERIOD }),
            json!({ "period": MAX_PERIOD }),
        ),
        (
            "the fewest evaluation periods and datapoints the model admits",
            json!({ "evaluationPeriods": MIN_DATAPOINTS, "datapointsToAlarm": MIN_DATAPOINTS }),
            json!({ "evaluationPeriods": MIN_DATAPOINTS, "datapointsToAlarm": MIN_DATAPOINTS }),
        ),
        (
            "a negative threshold",
            json!({ "threshold": -12.5 }),
            json!({ "threshold": -12.5 }),
        ),
        (
            "an integral threshold",
            json!({ "threshold": 100 }),
            json!({ "threshold": 100 }),
        ),
        (
            "a metric name at 64 characters",
            json!({ "metricName": longest_metric.clone() }),
            json!({ "metricName": longest_metric.clone() }),
        ),
        (
            "renamed",
            json!({ "name": "renamed-alarm-template" }),
            json!({ "name": "renamed-alarm-template" }),
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
            json!({ "groupIdentifier": "alarm-template-move-group" }),
            json!({ "groupId": other_group.clone() }),
        ),
        (
            "every member at once",
            json!({
                "description": "everything",
                "groupIdentifier": "alarm-template-move-group",
                "name": "everything-alarm-template",
                "comparisonOperator": COMPARISON_OPERATORS[0],
                "datapointsToAlarm": 4,
                "evaluationPeriods": 5,
                "metricName": "OutputVideoFrameRate",
                "period": 300,
                "statistic": STATISTICS[2],
                "targetResourceType": TARGET_RESOURCE_TYPES[6],
                "threshold": 0.5,
                "treatMissingData": TREAT_MISSING_DATA[1],
            }),
            json!({
                "description": "everything",
                "groupId": other_group.clone(),
                "name": "everything-alarm-template",
                "comparisonOperator": COMPARISON_OPERATORS[0],
                "datapointsToAlarm": 4,
                "evaluationPeriods": 5,
                "metricName": "OutputVideoFrameRate",
                "period": 300,
                "statistic": STATISTICS[2],
                "targetResourceType": TARGET_RESOURCE_TYPES[6],
                "threshold": 0.5,
                "treatMissingData": TREAT_MISSING_DATA[1],
            }),
        ),
        (
            "nothing at all",
            // A request that carries no member asks for no change: everything
            // the template holds stays, and only `ModifiedAt` moves.
            json!({}),
            json!({}),
        ),
    ];

    for (index, (case, body, changed)) in rows.into_iter().enumerate() {
        let name = format!("alarm-template-{index}");
        let created = create_template(&router, &group, &name).await;
        let id = minted_id(case, &created);

        let before = SystemTime::now();
        let (status, patched) = patch(&router, &id, body).await;
        let after = SystemTime::now();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {patched}"
        );
        reports_patch_over_stored(case, &patched, &created, &changed);
        stamped_in_window(case, &patched, before, after);

        // ROUND TRIP: the GET reports the STORED template, reached by the id the
        // update never rewrites — this is the read the issue saw refused.
        let (status, read) = get(&router, &id).await;
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

/// Rule: a template is reached by either form of `Identifier` the model
/// documents ("Can be either be its id or current name"), so a PATCH addressed
/// by its name writes the same template a PATCH addressed by its id does.
#[tokio::test]
async fn an_update_reaches_its_template_by_id_or_by_name() {
    let router = medialive();
    let group = create_group(&router, "alarm-template-identifier-group").await;

    for form in ["id", "name"] {
        let name = format!("alarm-template-by-{form}");
        let created = create_template(&router, &group, &name).await;
        let case = format!("patched by {form}");
        let id = minted_id(&case, &created);
        let identifier = if form == "id" { id.as_str() } else { name.as_str() };

        let description = format!("patched by {form}");
        let (status, patched) =
            patch(&router, identifier, json!({ "description": description })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {patched}"
        );
        reports_patch_over_stored(
            &case,
            &patched,
            &created,
            &json!({ "description": description }),
        );

        // Both readers answer the stored template, whichever form wrote it.
        for (read_form, read_identifier) in [("id", id.as_str()), ("name", name.as_str())] {
            let (status, read) = get(&router, read_identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?} read by {read_form}: expected 200, got {status}: {read}"
            );
            assert_eq!(
                read, patched,
                "case {case:?} read by {read_form}: expected the updated template {patched}, got {read}"
            );
        }
    }
}

/// Rule: each of the four enum members a template carries takes EVERY value its
/// closed set declares, and a later read reports the last one written.
///
/// The rows are the model's own catalogs, walked entry by entry, so no value is
/// special-cased to the one the recording used.
#[tokio::test]
async fn every_value_of_each_closed_enum_can_be_patched() {
    // (the request member, the response member, the catalog the model declares)
    let catalogs: [(&str, &[&str]); 4] = [
        ("comparisonOperator", &COMPARISON_OPERATORS),
        ("statistic", &STATISTICS),
        ("targetResourceType", &TARGET_RESOURCE_TYPES),
        ("treatMissingData", &TREAT_MISSING_DATA),
    ];

    let router = medialive();
    let group = create_group(&router, "alarm-template-enum-group").await;

    for (member_name, values) in catalogs {
        let name = format!("alarm-template-enum-{member_name}");
        let created = create_template(&router, &group, &name).await;
        let id = minted_id(member_name, &created);

        for value in values {
            let case = format!("{member_name} = {value}");
            let (status, patched) = patch(&router, &id, json!({ member_name: value })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {patched}"
            );
            assert_eq!(
                member(&patched, member_name),
                json!(value),
                "case {case:?}: expected {member_name} to be {value:?}, got {:?}",
                member(&patched, member_name)
            );

            let (status, read) = get(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the template to read back, got {status}: {read}"
            );
            assert_eq!(
                member(&read, member_name),
                json!(value),
                "case {case:?}: expected the read to report {value:?}, got {:?}",
                member(&read, member_name)
            );
        }
    }
}

/// Rule: an update the service refuses writes NOTHING — the template still reads
/// back exactly as it was — and each refusal carries the status and code its
/// model declares: 404 `NotFoundException` for an identifier the request's region
/// names no template (or no group) for, 409 `ConflictException` for a name the
/// region already uses for another template, and a client error for a member
/// outside the constraint its shape names.
#[tokio::test]
async fn a_refused_update_changes_nothing() {
    let past_name_bound = "n".repeat(MAX_NAME + 1);
    let past_description_bound = "d".repeat(MAX_DESCRIPTION + 1);
    let past_metric_bound = "m".repeat(MAX_METRIC_NAME + 1);

    let router = medialive();
    let group = create_group(&router, "alarm-template-refusal-group").await;
    let subject = create_template(&router, &group, "alarm-template-refused").await;
    // A second template, so a rename onto a name the region already uses is a
    // collision with ANOTHER template rather than with the template itself.
    let neighbour = create_template(&router, &group, "alarm-template-neighbour").await;

    // (case, identifier to patch, body, expected status, expected code)
    let unminted = "9999999";
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "an identifier nothing minted",
            unminted,
            json!({ "description": "nowhere" }),
            404,
            "NotFoundException",
        ),
        (
            "a name nothing uses",
            "no-such-template",
            json!({ "description": "nowhere" }),
            404,
            "NotFoundException",
        ),
        (
            "moved into a group nothing holds",
            "alarm-template-refused",
            json!({ "groupIdentifier": "no-such-group" }),
            404,
            "NotFoundException",
        ),
        (
            "renamed onto another template's name",
            "alarm-template-refused",
            json!({ "name": "alarm-template-neighbour" }),
            409,
            "ConflictException",
        ),
        (
            "a comparison operator the enum does not declare",
            "alarm-template-refused",
            json!({ "comparisonOperator": "EqualToThreshold" }),
            400,
            "client error",
        ),
        (
            "a statistic the enum does not declare",
            "alarm-template-refused",
            json!({ "statistic": "Median" }),
            400,
            "client error",
        ),
        (
            "a target resource type the enum does not declare",
            "alarm-template-refused",
            json!({ "targetResourceType": "EC2_INSTANCE" }),
            400,
            "client error",
        ),
        (
            "a treat-missing-data the enum does not declare",
            "alarm-template-refused",
            json!({ "treatMissingData": "alarming" }),
            400,
            "client error",
        ),
        (
            "a period one below the bound its shape names",
            "alarm-template-refused",
            json!({ "period": MIN_PERIOD - 1 }),
            400,
            "client error",
        ),
        (
            "a period one above the bound its shape names",
            "alarm-template-refused",
            json!({ "period": MAX_PERIOD + 1 }),
            400,
            "client error",
        ),
        (
            "no evaluation periods at all",
            "alarm-template-refused",
            json!({ "evaluationPeriods": MIN_DATAPOINTS - 1 }),
            400,
            "client error",
        ),
        (
            "no datapoints to alarm at all",
            "alarm-template-refused",
            json!({ "datapointsToAlarm": MIN_DATAPOINTS - 1 }),
            400,
            "client error",
        ),
        (
            "a description past the bound its shape names",
            "alarm-template-refused",
            json!({ "description": past_description_bound.clone() }),
            400,
            "client error",
        ),
        (
            "a name past the bound its shape names",
            "alarm-template-refused",
            json!({ "name": past_name_bound.clone() }),
            400,
            "client error",
        ),
        (
            "an empty name",
            "alarm-template-refused",
            json!({ "name": "" }),
            400,
            "client error",
        ),
        (
            "a metric name past the bound its shape names",
            "alarm-template-refused",
            json!({ "metricName": past_metric_bound.clone() }),
            400,
            "client error",
        ),
    ];

    for (case, identifier, body, expected, expected_code) in rows {
        let (status, refused) = patch(&router, identifier, body).await;
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

        // Nothing moved: both templates still read back exactly as the create
        // reported them.
        for stored in [&subject, &neighbour] {
            let id = minted_id(case, stored);
            let (status, read) = get(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the template to still read back, got {status}: {read}"
            );
            assert_eq!(
                &read, stored,
                "case {case:?}: expected the refused update to leave the template as it was {stored}, got {read}"
            );
        }
    }
}

/// Rule: a template belongs to the region it was created in — `Name` is unique
/// "within the scope of a resource type in a specific region" — so an update
/// signed for another region finds nothing to write and leaves the template
/// alone.
#[tokio::test]
async fn an_update_signed_for_another_region_finds_nothing() {
    let elsewhere = "eu-west-1";
    let router = medialive();
    let group = create_group(&router, "alarm-template-region-group").await;
    let name = "alarm-template-region-scoped";
    let created = create_template(&router, &group, name).await;
    let id = minted_id(name, &created);

    for (form, identifier) in [("id", id.as_str()), ("name", name)] {
        let case = format!("patched in {elsewhere} by {form}");
        let (status, refused) = patch_in(
            &router,
            elsewhere,
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

    let (status, read) = get(&router, &id).await;
    assert_eq!(
        status, 200,
        "expected the template to still read back, got {status}: {read}"
    );
    assert_eq!(
        read, created,
        "expected the other region's update to change nothing, got {read}"
    );
}

/// Rule: successive updates each replace the previous value, so the template
/// reports the LAST write, its `CreatedAt` stays the creation instant and its
/// `ModifiedAt` never moves backwards.
#[tokio::test]
async fn successive_updates_each_replace_the_previous_value() {
    let router = medialive();
    let group = create_group(&router, "alarm-template-successive-group").await;
    let created = create_template(&router, &group, "alarm-template-successive").await;
    let id = minted_id("successive", &created);
    let mut previous = epoch_seconds(timestamp("successive", "createdAt", &created));

    for round in 0..3 {
        let case = format!("round {round}");
        let description = format!("description round {round}");
        let period = MIN_PERIOD + i64::from(round) * 10;
        let (status, patched) = patch(
            &router,
            &id,
            json!({ "description": description, "period": period }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {patched}"
        );

        let (status, read) = get(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer, got {status}: {read}"
        );
        assert_eq!(
            member(&read, "description"),
            json!(description),
            "case {case:?}: expected the latest description {description:?}, got {:?}",
            member(&read, "description")
        );
        assert_eq!(
            member(&read, "period"),
            json!(period),
            "case {case:?}: expected the latest period {period}, got {:?}",
            member(&read, "period")
        );
        assert_eq!(
            member(&read, "createdAt"),
            member(&created, "createdAt"),
            "case {case:?}: expected createdAt to stay the creation instant {:?}, got {:?}",
            member(&created, "createdAt"),
            member(&read, "createdAt")
        );

        let stamped = epoch_seconds(timestamp(&case, "modifiedAt", &read));
        assert!(
            stamped >= previous,
            "case {case:?}: expected modifiedAt not to move backwards from {previous}, got {stamped}"
        );
        previous = stamped;
    }
}
