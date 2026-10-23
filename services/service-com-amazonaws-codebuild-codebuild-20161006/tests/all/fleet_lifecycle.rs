// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CodeBuild's compute-fleet slice (`CreateFleet` ->
//! `BatchGetFleets`), driven over the wire the router serves through the harness
//! [`super::project_build_lifecycle`] already owns: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! Every assertion here is about state: what a create persisted is what a later read
//! reports, and nothing is recomputed per call.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{call, emulator, emulator_on, error_type, expected_arn, REGION};

/// Long enough for a fleet's clock-owed move out of `CREATING` to have fallen due
/// (the service settles it [`state::FLEET_SETTLE_MILLIS`] after `CreateFleet`).
const PAST_THE_FLEET_SETTLE_SPAN: Duration =
    Duration::from_millis(state::FLEET_SETTLE_MILLIS as u64 + 350);

/// A minimal valid `CreateFleet` request for `name`, with `extra`'s members merged in.
///
/// The compute and environment types are two of the values the model's `ComputeType` /
/// `EnvironmentType` enums declare; a row that cares about them overrides them.
fn create_fleet_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "name": name,
        "baseCapacity": 2,
        "environmentType": "LINUX_CONTAINER",
        "computeType": "BUILD_GENERAL1_SMALL",
    });
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extra) {
        target.extend(members);
    }
    body
}

/// Create `name`, asserting it succeeded, and hand back the reported fleet.
async fn create_fleet(router: &MultiServiceRouter, name: &str, extra: Value) -> Value {
    let (status, body) = call(router, "CreateFleet", create_fleet_body(name, extra)).await;
    assert_eq!(
        status, 200,
        "CreateFleet {name:?}: expected 200, got {status} with {body}"
    );
    body["fleet"].clone()
}

/// `BatchGetFleets` over `identifiers`, asserting it succeeded.
async fn batch_get_fleets(router: &MultiServiceRouter, identifiers: &[&str]) -> Value {
    let (status, body) = call(router, "BatchGetFleets", json!({"names": identifiers})).await;
    assert_eq!(
        status, 200,
        "BatchGetFleets {identifiers:?}: expected 200, got {status} with {body}"
    );
    body
}

/// The whole slice, in the order a client drives it: what `CreateFleet` persisted is
/// what `BatchGetFleets` reports, resolved by name or by ARN, with the identifiers the
/// store holds no fleet for split out.
#[tokio::test]
async fn a_created_fleet_round_trips_through_batch_get_fleets() {
    let router = emulator();

    let created = create_fleet(
        &router,
        "round-trip-fleet",
        json!({
            "baseCapacity": 3,
            "environmentType": "ARM_CONTAINER",
            "computeType": "BUILD_GENERAL1_LARGE",
            "overflowBehavior": "QUEUE",
            "imageId": "ami-0123456789abcdef0",
            "fleetServiceRole": "arn:aws:iam::000000000000:role/service-role/fleet",
            "scalingConfiguration": {
                "scalingType": "TARGET_TRACKING_SCALING",
                "maxCapacity": 9,
                "targetTrackingScalingConfigs": [
                    {"metricType": "FLEET_UTILIZATION_RATE", "targetValue": 0.75},
                ],
            },
            "vpcConfig": {
                "vpcId": "vpc-0123456789abcdef0",
                "subnets": ["subnet-0123456789abcdef0"],
                "securityGroupIds": ["sg-0123456789abcdef0"],
            },
            "proxyConfiguration": {
                "defaultBehavior": "ALLOW_ALL",
                "orderedProxyRules": [
                    {"type": "DOMAIN", "effect": "DENY", "entities": ["example.com"]},
                ],
            },
            "tags": [{"key": "team", "value": "builders"}],
        }),
    )
    .await;

    let id = created["id"].as_str().expect("an id").to_string();
    assert_eq!(
        created["name"], "round-trip-fleet",
        "the created fleet reports the requested name, got {created}"
    );
    assert_eq!(
        id,
        format!(
            "round-trip-fleet:{}",
            id.split_once(':').expect("<name>:<uuid>").1
        ),
        "a fleet id is `<name>:<uuid>`, got {created}"
    );
    assert_eq!(
        created["arn"],
        json!(expected_arn(&format!("fleet/{id}"))),
        "the fleet ARN addresses `fleet/<id>` in the request's region, got {created}"
    );
    assert_eq!(
        created["status"]["statusCode"], "CREATING",
        "a fleet read straight after CreateFleet is still CREATING, got {created}"
    );
    assert_eq!(
        created["baseCapacity"], 3,
        "the created fleet reports the requested baseCapacity, got {created}"
    );
    assert_eq!(
        created["scalingConfiguration"]["desiredCapacity"], 3,
        "a new fleet wants exactly the machines baseCapacity asked for, got {created}"
    );
    assert_eq!(
        created["scalingConfiguration"]["maxCapacity"], 9,
        "the created fleet reports the requested maxCapacity, got {created}"
    );
    assert_eq!(
        created["created"], created["lastModified"],
        "a fleet nothing has updated was last modified when it was created, got {created}"
    );

    // Read it back by NAME and by ARN: `BatchGetFleets$names` takes either, and both
    // must resolve to the one stored record.
    let arn = created["arn"].as_str().expect("an arn").to_string();
    for identifier in [created["name"].as_str().expect("a name"), arn.as_str()] {
        let read = batch_get_fleets(&router, &[identifier]).await;
        assert_eq!(
            read["fleets"][0], created,
            "BatchGetFleets {identifier:?}: expected the stored fleet, got {read}"
        );
        assert!(
            read["fleetsNotFound"].is_null(),
            "BatchGetFleets {identifier:?}: nothing is missing, got {read}"
        );
    }

    // Identifiers the store holds no fleet for, in the order the request named them:
    // an unknown name, an ARN of another resource type, a fleet ARN with no id, an
    // ARN of another service, and this fleet's own ARN with the account, the region
    // and the fleet id each changed in turn — an ARN resolves only to the fleet it
    // addresses exactly.
    let foreign_account = arn.replace(engine::EMULATOR_ACCOUNT_ID, "000000000001");
    let foreign_region = arn.replace(REGION, "eu-west-1");
    let wrong_id = format!(
        "{}:{}",
        expected_arn("fleet/round-trip-fleet"),
        "00000000-0000-8000-8000-000000000000"
    );
    let missing = [
        "no-such-fleet".to_string(),
        expected_arn("project/round-trip-fleet"),
        expected_arn("fleet/round-trip-fleet"),
        format!("arn:aws:codepipeline:{REGION}:000000000000:fleet/round-trip-fleet:1"),
        foreign_account,
        foreign_region,
        wrong_id,
    ];
    let identifiers: Vec<&str> = missing.iter().map(String::as_str).collect();
    let read = batch_get_fleets(&router, &identifiers).await;
    assert!(
        read["fleets"].is_null(),
        "BatchGetFleets over unknown identifiers reports no fleets, got {read}"
    );
    assert_eq!(
        read["fleetsNotFound"],
        json!(missing),
        "every unresolvable identifier is reported back, got {read}"
    );

    // A mixed request keeps the requested order across both lists.
    let read = batch_get_fleets(&router, &["no-such-fleet", "round-trip-fleet"]).await;
    assert_eq!(
        read["fleets"],
        json!([created]),
        "a mixed BatchGetFleets reports the fleet it holds, got {read}"
    );
    assert_eq!(
        read["fleetsNotFound"],
        json!(["no-such-fleet"]),
        "a mixed BatchGetFleets reports the name it does not hold, got {read}"
    );
}

/// RULE: a fleet's capacity is provisioned, so the move out of `CREATING` is owed by the
/// CLOCK — materialised from the stored instants when the fleet is read, once, and
/// identically on every later read.
#[tokio::test]
async fn a_fleet_settles_to_active_from_stored_state_and_never_changes_again() {
    let router = emulator();
    let created = create_fleet(&router, "settle-fleet", json!({})).await;

    // Before the span elapses the caller still sees the transient status.
    let early = batch_get_fleets(&router, &["settle-fleet"]).await;
    assert_eq!(
        early["fleets"][0]["status"]["statusCode"], "CREATING",
        "a fleet read inside the provisioning span is still CREATING, got {early}"
    );

    tokio::time::sleep(PAST_THE_FLEET_SETTLE_SPAN).await;

    let settled = batch_get_fleets(&router, &["settle-fleet"]).await;
    let record = settled["fleets"][0].clone();
    assert_eq!(
        record["status"]["statusCode"], "ACTIVE",
        "a provisioned fleet reports ACTIVE, got {record}"
    );
    assert!(
        record["status"]["context"].is_null(),
        "an ACTIVE fleet carries no failure context, got {record}"
    );
    assert_eq!(
        record["created"], created["created"],
        "settling must not rewrite the creation instant, got {record}"
    );
    assert_eq!(
        record["lastModified"], created["lastModified"],
        "settling is not a modification of the fleet, got {record}"
    );

    // Read again: identical, because it was computed once from the stored instants.
    let reread = batch_get_fleets(&router, &["settle-fleet"]).await;
    assert_eq!(
        reread["fleets"][0], record,
        "a settled fleet must never be re-fabricated per call, got {reread}"
    );
}

/// RULE: every member of the model's `FleetCapacity` shape counts machines
/// (`baseCapacity` is "the initial number of machines allocated to the fleet",
/// `maxCapacity` "the maximum number of instances in the fleet when auto-scaling"), so a
/// value below one is refused with the modelled `InvalidInputException`; one is the
/// smallest accepted count. The model declares no `@range` on `FleetCapacity`, so the
/// boundary validator passes every row below through to the handler.
#[tokio::test]
async fn a_capacity_below_one_machine_is_refused_on_every_capacity_member() {
    let router = emulator();

    // (case, members merged into the request, the refused member or "" when accepted)
    let cases: [(&str, Value, &str); 11] = [
        ("baseCapacity at the bound", json!({"baseCapacity": 1}), ""),
        ("baseCapacity well inside", json!({"baseCapacity": 64}), ""),
        (
            "baseCapacity one past the bound",
            json!({"baseCapacity": 0}),
            "baseCapacity",
        ),
        (
            "baseCapacity negative",
            json!({"baseCapacity": -7}),
            "baseCapacity",
        ),
        (
            "maxCapacity at the bound",
            json!({"baseCapacity": 1, "scalingConfiguration": {"maxCapacity": 1}}),
            "",
        ),
        (
            "maxCapacity well inside",
            json!({"baseCapacity": 2, "scalingConfiguration": {"maxCapacity": 40}}),
            "",
        ),
        (
            "maxCapacity one past the bound",
            json!({"baseCapacity": 2, "scalingConfiguration": {"maxCapacity": 0}}),
            "scalingConfiguration.maxCapacity",
        ),
        (
            "maxCapacity negative",
            json!({"baseCapacity": 2, "scalingConfiguration": {"maxCapacity": -1}}),
            "scalingConfiguration.maxCapacity",
        ),
        (
            "both members past the bound: the first one named wins",
            json!({"baseCapacity": 0, "scalingConfiguration": {"maxCapacity": -3}}),
            "baseCapacity",
        ),
        // The range `maxCapacity` opens has to contain `baseCapacity`: a fleet cannot
        // be told to scale no higher than fewer machines than it is provisioned with.
        (
            "maxCapacity equal to baseCapacity",
            json!({"baseCapacity": 5, "scalingConfiguration": {"maxCapacity": 5}}),
            "",
        ),
        (
            "maxCapacity below baseCapacity",
            json!({"baseCapacity": 5, "scalingConfiguration": {"maxCapacity": 4}}),
            "scalingConfiguration.maxCapacity",
        ),
    ];

    for (index, (case, members, refused)) in cases.into_iter().enumerate() {
        let name = format!("capacity-fleet-{index}");
        let (status, body) = call(
            &router,
            "CreateFleet",
            create_fleet_body(&name, members.clone()),
        )
        .await;
        if refused.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(refused),
            "case {case:?}: the refusal must name {refused:?}, got {body}"
        );

        // A refused request persists nothing.
        let read = batch_get_fleets(&router, &[&name]).await;
        assert!(
            read["fleets"].is_null(),
            "case {case:?}: a refused CreateFleet must persist nothing, got {read}"
        );
    }
}

/// RULE: AWS documents `computeConfiguration` as required exactly when `computeType` is
/// `ATTRIBUTE_BASED_COMPUTE` or `CUSTOM_INSTANCE_TYPE` (`Fleet$computeConfiguration`) —
/// the two compute types whose machine is described by attributes rather than named by
/// the compute type itself — and optional for every other value of the enum. The rows
/// below take their compute types from the model's `ComputeType` enum
/// (`smithy_query(query="shape",shape="ComputeType")`).
#[tokio::test]
async fn compute_configuration_is_required_exactly_for_the_attribute_driven_compute_types()
{
    let router = emulator();

    // (compute type, a computeConfiguration describing it, is it required?)
    let cases: [(&str, Value, bool); 5] = [
        (
            "ATTRIBUTE_BASED_COMPUTE",
            json!({"vCpu": 4, "memory": 8, "disk": 64, "machineType": "GENERAL"}),
            true,
        ),
        (
            "CUSTOM_INSTANCE_TYPE",
            json!({"instanceType": "m6i.large"}),
            true,
        ),
        ("BUILD_GENERAL1_SMALL", json!({}), false),
        ("BUILD_GENERAL1_2XLARGE", json!({}), false),
        ("BUILD_LAMBDA_4GB", json!({}), false),
    ];

    for (compute_type, configuration, required) in cases {
        let bare = format!("bare-{}", compute_type.to_lowercase().replace('_', "-"));
        let (status, body) = call(
            &router,
            "CreateFleet",
            create_fleet_body(&bare, json!({"computeType": compute_type})),
        )
        .await;
        if required {
            assert_eq!(
                status, 400,
                "case {compute_type:?} without computeConfiguration: expected 400, \
                 got {status} with {body}"
            );
            assert_eq!(
                error_type(&body),
                "InvalidInputException",
                "case {compute_type:?} without computeConfiguration: \
                 expected InvalidInputException, got {body}"
            );
            assert!(
                body["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("computeConfiguration"),
                "case {compute_type:?}: the refusal must name computeConfiguration, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {compute_type:?} without computeConfiguration: expected 200, \
                 got {status} with {body}"
            );
            assert_eq!(
                body["fleet"]["computeType"],
                json!(compute_type),
                "case {compute_type:?}: the created fleet reports the requested \
                 computeType, got {body}"
            );
        }

        // With a configuration, every compute type is accepted and the configuration is
        // what a later read reports.
        let configured = format!("configured-{}", compute_type.to_lowercase().replace('_', "-"));
        let fleet = create_fleet(
            &router,
            &configured,
            json!({"computeType": compute_type, "computeConfiguration": configuration.clone()}),
        )
        .await;
        assert_eq!(
            fleet["computeConfiguration"], configuration,
            "case {compute_type:?}: the stored computeConfiguration is what was given, \
             got {fleet}"
        );
        let read = batch_get_fleets(&router, &[&configured]).await;
        assert_eq!(
            read["fleets"][0], fleet,
            "case {compute_type:?}: the read fleet is the created one, got {read}"
        );
    }
}

/// RULE: a fleet name identifies one fleet per account, so creating a second fleet under
/// a name the account already holds is refused with the modelled
/// `ResourceAlreadyExistsException` and leaves the held fleet untouched. The rows vary
/// the environment type, which the fleet echoes back from the model's `EnvironmentType`
/// enum (`smithy_query(query="shape",shape="EnvironmentType")`).
#[tokio::test]
async fn a_fleet_name_identifies_one_fleet_and_the_record_echoes_its_environment_type() {
    let router = emulator();

    for environment_type in ["LINUX_EC2", "WINDOWS_SERVER_2022_CONTAINER", "MAC_ARM"] {
        let name = format!("unique-{}", environment_type.to_lowercase().replace('_', "-"));
        let created = create_fleet(
            &router,
            &name,
            json!({"environmentType": environment_type}),
        )
        .await;
        assert_eq!(
            created["environmentType"],
            json!(environment_type),
            "case {environment_type:?}: the created fleet echoes the environment type, \
             got {created}"
        );

        let (status, body) = call(
            &router,
            "CreateFleet",
            create_fleet_body(&name, json!({"environmentType": "LINUX_CONTAINER"})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {environment_type:?}: a duplicate name expects 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceAlreadyExistsException",
            "case {environment_type:?}: expected ResourceAlreadyExistsException, got {body}"
        );

        let read = batch_get_fleets(&router, &[&name]).await;
        assert_eq!(
            read["fleets"][0], created,
            "case {environment_type:?}: the refused create must not rewrite the held \
             fleet, got {read}"
        );
    }
}

/// RULE: the account's compute-fleet quota is the ceiling that bounds the fleet store, and
/// a create past it is refused with the modelled `AccountLimitExceededException` rather
/// than growing the store. Seeded through the very store the handlers write, so the test
/// does not have to issue [`state::MAX_FLEETS`] requests.
#[tokio::test]
async fn creating_a_fleet_past_the_account_quota_is_refused() {
    use std::sync::Arc;

    use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::Fleet;
    use service_com_amazonaws_codebuild_codebuild_20161006::state::{
        ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StoredFleet,
    };

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let seeded = engine.state::<CodeBuildState>();
        let mut guard = seeded.lock();
        for index in 0..state::MAX_FLEETS {
            let name = format!("seeded-fleet-{index}");
            guard.fleets.insert(
                name.clone(),
                StoredFleet {
                    fleet: Fleet {
                        name: Some(name),
                        ..Fleet::default()
                    },
                    created_millis: 0,
                    last_modified_millis: 0,
                    active_due_millis: 0,
                },
            );
        }
    }
    let router = emulator_on(engine);

    let (status, body) = call(
        &router,
        "CreateFleet",
        create_fleet_body("one-past-the-quota", json!({})),
    )
    .await;
    assert_eq!(
        status, 400,
        "CreateFleet at the account quota: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_type(&body),
        "AccountLimitExceededException",
        "CreateFleet at the account quota: expected AccountLimitExceededException, got {body}"
    );
    let read = batch_get_fleets(&router, &["one-past-the-quota"]).await;
    assert!(
        read["fleets"].is_null(),
        "a refused CreateFleet must persist nothing, got {read}"
    );
}
