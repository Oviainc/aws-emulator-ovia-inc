// SPDX-License-Identifier: CC0-1.0

//! How AppConfig WORDS a refusal, over the set of lookups and conflicts that produce one.
//!
//! Two rules are under test, each one level more general than any single request:
//!
//! 1. A `ResourceNotFoundException` message is the looked-up resource KIND's own AppConfig
//!    template, filled in from the request's identifiers: the identifier labels that
//!    template echoes (and no identifier the template withholds), and the account clause
//!    only where that kind's template reports one. The templates are not uniform — the
//!    deployment strategy's names no account and the extension's echoes no identifier —
//!    so one invented sentence shape cannot serve them all, and every operation that
//!    reaches the same lookup must word it the same way.
//! 2. `StartDeployment`'s in-flight guard is a `ConflictException` attributed to the
//!    ENVIRONMENT: it names the environment and the state it currently has, and nothing
//!    about the deployment holding it.
//!
//! The tables span what those rules generalise over: the resource kind, the operation
//! reaching the lookup (read, write and delete paths), the shape of the identifier the
//! request sent (minted id, a name, an ARN, both branches of `DeploymentStrategyId`'s
//! pattern), and — for rule 2 — the state the blocking deployment is in, across strategies
//! whose rollout window and bake time put it in each of them.
//!
//! Rule 1 is also asserted ACROSS resource kinds, one row per kind, by
//! `control_plane_fidelity`'s own not-found table (configuration profile, hosted
//! configuration version, deployment and resource ARN among them); this suite is the
//! other axis of the same rule — every OPERATION that reaches the same lookup, over every
//! identifier form it accepts.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::DEPLOYMENT_STATE_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    ENVIRONMENT_DEPLOYING, PREDEFINED_DEPLOYMENT_STRATEGIES,
};

use super::appconfig_workflow::{
    account, create_application, create_environment, create_profile, json_call, router, send,
    Answer,
};
use super::resource_tags::pct;

/// One lookup that must be refused, and the message the looked-up kind's template makes
/// of it.
struct Lookup {
    /// What a failing assertion calls this row: the operation and the identifier shape.
    case: String,
    method: &'static str,
    uri: String,
    body: Value,
    /// The message the resource kind's own template produces for this request.
    expected: String,
    /// Text this kind's template WITHHOLDS, which the message must therefore not carry.
    withheld: Vec<String>,
}

/// Host version 1 of a profile's configuration, so there is something to deploy.
async fn host_version(router: &MultiServiceRouter, application: &str, profile: &str) {
    let answer = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        br#"{"enabled":true}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// A `StartDeployment` body naming the profile, the version and the strategy.
fn start_deployment_body(profile: &str, strategy: &str) -> Value {
    json!({
        "ConfigurationProfileId": profile,
        "ConfigurationVersion": "1",
        "DeploymentStrategyId": strategy,
    })
}

/// `POST /applications/{app}/environments/{env}/deployments`.
async fn start_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    body: &Value,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        body,
    )
    .await
}

// =============================================================================
// 1. the per-resource-kind not-found templates
// =============================================================================

/// Every operation that fails the same lookup refuses it with that resource kind's own
/// AppConfig template — identifier labels and account clause exactly as that kind carries
/// them — whatever identifier the request sent.
#[tokio::test]
async fn a_not_found_message_is_the_looked_up_kinds_own_template() {
    let router = router();
    let application = create_application(&router, "refusal-templates").await;
    let environment = create_environment(&router, &application, "prod").await;
    let profile = create_profile(&router, &application, "config").await;
    let account = account();

    let mut rows: Vec<Lookup> = Vec::new();

    // Identifiers that address nothing, differing along length and form. `ApplicationId`
    // and `EnvironmentId` target `Name` (`length 1..64`, no pattern — `smithy_query`
    // query="operation" for GetEnvironment), so every one of these reaches the lookup
    // rather than being refused as malformed.
    let unknown_names = [
        "zzzzzzz",
        "nosuch1",
        "ab12",
        "a",
        "an-environment-nobody-ever-created",
    ];

    for unknown in unknown_names {
        // The environment template names BOTH path labels and the account.
        let expected = format!(
            "Environment with Application Id {application} and Environment Id {unknown} could \
             not be found for account {account}."
        );
        let path = format!("/applications/{application}/environments/{unknown}");
        for (operation, method, uri, body) in [
            ("GetEnvironment", "GET", path.clone(), Value::Null),
            (
                "UpdateEnvironment",
                "PATCH",
                path.clone(),
                json!({ "Description": "x" }),
            ),
            ("DeleteEnvironment", "DELETE", path.clone(), Value::Null),
            (
                "StartDeployment",
                "POST",
                format!("{path}/deployments"),
                start_deployment_body(&profile, PREDEFINED_DEPLOYMENT_STRATEGIES[0].id),
            ),
        ] {
            rows.push(Lookup {
                case: format!("{operation} on environment {unknown:?}"),
                method,
                uri,
                body,
                expected: expected.clone(),
                withheld: Vec::new(),
            });
        }

        // The application template names its one label and the account — on read, write
        // and child-create paths alike, including a path whose deeper label would have
        // been the environment's.
        let expected = format!("Application with Id {unknown} could not be found for account {account}.");
        for (operation, method, uri, body) in [
            (
                "GetApplication",
                "GET",
                format!("/applications/{unknown}"),
                Value::Null,
            ),
            (
                "CreateConfigurationProfile",
                "POST",
                format!("/applications/{unknown}/configurationprofiles"),
                json!({ "Name": "orphan", "LocationUri": "hosted" }),
            ),
            (
                "CreateEnvironment",
                "POST",
                format!("/applications/{unknown}/environments"),
                json!({ "Name": "orphan" }),
            ),
            (
                "GetEnvironment under an unknown application",
                "GET",
                format!("/applications/{unknown}/environments/{environment}"),
                Value::Null,
            ),
        ] {
            rows.push(Lookup {
                case: format!("{operation} on application {unknown:?}"),
                method,
                uri,
                body,
                expected: expected.clone(),
                withheld: Vec::new(),
            });
        }
    }

    // Both branches of `DeploymentStrategyId`'s pattern
    // (`^(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)$`, from
    // `smithy_query(query="operation", operation="GetDeploymentStrategy")`), neither
    // naming a created or a predefined strategy.
    for unknown in ["nostrat", "zz12", "AppConfig.NotARealOne"] {
        // The strategy's template spells the kind as one word and ends at "could not be
        // found": it reports no account.
        let expected = format!("DeploymentStrategy with Id {unknown} could not be found.");
        for (operation, method, uri, body) in [
            (
                "GetDeploymentStrategy",
                "GET",
                format!("/deploymentstrategies/{unknown}"),
                Value::Null,
            ),
            (
                "DeleteDeploymentStrategy",
                "DELETE",
                // The delete's URI is the model's own misspelling of the collection
                // segment (`/deployementstrategies/{DeploymentStrategyId}` from
                // `smithy_query(query="operation",
                // operation="DeleteDeploymentStrategy")`), not the reader's path.
                format!("/deployementstrategies/{unknown}"),
                Value::Null,
            ),
            (
                "StartDeployment",
                "POST",
                format!("/applications/{application}/environments/{environment}/deployments"),
                start_deployment_body(&profile, unknown),
            ),
        ] {
            rows.push(Lookup {
                case: format!("{operation} on deployment strategy {unknown:?}"),
                method,
                uri,
                body,
                expected: expected.clone(),
                // The account clause this one template omits.
                withheld: vec!["for account".to_string()],
            });
        }
    }

    // `ExtensionIdentifier` is "the name, the ID, or the ARN of the extension", so the
    // rows span all three forms. The label is non-greedy, so an ARN travels
    // percent-encoded.
    for unknown in [
        "zzzz999".to_string(),
        "an-extension-name-nobody-used".to_string(),
        engine::emulator_arn("appconfig", "us-east-1", "extension/zzzz999/1"),
    ] {
        // The extension's template echoes NO identifier: only the account.
        let expected = format!("Extension could not be found for account {account}.");
        for (operation, method) in [("GetExtension", "GET"), ("DeleteExtension", "DELETE")] {
            rows.push(Lookup {
                case: format!("{operation} on extension {unknown:?}"),
                method,
                uri: format!("/extensions/{}", pct(&unknown)),
                body: Value::Null,
                expected: expected.clone(),
                // The identifier AppConfig withholds — it is not reflected back.
                withheld: vec![unknown.clone()],
            });
        }
    }

    for Lookup {
        case,
        method,
        uri,
        body,
        expected,
        withheld,
    } in &rows
    {
        let answer = json_call(&router, method, uri, body).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer
                .error_code()
                .split('#')
                .next_back()
                .unwrap_or_default(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.error_code()
        );
        let message = answer.string("Message").unwrap_or_default();
        assert_eq!(
            message, *expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        for absent in withheld {
            assert!(
                !message.contains(absent.as_str()),
                "case {case:?}: this kind's template withholds {absent:?}, got {message:?}"
            );
        }
    }
}

// =============================================================================
// 2. the in-flight-deployment conflict
// =============================================================================

/// One blocking deployment: the criteria of the strategy that paces it, chosen so the
/// deployment is still in flight when the second `StartDeployment` runs.
struct Blocking {
    case: &'static str,
    /// `None` names a predefined strategy by id instead of creating one.
    predefined: Option<&'static str>,
    duration: i32,
    growth_factor: f64,
    growth_type: &'static str,
    bake: i32,
}

/// The strategies the blocking deployment is paced by. The bounds are the model's own:
/// `MinutesBetween0And24Hours` is `range 0..1440` and `Percentage` is `range 1..100`
/// (`smithy_query(query="operation", operation="StartDeployment")`). The rows differ along
/// which phase the blocking deployment is in — rolling out, or rolled out and baking — and
/// along whether the strategy is the caller's or AppConfig's own.
const BLOCKING: [Blocking; 4] = [
    Blocking {
        case: "a rollout still in progress",
        predefined: None,
        duration: 60,
        growth_factor: 10.0,
        growth_type: "LINEAR",
        bake: 0,
    },
    Blocking {
        case: "rolled out to everybody, still baking",
        predefined: None,
        duration: 0,
        growth_factor: 100.0,
        growth_type: "LINEAR",
        bake: 20,
    },
    Blocking {
        case: "the longest rollout and the longest bake the model allows",
        predefined: None,
        duration: 1440,
        growth_factor: 1.0,
        growth_type: "EXPONENTIAL",
        bake: 1440,
    },
    Blocking {
        case: "AppConfig's own canary strategy",
        predefined: Some("AppConfig.Canary10Percent20Minutes"),
        duration: 20,
        growth_factor: 10.0,
        growth_type: "EXPONENTIAL",
        bake: 10,
    },
];

/// A second deployment into an environment that already has one in flight is a 409
/// `ConflictException` whose message names the ENVIRONMENT and the state it currently has
/// — not the deployment number holding it — whichever phase that deployment is in.
#[tokio::test]
async fn an_in_flight_deployment_conflicts_on_the_environments_state() {
    let router = router();

    for (index, row) in BLOCKING.iter().enumerate() {
        let case = row.case;
        // Its own application and environment, so each row's conflict is about its own
        // in-flight deployment.
        let application = create_application(&router, &format!("conflict-{index}")).await;
        let environment = create_environment(&router, &application, "prod").await;
        let profile = create_profile(&router, &application, "config").await;
        host_version(&router, &application, &profile).await;

        let strategy = match row.predefined {
            Some(id) => {
                assert!(
                    PREDEFINED_DEPLOYMENT_STRATEGIES
                        .iter()
                        .any(|predefined| predefined.id == id),
                    "case {case:?}: {id:?} must be in AppConfig's predefined catalog"
                );
                id.to_string()
            }
            None => {
                let created = json_call(
                    &router,
                    "POST",
                    "/deploymentstrategies",
                    &json!({
                        "Name": format!("blocking-{index}"),
                        "DeploymentDurationInMinutes": row.duration,
                        "GrowthFactor": row.growth_factor,
                        "GrowthType": row.growth_type,
                        "FinalBakeTimeInMinutes": row.bake,
                        "ReplicateTo": "NONE",
                    }),
                )
                .await;
                assert_eq!(
                    created.status,
                    201,
                    "case {case:?}: CreateDeploymentStrategy must answer 201, got {} / {}",
                    created.status,
                    String::from_utf8_lossy(&created.body)
                );
                created
                    .string("Id")
                    .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", created.json()))
            }
        };

        let body = start_deployment_body(&profile, &strategy);
        let first = start_deployment(&router, &application, &environment, &body).await;
        assert_eq!(
            first.status,
            201,
            "case {case:?}: the blocking deployment must start, got {} / {}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        // The state the refusal must report is read off the deployment that blocks, not
        // written per row.
        let blocking_state = first
            .string("State")
            .unwrap_or_else(|| panic!("case {case:?}: no State in {}", first.json()));
        assert!(
            DEPLOYMENT_STATE_VALUES.contains(&blocking_state.as_str()),
            "case {case:?}: the blocking deployment's State must be a modelled \
             DeploymentState, got {blocking_state:?}"
        );
        assert_ne!(
            blocking_state, "COMPLETE",
            "case {case:?}: this row's strategy must leave the deployment in flight, got \
             {blocking_state:?}"
        );

        let second = start_deployment(&router, &application, &environment, &body).await;
        assert_eq!(
            second.status,
            409,
            "case {case:?}: a second deployment must be refused, got {} / {}",
            second.status,
            String::from_utf8_lossy(&second.body)
        );
        assert_eq!(
            second
                .error_code()
                .split('#')
                .next_back()
                .unwrap_or_default(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {}",
            second.error_code()
        );
        let expected = format!(
            "Environment {environment} currently has a state of {blocking_state}. The current \
             deployment must finish before a new deployment can be started."
        );
        let message = second.string("Message").unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert!(
            !message.to_lowercase().contains("deployment number"),
            "case {case:?}: AppConfig attributes this conflict to the environment, not to a \
             deployment number, got {message:?}"
        );

        // The environment really is mid-deployment, and reports that through its own
        // `EnvironmentState` wire value — a different spelling from the DeploymentState
        // the refusal carries, which is why the message's state is not read off the
        // environment record.
        let read_back = json_call(
            &router,
            "GET",
            &format!("/applications/{application}/environments/{environment}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            read_back.status,
            200,
            "case {case:?}: GetEnvironment must answer 200, got {} / {}",
            read_back.status,
            String::from_utf8_lossy(&read_back.body)
        );
        assert_eq!(
            read_back.string("State").as_deref(),
            Some(ENVIRONMENT_DEPLOYING),
            "case {case:?}: an environment with a deployment in flight is Deploying, got {:?}",
            read_back.string("State")
        );
    }
}
