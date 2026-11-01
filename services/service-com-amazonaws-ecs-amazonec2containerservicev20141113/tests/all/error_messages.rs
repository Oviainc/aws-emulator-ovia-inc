// SPDX-License-Identifier: CC0-1.0

//! What a refused ECS request SAYS, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rules under test:
//!
//! * A refusal whose condition is that a CLUSTER, SERVICE, TASK or TASK DEFINITION the
//!   request names does not exist carries ONE fixed sentence for that condition — the
//!   same text for every request that meets it, naming neither the resource nor any
//!   other value the request carried. Each table below therefore varies the reference
//!   (short name, name with digits, the longest name the member accepts, a full ARN) and
//!   the operation it is sent to, and asserts the SAME sentence comes back every time.
//!   The container-instance refusal is NOT one of these: ECS builds it from the request,
//!   which [`crate::container_instance_not_found`] pins.
//! * A refusal the service derives from the request names the ONE member the caller has
//!   to change, and the first such member when more than one is wrong: the task-level
//!   members Fargate requires (`cpu` before `memory`), and the members of an EC2
//!   instance identity document (`accountId` before `instanceId`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, create_cluster, identity_document,
    identity_document_signature, ok, refusal, register_capacity, service, string, ACCOUNT,
    SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// The cluster, task-definition family and service every row points at, so a refusal is
/// reached by naming a MISSING resource rather than by a malformed request.
const CLUSTER: &str = "demo";
const FAMILY: &str = "probe-family";
const SERVICE: &str = "probe-service";

/// A second family whose ONLY revision is deregistered, so the family resolves to no
/// ACTIVE revision — the same condition as a family nothing was ever registered under
/// ("If a revision isn't specified, the latest ACTIVE revision is used").
const RETIRED_FAMILY: &str = "retired-family";

/// The fixed sentence for a `cluster` that names no cluster (recorded ECS behaviour:
/// every `ClusterNotFoundException` in the corpus carries this text).
const CLUSTER_NOT_FOUND: &str = "Cluster not found.";
/// The fixed sentence for a `service` that names no service.
const SERVICE_NOT_FOUND: &str = "Service not found.";
/// The fixed sentence for a `task` that names no task.
const TASK_NOT_FOUND: &str = "The referenced task was not found";
/// The fixed sentence a READING operation answers for a `taskDefinition` that names no
/// stored revision.
const TASK_DEFINITION_NOT_DESCRIBABLE: &str = "Unable to describe task definition.";
/// The fixed sentence an operation that would PUT a revision to work answers for the same
/// condition: the sentence belongs to the call that was refused, not to the store it
/// looked in.
const TASK_DEFINITION_NOT_FOUND: &str = "TaskDefinition not found.";

/// One row of an operation table: the operation's name and the request a row builds for
/// it from the reference it is given.
type OperationRow = (&'static str, fn(&str) -> Value);

/// One row of the task-definition table: an [`OperationRow`] plus the sentence THAT
/// operation answers for a reference naming no stored revision.
type DefinitionRow = (&'static str, fn(&str) -> Value, &'static str);

/// A router holding one cluster, one task-definition revision and one service.
async fn fixture() -> MultiServiceRouter {
    let router = service();
    create_cluster(&router, CLUSTER, "fixture").await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
        }),
        "fixture",
    )
    .await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": SERVICE,
            "taskDefinition": FAMILY,
            "desiredCount": 1,
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        "fixture",
    )
    .await;
    // The retired family: registered, then deregistered, so it is readable but holds no
    // revision any call may put to work.
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": RETIRED_FAMILY,
            "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
        }),
        "fixture",
    )
    .await;
    ok(
        &router,
        "DeregisterTaskDefinition",
        json!({ "taskDefinition": format!("{RETIRED_FAMILY}:1") }),
        "fixture",
    )
    .await;
    router
}

/// Assert one refusal reports `code` with exactly `message`, and that the sentence
/// carries nothing from the request.
fn assert_refusal(
    answered: (String, String),
    expected_code: &str,
    expected_message: &str,
    reference: &str,
    case: &str,
) {
    let (code, message) = answered;
    assert_eq!(
        code, expected_code,
        "case {case:?}: expected a {expected_code}, got {code:?} with {message:?}"
    );
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected {expected_message:?}, got {message:?}"
    );
    assert!(
        !message.contains(reference),
        "case {case:?}: the message must not carry the request's reference \
         {reference:?}, got {message:?}"
    );
}

/// The forms a reference to a missing resource takes: the case name and the reference.
/// Rows differ in length, character set and whether the reference is a bare name or a
/// full ARN — everything the sentence must be independent of.
fn missing_references(kind: &str, path: &str) -> Vec<(String, String)> {
    vec![
        ("short name".to_string(), format!("ghost-{kind}")),
        (
            "name with digits and underscores".to_string(),
            format!("ghost_{kind}_42"),
        ),
        ("longest name".to_string(), "g".repeat(255)),
        (
            "full ARN".to_string(),
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:{path}/ghost-by-arn"),
        ),
    ]
}

#[tokio::test]
async fn a_cluster_that_does_not_exist_is_refused_with_one_fixed_sentence() {
    let router = fixture().await;

    // Every implemented operation that resolves a `cluster` and reports a cluster it
    // cannot resolve under the cluster-specific error, each paired with a request that is
    // well formed apart from the cluster it names. The operations that report the same
    // condition under a class of their own — the agent plane's `ClientException`,
    // `RegisterContainerInstance` among them — carry the same SENTENCE and are covered by
    // their own suites.
    let requests: [OperationRow; 11] = [
        ("RunTask", |cluster| {
            json!({ "cluster": cluster, "taskDefinition": FAMILY })
        }),
        ("StopTask", |cluster| {
            json!({ "cluster": cluster, "task": "a".repeat(32) })
        }),
        ("ListTasks", |cluster| json!({ "cluster": cluster })),
        ("DescribeTasks", |cluster| {
            json!({ "cluster": cluster, "tasks": ["a".repeat(32)] })
        }),
        ("CreateService", |cluster| {
            json!({
                "cluster": cluster,
                "serviceName": "ghost-service",
                "taskDefinition": FAMILY,
                "desiredCount": 1,
            })
        }),
        ("UpdateService", |cluster| {
            json!({ "cluster": cluster, "service": "ghost-service" })
        }),
        ("DescribeServices", |cluster| {
            json!({ "cluster": cluster, "services": ["ghost-service"] })
        }),
        ("DeleteCluster", |cluster| json!({ "cluster": cluster })),
        ("ListContainerInstances", |cluster| {
            json!({ "cluster": cluster })
        }),
        ("DescribeContainerInstances", |cluster| {
            json!({ "cluster": cluster, "containerInstances": ["a".repeat(32)] })
        }),
        ("DeleteAttributes", |cluster| {
            json!({
                "cluster": cluster,
                "attributes": [{
                    "name": "stack",
                    "targetId": "a".repeat(32),
                    "targetType": "container-instance",
                }],
            })
        }),
    ];

    for (operation, build) in requests {
        for (form, reference) in missing_references("cluster", "cluster") {
            let case = format!("{operation} with a {form} cluster");
            let answered = refusal(&router, operation, build(&reference), &case).await;
            assert_refusal(
                answered,
                "ClusterNotFoundException",
                CLUSTER_NOT_FOUND,
                &reference,
                &case,
            );
        }
    }
}

#[tokio::test]
async fn a_service_that_does_not_exist_is_refused_with_one_fixed_sentence() {
    let router = fixture().await;

    for (form, reference) in missing_references("service", "service/demo") {
        // `UpdateService` takes a bare name or a service ARN (the ARN names the cluster
        // itself); `ListTasks` filters by the service name.
        let case = format!("UpdateService with a {form} service");
        let answered = refusal(
            &router,
            "UpdateService",
            json!({ "cluster": CLUSTER, "service": reference, "desiredCount": 1 }),
            &case,
        )
        .await;
        assert_refusal(
            answered,
            "ServiceNotFoundException",
            SERVICE_NOT_FOUND,
            &reference,
            &case,
        );

        if reference.starts_with("arn:") {
            continue;
        }
        let case = format!("ListTasks with a {form} service");
        let answered = refusal(
            &router,
            "ListTasks",
            json!({ "cluster": CLUSTER, "serviceName": reference }),
            &case,
        )
        .await;
        assert_refusal(
            answered,
            "ServiceNotFoundException",
            SERVICE_NOT_FOUND,
            &reference,
            &case,
        );
    }
}

#[tokio::test]
async fn a_task_that_does_not_exist_is_refused_with_one_fixed_sentence() {
    let router = fixture().await;

    // A task reference is an id or a full task ARN; the id forms differ in alphabet and
    // length, and none of them appears in the sentence.
    let references = [
        ("32-character id", "b".repeat(32)),
        (
            "uuid-shaped id",
            "00000000-0000-0000-0000-00000000beef".to_string(),
        ),
        (
            "full ARN",
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/{CLUSTER}/{}",
                "c".repeat(32)
            ),
        ),
    ];
    for (form, reference) in references {
        let case = format!("StopTask with a {form} task");
        let answered = refusal(
            &router,
            "StopTask",
            json!({ "cluster": CLUSTER, "task": reference }),
            &case,
        )
        .await;
        assert_refusal(
            answered,
            "InvalidParameterException",
            TASK_NOT_FOUND,
            &reference,
            &case,
        );
    }
}

#[tokio::test]
async fn a_task_definition_that_does_not_exist_is_refused_with_one_fixed_sentence() {
    let router = fixture().await;

    // A task-definition reference is a family, a `family:revision` or a full ARN, and a
    // stored family with an absent revision names no revision either — as does a family
    // whose every revision has been deregistered, since a reference without a revision
    // takes the latest ACTIVE one.
    let references = [
        ("absent family", "ghost-family".to_string()),
        (
            "absent revision of a stored family",
            format!("{FAMILY}:999"),
        ),
        ("all-INACTIVE family", RETIRED_FAMILY.to_string()),
        (
            "full ARN",
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/ghost-family:7"
            ),
        ),
    ];
    // Each operation with the sentence IT answers: the describe reports that it cannot
    // describe the revision, the three that would run or deploy it report that the
    // revision was not found.
    let requests: [DefinitionRow; 4] = [
        (
            "DescribeTaskDefinition",
            |reference| json!({ "taskDefinition": reference }),
            TASK_DEFINITION_NOT_DESCRIBABLE,
        ),
        (
            "RunTask",
            |reference| json!({ "cluster": CLUSTER, "taskDefinition": reference }),
            TASK_DEFINITION_NOT_FOUND,
        ),
        (
            "CreateService",
            |reference| {
                json!({
                    "cluster": CLUSTER,
                    "serviceName": "another-service",
                    "taskDefinition": reference,
                    "desiredCount": 1,
                })
            },
            TASK_DEFINITION_NOT_FOUND,
        ),
        (
            "UpdateService",
            |reference| {
                json!({ "cluster": CLUSTER, "service": SERVICE, "taskDefinition": reference })
            },
            TASK_DEFINITION_NOT_FOUND,
        ),
    ];

    for (operation, build, expected) in requests {
        for (form, reference) in &references {
            let case = format!("{operation} with an {form}");
            let answered = refusal(&router, operation, build(reference), &case).await;
            assert_refusal(answered, "ClientException", expected, reference, &case);
        }
    }
}

#[tokio::test]
async fn deleting_a_cluster_that_still_holds_work_is_refused_with_one_fixed_sentence() {
    let router = fixture().await;
    // The fixture's cluster holds a service, and a second cluster holds a bare task, so
    // each of the two state refusals is reached once.
    create_cluster(&router, "tasks-only", "delete").await;
    // The EC2 launch type places a task on an instance registered to the cluster, so the
    // cluster needs one before `RunTask` will place anything in it.
    register_capacity(&router, "tasks-only", "delete").await;
    ok(
        &router,
        "RunTask",
        json!({
            "cluster": "tasks-only",
            "taskDefinition": FAMILY,
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        "delete",
    )
    .await;

    // The contains-services sentence is the one the wire corpus RECORDS for that refusal
    // (the `ecs|DeleteCluster|4xx->4xx|$.message` evidence in
    // issues/things-that-now-work/20261022T152302Z-ecs-deletecluster-refuses-valid-deletes-and-never-tombstones.md);
    // no recording fixes the contains-tasks one, so it stays the error shape's own doc
    // sentence. Either way the sentence is a CONSTANT of the condition.
    let rows = [
        (
            "a cluster holding a service",
            CLUSTER,
            "ClusterContainsServicesException",
            "The Cluster cannot be deleted while Services are active.",
        ),
        (
            "a cluster holding a task",
            "tasks-only",
            "ClusterContainsTasksException",
            "You can't delete a cluster that has active tasks.",
        ),
    ];
    for (case, cluster, code, expected) in rows {
        let answered = refusal(&router, "DeleteCluster", json!({ "cluster": cluster }), case).await;
        assert_refusal(answered, code, expected, cluster, case);
    }
}

#[tokio::test]
async fn a_fargate_registration_names_the_single_member_it_is_missing() {
    let router = service();

    // `TaskDefinition$cpu` and `$memory`: "This field is required for Fargate";
    // `$networkMode`: "For Amazon ECS tasks on Fargate, the awsvpc network mode is
    // required". Rows: what the registration declares, and the one member the refusal
    // must name.
    let rows: [(&str, Value, &str); 7] = [
        (
            "no cpu, memory given",
            json!({ "networkMode": "awsvpc", "memory": "512" }),
            "Fargate requires that 'cpu' be defined at the task level.",
        ),
        (
            "no memory, cpu given",
            json!({ "networkMode": "awsvpc", "cpu": "256" }),
            "Fargate requires that 'memory' be defined at the task level.",
        ),
        (
            "neither cpu nor memory reports the first member",
            json!({ "networkMode": "awsvpc" }),
            "Fargate requires that 'cpu' be defined at the task level.",
        ),
        (
            "blank cpu defines nothing",
            json!({ "networkMode": "awsvpc", "cpu": "  ", "memory": "512" }),
            "Fargate requires that 'cpu' be defined at the task level.",
        ),
        (
            "blank memory defines nothing",
            json!({ "networkMode": "awsvpc", "cpu": "1024", "memory": "" }),
            "Fargate requires that 'memory' be defined at the task level.",
        ),
        (
            "bridge network mode",
            json!({ "networkMode": "bridge", "cpu": "256", "memory": "512" }),
            "Fargate only supports network mode \u{2018}awsvpc\u{2019}.",
        ),
        (
            "host network mode",
            json!({ "networkMode": "host", "cpu": "2048", "memory": "4096" }),
            "Fargate only supports network mode \u{2018}awsvpc\u{2019}.",
        ),
    ];

    for (case, declared, expected) in rows {
        let family = format!(
            "fargate-{}",
            case.chars()
                .map(|letter| if letter.is_ascii_alphanumeric() {
                    letter
                } else {
                    '-'
                })
                .collect::<String>()
        );
        let mut request = json!({
            "family": family,
            "requiresCompatibilities": ["FARGATE"],
            "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
        });
        for (member, value) in declared.as_object().expect("the row declares an object") {
            request[member] = value.clone();
        }
        let answered = refusal(&router, "RegisterTaskDefinition", request, case).await;
        assert_refusal(answered, "ClientException", expected, &family, case);
    }

    // The same registration with both members defined and `awsvpc` is stored, and reads
    // back through `DescribeTaskDefinition`.
    let body = ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "fargate-complete",
            "requiresCompatibilities": ["FARGATE"],
            "networkMode": "awsvpc",
            "cpu": "512",
            "memory": "1024",
            "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
        }),
        "complete",
    )
    .await;
    let registered = string(&body["taskDefinition"], "taskDefinitionArn", "complete");
    let described = ok(
        &router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": "fargate-complete" }),
        "complete",
    )
    .await;
    assert_eq!(
        string(&described["taskDefinition"], "taskDefinitionArn", "complete"),
        registered,
        "case \"complete\": the stored revision reads back: {described}"
    );
    assert_eq!(
        described["taskDefinition"]["cpu"].as_str(),
        Some("512"),
        "case \"complete\": the stored revision keeps its task-level cpu: {described}"
    );
}

#[tokio::test]
async fn a_registration_names_the_blank_member_of_the_identity_document() {
    let router = service();
    create_cluster(&router, CLUSTER, "identity").await;

    // The members of the EC2 instance identity document a registration must carry, in
    // the order they are validated: a document that leaves several blank is reported on
    // the first of them, and a document that is not JSON at all leaves every member
    // blank.
    let rows: [(&str, Value, &str); 8] = [
        ("empty document", json!("{}"), "accountId"),
        ("not json at all", json!("not-a-document"), "accountId"),
        (
            "no accountId",
            json!(json!({ "instanceId": "i-0123456789abcdef0" }).to_string()),
            "accountId",
        ),
        (
            "blank accountId",
            json!(json!({ "accountId": "   ", "instanceId": "i-0123456789abcdef0" }).to_string()),
            "accountId",
        ),
        (
            "accountId is not a string",
            json!(json!({ "accountId": 1234, "instanceId": "i-0123456789abcdef0" }).to_string()),
            "accountId",
        ),
        (
            "no instanceId",
            json!(json!({ "accountId": ACCOUNT, "region": SIGNED_REGION }).to_string()),
            "instanceId",
        ),
        (
            "blank instanceId",
            json!(json!({ "accountId": ACCOUNT, "instanceId": " " }).to_string()),
            "instanceId",
        ),
        (
            "instanceId is not a string",
            json!(json!({ "accountId": ACCOUNT, "instanceId": 7 }).to_string()),
            "instanceId",
        ),
    ];

    for (case, document, member) in rows {
        let answered = refusal(
            &router,
            "RegisterContainerInstance",
            json!({ "cluster": CLUSTER, "instanceIdentityDocument": document }),
            case,
        )
        .await;
        let (code, message) = answered;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message,
            format!("{member} cannot be blank."),
            "case {case:?}: expected the refusal to name {member:?}, got {message:?}"
        );
    }

    // A complete document, with the signature that vouches for it, registers the
    // instance, which reads back through `DescribeContainerInstances` with the EC2 id
    // the document named.
    let instance_id = "i-0abcdef1234567890";
    let registered = ok(
        &router,
        "RegisterContainerInstance",
        json!({
            "cluster": CLUSTER,
            "instanceIdentityDocument": identity_document(instance_id),
            "instanceIdentityDocumentSignature": identity_document_signature(),
        }),
        "complete",
    )
    .await;
    let arn = string(
        &registered["containerInstance"],
        "containerInstanceArn",
        "complete",
    );
    let described = ok(
        &router,
        "DescribeContainerInstances",
        json!({ "cluster": CLUSTER, "containerInstances": [arn.clone()] }),
        "complete",
    )
    .await;
    let read = &described["containerInstances"][0];
    assert_eq!(
        string(read, "containerInstanceArn", "complete"),
        arn,
        "case \"complete\": the registered instance reads back: {described}"
    );
    assert_eq!(
        read["ec2InstanceId"].as_str(),
        Some(instance_id),
        "case \"complete\": the stored instance keeps the id the document named: \
         {described}"
    );
}
