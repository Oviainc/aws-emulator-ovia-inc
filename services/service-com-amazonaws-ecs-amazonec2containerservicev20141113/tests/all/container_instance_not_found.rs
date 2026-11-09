// SPDX-License-Identifier: CC0-1.0

//! What ECS SAYS when a `containerInstance` reference resolves to no instance registered
//! in the named cluster, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rule under test: that refusal is DERIVED FROM THE REQUEST — every operation that
//! resolves such a reference answers `InvalidParameterException` (HTTP 400) whose message
//! names the container instance the request referred to, in the one sentence
//! "Referenced container instance <id> not found." The reference member's doc is "The
//! container instance ID or full ARN of the container instance", so the value a caller
//! sends is either form and the sentence carries the instance ID of it — the last segment
//! of an ARN (`...:container-instance/<cluster>/<id>`), the value itself otherwise.
//!
//! The table below therefore varies the reference along what the rule generalises over —
//! its form (bare id or full ARN), its alphabet, its length, the cluster an ARN names,
//! and whether it names nothing at all or an instance registered into ANOTHER cluster —
//! and the operation it is sent to, and asserts the sentence names that row's instance ID
//! every time. The recorded divergence (an ARN sent to `DeregisterContainerInstance`,
//! answered with the id alone) is ONE row.

use serde_json::{json, Value};

use crate::ecs_wire::{
    create_cluster, identity_document, identity_document_signature, ok, refusal, register_instance,
    service, string, ACCOUNT, SIGNED_REGION,
};

/// The cluster every row names, so a refusal is reached by naming a container instance
/// that is not registered in it rather than by naming a missing cluster.
const CLUSTER: &str = "demo";

/// The cluster holding an instance that this request's cluster does not: a reference is
/// scoped by `cluster` ("If you do not specify a cluster, the default cluster is
/// assumed"), so an instance registered elsewhere is not one the request names.
const OTHER_CLUSTER: &str = "elsewhere";

/// One row of the operation table: the operation's name and the request it is sent,
/// built from the reference the row carries.
type OperationRow = (&'static str, fn(&str) -> Value);

/// The sentence the rule requires, for the instance ID the request referred to.
fn expected_message(instance_id: &str) -> String {
    format!("Referenced container instance {instance_id} not found.")
}

/// Every implemented operation that resolves a container-instance reference and refuses
/// an unresolvable one with `InvalidParameterException`:
///
/// * `DeregisterContainerInstance$containerInstance` (required),
/// * `RegisterContainerInstance$containerInstanceArn` ("the ARN of the container instance
///   (if it was previously registered)").
///
/// `DescribeContainerInstances` is NOT one of them: it reports an unresolvable reference
/// as a `Failure` with reason `MISSING` instead of refusing the call, which
/// [`crate::container_instances`] pins. Neither is `ListTasks`: its `containerInstance` is
/// a FILTER, and a filter selects what it matches — an id no instance of the cluster
/// carries is matched by no task of it, so that listing is answered with the tasks that do
/// match (none), which [`crate::task_listing_filters`] pins. `StartTask`,
/// `UpdateContainerInstancesState` and `UpdateContainerAgent` are still scaffolded (HTTP
/// 501), so they resolve nothing yet.
const REQUESTS: [OperationRow; 2] = [
    ("DeregisterContainerInstance", |reference| {
        json!({ "cluster": CLUSTER, "containerInstance": reference })
    }),
    // A registration presents the identity document and its signature whatever else it
    // names, so this row's refusal is reached by the reference it cannot resolve.
    ("RegisterContainerInstance", |reference| {
        json!({
            "cluster": CLUSTER,
            "containerInstanceArn": reference,
            "instanceIdentityDocument": identity_document("i-0d0d0d0d0d0d0d0d0"),
            "instanceIdentityDocumentSignature": identity_document_signature(),
        })
    }),
];

#[tokio::test]
async fn an_unresolvable_container_instance_reference_is_named_in_the_refusal() {
    let router = service();
    create_cluster(&router, CLUSTER, "fixture").await;
    create_cluster(&router, OTHER_CLUSTER, "fixture").await;

    // An instance that IS registered — into another cluster. Both the id and the ARN of
    // this one are references that resolve to nothing in `CLUSTER`.
    let elsewhere_arn = register_instance(&router, OTHER_CLUSTER, "i-0b0b0b0b0b0b0b0b0", "fixture")
        .await;
    let elsewhere_id = elsewhere_arn
        .rsplit('/')
        .next()
        .expect("a container-instance ARN ends in its id")
        .to_string();

    // Rows: the form, the reference the request sends, and the instance ID the sentence
    // must name for it.
    let references = [
        (
            "uuid-shaped id",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
        ),
        ("32-character hex id", "d".repeat(32), "d".repeat(32)),
        ("short id", "x1".to_string(), "x1".to_string()),
        ("long id", "g".repeat(255), "g".repeat(255)),
        // The form the recorded divergence sent: an ARN, answered with its id alone.
        (
            "full ARN of nothing",
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:container-instance/{CLUSTER}/{}",
                "e".repeat(32)
            ),
            "e".repeat(32),
        ),
        (
            "full ARN naming another cluster",
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:container-instance/{OTHER_CLUSTER}/{}",
                "f".repeat(32)
            ),
            "f".repeat(32),
        ),
        (
            "id of an instance registered into another cluster",
            elsewhere_id.clone(),
            elsewhere_id.clone(),
        ),
        (
            "ARN of an instance registered into another cluster",
            elsewhere_arn.clone(),
            elsewhere_id.clone(),
        ),
    ];

    for (operation, build) in REQUESTS {
        for (form, reference, instance_id) in &references {
            let case = format!("{operation} with the {form}");
            let (code, message) = refusal(&router, operation, build(reference), &case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected an InvalidParameterException, got {code:?} with \
                 {message:?}"
            );
            assert_eq!(
                message,
                expected_message(instance_id),
                "case {case:?}: expected {:?}, got {message:?}",
                expected_message(instance_id)
            );
            assert!(
                message.contains(instance_id.as_str()),
                "case {case:?}: the refusal must name the instance the request referred \
                 to {instance_id:?}, got {message:?}"
            );
        }
    }

    // The instance registered elsewhere is untouched by every refusal above: it still
    // deregisters from the cluster it IS registered in.
    let body = ok(
        &router,
        "DeregisterContainerInstance",
        json!({ "cluster": OTHER_CLUSTER, "containerInstance": elsewhere_arn.clone() }),
        "elsewhere deregisters",
    )
    .await;
    assert_eq!(
        string(&body["containerInstance"], "containerInstanceArn", "elsewhere deregisters"),
        elsewhere_arn,
        "case \"elsewhere deregisters\": expected the instance the request named, got {body}"
    );
}

#[tokio::test]
async fn a_registered_container_instance_deregisters_by_either_reference_form() {
    let router = service();
    create_cluster(&router, CLUSTER, "fixture").await;

    // Rows: the case, the EC2 instance id the registration reports, and whether the
    // deregistration refers to the stored instance by its full ARN or by the bare id —
    // the two forms the member documents.
    let rows: [(&str, &str, bool); 2] = [
        ("by id", "i-0a0a0a0a0a0a0a0a0", false),
        ("by full ARN", "i-0c0c0c0c0c0c0c0c0", true),
    ];

    for (case, instance_id, by_arn) in rows {
        let arn = register_instance(&router, CLUSTER, instance_id, case).await;
        let reference = if by_arn {
            arn.clone()
        } else {
            arn.rsplit('/')
                .next()
                .expect("a container-instance ARN ends in its id")
                .to_string()
        };

        let body = ok(
            &router,
            "DeregisterContainerInstance",
            json!({ "cluster": CLUSTER, "containerInstance": reference }),
            case,
        )
        .await;
        let instance = &body["containerInstance"];
        assert_eq!(
            string(instance, "containerInstanceArn", case),
            arn,
            "case {case:?}: expected the stored instance back, got {body}"
        );
        assert_eq!(
            instance["status"].as_str(),
            Some("INACTIVE"),
            "case {case:?}: a deregistered instance reports INACTIVE, got {body}"
        );

        // It reads back from state as deregistered, so the refusal above is about
        // resolution and not about a missing write.
        let described = ok(
            &router,
            "DescribeContainerInstances",
            json!({ "cluster": CLUSTER, "containerInstances": [arn.clone()] }),
            case,
        )
        .await;
        assert_eq!(
            described["containerInstances"][0]["status"].as_str(),
            Some("INACTIVE"),
            "case {case:?}: the stored instance is INACTIVE, got {described}"
        );
    }
}
