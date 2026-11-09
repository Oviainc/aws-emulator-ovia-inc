// SPDX-License-Identifier: CC0-1.0

//! The delete ordering PCS holds a cluster's resources to, end to end over the awsJson1_0
//! wire (`POST /` with `X-Amz-Target: AWSParallelComputingService.<Operation>`) through
//! the same router the emulator serves from.
//!
//! RULE under test: a PCS resource can be deleted only once nothing of its cluster still
//! depends on it — "You must delete all queues associated with the compute node group
//! first" (`DeleteComputeNodeGroup`) and "You must delete all queues and compute node
//! groups associated with the cluster before you can delete the cluster"
//! (`DeleteCluster`) — and a delete attempted out of that order is refused with
//! `ConflictException` (HTTP 409) while the resource it names stays exactly as it was. A
//! delete made in order is accepted, reports the resource `DELETING`, and the clock then
//! removes it, so the read answers `ResourceNotFoundException` (404) and the listing drops
//! it.
//!
//! The suites are tables. Their rows differ along what the rule generalises over: the form
//! of the identifier each call addresses a resource by (its generated id or its name, the
//! two forms the `*Identifier` patterns allow), the form a queue names a compute node group
//! by, and how many queues depend on one group — rather than the single shape the recorded
//! workflow happened to use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_lifecycle::{call, emulator, wait_past_the_owed_transition};

/// The region the shared harness signs every call for.
const REGION: &str = "us-east-1";

/// How a resource is addressed by a call: by the id PCS generated for it, or by the name
/// the create chose — the two forms `ClusterIdentifier`, `ComputeNodeGroupIdentifier` and
/// `QueueIdentifier` all allow
/// (`smithy_query(query="shape", shape="ComputeNodeGroupIdentifier")`:
/// `^(pcs_[a-zA-Z0-9]+|[A-Za-z][A-Za-z0-9-]{2,25})$`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    /// The generated `pcs_...` id.
    Id,
    /// The caller-chosen name.
    Name,
}

/// One created resource: its generated id and the name it was created under.
#[derive(Debug, Clone)]
struct Created {
    id: String,
    name: String,
}

impl Created {
    /// How `address` names this resource.
    fn as_addressed(&self, address: Address) -> &str {
        match address {
            Address::Id => &self.id,
            Address::Name => &self.name,
        }
    }
}

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// Create a cluster and wait until the clock has made it ACTIVE.
async fn active_cluster(router: &MultiServiceRouter, case: &str, name: &str) -> Created {
    let body = json!({
        "clusterName": name,
        "scheduler": {"type": "SLURM", "version": "24.11"},
        "size": "SMALL",
        "networking": {
            "subnetIds": ["subnet-0123456789abcdef0"],
            "securityGroupIds": ["sg-0123456789abcdef0"],
        },
    });
    let (status, created) = call(router, "CreateCluster", &body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateCluster must succeed, got {status} with {created}"
    );
    wait_past_the_owed_transition().await;
    Created {
        id: created["cluster"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        name: name.to_string(),
    }
}

/// Create a compute node group in `cluster` and wait until it is ACTIVE.
async fn active_group(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    name: &str,
) -> Created {
    let body = json!({
        "clusterIdentifier": cluster,
        "computeNodeGroupName": name,
        "subnetIds": ["subnet-0123456789abcdef0"],
        "customLaunchTemplate": {"id": "lt-0123456789abcdef0", "version": "1"},
        "iamInstanceProfileArn": format!(
            "arn:aws:iam::{}:instance-profile/AWSPCS-demo",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "scalingConfiguration": {"minInstanceCount": 0, "maxInstanceCount": 4},
        "instanceConfigs": [{"instanceType": "c6i.large"}],
    });
    let (status, created) = call(router, "CreateComputeNodeGroup", &body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateComputeNodeGroup must succeed, got {status} with {created}"
    );
    wait_past_the_owed_transition().await;
    Created {
        id: created["computeNodeGroup"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        name: name.to_string(),
    }
}

/// Create a queue in `cluster` assigning jobs to each of `groups` (each written the way
/// that row addresses it), and wait until it is ACTIVE.
async fn active_queue(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    name: &str,
    groups: &[&str],
) -> Created {
    let body = json!({
        "clusterIdentifier": cluster,
        "queueName": name,
        "computeNodeGroupConfigurations": groups
            .iter()
            .map(|named| json!({"computeNodeGroupId": named}))
            .collect::<Vec<Value>>(),
    });
    let (status, created) = call(router, "CreateQueue", &body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateQueue must succeed, got {status} with {created}"
    );
    wait_past_the_owed_transition().await;
    Created {
        id: created["queue"]["id"].as_str().unwrap_or_default().to_string(),
        name: name.to_string(),
    }
}

/// Assert `operation` on `body` is refused with `ConflictException` carrying `message`,
/// `resourceId` and `resourceType`.
async fn assert_conflict(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    message: &str,
    resource_id: &str,
    resource_type: &str,
) {
    let (status, refused) = call(router, operation, body).await;
    assert_eq!(
        status, 409,
        "case {case}: expected {operation} {body} to be refused 409, got {status} with \
         {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictException",
        "case {case}: expected ConflictException from {operation}, got {refused}"
    );
    assert_eq!(
        refused["message"].as_str(),
        Some(message),
        "case {case}: expected the service's own sentence from {operation}, got {refused}"
    );
    assert_eq!(
        (
            refused["resourceId"].as_str(),
            refused["resourceType"].as_str()
        ),
        (Some(resource_id), Some(resource_type)),
        "case {case}: expected {operation} to name the resource it refused, got {refused}"
    );
}

/// The sentence PCS answers a compute node group delete with while a queue still assigns
/// jobs to it.
const GROUP_STILL_REFERENCED: &str = "AWS PCS can't delete the compute node group you \
                                      specified because the cluster has associated queues.";

/// The sentence PCS answers a cluster delete with while the cluster still holds a queue or
/// a compute node group.
const CLUSTER_STILL_HAS_CHILDREN: &str =
    "AWS PCS can't delete the cluster you specified because it has associated queues or \
     compute node groups.";

/// RULE: out of order, each delete is refused with `ConflictException` and changes nothing;
/// in dependency order, each delete is accepted, reports `DELETING`, and the clock then
/// removes the resource.
#[tokio::test]
async fn a_dependent_resource_blocks_the_delete_until_it_is_gone() {
    // Each row: how the delete calls address the resources, how the queue names the group,
    // and how many queues depend on that group.
    let rows: &[(&str, Address, Address, usize)] = &[
        ("addressed by id, one queue naming the group by id", Address::Id, Address::Id, 1),
        (
            "addressed by name, one queue naming the group by name",
            Address::Name,
            Address::Name,
            1,
        ),
        (
            "addressed by id, two queues naming the group by name",
            Address::Id,
            Address::Name,
            2,
        ),
    ];

    for (case, address, reference, queue_count) in rows {
        let router = emulator();
        let cluster = active_cluster(&router, case, "ordering-cluster").await;
        let group = active_group(&router, case, &cluster.id, "nodes").await;
        // A SECOND group nothing depends on, so the refusal below is shown to be keyed on
        // the queue's actual reference rather than on "a queue exists in this cluster".
        let unused = active_group(&router, case, &cluster.id, "spare-nodes").await;

        let mut queues = Vec::new();
        for index in 0..*queue_count {
            queues.push(
                active_queue(
                    &router,
                    case,
                    cluster.as_addressed(*address),
                    &format!("work-{index}"),
                    &[group.as_addressed(*reference)],
                )
                .await,
            );
        }

        let delete_group = json!({
            "clusterIdentifier": cluster.as_addressed(*address),
            "computeNodeGroupIdentifier": group.as_addressed(*address),
        });
        assert_conflict(
            &router,
            case,
            "DeleteComputeNodeGroup",
            &delete_group,
            GROUP_STILL_REFERENCED,
            &group.id,
            "computenodegroup",
        )
        .await;
        assert_conflict(
            &router,
            case,
            "DeleteCluster",
            &json!({"clusterIdentifier": cluster.as_addressed(*address)}),
            CLUSTER_STILL_HAS_CHILDREN,
            &cluster.id,
            "cluster",
        )
        .await;

        // The refused deletes changed nothing: both resources are still ACTIVE.
        for (operation, body, member) in [
            (
                "GetComputeNodeGroup",
                delete_group.clone(),
                "computeNodeGroup",
            ),
            (
                "GetCluster",
                json!({"clusterIdentifier": cluster.as_addressed(*address)}),
                "cluster",
            ),
        ] {
            let (status, read) = call(&router, operation, &body).await;
            assert_eq!(
                (status, read[member]["status"].as_str()),
                (200, Some("ACTIVE")),
                "case {case}: expected {operation} to still report ACTIVE after the refused \
                 delete, got {status} with {read}"
            );
        }

        // The group nothing depends on deletes while the queues are still there.
        let (status, deleted) = call(
            &router,
            "DeleteComputeNodeGroup",
            &json!({
                "clusterIdentifier": cluster.as_addressed(*address),
                "computeNodeGroupIdentifier": unused.as_addressed(*address),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the unreferenced group to delete, got {status} with \
             {deleted}"
        );

        // In order: every queue, then the group, then the cluster.
        for queue in &queues {
            let body = json!({
                "clusterIdentifier": cluster.as_addressed(*address),
                "queueIdentifier": queue.as_addressed(*address),
            });
            let (status, deleted) = call(&router, "DeleteQueue", &body).await;
            assert_eq!(
                status, 200,
                "case {case}: expected DeleteQueue {body} to succeed, got {status} with \
                 {deleted}"
            );
            let (status, read) = call(&router, "GetQueue", &body).await;
            assert_eq!(
                (status, read["queue"]["status"].as_str()),
                (200, Some("DELETING")),
                "case {case}: expected the queue to be reported DELETING straight after the \
                 delete, got {status} with {read}"
            );
            // Deleting it again, while it is on its way out, conflicts.
            let (status, again) = call(&router, "DeleteQueue", &body).await;
            assert_eq!(
                (status, error_code(&again).as_str()),
                (409, "ConflictException"),
                "case {case}: expected a second DeleteQueue to conflict, got {status} with \
                 {again}"
            );
        }

        wait_past_the_owed_transition().await;
        for queue in &queues {
            let body = json!({
                "clusterIdentifier": cluster.as_addressed(*address),
                "queueIdentifier": queue.as_addressed(*address),
            });
            let (status, gone) = call(&router, "GetQueue", &body).await;
            assert_eq!(
                (status, error_code(&gone).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case}: expected the queue to be gone, got {status} with {gone}"
            );
        }
        let (status, listed) = call(
            &router,
            "ListQueues",
            &json!({"clusterIdentifier": cluster.as_addressed(*address)}),
        )
        .await;
        assert_eq!(
            (status, listed["queues"].as_array().map(Vec::len)),
            (200, Some(0)),
            "case {case}: expected no queue left in the cluster, got {status} with {listed}"
        );

        let (status, deleted) = call(&router, "DeleteComputeNodeGroup", &delete_group).await;
        assert_eq!(
            status, 200,
            "case {case}: expected DeleteComputeNodeGroup to succeed once no queue depends \
             on it, got {status} with {deleted}"
        );
        let (status, read) = call(&router, "GetComputeNodeGroup", &delete_group).await;
        assert_eq!(
            (status, read["computeNodeGroup"]["status"].as_str()),
            (200, Some("DELETING")),
            "case {case}: expected the group to be reported DELETING straight after the \
             delete, got {status} with {read}"
        );

        wait_past_the_owed_transition().await;
        // The group's delete has come due: the one resource of the three whose status enum
        // declares DELETED is reported DELETED before it is forgotten.
        let (status, read) = call(&router, "GetComputeNodeGroup", &delete_group).await;
        assert_eq!(
            (status, read["computeNodeGroup"]["status"].as_str()),
            (200, Some("DELETED")),
            "case {case}: expected the group to be reported DELETED once its delete came \
             due, got {status} with {read}"
        );

        wait_past_the_owed_transition().await;
        let (status, gone) = call(&router, "GetComputeNodeGroup", &delete_group).await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected the group to be gone, got {status} with {gone}"
        );
        let (status, listed) = call(
            &router,
            "ListComputeNodeGroups",
            &json!({"clusterIdentifier": cluster.as_addressed(*address)}),
        )
        .await;
        assert_eq!(
            (
                status,
                listed["computeNodeGroups"].as_array().map(Vec::len)
            ),
            (200, Some(0)),
            "case {case}: expected no compute node group left in the cluster, got {status} \
             with {listed}"
        );

        let cluster_body = json!({"clusterIdentifier": cluster.as_addressed(*address)});
        let (status, deleted) = call(&router, "DeleteCluster", &cluster_body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected DeleteCluster to succeed once the cluster is empty, got \
             {status} with {deleted}"
        );
        wait_past_the_owed_transition().await;
        let (status, gone) = call(&router, "GetCluster", &cluster_body).await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected the cluster to be gone, got {status} with {gone}"
        );
    }
}

/// RULE: the delete order is about DEPENDENTS, not about waiting — a caller that deletes a
/// queue, then its compute node group, then the cluster back to back, with no wait between
/// the calls, is served every time, and the clock then removes all three.
#[tokio::test]
async fn deleting_the_hierarchy_back_to_back_is_served() {
    let router = emulator();
    let case = "back-to-back ordered deletes";
    let cluster = active_cluster(&router, case, "rapid-cluster").await;
    let group = active_group(&router, case, &cluster.id, "nodes").await;
    let queue = active_queue(&router, case, &cluster.id, "work", &[&group.id]).await;

    // Each row: the operation and the request, issued one after the other with no wait.
    let rows: Vec<(&str, Value)> = vec![
        (
            "DeleteQueue",
            json!({
                "clusterIdentifier": cluster.id.clone(),
                "queueIdentifier": queue.id.clone(),
            }),
        ),
        (
            "DeleteComputeNodeGroup",
            json!({
                "clusterIdentifier": cluster.id.clone(),
                "computeNodeGroupIdentifier": group.id.clone(),
            }),
        ),
        (
            "DeleteCluster",
            json!({"clusterIdentifier": cluster.id.clone()}),
        ),
    ];
    for (operation, body) in &rows {
        let (status, deleted) = call(&router, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} {body} to be served straight after the \
             previous delete, got {status} with {deleted}"
        );
    }

    // The clock then removes all three, so every read of them is a miss.
    wait_past_the_owed_transition().await;
    wait_past_the_owed_transition().await;
    for (operation, body) in [
        (
            "GetQueue",
            json!({
                "clusterIdentifier": cluster.id.clone(),
                "queueIdentifier": queue.id.clone(),
            }),
        ),
        (
            "GetComputeNodeGroup",
            json!({
                "clusterIdentifier": cluster.id.clone(),
                "computeNodeGroupIdentifier": group.id.clone(),
            }),
        ),
        (
            "GetCluster",
            json!({"clusterIdentifier": cluster.id.clone()}),
        ),
    ] {
        let (status, gone) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected {operation} to report the resource gone, got {status} \
             with {gone}"
        );
    }
}

/// RULE: a delete names a cluster and a resource inside it, so a name neither holds is
/// `ResourceNotFoundException` (404) naming the resource the request could not resolve:
/// `resourceId` is the identifier as supplied and `resourceType` the KIND of identifier
/// that did not resolve.
#[tokio::test]
async fn a_delete_of_a_resource_nothing_holds_is_not_found() {
    let router = emulator();
    let case = "a delete of what nothing holds";
    let cluster = active_cluster(&router, case, "lonely-cluster").await;

    // Each row: the operation, the request, the identifier that does not resolve, and the
    // `resourceType` the refusal reports for it. Each operation resolves the cluster
    // first, so a row naming no cluster is refused about the cluster whichever resource
    // inside one it also names.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "DeleteQueue",
            json!({
                "clusterIdentifier": "no-such-cluster",
                "queueIdentifier": "work",
            }),
            "no-such-cluster",
            "Cluster ID",
        ),
        (
            "DeleteQueue",
            json!({
                "clusterIdentifier": cluster.id.clone(),
                "queueIdentifier": "no-such-queue",
            }),
            "no-such-queue",
            "Queue ID",
        ),
        (
            "DeleteComputeNodeGroup",
            json!({
                "clusterIdentifier": "no-such-cluster",
                "computeNodeGroupIdentifier": "nodes",
            }),
            "no-such-cluster",
            "Cluster ID",
        ),
        (
            "DeleteComputeNodeGroup",
            json!({
                "clusterIdentifier": cluster.id.clone(),
                "computeNodeGroupIdentifier": "no-such-group",
            }),
            "no-such-group",
            "Compute Node Group ID",
        ),
        (
            "ListComputeNodeGroups",
            json!({"clusterIdentifier": "no-such-cluster"}),
            "no-such-cluster",
            "Cluster ID",
        ),
    ];

    for (operation, body, identifier, resource_type) in rows {
        let (status, refused) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected {operation} {body} to answer 404, got {status} with \
             {refused}"
        );
        // The refusal names the identifier it could not resolve and the kind of
        // identifier that was.
        assert_eq!(
            (
                refused["resourceId"].as_str(),
                refused["resourceType"].as_str()
            ),
            (Some(identifier), Some(resource_type)),
            "case {case}: expected {operation} to name the identifier it could not \
             resolve, got {refused}"
        );
    }
}

/// RULE: a compute node group listing reports the groups of its own cluster in creation
/// order, one page at a time, and resumes exactly where a page stopped.
#[tokio::test]
async fn the_compute_node_group_listing_pages_in_creation_order() {
    let router = emulator();
    let case = "paging a cluster's compute node groups";
    let cluster = active_cluster(&router, case, "listing-cluster").await;
    let mut created = Vec::new();
    for index in 0..3 {
        created.push(
            active_group(&router, case, &cluster.id, &format!("nodes-{index}"))
                .await
                .id,
        );
    }

    // Each row: the page size asked for.
    for page_size in [1, 2, 3, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({
                "clusterIdentifier": cluster.id.clone(),
                "maxResults": page_size,
            });
            if let Some(resume) = &token {
                body["nextToken"] = json!(resume);
            }
            let (status, page) = call(&router, "ListComputeNodeGroups", &body).await;
            assert_eq!(
                status, 200,
                "case {case} at maxResults={page_size}: expected 200, got {status} with \
                 {page}"
            );
            let groups = page["computeNodeGroups"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                groups.len() <= page_size,
                "case {case} at maxResults={page_size}: expected at most {page_size} \
                 groups in a page, got {page}"
            );
            for group in &groups {
                assert_eq!(
                    group["clusterId"].as_str(),
                    Some(cluster.id.as_str()),
                    "case {case}: expected every listed group to belong to this cluster, \
                     got {group}"
                );
                assert!(
                    group["arn"]
                        .as_str()
                        .is_some_and(|arn| arn.contains(&format!(":pcs:{REGION}:"))),
                    "case {case}: expected the group ARN to carry the signed region, got \
                     {group}"
                );
                collected.push(group["id"].as_str().unwrap_or_default().to_string());
            }
            match page["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
            assert!(
                collected.len() <= created.len(),
                "case {case} at maxResults={page_size}: paging reported more groups than \
                 the cluster holds: {collected:?}"
            );
        }
        assert_eq!(
            collected, created,
            "case {case} at maxResults={page_size}: expected the groups in creation order"
        );
    }

    // A token no listing of this cluster issued names no page.
    let (status, refused) = call(
        &router,
        "ListComputeNodeGroups",
        &json!({
            "clusterIdentifier": cluster.id.clone(),
            "nextToken": "not-a-token-this-listing-issued",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case {case}: expected an unissued token to be refused, got {status} with {refused}"
    );
}
