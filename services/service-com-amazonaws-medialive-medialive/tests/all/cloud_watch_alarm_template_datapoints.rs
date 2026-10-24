// SPDX-License-Identifier: CC0-1.0

//! The `DatapointsToAlarm` setting of a cloudwatch alarm template, end to end
//! over the restJson1 wire.
//!
//! Every test drives the service the way a client does — real HTTP requests
//! dispatched through `MultiServiceRouter` — so what is asserted is the
//! emulator's wire behaviour, not an internal function's return value.
//!
//! The rule these suites pin: `DatapointsToAlarm` is the M of the template's "M
//! out of N" alarm ("The number of datapoints within the evaluation period that
//! must be breaching to trigger the alarm") and `EvaluationPeriods` is its N
//! ("The number of periods over which data is compared to the specified
//! threshold"), so a template stores the M its create supplied and, when the
//! create supplied none, the N it did — the alarm that trips when every evaluated
//! period breaches. Either way the setting is readable for the whole lifetime of
//! the resource: the create response, every later `GetCloudWatchAlarmTemplate`
//! and every `UpdateCloudWatchAlarmTemplate` response report it.
//!
//! The recorded case in the issue (one create, then two reads) is ONE row of the
//! table below, which spans both the supplied and the omitted M, each over
//! several N — including the published minimum of the `__integerMin1` both
//! members target.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{percent_encoded, GROUPS};
use crate::wire::{call_in_region, medialive, DEFAULT_REGION as REGION};

/// The collection `CreateCloudWatchAlarmTemplate` is bound to, which the reader
/// and the update address an instance under (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplate")`: POST
/// /prod/cloudwatch-alarm-templates).
const TEMPLATES: &str = "/prod/cloudwatch-alarm-templates";

/// The minimum `EvaluationPeriods` and `DatapointsToAlarm` share, from their
/// common target shape `__integerMin1` (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplate")`).
const MIN_DATAPOINTS: i64 = 1;

/// Every required member of `CreateCloudWatchAlarmTemplate`, one declared value
/// each, with `GroupIdentifier` and `EvaluationPeriods` taken from the row, so a
/// row that varies only the datapoint members still sends a complete request
/// (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplate")`).
fn required_members(name: &str, group: &str, evaluation_periods: i64) -> Value {
    json!({
        "name": name,
        "groupIdentifier": group,
        "comparisonOperator": "GreaterThanOrEqualToThreshold",
        "evaluationPeriods": evaluation_periods,
        "metricName": "ActiveAlerts",
        "period": 300,
        "statistic": "Maximum",
        "targetResourceType": "MEDIALIVE_CHANNEL",
        "threshold": 1.0,
        "treatMissingData": "missing",
    })
}

async fn create_group(router: &MultiServiceRouter, name: &str) {
    let (status, group) = call_in_region(
        router,
        REGION,
        "POST",
        GROUPS,
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the group {name:?} to be created, got {status}: {group}"
    );
}

async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call_in_region(router, REGION, "POST", TEMPLATES, Some(body)).await
}

async fn get(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call_in_region(
        router,
        REGION,
        "GET",
        &format!("{TEMPLATES}/{}", percent_encoded(identifier)),
        None,
    )
    .await
}

async fn patch(router: &MultiServiceRouter, identifier: &str, body: Value) -> (u16, Value) {
    call_in_region(
        router,
        REGION,
        "PATCH",
        &format!("{TEMPLATES}/{}", percent_encoded(identifier)),
        Some(body),
    )
    .await
}

/// Rule: the M of the template's "M out of N" alarm is the `DatapointsToAlarm`
/// the create supplied, or the `EvaluationPeriods` it supplied when it named no
/// M, and every operation that reports the template reports that stored M.
///
/// The rows differ along what the rule generalises over: whether the create names
/// an M at all, the M it names, and the N it is evaluated over — including the
/// published minimum of `__integerMin1` and an N well above it.
#[tokio::test]
async fn an_omitted_datapoints_to_alarm_is_stored_as_the_evaluation_period_count() {
    // (case, the evaluationPeriods the create supplies, the datapointsToAlarm it
    // supplies, the M every report of the template must carry)
    let rows: [(&str, i64, Option<i64>, i64); 8] = [
        ("no M, N at its published minimum", MIN_DATAPOINTS, None, MIN_DATAPOINTS),
        ("no M, N of 2", 2, None, 2),
        ("no M, N of 5", 5, None, 5),
        ("no M, N of 24", 24, None, 24),
        (
            "M at its published minimum, N of 1",
            MIN_DATAPOINTS,
            Some(MIN_DATAPOINTS),
            MIN_DATAPOINTS,
        ),
        ("M of 3, N of 5", 5, Some(3), 3),
        ("M equal to N", 4, Some(4), 4),
        ("M above N", 2, Some(7), 7),
    ];

    for (index, (case, evaluation_periods, supplied, expected)) in rows.into_iter().enumerate() {
        let router = medialive();
        create_group(&router, "datapoints-group").await;

        let name = format!("alarm-{index}");
        let mut request = required_members(&name, "datapoints-group", evaluation_periods);
        if let Some(datapoints) = supplied {
            request["datapointsToAlarm"] = json!(datapoints);
        }

        let (status, created) = create(&router, request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateCloudWatchAlarmTemplate, got {status}: {created}"
        );
        assert_eq!(
            created["datapointsToAlarm"],
            json!(expected),
            "case {case:?}: expected the create to report datapointsToAlarm {expected}, got {:?}",
            created["datapointsToAlarm"]
        );
        assert_eq!(
            created["evaluationPeriods"],
            json!(evaluation_periods),
            "case {case:?}: expected the create to report evaluationPeriods {evaluation_periods}, got {:?}",
            created["evaluationPeriods"]
        );

        // ROUND TRIP: the stored setting is what the reader answers, by either
        // identifier form ("Can be either be its id or current name"), and a
        // second read answers the same thing — the read changed nothing.
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create reported no id: {created}"))
            .to_string();
        for identifier in [id.as_str(), name.as_str(), id.as_str()] {
            let (status, read) = get(&router, identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected reading {identifier:?} to answer 200, got {status}: {read}"
            );
            assert_eq!(
                read["datapointsToAlarm"],
                json!(expected),
                "case {case:?}: expected reading {identifier:?} to report datapointsToAlarm {expected}, got {:?}",
                read["datapointsToAlarm"]
            );
            assert_eq!(
                read, created,
                "case {case:?}: expected reading {identifier:?} to report the created template {created}, got {read}"
            );
        }

        // An update that names no M leaves the stored one as it is — a partial
        // write changes only the members its request carries — so the setting
        // outlives the create that made it, whether the create named it or the
        // store implied it from N.
        let (status, patched) = patch(&router, &id, json!({ "description": "unrelated" })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateCloudWatchAlarmTemplate, got {status}: {patched}"
        );
        assert_eq!(
            patched["datapointsToAlarm"],
            json!(expected),
            "case {case:?}: expected the update to keep datapointsToAlarm {expected}, got {:?}",
            patched["datapointsToAlarm"]
        );

        // The same when the update moves N itself: M is the template's own
        // stored setting from the create onwards, not a value recomputed from
        // whatever N happens to be, so an update of `evaluationPeriods` alone
        // changes N and leaves M where it was.
        let widened = evaluation_periods + 3;
        let (status, patched) =
            patch(&router, &id, json!({ "evaluationPeriods": widened })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the evaluationPeriods update, got {status}: {patched}"
        );
        assert_eq!(
            patched["evaluationPeriods"],
            json!(widened),
            "case {case:?}: expected the update to report evaluationPeriods {widened}, got {:?}",
            patched["evaluationPeriods"]
        );
        assert_eq!(
            patched["datapointsToAlarm"],
            json!(expected),
            "case {case:?}: expected the stored datapointsToAlarm {expected} to survive a change of evaluationPeriods, got {:?}",
            patched["datapointsToAlarm"]
        );

        // An update that DOES name one replaces it, and the read agrees.
        let replaced = expected + 1;
        let (status, patched) =
            patch(&router, &id, json!({ "datapointsToAlarm": replaced })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the second update, got {status}: {patched}"
        );
        assert_eq!(
            patched["datapointsToAlarm"],
            json!(replaced),
            "case {case:?}: expected the update to report datapointsToAlarm {replaced}, got {:?}",
            patched["datapointsToAlarm"]
        );
        let (status, read) = get(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated template to read back, got {status}: {read}"
        );
        assert_eq!(
            read["datapointsToAlarm"],
            json!(replaced),
            "case {case:?}: expected the read to report datapointsToAlarm {replaced}, got {:?}",
            read["datapointsToAlarm"]
        );
    }
}
