// SPDX-License-Identifier: CC0-1.0

//! The TERMINAL OUTCOME AppConfig's two stop operations report, and the class each one
//! refuses with — `StopDeployment` and `StopExperimentRun` end to end over HTTP.
//!
//! Two rules are under test, each stated one level above the single request that exposed
//! it:
//!
//! 1. a stop settles what it stopped: the record the stop answers is already in the
//!    TERMINAL state the stop left it in — a deployment the service was still working on is
//!    `ROLLED_BACK`, carrying the instant the rollback finished and, newest on its event
//!    log, the rollback's `ROLLBACK_COMPLETED` entry attributed to the request that asked
//!    for it — and every later reader of that record reports the same outcome, so nothing
//!    the caller must wait for is left in flight;
//! 2. an operation whose domain is a run still in flight refuses one that has reached the
//!    status its lifecycle ends in with HTTP 400 `BadRequestException` — the class for input
//!    no later request can make acceptable, not the 409 `ConflictException` a state that can
//!    still clear answers with — stating that operation's own request-derived template, and
//!    writes nothing.
//!
//! The tables span what those rules generalise over rather than one recorded request: the
//! pacing of the deployment being stopped (both published bounds of
//! `MinutesBetween0And24Hours`, range 0..1440, both published bounds of `Percentage`, range
//! 1..100, and both `GrowthType` values, from
//! `smithy_query(query="operation", operation="StopDeployment")`), whether the stop allows
//! a revert (absent, `false`, `true`), WHICH reader reads the settled deployment back
//! (`GetDeployment` and `ListDeployments`), WHICH operation of the experiment-run domain
//! refuses the finished run (`StopExperimentRun` and `UpdateExperimentRun`), WHICH run
//! number it addresses over a definition that keeps accumulating finished runs, and whether
//! the resources are addressed by minted id or by name (both are `Identifier` members,
//! documented as the resource's "ID or name").

use serde_json::{json, Value};

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, router, Answer,
};
use crate::deployment_wire_fidelity::{create_strategy, event_log, event_types, host_version, member, Row};
use crate::experiment_definitions::scope;
use crate::start_experiment_run::{
    assert_refused, create_test_definition, get_run, id_of, start_run,
};
use crate::stop_deployment::{get_deployment, start, stop};
use crate::stop_deployment_without_its_environment::list_deployments;
use crate::stop_experiment_run::stop_run;
use crate::update_experiment_run::update_run;

/// The terminal `DeploymentState` of a deployment a caller stopped while the service was
/// still working on it, from the model's `DeploymentState` enum (`BAKING | COMPLETE |
/// DEPLOYING | REVERTED | ROLLED_BACK | ROLLING_BACK | VALIDATING`,
/// `smithy_query(query="operation", operation="StopDeployment")`).
const ROLLED_BACK: &str = "ROLLED_BACK";

/// The `DeploymentEventType` of the entry a settled rollback records, and the
/// `Description` and `TriggeredBy` AppConfig reports it with when a request initiated the
/// rollback.
const ROLLBACK_COMPLETED: &str = "ROLLBACK_COMPLETED";
/// See [`ROLLBACK_COMPLETED`].
const ROLLED_BACK_BY_USER: &str = "Deployment rolled back by user request";
/// See [`ROLLBACK_COMPLETED`].
const TRIGGERED_BY_USER: &str = "USER";

/// The `State` `ListDeployments` reports for one deployment number.
fn listed_state(case: &str, listing: &Answer, number: i64) -> String {
    let body = listing.json();
    let items = body
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: ListDeployments must carry Items, got {body}"));
    items
        .iter()
        .find(|item| member(item, "DeploymentNumber").as_i64() == Some(number))
        .map(|item| member(item, "State").as_str().unwrap_or_default().to_string())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing must report deployment {number}, got {body}")
        })
}

/// Rule 1: a stop of a deployment in progress answers the deployment in its terminal
/// rolled-back state, and both implemented readers report that same settled record —
/// whatever paced the deployment and whatever the request said about a revert.
#[tokio::test]
async fn a_stop_answers_the_deployment_in_its_terminal_rolled_back_state() {
    let router = router();
    let application = create_application(&router, "settled-rollback").await;
    let profile = create_profile(&router, &application, "config").await;
    host_version(&router, &application, &profile).await;

    // Rows differ along the pacing of the deployment being stopped — the published bounds
    // of `MinutesBetween0And24Hours` (range 0..1440) and of `Percentage` (range 1..100),
    // and both `GrowthType` values — and along what the request says about a revert, which
    // changes nothing for a deployment that has not completed: "the status of an
    // in-progress deployment will be ROLLED_BACK".
    let rows: [(Row, Option<bool>); 6] = [
        (("the longest rollout the model allows", 1440, 100.0, "LINEAR", 0), None),
        (("no rollout window, so it is baking", 0, 100.0, "LINEAR", 1440), None),
        (
            ("an exponential rollout at the smallest growth factor", 60, 1.0, "EXPONENTIAL", 10),
            Some(false),
        ),
        (("a rollout in progress, with a revert allowed", 1440, 50.0, "LINEAR", 1440), Some(true)),
        (("the shortest bake the model allows", 0, 100.0, "LINEAR", 1), Some(true)),
        (("a long exponential rollout", 1440, 100.0, "EXPONENTIAL", 1440), Some(false)),
    ];

    for (index, (row, allow_revert)) in rows.iter().enumerate() {
        let case = row.0;
        let strategy = create_strategy(&router, index, row).await;
        // One environment per row, so no stop reaches another row's deployment.
        let environment =
            create_environment(&router, &application, &format!("env-{strategy}")).await;

        let started = start(&router, &application, &environment, &strategy, &profile).await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment must answer 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let at_start = member(&started.json(), "State");
        let at_start = at_start.as_str().unwrap_or_default();
        assert!(
            matches!(at_start, "DEPLOYING" | "BAKING"),
            "case {case:?}: this row's strategy must leave the deployment in progress, got \
             {at_start:?}"
        );

        let stopped = stop(&router, &application, &environment, 1, *allow_revert).await;
        assert_eq!(
            stopped.status,
            202,
            "case {case:?}: StopDeployment must answer the modelled 202, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
        let stop_body = stopped.json();
        assert_eq!(
            member(&stop_body, "State").as_str(),
            Some(ROLLED_BACK),
            "case {case:?}: the stop must answer the terminal state it left the deployment \
             in, got {}",
            member(&stop_body, "State")
        );
        assert!(
            member(&stop_body, "CompletedAt").is_null(),
            "case {case:?}: a settled rollback never completed the deployment, so it reports \
             no completion instant, got {}",
            member(&stop_body, "CompletedAt")
        );

        // The newest entry of the log the stop answers is the rollback's completion, which
        // reports what initiated it.
        let events = event_log(case, "StopDeployment", &stop_body);
        let newest = events
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the stop records an event"));
        assert_eq!(
            member(&newest, "EventType").as_str(),
            Some(ROLLBACK_COMPLETED),
            "case {case:?}: the newest event is the rollback finishing, got {:?}",
            event_types(&events)
        );
        assert_eq!(
            member(&newest, "Description").as_str(),
            Some(ROLLED_BACK_BY_USER),
            "case {case:?}: expected the rollback's own description, got {newest}"
        );
        assert_eq!(
            member(&newest, "TriggeredBy").as_str(),
            Some(TRIGGERED_BY_USER),
            "case {case:?}: the request initiated the rollback, got {newest}"
        );

        // Both readers answer the settled record, so nothing a caller reads afterwards
        // reports work still in flight.
        let fetched = get_deployment(&router, &application, &environment, 1).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetDeployment must answer 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            fetched.json(),
            stop_body,
            "case {case:?}: GetDeployment must report exactly the record the stop answered"
        );
        let listing = list_deployments(&router, &application, &environment).await;
        assert_eq!(
            listed_state(case, &listing, 1),
            ROLLED_BACK,
            "case {case:?}: the listing must report the state the stop left behind"
        );
    }
}

/// Rule 2: every operation of the experiment-run domain refuses a run that is not running
/// with HTTP 400 `BadRequestException` and its own request-derived template, and writes
/// nothing.
#[tokio::test]
async fn an_operation_on_a_running_run_refuses_a_finished_one_as_a_bad_request() {
    const LABEL: &str = "finished";
    let router = router();
    let scope = scope(&router, LABEL).await;
    let application_name = format!("app-{LABEL}");
    let name = format!("{LABEL}-definition");
    let definition = create_test_definition(&router, &scope, &name).await;
    let definition_id = id_of(&name, &definition);

    // Three runs, each started and then stopped, so the table addresses a definition that
    // keeps accumulating finished runs — the first run of a definition and the ones after
    // it, which a handler acting on "the running run" rather than the addressed one would
    // confuse.
    for run in [1_i64, 2, 3] {
        let started = start_run(
            &router,
            &scope.application,
            &definition_id,
            &json!({ "ExposurePercentage": 10.0 }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "run {run} must start, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let ended = stop_run(&router, &scope.application, &definition_id, run, &json!({})).await;
        assert_eq!(
            ended.status,
            200,
            "run {run} must stop once, got {} / {}",
            ended.status,
            String::from_utf8_lossy(&ended.body)
        );

        // Every finished run, addressed in both identifier forms, by both operations whose
        // domain is a run in flight. Each states its OWN template, rendered from the
        // request: the run number the path carried and the definition identifier it
        // addressed, with no value of this run's own in it.
        for by_name in [false, true] {
            let (application, addressed) = if by_name {
                (application_name.clone(), name.clone())
            } else {
                (scope.application.clone(), definition_id.clone())
            };
            let before = get_run(&router, &scope.application, &definition_id, run).await;
            assert_eq!(
                before.status,
                200,
                "run {run} must read back before the refusals, got {} / {}",
                before.status,
                String::from_utf8_lossy(&before.body)
            );
            let document = before.json();

            let stop_case = format!("StopExperimentRun on finished run {run}, by_name {by_name}");
            let refused_stop =
                stop_run(&router, &application, &addressed, run, &json!({})).await;
            assert_refused(&stop_case, &refused_stop, 400, "BadRequestException");
            let expected_stop = format!(
                "Experiment run {run} for experiment definition {addressed} is not running."
            );
            assert_eq!(
                refused_stop.string("Message").as_deref(),
                Some(expected_stop.as_str()),
                "case {stop_case:?}: expected the message {expected_stop:?}, got {:?}",
                refused_stop.string("Message")
            );

            let update_case =
                format!("UpdateExperimentRun on finished run {run}, by_name {by_name}");
            let refused_update = update_run(
                &router,
                &application,
                &addressed,
                run,
                &json!({ "ExposurePercentage": 100.0 }),
            )
            .await;
            assert_refused(&update_case, &refused_update, 400, "BadRequestException");
            let expected_update = format!(
                "Cannot update experiment run {run} for experiment definition {addressed} \
                 because it is already terminal."
            );
            assert_eq!(
                refused_update.string("Message").as_deref(),
                Some(expected_update.as_str()),
                "case {update_case:?}: expected the message {expected_update:?}, got {:?}",
                refused_update.string("Message")
            );

            // Neither refusal wrote anything.
            let after = get_run(&router, &scope.application, &definition_id, run).await;
            assert_eq!(
                after.json(),
                document,
                "case {stop_case:?}: a refused call must write nothing, expected {document}, \
                 got {}",
                after.json()
            );
        }
    }

    // And a run number nothing ever issued is still a miss, not a bad request: the
    // not-running rule is about a run the definition HOLDS.
    let missing = stop_run(&router, &scope.application, &definition_id, 4, &json!({})).await;
    assert_refused(
        "a run number the definition never issued",
        &missing,
        404,
        "ResourceNotFoundException",
    );
}
