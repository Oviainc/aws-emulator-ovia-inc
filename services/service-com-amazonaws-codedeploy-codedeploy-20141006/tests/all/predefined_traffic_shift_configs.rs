// SPDX-License-Identifier: CC0-1.0

//! The deployment configurations CodeDeploy predefines for time-based traffic
//! shifting, and the order `ContinueDeployment` classifies a request in.
//!
//! RULES under test, each over a SET of inputs rather than one recorded request:
//!
//! * every `CodeDeployDefault.*` configuration CodeDeploy ships resolves for
//!   every account without anyone creating it — the time-based canary and linear
//!   ones of the Lambda and Amazon ECS compute platforms included — so a reader
//!   answers it and a deployment may name it;
//! * an operation over an existing deployment classifies on THAT DEPLOYMENT
//!   first — whether it exists, then whether its (immutable) type supports the
//!   action, then how far it has got — and only then on the value of any other
//!   member the request carried.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness of `codedeploy_lifecycle` — so the boundary validator,
//! this service's refusal wording and the awsJson1_1 envelope are all exercised,
//! not bypassed.

use serde_json::{json, Value};

use std::time::{Duration, Instant};

use crate::codedeploy_lifecycle::{
    error_code, s3_revision, terminal_deployment, terminal_deployment_within, Emulator,
    POLL_DEADLINE, POLL_INTERVAL,
};

/// The IAM role every deployment group here is created with.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// How long a canary deployment created here is given to walk its window and
/// reach a terminal status: longer than the window the five-minute built-in
/// below holds its first increment for.
const CANARY_WINDOW_DEADLINE: Duration = Duration::from_secs(30);

/// One predefined deployment configuration that shifts traffic over TIME, with
/// the traffic-routing settings documented for it.
struct TimeBased {
    name: &'static str,
    /// The `computePlatform` the configuration belongs to.
    platform: &'static str,
    /// `trafficRoutingConfig.type`, a member of the model's enum
    /// (`smithy_query(query="shape",shape="TrafficRoutingType")`:
    /// TimeBasedCanary | TimeBasedLinear | AllAtOnce).
    routing: &'static str,
    /// The percentage of traffic the first (canary) or each (linear) increment
    /// shifts.
    percentage: i64,
    /// The number of minutes between traffic shifts.
    interval: i64,
}

/// Every predefined configuration that shifts traffic over time.
///
/// Catalog source: the AWS CodeDeploy User Guide's "Deployment configurations on
/// an AWS Lambda compute platform" and "Deployment configurations on an Amazon
/// ECS compute platform" tables, whose names are spelled exactly as those tables
/// spell them — the one-minute Lambda linear entry singular ("...Every1Minute"),
/// the one-minute Amazon ECS one plural ("...Every1Minutes"). The recorded unit
/// behind this suite named only `LambdaCanary10Percent5Minutes`; every other row
/// here is a configuration it did not name.
const TIME_BASED: &[TimeBased] = &[
    TimeBased {
        name: "CodeDeployDefault.LambdaCanary10Percent5Minutes",
        platform: "Lambda",
        routing: "TimeBasedCanary",
        percentage: 10,
        interval: 5,
    },
    TimeBased {
        name: "CodeDeployDefault.LambdaCanary10Percent10Minutes",
        platform: "Lambda",
        routing: "TimeBasedCanary",
        percentage: 10,
        interval: 10,
    },
    TimeBased {
        name: "CodeDeployDefault.LambdaCanary10Percent15Minutes",
        platform: "Lambda",
        routing: "TimeBasedCanary",
        percentage: 10,
        interval: 15,
    },
    TimeBased {
        name: "CodeDeployDefault.LambdaCanary10Percent30Minutes",
        platform: "Lambda",
        routing: "TimeBasedCanary",
        percentage: 10,
        interval: 30,
    },
    TimeBased {
        name: "CodeDeployDefault.LambdaLinear10PercentEvery1Minute",
        platform: "Lambda",
        routing: "TimeBasedLinear",
        percentage: 10,
        interval: 1,
    },
    TimeBased {
        name: "CodeDeployDefault.LambdaLinear10PercentEvery2Minutes",
        platform: "Lambda",
        routing: "TimeBasedLinear",
        percentage: 10,
        interval: 2,
    },
    TimeBased {
        name: "CodeDeployDefault.LambdaLinear10PercentEvery3Minutes",
        platform: "Lambda",
        routing: "TimeBasedLinear",
        percentage: 10,
        interval: 3,
    },
    TimeBased {
        name: "CodeDeployDefault.LambdaLinear10PercentEvery10Minutes",
        platform: "Lambda",
        routing: "TimeBasedLinear",
        percentage: 10,
        interval: 10,
    },
    TimeBased {
        name: "CodeDeployDefault.ECSCanary10Percent5Minutes",
        platform: "ECS",
        routing: "TimeBasedCanary",
        percentage: 10,
        interval: 5,
    },
    TimeBased {
        name: "CodeDeployDefault.ECSCanary10Percent15Minutes",
        platform: "ECS",
        routing: "TimeBasedCanary",
        percentage: 10,
        interval: 15,
    },
    TimeBased {
        name: "CodeDeployDefault.ECSLinear10PercentEvery1Minutes",
        platform: "ECS",
        routing: "TimeBasedLinear",
        percentage: 10,
        interval: 1,
    },
    TimeBased {
        name: "CodeDeployDefault.ECSLinear10PercentEvery3Minutes",
        platform: "ECS",
        routing: "TimeBasedLinear",
        percentage: 10,
        interval: 3,
    },
];

/// Every deployment configuration name the listing reports, following its
/// `nextToken` to the end so the listing is the whole listing.
async fn listed_configs(emulator: &Emulator) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = emulator.ok("ListDeploymentConfigs", request).await;
        collected.extend(
            page["deploymentConfigsList"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string)),
        );
        token = page["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return collected;
        }
        assert!(
            collected.len() < 1_000,
            "case the whole listing: paging did not terminate, got {} names",
            collected.len()
        );
    }
}

/// Poll `GetDeployment` until the deployment has STARTED, which is a transition
/// the clock owes it, answering its `deploymentInfo`.
async fn started_deployment(emulator: &Emulator, deployment_id: &str, case: &str) -> Value {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let info = emulator
            .ok("GetDeployment", json!({"deploymentId": deployment_id}))
            .await["deploymentInfo"]
            .clone();
        if info["startTime"].as_f64().is_some() {
            return info;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the deployment to start within {POLL_DEADLINE:?}, got \
             {info:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create an application on `platform` and a deployment group on it, answering
/// the group name. `extra` members are added to the group request when they are
/// not null.
async fn group_on(
    emulator: &Emulator,
    application: &str,
    platform: &str,
    extra: Value,
) -> String {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": platform}),
        )
        .await;
    let group = format!("{application}-dg");
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": group,
        "serviceRoleArn": SERVICE_ROLE,
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            request[member] = value.clone();
        }
    }
    emulator.ok("CreateDeploymentGroup", request).await;
    group
}

/// RULE: every predefined configuration resolves without being created, and
/// reports the compute platform and the traffic-routing settings documented for
/// it.
#[tokio::test]
async fn every_predefined_time_based_configuration_resolves() {
    let emulator = Emulator::new();
    let listed = listed_configs(&emulator).await;

    for case in TIME_BASED {
        let TimeBased {
            name,
            platform,
            routing,
            percentage,
            interval,
        } = case;

        let fetched = emulator
            .ok("GetDeploymentConfig", json!({"deploymentConfigName": name}))
            .await;
        let info = &fetched["deploymentConfigInfo"];
        assert_eq!(
            info["deploymentConfigName"].as_str(),
            Some(*name),
            "case {name:?}: expected the configuration's own name, got {info:?}"
        );
        assert!(
            !info["deploymentConfigId"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {name:?}: expected a deploymentConfigId, got {info:?}"
        );
        assert_eq!(
            info["computePlatform"].as_str(),
            Some(*platform),
            "case {name:?}: expected computePlatform {platform}, got {info:?}"
        );
        assert!(
            info["minimumHealthyHosts"].is_null(),
            "case {name:?}: a traffic-shifting configuration carries no \
             minimumHealthyHosts, got {info:?}"
        );
        let config = &info["trafficRoutingConfig"];
        assert_eq!(
            config["type"].as_str(),
            Some(*routing),
            "case {name:?}: expected trafficRoutingConfig type {routing}, got {info:?}"
        );
        let (shifted, every) = match *routing {
            "TimeBasedCanary" => (
                config["timeBasedCanary"]["canaryPercentage"].as_i64(),
                config["timeBasedCanary"]["canaryInterval"].as_i64(),
            ),
            _ => (
                config["timeBasedLinear"]["linearPercentage"].as_i64(),
                config["timeBasedLinear"]["linearInterval"].as_i64(),
            ),
        };
        assert_eq!(
            (shifted, every),
            (Some(*percentage), Some(*interval)),
            "case {name:?}: expected {percentage}% every {interval} minutes, got {info:?}"
        );

        // The reader and the listing answer from the same store, so a name one
        // resolves is a name the other reports.
        assert_eq!(
            listed.iter().filter(|listed| *listed == name).count(),
            1,
            "case {name:?}: expected it listed exactly once, got {listed:?}"
        );
    }
}

/// RULE: a deployment may name any predefined configuration — the call is
/// accepted and the deployment records the configuration it runs under — while a
/// name nothing ships and nobody created is refused with
/// `DeploymentConfigDoesNotExistException`.
#[tokio::test]
async fn a_deployment_can_name_a_predefined_time_based_configuration() {
    let emulator = Emulator::new();

    for (index, case) in TIME_BASED.iter().enumerate() {
        let TimeBased {
            name, platform, ..
        } = case;
        let application = format!("shift-app-{index:02}");
        // The group names its platform's all-at-once default; the DEPLOYMENT
        // overrides it with the time-based configuration under test, which is
        // how a client asks for a canary or linear traffic shift.
        let group = group_on(&emulator, &application, platform, Value::Null).await;

        let created = emulator
            .ok(
                "CreateDeployment",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": group,
                    "deploymentConfigName": name,
                    "revision": {
                        "revisionType": "AppSpecContent",
                        "appSpecContent": {"content": "version: 0.0", "sha256": "ab"},
                    },
                }),
            )
            .await;
        let deployment_id = created["deploymentId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !deployment_id.is_empty(),
            "case {name:?}: expected a minted deploymentId, got {created:?}"
        );

        // Read back through the implemented reader: the deployment records the
        // configuration the request named, not the group's default.
        let info = emulator
            .ok("GetDeployment", json!({"deploymentId": deployment_id}))
            .await["deploymentInfo"]
            .clone();
        assert_eq!(
            info["deploymentConfigName"].as_str(),
            Some(*name),
            "case {name:?}: expected the deployment to record the configuration it names, got \
             {info:?}"
        );
    }

    // Rows that name no configuration this account holds: a `CodeDeployDefault.*`
    // spelling nothing ships, a percentage/interval combination outside the
    // shipped catalog, and a name of a client's own.
    let group = group_on(&emulator, "shift-app-unknown", "Lambda", Value::Null).await;
    for unknown in [
        "CodeDeployDefault.LambdaCanary5Percent5Minutes",
        "CodeDeployDefault.LambdaLinear10PercentEvery7Minutes",
        "CodeDeployDefault.ECSCanary10Percent30Minutes",
        "my-own-canary",
    ] {
        let (status, body) = emulator
            .call(
                "CreateDeployment",
                json!({
                    "applicationName": "shift-app-unknown",
                    "deploymentGroupName": group,
                    "deploymentConfigName": unknown,
                    "revision": s3_revision("artifacts", "app.zip"),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "DeploymentConfigDoesNotExistException"),
            "case {unknown:?}: expected 400 DeploymentConfigDoesNotExistException, got {status} \
             {body:?}"
        );
    }
}

/// RULE: `ContinueDeployment` answers the refusal the DEPLOYMENT earns — it does
/// not exist, or its type can never be continued, or it is already complete —
/// before it judges the `deploymentWaitType` the request carried, so the answer
/// to a given deployment does not change with that member.
#[tokio::test]
async fn continue_deployment_classifies_the_deployment_before_the_wait_type() {
    let emulator = Emulator::new();

    // An in-place deployment (CodeDeploy's default style) driven to a terminal
    // status, on each compute platform that reaches one here: the type is fixed
    // when the deployment is created, so a finished deployment is refused for its
    // type just as a running one is.
    let server_group = group_on(&emulator, "continue-server", "Server", Value::Null).await;
    let server = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": "continue-server",
                "deploymentGroupName": server_group,
                "revision": s3_revision("artifacts", "app.zip"),
            }),
        )
        .await["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    terminal_deployment(&emulator, &server, "a finished in-place deployment").await;

    let lambda_group = group_on(&emulator, "continue-lambda", "Lambda", Value::Null).await;
    let lambda = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": "continue-lambda",
                "deploymentGroupName": lambda_group,
                "deploymentConfigName": "CodeDeployDefault.LambdaCanary10Percent5Minutes",
                "revision": {
                    "revisionType": "AppSpecContent",
                    "appSpecContent": {"content": "version: 0.0", "sha256": "cd"},
                },
            }),
        )
        .await["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    terminal_deployment_within(
        &emulator,
        &lambda,
        "a finished canary deployment",
        // A canary holds its first increment for the interval its configuration
        // states before it cuts over, so this deployment is non-terminal for that
        // whole window: the five-minute interval of the built-in above, on this
        // emulator's own clock scale.
        CANARY_WINDOW_DEADLINE,
    )
    .await;

    // A blue/green deployment whose group holds it open for a termination wait
    // far longer than this test, so it is neither complete nor of a type that
    // can never be continued: the rows against it are the ones that DO reach the
    // wait type the request carried.
    let blue_green_group = group_on(
        &emulator,
        "continue-blue-green",
        "Server",
        json!({
            "deploymentStyle": {
                "deploymentType": "BLUE_GREEN",
                "deploymentOption": "WITH_TRAFFIC_CONTROL",
            },
            "blueGreenDeploymentConfiguration": {
                "terminateBlueInstancesOnDeploymentSuccess": {
                    "action": "TERMINATE",
                    "terminationWaitTimeInMinutes": 2880,
                },
            },
        }),
    )
    .await;
    let blue_green = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": "continue-blue-green",
                "deploymentGroupName": blue_green_group,
                "revision": s3_revision("artifacts", "app.zip"),
            }),
        )
        .await["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The traffic shift of a blue/green deployment happens once it has STARTED,
    // which the clock owes it, so the termination wait is only there to be
    // continued past after that.
    let running = started_deployment(&emulator, &blue_green, "a running blue/green deployment").await;
    assert!(
        !matches!(running["status"].as_str(), Some("Succeeded" | "Failed" | "Stopped")),
        "case a running blue/green deployment: expected the termination wait to hold it open, got \
         {running:?}"
    );

    // Rows differ along the deployment the request names (its type, its progress,
    // the form of its id) and along the `deploymentWaitType` member — absent, and
    // each value the model's enum carries
    // (`smithy_query(query="shape",shape="DeploymentWaitType")`: READY_WAIT |
    // TERMINATION_WAIT). The deployment decides the answer; the member does not.
    let cases: &[(&str, Value, &str)] = &[
        (
            "a finished in-place deployment, no wait type",
            json!({"deploymentId": server}),
            "UnsupportedActionForDeploymentTypeException",
        ),
        (
            "a finished in-place deployment, READY_WAIT",
            json!({"deploymentId": server, "deploymentWaitType": "READY_WAIT"}),
            "UnsupportedActionForDeploymentTypeException",
        ),
        (
            "a finished in-place deployment, TERMINATION_WAIT",
            json!({"deploymentId": server, "deploymentWaitType": "TERMINATION_WAIT"}),
            "UnsupportedActionForDeploymentTypeException",
        ),
        (
            "a finished in-place canary deployment, no wait type",
            json!({"deploymentId": lambda}),
            "UnsupportedActionForDeploymentTypeException",
        ),
        (
            "a well-formed id no deployment has, no wait type",
            json!({"deploymentId": "d-ABCDEFGHI"}),
            "DeploymentDoesNotExistException",
        ),
        (
            "a value that is not a deployment id, no wait type",
            json!({"deploymentId": "not-an-id"}),
            "InvalidDeploymentIdException",
        ),
        (
            "no deployment id at all, no wait type",
            json!({}),
            "DeploymentIdRequiredException",
        ),
        (
            // Only a deployment that passes its OWN checks reaches the member:
            // a blue/green deployment still running has two waits, so a request
            // that named neither is the one the wait-type refusal is for.
            "a running blue/green deployment, no wait type",
            json!({"deploymentId": blue_green}),
            "InvalidDeploymentWaitTypeException",
        ),
        (
            "a running blue/green deployment, READY_WAIT",
            json!({"deploymentId": blue_green, "deploymentWaitType": "READY_WAIT"}),
            "DeploymentIsNotInReadyStateException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator.call("ContinueDeployment", request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case:?}: expected 400 {expected}, got {status} {body:?}"
        );
    }

    // The refusals changed nothing: the blue/green deployment is still there to
    // be continued, and continuing its termination wait succeeds.
    emulator
        .ok(
            "ContinueDeployment",
            json!({
                "deploymentId": blue_green,
                "deploymentWaitType": "TERMINATION_WAIT",
            }),
        )
        .await;
}
