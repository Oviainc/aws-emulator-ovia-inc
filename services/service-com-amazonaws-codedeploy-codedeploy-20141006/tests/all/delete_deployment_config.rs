// SPDX-License-Identifier: CC0-1.0

//! `DeleteDeploymentConfig` over the wire: what it removes from the store, what
//! it refuses, and what it leaves exactly where it was.
//!
//! RULE: the call removes the deployment configuration the request names from
//! this account's configurations and answers success whether or not the store
//! held one under that name (the operation models no not-found error, so the
//! delete is idempotent) — except that a configuration CodeDeploy itself ships is
//! refused with `InvalidOperationException` ("Predefined configurations cannot be
//! deleted") and one something still uses is refused with
//! `DeploymentConfigInUseException` ("A deployment configuration cannot be
//! deleted if it is currently in use"), and a refused call removes nothing.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness in `codedeploy_lifecycle` — so the boundary validator,
//! this service's refusal wording and the awsJson1_1 envelope are all exercised,
//! not bypassed.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006 as codedeploy;

use crate::codedeploy_lifecycle::{error_code, s3_revision, seeded_group, Emulator};

/// The `CodeDeployDefault.*` configurations this service ships, one per compute
/// platform, as the AWS CodeDeploy User Guide's "deployment configurations"
/// pages list them (the same catalog `src/state.rs` seeds).
const PREDEFINED: &[&str] = &[
    "CodeDeployDefault.OneAtATime",
    "CodeDeployDefault.HalfAtATime",
    "CodeDeployDefault.AllAtOnce",
    "CodeDeployDefault.LambdaAllAtOnce",
    "CodeDeployDefault.ECSAllAtOnce",
];

/// Create the deployment configuration `name` from `members`, failing the case
/// when the create itself is refused.
async fn create_config(emulator: &Emulator, name: &str, members: &Value) {
    let mut request = json!({"deploymentConfigName": name});
    for (member, value) in members.as_object().expect("the case body is an object") {
        request[member] = value.clone();
    }
    let (status, body) = emulator.call("CreateDeploymentConfig", request).await;
    assert_eq!(
        status, 200,
        "case {name:?}: the configuration must be creatable, got {status} {body:?}"
    );
}

/// Call `DeleteDeploymentConfig` for `name`, answering its status and body.
async fn delete_config(emulator: &Emulator, name: &str) -> (u16, Value) {
    emulator
        .call(
            "DeleteDeploymentConfig",
            json!({"deploymentConfigName": name}),
        )
        .await
}

/// `GetDeploymentConfig` for `name`, as (status, body) so a case can assert the
/// configuration is gone as well as that it is there.
async fn get_config(emulator: &Emulator, name: &str) -> (u16, Value) {
    emulator
        .call("GetDeploymentConfig", json!({"deploymentConfigName": name}))
        .await
}

/// Every deployment configuration name `ListDeploymentConfigs` reports, following
/// its `nextToken` to the end so the listing is the whole listing and not one
/// page of it.
///
/// The listing is what a stale index shows up in: the reader answers from the
/// map, so a name left behind in the order index is invisible to
/// `GetDeploymentConfig` and visible only here.
async fn listed_configs(emulator: &Emulator) -> Vec<String> {
    let mut names = Vec::new();
    let mut token = Value::Null;
    loop {
        let request = if token.is_null() {
            json!({})
        } else {
            json!({"nextToken": token})
        };
        let page = emulator.ok("ListDeploymentConfigs", request).await;
        names.extend(
            page["deploymentConfigsList"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string)),
        );
        token = page["nextToken"].clone();
        if token.is_null() {
            return names;
        }
    }
}

/// How many times `name` appears in the listing.
fn listed_count(names: &[String], name: &str) -> usize {
    names.iter().filter(|listed| listed.as_str() == name).count()
}

/// Require that the store no longer answers for `name`.
async fn assert_absent(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = get_config(emulator, name).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a deleted configuration must no longer be readable, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "DeploymentConfigDoesNotExistException",
        "case {case:?}: expected DeploymentConfigDoesNotExistException for {name}, got {body:?}"
    );
}

/// Require that the store still answers for `name`.
async fn assert_present(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = get_config(emulator, name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {name} must still be readable, got {status} {body:?}"
    );
    assert_eq!(
        body["deploymentConfigInfo"]["deploymentConfigName"].as_str(),
        Some(name),
        "case {case:?}: expected the stored record for {name}, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// What a delete removes
// ---------------------------------------------------------------------------

/// RULE: a configuration a client created is removed by the call — it is no
/// longer readable, and its name is free for a new configuration — whatever the
/// configuration carries.
#[tokio::test]
async fn a_created_configuration_is_removed_by_the_call() {
    let emulator = Emulator::new();
    // Rows differ along what the configuration IS (the compute platform and
    // which optional member it carries, including the canary traffic shift) and
    // along the FORM of its name: the model's `DeploymentConfigName` is 1..100
    // characters of `^[A-Za-z0-9+=,.@_-]*$`, so the bound (100) and the
    // punctuation the pattern allows are rows too.
    let long_name = "n".repeat(100);
    let cases: &[(&str, Value)] = &[
        (
            "cfg-host-count",
            json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 3}}),
        ),
        (
            "cfg-fleet-percent",
            json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "FLEET_PERCENT", "value": 75}}),
        ),
        ("cfg-bare", json!({"computePlatform": "Server"})),
        (
            "cfg-lambda-canary",
            json!({
                "computePlatform": "Lambda",
                "trafficRoutingConfig": {
                    "type": "TimeBasedCanary",
                    "timeBasedCanary": {"canaryPercentage": 10, "canaryInterval": 5},
                },
            }),
        ),
        (
            "cfg-ecs-linear",
            json!({
                "computePlatform": "ECS",
                "trafficRoutingConfig": {
                    "type": "TimeBasedLinear",
                    "timeBasedLinear": {"linearPercentage": 20, "linearInterval": 1},
                },
            }),
        ),
        (
            "cfg-zonal",
            json!({
                "computePlatform": "Server",
                "minimumHealthyHosts": {"type": "FLEET_PERCENT", "value": 50},
                "zonalConfig": {
                    "firstZoneMonitorDurationInSeconds": 120,
                    "monitorDurationInSeconds": 60,
                    "minimumHealthyHostsPerZone": {"type": "HOST_COUNT", "value": 2},
                },
            }),
        ),
        // The punctuation the modelled pattern allows, and a name on the
        // modelled length bound.
        ("cfg+=,.@_-", json!({"computePlatform": "Server"})),
        (long_name.as_str(), json!({"computePlatform": "Server"})),
    ];

    for (name, members) in cases {
        create_config(&emulator, name, members).await;
        assert_present(&emulator, name, name).await;

        let (status, body) = delete_config(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: deleting a created configuration must succeed, got {status} {body:?}"
        );
        assert_eq!(
            body["__type"],
            Value::Null,
            "case {name:?}: a successful delete carries no error, got {body:?}"
        );
        assert_absent(&emulator, name, name).await;
        // The listing answers from the same store, so a deleted configuration is
        // gone from it too.
        let listed = listed_configs(&emulator).await;
        assert_eq!(
            listed_count(&listed, name),
            0,
            "case {name:?}: a deleted configuration must not be listed, got {listed:?}"
        );

        // The name is free again, which is what proves the record really left
        // the store rather than being hidden from the reader — and it comes back
        // ONCE, so the delete left nothing of it behind in the listing index.
        create_config(&emulator, name, members).await;
        assert_present(&emulator, name, name).await;
        let listed = listed_configs(&emulator).await;
        assert_eq!(
            listed_count(&listed, name),
            1,
            "case {name:?}: a re-created configuration must be listed exactly once, got {listed:?}"
        );
        for shipped in PREDEFINED {
            assert_eq!(
                listed_count(&listed, shipped),
                1,
                "case {name:?}: {shipped} must still be listed exactly once, got {listed:?}"
            );
        }
    }
}

/// RULE: the delete names ONE configuration — every other configuration the
/// store holds, created or shipped, is untouched.
#[tokio::test]
async fn a_delete_leaves_every_other_configuration_in_place() {
    let emulator = Emulator::new();
    let created = ["cfg-keep-a", "cfg-deleted", "cfg-keep-b"];
    for name in created {
        create_config(&emulator, name, &json!({"computePlatform": "Server"})).await;
    }

    let (status, body) = delete_config(&emulator, "cfg-deleted").await;
    assert_eq!(
        status, 200,
        "case cfg-deleted: expected HTTP 200, got {status} {body:?}"
    );

    assert_absent(&emulator, "cfg-deleted", "cfg-deleted").await;
    for name in ["cfg-keep-a", "cfg-keep-b"] {
        assert_present(&emulator, "neighbour", name).await;
    }
    for name in PREDEFINED {
        assert_present(&emulator, "predefined-neighbour", name).await;
    }

    // The listing keeps every neighbour, in the order they entered the store,
    // with the deleted one dropped out of it.
    let listed = listed_configs(&emulator).await;
    let mut expected: Vec<String> = PREDEFINED.iter().map(|name| name.to_string()).collect();
    expected.extend(["cfg-keep-a".to_string(), "cfg-keep-b".to_string()]);
    assert_eq!(
        listed, expected,
        "case cfg-deleted: expected the store's remaining configurations in entry order, got \
         {listed:?}"
    );
}

/// RULE: the operation models no not-found error, so deleting a name the store
/// does not hold is a success that removes nothing — a second delete of the same
/// configuration answers exactly as the first did.
#[tokio::test]
async fn deleting_a_name_the_store_does_not_hold_is_a_success() {
    let emulator = Emulator::new();
    create_config(&emulator, "cfg-once", &json!({"computePlatform": "Server"})).await;

    // Rows: a name nothing ever created, two more never-created forms, and a
    // name that WAS created and is deleted twice — the repeat is the idempotence
    // row.
    let cases: &[&str] = &[
        "cfg-never-created",
        "CodeDeployDefault.NotShipped",
        "x",
        "cfg-once",
        "cfg-once",
    ];
    for name in cases {
        let (status, body) = delete_config(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: deleting a name the store does not hold must succeed, got {status} \
             {body:?}"
        );
        assert_eq!(
            body["__type"],
            Value::Null,
            "case {name:?}: a successful delete carries no error, got {body:?}"
        );
        assert_absent(&emulator, name, name).await;
    }
}

// ---------------------------------------------------------------------------
// The configurations CodeDeploy ships
// ---------------------------------------------------------------------------

/// RULE: a configuration CodeDeploy itself ships cannot be deleted — the call is
/// refused with `InvalidOperationException` and the configuration stays readable
/// — and the refusal keys on that shipped catalog, not on how a name is spelled:
/// a configuration a client created under a `CodeDeployDefault.*`-shaped name is
/// a created configuration and is deleted like any other.
#[tokio::test]
async fn a_predefined_configuration_is_refused() {
    let emulator = Emulator::new();

    // Rows: every shipped configuration, including the two this issue's units
    // did not name.
    for name in PREDEFINED {
        let (status, body) = delete_config(&emulator, name).await;
        assert_eq!(
            status, 400,
            "case {name:?}: a predefined configuration must not be deletable, got {status} \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidOperationException",
            "case {name:?}: expected InvalidOperationException, got {body:?}"
        );
        // The refusal removed nothing.
        assert_present(&emulator, name, name).await;
    }

    // A name OUTSIDE that catalog is a created configuration, however it is
    // spelled.
    let lookalikes = ["CodeDeployDefault.TwoAtATime", "CodeDeployDefault"];
    for name in lookalikes {
        create_config(&emulator, name, &json!({"computePlatform": "Server"})).await;
        let (status, body) = delete_config(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a configuration a client created is deletable however it is spelled, \
             got {status} {body:?}"
        );
        assert_absent(&emulator, name, name).await;
    }
}

// ---------------------------------------------------------------------------
// A configuration something still uses
// ---------------------------------------------------------------------------

/// RULE: a configuration a deployment group names is in use, so deleting it is
/// refused with `DeploymentConfigInUseException` and it stays readable; once the
/// group names a different configuration, nothing uses it any more and the
/// delete succeeds.
#[tokio::test]
async fn a_configuration_a_deployment_group_names_is_in_use() {
    let emulator = Emulator::new();
    // Rows differ along the compute platform of the application whose group
    // holds the configuration, and along what the group is pointed at once it
    // lets go: another created configuration, and a shipped one.
    let cases: &[(&str, &str, &str, &str)] = &[
        ("app-in-use-server", "Server", "cfg-held-server", "cfg-free"),
        (
            "app-in-use-lambda",
            "Lambda",
            "cfg-held-lambda",
            "CodeDeployDefault.LambdaAllAtOnce",
        ),
    ];

    create_config(&emulator, "cfg-free", &json!({"computePlatform": "Server"})).await;
    for (application, platform, held, replacement) in cases {
        create_config(&emulator, held, &json!({"computePlatform": platform})).await;
        let group = seeded_group(&emulator, application, platform).await;
        emulator
            .ok(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "deploymentConfigName": held,
                }),
            )
            .await;

        let (status, body) = delete_config(&emulator, held).await;
        assert_eq!(
            status, 400,
            "case {held:?}: a configuration a deployment group names must not be deletable, got \
             {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "DeploymentConfigInUseException",
            "case {held:?}: expected DeploymentConfigInUseException, got {body:?}"
        );
        assert_present(&emulator, held, held).await;

        // The group lets go of it, so nothing uses it any more.
        emulator
            .ok(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "deploymentConfigName": replacement,
                }),
            )
            .await;
        let (status, body) = delete_config(&emulator, held).await;
        assert_eq!(
            status, 200,
            "case {held:?}: a configuration no group names any more must be deletable, got \
             {status} {body:?}"
        );
        assert_absent(&emulator, held, held).await;
    }
}

/// RULE: a configuration a deployment that has not reached an outcome runs under
/// is in use too, so deleting it is refused; once that deployment is terminal it
/// only RECORDS the configuration it ran under, so the delete succeeds.
///
/// The deployment is held in flight through the shared store the handlers write
/// through (`state::service_state`) — its clock-owed transitions are pushed out
/// of reach — so the case asserts the rule rather than racing the clock that owes
/// the deployment its completion.
#[tokio::test]
async fn a_configuration_an_unfinished_deployment_uses_is_in_use() {
    let emulator = Emulator::new();
    create_config(
        &emulator,
        "cfg-running",
        &json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 1}}),
    )
    .await;
    let group = seeded_group(&emulator, "app-running", "Server").await;
    // The deployment names the configuration itself; its GROUP still names
    // CodeDeploy's default, so what holds `cfg-running` is the deployment alone.
    let created = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": "app-running",
                "deploymentGroupName": group,
                "deploymentConfigName": "cfg-running",
                "revision": s3_revision("bucket-running", "app.zip"),
            }),
        )
        .await;
    let deployment_id = created["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !deployment_id.is_empty(),
        "case cfg-running: expected a minted deploymentId, got {created:?}"
    );

    hold_in_flight(&emulator, &deployment_id);
    let info = emulator
        .ok("GetDeployment", json!({"deploymentId": &deployment_id}))
        .await["deploymentInfo"]
        .clone();
    assert!(
        !matches!(
            info["status"].as_str(),
            Some("Succeeded" | "Failed" | "Stopped")
        ),
        "case cfg-running: the deployment must still be in flight, got {info:?}"
    );

    let (status, body) = delete_config(&emulator, "cfg-running").await;
    assert_eq!(
        status, 400,
        "case cfg-running: a configuration an unfinished deployment uses must not be deletable, \
         got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "DeploymentConfigInUseException",
        "case cfg-running: expected DeploymentConfigInUseException, got {body:?}"
    );
    assert_present(&emulator, "cfg-running", "cfg-running").await;

    // The deployment reaches its outcome: what it holds is now history.
    emulator
        .ok("StopDeployment", json!({"deploymentId": &deployment_id}))
        .await;
    let (status, body) = delete_config(&emulator, "cfg-running").await;
    assert_eq!(
        status, 200,
        "case cfg-running: a configuration only a finished deployment names must be deletable, \
         got {status} {body:?}"
    );
    assert_absent(&emulator, "cfg-running", "cfg-running").await;
}

/// Push the transitions the clock owes `deployment_id` out of reach, so the
/// deployment stays in flight for as long as the case needs it to.
///
/// This writes through `state::service_state` — the very store every handler
/// reads and writes, never a private one — because no API moves a deployment's
/// clock, and a case that instead raced the emulator's own 250–750 ms spans
/// would assert the clock rather than the rule.
fn hold_in_flight(emulator: &Emulator, deployment_id: &str) {
    /// Far enough out that no case here can reach it.
    const BEYOND_THE_CASE_MS: i64 = 10 * 60 * 1_000;

    let state = codedeploy::state::service_state(&*emulator.engine)
        .expect("the shared in-memory store is reachable");
    let mut guard = state.lock();
    let deployment = guard
        .deployments
        .get_mut(deployment_id)
        .expect("the deployment the emulator just created is in the store");
    let due = codedeploy::state::now_ms() + BEYOND_THE_CASE_MS;
    deployment.starts_at_ms = due;
    deployment.completes_at_ms = due;
    deployment.start_time_ms = None;
    deployment.complete_time_ms = None;
    deployment.status = codedeploy::state::DEPLOYMENT_STATUS_CREATED.to_string();
}
