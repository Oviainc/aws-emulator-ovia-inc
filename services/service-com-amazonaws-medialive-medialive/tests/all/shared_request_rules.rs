// SPDX-License-Identifier: CC0-1.0

//! The request rules this crate states ONCE, driven end to end through every
//! operation that applies them.
//!
//! Every test drives the service the way a client does — real HTTP requests
//! dispatched through `MultiServiceRouter` — so what is asserted is the
//! emulator's wire behaviour, not an internal function's return value.
//!
//! The rules pinned here:
//!
//! 1. One length rule for every operation that holds a caller-chosen string to
//!    the bounds its target shape names: the refusal names the member, the
//!    bound and the length that broke it, counted in CHARACTERS, and reads
//!    identically whichever operation answered it.
//! 2. One settings rule for the cloudwatch alarm template: the create and the
//!    update hold the same members of the same resource to the same closed enum
//!    catalogs, the same numeric ranges and the same thresholds, and refuse a
//!    value outside them with the same message.
//! 3. One floor of one for the counts the reservation operations carry.
//!
//! Each table spans the class its rule generalises over — which OPERATION
//! applies it, which MEMBER it is applied to, the length or value supplied and
//! whether that character is one byte or two — so the recorded case of any one
//! operation is one row of it.

use serde_json::{json, Map, Value};
use service_com_amazonaws_medialive_medialive::ops::types::{
    CLOUD_WATCH_ALARM_TEMPLATE_COMPARISON_OPERATOR_VALUES,
    CLOUD_WATCH_ALARM_TEMPLATE_STATISTIC_VALUES,
    CLOUD_WATCH_ALARM_TEMPLATE_TARGET_RESOURCE_TYPE_VALUES,
    CLOUD_WATCH_ALARM_TEMPLATE_TREAT_MISSING_DATA_VALUES,
};
use service_com_amazonaws_medialive_medialive::state::{
    MAX_ALARM_PERIOD_SECONDS, MAX_METRIC_NAME_LEN, MAX_REQUEST_ID_LEN,
    MAX_RESOURCE_DESCRIPTION_LEN, MAX_RESOURCE_NAME_LEN, MIN_ALARM_DATAPOINTS,
    MIN_ALARM_PERIOD_SECONDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, error_message, medialive};

/// The collections these operations are bound to, from
/// `smithy_query(query="operation", operation="<Operation>")`: POST
/// /prod/cloudwatch-alarm-templates and PATCH
/// /prod/cloudwatch-alarm-templates/{Identifier}, PATCH
/// /prod/cloudwatch-alarm-template-groups/{Identifier}, POST
/// /prod/eventbridge-rule-template-groups and PATCH
/// /prod/eventbridge-rule-template-groups/{Identifier}, POST /prod/clusters and
/// PUT /prod/clusters/{ClusterId}, POST
/// /prod/offerings/{OfferingId}/purchase and PUT
/// /prod/reservations/{ReservationId}.
const ALARM_TEMPLATES: &str = "/prod/cloudwatch-alarm-templates";
const ALARM_TEMPLATE_GROUPS: &str = "/prod/cloudwatch-alarm-template-groups";
const RULE_TEMPLATE_GROUPS: &str = "/prod/eventbridge-rule-template-groups";
const CLUSTERS: &str = "/prod/clusters";
const OFFERINGS: &str = "/prod/offerings";
const RESERVATIONS: &str = "/prod/reservations";

/// The only `ClusterType` the model declares
/// (`smithy_query(query="shape", shape="ClusterType")`: ON_PREMISES).
const ON_PREMISES: &str = "ON_PREMISES";

/// Which operation a row drives the shared rule through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    CreateAlarmTemplate,
    UpdateAlarmTemplate,
    UpdateAlarmTemplateGroup,
    CreateRuleTemplateGroup,
    UpdateRuleTemplateGroup,
    CreateCluster,
    UpdateCluster,
    PurchaseOffering,
    UpdateReservation,
}

impl Op {
    /// The status this operation answers a request it serves with
    /// (`smithy_query(query="operation", operation="<Operation>")`: 201 for a
    /// create and a purchase, 200 for an update).
    fn served(self) -> u16 {
        match self {
            Self::CreateAlarmTemplate
            | Self::CreateRuleTemplateGroup
            | Self::CreateCluster
            | Self::PurchaseOffering => 201,
            Self::UpdateAlarmTemplate
            | Self::UpdateAlarmTemplateGroup
            | Self::UpdateRuleTemplateGroup
            | Self::UpdateCluster
            | Self::UpdateReservation => 200,
        }
    }
}

/// A request body carrying exactly one member.
fn one_member(member: &str, value: Value) -> Value {
    let mut body = Map::new();
    body.insert(member.to_string(), value);
    Value::Object(body)
}

/// `body` with `member` set to `value`.
fn with_member(body: Value, member: &str, value: Value) -> Value {
    let mut body = body;
    body[member] = value;
    body
}

/// One cloudwatch alarm template group, answering its minted id.
async fn alarm_template_group(router: &MultiServiceRouter) -> String {
    let (status, group) = call(
        router,
        "POST",
        ALARM_TEMPLATE_GROUPS,
        Some(json!({ "name": "shared-rules" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the alarm template group to be created, got {status}: {group}"
    );
    group["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the created group carries no id: {group}"))
        .to_string()
}

/// Every REQUIRED member of `CreateCloudWatchAlarmTemplate`, with one modelled
/// value each, so a row that varies ONE member still sends a complete request.
fn alarm_template_body(name: &str, group: &str) -> Value {
    json!({
        "name": name,
        "groupIdentifier": group,
        "comparisonOperator": CLOUD_WATCH_ALARM_TEMPLATE_COMPARISON_OPERATOR_VALUES[1],
        "evaluationPeriods": 1,
        "metricName": "InputVideoFrameRate",
        "period": 60,
        "statistic": CLOUD_WATCH_ALARM_TEMPLATE_STATISTIC_VALUES[0],
        "targetResourceType": CLOUD_WATCH_ALARM_TEMPLATE_TARGET_RESOURCE_TYPE_VALUES[3],
        "threshold": 29.97,
        "treatMissingData": CLOUD_WATCH_ALARM_TEMPLATE_TREAT_MISSING_DATA_VALUES[3],
    })
}

/// One cloudwatch alarm template, answering its minted id.
async fn alarm_template(router: &MultiServiceRouter, group: &str) -> String {
    let (status, template) = call(
        router,
        "POST",
        ALARM_TEMPLATES,
        Some(alarm_template_body("shared-rules", group)),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the alarm template to be created, got {status}: {template}"
    );
    template["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the created template carries no id: {template}"))
        .to_string()
}

/// One eventbridge rule template group, answering its minted id.
async fn rule_template_group(router: &MultiServiceRouter) -> String {
    let (status, group) = call(
        router,
        "POST",
        RULE_TEMPLATE_GROUPS,
        Some(json!({ "name": "shared-rules" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the rule template group to be created, got {status}: {group}"
    );
    group["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the created group carries no id: {group}"))
        .to_string()
}

/// One cluster, answering its minted id.
async fn cluster(router: &MultiServiceRouter) -> String {
    let (status, created) = call(
        router,
        "POST",
        CLUSTERS,
        Some(json!({ "name": "shared-rules", "clusterType": ON_PREMISES })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the cluster to be created, got {status}: {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the created cluster carries no id: {created}"))
        .to_string()
}

/// The id of the first offering the catalog holds, read off `ListOfferings`, so
/// a purchase names a configuration this service really sells.
async fn offering(router: &MultiServiceRouter) -> String {
    let (status, body) = call(router, "GET", &format!("{OFFERINGS}?maxResults=1"), None).await;
    assert_eq!(
        status, 200,
        "setup: expected ListOfferings to answer 200, got {status}: {body}"
    );
    body["offerings"][0]["offeringId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: expected the catalog to hold an offering, got {body}"))
        .to_string()
}

/// One purchased reservation, answering its minted id.
async fn reservation(router: &MultiServiceRouter) -> String {
    let offering_id = offering(router).await;
    let uri = format!("{OFFERINGS}/{offering_id}/purchase");
    let (status, purchased) = call(router, "POST", &uri, Some(json!({ "count": 1 }))).await;
    assert_eq!(
        status, 201,
        "setup: expected the reservation to be purchased, got {status}: {purchased}"
    );
    purchased["reservation"]["reservationId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the purchase carries no reservation id: {purchased}"))
        .to_string()
}

/// Drive `op` with a request whose own members are `members`, on a router of its
/// own, and answer the status and body it responded with.
///
/// One dispatch path for every row of every table below: each row says only
/// which operation it drives and what its request carries, and this is where the
/// resources that request needs are created.
async fn drive(op: Op, members: Value) -> (u16, Value) {
    let router = medialive();
    match op {
        Op::CreateAlarmTemplate => {
            let group = alarm_template_group(&router).await;
            let mut body = alarm_template_body("under-test", &group);
            for (member, value) in members.as_object().cloned().unwrap_or_default() {
                body[member.as_str()] = value;
            }
            call(&router, "POST", ALARM_TEMPLATES, Some(body)).await
        }
        Op::UpdateAlarmTemplate => {
            let group = alarm_template_group(&router).await;
            let id = alarm_template(&router, &group).await;
            let uri = format!("{ALARM_TEMPLATES}/{id}");
            call(&router, "PATCH", &uri, Some(members)).await
        }
        Op::UpdateAlarmTemplateGroup => {
            let id = alarm_template_group(&router).await;
            let uri = format!("{ALARM_TEMPLATE_GROUPS}/{id}");
            call(&router, "PATCH", &uri, Some(members)).await
        }
        Op::CreateRuleTemplateGroup => {
            // `Name` is required, so a row that is about another member still
            // sends one; a row that IS about `Name` keeps the value it chose.
            let body = match members.get("name") {
                Some(_) => members,
                None => with_member(members, "name", json!("under-test")),
            };
            call(&router, "POST", RULE_TEMPLATE_GROUPS, Some(body)).await
        }
        Op::UpdateRuleTemplateGroup => {
            let id = rule_template_group(&router).await;
            let uri = format!("{RULE_TEMPLATE_GROUPS}/{id}");
            call(&router, "PATCH", &uri, Some(members)).await
        }
        Op::CreateCluster => {
            let body = with_member(members, "clusterType", json!(ON_PREMISES));
            call(&router, "POST", CLUSTERS, Some(body)).await
        }
        Op::UpdateCluster => {
            let id = cluster(&router).await;
            let uri = format!("{CLUSTERS}/{id}");
            call(&router, "PUT", &uri, Some(members)).await
        }
        Op::PurchaseOffering => {
            let offering_id = offering(&router).await;
            let uri = format!("{OFFERINGS}/{offering_id}/purchase");
            let body = match members.get("count") {
                Some(_) => members,
                None => with_member(members, "count", json!(1)),
            };
            call(&router, "POST", &uri, Some(body)).await
        }
        Op::UpdateReservation => {
            let id = reservation(&router).await;
            let uri = format!("{RESERVATIONS}/{id}");
            call(&router, "PUT", &uri, Some(members)).await
        }
    }
}

/// Assert `op` served the request `members` asked for.
async fn served(case: &str, op: Op, members: Value) {
    let (status, body) = drive(op, members).await;
    assert_eq!(
        status,
        op.served(),
        "case {case:?}: expected {op:?} to serve the request, got {status}: {body}"
    );
}

/// Assert `op` refused the request `members` asked for as the modelled 400,
/// carrying exactly `message`.
async fn refused(case: &str, op: Op, members: Value, message: &str) {
    let (status, body) = drive(op, members).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {op:?} to refuse the request, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "BadRequestException",
        "case {case:?}: expected the modelled client error, got {body}"
    );
    assert_eq!(
        error_message(&body),
        message,
        "case {case:?}: expected the shared refusal wording, got {:?}",
        error_message(&body)
    );
}

/// Rule 1: a caller-chosen string is held to the length bounds its target shape
/// names, counted in characters, and a string outside them is refused with the
/// modelled 400 naming the member, the bound and the length that broke it —
/// identically for every operation that holds a string to a bound, because it is
/// one rule.
///
/// The rows span which OPERATION applies the rule (each of the operations that
/// held its own copy of it), which MEMBER it is applied to (each member each one
/// bounds), the length supplied (the published bound, one past it, and one below
/// the floor where the floor is one) and whether the characters are one byte or
/// two, because the rule counts characters rather than bytes.
#[tokio::test]
async fn one_length_rule_for_every_operation_that_bounds_a_string() {
    // (the operation, the member it bounds, the floor, the ceiling)
    let rows: [(Op, &str, usize, usize); 18] = [
        (
            Op::CreateAlarmTemplate,
            "name",
            1,
            MAX_RESOURCE_NAME_LEN,
        ),
        (
            Op::CreateAlarmTemplate,
            "description",
            0,
            MAX_RESOURCE_DESCRIPTION_LEN,
        ),
        (
            Op::CreateAlarmTemplate,
            "requestId",
            1,
            MAX_REQUEST_ID_LEN,
        ),
        (
            Op::CreateAlarmTemplate,
            "metricName",
            0,
            MAX_METRIC_NAME_LEN,
        ),
        (
            Op::UpdateAlarmTemplate,
            "name",
            1,
            MAX_RESOURCE_NAME_LEN,
        ),
        (
            Op::UpdateAlarmTemplate,
            "description",
            0,
            MAX_RESOURCE_DESCRIPTION_LEN,
        ),
        (
            Op::UpdateAlarmTemplate,
            "metricName",
            0,
            MAX_METRIC_NAME_LEN,
        ),
        (
            Op::UpdateAlarmTemplateGroup,
            "description",
            0,
            MAX_RESOURCE_DESCRIPTION_LEN,
        ),
        (
            Op::CreateRuleTemplateGroup,
            "name",
            1,
            MAX_RESOURCE_NAME_LEN,
        ),
        (
            Op::CreateRuleTemplateGroup,
            "description",
            0,
            MAX_RESOURCE_DESCRIPTION_LEN,
        ),
        (
            Op::CreateRuleTemplateGroup,
            "requestId",
            1,
            MAX_REQUEST_ID_LEN,
        ),
        (
            Op::UpdateRuleTemplateGroup,
            "description",
            0,
            MAX_RESOURCE_DESCRIPTION_LEN,
        ),
        (Op::CreateCluster, "name", 1, MAX_RESOURCE_NAME_LEN),
        (Op::CreateCluster, "requestId", 1, MAX_REQUEST_ID_LEN),
        (Op::UpdateCluster, "name", 1, MAX_RESOURCE_NAME_LEN),
        (Op::PurchaseOffering, "name", 1, MAX_RESOURCE_NAME_LEN),
        (Op::PurchaseOffering, "requestId", 1, MAX_REQUEST_ID_LEN),
        (Op::UpdateReservation, "name", 1, MAX_RESOURCE_NAME_LEN),
    ];

    // A one-byte character and a two-byte one, so a row at the bound proves the
    // rule counts characters: 255 two-byte characters are 510 bytes.
    let fills: [(&str, char); 2] = [("one-byte characters", 'c'), ("two-byte characters", 'é')];

    for (op, member, min, max) in rows {
        for (fill_case, fill) in fills {
            let case = format!("{op:?} bounds {member} to {min}..={max}, {fill_case}");

            served(
                &format!("{case}: at the ceiling"),
                op,
                one_member(member, json!(fill.to_string().repeat(max))),
            )
            .await;

            let past = max + 1;
            refused(
                &format!("{case}: one past the ceiling"),
                op,
                one_member(member, json!(fill.to_string().repeat(past))),
                &format!(
                    "{member} must be between {min} and {max} characters long, but was {past}"
                ),
            )
            .await;

            if min == 0 {
                served(
                    &format!("{case}: the empty string"),
                    op,
                    one_member(member, json!("")),
                )
                .await;
                continue;
            }

            served(
                &format!("{case}: at the floor"),
                op,
                one_member(member, json!(fill.to_string().repeat(min))),
            )
            .await;
            refused(
                &format!("{case}: below the floor"),
                op,
                one_member(member, json!("")),
                &format!("{member} must be between {min} and {max} characters long, but was 0"),
            )
            .await;
        }
    }
}

/// Rule 2: the cloudwatch alarm template's create and its update hold the same
/// members of the same resource to the same settings rule — the closed enum
/// catalogs the model declares, the numeric ranges the members' target shapes
/// name and a threshold that is a real number — so a value either one serves the
/// other serves, and a value either one refuses the other refuses with the same
/// message.
///
/// The rows span which rule is under test (the closed sets, each numeric range
/// and the threshold), the value supplied (every value of each catalog, each
/// published bound and one past it) and the operation, which is what the rule
/// generalises over: both operations run every row.
#[tokio::test]
async fn one_settings_rule_for_the_alarm_template_create_and_update() {
    let both = [Op::CreateAlarmTemplate, Op::UpdateAlarmTemplate];

    // Every value of every closed set the four enum members target is served by
    // both operations. The catalogs are the model's own.
    let catalogs: [(&str, &[&str]); 4] = [
        (
            "comparisonOperator",
            CLOUD_WATCH_ALARM_TEMPLATE_COMPARISON_OPERATOR_VALUES,
        ),
        ("statistic", CLOUD_WATCH_ALARM_TEMPLATE_STATISTIC_VALUES),
        (
            "targetResourceType",
            CLOUD_WATCH_ALARM_TEMPLATE_TARGET_RESOURCE_TYPE_VALUES,
        ),
        (
            "treatMissingData",
            CLOUD_WATCH_ALARM_TEMPLATE_TREAT_MISSING_DATA_VALUES,
        ),
    ];

    for op in both {
        for (member, values) in catalogs {
            for value in values {
                served(
                    &format!("{op:?} serves {member} = {value}"),
                    op,
                    one_member(member, json!(value)),
                )
                .await;
            }
        }
    }

    // (the member, its floor, its ceiling) — `Period` is
    // `__integerMin10Max86400`, and `EvaluationPeriods` and `DatapointsToAlarm`
    // are both `__integerMin1`.
    let ranges: [(&str, i64, i64); 3] = [
        (
            "period",
            i64::from(MIN_ALARM_PERIOD_SECONDS),
            i64::from(MAX_ALARM_PERIOD_SECONDS),
        ),
        (
            "evaluationPeriods",
            i64::from(MIN_ALARM_DATAPOINTS),
            i64::from(i32::MAX),
        ),
        (
            "datapointsToAlarm",
            i64::from(MIN_ALARM_DATAPOINTS),
            i64::from(i32::MAX),
        ),
    ];

    for op in both {
        for (member, min, max) in ranges {
            for value in [min, max] {
                served(
                    &format!("{op:?} serves {member} = {value}"),
                    op,
                    one_member(member, json!(value)),
                )
                .await;
            }
            for value in [min - 1, -7] {
                refused(
                    &format!("{op:?} refuses {member} = {value}"),
                    op,
                    one_member(member, json!(value)),
                    &format!("{member} must be between {min} and {max}, but was {value}"),
                )
                .await;
            }
        }
    }

    // `Threshold` is a `__double` with no modelled bound, so every real number
    // is a threshold both operations serve.
    for op in both {
        for threshold in [0.0, -29.97, 1e308, -1e308, f64::MIN_POSITIVE] {
            served(
                &format!("{op:?} serves threshold = {threshold}"),
                op,
                one_member("threshold", json!(threshold)),
            )
            .await;
        }
    }
}

/// Rule 3: a count the model floors at one is refused below that floor, with the
/// modelled 400 naming the member and the count that broke it — the same rule,
/// and the same wording, for the purchase and the update beside it.
///
/// The rows span the operation, the member each one counts and the value supplied
/// (the floor, below it and a negative count).
#[tokio::test]
async fn one_floor_of_one_for_every_count_the_reservations_carry() {
    // The floor `Count` and `RenewalSettings$RenewalCount` share, from their
    // target shape `__integerMin1`.
    const FLOOR: i64 = 1;

    /// How a row builds the request that carries its count.
    type CountBody = fn(i64) -> Value;

    // (the operation, the member, the body that carries `count` as that member)
    let rows: [(Op, &str, CountBody); 3] = [
        (Op::PurchaseOffering, "count", |count| {
            json!({ "count": count })
        }),
        (
            Op::PurchaseOffering,
            "renewalSettings.renewalCount",
            |count| json!({ "count": 1, "renewalSettings": { "renewalCount": count } }),
        ),
        (
            Op::UpdateReservation,
            "renewalSettings.renewalCount",
            |count| json!({ "renewalSettings": { "renewalCount": count } }),
        ),
    ];

    for (op, member, body) in rows {
        served(
            &format!("{op:?} serves {member} = {FLOOR}"),
            op,
            body(FLOOR),
        )
        .await;
        for count in [0, -2] {
            refused(
                &format!("{op:?} refuses {member} = {count}"),
                op,
                body(count),
                &format!("{member} must be at least 1, got {count}"),
            )
            .await;
        }
    }
}
