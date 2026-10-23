// SPDX-License-Identifier: CC0-1.0

//! `CreateDeploymentConfig`, driven over the wire: what it stores, what it reads
//! back through `GetDeploymentConfig`, and which requests it refuses with which
//! modelled error.
//!
//! Every case here goes through the same front door a client uses: a signed-shape
//! `POST /` with `X-Amz-Target: CodeDeploy_20141006.<Operation>` dispatched by the
//! multi-service router, so the boundary validator, this service's refusal
//! wording and the awsJson1_1 envelope are all exercised, not bypassed. The
//! harness is this test target's single one, from `codedeploy_lifecycle`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator};

/// The `CodeDeployDefault.*` built-ins this service seeds, one per compute
/// platform, as the AWS CodeDeploy User Guide's "deployment configurations"
/// pages list them.
const PREDEFINED: &[&str] = &[
    "CodeDeployDefault.OneAtATime",
    "CodeDeployDefault.HalfAtATime",
    "CodeDeployDefault.AllAtOnce",
    "CodeDeployDefault.LambdaAllAtOnce",
    "CodeDeployDefault.ECSAllAtOnce",
];

// ---------------------------------------------------------------------------
// What a created configuration stores
// ---------------------------------------------------------------------------

/// RULE: a created deployment configuration is answered from the stored record —
/// `GetDeploymentConfig` reports the id `CreateDeploymentConfig` minted, the name
/// the request named, the compute platform it asked for (CodeDeploy's `Server`
/// when it named none) and every optional member it sent, unchanged.
#[tokio::test]
async fn a_deployment_config_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    // Rows differ along the compute platform and along WHICH optional member the
    // request carries, which is what the platform decides: a healthy-host count
    // or a zonal configuration on an instance fleet, a traffic-shifting
    // configuration on Lambda/ECS, and neither on a bare configuration.
    let cases: &[(&str, Value, &str)] = &[
        (
            "cfg-host-count",
            json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 3}}),
            "Server",
        ),
        (
            "cfg-fleet-percent",
            json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "FLEET_PERCENT", "value": 75}}),
            "Server",
        ),
        // `computePlatform` omitted: CodeDeploy's default platform.
        (
            "cfg-defaulted-platform",
            json!({"minimumHealthyHosts": {"type": "HOST_COUNT", "value": 1}}),
            "Server",
        ),
        (
            "cfg-no-optional-members",
            json!({"computePlatform": "Server"}),
            "Server",
        ),
        (
            "cfg-lambda-canary",
            json!({
                "computePlatform": "Lambda",
                "trafficRoutingConfig": {
                    "type": "TimeBasedCanary",
                    "timeBasedCanary": {"canaryPercentage": 10, "canaryInterval": 5},
                },
            }),
            "Lambda",
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
            "ECS",
        ),
        (
            "cfg-lambda-all-at-once",
            json!({"computePlatform": "Lambda", "trafficRoutingConfig": {"type": "AllAtOnce"}}),
            "Lambda",
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
            "Server",
        ),
    ];

    let mut minted_ids: BTreeSet<String> = BTreeSet::new();
    for (name, extra, expected_platform) in cases {
        let mut request = json!({"deploymentConfigName": name});
        for (member, value) in extra.as_object().expect("the case body is an object") {
            request[member] = value.clone();
        }

        let created = emulator
            .ok("CreateDeploymentConfig", request.clone())
            .await;
        let config_id = created["deploymentConfigId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !config_id.is_empty(),
            "case {name:?}: expected a minted deploymentConfigId, got {created:?}"
        );
        assert!(
            minted_ids.insert(config_id.clone()),
            "case {name:?}: expected a unique deploymentConfigId, but {config_id} was minted twice"
        );

        let fetched = emulator
            .ok("GetDeploymentConfig", json!({"deploymentConfigName": name}))
            .await;
        let info = &fetched["deploymentConfigInfo"];
        assert_eq!(
            info["deploymentConfigId"].as_str(),
            Some(config_id.as_str()),
            "case {name:?}: expected the minted deploymentConfigId {config_id}, got {info:?}"
        );
        assert_eq!(
            info["deploymentConfigName"].as_str(),
            Some(*name),
            "case {name:?}: expected the stored name, got {info:?}"
        );
        assert_eq!(
            info["computePlatform"].as_str(),
            Some(*expected_platform),
            "case {name:?}: expected computePlatform {expected_platform}, got {info:?}"
        );
        assert!(
            info["createTime"].is_number(),
            "case {name:?}: expected an epoch-seconds createTime, got {info:?}"
        );
        // Each optional member the request carried comes back exactly as sent,
        // and one it did not carry is not invented.
        for member in [
            "minimumHealthyHosts",
            "trafficRoutingConfig",
            "zonalConfig",
        ] {
            assert_eq!(
                &info[member],
                &request[member],
                "case {name:?}: expected {member} to read back as it was created, got {info:?}"
            );
        }
    }
}

/// RULE: a deployment configuration is read back by name, so two configurations
/// created with the same members under different names stay separate records.
#[tokio::test]
async fn two_configs_with_the_same_members_are_separate_records() {
    let emulator = Emulator::new();
    let members = json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "FLEET_PERCENT", "value": 90}});

    let mut ids = Vec::new();
    for name in ["twin-a", "twin-b"] {
        let mut request = members.clone();
        request["deploymentConfigName"] = json!(name);
        let created = emulator.ok("CreateDeploymentConfig", request).await;
        ids.push(
            created["deploymentConfigId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        let fetched = emulator
            .ok("GetDeploymentConfig", json!({"deploymentConfigName": name}))
            .await;
        assert_eq!(
            fetched["deploymentConfigInfo"]["deploymentConfigName"].as_str(),
            Some(name),
            "case {name:?}: expected the record stored under this name, got {fetched:?}"
        );
    }
    assert_ne!(
        ids[0], ids[1],
        "case twin-a/twin-b: expected two distinct ids, got {ids:?}"
    );
}

// ---------------------------------------------------------------------------
// Names already taken
// ---------------------------------------------------------------------------

/// RULE: a deployment configuration name is unique within the account, and the
/// `CodeDeployDefault.*` built-ins already hold theirs — so creating one under a
/// name the store already carries is refused with
/// `DeploymentConfigAlreadyExistsException`, and the stored record is untouched.
#[tokio::test]
async fn a_name_already_in_the_store_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateDeploymentConfig",
            json!({
                "deploymentConfigName": "cfg-taken",
                "computePlatform": "Server",
                "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 4},
            }),
        )
        .await;

    // Rows: a name a client created, and every built-in name — the store carries
    // both kinds, so both are taken.
    let mut taken: Vec<&str> = vec!["cfg-taken"];
    taken.extend_from_slice(PREDEFINED);

    for name in taken {
        let (status, body) = emulator
            .call(
                "CreateDeploymentConfig",
                json!({
                    "deploymentConfigName": name,
                    "computePlatform": "Server",
                    "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 9},
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 for a name already in use, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "DeploymentConfigAlreadyExistsException",
            "case {name:?}: expected DeploymentConfigAlreadyExistsException, got {body:?}"
        );
    }

    // The refused creates changed nothing: the record still carries what the
    // accepted create stored.
    let fetched = emulator
        .ok(
            "GetDeploymentConfig",
            json!({"deploymentConfigName": "cfg-taken"}),
        )
        .await;
    assert_eq!(
        fetched["deploymentConfigInfo"]["minimumHealthyHosts"]["value"].as_i64(),
        Some(4),
        "case cfg-taken: a refused duplicate must not overwrite the stored record, got {fetched:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: each optional member describes one side of the compute-platform split —
/// `minimumHealthyHosts` and `zonalConfig` an EC2/on-premises instance fleet,
/// `trafficRoutingConfig` a Lambda/ECS traffic shift — and a value is refused
/// both on the platform it does not describe and when it falls outside what that
/// member can mean, each with the modelled error for that member; nothing is
/// persisted by a refused request.
#[tokio::test]
async fn an_inapplicable_or_out_of_range_member_is_refused() {
    let emulator = Emulator::new();
    // Rows differ along which member is at fault, the compute platform, and the
    // kind of fault (inapplicable, missing half of a pair, mismatched increment,
    // out of range). The published bounds are taken from the member docs:
    // FLEET_PERCENT is a percentage (0–100), HOST_COUNT a count of instances
    // (0 or more), a traffic increment a share that leaves traffic on both sides
    // of the shift (1–99), and a monitor duration a period of seconds (0 or
    // more).
    let cases: &[(&str, Value, &str)] = &[
        // minimumHealthyHosts
        (
            "min-healthy-on-lambda",
            json!({"computePlatform": "Lambda", "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 1}}),
            "InvalidMinimumHealthyHostValueException",
        ),
        (
            "min-healthy-on-ecs",
            json!({"computePlatform": "ECS", "minimumHealthyHosts": {"type": "FLEET_PERCENT", "value": 50}}),
            "InvalidMinimumHealthyHostValueException",
        ),
        (
            "min-healthy-without-type",
            json!({"computePlatform": "Server", "minimumHealthyHosts": {"value": 5}}),
            "InvalidMinimumHealthyHostValueException",
        ),
        (
            "min-healthy-fleet-percent-over-100",
            json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "FLEET_PERCENT", "value": 101}}),
            "InvalidMinimumHealthyHostValueException",
        ),
        (
            "min-healthy-fleet-percent-negative",
            json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "FLEET_PERCENT", "value": -1}}),
            "InvalidMinimumHealthyHostValueException",
        ),
        (
            "min-healthy-host-count-negative",
            json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "HOST_COUNT", "value": -1}}),
            "InvalidMinimumHealthyHostValueException",
        ),
        // trafficRoutingConfig
        (
            "traffic-routing-on-server",
            json!({
                "computePlatform": "Server",
                "trafficRoutingConfig": {"type": "TimeBasedCanary", "timeBasedCanary": {"canaryPercentage": 10, "canaryInterval": 5}},
            }),
            "InvalidTrafficRoutingConfigurationException",
        ),
        (
            "traffic-routing-on-defaulted-platform",
            json!({"trafficRoutingConfig": {"type": "AllAtOnce"}}),
            "InvalidTrafficRoutingConfigurationException",
        ),
        (
            "traffic-routing-without-type",
            json!({"computePlatform": "Lambda", "trafficRoutingConfig": {"timeBasedCanary": {"canaryPercentage": 10, "canaryInterval": 5}}}),
            "InvalidTrafficRoutingConfigurationException",
        ),
        (
            "canary-without-its-increment",
            json!({"computePlatform": "Lambda", "trafficRoutingConfig": {"type": "TimeBasedCanary"}}),
            "InvalidTrafficRoutingConfigurationException",
        ),
        (
            "canary-with-a-linear-increment",
            json!({
                "computePlatform": "Lambda",
                "trafficRoutingConfig": {"type": "TimeBasedCanary", "timeBasedLinear": {"linearPercentage": 10, "linearInterval": 5}},
            }),
            "InvalidTrafficRoutingConfigurationException",
        ),
        (
            "linear-without-its-increment",
            json!({"computePlatform": "ECS", "trafficRoutingConfig": {"type": "TimeBasedLinear"}}),
            "InvalidTrafficRoutingConfigurationException",
        ),
        (
            "all-at-once-with-an-increment",
            json!({
                "computePlatform": "Lambda",
                "trafficRoutingConfig": {"type": "AllAtOnce", "timeBasedCanary": {"canaryPercentage": 10, "canaryInterval": 5}},
            }),
            "InvalidTrafficRoutingConfigurationException",
        ),
        (
            "canary-percentage-zero",
            json!({
                "computePlatform": "Lambda",
                "trafficRoutingConfig": {"type": "TimeBasedCanary", "timeBasedCanary": {"canaryPercentage": 0, "canaryInterval": 5}},
            }),
            "InvalidTrafficRoutingConfigurationException",
        ),
        (
            "canary-percentage-100",
            json!({
                "computePlatform": "Lambda",
                "trafficRoutingConfig": {"type": "TimeBasedCanary", "timeBasedCanary": {"canaryPercentage": 100, "canaryInterval": 5}},
            }),
            "InvalidTrafficRoutingConfigurationException",
        ),
        (
            "linear-interval-negative",
            json!({
                "computePlatform": "ECS",
                "trafficRoutingConfig": {"type": "TimeBasedLinear", "timeBasedLinear": {"linearPercentage": 10, "linearInterval": -1}},
            }),
            "InvalidTrafficRoutingConfigurationException",
        ),
        // zonalConfig
        (
            "zonal-on-lambda",
            json!({"computePlatform": "Lambda", "zonalConfig": {"monitorDurationInSeconds": 60}}),
            "InvalidZonalDeploymentConfigurationException",
        ),
        (
            "zonal-monitor-duration-negative",
            json!({"computePlatform": "Server", "zonalConfig": {"monitorDurationInSeconds": -1}}),
            "InvalidZonalDeploymentConfigurationException",
        ),
        (
            "zonal-first-zone-duration-negative",
            json!({"computePlatform": "Server", "zonalConfig": {"firstZoneMonitorDurationInSeconds": -60}}),
            "InvalidZonalDeploymentConfigurationException",
        ),
        (
            "zonal-per-zone-percent-over-100",
            json!({
                "computePlatform": "Server",
                "zonalConfig": {"minimumHealthyHostsPerZone": {"type": "FLEET_PERCENT", "value": 150}},
            }),
            "InvalidZonalDeploymentConfigurationException",
        ),
        (
            "zonal-per-zone-without-type",
            json!({"computePlatform": "Server", "zonalConfig": {"minimumHealthyHostsPerZone": {"value": 1}}}),
            "InvalidZonalDeploymentConfigurationException",
        ),
    ];

    for (name, extra, expected_code) in cases {
        let mut request = json!({"deploymentConfigName": name});
        for (member, value) in extra.as_object().expect("the case body is an object") {
            request[member] = value.clone();
        }

        let (status, body) = emulator.call("CreateDeploymentConfig", request).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {name:?}: expected {expected_code}, got {body:?}"
        );

        // A refused request persists nothing: the name is still unknown.
        let (get_status, get_body) = emulator
            .call("GetDeploymentConfig", json!({"deploymentConfigName": name}))
            .await;
        assert_eq!(
            get_status, 400,
            "case {name:?}: a refused create must persist nothing, got {get_status} {get_body:?}"
        );
        assert_eq!(
            error_code(&get_body),
            "DeploymentConfigDoesNotExistException",
            "case {name:?}: a refused create must persist nothing, got {get_body:?}"
        );
    }
}

/// RULE: each bound a member's meaning carries is INCLUSIVE — a value on the
/// bound is stored, one past it is refused. Here: `FLEET_PERCENT` 0 and 100, a
/// `HOST_COUNT` of 0, a traffic increment of 1% and 99%, and a zero monitor
/// duration.
#[tokio::test]
async fn a_value_on_a_published_bound_is_accepted() {
    let emulator = Emulator::new();
    let cases: &[(&str, Value)] = &[
        (
            "bound-fleet-percent-0",
            json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "FLEET_PERCENT", "value": 0}}),
        ),
        (
            "bound-fleet-percent-100",
            json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "FLEET_PERCENT", "value": 100}}),
        ),
        (
            "bound-host-count-0",
            json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 0}}),
        ),
        (
            "bound-canary-1-percent",
            json!({
                "computePlatform": "Lambda",
                "trafficRoutingConfig": {"type": "TimeBasedCanary", "timeBasedCanary": {"canaryPercentage": 1, "canaryInterval": 0}},
            }),
        ),
        (
            "bound-linear-99-percent",
            json!({
                "computePlatform": "ECS",
                "trafficRoutingConfig": {"type": "TimeBasedLinear", "timeBasedLinear": {"linearPercentage": 99, "linearInterval": 0}},
            }),
        ),
        (
            "bound-zonal-zero-durations",
            json!({
                "computePlatform": "Server",
                "zonalConfig": {
                    "firstZoneMonitorDurationInSeconds": 0,
                    "monitorDurationInSeconds": 0,
                    "minimumHealthyHostsPerZone": {"type": "FLEET_PERCENT", "value": 100},
                },
            }),
        ),
    ];

    for (name, extra) in cases {
        let mut request = json!({"deploymentConfigName": name});
        for (member, value) in extra.as_object().expect("the case body is an object") {
            request[member] = value.clone();
        }
        let (status, body) = emulator
            .call("CreateDeploymentConfig", request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a value on the bound must be accepted, got {status} {body:?}"
        );

        let fetched = emulator
            .ok("GetDeploymentConfig", json!({"deploymentConfigName": name}))
            .await;
        let info = &fetched["deploymentConfigInfo"];
        for member in [
            "minimumHealthyHosts",
            "trafficRoutingConfig",
            "zonalConfig",
        ] {
            assert_eq!(
                &info[member],
                &request[member],
                "case {name:?}: expected {member} to read back as it was created, got {info:?}"
            );
        }
    }
}

/// RULE: the modelled constraints on `deploymentConfigName` are refused with
/// CodeDeploy's own codes before the handler runs — an absent or empty name with
/// `DeploymentConfigNameRequiredException`, one outside the modelled pattern or
/// length with `InvalidDeploymentConfigNameException` — and an unmodelled
/// `computePlatform` with `InvalidComputePlatformException`.
#[tokio::test]
async fn a_name_or_platform_the_model_refuses_keeps_codedeploys_codes() {
    let emulator = Emulator::new();
    // Rows differ along which modelled constraint fails: required, length
    // minimum, length maximum (100, per the model's DeploymentConfigName),
    // pattern, and the computePlatform value set.
    let cases: &[(&str, Value, &str)] = &[
        (
            "name-absent",
            json!({"computePlatform": "Server"}),
            "DeploymentConfigNameRequiredException",
        ),
        (
            "name-empty",
            json!({"deploymentConfigName": ""}),
            "DeploymentConfigNameRequiredException",
        ),
        (
            "name-too-long",
            json!({"deploymentConfigName": "n".repeat(101)}),
            "InvalidDeploymentConfigNameException",
        ),
        (
            "name-outside-the-pattern",
            json!({"deploymentConfigName": "cfg with spaces"}),
            "InvalidDeploymentConfigNameException",
        ),
        (
            "platform-not-modelled",
            json!({"deploymentConfigName": "cfg-bad-platform", "computePlatform": "Mainframe"}),
            "InvalidComputePlatformException",
        ),
    ];

    for (name, request, expected_code) in cases {
        let (status, body) = emulator
            .call("CreateDeploymentConfig", request.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {name:?}: expected {expected_code}, got {body:?}"
        );
    }
}

/// RULE: a created deployment configuration is a configuration a deployment
/// group can name, so `CreateDeploymentGroup` accepts it where it refuses a name
/// no configuration carries.
#[tokio::test]
async fn a_created_config_is_usable_by_a_deployment_group() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateDeploymentConfig",
            json!({
                "deploymentConfigName": "cfg-for-group",
                "computePlatform": "Server",
                "minimumHealthyHosts": {"type": "FLEET_PERCENT", "value": 80},
            }),
        )
        .await;
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": "app-using-cfg", "computePlatform": "Server"}),
        )
        .await;

    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": "app-using-cfg",
                "deploymentGroupName": "dg-using-cfg",
                "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
                "deploymentConfigName": "cfg-for-group",
            }),
        )
        .await;
    let group = emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": "app-using-cfg", "deploymentGroupName": "dg-using-cfg"}),
        )
        .await;
    assert_eq!(
        group["deploymentGroupInfo"]["deploymentConfigName"].as_str(),
        Some("cfg-for-group"),
        "case dg-using-cfg: expected the created configuration on the group, got {group:?}"
    );

    let (status, body) = emulator
        .call(
            "CreateDeploymentGroup",
            json!({
                "applicationName": "app-using-cfg",
                "deploymentGroupName": "dg-unknown-cfg",
                "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
                "deploymentConfigName": "cfg-never-created",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case dg-unknown-cfg: expected HTTP 400 for a configuration that was never created, got \
         {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "DeploymentConfigDoesNotExistException",
        "case dg-unknown-cfg: expected DeploymentConfigDoesNotExistException, got {body:?}"
    );
}
