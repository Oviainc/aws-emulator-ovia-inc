// SPDX-License-Identifier: CC0-1.0

//! A patch of a deployment strategy is what every LATER consumer of that strategy reads.
//!
//! The rule under test: the members `UpdateDeploymentStrategy` carries
//! (`Description`, `DeploymentDurationInMinutes`, `FinalBakeTimeInMinutes`, `GrowthFactor`,
//! `GrowthType`) overwrite the one stored deployment-strategy record, so the patched
//! record — not the creation-time one — is what the update's own response, a later
//! `GetDeploymentStrategy` and every deployment started with that strategy afterwards all
//! report
//! (`smithy_query(query="operation",operation="UpdateDeploymentStrategy,GetDeploymentStrategy,StartDeployment")`:
//! the three operations carry the same rollout-criteria members, and `StartDeployment`
//! paces a deployment by the strategy its `DeploymentStrategyId` names).
//!
//! `update_deployment_strategy.rs` pins the patch against the strategy's own readers; this
//! suite pins the consumer a stale record would mis-pace: a deployment. Its rows span what
//! the rule generalises over — which members the patch carries (one alone, all of them,
//! none) and the values they carry (both published bounds of `MinutesBetween0And24Hours`
//! (range 0..1440) and of `GrowthFactor` (range 1..100), and each `GrowthType` the model
//! lists). Every assertion names its case and prints the actual value.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, send, Answer,
};

/// The collection the model binds the deployment-strategy operations to.
const STRATEGIES: &str = "/deploymentstrategies";

/// The rollout criteria a strategy record holds, as every reader of it must report them.
#[derive(Clone, Debug)]
struct Criteria {
    description: Option<&'static str>,
    duration: i64,
    bake: i64,
    growth_factor: f64,
    growth_type: &'static str,
}

/// Assert `document` reports the rollout criteria `expected`, naming the reader it came
/// from. `Description` belongs to the strategy's own readers, so it is checked only when
/// `with_description` says this document is one of them (a deployment carries a
/// description of its own).
fn assert_criteria(
    case: &str,
    reader: &str,
    document: &Value,
    expected: &Criteria,
    with_description: bool,
) {
    assert_eq!(
        document
            .get("DeploymentDurationInMinutes")
            .and_then(Value::as_i64),
        Some(expected.duration),
        "case {case:?}: {reader} expected DeploymentDurationInMinutes {}, got {document}",
        expected.duration
    );
    assert_eq!(
        document
            .get("FinalBakeTimeInMinutes")
            .and_then(Value::as_i64),
        Some(expected.bake),
        "case {case:?}: {reader} expected FinalBakeTimeInMinutes {}, got {document}",
        expected.bake
    );
    let growth_factor = document
        .get("GrowthFactor")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {reader} reported no numeric GrowthFactor, got {document}")
        });
    assert!(
        (growth_factor - expected.growth_factor).abs() < 1e-6,
        "case {case:?}: {reader} expected GrowthFactor {}, got {growth_factor}",
        expected.growth_factor
    );
    assert_eq!(
        document.get("GrowthType").and_then(Value::as_str),
        Some(expected.growth_type),
        "case {case:?}: {reader} expected GrowthType {:?}, got {document}",
        expected.growth_type
    );
    if with_description {
        assert_eq!(
            document.get("Description").and_then(Value::as_str),
            expected.description,
            "case {case:?}: {reader} expected Description {:?}, got {document}",
            expected.description
        );
    }
}

/// `StartDeployment` into `environment`, deploying version 1 of `profile` with `strategy`.
async fn start_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
    strategy: &str,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await
}

#[tokio::test]
async fn a_patched_strategy_paces_every_deployment_started_after_it() {
    let router = router();
    let application = create_application(&router, "patched-pacing").await;
    let profile = create_profile(&router, &application, "deployed-config").await;

    // There must be something to deploy before a deployment can name version 1 of it.
    let hosted = send(
        &router,
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
        hosted.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        hosted.status,
        String::from_utf8_lossy(&hosted.body)
    );

    let created = json_call(
        &router,
        "POST",
        STRATEGIES,
        &json!({
            "Name": "pacing",
            "Description": "created",
            "DeploymentDurationInMinutes": 10,
            "FinalBakeTimeInMinutes": 0,
            "GrowthFactor": 10,
            "GrowthType": "LINEAR",
            "ReplicateTo": "NONE",
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateDeploymentStrategy expected 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let strategy = created
        .string("Id")
        .expect("CreateDeploymentStrategy answered no Id");

    // A deployment started BEFORE any patch is paced by the creation-time record, which is
    // the control the rows below are a change from.
    let before_environment = create_environment(&router, &application, "env-before").await;
    let before = start_deployment(
        &router,
        &application,
        &before_environment,
        &profile,
        &strategy,
    )
    .await;
    assert_eq!(
        before.status,
        201,
        "case \"before any patch\": StartDeployment expected 201, got {} / {}",
        before.status,
        String::from_utf8_lossy(&before.body)
    );
    let creation_time = Criteria {
        description: Some("created"),
        duration: 10,
        bake: 0,
        growth_factor: 10.0,
        growth_type: "LINEAR",
    };
    assert_criteria(
        "before any patch",
        "StartDeployment",
        &before.json(),
        &creation_time,
        false,
    );
    let before_number = before
        .json()
        .get("DeploymentNumber")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            panic!(
                "case \"before any patch\": StartDeployment answered no DeploymentNumber, got {}",
                before.json()
            )
        });

    // Rows are applied in order to the one strategy, so a row also asserts that the members
    // it does not carry still report what the previous row left stored.
    let rows: Vec<(&str, Value, Criteria)> = vec![
        (
            "duration, bake time, growth factor and description together",
            json!({
                "Description": "updated-strategy",
                "DeploymentDurationInMinutes": 20,
                "FinalBakeTimeInMinutes": 5,
                "GrowthFactor": 25,
            }),
            Criteria {
                description: Some("updated-strategy"),
                duration: 20,
                bake: 5,
                growth_factor: 25.0,
                growth_type: "LINEAR",
            },
        ),
        (
            "the growth factor alone",
            json!({ "GrowthFactor": 50 }),
            Criteria {
                description: Some("updated-strategy"),
                duration: 20,
                bake: 5,
                growth_factor: 50.0,
                growth_type: "LINEAR",
            },
        ),
        (
            "every member at its published lower bound, growing exponentially",
            json!({
                "DeploymentDurationInMinutes": 0,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 1,
                "GrowthType": "EXPONENTIAL",
            }),
            Criteria {
                description: Some("updated-strategy"),
                duration: 0,
                bake: 0,
                growth_factor: 1.0,
                growth_type: "EXPONENTIAL",
            },
        ),
        (
            "every member at its published upper bound",
            json!({
                "DeploymentDurationInMinutes": 1440,
                "FinalBakeTimeInMinutes": 1440,
                "GrowthFactor": 100,
                "GrowthType": "LINEAR",
            }),
            Criteria {
                description: Some("updated-strategy"),
                duration: 1440,
                bake: 1440,
                growth_factor: 100.0,
                growth_type: "LINEAR",
            },
        ),
        (
            "a patch carrying no member at all",
            json!({}),
            Criteria {
                description: Some("updated-strategy"),
                duration: 1440,
                bake: 1440,
                growth_factor: 100.0,
                growth_type: "LINEAR",
            },
        ),
    ];

    for (index, (case, body, expected)) in rows.iter().enumerate() {
        let patched = json_call(
            &router,
            "PATCH",
            &format!("{STRATEGIES}/{strategy}"),
            body,
        )
        .await;
        assert_eq!(
            patched.status,
            200,
            "case {case:?}: UpdateDeploymentStrategy expected 200, got {} / {}",
            patched.status,
            String::from_utf8_lossy(&patched.body)
        );
        assert_criteria(
            case,
            "UpdateDeploymentStrategy",
            &patched.json(),
            expected,
            true,
        );

        let fetched = json_call(
            &router,
            "GET",
            &format!("{STRATEGIES}/{strategy}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetDeploymentStrategy expected 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_criteria(
            case,
            "GetDeploymentStrategy",
            &fetched.json(),
            expected,
            true,
        );

        // Its own environment per row: only one deployment at a time can be in progress for
        // an environment, and the rows above leave deployments rolling out for hours.
        let environment =
            create_environment(&router, &application, &format!("env-patch-{index}")).await;
        let started = start_deployment(&router, &application, &environment, &profile, &strategy)
            .await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment expected 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        assert_criteria(case, "StartDeployment", &started.json(), expected, false);

        // Read back the deployment this row just started, by the number the start answered,
        // rather than by a positional assumption about the environment's numbering.
        let number = started
            .json()
            .get("DeploymentNumber")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: StartDeployment answered no DeploymentNumber, got {}",
                    started.json()
                )
            });
        let read_back = json_call(
            &router,
            "GET",
            &format!(
                "/applications/{application}/environments/{environment}/deployments/{number}"
            ),
            &Value::Null,
        )
        .await;
        assert_eq!(
            read_back.status,
            200,
            "case {case:?}: GetDeployment expected 200, got {} / {}",
            read_back.status,
            String::from_utf8_lossy(&read_back.body)
        );
        assert_criteria(case, "GetDeployment", &read_back.json(), expected, false);

        // The deployment started before any patch keeps the criteria it was paced with: a
        // patch rewrites the strategy, never a deployment already under way.
        let earlier = json_call(
            &router,
            "GET",
            &format!(
                "/applications/{application}/environments/{before_environment}/deployments/\
                 {before_number}"
            ),
            &Value::Null,
        )
        .await;
        assert_eq!(
            earlier.status,
            200,
            "case {case:?}: GetDeployment of the earlier deployment expected 200, got {} / {}",
            earlier.status,
            String::from_utf8_lossy(&earlier.body)
        );
        assert_criteria(
            case,
            "GetDeployment of the deployment started before any patch",
            &earlier.json(),
            &creation_time,
            false,
        );
    }
}
