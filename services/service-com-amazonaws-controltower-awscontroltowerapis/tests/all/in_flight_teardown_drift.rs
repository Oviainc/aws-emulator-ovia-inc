// SPDX-License-Identifier: CC0-1.0

//! The drift an enablement reports WHILE the teardown it accepted is still removing it, end
//! to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The RULE under test is the one `in_flight_configuration_change_drift` asserts for a call
//! that applies a configuration, here for the call that REMOVES one: drift is a property of
//! a DEPLOYED configuration, so an enablement whose deployed configuration a call has
//! started changing reports no drift assessment at all until the clock makes that operation
//! due. A teardown is such a call — what it starts changing is that the configuration stops
//! being deployed — so the assessment the configuration it carried earned stops being
//! reported the moment the teardown starts, and the enablement is gone once the teardown is
//! due. For an enabled baseline "no assessment" is the absent `driftStatusSummary` member
//! (the status set the model declares is `IN_SYNC` | `DRIFTED`, neither of which means
//! "unassessed"); for an enabled control it is `UNKNOWN` in all three dimensions, the value
//! for which the service "is not able to check the drift status".
//!
//! Asserted over a TABLE whose rows are what the rule generalises over — WHICH enablement
//! is torn down, and WHICH configuration it had deployed when the teardown started:
//!
//! * for an enabled baseline, every baseline the catalog publishes (including the ones
//!   whose inheritance drift AWS does not assess, which must report no assessment
//!   mid-teardown either), each at the oldest version it offers with no `parameters` member
//!   and at the newest with a configuration;
//! * for an enabled control, a control the catalog publishes WITH parameters — configured
//!   with every key it declares, with an empty configuration, and with no `parameters`
//!   member — a control that declares none, and a control implemented with a service
//!   control policy, so the rows span both control implementations.
//!
//! The baselines, their versions and their parameter keys are read off
//! `state::BASELINE_CATALOG`, and the controls and their parameter keys off
//! `state::CONTROL_CATALOG` — the catalogs these operations resolve an identifier against —
//! so no row can name a baseline, version, control or parameter key this emulator does not
//! publish. Which baselines report an inheritance drift status comes from the same catalog
//! through `state::assesses_inheritance_drift`. The enum values written out here come from
//! `smithy_query(query="shape", shape="EnablementStatus,EnabledBaselineDriftStatus,\
//! DriftStatus")` on this crate's `smithy-model/`: `SUCCEEDED` | `FAILED` | `UNDER_CHANGE`,
//! `IN_SYNC` | `DRIFTED`, and `DRIFTED` | `IN_SYNC` | `NOT_CHECKING` | `UNKNOWN`; the
//! refusal a torn-down identifier earns comes from `smithy_query(query="operation",
//! operation="GetEnabledBaseline,GetEnabledControl")`, which model
//! `ResourceNotFoundException` (404).

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogBaseline, CatalogControl, BASELINE_CATALOG, CONTROL_CATALOG,
};

use crate::enabled_control_lifecycle::{
    control, details as control_details, drift_dimensions, enable_with, ou,
    status_summary_status as control_status,
};
use crate::in_flight_configuration_change_drift::{
    baseline_details, baseline_status, baseline_summary, deployed_summary, drift_summary,
    enable_baseline_on, ou_target, parameters_of,
};
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator, Answer};

/// One row of the baseline table: the configuration an enablement has deployed when
/// `DisableBaseline` starts removing it.
#[derive(Debug, Clone)]
struct BaselineRow {
    case: String,
    baseline: &'static CatalogBaseline,
    /// The version `EnableBaseline` applies.
    version: &'static str,
    /// The `parameters` member `EnableBaseline` is given; `None` omits it.
    parameters: Option<serde_json::Value>,
}

/// The baseline table: every baseline the catalog publishes, deployed at each end of the
/// version range it offers and with and without a configuration of its own.
fn baseline_rows() -> Vec<BaselineRow> {
    let mut rows = Vec::new();
    for baseline in BASELINE_CATALOG {
        let oldest = baseline
            .versions
            .first()
            .copied()
            .expect("every catalog baseline offers at least one version");
        let newest = baseline
            .versions
            .last()
            .copied()
            .expect("every catalog baseline offers at least one version");
        // A parameters list names only keys the baseline declares, so a baseline that
        // declares none can only be given the empty list.
        let configured = baseline.parameters.first().copied().map_or_else(
            || serde_json::json!([]),
            |key| serde_json::json!([{ "key": key, "value": { "on": true, "at": [1, 2] } }]),
        );

        rows.push(BaselineRow {
            case: format!(
                "{}: a teardown of version {oldest}, enabled with no parameters member",
                baseline.name
            ),
            baseline,
            version: oldest,
            parameters: None,
        });
        rows.push(BaselineRow {
            case: format!(
                "{}: a teardown of version {newest}, enabled with a configuration",
                baseline.name
            ),
            baseline,
            version: newest,
            parameters: Some(configured),
        });
    }
    rows
}

/// POST `/disable-baseline` for the enablement `arn` names.
async fn disable_baseline(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "/disable-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": arn }),
    )
    .await
}

/// POST `/get-enabled-baseline` without asserting the answer, for the cases whose subject
/// is whether the enablement still exists.
async fn get_enabled_baseline(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "/get-enabled-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": arn }),
    )
    .await
}

/// RULE: an enabled baseline whose teardown is in flight reports no `driftStatusSummary`
/// until the clock makes that operation due — whatever version and configuration it had
/// deployed — and once it is due the enablement is gone.
#[tokio::test]
async fn a_baseline_teardown_in_flight_reports_no_drift_assessment() {
    let router = set_up_emulator().await;

    let rows = baseline_rows();
    assert!(
        rows.len() >= 4,
        "the catalog must publish enough baselines for this table to span the rule, got {} \
         rows",
        rows.len()
    );

    // Every row is its own enablement on its own target, so the rows do not interact.
    let mut enabled: Vec<(BaselineRow, String)> = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        let answer = enable_baseline_on(
            &router,
            row.baseline,
            row.version,
            row.parameters.clone(),
            &ou_target("tdwn", index),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: enable-baseline expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );
        let arn = answer.string("arn");
        enabled.push((row, arn));
    }

    // The enables are asynchronous too, so wait for them: the assessment a teardown
    // withdraws has to exist first. Nothing sleeps server-side; the wait is the test's.
    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn) in &enabled {
        let expected = deployed_summary(row.baseline);
        let detail = baseline_details(&router, &row.case, arn).await;
        assert_eq!(
            (baseline_status(&detail), drift_summary(&detail)),
            (Some("SUCCEEDED"), expected.clone()),
            "case {:?}: expected a deployed enablement reporting driftStatusSummary \
             {expected:?} before the teardown, got {detail:?}",
            row.case
        );
    }

    // Each teardown is started and read back straight away, so the read falls inside the
    // window the operation is in flight.
    for (row, arn) in &enabled {
        let answer = disable_baseline(&router, arn).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: disable-baseline expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );

        let detail = baseline_details(&router, &row.case, arn).await;
        assert_eq!(
            (baseline_status(&detail), drift_summary(&detail)),
            (Some("UNDER_CHANGE"), None),
            "case {:?}: expected UNDER_CHANGE and no driftStatusSummary while the teardown \
             is still running, got {detail:?}",
            row.case
        );

        // The listing reads the same record, so it withdraws the assessment too.
        let summary = baseline_summary(&router, &row.case, arn).await;
        assert_eq!(
            (baseline_status(&summary), drift_summary(&summary)),
            (Some("UNDER_CHANGE"), None),
            "case {:?}: expected the listing to report UNDER_CHANGE and no \
             driftStatusSummary while the teardown is still running, got {summary:?}",
            row.case
        );

        // Reporting does not disturb what a later caller sees: the second read is the same.
        let again = baseline_details(&router, &row.case, arn).await;
        assert_eq!(
            (baseline_status(&again), drift_summary(&again)),
            (Some("UNDER_CHANGE"), None),
            "case {:?}: expected the in-flight reading to be stable across reads, got \
             {again:?}",
            row.case
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn) in &enabled {
        let gone = get_enabled_baseline(&router, arn).await;
        assert_eq!(
            (gone.status, gone.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {:?}: expected the enablement gone once its teardown is due, got {:?} body \
             {:?}",
            row.case,
            (gone.status, gone.error_code.as_deref()),
            gone.body
        );
    }
}

/// One row of the control table: the configuration an enabled control has deployed when
/// `DisableControl` starts removing it.
#[derive(Debug, Clone)]
pub(crate) struct ControlRow {
    pub(crate) case: String,
    pub(crate) control_identifier: String,
    pub(crate) target_identifier: String,
    /// The `parameters` member `EnableControl` is given; `None` omits it.
    pub(crate) initial: Option<serde_json::Value>,
}

/// The control table: a control the catalog publishes WITH parameters under each kind of
/// configuration it can carry, a control that declares none, and a control implemented with
/// a service control policy, so the rows span both control implementations.
pub(crate) fn control_rows() -> Vec<ControlRow> {
    let parameterized = CONTROL_CATALOG
        .iter()
        .find(|control| control.enablable && !control.parameters.is_empty())
        .expect("the catalog must publish an enablable control that declares parameters");
    let plain = CONTROL_CATALOG
        .iter()
        .find(|control| control.enablable && control.parameters.is_empty())
        .expect("the catalog must publish an enablable control that declares no parameters");
    let policy_backed: &CatalogControl = CONTROL_CATALOG
        .iter()
        .find(|control| control.enablable && control.implemented_with_service_control_policy())
        .expect("the catalog must publish an enablable control implemented with an SCP");
    let values = [
        serde_json::json!(["us-west-2", "eu-west-1"]),
        serde_json::json!("arn:aws:iam::123456789012:role/exempted"),
        serde_json::json!({ "nested": { "deep": [1, 2, 3] }, "on": true }),
    ];

    vec![
        ControlRow {
            case: format!(
                "{}: a teardown of a control configured with every parameter it declares",
                parameterized.name
            ),
            control_identifier: control(parameterized.name),
            target_identifier: ou("tdwna"),
            initial: Some(parameters_of(
                parameterized,
                parameterized.parameters.len(),
                &values,
            )),
        },
        ControlRow {
            case: format!(
                "{}: a teardown of a control enabled with an empty configuration",
                parameterized.name
            ),
            control_identifier: control(parameterized.name),
            target_identifier: ou("tdwnb"),
            initial: Some(serde_json::json!([])),
        },
        ControlRow {
            case: format!(
                "{}: a teardown of a control enabled with no parameters member",
                parameterized.name
            ),
            control_identifier: control(parameterized.name),
            target_identifier: ou("tdwnc"),
            initial: None,
        },
        ControlRow {
            case: format!(
                "{}: a teardown of a control that declares no parameters",
                plain.name
            ),
            control_identifier: control(plain.name),
            target_identifier: ou("tdwnd"),
            initial: None,
        },
        ControlRow {
            case: format!(
                "{}: a teardown of a control implemented with a service control policy",
                policy_backed.name
            ),
            control_identifier: control(policy_backed.name),
            target_identifier: ou("tdwne"),
            initial: None,
        },
    ]
}

/// POST `/disable-control` for the enablement `arn` names.
pub(crate) async fn disable_control(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "/disable-control",
        serde_json::json!({ "enabledControlIdentifier": arn }),
    )
    .await
}

/// POST `/get-enabled-control` without asserting the answer, for the cases whose subject is
/// whether the enablement still exists.
pub(crate) async fn get_enabled_control(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "/get-enabled-control",
        serde_json::json!({ "enabledControlIdentifier": arn }),
    )
    .await
}

/// RULE (the same one, on the sibling resource): an enabled control whose teardown is in
/// flight reports `UNKNOWN` in all three drift dimensions until the clock makes that
/// operation due — whatever configuration it had deployed — and once it is due the
/// enablement is gone.
#[tokio::test]
async fn a_control_teardown_in_flight_reports_unknown_drift() {
    let router = set_up_emulator().await;

    // Every row is its own enablement on its own target, so the rows do not interact.
    let mut enabled: Vec<(ControlRow, String)> = Vec::new();
    for row in control_rows() {
        let (arn, _) = enable_with(
            &router,
            &row.control_identifier,
            &row.target_identifier,
            row.initial.clone(),
        )
        .await;
        enabled.push((row, arn));
    }

    // The enables are asynchronous too, so wait for them: a control still being deployed
    // refuses a teardown as a conflict, and the assessment a teardown withdraws has to
    // exist first.
    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn) in &enabled {
        let detail = control_details(&router, arn).await;
        assert_eq!(
            (control_status(&detail), drift_dimensions(&detail)),
            (
                Some("SUCCEEDED"),
                (Some("IN_SYNC"), Some("IN_SYNC"), Some("IN_SYNC"))
            ),
            "case {:?}: expected a deployed control assessed IN_SYNC before the teardown, \
             got {detail:?}",
            row.case
        );
    }

    // Each teardown is started and read back straight away, so the read falls inside the
    // window the operation is in flight.
    for (row, arn) in &enabled {
        let answer = disable_control(&router, arn).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: disable-control expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );

        let detail = control_details(&router, arn).await;
        assert_eq!(
            (control_status(&detail), drift_dimensions(&detail)),
            (
                Some("UNDER_CHANGE"),
                (Some("UNKNOWN"), Some("UNKNOWN"), Some("UNKNOWN"))
            ),
            "case {:?}: expected UNDER_CHANGE with UNKNOWN drift while the teardown is \
             still running, got {detail:?}",
            row.case
        );

        // Reporting does not disturb what a later caller sees: the second read is the same.
        let again = control_details(&router, arn).await;
        assert_eq!(
            (control_status(&again), drift_dimensions(&again)),
            (
                Some("UNDER_CHANGE"),
                (Some("UNKNOWN"), Some("UNKNOWN"), Some("UNKNOWN"))
            ),
            "case {:?}: expected the in-flight reading to be stable across reads, got \
             {again:?}",
            row.case
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn) in &enabled {
        let gone = get_enabled_control(&router, arn).await;
        assert_eq!(
            (gone.status, gone.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {:?}: expected the enablement gone once its teardown is due, got {:?} body \
             {:?}",
            row.case,
            (gone.status, gone.error_code.as_deref()),
            gone.body
        );
    }
}
