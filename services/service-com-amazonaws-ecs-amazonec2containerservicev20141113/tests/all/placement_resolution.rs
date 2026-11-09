// SPDX-License-Identifier: CC0-1.0

//! What a PLACEMENT is resolved against before it is made, driven end to end over the
//! shared wire harness in [`crate::ecs_wire`].
//!
//! Three rules, each asserted over a table that spans its class rather than over one
//! request:
//!
//! * the `awsvpcConfiguration` a placement carries is resolved in Amazon VPC — the
//!   interface an `awsvpc` task is allocated is created IN one of the subnets it names
//!   (`AwsVpcConfiguration$subnets`: "The IDs of the subnets associated with the task or
//!   service"), so a placement naming a subnet the account holds none of is refused with
//!   the lookup failure Amazon VPC answered, and one naming NO subnet is refused for the
//!   empty list it is;
//! * a revision is placed on the infrastructure it was validated against
//!   (`TaskDefinition$compatibilities`: "Amazon ECS validates the task definition
//!   parameters with those supported by the launch type"), whichever way the placement
//!   named that infrastructure — a `launchType`, the capacity providers of its strategy, or
//!   the cluster's default strategy, which is what a placement naming neither runs on;
//! * a capacity provider places tasks in a cluster only once it is ASSOCIATED with that
//!   cluster (`PutClusterCapacityProviders` takes "the capacity providers to associate with
//!   the cluster", and `Cluster$capacityProviders` is the list it leaves behind), so a
//!   strategy naming one outside that list is refused — including the providers Amazon ECS
//!   publishes for every account, which a cluster holds the same way as any other.
//!
//! Every refusal below also has to leave the store as it was: a placement Amazon ECS
//! refuses runs nothing and stores nothing, which is what lets a client correct the request
//! and send it again under the same name.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    answered, cluster_with_capacity, list, ok, refusal, register_capacity, service, string,
    AWSVPC_SUBNET,
};
use crate::task_lifecycle::container;

/// The cluster every placement below is made in.
const CLUSTER: &str = "placementscope";

/// The `awsvpc` family, Fargate-capable (task-level size), so one revision serves both the
/// subnet rows and the Fargate placements.
const FAMILY: &str = "placementfamily";

/// A `bridge` family: a revision with no interface of its own, and one Fargate was never
/// validated for.
const BRIDGE_FAMILY: &str = "placementbridgefamily";

/// The one error code every refusal below carries.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The image every container starts from; irrelevant to every rule here, so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// A `networkConfiguration` naming exactly `subnets`.
fn configuration(subnets: &[&str]) -> Value {
    json!({ "awsvpcConfiguration": { "subnets": subnets } })
}

/// A cluster with capacity, one `awsvpc` Fargate-capable revision and one `bridge`
/// revision.
async fn fixture() -> MultiServiceRouter {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "fixture").await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [container("web", IMAGE)],
        }),
        "fixture",
    )
    .await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": BRIDGE_FAMILY,
            "networkMode": "bridge",
            "containerDefinitions": [container("web", IMAGE)],
        }),
        "fixture",
    )
    .await;
    router
}

/// The sentence a placement naming a subnet Amazon VPC holds none of earns: the subnets it
/// asked about, then Amazon EC2's own sentence for the first id that resolved to nothing.
fn lookup_failure(requested: &[&str], missing: &str) -> String {
    format!(
        "Error retrieving subnet information for [{}]: The subnet ID '{missing}' does not \
         exist (ErrorCode: InvalidSubnetID.NotFound)",
        requested.join(", ")
    )
}

/// Whether the cluster holds a service under `name`.
async fn stored(router: &MultiServiceRouter, name: &str, case: &str) -> bool {
    let described = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [name] }),
        case,
    )
    .await;
    !list(&described, "services", case).is_empty()
}

#[tokio::test]
async fn a_placement_is_refused_unless_amazon_vpc_holds_every_subnet_it_names() {
    let router = fixture().await;
    // A second subnet of the account, so a row can name two and so a row can name one held
    // beside one that is not.
    let second_held = "subnet-0123456789abcdef1";

    // The rows differ along how many subnets the placement names, which of them the account
    // holds, the POSITION of the one it does not, and the form of the id that resolves to
    // nothing (the 17-digit and the 8-digit forms a subnet id takes, and a value of neither
    // length).
    let rows: [(&str, Vec<&str>, Option<&str>); 7] = [
        ("one subnet the account holds", vec![AWSVPC_SUBNET], None),
        (
            "two subnets the account holds",
            vec![AWSVPC_SUBNET, second_held],
            None,
        ),
        (
            "one 17-digit id the account holds none of",
            vec!["subnet-0fedcba987654321f"],
            Some("subnet-0fedcba987654321f"),
        ),
        (
            "one 8-digit id the account holds none of",
            vec!["subnet-1a2b3c4d"],
            Some("subnet-1a2b3c4d"),
        ),
        (
            "an id of neither length",
            vec!["subnet-nothing"],
            Some("subnet-nothing"),
        ),
        (
            "a held subnet beside one the account holds none of",
            vec![AWSVPC_SUBNET, "subnet-0999999999999999a"],
            Some("subnet-0999999999999999a"),
        ),
        (
            "one the account holds none of beside a held subnet",
            vec!["subnet-0888888888888888b", AWSVPC_SUBNET],
            Some("subnet-0888888888888888b"),
        ),
    ];

    for (index, (case, subnets, missing)) in rows.iter().enumerate() {
        let name = format!("subnetsvc{index}");
        // The same pair of subnets is sent to a RUN and to a CREATE: both place a task of
        // the revision, so both resolve the configuration they were given.
        let (run_status, run_body) = answered(
            &router,
            "RunTask",
            json!({
                "cluster": CLUSTER,
                "taskDefinition": FAMILY,
                "launchType": "EC2",
                "networkConfiguration": configuration(subnets),
            }),
        )
        .await;
        let (create_status, create_body) = answered(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": name.clone(),
                "taskDefinition": FAMILY,
                "desiredCount": 0,
                "launchType": "EC2",
                "networkConfiguration": configuration(subnets),
            }),
        )
        .await;

        match missing {
            Some(missing) => {
                let expected = lookup_failure(subnets, missing);
                for (operation, status, body) in [
                    ("RunTask", run_status, &run_body),
                    ("CreateService", create_status, &create_body),
                ] {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected {operation} to be refused, got {status} \
                         with {body}"
                    );
                    assert_eq!(
                        (
                            body["__type"].as_str().unwrap_or_default(),
                            body["message"].as_str().unwrap_or_default()
                        ),
                        (INVALID_PARAMETER, expected.as_str()),
                        "case {case:?}: expected {operation} to answer the lookup failure, \
                         got {body}"
                    );
                }
                assert!(
                    list(&run_body, "tasks", case).is_empty(),
                    "case {case:?}: a refused run places no task, got {run_body}"
                );
                assert!(
                    !stored(&router, &name, case).await,
                    "case {case:?}: a refused create stores no service"
                );
            }
            None => {
                assert_eq!(
                    run_status, 200,
                    "case {case:?}: expected the run to be answered, got {run_status} with \
                     {run_body}"
                );
                assert_eq!(
                    create_status, 200,
                    "case {case:?}: expected the create to be answered, got \
                     {create_status} with {create_body}"
                );
                assert_eq!(
                    string(&create_body["service"], "serviceName", case),
                    name,
                    "case {case:?}: expected the service stored, got {create_body}"
                );
                assert!(
                    stored(&router, &name, case).await,
                    "case {case:?}: an answered create reads back through DescribeServices"
                );
            }
        }
    }

    // The create of a name a refused attempt asked for is served once its configuration
    // names a subnet the account holds: the refusal stored nothing.
    let case = "the corrected create of a refused name";
    let corrected = ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "subnetsvc2",
            "taskDefinition": FAMILY,
            "desiredCount": 0,
            "launchType": "EC2",
            "networkConfiguration": configuration(&[AWSVPC_SUBNET]),
        }),
        case,
    )
    .await;
    assert_eq!(
        string(&corrected["service"], "serviceName", case),
        "subnetsvc2",
        "case {case:?}: expected the corrected create to store the service, got {corrected}"
    );
}

#[tokio::test]
async fn a_network_configuration_is_held_to_the_revisions_mode_and_to_naming_a_subnet() {
    let router = fixture().await;

    // The rows differ along what is wrong with the pair (the revision's network mode, the
    // configuration): no configuration for a mode that needs one, a configuration for a
    // mode that has no interface of its own, and a configuration that names no subnet at
    // all — on a listing of launches and on a create.
    let rows: [(&str, &str, Option<Value>, &str); 4] = [
        (
            "an awsvpc revision with no configuration",
            FAMILY,
            None,
            "Network Configuration must be provided when networkMode 'awsvpc' is specified.",
        ),
        (
            "a bridge revision with a configuration",
            BRIDGE_FAMILY,
            Some(configuration(&[AWSVPC_SUBNET])),
            "Network Configuration is not valid for the given networkMode of this task \
             definition.",
        ),
        (
            "an awsvpc revision whose configuration names no subnet",
            FAMILY,
            Some(configuration(&[])),
            "subnets can not be empty.",
        ),
        (
            "a bridge revision whose configuration names no subnet",
            BRIDGE_FAMILY,
            Some(configuration(&[])),
            "Network Configuration is not valid for the given networkMode of this task \
             definition.",
        ),
    ];

    for (index, (case, family, network_configuration, expected)) in rows.iter().enumerate() {
        let name = format!("configsvc{index}");
        let mut run = json!({
            "cluster": CLUSTER,
            "taskDefinition": family,
            "launchType": "EC2",
        });
        let mut create = json!({
            "cluster": CLUSTER,
            "serviceName": name.clone(),
            "taskDefinition": family,
            "desiredCount": 0,
            "launchType": "EC2",
        });
        if let Some(configuration) = network_configuration {
            run["networkConfiguration"] = configuration.clone();
            create["networkConfiguration"] = configuration.clone();
        }
        for (operation, request) in [("RunTask", run), ("CreateService", create)] {
            let (code, message) = refusal(&router, operation, request, case).await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (INVALID_PARAMETER, *expected),
                "case {case:?}: expected {operation} to answer {expected:?}, got {code:?} \
                 with {message:?}"
            );
        }
        assert!(
            !stored(&router, &name, case).await,
            "case {case:?}: a refused create stores no service"
        );
    }
}

#[tokio::test]
async fn a_revision_is_placed_only_on_infrastructure_it_was_validated_against() {
    let router = fixture().await;
    // Both clusters hold the Fargate providers, so a row is refused for the REVISION rather
    // than for a provider the cluster does not hold; one of them also makes a Fargate
    // provider its default strategy, which is what a placement naming neither runs on.
    let defaulted = "placementdefaulted";
    cluster_with_capacity(&router, defaulted, "fixture").await;
    for (cluster, default_strategy) in [
        (CLUSTER, json!([])),
        (
            defaulted,
            json!([{ "capacityProvider": "FARGATE", "weight": 1 }]),
        ),
    ] {
        ok(
            &router,
            "PutClusterCapacityProviders",
            json!({
                "cluster": cluster,
                "capacityProviders": ["FARGATE", "FARGATE_SPOT"],
                "defaultCapacityProviderStrategy": default_strategy,
            }),
            "fixture",
        )
        .await;
    }

    // The rows differ along HOW the placement names Fargate — the launch type, each of the
    // two Fargate capacity providers, and the cluster's own default strategy — and along
    // whether the revision it places was validated against Fargate.
    let rows: [(&str, &str, &str, Value, bool); 7] = [
        (
            "the Fargate launch type on a Fargate-capable revision",
            CLUSTER,
            FAMILY,
            json!({ "launchType": "FARGATE" }),
            true,
        ),
        (
            "the Fargate launch type on a bridge revision",
            CLUSTER,
            BRIDGE_FAMILY,
            json!({ "launchType": "FARGATE" }),
            false,
        ),
        (
            "the FARGATE provider on a Fargate-capable revision",
            CLUSTER,
            FAMILY,
            json!({ "capacityProviderStrategy": [{ "capacityProvider": "FARGATE", "weight": 1 }] }),
            true,
        ),
        (
            "the FARGATE provider on a bridge revision",
            CLUSTER,
            BRIDGE_FAMILY,
            json!({ "capacityProviderStrategy": [{ "capacityProvider": "FARGATE", "weight": 1 }] }),
            false,
        ),
        (
            "the FARGATE_SPOT provider on a bridge revision",
            CLUSTER,
            BRIDGE_FAMILY,
            json!({
                "capacityProviderStrategy": [
                    { "capacityProvider": "FARGATE_SPOT", "weight": 1 },
                ],
            }),
            false,
        ),
        (
            "the cluster's default Fargate strategy on a bridge revision",
            defaulted,
            BRIDGE_FAMILY,
            json!({}),
            false,
        ),
        (
            // A launch type the request NAMES is where the tasks run, so the cluster's
            // Fargate default decides nothing for it.
            "the EC2 launch type in that same cluster, on a bridge revision",
            defaulted,
            BRIDGE_FAMILY,
            json!({ "launchType": "EC2" }),
            true,
        ),
    ];

    for (index, (case, cluster, family, placement, served)) in rows.iter().enumerate() {
        let name = format!("launchsvc{index}");
        let mut create = json!({
            "cluster": cluster,
            "serviceName": name.clone(),
            "taskDefinition": family,
            "desiredCount": 0,
        });
        if *family == FAMILY {
            create["networkConfiguration"] = configuration(&[AWSVPC_SUBNET]);
        }
        for (member, value) in placement.as_object().expect("a placement is an object") {
            create[member] = value.clone();
        }
        let (status, body) = answered(&router, "CreateService", create).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be answered, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status} with {body}"
        );
        assert_eq!(
            (
                body["__type"].as_str().unwrap_or_default(),
                body["message"].as_str().unwrap_or_default()
            ),
            (
                INVALID_PARAMETER,
                "Task definition does not support launch_type FARGATE."
            ),
            "case {case:?}: expected the refusal to name the launch type, got {body}"
        );
    }
}

#[tokio::test]
async fn a_run_is_refused_a_capacity_provider_its_cluster_does_not_hold() {
    let router = fixture().await;
    // A cluster holding NOTHING but its registered capacity, and one holding the Fargate
    // providers: the same strategy is refused in the first and served in the second.
    let holding = "placementholding";
    cluster_with_capacity(&router, holding, "fixture").await;
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": holding,
            "capacityProviders": ["FARGATE"],
            "defaultCapacityProviderStrategy": [],
        }),
        "fixture",
    )
    .await;
    register_capacity(&router, CLUSTER, "fixture").await;

    let case = "a cluster that does not hold the provider";
    let (code, message) = refusal(
        &router,
        "RunTask",
        json!({
            "cluster": CLUSTER,
            "taskDefinition": FAMILY,
            "capacityProviderStrategy": [{ "capacityProvider": "FARGATE", "weight": 1 }],
            "networkConfiguration": configuration(&[AWSVPC_SUBNET]),
        }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            INVALID_PARAMETER,
            "The specified capacity provider strategy cannot contain a capacity provider \
             that is not associated with the cluster. Associate the capacity provider with \
             the cluster or specify a valid capacity provider and try again."
        ),
        "case {case:?}: expected the association sentence, got {code:?} with {message:?}"
    );
    let listed = ok(
        &router,
        "ListTasks",
        json!({ "cluster": CLUSTER }),
        case,
    )
    .await;
    assert!(
        list(&listed, "taskArns", case).is_empty(),
        "case {case:?}: a refused run places no task, got {listed}"
    );

    let case = "the same strategy in a cluster that holds it";
    let run = ok(
        &router,
        "RunTask",
        json!({
            "cluster": holding,
            "taskDefinition": FAMILY,
            "capacityProviderStrategy": [{ "capacityProvider": "FARGATE", "weight": 1 }],
            "networkConfiguration": configuration(&[AWSVPC_SUBNET]),
        }),
        case,
    )
    .await;
    let tasks = list(&run, "tasks", case);
    assert_eq!(
        tasks.len(),
        1,
        "case {case:?}: expected the run to place one task, got {run}"
    );
    assert_eq!(
        string(&tasks[0], "capacityProviderName", case),
        "FARGATE",
        "case {case:?}: expected the task to name the provider it was placed through, got \
         {run}"
    );
}
