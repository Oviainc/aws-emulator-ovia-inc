// SPDX-License-Identifier: CC0-1.0

//! The request classes Amazon ECS ANSWERS, and the refusals that must not stand in their
//! way.
//!
//! Four rules, each asserted over a table that spans its class rather than over one
//! request:
//!
//! * a request that names a resource of another service — a capacity provider, a Cloud Map
//!   registry — is resolved against THAT service's records, so a create naming one nothing
//!   holds is refused and leaves no service behind, and the next create of the same name
//!   succeeds;
//! * a cluster whose services have all been deleted deletes, because every service it ever
//!   held is one a create actually stored;
//! * a structure member the model gives a `@default` is satisfied by that default when the
//!   request omits it, so a partial structure is accepted and reports the default, and an
//!   update applies the members it supplies over the ones the service already holds;
//! * `ListTasks` answers the tasks that match the `desiredStatus` filter — "The default
//!   status filter is RUNNING" — whichever of the enum's values a request names and however
//!   it spells the cluster.
//!
//! Every case drives the emulator through the shared wire harness in [`crate::ecs_wire`],
//! the same front door a client uses, so nothing reaches into a state struct and a response
//! that is not computed from persisted state cannot pass. The Cloud Map suite mounts the
//! service-discovery service beside this one on ONE engine, the way the production router
//! mounts them, and creates its namespace through that service's own API.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    answered, awsvpc_network_configuration, cloud_map, cluster_with_capacity,
    create_auto_scaling_group, emulator_with_cloud_map, list, ok, refusal, service, string,
    ACCOUNT, SIGNED_REGION,
};

/// The modelled error every bad-parameter refusal below carries.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The task-definition family the suites register.
const FAMILY: &str = "acceptedfamily";

/// The image every container starts from; irrelevant to every rule here, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// Register one Fargate-capable `awsvpc` revision of [`FAMILY`] (task-level `cpu` and
/// `memory`), so a row may place it under either launch type.
async fn register_fargate_capable(router: &MultiServiceRouter, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [crate::task_lifecycle::container("web", IMAGE)],
        }),
        case,
    )
    .await;
}

/// The ARN a capacity provider of `name` carries in this account and Region.
fn capacity_provider_arn(name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:capacity-provider/{name}")
}

/// The `CreateService` request body every placement row sends, with `extra`'s members
/// merged over it.
fn create_service_request(cluster: &str, name: &str, extra: Value) -> Value {
    let mut request = json!({
        "cluster": cluster,
        "serviceName": name,
        "taskDefinition": FAMILY,
        "desiredCount": 0,
        "networkConfiguration": awsvpc_network_configuration(),
    });
    let Value::Object(members) = extra else {
        panic!("a request overlay is an object");
    };
    for (member, value) in members {
        request[member] = value;
    }
    request
}

// ---------------------------------------------------------------------------
// A capacity provider this account holds none of
// ---------------------------------------------------------------------------

/// One row of the capacity-provider table: the strategy a request names, and the whole
/// sentence the refusal must carry — `None` for a strategy that resolves.
struct StrategyRow {
    case: &'static str,
    strategy: Value,
    refused: Option<String>,
}

/// The sentence a strategy naming a provider the ACCOUNT holds none of earns, for the
/// reference the request spelled.
fn not_found(reference: &str) -> String {
    format!(
        "The specified capacity provider {reference} was not found. Specify a valid capacity \
         provider and try again."
    )
}

/// The sentence a strategy naming a provider the account holds but the CLUSTER does not
/// earns: a provider places tasks in a cluster only once it is associated with it
/// (`PutClusterCapacityProviders`), so the sentence is about the pair and names neither.
fn not_associated() -> String {
    "The specified capacity provider strategy cannot contain a capacity provider that is \
     not associated with the cluster. Associate the capacity provider with the cluster or \
     specify a valid capacity provider and try again."
        .to_string()
}

#[tokio::test]
async fn a_capacity_provider_the_account_holds_none_of_is_refused_and_stores_no_service() {
    let router = service();
    let cluster = "providerscope";
    cluster_with_capacity(&router, cluster, "setup").await;
    register_fargate_capable(&router, "setup").await;
    for name in ["real-provider", "unassociated-provider"] {
        let group = format!("asg-{name}");
        create_auto_scaling_group(&router, &group, "setup").await;
        ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": name,
                "autoScalingGroupProvider": { "autoScalingGroupArn": group },
            }),
            "setup",
        )
        .await;
    }
    // A placement uses the providers its CLUSTER holds, so the ones these rows place on are
    // associated with it through the operation that associates one; `unassociated-provider`
    // is deliberately left out of the list.
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": cluster,
            "capacityProviders": ["FARGATE", "FARGATE_SPOT", "real-provider"],
            "defaultCapacityProviderStrategy": [],
        }),
        "setup",
    )
    .await;
    // The rows differ along how the strategy spells the provider (a short name, an ARN),
    // how many entries it carries, and whether the provider is one nothing created, one
    // Amazon ECS publishes for every account (`FARGATE`, `FARGATE_SPOT`), one this account
    // created and associated with the cluster, or one it created and did not.
    let rows = [
        StrategyRow {
            case: "a name nothing was created under",
            strategy: json!([{ "capacityProvider": "never-created", "weight": 1 }]),
            refused: Some(not_found("never-created")),
        },
        StrategyRow {
            case: "an unknown name beside a created one",
            strategy: json!([
                { "capacityProvider": "real-provider", "weight": 1 },
                { "capacityProvider": "also-never-created", "weight": 1 },
            ]),
            refused: Some(not_found("also-never-created")),
        },
        StrategyRow {
            case: "an ARN of a provider nothing holds",
            strategy: json!([
                { "capacityProvider": capacity_provider_arn("arn-never-created"), "weight": 1 },
            ]),
            refused: Some(not_found(&capacity_provider_arn("arn-never-created"))),
        },
        StrategyRow {
            case: "a name that only looks like a managed provider",
            strategy: json!([{ "capacityProvider": "FARGATE_SPOTS", "weight": 1 }]),
            refused: Some(not_found("FARGATE_SPOTS")),
        },
        StrategyRow {
            case: "a provider this account created but the cluster does not hold",
            strategy: json!([{ "capacityProvider": "unassociated-provider", "weight": 1 }]),
            refused: Some(not_associated()),
        },
        StrategyRow {
            case: "an associated provider beside one the cluster does not hold",
            strategy: json!([
                { "capacityProvider": "real-provider", "weight": 1 },
                { "capacityProvider": "unassociated-provider", "weight": 1 },
            ]),
            refused: Some(not_associated()),
        },
        StrategyRow {
            case: "the managed Fargate providers",
            strategy: json!([
                { "capacityProvider": "FARGATE", "weight": 1 },
                { "capacityProvider": "FARGATE_SPOT", "weight": 1 },
            ]),
            refused: None,
        },
        StrategyRow {
            case: "a provider this account created, by ARN",
            strategy: json!([
                { "capacityProvider": capacity_provider_arn("real-provider"), "weight": 1 },
            ]),
            refused: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("providersvc{index}");
        let (status, body) = answered(
            &router,
            "CreateService",
            create_service_request(
                cluster,
                &name,
                json!({ "capacityProviderStrategy": row.strategy.clone() }),
            ),
        )
        .await;
        match &row.refused {
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the create to be refused, got {status} with {body}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(expected.as_str()),
                    "case {case:?}: expected the refusal to read {expected:?}, got {body}"
                );
                // A refused create stores nothing: the name is still free, which is what
                // lets the client correct the request and try again.
                let described = ok(
                    &router,
                    "DescribeServices",
                    json!({ "cluster": cluster, "services": [name.clone()] }),
                    case,
                )
                .await;
                assert!(
                    list(&described, "services", case).is_empty(),
                    "case {case:?}: a refused create leaves no service, got {described}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be answered, got {status} with {body}"
            ),
        }
    }

    // The whole point of refusing: the create the client CORRECTS is served, under the
    // name the refused attempt asked for.
    let case = "the corrected create of the refused name";
    let corrected = ok(
        &router,
        "CreateService",
        create_service_request(
            cluster,
            "providersvc0",
            json!({ "capacityProviderStrategy": [{ "capacityProvider": "FARGATE", "weight": 1 }] }),
        ),
        case,
    )
    .await;
    assert_eq!(
        string(&corrected["service"], "serviceName", case),
        "providersvc0",
        "case {case:?}: expected the corrected create to store the service, got {corrected}"
    );
}

// ---------------------------------------------------------------------------
// A cluster whose services have all been deleted
// ---------------------------------------------------------------------------

/// One row of the refused-create table: the members a create carries beyond the valid
/// ones, and the modelled error it earns.
struct RefusedCreate {
    case: &'static str,
    overlay: Value,
    code: &'static str,
    message: &'static str,
}

#[tokio::test]
async fn a_cluster_deletes_once_the_services_it_holds_are_deleted() {
    let router = service();
    let cluster = "teardownscope";
    cluster_with_capacity(&router, cluster, "setup").await;
    register_fargate_capable(&router, "setup").await;

    // Every row is a create Amazon ECS refuses, spanning the classes of cross-field rule a
    // create is held to: a percentage below the floor its member admits, a bake time past
    // the range it admits, a task definition on a controller that takes it from a task set,
    // a public IP on infrastructure that cannot attach one, and a Cloud Map registry
    // nothing holds. None of them may leave a service in the cluster.
    let rows = [
        RefusedCreate {
            case: "maximumPercent below the floor",
            overlay: json!({ "deploymentConfiguration": { "maximumPercent": 50 } }),
            code: INVALID_PARAMETER,
            message: "maximumPercent must be at least 100",
        },
        RefusedCreate {
            case: "a bake time past a day",
            overlay: json!({ "deploymentConfiguration": { "bakeTimeInMinutes": 1441 } }),
            code: INVALID_PARAMETER,
            message: "The bake time must be between 0 and 1440 for ROLLING deployment strategy.",
        },
        RefusedCreate {
            case: "a negative bake time",
            overlay: json!({
                "deploymentConfiguration": { "bakeTimeInMinutes": -1, "strategy": "ROLLING" },
            }),
            code: INVALID_PARAMETER,
            message: "The bake time must be between 0 and 1440 for ROLLING deployment strategy.",
        },
        RefusedCreate {
            case: "a task definition under the EXTERNAL controller",
            overlay: json!({ "deploymentController": { "type": "EXTERNAL" } }),
            code: INVALID_PARAMETER,
            message: "TaskDefinition must be blank.",
        },
        RefusedCreate {
            case: "a public IP on the EC2 launch type",
            overlay: json!({
                "launchType": "EC2",
                "networkConfiguration": {
                    "awsvpcConfiguration": {
                        "subnets": ["subnet-0123456789abcdef0"],
                        "assignPublicIp": "ENABLED",
                    },
                },
            }),
            code: INVALID_PARAMETER,
            message: "Assign public IP is not supported for this launch type.",
        },
        RefusedCreate {
            case: "a Cloud Map registry nothing holds",
            overlay: json!({
                "serviceRegistries": [{
                    "registryArn": format!(
                        "arn:aws:servicediscovery:{SIGNED_REGION}:{ACCOUNT}:service/srv-missing"
                    ),
                }],
            }),
            code: "ClientException",
            message: "The service registry could not be validated. Route 53 error: \
                      \"ServiceNotFound.\"",
        },
    ];

    for row in &rows {
        let case = row.case;
        let (code, message) = refusal(
            &router,
            "CreateService",
            create_service_request(cluster, "refusedsvc", row.overlay.clone()),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (row.code, row.message),
            "case {case:?}: expected {} {:?}, got {code} {message:?}",
            row.code,
            row.message
        );
    }

    // The two services the client DID create: one it scales to zero and deletes, one it
    // deletes outright with `force` ("allows you to delete a service even if it wasn't
    // scaled down to zero tasks").
    for (name, desired) in [("drained", 0), ("forced", 1)] {
        ok(
            &router,
            "CreateService",
            create_service_request(
                cluster,
                name,
                json!({ "desiredCount": desired, "launchType": "FARGATE" }),
            ),
            "setup",
        )
        .await;
    }
    let case = "a service scaled to zero";
    let deleted = ok(
        &router,
        "DeleteService",
        json!({ "cluster": cluster, "service": "drained" }),
        case,
    )
    .await;
    assert_eq!(
        string(&deleted["service"], "serviceName", case),
        "drained",
        "case {case:?}: expected the deleted service back, got {deleted}"
    );
    assert_ne!(
        string(&deleted["service"], "status", case),
        "ACTIVE",
        "case {case:?}: expected the delete to move the service off ACTIVE, got {deleted}"
    );
    let case = "a service deleted with force";
    let deleted = ok(
        &router,
        "DeleteService",
        json!({ "cluster": cluster, "service": "forced", "force": true }),
        case,
    )
    .await;
    assert_ne!(
        string(&deleted["service"], "status", case),
        "ACTIVE",
        "case {case:?}: expected the forced delete to move the service off ACTIVE, got \
         {deleted}"
    );

    // Nothing is left holding the cluster open, so the teardown completes.
    let case = "the cluster after its services were deleted";
    let body = ok(
        &router,
        "DeleteCluster",
        json!({ "cluster": cluster }),
        case,
    )
    .await;
    assert_eq!(
        string(&body["cluster"], "status", case),
        "INACTIVE",
        "case {case:?}: expected the deleted cluster to report its tombstone, got {body}"
    );
}

// ---------------------------------------------------------------------------
// A structure the request carries only part of
// ---------------------------------------------------------------------------

/// One row of the partial-structure table: the `deploymentConfiguration` a request carries,
/// and the member of it every read must report, with the value the MODEL's default gives
/// it.
struct PartialStructureRow {
    case: &'static str,
    configuration: Value,
    /// The path inside the reported `deploymentConfiguration`, and the value it must hold.
    reported: Vec<(&'static str, Value)>,
}

#[tokio::test]
async fn a_structure_the_request_carries_only_part_of_takes_the_modelled_defaults() {
    let router = service();
    let cluster = "partialscope";
    cluster_with_capacity(&router, cluster, "setup").await;
    register_fargate_capable(&router, "setup").await;

    // The rows differ along WHICH defaulted member the request leaves out, and in which of
    // the structures a deployment configuration is built from: the circuit breaker's two
    // `@default: false` members, the alarms' two, the breaker's threshold `value`
    // (`@default: 0`) and the early-success criteria's `enable`. Member and default source:
    // `smithy_query(query="shape", shape="DeploymentCircuitBreaker,DeploymentAlarms,
    // ThresholdConfiguration,DeploymentEarlySuccessCriteria")`.
    let rows = [
        PartialStructureRow {
            case: "a circuit breaker without rollback",
            configuration: json!({ "deploymentCircuitBreaker": { "enable": true } }),
            reported: vec![
                ("deploymentCircuitBreaker/enable", json!(true)),
                ("deploymentCircuitBreaker/rollback", json!(false)),
            ],
        },
        PartialStructureRow {
            case: "a circuit breaker without enable",
            configuration: json!({ "deploymentCircuitBreaker": { "rollback": true } }),
            reported: vec![
                ("deploymentCircuitBreaker/enable", json!(false)),
                ("deploymentCircuitBreaker/rollback", json!(true)),
            ],
        },
        PartialStructureRow {
            case: "a circuit breaker with neither member",
            configuration: json!({ "deploymentCircuitBreaker": {} }),
            reported: vec![
                ("deploymentCircuitBreaker/enable", json!(false)),
                ("deploymentCircuitBreaker/rollback", json!(false)),
            ],
        },
        PartialStructureRow {
            case: "a threshold configuration without its value",
            configuration: json!({
                "deploymentCircuitBreaker": {
                    "enable": true,
                    "rollback": true,
                    "thresholdConfiguration": { "type": "COUNT" },
                },
            }),
            reported: vec![
                (
                    "deploymentCircuitBreaker/thresholdConfiguration/type",
                    json!("COUNT"),
                ),
                (
                    "deploymentCircuitBreaker/thresholdConfiguration/value",
                    json!(0),
                ),
            ],
        },
        PartialStructureRow {
            case: "alarms without rollback",
            configuration: json!({
                "alarms": { "alarmNames": ["unhealthy"], "enable": true },
            }),
            reported: vec![
                ("alarms/enable", json!(true)),
                ("alarms/rollback", json!(false)),
                ("alarms/alarmNames", json!(["unhealthy"])),
            ],
        },
        PartialStructureRow {
            case: "alarms with neither flag",
            configuration: json!({ "alarms": { "alarmNames": [] } }),
            reported: vec![
                ("alarms/enable", json!(false)),
                ("alarms/rollback", json!(false)),
            ],
        },
        PartialStructureRow {
            case: "early success criteria without enable",
            configuration: json!({ "earlySuccessCriteria": { "healthyPercent": 50 } }),
            reported: vec![
                ("earlySuccessCriteria/enable", json!(false)),
                ("earlySuccessCriteria/healthyPercent", json!(50)),
            ],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("partialsvc{index}");
        let created = ok(
            &router,
            "CreateService",
            create_service_request(
                cluster,
                &name,
                json!({
                    "launchType": "FARGATE",
                    "deploymentConfiguration": row.configuration.clone(),
                }),
            ),
            case,
        )
        .await;
        // The create's own answer and a later read are two projections of ONE stored
        // configuration, so both report the structure with its defaults filled in.
        let read = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": cluster, "services": [name.clone()] }),
            case,
        )
        .await;
        let described = list(&read, "services", case)
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {read}"));
        for (source, view) in [
            ("CreateService", &created["service"]),
            ("DescribeServices", &described),
        ] {
            let configuration = &view["deploymentConfiguration"];
            for (path, expected) in &row.reported {
                let mut found = configuration.clone();
                for member in path.split('/') {
                    found = found[member].clone();
                }
                assert_eq!(
                    &found, expected,
                    "case {case:?}: {source} expected {path} to read {expected}, got \
                     {configuration}"
                );
            }
        }
    }

    // The other half of the rule: it is the DEFAULTS this relaxes, so a member the model
    // gives none is still the modelled validation failure when the request omits it. The
    // rows differ along which structure the undefaulted member sits in and whether the
    // request carried that structure's OTHER members.
    let refused: [(&str, Value, &str); 4] = [
        (
            "a threshold configuration without its type",
            json!({
                "deploymentCircuitBreaker": {
                    "enable": true,
                    "thresholdConfiguration": { "value": 10 },
                },
            }),
            "deploymentConfiguration.deploymentCircuitBreaker.thresholdConfiguration.type",
        ),
        (
            "a threshold configuration with neither member",
            json!({ "deploymentCircuitBreaker": { "thresholdConfiguration": {} } }),
            "deploymentConfiguration.deploymentCircuitBreaker.thresholdConfiguration.type",
        ),
        (
            "alarms without the names they watch",
            json!({ "alarms": { "enable": true, "rollback": true } }),
            "deploymentConfiguration.alarms.alarmNames",
        ),
        (
            "alarms with no member at all",
            json!({ "alarms": {} }),
            "deploymentConfiguration.alarms.alarmNames",
        ),
    ];

    for (case, configuration, path) in refused {
        let (code, message) = refusal(
            &router,
            "CreateService",
            create_service_request(
                cluster,
                "refusedpartial",
                json!({
                    "launchType": "FARGATE",
                    "deploymentConfiguration": configuration,
                }),
            ),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                INVALID_PARAMETER,
                format!(
                    "1 validation error detected: Value null at '{path}' failed to satisfy \
                     constraint: Member must not be null"
                )
                .as_str()
            ),
            "case {case:?}: expected the modelled refusal for the absent {path}, got {code} \
             {message:?}"
        );
        // A refused create stores nothing, so the name it asked for is still free.
        let described = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": cluster, "services": ["refusedpartial"] }),
            case,
        )
        .await;
        assert!(
            list(&described, "services", case).is_empty(),
            "case {case:?}: a refused create leaves no service, got {described}"
        );
    }
}

/// One row of the merge table: the `deploymentConfiguration` an update supplies, and the
/// paths inside the reported structure that must hold the values beside them afterwards.
struct MergeRow {
    case: &'static str,
    configuration: Value,
    reported: Vec<(&'static str, Value)>,
}

#[tokio::test]
async fn an_update_applies_the_deployment_members_it_supplies_over_the_stored_ones() {
    let router = service();
    let cluster = "mergescope";
    cluster_with_capacity(&router, cluster, "setup").await;
    register_fargate_capable(&router, "setup").await;
    let case = "setup";
    ok(
        &router,
        "CreateService",
        create_service_request(
            cluster,
            "merged",
            json!({
                "launchType": "FARGATE",
                "deploymentConfiguration": {
                    "maximumPercent": 150,
                    "minimumHealthyPercent": 50,
                    "deploymentCircuitBreaker": { "enable": false, "rollback": false },
                },
            }),
        ),
        case,
    )
    .await;

    // Every row updates ONE member of the structure and leaves the rest out; after it, the
    // member it named has the new value and every other member still has the one it had.
    let rows: [MergeRow; 4] = [
        MergeRow {
            case: "only the circuit breaker",
            configuration: json!({ "deploymentCircuitBreaker": { "enable": true } }),
            reported: vec![
                ("deploymentCircuitBreaker/enable", json!(true)),
                ("deploymentCircuitBreaker/rollback", json!(false)),
                ("maximumPercent", json!(150)),
                ("minimumHealthyPercent", json!(50)),
            ],
        },
        MergeRow {
            case: "only the maximum percent",
            configuration: json!({ "maximumPercent": 200 }),
            reported: vec![
                ("maximumPercent", json!(200)),
                ("minimumHealthyPercent", json!(50)),
                ("deploymentCircuitBreaker/enable", json!(true)),
            ],
        },
        MergeRow {
            case: "only the alarms",
            configuration: json!({
                "alarms": { "alarmNames": ["five-hundreds"], "enable": true },
            }),
            reported: vec![
                ("alarms/alarmNames", json!(["five-hundreds"])),
                ("alarms/rollback", json!(false)),
                ("maximumPercent", json!(200)),
                ("minimumHealthyPercent", json!(50)),
            ],
        },
        MergeRow {
            case: "an empty structure",
            configuration: json!({}),
            reported: vec![
                ("maximumPercent", json!(200)),
                ("minimumHealthyPercent", json!(50)),
                ("alarms/enable", json!(true)),
                ("deploymentCircuitBreaker/enable", json!(true)),
            ],
        },
    ];

    for row in rows {
        let (case, configuration, expected) = (row.case, row.configuration, row.reported);
        let updated = ok(
            &router,
            "UpdateService",
            json!({
                "cluster": cluster,
                "service": "merged",
                "deploymentConfiguration": configuration,
            }),
            case,
        )
        .await;
        let read = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": cluster, "services": ["merged"] }),
            case,
        )
        .await;
        let described = list(&read, "services", case)
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {read}"));
        for (source, view) in [
            ("UpdateService", &updated["service"]),
            ("DescribeServices", &described),
        ] {
            let configuration = &view["deploymentConfiguration"];
            for (path, want) in &expected {
                let mut found = configuration.clone();
                for member in path.split('/') {
                    found = found[member].clone();
                }
                assert_eq!(
                    &found, want,
                    "case {case:?}: {source} expected {path} to read {want}, got \
                     {configuration}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_update_is_held_to_the_public_ip_rule_the_create_is() {
    let router = service();
    let cluster = "updatepublicip";
    cluster_with_capacity(&router, cluster, "setup").await;
    register_fargate_capable(&router, "setup").await;
    // One service per placement, so the rule is shown to be decided by WHERE the tasks run
    // and not by the request that sets the member.
    for (name, launch_type) in [("onec2", "EC2"), ("onfargate", "FARGATE")] {
        ok(
            &router,
            "CreateService",
            create_service_request(cluster, name, json!({ "launchType": launch_type })),
            "setup",
        )
        .await;
    }

    let enabled = json!({
        "awsvpcConfiguration": {
            "subnets": ["subnet-0123456789abcdef0"],
            "assignPublicIp": "ENABLED",
        },
    });
    for (case, name, refused) in [
        ("an update of an EC2-placed service", "onec2", true),
        ("an update of a Fargate-placed service", "onfargate", false),
    ] {
        let (status, body) = answered(
            &router,
            "UpdateService",
            json!({
                "cluster": cluster,
                "service": name,
                "networkConfiguration": enabled.clone(),
            }),
        )
        .await;
        if refused {
            assert_eq!(
                (status, body["message"].as_str()),
                (
                    400,
                    Some("Assign public IP is not supported for this launch type.")
                ),
                "case {case:?}: expected the public IP to be refused off Fargate, got \
                 {status} with {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be answered, got {status} with {body}"
            );
            assert_eq!(
                body["service"]["networkConfiguration"]["awsvpcConfiguration"]
                    ["assignPublicIp"]
                    .as_str(),
                Some("ENABLED"),
                "case {case:?}: expected the stored configuration to carry the public IP, \
                 got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The tasks a `desiredStatus` filter selects
// ---------------------------------------------------------------------------
#[tokio::test]
async fn listing_tasks_answers_the_arns_the_desired_status_filter_selects() {
    let router = service();
    let cluster = "taskfilter";
    let cluster_arn = cluster_with_capacity(&router, cluster, "setup").await;
    register_fargate_capable(&router, "setup").await;

    let case = "setup";
    let run = ok(
        &router,
        "RunTask",
        json!({
            "cluster": cluster,
            "taskDefinition": FAMILY,
            "launchType": "FARGATE",
            "count": 2,
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        case,
    )
    .await;
    let mut started: Vec<String> = list(&run, "tasks", case)
        .iter()
        .map(|task| string(task, "taskArn", case))
        .collect();
    started.sort();
    assert_eq!(
        started.len(),
        2,
        "case {case:?}: expected both tasks to be placed, got {run}"
    );
    let stopped_arn = started[0].clone();
    let running_arn = started[1].clone();
    ok(
        &router,
        "StopTask",
        json!({ "cluster": cluster, "task": stopped_arn.clone(), "reason": "test" }),
        case,
    )
    .await;

    // The rows span the `DesiredStatus` enum (`smithy_query(query="shape",
    // shape="DesiredStatus")`: RUNNING, PENDING, STOPPED), the absent filter the member's
    // doc gives a default for ("The default status filter is RUNNING"), and both forms of
    // the `cluster` reference — "The short name or full Amazon Resource Name (ARN) of the
    // cluster".
    let rows: [(&str, Value, Vec<String>); 8] = [
        (
            "no filter at all, by cluster name",
            json!({ "cluster": cluster }),
            vec![running_arn.clone()],
        ),
        (
            "no filter at all, by cluster ARN",
            json!({ "cluster": cluster_arn.clone() }),
            vec![running_arn.clone()],
        ),
        (
            "RUNNING named explicitly",
            json!({ "cluster": cluster, "desiredStatus": "RUNNING" }),
            vec![running_arn.clone()],
        ),
        (
            "STOPPED, by cluster name",
            json!({ "cluster": cluster, "desiredStatus": "STOPPED" }),
            vec![stopped_arn.clone()],
        ),
        (
            "STOPPED, by cluster ARN",
            json!({ "cluster": cluster_arn.clone(), "desiredStatus": "STOPPED" }),
            vec![stopped_arn.clone()],
        ),
        (
            "STOPPED, beside the family filter",
            json!({ "cluster": cluster, "desiredStatus": "STOPPED", "family": FAMILY }),
            vec![stopped_arn.clone()],
        ),
        (
            "STOPPED, beside the launch type the tasks ran under",
            json!({
                "cluster": cluster,
                "desiredStatus": "STOPPED",
                "launchType": "FARGATE",
            }),
            vec![stopped_arn.clone()],
        ),
        (
            "PENDING, which no task was asked for",
            json!({ "cluster": cluster, "desiredStatus": "PENDING" }),
            Vec::new(),
        ),
    ];

    for (case, request, expected) in rows {
        let (status, body) = answered(&router, "ListTasks", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the listing to be answered, got {status} with {body}"
        );
        let mut arns: Vec<String> = list(&body, "taskArns", case)
            .iter()
            .map(|arn| {
                arn.as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a task ARN, got {arn}"))
                    .to_string()
            })
            .collect();
        arns.sort();
        assert_eq!(
            arns, expected,
            "case {case:?}: expected the listing to report {expected:?}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// A Cloud Map namespace with no ECS service in it
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cloud_map_namespace_with_no_ecs_service_in_it_lists_an_empty_set() {
    let router = emulator_with_cloud_map();
    let case = "setup";
    cluster_with_capacity(&router, "namespacescope", case).await;
    register_fargate_capable(&router, case).await;

    // The namespace is Cloud Map's resource, so it is created through Cloud Map's own API.
    cloud_map(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": "empty-namespace" }),
        case,
    )
    .await;
    let listed = cloud_map(&router, "ListNamespaces", json!({}), case).await;
    let namespace = list(&listed, "Namespaces", case)
        .into_iter()
        .find(|namespace| namespace["Name"].as_str() == Some("empty-namespace"))
        .unwrap_or_else(|| panic!("case {case:?}: expected the namespace back, got {listed}"));
    let namespace_id = string(&namespace, "Id", case);
    // A second namespace, so "the namespace with nothing in it" is a class and not one
    // name, and so the listing has to SELECT rather than answer empty for everything.
    cloud_map(
        &router,
        "CreateHttpNamespace",
        json!({ "Name": "other.namespace-2" }),
        case,
    )
    .await;

    // Every row names a namespace Cloud Map holds, or one nothing holds: a namespace Cloud
    // Map holds is one this listing walks, however few ECS services have joined it, and a
    // namespace nothing holds is the modelled refusal. (`namespace` is matched against the
    // member's own `@pattern`, which admits no `/`, so a namespace ARN is not one of the
    // forms a request may spell here — the id inside one is.)
    let rows: [(&str, Value, Option<Vec<String>>); 5] = [
        (
            "the namespace by name, with nothing in it",
            json!({ "namespace": "empty-namespace" }),
            Some(Vec::new()),
        ),
        (
            "the second namespace by name, with nothing in it",
            json!({ "namespace": "other.namespace-2" }),
            Some(Vec::new()),
        ),
        (
            "the namespace by its Cloud Map id",
            json!({ "namespace": namespace_id.clone() }),
            Some(Vec::new()),
        ),
        (
            "a namespace nothing holds",
            json!({ "namespace": "never-created-namespace" }),
            None,
        ),
        (
            "a namespace id nothing minted",
            json!({ "namespace": "ns-000000000000never" }),
            None,
        ),
    ];

    for (case, request, expected) in &rows {
        let (status, body) = answered(&router, "ListServicesByNamespace", request.clone()).await;
        match expected {
            Some(arns) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the listing to be answered, got {status} with \
                     {body}"
                );
                let reported: Vec<String> = list(&body, "serviceArns", case)
                    .iter()
                    .map(|arn| arn.as_str().unwrap_or_default().to_string())
                    .collect();
                assert_eq!(
                    &reported, arns,
                    "case {case:?}: expected the listing to report {arns:?}, got {body}"
                );
                assert!(
                    body["serviceArns"].is_array(),
                    "case {case:?}: an empty result is still a list on the wire, got {body}"
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a namespace nothing holds to be refused, got \
                     {status} with {body}"
                );
                assert!(
                    body["__type"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("NamespaceNotFoundException"),
                    "case {case:?}: expected the modelled namespace refusal, got {body}"
                );
            }
        }
    }

    // And once a service joins the namespace, the same listing reports it: the namespace
    // selects, the cluster does not.
    let case = "the namespace once a service joined it";
    let created = ok(
        &router,
        "CreateService",
        create_service_request(
            "namespacescope",
            "joined",
            json!({
                "launchType": "FARGATE",
                "serviceConnectConfiguration": {
                    "enabled": true,
                    "namespace": "empty-namespace",
                },
            }),
        ),
        case,
    )
    .await;
    let joined_arn = string(&created["service"], "serviceArn", case);
    let body = ok(
        &router,
        "ListServicesByNamespace",
        json!({ "namespace": "empty-namespace" }),
        case,
    )
    .await;
    let reported: Vec<String> = list(&body, "serviceArns", case)
        .iter()
        .map(|arn| arn.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported,
        vec![joined_arn],
        "case {case:?}: expected the namespace's one service back, got {body}"
    );
}
