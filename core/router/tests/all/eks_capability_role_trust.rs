// SPDX-License-Identifier: CC0-1.0

//! A managed capability's `roleArn` is resolved against IAM, and every service
//! principal Amazon EKS publishes for a capability role satisfies it.
//!
//! The RULE under test: "This role must have a trust policy that allows the EKS
//! service principal to assume it" (`CreateCapability$roleArn`, and the same member
//! on `UpdateCapability`) — Amazon EKS runs a capability's infrastructure outside the
//! cluster, so the plane that assumes the role has its own principal under the
//! Amazon EKS domain (`capabilities.eks.amazonaws.com`) beside the control plane's
//! (`eks.amazonaws.com`). A role IAM holds whose trust policy admits EITHER is a role
//! the capability can use, and the `InvalidParameterException` (400) is reserved for
//! a role that IAM does not hold or whose trust policy admits neither.
//!
//! This needs more than one service on one engine — EKS asks IAM over the in-process
//! callback, so a single-service router has nothing to ask — which is why the suite
//! lives here rather than in the EKS crate. The rows differ along which principal the
//! trust document names and how (one principal, a list of both), which actions it
//! allows (`sts:AssumeRole` alone, with `sts:TagSession` beside it), whether the
//! statement allows or denies, which operation resolves the role (`CreateCapability`
//! and `UpdateCapability`), and a role IAM never held.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built —
//! and the requests driven — on a sized thread. The request harness is the test
//! crate's own: `eks_wire` for the drivers and the answer, `query_wire` for the XML
//! readers.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::eks_wire::{cluster_body, ec2, eks, iam, role_arn, Answer};
use crate::query_wire::elements;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The principal the plane that runs a capability assumes its role as.
const CAPABILITIES_PRINCIPAL: &str = "\"capabilities.eks.amazonaws.com\"";

/// The Amazon EKS control plane's own principal.
const EKS_PRINCIPAL: &str = "\"eks.amazonaws.com\"";

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

/// Assert a refusal of a role the capability cannot use: the class, and that it
/// names the role at fault and the SERVICE whose principal the role has to trust.
fn assert_role_refusal(case: &str, role: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_type.as_deref()),
        (400, Some("InvalidParameterException")),
        "case {case:?}: an unusable capability role must be refused 400 \
         InvalidParameterException, got {answer:?}"
    );
    let message = answer.message();
    assert!(
        message.contains(role),
        "case {case:?}: the refusal must name the role at fault, got {message:?}"
    );
    assert!(
        message.contains("eks service principal"),
        "case {case:?}: the refusal must name the service the role has to trust, got \
         {message:?}"
    );
}

#[test]
fn a_capability_role_may_trust_any_principal_eks_publishes_for_one() {
    std::thread::Builder::new()
        .name("eks-capability-role-trust".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // A cluster is created in subnets EC2 resolves, so the account's own
            // default subnets are read from EC2 rather than invented.
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
            let subnets = elements(&defaults.text, "subnetId");
            assert!(
                subnets.len() >= 2,
                "setup: the account must have default subnets, got {subnets:?}"
            );
            let subnets = vec![subnets[0].clone(), subnets[1].clone()];

            // The roles, each with the trust document its rows are about.
            let assume_and_tag = "[\"sts:AssumeRole\",\"sts:TagSession\"]";
            let both_principals = format!("[{CAPABILITIES_PRINCIPAL},{EKS_PRINCIPAL}]");
            let fixtures: Vec<(&str, String)> = vec![
                (
                    "capability-plane-role",
                    trust_policy("Allow", assume_and_tag, CAPABILITIES_PRINCIPAL),
                ),
                (
                    "capability-plane-assume-only",
                    trust_policy("Allow", "\"sts:AssumeRole\"", CAPABILITIES_PRINCIPAL),
                ),
                (
                    "capability-both-principals",
                    trust_policy("Allow", assume_and_tag, both_principals.as_str()),
                ),
                (
                    "capability-control-plane-role",
                    trust_policy("Allow", "\"sts:AssumeRole\"", EKS_PRINCIPAL),
                ),
                (
                    "capability-lambda-role",
                    trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
                ),
                (
                    "capability-denied-role",
                    trust_policy("Deny", assume_and_tag, CAPABILITIES_PRINCIPAL),
                ),
                (
                    "capability-wrong-action-role",
                    trust_policy("Allow", "\"ssm:SendCommand\"", CAPABILITIES_PRINCIPAL),
                ),
            ];
            for (name, policy) in &fixtures {
                let answer = iam(
                    &router,
                    &runtime,
                    "CreateRole",
                    &[
                        ("RoleName", name),
                        ("AssumeRolePolicyDocument", policy.as_str()),
                    ],
                );
                assert_eq!(
                    answer.status, 200,
                    "setup: CreateRole {name} must succeed, got {answer:?}"
                );
            }

            // Two clusters: "you can only create one Capability of each type on a
            // given Amazon EKS cluster", so the accepted rows are spread over them.
            let hosts = ["capability-trust-host", "capability-trust-host-two"];
            for host in hosts {
                let created = eks(
                    &router,
                    &runtime,
                    "POST",
                    "/clusters",
                    Some(cluster_body(host, &subnets)),
                );
                assert_eq!(
                    created.status, 200,
                    "setup: the cluster a capability hangs off must be created, got {created:?}"
                );
            }
            // Nothing sleeps for a due instant inside the emulator: a READ after it
            // materialises the move, so this polls the read.
            for host in hosts {
                let mut active = false;
                for _ in 0..40 {
                    let read = eks(&router, &runtime, "GET", &format!("/clusters/{host}"), None);
                    if read.body["cluster"]["status"] == serde_json::json!("ACTIVE") {
                        active = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                assert!(
                    active,
                    "setup: {host} must reach ACTIVE before a capability is created"
                );
            }

            // (case, the host cluster, the capability's name, its type, the role, accepted)
            let rows: Vec<(&str, &str, &str, &str, String, bool)> = vec![
                (
                    "the documented capability trust policy: the capabilities principal \
                     with sts:AssumeRole and sts:TagSession",
                    hosts[0],
                    "capability-plane",
                    "ACK",
                    role_arn("capability-plane-role"),
                    true,
                ),
                (
                    "the capabilities principal with sts:AssumeRole alone",
                    hosts[0],
                    "capability-plane-assume",
                    "ARGOCD",
                    role_arn("capability-plane-assume-only"),
                    true,
                ),
                (
                    "a trust policy naming both principals as a list",
                    hosts[0],
                    "capability-both",
                    "KRO",
                    role_arn("capability-both-principals"),
                    true,
                ),
                (
                    "the control plane's own principal",
                    hosts[1],
                    "capability-control-plane",
                    "ACK",
                    role_arn("capability-control-plane-role"),
                    true,
                ),
                (
                    "a role that trusts another service only",
                    hosts[0],
                    "capability-lambda",
                    "ACK",
                    role_arn("capability-lambda-role"),
                    false,
                ),
                (
                    "a role whose statement denies the assume",
                    hosts[0],
                    "capability-denied",
                    "ACK",
                    role_arn("capability-denied-role"),
                    false,
                ),
                (
                    "a role whose statement names another action",
                    hosts[0],
                    "capability-wrong-action",
                    "ACK",
                    role_arn("capability-wrong-action-role"),
                    false,
                ),
                (
                    "a role IAM never held",
                    hosts[0],
                    "capability-absent",
                    "ACK",
                    role_arn("capability-never-created-role"),
                    false,
                ),
            ];

            for (case, host, name, capability_type, role, accepted) in &rows {
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/clusters/{host}/capabilities"),
                    Some(serde_json::json!({
                        "capabilityName": name,
                        "type": capability_type,
                        "roleArn": role,
                        "deletePropagationPolicy": "RETAIN",
                    })),
                );
                let read = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/clusters/{host}/capabilities/{name}"),
                    None,
                );
                if *accepted {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: a role Amazon EKS can assume for a capability must be \
                         accepted, got {answer:?} ({})",
                        answer.message()
                    );
                    assert_eq!(
                        read.status, 200,
                        "case {case:?}: the capability must read back, got {read:?}"
                    );
                    assert_eq!(
                        read.body["capability"]["roleArn"],
                        serde_json::json!(role),
                        "case {case:?}: the stored capability must carry the role it was given, \
                         got {}",
                        read.text
                    );
                } else {
                    assert_role_refusal(case, role, &answer);
                    assert_eq!(
                        read.status, 404,
                        "case {case:?}: a refused create must store nothing, got {read:?}"
                    );
                }
            }

            // THE SAME RULE on `UpdateCapability$roleArn`: the capability created with
            // the control plane's principal above is handed a role that trusts the
            // capabilities principal, and then one no capability can use. An update
            // starts only from a settled capability, so the clock-owed
            // `CREATING` -> `ACTIVE` move is polled out first.
            let read_capability = || {
                eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/clusters/{}/capabilities/capability-control-plane", hosts[1]),
                    None,
                )
            };
            let mut settled = false;
            for _ in 0..40 {
                if read_capability().body["capability"]["status"]
                    == serde_json::json!("ACTIVE")
                {
                    settled = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            assert!(
                settled,
                "setup: the capability must reach ACTIVE before it is updated"
            );
            let update = |role: &str| {
                eks(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/clusters/{}/capabilities/capability-control-plane", hosts[1]),
                    Some(serde_json::json!({ "roleArn": role })),
                )
            };
            let accepted_role = role_arn("capability-plane-role");
            let answer = update(&accepted_role);
            assert_eq!(
                answer.status, 200,
                "case \"UpdateCapability with the capabilities principal\": the role must be \
                 accepted, got {answer:?} ({})",
                answer.message()
            );
            let refused_role = role_arn("capability-lambda-role");
            assert_role_refusal(
                "UpdateCapability with a role that trusts another service only",
                &refused_role,
                &update(&refused_role),
            );
        })
        .expect("spawn the eks capability-role-trust thread")
        .join()
        .expect("the eks capability-role-trust thread");
}
