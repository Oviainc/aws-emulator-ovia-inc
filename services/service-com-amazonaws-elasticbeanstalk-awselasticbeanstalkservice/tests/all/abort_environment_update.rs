// SPDX-License-Identifier: CC0-1.0

//! `AbortEnvironmentUpdate`, driven end to end over this service's awsQuery
//! surface.
//!
//! The operation "cancels in-progress environment configuration update or
//! application version deployment", so the rule it follows is: it names the
//! environment by whichever of the either-or pair `EnvironmentId` /
//! `EnvironmentName` the request carries, and cancels only an environment whose
//! stored record reports an abortable operation in progress — the environment
//! then moves to `Aborting` and the clock owes its return to `Ready`. Every
//! other request is refused with this service's own client-error codes, never
//! with a silent success.
//!
//! The harness ([`Emulator`], [`PAST_DUE`]) comes from the `lifecycle` suite in
//! this same test target rather than being copied here; the emulator is driven
//! over HTTP exactly as a client drives it.
//!
//! One precondition is still seeded rather than produced by a call: an update that is
//! in flight with NOTHING owed by the clock. `UpdateEnvironment` is implemented now,
//! but the update it starts carries its own completion to `Ready` one transition span
//! later, so an abort driven after it would race that completion. [`begin_update`]
//! writes the same starting state — `Updating`, abortable, health checks suspended —
//! with no pending completion, in the very store the handler reads; the behaviour under
//! test is still the HTTP call and the HTTP read-back.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::lifecycle::{Emulator, PAST_DUE, SOLUTION_STACK};

/// The `EnvironmentStatus` the real service reports while a configuration
/// update or an application-version deployment is in flight: "Updating:
/// Environment is in the process of updating its configuration settings or
/// application version" (the modelled value set is
/// `eb::ops::types::ENVIRONMENT_STATUS_VALUES`). Read from the state module's
/// own constant — the value `UpdateEnvironment` writes — so the seeded
/// precondition cannot drift from the status an update really leaves.
const STATUS_UPDATING: &str = eb::state::STATUS_UPDATING;

/// Put the environment named `environment_name` into the state an in-flight
/// update leaves it in — `Updating`, with `AbortableOperationInProgress` true
/// and health checks suspended — and answer its `EnvironmentId`.
///
/// See this file's module docs for why the precondition is seeded rather than
/// produced by a call.
fn begin_update(emulator: &Emulator, environment_name: &str) -> String {
    // The seeded status is a value the MODEL declares, so this test cannot drift
    // onto a status the service could never report.
    assert!(
        eb::ops::types::ENVIRONMENT_STATUS_VALUES.contains(&STATUS_UPDATING),
        "case {environment_name:?}: {STATUS_UPDATING:?} must be a modelled EnvironmentStatus, the modelled set is {:?}",
        eb::ops::types::ENVIRONMENT_STATUS_VALUES
    );
    let state = emulator.store();
    let mut store = state.lock();
    let environment_id = store
        .environment_name_index
        .get(environment_name)
        .cloned()
        .unwrap_or_else(|| panic!("case {environment_name:?}: the environment must exist first"));
    let environment = store
        .environments
        .get_mut(&environment_id)
        .unwrap_or_else(|| panic!("case {environment_name:?}: the environment must exist first"));
    environment.status = STATUS_UPDATING.to_string();
    environment.health = eb::state::HEALTH_GREY.to_string();
    environment.health_status = Some(eb::state::HEALTH_STATUS_PENDING.to_string());
    environment.abortable_operation_in_progress = true;
    // Nothing is owed by the clock: the update is in flight until it is either
    // aborted or completes, and this emulator runs no update pipeline.
    environment.pending_status = None;
    environment_id
}

/// Create `application`, and `environment` under it, and let the launch the
/// clock owed complete so the environment is `Ready`.
async fn ready_environment(emulator: &Emulator, application: &str, environment: &str) {
    let created_application = emulator
        .call("CreateApplication", &[("ApplicationName", application)])
        .await;
    assert_eq!(
        created_application.status, 200,
        "case {environment:?}: expected 200 from CreateApplication, got {} and body {}",
        created_application.status, created_application.body
    );
    let created = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", application),
                ("EnvironmentName", environment),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {environment:?}: expected 200 from CreateEnvironment, got {} and body {}",
        created.status, created.body
    );
    std::thread::sleep(PAST_DUE);
}

/// The status, abortable flag, health and health status DescribeEnvironments
/// reports for one environment.
async fn described(emulator: &Emulator, environment: &str) -> Vec<String> {
    let answer = emulator
        .call(
            "DescribeEnvironments",
            &[("EnvironmentNames.member.1", environment)],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {environment:?}: expected 200 from DescribeEnvironments, got {} and body {}",
        answer.status, answer.body
    );
    [
        "Status",
        "AbortableOperationInProgress",
        "Health",
        "HealthStatus",
    ]
    .iter()
    .map(|member| {
        answer
            .value(&format!("Environments/member/{member}"))
            .unwrap_or_default()
            .to_string()
    })
    .collect()
}

/// One row of the success table: which members of the either-or identifier pair
/// the abort names the environment by. The rule generalises over the
/// identifier, so every row must cancel the same in-progress update.
struct AbortBy {
    case: &'static str,
    application: &'static str,
    environment: &'static str,
    send_name: bool,
    send_id: bool,
}

#[tokio::test]
async fn an_in_progress_update_is_cancelled_whichever_identifier_names_the_environment() {
    let rows = [
        AbortBy {
            case: "by EnvironmentName",
            application: "abort-by-name",
            environment: "env-by-name",
            send_name: true,
            send_id: false,
        },
        AbortBy {
            case: "by EnvironmentId",
            application: "abort-by-id",
            environment: "env-by-id",
            send_name: false,
            send_id: true,
        },
        AbortBy {
            // "You must specify either this or an EnvironmentName, or both."
            case: "by both",
            application: "abort-by-both",
            environment: "env-by-both",
            send_name: true,
            send_id: true,
        },
        AbortBy {
            // The modelled minimum length for `EnvironmentName` is 4.
            case: "by a name at the modelled minimum length of 4",
            application: "abort-short-name",
            environment: "envz",
            send_name: true,
            send_id: false,
        },
    ];

    for AbortBy {
        case,
        application,
        environment,
        send_name,
        send_id,
    } in &rows
    {
        let emulator = Emulator::new();
        ready_environment(&emulator, application, environment).await;
        let environment_id = begin_update(&emulator, environment);
        assert_eq!(
            described(&emulator, environment).await,
            vec![
                STATUS_UPDATING.to_string(),
                "true".to_string(),
                "Grey".to_string(),
                "Pending".to_string()
            ],
            "case {case:?}: the seeded update must be reported as abortable"
        );

        let mut params: Vec<(&str, &str)> = Vec::new();
        if *send_name {
            params.push(("EnvironmentName", environment));
        }
        if *send_id {
            params.push(("EnvironmentId", environment_id.as_str()));
        }
        let aborted = emulator.call("AbortEnvironmentUpdate", &params).await;
        assert_eq!(
            aborted.status, 200,
            "case {case:?}: expected 200 from AbortEnvironmentUpdate, got {} and body {}",
            aborted.status, aborted.body
        );
        assert_eq!(
            aborted.error_code(),
            "",
            "case {case:?}: expected a success envelope, got {}",
            aborted.body
        );

        // The cancellation is observable through an implemented reader: the
        // environment is aborting the deployment and nothing is abortable any
        // more.
        assert_eq!(
            described(&emulator, environment).await,
            vec![
                "Aborting".to_string(),
                "false".to_string(),
                "Grey".to_string(),
                "Pending".to_string()
            ],
            "case {case:?}: unexpected state after the abort"
        );
        // A read does not move the environment on by itself.
        assert_eq!(
            described(&emulator, environment).await,
            vec![
                "Aborting".to_string(),
                "false".to_string(),
                "Grey".to_string(),
                "Pending".to_string()
            ],
            "case {case:?}: a read must not change what a later read sees"
        );

        // The return to `Ready` is what the clock owes once the span is past.
        std::thread::sleep(PAST_DUE);
        assert_eq!(
            described(&emulator, environment).await,
            vec![
                "Ready".to_string(),
                "false".to_string(),
                "Green".to_string(),
                "Ok".to_string()
            ],
            "case {case:?}: the abort the clock owed must have completed"
        );
    }
}

/// One row per request the operation must refuse, with the code and status it
/// refuses with.
struct RefusalRow {
    case: &'static str,
    params: Vec<(&'static str, String)>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn an_abort_with_nothing_to_cancel_is_refused_with_the_services_own_error_codes() {
    let emulator = Emulator::new();
    // `ready-env` has no operation in progress; `aborting-env` has one that is
    // already being cancelled; `gone-env` is on its way out.
    ready_environment(&emulator, "refusals", "ready-env").await;
    let ready_id = {
        let state = emulator.store();
        let store = state.lock();
        store
            .environment_name_index
            .get("ready-env")
            .cloned()
            .expect("the environment was just created")
    };

    for environment in ["aborting-env", "gone-env"] {
        let created = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", "refusals"),
                    ("EnvironmentName", environment),
                    ("SolutionStackName", SOLUTION_STACK.as_str()),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {environment:?}: expected 200 from CreateEnvironment, got {}",
            created.body
        );
    }
    std::thread::sleep(PAST_DUE);
    begin_update(&emulator, "aborting-env");
    let first_abort = emulator
        .call(
            "AbortEnvironmentUpdate",
            &[("EnvironmentName", "aborting-env")],
        )
        .await;
    assert_eq!(
        first_abort.status, 200,
        "case \"abort twice\": the first abort must succeed, got {} and body {}",
        first_abort.status, first_abort.body
    );
    let terminated = emulator
        .call("TerminateEnvironment", &[("EnvironmentName", "gone-env")])
        .await;
    assert_eq!(
        terminated.status, 200,
        "case \"terminating\": expected 200 from TerminateEnvironment, got {}",
        terminated.body
    );

    // A launching environment: created here, and not waited out, so its launch
    // is still in progress when the abort arrives.
    let launching = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "refusals"),
                ("EnvironmentName", "launch-env"),
                ("SolutionStackName", SOLUTION_STACK.as_str()),
            ],
        )
        .await;
    assert_eq!(
        launching.status, 200,
        "case \"launching\": expected 200 from CreateEnvironment, got {}",
        launching.body
    );

    // Rows span the classes of refusal: no identifier at all, an identifier
    // that names nothing (either member), an identifier past a modelled bound,
    // and an environment in each status that reports no abortable operation.
    let rows = [
        RefusalRow {
            case: "neither EnvironmentId nor EnvironmentName",
            params: vec![],
            status: 400,
            code: "MissingRequiredParameter",
        },
        RefusalRow {
            case: "an EnvironmentName that names nothing",
            params: vec![("EnvironmentName", "ghost-env".to_string())],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "an EnvironmentId that names nothing",
            params: vec![("EnvironmentId", "e-00000000".to_string())],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "an EnvironmentId that names nothing, with a name that does",
            params: vec![
                ("EnvironmentId", "e-00000000".to_string()),
                ("EnvironmentName", "ready-env".to_string()),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            // The modelled maximum length for `EnvironmentName` is 40, so 41
            // characters is one past the published bound. A value the MODEL
            // refuses is a request-shape failure, so it carries the
            // request-validation code rather than `InvalidParameterValue`.
            case: "an EnvironmentName one over the modelled maximum of 40",
            params: vec![("EnvironmentName", "e".repeat(41))],
            status: 400,
            code: "ValidationError",
        },
        RefusalRow {
            case: "an EnvironmentName one under the modelled minimum of 4",
            params: vec![("EnvironmentName", "env".to_string())],
            status: 400,
            code: "ValidationError",
        },
        RefusalRow {
            case: "a Ready environment with no update in progress",
            params: vec![("EnvironmentName", "ready-env".to_string())],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "the same Ready environment named by its id",
            params: vec![("EnvironmentId", ready_id.clone())],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "a Launching environment",
            params: vec![("EnvironmentName", "launch-env".to_string())],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "a Terminating environment",
            params: vec![("EnvironmentName", "gone-env".to_string())],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "abort twice: the cancellation is already under way",
            params: vec![("EnvironmentName", "aborting-env".to_string())],
            status: 400,
            code: "InvalidParameterValue",
        },
    ];

    for RefusalRow {
        case,
        params,
        status,
        code,
    } in &rows
    {
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let refused = emulator.call("AbortEnvironmentUpdate", &params).await;
        assert_eq!(
            refused.status, *status,
            "case {case:?}: expected HTTP {status}, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected error code {code}, got body {}",
            refused.body
        );
        assert!(
            refused.value("Error/Message").is_some(),
            "case {case:?}: a refusal must say what to fix, got body {}",
            refused.body
        );
    }

    // And a refused abort changed nothing: the Ready environment is still ready
    // and still reports no abortable operation.
    assert_eq!(
        described(&emulator, "ready-env").await,
        vec![
            "Ready".to_string(),
            "false".to_string(),
            "Green".to_string(),
            "Ok".to_string()
        ],
        "case \"refused abort\": a refusal must not move the environment"
    );
}
