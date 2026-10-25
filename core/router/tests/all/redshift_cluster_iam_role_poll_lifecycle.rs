// SPDX-License-Identifier: CC0-1.0

//! A Redshift cluster's IAM roles are ARNs ANOTHER service mints, and every call that
//! changes them leaves work the clock owes — so a client's poll loop is the only way to
//! see the change land.
//!
//! RULE: every clock-owed transition a mutating cluster call starts (`creating` /
//! `modifying` -> `available` on the cluster, `adding` / `removing` -> `in-sync` /
//! gone on an association) becomes due a bounded span after the call that started it,
//! so a caller polling a reporting operation reaches the settled state in bounded time
//! WITHOUT making another mutating call. An IAM role is another service's resource, so
//! this service never resolves one: a well-formed ARN naming a role that was never
//! created is associated like any other.
//!
//! This needs BOTH services: the role ARN comes from IAM's `CreateRole`, which is what a
//! client really does before attaching a role to a cluster.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// A trust policy that lets Amazon Redshift assume the role — the document a client
/// creating a role for a cluster really sends.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"redshift.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// How long a poll loop is given before the suite calls the transition unobservable.
///
/// Generous next to the span a clock-owed transition is due after, so a loaded runner
/// cannot fail the suite, and still far short of a client's patience.
const POLL_DEADLINE: Duration = Duration::from_secs(20);

/// The gap between two polls of the same loop.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A SigV4 `Authorization` header whose credential scope names `signing_name`; the
/// signature is never checked, because the router here runs with request
/// authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=deadbeef"
    )
}

/// One awsQuery call, answered with the status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "emulator.local")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(bytes::Bytes::from(body))
        .expect("a well-formed query request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    (status, body)
}

/// One Redshift call.
async fn redshift(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    query(router, "redshift", "2012-12-01", action, params).await
}

/// The `(IamRoleArn, ApplyStatus)` of every association an answer's `IamRoles` list
/// carries, in report order.
fn associations(body: &str) -> Vec<(String, String)> {
    let Some(block) = opt_element(body, "IamRoles") else {
        return Vec::new();
    };
    elements(&block, "IamRoleArn")
        .into_iter()
        .zip(elements(&block, "ApplyStatus"))
        .collect()
}

/// The node type of the first orderable option offering the single-node cluster type,
/// read out of the catalog rather than written here: the suite creates the cluster the
/// service says is orderable.
fn single_node_node_type(body: &str) -> String {
    for option in body.split("<OrderableClusterOption>").skip(1) {
        if opt_element(option, "ClusterType").as_deref() == Some("single-node") {
            return element(option, "NodeType");
        }
    }
    panic!("the orderable catalog offers no single-node option: {body}")
}

/// Poll `DescribeClusters` until `settled` holds, answering the body it held on and how
/// many polls it took; panics with the last body when the deadline passes first.
///
/// This is the shape of the loop a real client writes, and the thing under test: it
/// makes no mutating call, so only a transition the clock owes can end it.
async fn poll_until(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    settled: impl Fn(&str) -> bool,
) -> (String, usize) {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut polls = 0usize;
    let mut last = String::new();
    while Instant::now() < deadline {
        let (status, body) = redshift(router, "DescribeClusters", &[("ClusterIdentifier", cluster)]).await;
        polls += 1;
        assert_eq!(
            status, 200,
            "case {case:?}: poll {polls} expected DescribeClusters 200, got {status}: {body}"
        );
        if settled(&body) {
            return (body, polls);
        }
        last = body;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the poll loop never settled in {POLL_DEADLINE:?} ({polls} polls); last answer: {last}");
}

/// One row of the role table: the case name, the ARNs the call associates, the
/// ARNs it disassociates, and the association set the cluster must carry once the
/// change has settled.
type RoleCase<'a> = (&'a str, &'a [&'a str], &'a [&'a str], &'a [&'a str]);

#[test]
fn a_cluster_and_its_iam_role_associations_settle_for_a_polling_client() {
    std::thread::Builder::new()
        .name("redshift-cluster-iam-roles".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("a current-thread runtime");

            runtime.block_on(async {
                // 1. The role the cluster will carry, minted by IAM.
                let (status, created_role) = query(
                    &router,
                    "iam",
                    "2010-05-08",
                    "CreateRole",
                    &[
                        ("RoleName", "redshift-probe-role"),
                        ("AssumeRolePolicyDocument", TRUST_POLICY),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case \"the role a cluster carries\": expected CreateRole 200, got {status}: {created_role}"
                );
                let role_arn = element(&created_role, "Arn");
                // A well-formed ARN naming a role nothing created: IAM holds no such
                // role, and Redshift associates it all the same.
                let missing_role_arn = format!("{role_arn}-does-not-exist");

                // 2. The node type to create, taken from the catalog.
                let (status, catalog) =
                    redshift(&router, "DescribeOrderableClusterOptions", &[]).await;
                assert_eq!(
                    status, 200,
                    "case \"the orderable catalog\": expected 200, got {status}: {catalog}"
                );
                let node_type = single_node_node_type(&catalog);

                // 3. The cluster.
                let cluster = "redshift-probe-cluster";
                let (status, created) = redshift(
                    &router,
                    "CreateCluster",
                    &[
                        ("ClusterIdentifier", cluster),
                        ("NodeType", &node_type),
                        ("MasterUsername", "adminuser"),
                        ("MasterUserPassword", "Probe-Password-1"),
                        ("ClusterType", "single-node"),
                        ("DBName", "devdb"),
                        ("PubliclyAccessible", "false"),
                        ("Encrypted", "true"),
                        ("AutomatedSnapshotRetentionPeriod", "1"),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case \"the create\": expected CreateCluster 200, got {status}: {created}"
                );

                // 4. A poll loop reaches the settled cluster, and what it reports there.
                let (available, polls) = poll_until(&router, "after the create", cluster, |body| {
                    opt_element(body, "ClusterStatus").as_deref() == Some("available")
                })
                .await;
                assert!(
                    polls >= 1,
                    "case \"after the create\": expected at least one poll, got {polls}"
                );
                assert_eq!(
                    opt_element(&available, "ClusterAvailabilityStatus").as_deref(),
                    Some("Available"),
                    "case \"after the create\": expected ClusterAvailabilityStatus=Available, got {available}"
                );
                assert_eq!(
                    opt_element(&available, "AvailabilityZoneRelocationStatus").as_deref(),
                    Some("enabled"),
                    "case \"after the create\": expected AvailabilityZoneRelocationStatus=enabled, got {available}"
                );
                assert!(
                    associations(&available).is_empty(),
                    "case \"after the create\": expected no association, got {:?}",
                    associations(&available)
                );

                // 5. Each role change: the call reports the work in progress, and the
                // poll loop reaches the settled association set.
                //
                // Rows: the real role, then the ARN of a role that does not exist (which
                // is accepted like any other), then both disassociated at once.
                let rows: [RoleCase<'_>; 3] = [
                    ("the real role attached", &[role_arn.as_str()], &[], &[role_arn.as_str()]),
                    (
                        "an ARN naming no role attached",
                        &[missing_role_arn.as_str()],
                        &[],
                        &[role_arn.as_str(), missing_role_arn.as_str()],
                    ),
                    (
                        "both detached",
                        &[],
                        &[role_arn.as_str(), missing_role_arn.as_str()],
                        &[],
                    ),
                ];

                for (case, add, remove, settled) in rows {
                    let mut params: Vec<(String, String)> =
                        vec![("ClusterIdentifier".to_string(), cluster.to_string())];
                    for (position, arn) in add.iter().enumerate() {
                        params.push((
                            format!("AddIamRoles.IamRoleArn.{}", position + 1),
                            (*arn).to_string(),
                        ));
                    }
                    for (position, arn) in remove.iter().enumerate() {
                        params.push((
                            format!("RemoveIamRoles.IamRoleArn.{}", position + 1),
                            (*arn).to_string(),
                        ));
                    }
                    let borrowed: Vec<(&str, &str)> = params
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.as_str()))
                        .collect();
                    let (status, answered) =
                        redshift(&router, "ModifyClusterIamRoles", &borrowed).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected ModifyClusterIamRoles 200, got {status}: {answered}"
                    );
                    assert_ne!(
                        opt_element(&answered, "ClusterStatus").as_deref(),
                        Some("available"),
                        "case {case:?}: a call that asked for a change leaves it being applied, \
                         got {answered}"
                    );

                    let expected: Vec<(String, String)> = settled
                        .iter()
                        .map(|arn| ((*arn).to_string(), "in-sync".to_string()))
                        .collect();
                    let (read, polls) = poll_until(&router, case, cluster, |body| {
                        opt_element(body, "ClusterStatus").as_deref() == Some("available")
                            && associations(body) == expected
                    })
                    .await;
                    assert_eq!(
                        associations(&read),
                        expected,
                        "case {case:?}: expected {expected:?} after {polls} poll(s), got {:?}",
                        associations(&read)
                    );
                }
            });
        })
        .expect("spawn the sized test thread")
        .join()
        .expect("the test thread must not panic");
}
