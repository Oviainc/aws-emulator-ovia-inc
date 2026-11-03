// SPDX-License-Identifier: CC0-1.0

//! Amazon EKS provisions the cluster's own security group in EC2, and releases it with
//! the cluster.
//!
//! Two rules are under test, and both need EKS and EC2 on one engine — EKS reaches EC2
//! through the in-process callback, so a single-service router has nothing to ask and
//! nothing to create in — which is why this suite lives here rather than in the EKS
//! crate:
//!
//! * THE CLUSTER SECURITY GROUP. Creating a cluster mints exactly ONE security group of
//!   its own, in the VPC the subnets it was created in belong to: named
//!   `eks-cluster-sg-<cluster>-<ten digits>`, carrying the cluster's name under AWS's own
//!   `aws:eks:cluster-name` tag and the Kubernetes `kubernetes.io/cluster/<cluster>`
//!   ownership tag, reported by the cluster as its `clusterSecurityGroupId` beside the
//!   `vpcId` it resolved. `DeleteCluster` releases that group, and leaves every other
//!   cluster's group alone. Rows span which VPC the subnets sit in (two the client
//!   created and the account's own default one), how many subnets a create names, and the
//!   characters a cluster name is built from — the group's name is built out of it.
//! * THE WRAPPED TAIL OF A PEER'S REFUSAL. When EKS refuses a create because a call it
//!   made to EC2 on the caller's behalf failed, the message carries that call's whole
//!   wrapped form: the service, the HTTP status, the error code, the id of the call and
//!   the proxy it went through. Rows span both creates that resolve subnets
//!   (`CreateCluster`, `CreateNodegroup`) and both of EC2's subnet-id forms, and two
//!   refusals are compared to each other: a request id names ONE call, so two refusals do
//!   not share one.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and
//! the requests driven — on a sized thread.
//!
//! The request harness is the test crate's own: `eks_wire` for the drivers and the answer,
//! `query_wire` for the XML readers.

use std::collections::BTreeMap;
use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::eks_wire::{cluster_body, ec2, eks, role_arn, Answer, EC2_NOT_FOUND_CODE};
use crate::query_wire::{element, elements, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The tag Amazon EKS stamps on the EC2 resources a cluster owns, naming that cluster.
const CLUSTER_NAME_TAG: &str = "aws:eks:cluster-name";

/// The prefix of the Kubernetes cluster-ownership tag.
const KUBERNETES_CLUSTER_TAG_PREFIX: &str = "kubernetes.io/cluster/";

/// The name prefix Amazon EKS gives the group it creates with a cluster.
const GROUP_NAME_PREFIX: &str = "eks-cluster-sg-";

/// The decimal width of the numeric suffix of that name.
const GROUP_NAME_SUFFIX_DIGITS: usize = 10;

/// Every `<key>`/`<value>` pair a `<tagSet>` reports, as a map.
///
/// A tag's two members are written in that order and nothing else in a security group's
/// body carries a `<key>`, so the keys and the values line up position by position.
fn tag_map(xml: &str) -> BTreeMap<String, String> {
    let keys = elements(xml, "key");
    let values = elements(xml, "value");
    keys.into_iter().zip(values).collect()
}

/// The answer to the poll a client makes to find the group a cluster owns:
/// `DescribeSecurityGroups` narrowed to the cluster-name tag of `cluster`.
fn groups_of_cluster(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    cluster: &str,
) -> Answer {
    let filter_name = format!("tag:{CLUSTER_NAME_TAG}");
    let answer = ec2(
        router,
        runtime,
        "DescribeSecurityGroups",
        &[
            ("Filter.1.Name", filter_name.as_str()),
            ("Filter.1.Value.1", cluster),
        ],
    );
    assert_eq!(
        answer.status, 200,
        "case {case:?}: DescribeSecurityGroups must succeed, got {answer:?}"
    );
    answer
}

/// One row of the security-group table: what a cluster is created with.
struct ClusterCase {
    case: &'static str,
    cluster: &'static str,
    /// Which network the subnets come from, resolved in the test body.
    network: Network,
    /// How many of that network's subnets the create names.
    subnet_count: usize,
}

/// Which of the account's networks a row's subnets are drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Network {
    /// The first VPC the test created.
    First,
    /// A second VPC the test created, so the group follows the subnets rather than
    /// landing in whichever VPC happens to be first.
    Second,
    /// The account's own default VPC, which nothing in this test created.
    Default,
}

/// The ten-digit suffix of a group name, when the name is the one Amazon EKS builds for
/// `cluster`.
fn group_name_suffix(case: &str, cluster: &str, group_name: &str) -> String {
    let prefix = format!("{GROUP_NAME_PREFIX}{cluster}-");
    let suffix = group_name.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!(
            "case {case:?}: the group must be named {prefix:?} plus a suffix, \
             got {group_name:?}"
        )
    });
    assert!(
        suffix.len() == GROUP_NAME_SUFFIX_DIGITS
            && suffix.bytes().all(|byte| byte.is_ascii_digit()),
        "case {case:?}: the group name's suffix must be {GROUP_NAME_SUFFIX_DIGITS} decimal \
         digits, got {suffix:?} in {group_name:?}"
    );
    suffix.to_string()
}

#[test]
fn eks_mints_the_cluster_security_group_in_ec2_and_releases_it_on_delete() {
    std::thread::Builder::new()
        .name("eks-cluster-security-group".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // ---------------------------------------------------------------
            // Setup: two VPCs with subnets of their own, plus the account's own
            // default network, all through EC2 itself.
            // ---------------------------------------------------------------
            let mut networks: Vec<(Network, String, Vec<String>)> = Vec::new();
            for (network, vpc_cidr, subnet_cidrs) in [
                (
                    Network::First,
                    "10.7.0.0/16",
                    ["10.7.1.0/24", "10.7.2.0/24", "10.7.3.0/24"],
                ),
                (
                    Network::Second,
                    "10.8.0.0/16",
                    ["10.8.1.0/24", "10.8.2.0/24", "10.8.3.0/24"],
                ),
            ] {
                let created = ec2(&router, &runtime, "CreateVpc", &[("CidrBlock", vpc_cidr)]);
                assert_eq!(
                    created.status, 200,
                    "setup: CreateVpc {vpc_cidr} must succeed, got {created:?}"
                );
                let vpc_id = element(&created.text, "vpcId");
                let mut subnets = Vec::new();
                for cidr_block in subnet_cidrs {
                    let answer = ec2(
                        &router,
                        &runtime,
                        "CreateSubnet",
                        &[("VpcId", vpc_id.as_str()), ("CidrBlock", cidr_block)],
                    );
                    assert_eq!(
                        answer.status, 200,
                        "setup: CreateSubnet {cidr_block} must succeed, got {answer:?}"
                    );
                    subnets.push(element(&answer.text, "subnetId"));
                }
                networks.push((network, vpc_id, subnets));
            }
            // The account's default subnets: a network nothing in this test created, so
            // the rule is shown to hold for one the account simply HAS as well.
            let defaults = ec2(
                &router,
                &runtime,
                "DescribeSubnets",
                &[
                    ("Filter.1.Name", "default-for-az"),
                    ("Filter.1.Value.1", "true"),
                ],
            );
            assert_eq!(
                defaults.status, 200,
                "setup: DescribeSubnets must succeed, got {defaults:?}"
            );
            let default_subnets = elements(&defaults.text, "subnetId");
            let default_vpc = element(&defaults.text, "vpcId");
            assert!(
                default_subnets.len() >= 2,
                "setup: the account must have at least two default subnets, got {default_subnets:?}"
            );
            networks.push((Network::Default, default_vpc, default_subnets));

            // ---------------------------------------------------------------
            // RULE: a create mints one group of the cluster's own in the VPC its
            // subnets belong to.
            // ---------------------------------------------------------------
            let rows = [
                ClusterCase {
                    case: "two subnets of the first VPC",
                    cluster: "sg-two-subnets",
                    network: Network::First,
                    subnet_count: 2,
                },
                ClusterCase {
                    case: "three subnets of the first VPC",
                    cluster: "sg-three-subnets",
                    network: Network::First,
                    subnet_count: 3,
                },
                ClusterCase {
                    case: "subnets of a second VPC, so the group follows the subnets",
                    cluster: "sg-second-vpc",
                    network: Network::Second,
                    subnet_count: 2,
                },
                ClusterCase {
                    case: "the account's own default subnets",
                    cluster: "sg-default-vpc",
                    network: Network::Default,
                    subnet_count: 2,
                },
                ClusterCase {
                    case: "a cluster name of every character a name may carry",
                    cluster: "Cluster_9-name",
                    network: Network::Second,
                    subnet_count: 3,
                },
            ];

            // What each row's cluster ended up owning, so the delete half can check that
            // releasing one group leaves the others in place.
            let mut owned: Vec<(&str, &str, String)> = Vec::new();
            for row in &rows {
                let (_, vpc_id, subnets) = networks
                    .iter()
                    .find(|(network, _, _)| *network == row.network)
                    .expect("every row names a network the setup created");
                let named: Vec<String> = subnets
                    .iter()
                    .take(row.subnet_count)
                    .cloned()
                    .collect();
                let created = eks(
                    &router,
                    &runtime,
                    "POST",
                    "/clusters",
                    Some(cluster_body(row.cluster, &named)),
                );
                assert_eq!(
                    created.status, 200,
                    "case {:?}: the cluster must be created, got {created:?}",
                    row.case
                );
                // The group is minted while the cluster is still being created, which is
                // the state the create itself reports.
                assert_eq!(
                    created.body["cluster"]["status"],
                    serde_json::json!("CREATING"),
                    "case {:?}: a new cluster is reported CREATING, got {}",
                    row.case,
                    created.text
                );

                let found = groups_of_cluster(&router, &runtime, row.case, row.cluster);
                // One `<groupName>` is one group: a group with no rules naming another
                // group carries that element exactly once.
                let names = elements(&found.text, "groupName");
                assert_eq!(
                    names.len(),
                    1,
                    "case {:?}: exactly one security group must carry the cluster's tag, \
                     got {names:?} in {}",
                    row.case,
                    found.text
                );
                let group_name = &names[0];
                group_name_suffix(row.case, row.cluster, group_name);
                let group_id = element(&found.text, "groupId");
                assert_eq!(
                    element(&found.text, "vpcId"),
                    *vpc_id,
                    "case {:?}: the group must sit in the subnets' VPC, got {}",
                    row.case,
                    found.text
                );
                let tags = tag_map(&found.text);
                assert_eq!(
                    tags.get(CLUSTER_NAME_TAG).map(String::as_str),
                    Some(row.cluster),
                    "case {:?}: the group must carry {CLUSTER_NAME_TAG} for its cluster, \
                     got {tags:?}",
                    row.case
                );
                assert_eq!(
                    tags.get(&format!("{KUBERNETES_CLUSTER_TAG_PREFIX}{}", row.cluster))
                        .map(String::as_str),
                    Some("owned"),
                    "case {:?}: the group must be tagged as owned by its cluster, got {tags:?}",
                    row.case
                );
                assert_eq!(
                    tags.get("Name").map(String::as_str),
                    Some(group_name.as_str()),
                    "case {:?}: the group's Name tag must be the name it was created \
                     under, got {tags:?}",
                    row.case
                );

                // The cluster reports the group it owns and the VPC it resolved, read
                // back through `DescribeCluster` rather than off the create's answer.
                // The group is part of what provisioning the control plane produces, so
                // the cluster reports it from the moment it is `ACTIVE`: nothing sleeps
                // for that due instant inside the emulator, so this polls the read
                // rather than waiting on a span it would have to know.
                let mut read = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/clusters/{}", row.cluster),
                    None,
                );
                for _ in 0..40 {
                    if read.body["cluster"]["status"] == serde_json::json!("ACTIVE") {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    read = eks(
                        &router,
                        &runtime,
                        "GET",
                        &format!("/clusters/{}", row.cluster),
                        None,
                    );
                }
                assert_eq!(
                    read.status, 200,
                    "case {:?}: the cluster must be readable back, got {read:?}",
                    row.case
                );
                assert_eq!(
                    read.body["cluster"]["status"],
                    serde_json::json!("ACTIVE"),
                    "case {:?}: the cluster must reach ACTIVE, got {}",
                    row.case,
                    read.text
                );
                assert_eq!(
                    read.body["cluster"]["resourcesVpcConfig"]["clusterSecurityGroupId"],
                    serde_json::json!(group_id),
                    "case {:?}: the cluster must report the group it owns, got {}",
                    row.case,
                    read.text
                );
                assert_eq!(
                    read.body["cluster"]["resourcesVpcConfig"]["vpcId"],
                    serde_json::json!(vpc_id),
                    "case {:?}: the cluster must report the VPC its subnets sit in, got {}",
                    row.case,
                    read.text
                );
                owned.push((row.case, row.cluster, group_id));
            }

            // Two clusters of the same subnets own two DIFFERENT groups: the group
            // belongs to one cluster, not to the network.
            let distinct: std::collections::BTreeSet<&str> = owned
                .iter()
                .map(|(_, _, group_id)| group_id.as_str())
                .collect();
            assert_eq!(
                distinct.len(),
                owned.len(),
                "every cluster must own a group of its own, got {owned:?}"
            );

            // ---------------------------------------------------------------
            // RULE: deleting the cluster releases its group, and only its own.
            // ---------------------------------------------------------------
            for position in 0..owned.len() {
                let (case, cluster, group_id) = owned[position].clone();
                let deleted = eks(
                    &router,
                    &runtime,
                    "DELETE",
                    &format!("/clusters/{cluster}"),
                    None,
                );
                assert_eq!(
                    deleted.status, 200,
                    "case {case:?}: the cluster must be deleted, got {deleted:?}"
                );

                let gone = groups_of_cluster(&router, &runtime, case, cluster);
                assert!(
                    elements(&gone.text, "groupName").is_empty(),
                    "case {case:?}: the deleted cluster's group must be gone from EC2, \
                     got {}",
                    gone.text
                );
                let by_id = ec2(
                    &router,
                    &runtime,
                    "DescribeSecurityGroups",
                    &[("GroupId.1", group_id.as_str())],
                );
                assert_eq!(
                    (by_id.status, opt_element(&by_id.text, "Code").as_deref()),
                    (400, Some("InvalidGroup.NotFound")),
                    "case {case:?}: EC2 must hold no group {group_id} after the delete, \
                     got {by_id:?}"
                );

                // Every cluster not yet deleted still owns its group: a release takes
                // one group, not the family.
                for (other_case, other_cluster, other_group) in owned.iter().skip(position + 1) {
                    let still = groups_of_cluster(&router, &runtime, other_case, other_cluster);
                    assert_eq!(
                        opt_element(&still.text, "groupId").as_deref(),
                        Some(other_group.as_str()),
                        "case {case:?}: deleting {cluster} must leave {other_cluster}'s \
                         group {other_group} in place, got {}",
                        still.text
                    );
                }
            }
        })
        .expect("spawning the sized thread")
        .join()
        .expect("the suite thread");
}

/// The wrapped tail of the refusal EKS answers when the EC2 call it made on the caller's
/// behalf failed: `(Service: …; Status Code: …; Error Code: …; Request ID: …; Proxy: …)`.
///
/// The request id is a per-call value, so it is checked for its shape and for being
/// distinct between two refusals rather than compared to a literal.
fn assert_wrapped_tail(case: &str, unresolved: &str, answer: &Answer) -> String {
    assert_eq!(
        (answer.status, answer.error_type.as_deref()),
        (400, Some("InvalidParameterException")),
        "case {case:?}: an unknown subnet must be refused 400 InvalidParameterException, \
         got {answer:?}"
    );
    let message = answer.message();
    let head = format!(
        "The subnet ID '{unresolved}' does not exist (Service: AmazonEC2; Status Code: 400; \
         Error Code: {EC2_NOT_FOUND_CODE}; Request ID: "
    );
    let tail = "; Proxy: null)";
    assert!(
        message.starts_with(&head),
        "case {case:?}: the refusal must carry EC2's wrapped head {head:?}, got {message:?}"
    );
    assert!(
        message.ends_with(tail),
        "case {case:?}: the wrapped form must end {tail:?}, got {message:?}"
    );
    let request_id = message[head.len()..message.len() - tail.len()].to_string();
    // A request id is a UUID: 32 hex digits in five hyphenated groups.
    assert!(
        request_id.len() == 36
            && request_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
            && request_id.matches('-').count() == 4,
        "case {case:?}: the wrapped tail must name the inner call's request id, \
         got {request_id:?} in {message:?}"
    );
    request_id
}

#[test]
fn a_refusal_that_carries_an_ec2_failure_carries_its_whole_wrapped_form() {
    std::thread::Builder::new()
        .name("eks-wrapped-ec2-refusal".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let created = ec2(&router, &runtime, "CreateVpc", &[("CidrBlock", "10.6.0.0/16")]);
            assert_eq!(
                created.status, 200,
                "setup: CreateVpc must succeed, got {created:?}"
            );
            let vpc_id = element(&created.text, "vpcId");
            let mut held: Vec<String> = Vec::new();
            for cidr_block in ["10.6.1.0/24", "10.6.2.0/24"] {
                let answer = ec2(
                    &router,
                    &runtime,
                    "CreateSubnet",
                    &[("VpcId", vpc_id.as_str()), ("CidrBlock", cidr_block)],
                );
                assert_eq!(
                    answer.status, 200,
                    "setup: CreateSubnet {cidr_block} must succeed, got {answer:?}"
                );
                held.push(element(&answer.text, "subnetId"));
            }
            let host = "wrapped-tail-host";
            let host_created = eks(
                &router,
                &runtime,
                "POST",
                "/clusters",
                Some(cluster_body(host, &held)),
            );
            assert_eq!(
                host_created.status, 200,
                "setup: the cluster a node group hangs off must be created, got {host_created:?}"
            );

            // Both of EC2's subnet-id forms (`^subnet-[0-9a-f]{8,}$`: 8 and 17 hex
            // digits), on both creates that resolve subnets against EC2.
            let mut request_ids: Vec<String> = Vec::new();
            for (index, unresolved) in ["subnet-0badf00d", "subnet-00000000deadbeef1"]
                .into_iter()
                .enumerate()
            {
                let case = format!("CreateCluster naming {unresolved}");
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    "/clusters",
                    Some(cluster_body(
                        "wrapped-tail-cluster",
                        &[held[0].clone(), unresolved.to_string()],
                    )),
                );
                request_ids.push(assert_wrapped_tail(&case, unresolved, &answer));

                let case = format!("CreateNodegroup naming {unresolved}");
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/clusters/{host}/node-groups"),
                    Some(serde_json::json!({
                        "nodegroupName": format!("wrapped-tail-nodes-{index}"),
                        "nodeRole": role_arn("eksNodeRole"),
                        "subnets": [held[0].clone(), unresolved.to_string()],
                    })),
                );
                request_ids.push(assert_wrapped_tail(&case, unresolved, &answer));
            }

            // A request id names ONE call, so no two of these refusals share one.
            let distinct: std::collections::BTreeSet<&str> =
                request_ids.iter().map(String::as_str).collect();
            assert_eq!(
                distinct.len(),
                request_ids.len(),
                "each refusal must name the id of its own inner call, got {request_ids:?}"
            );
        })
        .expect("spawning the sized thread")
        .join()
        .expect("the suite thread");
}
