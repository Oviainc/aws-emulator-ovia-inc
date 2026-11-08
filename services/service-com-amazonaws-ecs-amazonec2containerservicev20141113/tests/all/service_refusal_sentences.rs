// SPDX-License-Identifier: CC0-1.0

//! The SENTENCES Amazon ECS fixes for the refusals a service create or update earns, and
//! the per-name report a describe answers for a cluster that was deleted — driven end to
//! end over the shared wire harness in [`crate::ecs_wire`], the same front door a client
//! uses.
//!
//! The rules under test:
//!
//! 1. Each of these refusals carries ONE sentence for its whole class: the text is the
//!    same for every request that meets the condition, and nothing the request carried —
//!    no service name, no cluster, no count, no network mode — reaches it. The message is
//!    the only machine-readable discriminator between the several
//!    `InvalidParameterException` causes one operation answers, so it is part of the
//!    contract and is asserted by equality, not by substring.
//!    * a `serviceName` the cluster already holds a service under is the failed idempotent
//!      CREATE it is (`clientToken`: "An identifier that you provide to ensure the
//!      idempotency of the request"), and the SAME name is free in another cluster;
//!    * a `desiredCount` below zero is refused in terms of the task count it is
//!      (`CreateService$desiredCount`/`UpdateService$desiredCount` are `BoxedInteger` with
//!      no `@range`, so the bound is the handler's);
//!    * a service whose scheduling strategy needs a `desiredCount` the request left out
//!      ("This is required if schedulingStrategy is REPLICA or isn't specified") states the
//!      member that is missing;
//!    * a `networkConfiguration` the task definition's `networkMode` gives nothing to
//!      configure points at that task definition rather than quoting the mode.
//! 2. A cluster record this account still HOLDS, in the status a delete leaves it in, is
//!    one `DescribeServices` resolves: the call answers, and each name it carries comes
//!    back as a `MISSING` failure — the services the cluster held went with the delete.
//!    `ClusterNotFoundException` ("Cluster not found.") is left to a reference that
//!    resolves to no cluster record at all.
//!
//! Every table spans the inputs its rule generalises over — the shape of the name, the
//! value of the count, the scheduling strategy, the network mode, the identifier form —
//! rather than the one request a defect was found on.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    answered, awsvpc_network_configuration, cluster_with_capacity, create_cluster, list, ok,
    refusal, register_definition, service, string, ACCOUNT, SIGNED_REGION,
};

/// The cluster the create of a service acts in.
const ALPHA: &str = "sentences-alpha";

/// A SECOND cluster, which the same service name is created in to show the collision is
/// scoped to one cluster rather than to the account.
const BETA: &str = "sentences-beta";

/// The sentence a create of a name the cluster already holds carries
/// (`state`'s `SERVICE_CREATION_NOT_IDEMPOTENT_MESSAGE`).
const NOT_IDEMPOTENT: &str = "Creation of service was not idempotent.";

/// The sentence a `desiredCount` below zero carries (`state`'s
/// `DESIRED_COUNT_NEGATIVE_MESSAGE`).
const TASK_COUNT_NEGATIVE: &str = "Task count can not be lesser than 0";

/// The sentence a service that needs a `desiredCount` and was given none carries
/// (`state`'s `DESIRED_COUNT_MISSING_MESSAGE`).
const DESIRED_COUNT_MISSING: &str = "DesiredCount is missing";

/// The sentence a `networkConfiguration` the revision's mode cannot take carries
/// (`state`'s `NETWORK_CONFIGURATION_NOT_VALID_MESSAGE`).
const NETWORK_CONFIGURATION_NOT_VALID: &str =
    "Network Configuration is not valid for the given networkMode of this task definition.";

/// The refusal class every row of this suite answers under.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// Register one revision of `family` whose `networkMode` is `mode`, and return its ARN.
async fn register_mode(
    router: &MultiServiceRouter,
    family: &str,
    mode: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": mode,
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": "web",
                "image": "public.ecr.aws/nginx/nginx:latest",
                "memory": 128,
            }],
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// Assert `message` is the fixed sentence for its class AND that none of `values` — the
/// things the request carried — reached it.
fn fixed_sentence(message: &str, expected: &str, values: &[&str], case: &str) {
    assert_eq!(
        message, expected,
        "case {case:?}: expected the fixed sentence {expected:?}, got {message:?}"
    );
    for value in values {
        assert!(
            !message.contains(value),
            "case {case:?}: the sentence is fixed text and must not carry {value:?}, got \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn a_service_name_the_cluster_holds_is_refused_as_a_create_that_was_not_idempotent() {
    let router = service();
    cluster_with_capacity(&router, ALPHA, "setup").await;
    cluster_with_capacity(&router, BETA, "setup").await;
    register_definition(&router, "sentencesfamily", "setup").await;
    register_definition(&router, "sentencesother", "setup").await;

    // Rows differ along the SHAPE of the name that is taken — `serviceName`: "Up to 255
    // letters (uppercase and lowercase), numbers, underscores, and hyphens are allowed" —
    // because the sentence is fixed text: no name may reach it, however it is spelled.
    let names = ["web", "Svc_2-b", &"s".repeat(255)];
    for name in names {
        let setup = format!("the first create of {}", &name[..name.len().min(16)]);
        let first = ok(
            &router,
            "CreateService",
            json!({
                "cluster": ALPHA,
                "serviceName": name,
                "taskDefinition": "sentencesfamily",
                "desiredCount": 0,
                "launchType": "EC2",
                "clientToken": "token-0001",
            }),
            &setup,
        )
        .await;
        let arn = string(&first["service"], "serviceArn", &setup);

        // Rows differ along how the SECOND create differs from the first: the token it
        // carries, the count it asks for and the revision it names. Every one of them is a
        // create of a name that is taken, so every one of them earns the same sentence.
        let repeats: [(&str, Value); 4] = [
            (
                "a repeat carrying no clientToken at all",
                json!({
                    "cluster": ALPHA,
                    "serviceName": name,
                    "taskDefinition": "sentencesfamily",
                    "desiredCount": 0,
                    "launchType": "EC2",
                }),
            ),
            (
                "the same request under a NEW clientToken",
                json!({
                    "cluster": ALPHA,
                    "serviceName": name,
                    "taskDefinition": "sentencesfamily",
                    "desiredCount": 0,
                    "launchType": "EC2",
                    "clientToken": "token-0002",
                }),
            ),
            (
                "a different count under a new clientToken",
                json!({
                    "cluster": ALPHA,
                    "serviceName": name,
                    "taskDefinition": "sentencesfamily",
                    "desiredCount": 3,
                    "launchType": "EC2",
                    "clientToken": "token-0003",
                }),
            ),
            (
                "another task definition under a new clientToken",
                json!({
                    "cluster": ALPHA,
                    "serviceName": name,
                    "taskDefinition": "sentencesother",
                    "desiredCount": 0,
                    "launchType": "EC2",
                    "clientToken": "token-0004",
                }),
            ),
        ];
        for (variant, request) in repeats {
            let case = format!("{variant}, name {}", &name[..name.len().min(16)]);
            let (code, message) = refusal(&router, "CreateService", request, &case).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected a {INVALID_PARAMETER}, got {code:?} with \
                 {message:?}"
            );
            fixed_sentence(&message, NOT_IDEMPOTENT, &[name, ALPHA], &case);
        }

        // The create under the SAME token is the same request, so it is answered with the
        // service the first one created rather than refused: the refusal above is what a
        // DIFFERENT request for a taken name earns.
        let case = format!("the same request under the same token, name {}", &name[..name.len().min(16)]);
        let repeated = ok(
            &router,
            "CreateService",
            json!({
                "cluster": ALPHA,
                "serviceName": name,
                "taskDefinition": "sentencesfamily",
                "desiredCount": 0,
                "launchType": "EC2",
                "clientToken": "token-0001",
            }),
            &case,
        )
        .await;
        assert_eq!(
            string(&repeated["service"], "serviceArn", &case),
            arn,
            "case {case:?}: expected the stored service back, got {repeated}"
        );

        // And the same name in ANOTHER cluster is a different service: the collision is
        // scoped to the cluster, which is why the sentence names neither.
        let case = format!("the same name in a second cluster, name {}", &name[..name.len().min(16)]);
        let elsewhere = ok(
            &router,
            "CreateService",
            json!({
                "cluster": BETA,
                "serviceName": name,
                "taskDefinition": "sentencesfamily",
                "desiredCount": 0,
                "launchType": "EC2",
            }),
            &case,
        )
        .await;
        let other_arn = string(&elsewhere["service"], "serviceArn", &case);
        assert_ne!(
            other_arn, arn,
            "case {case:?}: expected a service of its own in the second cluster, got \
             {elsewhere}"
        );

        // Read it back through its own describe: the acceptance is persisted state.
        let read = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": BETA, "services": [name] }),
            &case,
        )
        .await;
        let described = list(&read, "services", &case);
        assert_eq!(
            described.len(),
            1,
            "case {case:?}: expected the second cluster's service to read back, got {read}"
        );
        assert_eq!(
            string(&described[0], "serviceArn", &case),
            other_arn,
            "case {case:?}: expected the stored service, got {read}"
        );
    }
}

#[tokio::test]
async fn a_task_count_below_zero_is_refused_in_terms_of_the_task_count() {
    let router = service();
    cluster_with_capacity(&router, ALPHA, "setup").await;
    register_definition(&router, "countsfamily", "setup").await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": ALPHA,
            "serviceName": "counted",
            "taskDefinition": "countsfamily",
            "desiredCount": 0,
            "launchType": "EC2",
        }),
        "setup",
    )
    .await;

    // Rows differ along HOW FAR below the bound the count is, the smallest `BoxedInteger`
    // included: the sentence states the bound, never the value, so every row earns the
    // same text.
    for count in [-1, -2, -1000, i32::MIN] {
        for operation in ["CreateService", "UpdateService"] {
            let case = format!("{operation} with desiredCount {count}");
            let request = if operation == "CreateService" {
                json!({
                    "cluster": ALPHA,
                    "serviceName": "negative",
                    "taskDefinition": "countsfamily",
                    "desiredCount": count,
                    "launchType": "EC2",
                })
            } else {
                json!({ "cluster": ALPHA, "service": "counted", "desiredCount": count })
            };
            let (code, message) = refusal(&router, operation, request, &case).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected a {INVALID_PARAMETER}, got {code:?} with \
                 {message:?}"
            );
            fixed_sentence(
                &message,
                TASK_COUNT_NEGATIVE,
                &[&count.to_string(), "desiredCount"],
                &case,
            );
        }
    }

    // The bound itself is accepted on both calls — a service that keeps no task running is
    // a service — so the rule refuses what is below zero and nothing else.
    let case = "the bound itself";
    ok(
        &router,
        "UpdateService",
        json!({ "cluster": ALPHA, "service": "counted", "desiredCount": 0 }),
        case,
    )
    .await;
    let created = ok(
        &router,
        "CreateService",
        json!({
            "cluster": ALPHA,
            "serviceName": "zero",
            "taskDefinition": "countsfamily",
            "desiredCount": 0,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    assert_eq!(
        string(&created["service"], "serviceName", case),
        "zero",
        "case {case:?}: expected a service wanting no tasks to be created, got {created}"
    );

    // And the refused create stored nothing.
    let read = ok(
        &router,
        "DescribeServices",
        json!({ "cluster": ALPHA, "services": ["negative"] }),
        case,
    )
    .await;
    assert!(
        list(&read, "services", case).is_empty(),
        "case {case:?}: a refused create must store no service, got {read}"
    );
}

#[tokio::test]
async fn a_service_that_needs_a_task_count_and_was_given_none_names_the_missing_member() {
    let router = service();
    cluster_with_capacity(&router, ALPHA, "setup").await;
    register_definition(&router, "strategyfamily", "setup").await;

    // Every value of the closed `SchedulingStrategy` enum (`smithy_query(query="shape",
    // shape="SchedulingStrategy")`: REPLICA, DAEMON) plus the strategy a request that
    // names none takes — the member's doc: "This is required if schedulingStrategy is
    // REPLICA or isn't specified. If schedulingStrategy is DAEMON then this isn't
    // required."
    let rows: [(&str, Option<&str>, bool); 3] = [
        ("no schedulingStrategy at all", None, true),
        ("REPLICA", Some("REPLICA"), true),
        ("DAEMON", Some("DAEMON"), false),
    ];
    for (case, strategy, required) in rows {
        let name = format!("strategy{}", strategy.unwrap_or("default"));
        let mut request = json!({
            "cluster": ALPHA,
            "serviceName": name,
            "taskDefinition": "strategyfamily",
            "launchType": "EC2",
        });
        if let Some(strategy) = strategy {
            request["schedulingStrategy"] = json!(strategy);
        }

        if !required {
            let created = ok(&router, "CreateService", request, case).await;
            assert_eq!(
                string(&created["service"], "schedulingStrategy", case),
                "DAEMON",
                "case {case:?}: expected the daemon to be created without a count, got \
                 {created}"
            );
            continue;
        }

        let (code, message) = refusal(&router, "CreateService", request, case).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected a {INVALID_PARAMETER}, got {code:?} with {message:?}"
        );
        fixed_sentence(
            &message,
            DESIRED_COUNT_MISSING,
            &[&name, ALPHA, "REPLICA"],
            case,
        );
    }
}

#[tokio::test]
async fn a_network_configuration_the_revisions_mode_cannot_take_points_at_the_revision() {
    let router = service();
    cluster_with_capacity(&router, ALPHA, "setup").await;

    // Every value of the closed `NetworkMode` enum (`smithy_query(query="shape",
    // shape="NetworkMode")`: bridge, host, awsvpc, none) that gives a task no interface of
    // its own to configure. `awsvpc` is the one mode that takes a configuration, and its
    // own refusal — for a launch that carries NONE — is the awsvpc sentence asserted in
    // `crate::service_validation`.
    for mode in ["bridge", "host", "none"] {
        let family = format!("modefamily{mode}");
        register_mode(&router, &family, mode, "setup").await;
        // A service of that revision WITHOUT a configuration, so the update below changes
        // nothing but the configuration it offers.
        let name = format!("mode{mode}");
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": ALPHA,
                "serviceName": name,
                "taskDefinition": family,
                "desiredCount": 0,
                "launchType": "EC2",
            }),
            "setup",
        )
        .await;

        // Both the create and the update of a service hold the same pair to the same rule.
        let rows: [(&str, Value); 2] = [
            (
                "CreateService",
                json!({
                    "cluster": ALPHA,
                    "serviceName": format!("{name}-configured"),
                    "taskDefinition": family,
                    "desiredCount": 0,
                    "launchType": "EC2",
                    "networkConfiguration": awsvpc_network_configuration(),
                }),
            ),
            (
                "UpdateService",
                json!({
                    "cluster": ALPHA,
                    "service": name,
                    "networkConfiguration": awsvpc_network_configuration(),
                }),
            ),
        ];
        for (operation, request) in rows {
            let case = format!("{operation} with a networkConfiguration for {mode}");
            let (code, message) = refusal(&router, operation, request, &case).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected a {INVALID_PARAMETER}, got {code:?} with \
                 {message:?}"
            );
            fixed_sentence(
                &message,
                NETWORK_CONFIGURATION_NOT_VALID,
                &[mode, &family, &name],
                &case,
            );
        }
    }
}

#[tokio::test]
async fn describe_services_resolves_a_deleted_cluster_and_reports_its_names_missing() {
    let router = service();
    let arn = create_cluster(&router, ALPHA, "setup").await;
    register_definition(&router, "tombstonefamily", "setup").await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": ALPHA,
            "serviceName": "web",
            "taskDefinition": "tombstonefamily",
            "desiredCount": 0,
            "launchType": "EC2",
        }),
        "setup",
    )
    .await;
    ok(
        &router,
        "DeleteService",
        json!({ "cluster": ALPHA, "service": "web" }),
        "setup",
    )
    .await;
    ok(&router, "DeleteCluster", json!({ "cluster": ALPHA }), "setup").await;

    // The cluster is still discoverable in the status the delete left it in, which is the
    // state that selects the answers below.
    let described = ok(
        &router,
        "DescribeClusters",
        json!({ "clusters": [ALPHA] }),
        "setup",
    )
    .await;
    assert_eq!(
        string(&list(&described, "clusters", "setup")[0], "status", "setup"),
        "INACTIVE",
        "case \"setup\": expected the deleted cluster to describe as INACTIVE, got \
         {described}"
    );

    // Rows differ along the cluster's STATE and the documented form of the reference —
    // `cluster`: "The short name or full Amazon Resource Name (ARN) the cluster" — and
    // along how many names the request asks about, since each name gets its own answer.
    let absent = format!("{ALPHA}-never-created");
    let absent_arn = format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{absent}");
    let held: [(&str, String); 2] = [
        ("the tombstone by name", ALPHA.to_string()),
        ("the tombstone by ARN", arn.clone()),
    ];
    for (case, reference) in held {
        for names in [vec!["web"], vec!["web", "never-created"]] {
            let case = format!("{case}, {} name(s)", names.len());
            let (status, body) = answered(
                &router,
                "DescribeServices",
                json!({ "cluster": reference, "services": names.clone() }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the held cluster record to be resolved, got \
                 {status} with {body}"
            );
            assert!(
                list(&body, "services", &case).is_empty(),
                "case {case:?}: the delete took the cluster's services with it, got {body}"
            );
            let failures = list(&body, "failures", &case);
            assert_eq!(
                failures.len(),
                names.len(),
                "case {case:?}: expected one failure per name asked about, got {body}"
            );
            for (failure, name) in failures.iter().zip(&names) {
                assert_eq!(
                    string(failure, "reason", &case),
                    "MISSING",
                    "case {case:?}: expected a MISSING failure, got {body}"
                );
                assert!(
                    string(failure, "arn", &case).ends_with(&format!("/{name}")),
                    "case {case:?}: expected the failure to name {name:?}, got {body}"
                );
            }
        }
    }

    // A reference that resolves to NO cluster record leaves the describe nothing to answer
    // about, which is the condition the exception reports — in both identifier forms.
    for reference in [absent.clone(), absent_arn] {
        let case = format!("a cluster this account never held, as {reference:?}");
        let (code, message) = refusal(
            &router,
            "DescribeServices",
            json!({ "cluster": reference, "services": ["web"] }),
            &case,
        )
        .await;
        assert_eq!(
            code, "ClusterNotFoundException",
            "case {case:?}: expected a ClusterNotFoundException, got {code:?} with \
             {message:?}"
        );
        fixed_sentence(&message, "Cluster not found.", &[&absent], &case);
    }
}
