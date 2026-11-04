// SPDX-License-Identifier: CC0-1.0

//! Amazon EKS resolves the inputs a cluster create and a cluster registration name
//! against the services that OWN them, and refuses what does not resolve.
//!
//! Two rules are under test, and both need more than one service on one engine — EKS
//! reaches the others through the in-process callback, so a single-service router has
//! nothing to ask — which is why this suite lives here rather than in the EKS crate:
//!
//! * SUBNETS. A create that places an Amazon EKS resource in VPC subnets resolves every
//!   subnet id it names against the account's EC2 subnets; an id that names none of them
//!   is refused `InvalidParameterException` (400), naming that id and the cluster, and
//!   nothing is created. Rows span which position the unknown id sits at, how many ids
//!   are unknown, the id form (EC2's 8- and 17-hex forms), whether the subnet was
//!   created by the client or is one of the account's default subnets, and both
//!   operations that take subnets (`CreateCluster`, `CreateNodegroup`). When more than
//!   one id is unknown, the one reported is the LAST of them in the order the request
//!   listed them (`eks_wrapped_peer_refusals` pins that half on its own).
//! * THE CONNECTOR ROLE. A registration resolves `connectorConfig.roleArn` against IAM
//!   and is refused `InvalidRequestException` (400), naming the role, unless IAM holds
//!   that role and its trust policy lets the SSM service principal assume it. Rows span
//!   the trust document's shape (one string or a list, a wildcard action, an explicit
//!   deny, another service's principal), the role's path, and a role that was never
//!   created.
//! * THE ORDER THE TWO RUN IN. A create of a resource INSIDE a cluster resolves the
//!   cluster its `@httpLabel` names against EKS's own state — answering one it cannot
//!   resolve with the 404 that names only that cluster — BEFORE it resolves anything the
//!   body names against EC2 or IAM, so a caller whose cluster does not exist is told
//!   that rather than that the subnet or the role it supplied is unusable. Rows span
//!   both child creates that resolve a peer, the peer-owned member each gets wrong, and
//!   the form of the cluster name; the control half is the same body against a cluster
//!   that resolves, which is still refused 400.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and
//! the requests driven — on a sized thread.
//!
//! The request harness is the test crate's own: `eks_wire` for the drivers and the answer,
//! `query_wire` for the XML readers.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::eks_wire::{
    cluster_body, ec2, eks, iam, role_arn, Answer, EC2_NOT_FOUND_CODE,
};
use crate::query_wire::opt_element;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A trust policy with one statement, built from the pieces that vary between rows.
///
/// Written compactly and with no `&`, `=`, `+` or `%`, so it travels in an `awsQuery`
/// form body as written.
fn trust_policy(effect: &str, action: &str, service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":{action}}}]}}"
    )
}

/// One subnet row: the ids a create names, and the id (if any) that resolves to nothing.
struct SubnetCase {
    case: &'static str,
    cluster: &'static str,
    subnets: Vec<String>,
    /// `Some(id)` when the create must be refused naming `id`; `None` when it must be
    /// served.
    unresolved: Option<String>,
}

/// Assert one refusal: the status, the code, the cluster it names and the subnet id it
/// quotes — the rule is that the id the caller got wrong is the one reported.
fn assert_subnet_refusal(case: &str, cluster: &str, unresolved: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_type.as_deref()),
        (400, Some("InvalidParameterException")),
        "case {case:?}: an unknown subnet must be refused 400 InvalidParameterException, \
         got {answer:?}"
    );
    assert_eq!(
        answer
            .body
            .get("clusterName")
            .and_then(serde_json::Value::as_str),
        Some(cluster),
        "case {case:?}: the refusal must name the cluster the request asked for, got {}",
        answer.text
    );
    let message = answer.message();
    assert!(
        message.contains(unresolved),
        "case {case:?}: the refusal must name the subnet {unresolved:?}, got {message:?}"
    );
    assert!(
        message.contains(EC2_NOT_FOUND_CODE),
        "case {case:?}: the refusal must carry EC2's own error code \
         {EC2_NOT_FOUND_CODE:?}, got {message:?}"
    );
}

#[test]
fn eks_resolves_its_subnets_against_ec2_and_its_connector_role_against_iam() {
    std::thread::Builder::new()
        .name("eks-cluster-inputs".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // ---------------------------------------------------------------
            // Setup: a VPC with two subnets the client created, and the
            // account's own default subnet, all through EC2 itself.
            // ---------------------------------------------------------------
            let created = ec2(&router, &runtime, "CreateVpc", &[("CidrBlock", "10.9.0.0/16")]);
            assert_eq!(
                created.status, 200,
                "setup: CreateVpc must succeed, got {created:?}"
            );
            let vpc_id = opt_element(&created.text, "vpcId")
                .unwrap_or_else(|| panic!("setup: no <vpcId> in {}", created.text));
            let mut client_subnets: Vec<String> = Vec::new();
            for cidr_block in ["10.9.1.0/24", "10.9.2.0/24"] {
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
                client_subnets.push(
                    opt_element(&answer.text, "subnetId")
                        .unwrap_or_else(|| panic!("setup: no <subnetId> in {}", answer.text)),
                );
            }
            // A default subnet of the account's default VPC: a subnet nothing in this
            // test created, so the rule is shown to hold for a subnet the account simply
            // HAS as well as for one a client made.
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
            let default_subnet = opt_element(&defaults.text, "subnetId").unwrap_or_else(|| {
                panic!(
                    "setup: the account must have a default subnet, got {}",
                    defaults.text
                )
            });

            // Two ids no subnet of the account carries, in both of EC2's id forms
            // (`^subnet-[0-9a-f]{8,}$`: 8 and 17 hex digits).
            let unknown_long = "subnet-00000000deadbeef1".to_string();
            let unknown_short = "subnet-0badf00d".to_string();

            // ---------------------------------------------------------------
            // RULE: every subnet a create names must resolve in EC2.
            // ---------------------------------------------------------------
            let rows = vec![
                SubnetCase {
                    case: "two subnets the client created in EC2",
                    cluster: "created-subnets",
                    subnets: client_subnets.clone(),
                    unresolved: None,
                },
                SubnetCase {
                    case: "a default subnet of the account, beside a created one",
                    cluster: "default-subnet",
                    subnets: vec![default_subnet.clone(), client_subnets[0].clone()],
                    unresolved: None,
                },
                SubnetCase {
                    case: "an unknown id in the last position",
                    cluster: "unknown-last",
                    subnets: vec![client_subnets[0].clone(), unknown_long.clone()],
                    unresolved: Some(unknown_long.clone()),
                },
                SubnetCase {
                    case: "an unknown id in the first position",
                    cluster: "unknown-first",
                    subnets: vec![unknown_long.clone(), client_subnets[1].clone()],
                    unresolved: Some(unknown_long.clone()),
                },
                SubnetCase {
                    case: "two unknown ids: the last one in list order is reported",
                    cluster: "unknown-both",
                    subnets: vec![unknown_long.clone(), unknown_short.clone()],
                    unresolved: Some(unknown_short.clone()),
                },
                SubnetCase {
                    case: "an unknown id in EC2's short 8-hex form",
                    cluster: "unknown-short",
                    subnets: vec![unknown_short.clone(), client_subnets[1].clone()],
                    unresolved: Some(unknown_short.clone()),
                },
                SubnetCase {
                    case: "three subnets, one of them unknown",
                    cluster: "unknown-of-three",
                    subnets: vec![
                        client_subnets[0].clone(),
                        client_subnets[1].clone(),
                        unknown_short.clone(),
                    ],
                    unresolved: Some(unknown_short.clone()),
                },
            ];

            for row in &rows {
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    "/clusters",
                    Some(cluster_body(row.cluster, &row.subnets)),
                );
                let read = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/clusters/{}", row.cluster),
                    None,
                );
                match row.unresolved.as_deref() {
                    None => {
                        assert_eq!(
                            answer.status, 200,
                            "case {:?}: every subnet resolves, so the cluster must be \
                             created, got {answer:?}",
                            row.case
                        );
                        assert_eq!(
                            read.status, 200,
                            "case {:?}: the cluster must be readable back, got {read:?}",
                            row.case
                        );
                        assert_eq!(
                            read.body["cluster"]["resourcesVpcConfig"]["subnetIds"],
                            serde_json::json!(row.subnets),
                            "case {:?}: the stored cluster must carry the subnets it was \
                             given, got {}",
                            row.case,
                            read.text
                        );
                    }
                    Some(unresolved) => {
                        assert_subnet_refusal(row.case, row.cluster, unresolved, &answer);
                        assert_eq!(
                            (read.status, read.error_type.as_deref()),
                            (404, Some("ResourceNotFoundException")),
                            "case {:?}: a refused create must store nothing, got {read:?}",
                            row.case
                        );
                    }
                }
            }

            // The same rule on `CreateNodegroup$subnets`, read back through
            // `DescribeNodegroup`: the node group's nodes are launched into those
            // subnets, so they must exist too.
            let host = "nodegroup-host";
            let created = eks(
                &router,
                &runtime,
                "POST",
                "/clusters",
                Some(cluster_body(host, &client_subnets)),
            );
            assert_eq!(
                created.status, 200,
                "setup: the cluster a node group hangs off must be created, got {created:?}"
            );
            for (case, nodegroup, subnets, unresolved) in [
                (
                    "a node group in the subnets the client created",
                    "resolved-nodes",
                    client_subnets.clone(),
                    None,
                ),
                (
                    "a node group naming a subnet that does not exist",
                    "unresolved-nodes",
                    vec![client_subnets[0].clone(), unknown_long.clone()],
                    Some(unknown_long.clone()),
                ),
            ] {
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/clusters/{host}/node-groups"),
                    Some(serde_json::json!({
                        "nodegroupName": nodegroup,
                        "nodeRole": role_arn("eksNodeRole"),
                        "subnets": subnets,
                    })),
                );
                let read = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/clusters/{host}/node-groups/{nodegroup}"),
                    None,
                );
                match unresolved {
                    None => {
                        assert_eq!(
                            answer.status, 200,
                            "case {case:?}: the node group must be created, got {answer:?}"
                        );
                        assert_eq!(
                            read.status, 200,
                            "case {case:?}: the node group must be readable back, got {read:?}"
                        );
                        assert_eq!(
                            read.body["nodegroup"]["subnets"],
                            serde_json::json!(subnets),
                            "case {case:?}: the stored node group must carry the subnets it \
                             was given, got {}",
                            read.text
                        );
                    }
                    Some(unresolved) => {
                        assert_subnet_refusal(case, host, &unresolved, &answer);
                        assert_eq!(
                            read.status, 404,
                            "case {case:?}: a refused create must store nothing, got {read:?}"
                        );
                    }
                }
            }

            // ---------------------------------------------------------------
            // RULE: the connector role must exist in IAM and let the SSM service
            // principal assume it.
            // ---------------------------------------------------------------
            let ssm = "\"ssm.amazonaws.com\"";
            let fixtures: Vec<(&str, Option<&str>, String)> = vec![
                (
                    "connector-role",
                    None,
                    trust_policy("Allow", "\"sts:AssumeRole\"", ssm),
                ),
                (
                    "connector-role-lists",
                    None,
                    trust_policy(
                        "Allow",
                        "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                        "[\"eks.amazonaws.com\",\"ssm.amazonaws.com\"]",
                    ),
                ),
                (
                    "connector-role-wildcard",
                    None,
                    trust_policy("Allow", "\"sts:*\"", ssm),
                ),
                (
                    "path-scoped-connector",
                    Some("/service-role/"),
                    trust_policy("Allow", "\"sts:AssumeRole\"", ssm),
                ),
                (
                    "lambda-exec-role",
                    None,
                    trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
                ),
                (
                    "denied-connector",
                    None,
                    trust_policy("Deny", "\"sts:AssumeRole\"", ssm),
                ),
                (
                    "wrong-action-connector",
                    None,
                    trust_policy("Allow", "\"ssm:SendCommand\"", ssm),
                ),
                (
                    "capability-role",
                    None,
                    trust_policy("Allow", "\"sts:AssumeRole\"", "\"eks.amazonaws.com\""),
                ),
            ];
            for (name, path, policy) in &fixtures {
                let mut params = vec![
                    ("RoleName", *name),
                    ("AssumeRolePolicyDocument", policy.as_str()),
                ];
                if let Some(path) = path {
                    params.push(("Path", path));
                }
                let answer = iam(&router, &runtime, "CreateRole", &params);
                assert_eq!(
                    answer.status, 200,
                    "setup: CreateRole {name} must succeed, got {answer:?}"
                );
            }

            // (case, the registration's name, the connector role, whether it registers)
            let connector_rows: Vec<(&str, &str, String, bool)> = vec![
                (
                    "a role that exists and trusts SSM",
                    "trusted-connector",
                    role_arn("connector-role"),
                    true,
                ),
                (
                    "a trust policy whose Action and Service are lists",
                    "trusted-lists",
                    role_arn("connector-role-lists"),
                    true,
                ),
                (
                    "a trust policy allowing the whole sts namespace",
                    "trusted-wildcard",
                    role_arn("connector-role-wildcard"),
                    true,
                ),
                (
                    "a role under a path",
                    "trusted-path",
                    format!(
                        "arn:aws:iam::{}:role/service-role/path-scoped-connector",
                        engine::emulator_account_id()
                    ),
                    true,
                ),
                (
                    "a role that trusts another service only",
                    "lambda-connector",
                    role_arn("lambda-exec-role"),
                    false,
                ),
                (
                    "a role whose statement denies the assume",
                    "denied-connector",
                    role_arn("denied-connector"),
                    false,
                ),
                (
                    "a role whose statement names another action",
                    "wrong-action",
                    role_arn("wrong-action-connector"),
                    false,
                ),
                (
                    "a role that was never created",
                    "absent-connector",
                    role_arn("never-created-connector"),
                    false,
                ),
            ];
            for (case, name, connector_role, accepted) in &connector_rows {
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    "/cluster-registrations",
                    Some(serde_json::json!({
                        "name": name,
                        "connectorConfig": { "roleArn": connector_role, "provider": "OTHER" },
                    })),
                );
                let read = eks(&router, &runtime, "GET", &format!("/clusters/{name}"), None);
                if *accepted {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: a usable connector role must register, got {answer:?}"
                    );
                    assert_eq!(
                        read.body["cluster"]["connectorConfig"]["roleArn"],
                        serde_json::json!(connector_role),
                        "case {case:?}: the stored registration must carry its role, got {}",
                        read.text
                    );
                } else {
                    assert_eq!(
                        (answer.status, answer.error_type.as_deref()),
                        (400, Some("InvalidRequestException")),
                        "case {case:?}: an unusable connector role must be refused 400 \
                         InvalidRequestException, got {answer:?}"
                    );
                    let message = answer.message();
                    assert!(
                        message.contains(connector_role.as_str()),
                        "case {case:?}: the refusal must name the role at fault, got \
                         {message:?}"
                    );
                    assert_eq!(
                        (read.status, read.error_type.as_deref()),
                        (404, Some("ResourceNotFoundException")),
                        "case {case:?}: a refused registration must store nothing, got \
                         {read:?}"
                    );
                }
            }

            // ---------------------------------------------------------------
            // The SAME trust rule on `CreateCapability$roleArn`, whose own model text
            // names the principal that must be able to assume it ("a trust policy that
            // allows the EKS service principal"): the principal differs, the rule does
            // not. A capability is only created on an ACTIVE cluster, so the host
            // cluster's clock-owed `CREATING` -> `ACTIVE` move is waited out first.
            // ---------------------------------------------------------------
            let capability_host = "capability-host";
            let created = eks(
                &router,
                &runtime,
                "POST",
                "/clusters",
                Some(cluster_body(capability_host, &client_subnets)),
            );
            assert_eq!(
                created.status, 200,
                "setup: the cluster a capability hangs off must be created, got {created:?}"
            );
            // Nothing sleeps for the due instant inside the emulator: a READ after it
            // materialises the move, so this polls the read rather than waiting on a
            // span it would have to know.
            let mut active = false;
            for _ in 0..40 {
                let read = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/clusters/{capability_host}"),
                    None,
                );
                if read.body["cluster"]["status"] == serde_json::json!("ACTIVE") {
                    active = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            assert!(
                active,
                "setup: the cluster must reach ACTIVE before a capability is created"
            );

            // (case, the capability's name, the role it names, whether it is created)
            let capability_rows: Vec<(&str, &str, String, bool)> = vec![
                (
                    "a role that exists and trusts Amazon EKS",
                    "trusted-capability",
                    role_arn("capability-role"),
                    true,
                ),
                (
                    "a role that trusts another service only",
                    "lambda-capability",
                    role_arn("lambda-exec-role"),
                    false,
                ),
                (
                    "a role that was never created",
                    "absent-capability",
                    role_arn("never-created-capability"),
                    false,
                ),
            ];
            for (case, name, capability_role, accepted) in &capability_rows {
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/clusters/{capability_host}/capabilities"),
                    Some(serde_json::json!({
                        "capabilityName": name,
                        "type": "ACK",
                        "roleArn": capability_role,
                        "deletePropagationPolicy": "RETAIN",
                    })),
                );
                let read = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/clusters/{capability_host}/capabilities/{name}"),
                    None,
                );
                if *accepted {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: a usable capability role must be accepted, got \
                         {answer:?}"
                    );
                    assert_eq!(
                        read.status, 200,
                        "case {case:?}: the capability must be readable back, got {read:?}"
                    );
                } else {
                    assert_eq!(
                        (answer.status, answer.error_type.as_deref()),
                        (400, Some("InvalidParameterException")),
                        "case {case:?}: an unusable capability role must be refused 400 \
                         InvalidParameterException, got {answer:?}"
                    );
                    let message = answer.message();
                    assert!(
                        message.contains(capability_role.as_str()),
                        "case {case:?}: the refusal must name the role at fault, got \
                         {message:?}"
                    );
                    assert_eq!(
                        read.status, 404,
                        "case {case:?}: a refused create must store nothing, got {read:?}"
                    );
                }
            }

            // ---------------------------------------------------------------
            // RULE: the PARENT resolves first. A create of a resource inside a
            // cluster resolves the cluster its `@httpLabel` names against EKS's own
            // state — and answers one it cannot resolve with the 404 that names only
            // that cluster — BEFORE it resolves anything the body names against EC2
            // or IAM. A caller whose cluster does not exist is told THAT, not that
            // the subnet or the role it supplied is unusable, so its recovery
            // (create the cluster, retry) is the one the answer points at.
            //
            // The rows reuse the fixtures above — a role IAM holds that does not
            // trust Amazon EKS, a role IAM never created, and subnet ids no subnet of
            // the account carries — each of which IS refused on a cluster that
            // resolves (the control half below), and span both child creates that
            // resolve a peer, the member they get wrong, and the form of the cluster
            // name the request carries (`clusterName` is `length 1..100`).
            // ---------------------------------------------------------------
            let ghost_longest = "g".repeat(100);
            let ghost_clusters: Vec<(&str, &str)> = vec![
                ("a cluster that was never created", "ghost-cluster"),
                (
                    "a cluster name differing only in case from a real one",
                    "CAPABILITY-HOST",
                ),
                (
                    "a cluster name at the documented length ceiling",
                    ghost_longest.as_str(),
                ),
                ("a one-character cluster name", "g"),
            ];
            // (case, the collection under the cluster, a body whose peer-owned member
            // resolves to nothing)
            let peer_rows: Vec<(&str, &str, serde_json::Value)> = vec![
                (
                    "CreateCapability naming a role that does not trust Amazon EKS",
                    "capabilities",
                    serde_json::json!({
                        "capabilityName": "orphan-capability",
                        "type": "ACK",
                        "roleArn": role_arn("lambda-exec-role"),
                        "deletePropagationPolicy": "RETAIN",
                    }),
                ),
                (
                    "CreateCapability naming a role IAM never created",
                    "capabilities",
                    serde_json::json!({
                        "capabilityName": "orphan-capability",
                        "type": "ARGOCD",
                        "roleArn": role_arn("never-created-capability"),
                        "deletePropagationPolicy": "RETAIN",
                    }),
                ),
                (
                    "CreateNodegroup naming one subnet EC2 does not hold",
                    "node-groups",
                    serde_json::json!({
                        "nodegroupName": "orphan-nodes",
                        "nodeRole": role_arn("eksNodeRole"),
                        "subnets": [client_subnets[0].clone(), unknown_long.clone()],
                    }),
                ),
                (
                    "CreateNodegroup naming only subnets EC2 does not hold",
                    "node-groups",
                    serde_json::json!({
                        "nodegroupName": "orphan-nodes",
                        "nodeRole": role_arn("eksNodeRole"),
                        "subnets": [unknown_long.clone(), unknown_short.clone()],
                    }),
                ),
            ];

            for (why, ghost) in &ghost_clusters {
                for (row, collection, body) in &peer_rows {
                    let case = format!("{why} / {row}");
                    let answer = eks(
                        &router,
                        &runtime,
                        "POST",
                        &format!("/clusters/{ghost}/{collection}"),
                        Some(body.clone()),
                    );
                    assert_eq!(
                        (answer.status, answer.error_type.as_deref()),
                        (404, Some("ResourceNotFoundException")),
                        "case {case:?}: the unresolvable cluster must be reported before the \
                         peer-owned member, got {answer:?}"
                    );
                    assert_eq!(
                        answer
                            .body
                            .get("clusterName")
                            .and_then(serde_json::Value::as_str),
                        Some(*ghost),
                        "case {case:?}: the payload must name the cluster that was not found, \
                         got {}",
                        answer.text
                    );
                    assert_eq!(
                        answer.message(),
                        service_com_amazonaws_eks_awswesleyfrontend::state::cluster_not_found_message(
                            ghost
                        ),
                        "case {case:?}: expected the message built from the request, got {}",
                        answer.text
                    );
                    // The cluster is the resource reported missing, so the child the
                    // request asked for is not echoed into the payload.
                    for member in ["nodegroupName", "addonName", "fargateProfileName"] {
                        assert!(
                            answer
                                .body
                                .get(member)
                                .is_none_or(serde_json::Value::is_null),
                            "case {case:?}: the payload must not echo the request's {member}, \
                             got {}",
                            answer.text
                        );
                    }
                }
            }

            // CONTROL: the rule is about ORDER, not about dropping the peer
            // resolution — against a cluster that DOES resolve, every one of those
            // bodies is still refused 400 `InvalidParameterException`.
            for (row, collection, body) in &peer_rows {
                let resolvable = if *collection == "capabilities" {
                    capability_host
                } else {
                    host
                };
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/clusters/{resolvable}/{collection}"),
                    Some(body.clone()),
                );
                assert_eq!(
                    (answer.status, answer.error_type.as_deref()),
                    (400, Some("InvalidParameterException")),
                    "case {row:?}: once the cluster resolves, the peer-owned member is still \
                     resolved, got {answer:?}"
                );
            }
        })
        .expect("spawn the eks cluster-inputs thread")
        .join()
        .expect("every cluster input must be resolved against the service that owns it");
}
