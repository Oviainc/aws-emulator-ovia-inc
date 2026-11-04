// SPDX-License-Identifier: CC0-1.0

//! A refusal Amazon EKS carries through from a call it made to ANOTHER service on the
//! caller's behalf is reported WRAPPED, and it names the input the inner service
//! complained about.
//!
//! Two halves of one rule, both needing more than one service on one engine (EKS
//! reaches IAM and EC2 through the in-process callback, so a single-service router has
//! nothing to ask), which is why this suite lives here rather than in the EKS crate:
//!
//! * THE WRAPPED TAIL. The message the caller gets is the inner sentence followed by a
//!   tail naming the service that answered, the status it answered with and the id of
//!   that call, plus how many attempts were made to reach it. `RegisterCluster`'s
//!   connector-role refusal is answered by the plane that manages external clusters, so
//!   its tail names that service; the status is the refusal's own 400, and the request
//!   id is minted per call, so this suite asserts the FIXED text exactly and the id by
//!   its shape. Rows span why the role is unusable (IAM holds it but its trust policy
//!   admits another service only, IAM holds no such role) and the path the role sits
//!   under.
//! * WHICH SUBNET A SUBNET REFUSAL NAMES. A create that places an Amazon EKS resource in
//!   VPC subnets resolves every id it names against EC2, and when more than one of them
//!   resolves to nothing the refusal names the LAST such id in the order the request
//!   listed them — the whole wrapped sentence, EC2's own, is asserted. Rows span how
//!   many ids are unknown and where in the list they sit, the id form (EC2's 8- and
//!   17-hex forms), a repeated id (the same question asked twice), and both operations
//!   that take subnets (`CreateCluster`, `CreateNodegroup`).
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and
//! the requests driven — on a sized thread. The request harness is the test crate's own
//! (`eks_wire` for the drivers and the answer, `query_wire` for the XML readers).

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::eks_wire::{cluster_body, ec2, eks, iam, role_arn, EC2_NOT_FOUND_CODE};
use crate::query_wire::opt_element;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The service the plane that manages EXTERNAL clusters is reported as.
const EXTERNAL_CLUSTER_MANAGER: &str = "EksExternalClusterManager";

/// The service EC2 is reported as in a wrapped refusal.
const EC2_SERVICE: &str = "AmazonEC2";

/// A trust policy with one statement, allowing `service` to assume the role.
///
/// Written compactly and with no `&`, `=`, `+` or `%`, so it travels in an `awsQuery`
/// form body as written.
fn trust_policy(service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{service}\"}},\"Action\":\"sts:AssumeRole\"}}]}}"
    )
}

/// Whether `value` has the shape of the request id a wrapped tail carries: a
/// version-4 UUID, `8-4-4-4-12` lowercase hex.
///
/// The id itself is minted per call — that is what a request id IS — so a test may
/// assert its FORM and never a value.
fn is_request_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit()))
        && value.chars().all(|character| !character.is_ascii_uppercase())
}

/// Split a message into the text before the request id and the text after it, given
/// the fixed `head` it must begin with and `tail` it must end with.
///
/// `None` when either fixed part is missing, which the caller reports as the failure.
fn request_id_between<'a>(message: &'a str, head: &str, tail: &str) -> Option<&'a str> {
    let rest = message.strip_prefix(head)?;
    rest.strip_suffix(tail)
}

/// One subnet row: the ids a create names, and the one the refusal must name.
struct SubnetCase {
    case: &'static str,
    resource: String,
    subnets: Vec<String>,
    reported: String,
}

#[test]
fn eks_reports_a_peer_refusal_wrapped_and_names_the_input_the_peer_refused() {
    std::thread::Builder::new()
        .name("eks-wrapped-peer-refusals".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // ---------------------------------------------------------------
            // RULE: a connector role Amazon EKS cannot use is refused with the
            // inner sentence plus the wrapped tail of the service that judged it.
            // ---------------------------------------------------------------
            for (name, path) in [("wrapped-lambda-role", None), ("wrapped-scoped", Some("/svc/"))] {
                let policy = trust_policy("lambda.amazonaws.com");
                let mut params = vec![
                    ("RoleName", name),
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

            // (case, the registration's name, the connector role)
            let connector_rows: Vec<(&str, &str, String)> = vec![
                (
                    "a role IAM holds whose trust policy admits another service only",
                    "wrapped-lambda",
                    role_arn("wrapped-lambda-role"),
                ),
                (
                    "the same, under a path",
                    "wrapped-scoped",
                    format!(
                        "arn:aws:iam::{}:role/svc/wrapped-scoped",
                        engine::emulator_account_id()
                    ),
                ),
                (
                    "a role IAM never created",
                    "wrapped-absent",
                    role_arn("wrapped-never-created"),
                ),
            ];

            for (case, name, connector_role) in &connector_rows {
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
                assert_eq!(
                    (answer.status, answer.error_type.as_deref()),
                    (400, Some("InvalidRequestException")),
                    "case {case:?}: an unusable connector role must be refused 400 \
                     InvalidRequestException, got {answer:?}"
                );
                let message = answer.message();
                // The inner sentence names the role at fault and the principal the
                // trust policy had to admit; the tail names the service that answered,
                // the status of this refusal and how many attempts reached it.
                let head = format!(
                    "Nonexistent role or missing ssm service principal in trust policy: \
                     {connector_role} (Service: {EXTERNAL_CLUSTER_MANAGER}, Status Code: \
                     400, Request ID: "
                );
                let tail = ") (SDK Attempt Count: 1)";
                let request_id = request_id_between(&message, &head, tail).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected {head:?} then a request id then {tail:?}, \
                         got {message:?}"
                    )
                });
                assert!(
                    is_request_id(request_id),
                    "case {case:?}: the tail must carry the id of the call that was made, \
                     got {request_id:?}"
                );
            }

            // The SAME role refused twice correlates with two different calls, so the
            // id in the tail is minted per call rather than being a constant. The
            // request's own values are identical between the two, so nothing but the
            // id can differ: it is extracted from each answer and the two compared.
            let (_, repeated_name, repeated_role) = &connector_rows[0];
            let head = format!(
                "Nonexistent role or missing ssm service principal in trust policy: \
                 {repeated_role} (Service: {EXTERNAL_CLUSTER_MANAGER}, Status Code: 400, \
                 Request ID: "
            );
            let tail = ") (SDK Attempt Count: 1)";
            let mut minted: Vec<String> = Vec::new();
            for attempt in 0..2 {
                let answer = eks(
                    &router,
                    &runtime,
                    "POST",
                    "/cluster-registrations",
                    Some(serde_json::json!({
                        "name": format!("{repeated_name}-again-{attempt}"),
                        "connectorConfig": {
                            "roleArn": repeated_role,
                            "provider": "OTHER",
                        },
                    })),
                );
                let message = answer.message();
                minted.push(
                    request_id_between(&message, &head, tail)
                        .unwrap_or_else(|| {
                            panic!(
                                "attempt {attempt}: expected {head:?} then a request id then \
                                 {tail:?}, got {message:?}"
                            )
                        })
                        .to_string(),
                );
            }
            assert_ne!(
                minted[0], minted[1],
                "the request id in the tail must be minted per call, got {minted:?}"
            );

            // ---------------------------------------------------------------
            // Setup for the subnet half: a VPC with three subnets of the account,
            // created through EC2 itself.
            // ---------------------------------------------------------------
            let created = ec2(&router, &runtime, "CreateVpc", &[("CidrBlock", "10.21.0.0/16")]);
            assert_eq!(
                created.status, 200,
                "setup: CreateVpc must succeed, got {created:?}"
            );
            let vpc_id = opt_element(&created.text, "vpcId")
                .unwrap_or_else(|| panic!("setup: no <vpcId> in {}", created.text));
            let mut held: Vec<String> = Vec::new();
            for cidr_block in ["10.21.1.0/24", "10.21.2.0/24", "10.21.3.0/24"] {
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
                held.push(
                    opt_element(&answer.text, "subnetId")
                        .unwrap_or_else(|| panic!("setup: no <subnetId> in {}", answer.text)),
                );
            }

            // Ids no subnet of the account carries, in both of EC2's id forms
            // (`^subnet-[0-9a-f]{8,}$`: 8 and 17 hex digits).
            let unknown_a = "subnet-0badf00d".to_string();
            let unknown_b = "subnet-00000000deadbeef1".to_string();
            let unknown_c = "subnet-00000000deadbeef2".to_string();

            // ---------------------------------------------------------------
            // RULE: the id reported is the LAST one of the request's list that
            // resolves to nothing, and the sentence around it is EC2's own.
            // ---------------------------------------------------------------
            let rows: Vec<SubnetCase> = vec![
                SubnetCase {
                    case: "one unknown id, last in the list",
                    resource: "wrapped-one-last".to_string(),
                    subnets: vec![held[0].clone(), unknown_a.clone()],
                    reported: unknown_a.clone(),
                },
                SubnetCase {
                    case: "one unknown id, first in the list",
                    resource: "wrapped-one-first".to_string(),
                    subnets: vec![unknown_b.clone(), held[0].clone()],
                    reported: unknown_b.clone(),
                },
                SubnetCase {
                    case: "two unknown ids and nothing else",
                    resource: "wrapped-two".to_string(),
                    subnets: vec![unknown_b.clone(), unknown_a.clone()],
                    reported: unknown_a.clone(),
                },
                SubnetCase {
                    case: "two unknown ids in the other order",
                    resource: "wrapped-two-swapped".to_string(),
                    subnets: vec![unknown_a.clone(), unknown_b.clone()],
                    reported: unknown_b.clone(),
                },
                SubnetCase {
                    case: "three unknown ids",
                    resource: "wrapped-three".to_string(),
                    subnets: vec![unknown_b.clone(), unknown_c.clone(), unknown_a.clone()],
                    reported: unknown_a.clone(),
                },
                SubnetCase {
                    case: "two unknown ids around a held one",
                    resource: "wrapped-around".to_string(),
                    subnets: vec![unknown_a.clone(), held[1].clone(), unknown_c.clone()],
                    reported: unknown_c.clone(),
                },
                SubnetCase {
                    case: "two unknown ids before a held one",
                    resource: "wrapped-before".to_string(),
                    subnets: vec![unknown_c.clone(), unknown_b.clone(), held[2].clone()],
                    reported: unknown_b.clone(),
                },
                SubnetCase {
                    case: "a repeated id, which is the same question asked twice",
                    resource: "wrapped-repeated".to_string(),
                    subnets: vec![
                        unknown_a.clone(),
                        held[0].clone(),
                        held[0].clone(),
                        unknown_c.clone(),
                        unknown_c.clone(),
                    ],
                    reported: unknown_c.clone(),
                },
            ];

            // A cluster in subnets the account holds, for the `CreateNodegroup` half:
            // the node group's own `subnets` are resolved the same way.
            let host = "wrapped-subnet-host";
            let host_created = eks(
                &router,
                &runtime,
                "POST",
                "/clusters",
                Some(cluster_body(host, &held)),
            );
            assert_eq!(
                host_created.status, 200,
                "setup: the cluster a node group hangs off must be created, got \
                 {host_created:?}"
            );

            for row in &rows {
                for (operation, answer) in [
                    (
                        "CreateCluster",
                        eks(
                            &router,
                            &runtime,
                            "POST",
                            "/clusters",
                            Some(cluster_body(&row.resource, &row.subnets)),
                        ),
                    ),
                    (
                        "CreateNodegroup",
                        eks(
                            &router,
                            &runtime,
                            "POST",
                            &format!("/clusters/{host}/node-groups"),
                            Some(serde_json::json!({
                                "nodegroupName": row.resource,
                                "nodeRole": role_arn("eksNodeRole"),
                                "subnets": row.subnets,
                            })),
                        ),
                    ),
                ] {
                    let case = format!("{} / {operation}", row.case);
                    assert_eq!(
                        (answer.status, answer.error_type.as_deref()),
                        (400, Some("InvalidParameterException")),
                        "case {case:?}: an unknown subnet must be refused 400 \
                         InvalidParameterException, got {answer:?}"
                    );
                    let message = answer.message();
                    let head = format!(
                        "The subnet ID '{}' does not exist (Service: {EC2_SERVICE}; Status \
                         Code: 400; Error Code: {EC2_NOT_FOUND_CODE}; Request ID: ",
                        row.reported
                    );
                    let tail = "; Proxy: null)";
                    let request_id =
                        request_id_between(&message, &head, tail).unwrap_or_else(|| {
                            panic!(
                                "case {case:?}: expected {head:?} then a request id then \
                                 {tail:?}, got {message:?}"
                            )
                        });
                    assert!(
                        is_request_id(request_id),
                        "case {case:?}: the tail must carry the id of the call that was \
                         made, got {request_id:?}"
                    );
                    // The refusal points at ONE id: the other unknown ids of the same
                    // request are not named.
                    for other in &row.subnets {
                        assert!(
                            other == &row.reported || !message.contains(other.as_str()),
                            "case {case:?}: only the reported id may be named, but \
                             {other:?} appears in {message:?}"
                        );
                    }
                }
            }
        })
        .expect("spawn the eks wrapped-peer-refusals thread")
        .join()
        .expect("every peer refusal must be reported wrapped, naming the refused input");
}
