// SPDX-License-Identifier: CC0-1.0

//! How a `taskDefinition` reference a call would put to WORK is refused, across every
//! operation that takes one.
//!
//! The rule under test: a reference that RESOLVES to a revision this service holds, in a
//! status the call may not use, names an existing resource whose value is wrong for this
//! request — refused with HTTP 400 `InvalidParameterException` ("You can't use an
//! `INACTIVE` task definition to run new tasks or create new services, and you can't
//! update an existing service to reference an `INACTIVE` task definition", the
//! `DeregisterTaskDefinition` doc) — while a reference that resolves to NO revision at all
//! names an identifier this service knows nothing about, which stays the modelled
//! `ClientException` ("it might be specifying an identifier that isn't valid"). The two
//! conditions carry two distinct error codes, because a caller branches on the code to
//! decide whether to register a task definition or to point the request at a live
//! revision. `CreateService`, `UpdateService` and `RunTask` all resolve such a reference,
//! so the rule is asserted on all three.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, list, ok, refusal, service, string, ACCOUNT, SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// A container image, as a container definition names one.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// The cluster every row below runs in.
const CLUSTER: &str = "refusalcluster";

/// The family holding one ACTIVE and one deregistered revision.
const MIXED_FAMILY: &str = "mixedfamily";

/// The family whose ONLY revision was deregistered, so it holds no ACTIVE revision at all.
const RETIRED_FAMILY: &str = "retiredfamily";

/// The ARN a revision of `family` carries, as
/// `arn:aws:ecs:<region>:<account>:task-definition/<family>:<revision>`.
fn definition_arn(family: &str, revision: u32) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{family}:{revision}")
}

/// Register one revision of `family`.
async fn register(router: &MultiServiceRouter, family: &str, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [container("app", IMAGE)],
        }),
        case,
    )
    .await;
}

/// Deregister one revision of `family`.
async fn deregister(router: &MultiServiceRouter, family: &str, revision: u32, case: &str) {
    ok(
        router,
        "DeregisterTaskDefinition",
        json!({ "taskDefinition": format!("{family}:{revision}") }),
        case,
    )
    .await;
}

/// The request body for `operation` naming `reference` as its task definition. `slot`
/// keeps the service a `CreateService` row would mint distinct per row.
fn request_for(operation: &str, reference: &str, slot: usize) -> Value {
    match operation {
        "CreateService" => json!({
            "cluster": CLUSTER,
            "serviceName": format!("fresh{slot}"),
            "taskDefinition": reference,
            "desiredCount": 0,
            "launchType": "EC2",
        }),
        "RunTask" => json!({ "cluster": CLUSTER, "taskDefinition": reference, "count": 1 }),
        "UpdateService" => json!({
            "cluster": CLUSTER,
            "service": "running",
            "taskDefinition": reference,
        }),
        other => panic!("unexpected operation {other:?}"),
    }
}

#[tokio::test]
async fn a_reference_that_resolves_to_an_unusable_revision_is_not_an_unknown_identifier() {
    let router = service();
    let case = "setup";
    cluster_with_capacity(&router, CLUSTER, case).await;
    register(&router, MIXED_FAMILY, case).await;
    register(&router, MIXED_FAMILY, case).await;
    register(&router, RETIRED_FAMILY, case).await;
    // The service the `UpdateService` rows act on runs the revision that stays ACTIVE.
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "running",
            "taskDefinition": format!("{MIXED_FAMILY}:2"),
            "desiredCount": 0,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    deregister(&router, MIXED_FAMILY, 1, case).await;
    deregister(&router, RETIRED_FAMILY, 1, case).await;

    // The rows differ along WHAT the reference resolves to — a held revision in a status
    // the call may not use, or nothing at all — and along the FORM the reference takes:
    // `family:revision`, a full ARN, and a bare family. The expected code is the
    // requirement.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a deregistered revision of a family that still has an ACTIVE one, as \
             family:revision",
            format!("{MIXED_FAMILY}:1"),
            "InvalidParameterException",
        ),
        (
            "the same deregistered revision, by its full ARN",
            definition_arn(MIXED_FAMILY, 1),
            "InvalidParameterException",
        ),
        (
            "the only revision of a family, deregistered, as family:revision",
            format!("{RETIRED_FAMILY}:1"),
            "InvalidParameterException",
        ),
        (
            "the only revision of a family, deregistered, by its full ARN",
            definition_arn(RETIRED_FAMILY, 1),
            "InvalidParameterException",
        ),
        (
            "a family whose every revision is deregistered, named bare",
            RETIRED_FAMILY.to_string(),
            "ClientException",
        ),
        (
            "a revision number that was never registered in a family that exists",
            format!("{MIXED_FAMILY}:9"),
            "ClientException",
        ),
        (
            "the ARN of a revision number that was never registered",
            definition_arn(MIXED_FAMILY, 9),
            "ClientException",
        ),
        (
            "a family that was never registered, named bare",
            "ghostfamily".to_string(),
            "ClientException",
        ),
        (
            "a family that was never registered, with a revision",
            "ghostfamily:1".to_string(),
            "ClientException",
        ),
        (
            "the ARN of a revision of a family that was never registered",
            definition_arn("ghostfamily", 1),
            "ClientException",
        ),
    ];

    for operation in ["CreateService", "RunTask", "UpdateService"] {
        for (slot, (row, reference, want)) in rows.iter().enumerate() {
            let case = format!("{operation}: {row}");
            let (code, message) = refusal(
                &router,
                operation,
                request_for(operation, reference, slot),
                &case,
            )
            .await;
            assert_eq!(
                &code, want,
                "case {case:?}: expected {want} for {reference:?}, got {code} ({message:?})"
            );
            assert!(
                !message.is_empty(),
                "case {case:?}: every refusal carries a message, got {message:?}"
            );
        }

        // The control: the SAME operation accepts the revision that is still ACTIVE, named
        // explicitly and through its family, so the refusals above are keyed on what the
        // reference resolves to and not on the operation.
        let active = definition_arn(MIXED_FAMILY, 2);
        for (slot, reference) in [format!("{MIXED_FAMILY}:2"), MIXED_FAMILY.to_string()]
            .iter()
            .enumerate()
        {
            let case = format!("{operation}: the ACTIVE revision as {reference:?}");
            let body = ok(
                &router,
                operation,
                request_for(operation, reference, 100 + slot * 10 + operation.len()),
                &case,
            )
            .await;
            let resolved = match operation {
                "RunTask" => {
                    let tasks = list(&body, "tasks", &case);
                    assert_eq!(tasks.len(), 1, "case {case:?}: expected one task, got {body}");
                    string(&tasks[0], "taskDefinitionArn", &case)
                }
                _ => string(&body["service"], "taskDefinition", &case),
            };
            assert_eq!(
                resolved, active,
                "case {case:?}: expected the ACTIVE revision {active} to be resolved, got \
                 {resolved} in {body}"
            );
        }
    }

    // The deregistered revisions are still THERE — the refusals above are about the status
    // they hold, not about a record that went missing.
    for family in [MIXED_FAMILY, RETIRED_FAMILY] {
        let case = format!("{family} revision 1 stays describable");
        let body = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": format!("{family}:1") }),
            &case,
        )
        .await;
        let described = &body["taskDefinition"];
        assert_eq!(
            string(described, "status", &case),
            "INACTIVE",
            "case {case:?}: expected the deregistered revision to read INACTIVE, got {body}"
        );
        assert_eq!(
            string(described, "taskDefinitionArn", &case),
            definition_arn(family, 1),
            "case {case:?}: expected the deregistered revision's own ARN, got {body}"
        );
    }
}
