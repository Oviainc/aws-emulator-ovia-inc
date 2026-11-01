// SPDX-License-Identifier: CC0-1.0

//! Which refusal Amazon ECS answers for an identifier it will not resolve, driven end to
//! end over HTTP.
//!
//! The rules under test, each over a TABLE of inputs rather than one request:
//!
//! 1. An ARN is a WHOLE identifier, account included. The account segment is checked
//!    BEFORE the identifier is resolved, so an ARN whose account is not the caller's is
//!    refused `InvalidParameterException` — "AccountIDs mismatch" where the member takes
//!    "the short name or full Amazon Resource Name (ARN)" of the resource, and "The ARN
//!    provided is invalid." where the member takes nothing but an ARN (a tagging
//!    `resourceArn`) — instead of resolving to this account's resource of the same name.
//! 2. A deleted cluster is OBSERVABLE as the tombstone it is: a write of its settings is
//!    refused `ClientException` naming it and the state it is not in, and a tagging call
//!    on its ARN is refused `InvalidParameterException` rather than answering 200 with
//!    the tags it carried.
//! 3. A `nextToken` ListClusters never minted is refused `InvalidParameterException`
//!    with the token the request carried, which is the class a paginating client branches
//!    on to abandon a stale cursor; the class and the sentence belong to the OPERATION,
//!    and the task-definition and attribute listings keep their own.
//! 4. `ListDaemons` FILTERS by a cluster ARN, so a well-formed cluster ARN that names no
//!    cluster selects no daemon and answers an empty page.
//! 5. `RegisterContainerInstance` reports a missing cluster in a class of its own, apart
//!    from the one it reports an unreadable identity document in.
//! 6. `DeleteService` and `UpdateService` word a service they cannot resolve
//!    differently: the delete carries no sentence at all.
//! 7. A `taskDefinition` whose family part is not a family name is refused with ONE fixed
//!    sentence, carrying nothing of the request.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    answered, create_cluster, identity_document, identity_document_signature, list, ok, refusal,
    register_definition, register_instance, service, string, ACCOUNT, SIGNED_REGION,
};

/// One row of an operation table: the operation, and the request it builds around the
/// ONE value the row is testing.
type OperationRow = (&'static str, fn(&str) -> Value);

/// The sentence a resource IDENTIFIER that is an ARN of another account earns.
const ACCOUNT_IDS_MISMATCH: &str = "AccountIDs mismatch";

/// The sentence a tagging `resourceArn` of another account earns.
const INVALID_ARN: &str = "The ARN provided is invalid.";

/// The sentence a tagging call on a DELETED cluster's ARN earns.
const CLUSTER_INACTIVE_FOR_TAGGING: &str =
    "The specified cluster is inactive. Specify an active cluster and try again.";

/// The fixed sentence a `taskDefinition` whose family part is no family name earns.
const FAMILY_CHARACTERS: &str = "Family contains invalid characters.";

/// Accounts that are not this emulator's ([`ACCOUNT`]). Rows differ in the shape of the
/// account segment as well as its value, because what makes an ARN another account's is
/// only that the segment is not the caller's.
const OTHER_ACCOUNTS: [&str; 3] = ["111122223333", "999999999999", "1"];

/// A task id of the form `RunTask` mints: 32 hexadecimal digits.
const TASK_ID: &str = "0123456789abcdef0123456789abcdef";

/// The cluster every row that needs a real one works in.
const CLUSTER: &str = "arnaccount";

/// The family every row that needs a stored revision runs.
const FAMILY: &str = "arnaccountfamily";

/// An ECS ARN in `account`, for the resource path `resource`.
fn ecs_arn(account: &str, resource: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{account}:{resource}")
}

/// One row of the foreign-ARN table: the case, the operation, a request built around ONE
/// ARN in the account the row is given, the sentence the row earns when that account is
/// not the caller's, and the error code the SAME request earns when it is — `None` for a
/// request that is answered 200 for the caller's own account.
struct ArnRow {
    case: &'static str,
    operation: &'static str,
    resource: fn(&str) -> String,
    request: fn(&str) -> Value,
    message: &'static str,
    own_account_code: Option<&'static str>,
}

/// Every member kind an ARN identifier reaches this service through, over the operations
/// that resolve one: a `cluster` (by name or ARN), a `task`, a list of either, and a
/// tagging `resourceArn`.
fn arn_rows() -> Vec<ArnRow> {
    vec![
        ArnRow {
            case: "DeleteCluster, a cluster ARN",
            operation: "DeleteCluster",
            resource: |account| ecs_arn(account, "cluster/does-not-exist-xyz"),
            request: |arn| json!({ "cluster": arn }),
            message: ACCOUNT_IDS_MISMATCH,
            own_account_code: Some("ClusterNotFoundException"),
        },
        ArnRow {
            case: "UpdateClusterSettings, a cluster ARN",
            operation: "UpdateClusterSettings",
            resource: |account| ecs_arn(account, "cluster/does-not-exist-xyz"),
            request: |arn| {
                json!({
                    "cluster": arn,
                    "settings": [{ "name": "containerInsights", "value": "enabled" }],
                })
            },
            message: ACCOUNT_IDS_MISMATCH,
            own_account_code: Some("ClusterNotFoundException"),
        },
        ArnRow {
            case: "ListServices, a cluster ARN",
            operation: "ListServices",
            resource: |account| ecs_arn(account, "cluster/does-not-exist-xyz"),
            request: |arn| json!({ "cluster": arn }),
            message: ACCOUNT_IDS_MISMATCH,
            own_account_code: Some("ClusterNotFoundException"),
        },
        ArnRow {
            case: "RunTask, a cluster ARN",
            operation: "RunTask",
            resource: |account| ecs_arn(account, "cluster/does-not-exist-xyz"),
            request: |arn| json!({ "cluster": arn, "taskDefinition": FAMILY }),
            message: ACCOUNT_IDS_MISMATCH,
            own_account_code: Some("ClusterNotFoundException"),
        },
        ArnRow {
            case: "RegisterContainerInstance, a cluster ARN",
            operation: "RegisterContainerInstance",
            resource: |account| ecs_arn(account, "cluster/does-not-exist-xyz"),
            request: |arn| {
                json!({
                    "cluster": arn,
                    "instanceIdentityDocument": identity_document("i-0a1b2c3d4e5f60718"),
                    "instanceIdentityDocumentSignature": identity_document_signature(),
                })
            },
            message: ACCOUNT_IDS_MISMATCH,
            own_account_code: Some("ClientException"),
        },
        ArnRow {
            case: "DescribeClusters, a cluster ARN in the list",
            operation: "DescribeClusters",
            resource: |account| ecs_arn(account, "cluster/does-not-exist-xyz"),
            request: |arn| json!({ "clusters": [arn] }),
            message: ACCOUNT_IDS_MISMATCH,
            // A cluster of the caller's own account that nothing holds is a MISSING
            // failure rather than a refusal, which is exactly the answer an ARN of
            // another account must not reach.
            own_account_code: None,
        },
        ArnRow {
            case: "StopTask, a task ARN",
            operation: "StopTask",
            resource: |account| ecs_arn(account, &format!("task/{CLUSTER}/{TASK_ID}")),
            request: |arn| json!({ "cluster": CLUSTER, "task": arn }),
            message: ACCOUNT_IDS_MISMATCH,
            own_account_code: Some("InvalidParameterException"),
        },
        ArnRow {
            case: "StopTask, a cluster ARN beside a bare task id",
            operation: "StopTask",
            resource: |account| ecs_arn(account, &format!("cluster/{CLUSTER}")),
            request: |arn| json!({ "cluster": arn, "task": TASK_ID }),
            message: ACCOUNT_IDS_MISMATCH,
            own_account_code: Some("InvalidParameterException"),
        },
        ArnRow {
            case: "DescribeTasks, a task ARN in the list",
            operation: "DescribeTasks",
            resource: |account| ecs_arn(account, &format!("task/{CLUSTER}/{TASK_ID}")),
            request: |arn| json!({ "cluster": CLUSTER, "tasks": [arn] }),
            message: ACCOUNT_IDS_MISMATCH,
            own_account_code: None,
        },
        ArnRow {
            case: "GetTaskProtection, a task ARN in the list",
            operation: "GetTaskProtection",
            resource: |account| ecs_arn(account, &format!("task/{CLUSTER}/{TASK_ID}")),
            request: |arn| json!({ "cluster": CLUSTER, "tasks": [arn] }),
            message: ACCOUNT_IDS_MISMATCH,
            own_account_code: None,
        },
        ArnRow {
            case: "TagResource, a cluster resourceArn",
            operation: "TagResource",
            resource: |account| ecs_arn(account, &format!("cluster/{CLUSTER}")),
            request: |arn| {
                json!({ "resourceArn": arn, "tags": [{ "key": "team", "value": "platform" }] })
            },
            message: INVALID_ARN,
            own_account_code: None,
        },
        ArnRow {
            case: "ListTagsForResource, a cluster resourceArn",
            operation: "ListTagsForResource",
            resource: |account| ecs_arn(account, &format!("cluster/{CLUSTER}")),
            request: |arn| json!({ "resourceArn": arn }),
            message: INVALID_ARN,
            own_account_code: None,
        },
        ArnRow {
            case: "UntagResource, a task resourceArn",
            operation: "UntagResource",
            resource: |account| ecs_arn(account, &format!("task/{CLUSTER}/{TASK_ID}")),
            request: |arn| json!({ "resourceArn": arn, "tagKeys": ["team"] }),
            message: INVALID_ARN,
            own_account_code: Some("InvalidParameterException"),
        },
    ]
}

/// A router holding one cluster and one task-definition revision.
async fn fixture() -> MultiServiceRouter {
    let router = service();
    create_cluster(&router, CLUSTER, "fixture").await;
    register_definition(&router, FAMILY, "fixture").await;
    router
}

#[tokio::test]
async fn an_arn_identifier_of_another_account_is_refused_before_the_resource_is_resolved() {
    let router = fixture().await;

    for row in arn_rows() {
        for account in OTHER_ACCOUNTS {
            let arn = (row.resource)(account);
            let case = format!("{} in account {account:?}", row.case);
            let (code, message) =
                refusal(&router, row.operation, (row.request)(&arn), &case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected an InvalidParameterException for {arn}, got \
                 {code} with {message:?}"
            );
            assert_eq!(
                message, row.message,
                "case {case:?}: expected {:?} for {arn}, got {message:?}",
                row.message
            );
        }

        // The SAME request with the caller's own account is resolved: whichever answer it
        // earns, it is not the foreign-ARN refusal, so the account segment is what the
        // rows above were refused for.
        let own = (row.resource)(ACCOUNT);
        let case = format!("{}, the caller's own account", row.case);
        let (status, body) = answered(&router, row.operation, (row.request)(&own)).await;
        match row.own_account_code {
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected {expected} for {own}, got {status} with \
                     {body}"
                );
                let wire_type = body["__type"].as_str().unwrap_or_default();
                assert!(
                    wire_type.contains(expected),
                    "case {case:?}: expected {expected} for {own}, got {body}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected {own} to be answered, got {status} with {body}"
            ),
        }
        assert_ne!(
            body["message"].as_str(),
            Some(row.message),
            "case {case:?}: the caller's own ARN must not earn the foreign-ARN sentence, \
             got {body}"
        );
    }
}

#[tokio::test]
async fn a_value_that_is_no_arn_is_resolved_as_the_name_it_is() {
    let router = fixture().await;

    // The check is about an ARN's ACCOUNT segment, so a value that carries no account —
    // a bare cluster name, and an ARN whose account field is empty — is resolved rather
    // than refused for an account it does not have.
    let rows = [
        ("a bare cluster name", CLUSTER.to_string()),
        (
            "an ARN with an empty account segment",
            format!("arn:aws:ecs:{SIGNED_REGION}::cluster/{CLUSTER}"),
        ),
        ("a cluster ARN of the caller's account", ecs_arn(ACCOUNT, &format!("cluster/{CLUSTER}"))),
    ];
    for (case, reference) in rows {
        let body = ok(&router, "ListServices", json!({ "cluster": reference }), case).await;
        assert!(
            list(&body, "serviceArns", case).is_empty(),
            "case {case:?}: expected the stored cluster to be resolved and hold no \
             service, got {body}"
        );
    }
}

#[tokio::test]
async fn a_deleted_cluster_is_reported_as_the_tombstone_it_is_by_both_paths() {
    let router = service();

    // Rows differ in the cluster NAME's shape, because the settings refusal carries that
    // name from the request while the tagging refusal carries nothing at all.
    for name in ["lifecycle", "b_2", "a-very-long-cluster-name-for-one-row"] {
        let arn = create_cluster(&router, name, name).await;
        ok(
            &router,
            "TagResource",
            json!({ "resourceArn": arn, "tags": [{ "key": "team", "value": "platform" }] }),
            name,
        )
        .await;
        // While the cluster is ACTIVE the tag set reads back, so the refusals below are
        // the DELETE's doing and not a tagging call that never worked.
        let read = ok(
            &router,
            "ListTagsForResource",
            json!({ "resourceArn": arn }),
            name,
        )
        .await;
        assert_eq!(
            list(&read, "tags", name).len(),
            1,
            "case {name:?}: expected the stored tag to read back while ACTIVE, got {read}"
        );
        ok(&router, "DeleteCluster", json!({ "cluster": name }), name).await;

        // The settings write finds the cluster and refuses the STATE it is in, naming the
        // cluster the request asked for.
        let settings = json!({
            "cluster": name,
            "settings": [{ "name": "containerInsights", "value": "enabled" }],
        });
        let case = format!("UpdateClusterSettings on deleted {name:?}");
        let (code, message) = refusal(&router, "UpdateClusterSettings", settings, &case).await;
        assert_eq!(
            code, "ClientException",
            "case {case:?}: expected the not-ACTIVE class, got {code} with {message:?}"
        );
        assert_eq!(
            message,
            format!("Cluster {name} is not in ACTIVE state"),
            "case {case:?}: expected the not-ACTIVE sentence naming the cluster, got \
             {message:?}"
        );

        // And every tagging call on the same ARN refuses it too, rather than reporting
        // the tags as if the cluster were live.
        for (operation, request) in [
            ("ListTagsForResource", json!({ "resourceArn": arn })),
            (
                "TagResource",
                json!({
                    "resourceArn": arn,
                    "tags": [{ "key": "team", "value": "other" }],
                }),
            ),
            (
                "UntagResource",
                json!({ "resourceArn": arn, "tagKeys": ["team"] }),
            ),
        ] {
            let case = format!("{operation} on deleted {name:?}");
            let (code, message) = refusal(&router, operation, request, &case).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected the inactive-cluster refusal, got {code} with \
                 {message:?}"
            );
            assert_eq!(
                message, CLUSTER_INACTIVE_FOR_TAGGING,
                "case {case:?}: expected {CLUSTER_INACTIVE_FOR_TAGGING:?}, got {message:?}"
            );
        }

        // A name this account never held stays the NOT-FOUND refusal: the two conditions
        // are told apart by the stored record, not by the request.
        let absent = format!("{name}-never-created");
        let case = format!("UpdateClusterSettings on absent {absent:?}");
        let (code, message) = refusal(
            &router,
            "UpdateClusterSettings",
            json!({
                "cluster": absent,
                "settings": [{ "name": "containerInsights", "value": "enabled" }],
            }),
            &case,
        )
        .await;
        assert_eq!(
            code, "ClusterNotFoundException",
            "case {case:?}: expected the not-found class, got {code} with {message:?}"
        );
        assert_eq!(
            message, "Cluster not found.",
            "case {case:?}: expected the fixed not-found sentence, got {message:?}"
        );
        let case = format!("ListTagsForResource on absent {absent:?}");
        let (code, _) = refusal(
            &router,
            "ListTagsForResource",
            json!({ "resourceArn": ecs_arn(ACCOUNT, &format!("cluster/{absent}")) }),
            &case,
        )
        .await;
        assert_eq!(
            code, "ClusterNotFoundException",
            "case {case:?}: expected a cluster nothing holds to be not found, got {code}"
        );
    }
}

#[tokio::test]
async fn a_token_list_clusters_never_minted_is_refused_as_an_invalid_parameter() {
    let router = service();
    create_cluster(&router, CLUSTER, "tokens").await;
    register_definition(&router, FAMILY, "tokens").await;

    // The tokens span the ways a cursor can fail to name a page: empty, prose, a
    // base64-looking blob, a resource name where an ARN is minted, and an ARN of a
    // resource nothing holds.
    let tokens = [
        String::new(),
        "sim2real-totally-bogus-token".to_string(),
        "c2ltdWxhdGVkLXRva2Vu==".to_string(),
        CLUSTER.to_string(),
        ecs_arn(ACCOUNT, "cluster/nosuchcluster"),
    ];
    for token in &tokens {
        let case = format!("ListClusters resuming from {token:?}");
        let (code, message) = refusal(
            &router,
            "ListClusters",
            json!({ "nextToken": token }),
            &case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code} with \
             {message:?}"
        );
        assert_eq!(
            message,
            format!("Invalid token {token}"),
            "case {case:?}: expected the sentence ECS answers for an unusable token, \
             got {message:?}"
        );
    }

    // Which class and which sentence a cursor refusal takes is a property of the
    // OPERATION, not of the condition: the task-definition and attribute listings answer
    // the other pair for the very same tokens, so the rows above are not a service-wide
    // rename.
    let listings: [OperationRow; 3] = [
        ("ListTaskDefinitions", |token| json!({ "nextToken": token })),
        ("ListTaskDefinitionFamilies", |token| {
            json!({ "nextToken": token })
        }),
        ("ListAttributes", |token| {
            json!({
                "cluster": CLUSTER,
                "targetType": "container-instance",
                "nextToken": token,
            })
        }),
    ];
    for (operation, build) in listings {
        for token in &tokens {
            let case = format!("{operation} resuming from {token:?}");
            let (code, message) = refusal(&router, operation, build(token), &case).await;
            assert_eq!(
                code, "ClientException",
                "case {case:?}: expected a ClientException, got {code} with {message:?}"
            );
            assert_eq!(
                message, "Invalid nextToken.",
                "case {case:?}: expected this operation's own sentence, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn list_daemons_answers_an_empty_page_for_a_cluster_arn_that_names_no_cluster() {
    let router = service();
    create_cluster(&router, CLUSTER, "daemons").await;

    // A client sweeping daemons across a set of cluster ARNs — some of them since
    // deleted, some never created — completes the sweep: each ARN selects the daemons of
    // that cluster, which for a cluster that is not there is none.
    let deleted = create_cluster(&router, "sweptaway", "daemons").await;
    ok(
        &router,
        "DeleteCluster",
        json!({ "cluster": "sweptaway" }),
        "daemons",
    )
    .await;
    let arns = [
        ("the stored cluster", ecs_arn(ACCOUNT, &format!("cluster/{CLUSTER}"))),
        ("a cluster that was never created", ecs_arn(ACCOUNT, "cluster/nosuchcluster")),
        (
            "a cluster name of the longest form",
            ecs_arn(ACCOUNT, &format!("cluster/{}", "d".repeat(255))),
        ),
        ("a cluster that was deleted", deleted),
        (
            "a cluster ARN in another Region of this account",
            format!("arn:aws:ecs:eu-west-1:{ACCOUNT}:cluster/elsewhere"),
        ),
    ];
    for (case, arn) in arns {
        let body = ok(&router, "ListDaemons", json!({ "clusterArn": arn }), case).await;
        assert!(
            list(&body, "daemonSummariesList", case).is_empty(),
            "case {case:?}: expected an empty page for {arn}, got {body}"
        );
        assert_eq!(
            body["nextToken"].as_str(),
            None,
            "case {case:?}: expected no nextToken for {arn}, got {body}"
        );
    }

    // A reference that is a cluster NAME nothing holds names a cluster this account could
    // have, so it keeps the cluster-resolution refusal.
    for (case, reference) in [
        ("a bare name nothing holds", "nosuchcluster".to_string()),
        ("the default cluster, never created", String::new()),
    ] {
        let mut request = json!({});
        if !reference.is_empty() {
            request["clusterArn"] = json!(reference);
        }
        let (code, message) = refusal(&router, "ListDaemons", request, case).await;
        assert_eq!(
            code, "ClusterNotFoundException",
            "case {case:?}: expected {reference:?} to be refused, got {code} with \
             {message:?}"
        );
    }

    // A reference that is NEITHER a cluster ARN nor a cluster name identifies no cluster
    // in any account, so there is nothing to filter by and nothing to report absent: the
    // request is answered as one this service cannot act on.
    for (case, reference) in [
        ("a name outside the cluster character set", "not a cluster!!".to_string()),
        ("a path where an identifier belongs", "clusters/one".to_string()),
        (
            "an ARN of another resource kind",
            ecs_arn(ACCOUNT, &format!("service/{CLUSTER}/web")),
        ),
        (
            "an ARN whose cluster segment is no cluster name",
            ecs_arn(ACCOUNT, "cluster/not a name"),
        ),
    ] {
        let (status, body) =
            answered(&router, "ListDaemons", json!({ "clusterArn": reference })).await;
        assert_eq!(
            status, 500,
            "case {case:?}: expected {reference:?} to answer the server class, got \
             {status} with {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("Service Unavailable. Please try again later."),
            "case {case:?}: expected the server-class sentence, got {body}"
        );
    }
}

#[tokio::test]
async fn register_container_instance_tells_a_missing_cluster_from_a_document_it_cannot_read() {
    let router = service();
    create_cluster(&router, CLUSTER, "register").await;

    // Against a REAL cluster, a missing or unusable identity document is the
    // request-shape error; against a cluster that is not there, the cluster is reported
    // first and in its own class — so an agent can tell "retry with a document" from
    // "this cluster is gone".
    let documents: [(&str, Value); 3] = [
        ("no identity document at all", json!({})),
        (
            "a document with no signature",
            json!({ "instanceIdentityDocument": identity_document("i-0a1b2c3d4e5f60718") }),
        ),
        (
            "a document that is not JSON",
            json!({
                "instanceIdentityDocument": "not-a-document",
                "instanceIdentityDocumentSignature": identity_document_signature(),
            }),
        ),
    ];
    for (case, extra) in documents {
        let mut request = json!({ "cluster": CLUSTER });
        for (member, value) in extra.as_object().expect("the row is an object") {
            request[member] = value.clone();
        }
        let case = format!("{case}, against a real cluster");
        let (code, message) =
            refusal(&router, "RegisterContainerInstance", request.clone(), &case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the request-shape class, got {code} with {message:?}"
        );

        // The same request against a cluster nothing holds reports the CLUSTER.
        for cluster in ["no-such-cluster-abc", "no-such-cluster-xyz"] {
            let mut missing = request.clone();
            missing["cluster"] = json!(cluster);
            let case = format!("{case}, against cluster {cluster:?}");
            let (code, message) =
                refusal(&router, "RegisterContainerInstance", missing, &case).await;
            assert_eq!(
                code, "ClientException",
                "case {case:?}: expected the missing-cluster class, got {code} with \
                 {message:?}"
            );
            assert_eq!(
                message, "Cluster not found.",
                "case {case:?}: expected the fixed not-found sentence, got {message:?}"
            );
        }
    }

    // And a complete registration into the real cluster still works, so the reordering
    // above refuses nothing it should serve: the instance reads back through its describe.
    let body = ok(
        &router,
        "RegisterContainerInstance",
        json!({
            "cluster": CLUSTER,
            "instanceIdentityDocument": identity_document("i-0f0f0f0f0f0f0f0f0"),
            "instanceIdentityDocumentSignature": identity_document_signature(),
        }),
        "complete",
    )
    .await;
    let arn = string(&body["containerInstance"], "containerInstanceArn", "complete");
    let read = ok(
        &router,
        "DescribeContainerInstances",
        json!({ "cluster": CLUSTER, "containerInstances": [arn.clone()] }),
        "complete",
    )
    .await;
    let instances = list(&read, "containerInstances", "complete");
    assert_eq!(
        instances.len(),
        1,
        "case \"complete\": expected the registered instance to read back, got {read}"
    );
    assert_eq!(
        string(&instances[0], "containerInstanceArn", "complete"),
        arn,
        "case \"complete\": expected the stored instance, got {read}"
    );
}

#[tokio::test]
async fn update_container_instances_state_moves_the_stored_instance_between_its_statuses() {
    let router = service();
    create_cluster(&router, CLUSTER, "instances").await;
    let first = register_instance(&router, CLUSTER, "i-0a0a0a0a0a0a0a0a0", "instances").await;
    let second = register_instance(&router, CLUSTER, "i-0b0b0b0b0b0b0b0b0", "instances").await;

    // The `status` the rows walk the instances through, and how the request names them:
    // by full ARN, and by the bare id the ARN ends with.
    let rows: [(&str, bool); 4] = [
        ("DRAINING", false),
        ("ACTIVE", false),
        ("DRAINING", true),
        ("ACTIVE", true),
    ];
    for (status, by_id) in rows {
        let references: Vec<String> = [&first, &second]
            .iter()
            .map(|arn| match by_id {
                true => arn.rsplit('/').next().expect("an ARN ends in its id").to_string(),
                false => (*arn).clone(),
            })
            .collect();
        let case = format!("moving to {status} named {}", match by_id {
            true => "by id",
            false => "by ARN",
        });
        let body = ok(
            &router,
            "UpdateContainerInstancesState",
            json!({
                "cluster": CLUSTER,
                "containerInstances": references,
                "status": status,
            }),
            &case,
        )
        .await;
        let updated = list(&body, "containerInstances", &case);
        assert_eq!(
            updated.len(),
            2,
            "case {case:?}: expected both instances to be answered, got {body}"
        );
        assert!(
            list(&body, "failures", &case).is_empty(),
            "case {case:?}: expected no failure for stored instances, got {body}"
        );

        // The write is into the STORED record: both the describe and the status-filtered
        // listing report the new status.
        let read = ok(
            &router,
            "DescribeContainerInstances",
            json!({ "cluster": CLUSTER, "containerInstances": [first.clone(), second.clone()] }),
            &case,
        )
        .await;
        for instance in list(&read, "containerInstances", &case) {
            assert_eq!(
                string(&instance, "status", &case),
                status,
                "case {case:?}: expected the stored status to be {status}, got {read}"
            );
        }
        let listed = ok(
            &router,
            "ListContainerInstances",
            json!({ "cluster": CLUSTER, "status": status }),
            &case,
        )
        .await;
        assert_eq!(
            list(&listed, "containerInstanceArns", &case).len(),
            2,
            "case {case:?}: expected the {status} filter to report both, got {listed}"
        );
    }

    // An id that names no registered instance of this cluster is reported in the
    // `failures` channel, under the identifier the request sent.
    let case = "an id that names no registered instance";
    let body = ok(
        &router,
        "UpdateContainerInstancesState",
        json!({
            "cluster": CLUSTER,
            "containerInstances": ["f".repeat(32)],
            "status": "DRAINING",
        }),
        case,
    )
    .await;
    assert!(
        list(&body, "containerInstances", case).is_empty(),
        "case {case:?}: expected nothing to be updated, got {body}"
    );
    let failures = list(&body, "failures", case);
    assert_eq!(
        failures.len(),
        1,
        "case {case:?}: expected one failure, got {body}"
    );
    assert_eq!(
        string(&failures[0], "reason", case),
        "MISSING",
        "case {case:?}: expected a MISSING failure, got {body}"
    );

    // The statuses this action does not govern are refused: "The only valid values for
    // this action are ACTIVE and DRAINING."
    for status in ["REGISTERING", "DEREGISTERING", "REGISTRATION_FAILED"] {
        let case = format!("asking for {status}");
        let (code, message) = refusal(
            &router,
            "UpdateContainerInstancesState",
            json!({
                "cluster": CLUSTER,
                "containerInstances": [first.clone()],
                "status": status,
            }),
            &case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected a status outside the two to be refused, got {code} \
             with {message:?}"
        );
        // Nothing moved: the instance still reads back in the status it was left in.
        let read = ok(
            &router,
            "DescribeContainerInstances",
            json!({ "cluster": CLUSTER, "containerInstances": [first.clone()] }),
            &case,
        )
        .await;
        assert_eq!(
            string(&list(&read, "containerInstances", &case)[0], "status", &case),
            "ACTIVE",
            "case {case:?}: expected the refused call to change nothing, got {read}"
        );
    }

    // And a cluster this account does not hold hosts no instance to update: the sibling
    // container-instance operations' own refusal.
    let case = "a cluster that was never created";
    let (code, message) = refusal(
        &router,
        "UpdateContainerInstancesState",
        json!({
            "cluster": "no-such-cluster",
            "containerInstances": [first.clone()],
            "status": "DRAINING",
        }),
        case,
    )
    .await;
    assert_eq!(
        code, "ClusterNotFoundException",
        "case {case:?}: expected a ClusterNotFoundException, got {code} with {message:?}"
    );
    assert_eq!(
        message, "Cluster not found.",
        "case {case:?}: expected the fixed not-found sentence, got {message:?}"
    );
}

#[tokio::test]
async fn a_service_that_does_not_exist_is_worded_by_the_operation_that_was_refused() {
    let router = service();
    create_cluster(&router, CLUSTER, "services").await;
    register_definition(&router, FAMILY, "services").await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": "web",
            "taskDefinition": FAMILY,
            "desiredCount": 0,
            "launchType": "EC2",
        }),
        "services",
    )
    .await;

    // Rows differ in the shape of the name that resolves to nothing, including a name a
    // stored service's own name extends — the sentence is a property of the OPERATION,
    // not of the value.
    for name in ["nonexistent-svc", "we", "web2", "a_long_service_name_42"] {
        let case = format!("UpdateService naming {name:?}");
        let (code, message) = refusal(
            &router,
            "UpdateService",
            json!({ "cluster": CLUSTER, "service": name, "desiredCount": 1 }),
            &case,
        )
        .await;
        assert_eq!(
            code, "ServiceNotFoundException",
            "case {case:?}: expected a ServiceNotFoundException, got {code} with \
             {message:?}"
        );
        assert_eq!(
            message, "Service not found.",
            "case {case:?}: expected the update's own sentence, got {message:?}"
        );

        let case = format!("DeleteService naming {name:?}");
        let (code, message) = refusal(
            &router,
            "DeleteService",
            json!({ "cluster": CLUSTER, "service": name }),
            &case,
        )
        .await;
        assert_eq!(
            code, "ServiceNotFoundException",
            "case {case:?}: expected a ServiceNotFoundException, got {code} with \
             {message:?}"
        );
        assert_eq!(
            message, "",
            "case {case:?}: expected the delete to carry no sentence, got {message:?}"
        );
    }

    // The service that IS stored is still deleted by the same call, so the refusal above
    // is about the name resolving to nothing.
    let deleted = ok(
        &router,
        "DeleteService",
        json!({ "cluster": CLUSTER, "service": "web" }),
        "stored",
    )
    .await;
    assert_eq!(
        string(&deleted["service"], "serviceName", "stored"),
        "web",
        "case \"stored\": expected the stored service to be deleted, got {deleted}"
    );
}

#[tokio::test]
async fn a_task_definition_identifier_that_names_no_family_is_refused_with_one_fixed_sentence() {
    let router = service();
    create_cluster(&router, CLUSTER, "definitions").await;
    register_definition(&router, FAMILY, "definitions").await;

    // A family is "Up to 255 letters (uppercase and lowercase), numbers, underscores, and
    // hyphens" (`RegisterTaskDefinition$family`), so the rows span the ways a value can
    // carry a character that set does not admit — and the ARN of another resource kind,
    // whose trailing path is no family either.
    let identifiers = [
        "not a valid @@@ identifier",
        "spaces are not allowed",
        "slash/inside",
        "colon:colon:colon",
        "exclaim!",
        &ecs_arn(ACCOUNT, &format!("service/{CLUSTER}/web")),
    ];
    // Every operation that resolves a `taskDefinition` reference states the same sentence.
    let operations: [OperationRow; 4] = [
        ("DescribeTaskDefinition", |reference| {
            json!({ "taskDefinition": reference })
        }),
        ("DeregisterTaskDefinition", |reference| {
            json!({ "taskDefinition": reference })
        }),
        ("RunTask", |reference| {
            json!({ "cluster": CLUSTER, "taskDefinition": reference })
        }),
        ("CreateService", |reference| {
            json!({
                "cluster": CLUSTER,
                "serviceName": "from-a-bad-reference",
                "taskDefinition": reference,
                "desiredCount": 0,
                "launchType": "EC2",
            })
        }),
    ];
    for (operation, build) in operations {
        for identifier in identifiers {
            let case = format!("{operation} naming {identifier:?}");
            let (code, message) = refusal(&router, operation, build(identifier), &case).await;
            assert_eq!(
                code, "ClientException",
                "case {case:?}: expected a ClientException, got {code} with {message:?}"
            );
            assert_eq!(
                message, FAMILY_CHARACTERS,
                "case {case:?}: expected {FAMILY_CHARACTERS:?}, got {message:?}"
            );
            assert!(
                !message.contains(identifier),
                "case {case:?}: the sentence is fixed text and must carry nothing of the \
                 request, got {message:?}"
            );
        }
    }

    // A reference whose family IS a family name still resolves, so the refusal above is
    // about the characters rather than about every value that names no revision.
    let body = ok(
        &router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": FAMILY }),
        "stored",
    )
    .await;
    assert_eq!(
        string(&body["taskDefinition"], "family", "stored"),
        FAMILY,
        "case \"stored\": expected the stored revision to be described, got {body}"
    );
}
