// SPDX-License-Identifier: CC0-1.0

//! The cluster-parameter-group control plane of the shared `rds` Query surface, through the
//! PRODUCTION router.
//!
//! Amazon RDS, Amazon DocumentDB and Amazon Neptune are three Smithy models of ONE AWS
//! front door: all three declare `sigv4Name`/`endpointPrefix` `rds`, so every AWS SDK
//! client of any of them signs `rds`, and that name resolves to the one service whose own
//! identity it is — Amazon RDS (see `all/rds_docdb_neptune_signing_name_routing.rs` for the
//! routing rule itself, and each crate's `src/service.rs` for the claim). An SDK
//! DocumentDB client's `CreateDBClusterParameterGroup` therefore has to be SERVED there,
//! which is what this suite pins: with every service mounted, the whole group lifecycle
//! answers in the awsQuery XML envelope, with the modelled status and error code, and never
//! a router-level JSON refusal.
//!
//! The rows span the lifecycle's own classes: a create and the read that must find it, a
//! duplicate create, a modify and the read that must report it, a reset refused by the
//! group's pending state, a describe of a group that is not there, and the delete plus the
//! read that must no longer find it.
//!
//! This suite needs the WHOLE router: the shared signing name only resolves once all three
//! services are mounted. Assembling the router costs stack (see `boot_serve.rs`), so it is
//! built — and the requests driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The awsQuery API version all three models of this front door share.
const API_VERSION: &str = "2014-10-31";

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The group this suite works with, and the DocumentDB engine family it is created for.
const GROUP: &str = "router-probe-cpg";
const FAMILY: &str = "docdb5.0";

/// One step of the lifecycle: the `Action`, its form parameters, the status the owning
/// service must answer and a fragment its answer must carry.
struct Step {
    case: &'static str,
    action: &'static str,
    params: &'static [(&'static str, &'static str)],
    expected_status: u16,
    expected_fragment: &'static str,
}

#[tokio::test]
async fn the_shared_rds_surface_serves_the_cluster_parameter_group_lifecycle() {
    let joined = std::thread::Builder::new()
        .name("rds-cluster-parameter-groups".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let steps: [Step; 8] = [
                Step {
                    // The request the issue reports as `500 AmbiguousService`.
                    case: "create the group",
                    action: "CreateDBClusterParameterGroup",
                    params: &[
                        ("DBClusterParameterGroupName", GROUP),
                        ("DBParameterGroupFamily", FAMILY),
                        ("Description", "router probe"),
                    ],
                    expected_status: 200,
                    expected_fragment: "<CreateDBClusterParameterGroupResponse",
                },
                Step {
                    // ...and the group it stored reads back through the same router.
                    case: "describe the group",
                    action: "DescribeDBClusterParameterGroups",
                    params: &[("DBClusterParameterGroupName", GROUP)],
                    expected_status: 200,
                    expected_fragment: "<DBClusterParameterGroupArn>",
                },
                Step {
                    // A second create under the same name is the modelled refusal, in the
                    // XML envelope an awsQuery client can parse — the typed error the JSON
                    // refusal denied it.
                    case: "create it again",
                    action: "CreateDBClusterParameterGroup",
                    params: &[
                        ("DBClusterParameterGroupName", GROUP),
                        ("DBParameterGroupFamily", FAMILY),
                        ("Description", "router probe"),
                    ],
                    expected_status: 400,
                    expected_fragment: "<Code>DBParameterGroupAlreadyExists</Code>",
                },
                Step {
                    case: "set a parameter",
                    action: "ModifyDBClusterParameterGroup",
                    params: &[
                        ("DBClusterParameterGroupName", GROUP),
                        ("Parameters.Parameter.1.ParameterName", "tls"),
                        ("Parameters.Parameter.1.ParameterValue", "disabled"),
                        ("Parameters.Parameter.1.ApplyMethod", "pending-reboot"),
                    ],
                    expected_status: 200,
                    expected_fragment: "<ModifyDBClusterParameterGroupResponse",
                },
                Step {
                    // The value is computed from state, not echoed: it comes back on the
                    // group's own parameter list.
                    case: "read the parameter back",
                    action: "DescribeDBClusterParameters",
                    params: &[("DBClusterParameterGroupName", GROUP)],
                    expected_status: 200,
                    expected_fragment: "<ParameterValue>disabled</ParameterValue>",
                },
                Step {
                    // A static parameter's change waits for a reboot, so the group is not
                    // in a state that can be reset.
                    case: "reset while the change is pending",
                    action: "ResetDBClusterParameterGroup",
                    params: &[
                        ("DBClusterParameterGroupName", GROUP),
                        ("ResetAllParameters", "true"),
                    ],
                    expected_status: 400,
                    expected_fragment: "<Code>InvalidDBParameterGroupState</Code>",
                },
                Step {
                    case: "delete the group",
                    action: "DeleteDBClusterParameterGroup",
                    params: &[("DBClusterParameterGroupName", GROUP)],
                    expected_status: 200,
                    expected_fragment: "<DeleteDBClusterParameterGroupResponse",
                },
                Step {
                    // ...and it is gone from state afterwards.
                    case: "describe the deleted group",
                    action: "DescribeDBClusterParameterGroups",
                    params: &[("DBClusterParameterGroupName", GROUP)],
                    expected_status: 404,
                    expected_fragment: "<Code>DBParameterGroupNotFound</Code>",
                },
            ];

            for step in steps {
                let mut body = format!("Action={}&Version={API_VERSION}", step.action);
                for (name, value) in step.params {
                    body.push('&');
                    body.push_str(name);
                    body.push('=');
                    body.push_str(value);
                }
                let request = http::Request::builder()
                    .method("POST")
                    .uri("http://localhost/")
                    .header("host", "localhost")
                    .header("content-type", "application/x-www-form-urlencoded")
                    // Every SDK client of this front door signs `rds`, whichever of the
                    // three APIs it speaks.
                    .header(
                        "authorization",
                        format!(
                            "AWS4-HMAC-SHA256 \
                             Credential=AKIDEXAMPLE/20260101/{REGION}/rds/aws4_request, \
                             SignedHeaders=host, Signature=not-verified"
                        ),
                    )
                    .body(bytes::Bytes::from(body))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                let case = step.case;

                // Neither routing failure may happen: `AmbiguousService` (500, several
                // claimants) nor `UnknownService` (404, none).
                assert!(
                    !rendered.contains("AmbiguousService") && !rendered.contains("UnknownService"),
                    "case {case:?}: the request must reach exactly one service, got {status} \
                     with {rendered}"
                );
                // Whatever the answer is, it is an awsQuery XML envelope a query-protocol
                // client can parse — never the router's JSON `__type` document.
                assert!(
                    rendered.trim_start().starts_with('<') && !rendered.contains("__type"),
                    "case {case:?}: an awsQuery client must get XML, got {status} with \
                     {rendered}"
                );
                assert_eq!(
                    status, step.expected_status,
                    "case {case:?}: expected HTTP {}, got {status} with {rendered}",
                    step.expected_status
                );
                assert!(
                    rendered.contains(step.expected_fragment),
                    "case {case:?}: expected {} in the answer, got {rendered}",
                    step.expected_fragment
                );
            }
        })
        .expect("spawn the lifecycle thread")
        .join();
    joined.expect("the shared rds surface must serve the cluster parameter group lifecycle");
}
