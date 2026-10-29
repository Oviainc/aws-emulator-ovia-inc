// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that deploys an OpenSearch domain inside a VPC is observable as
//! deployed by a client polling at AWS-sized intervals, and the VPC-endpoint access grants
//! made against the domain it published behave as the owning service's model declares.
//!
//! The rule the first suite pins: **a reporting read of a stack materialises the status
//! transitions the stack's clock already owed, so the FIRST poll made after everything the
//! operation was waiting for is ready reports its terminal status. A resource whose owning
//! service had not reported it ready when the create answered HOLDS the operation's
//! conclusion; it does not postpone it, so the poll that asks that service and learns the
//! resource came up reports the conclusion the clock owed from then, rather than charging
//! the caller a further span for having been the one to notice.** The span itself is never
//! zero — a client reading straight after `CreateStack` still sees `CREATE_IN_PROGRESS` —
//! but it is paid once, by the clock, not once per poll interval. A client polling every
//! 15-30 s (which is what an AWS-sized poll of a stack is) otherwise spends one whole
//! interval per gated operation learning nothing, and a chain of such waits outruns any
//! budget it has.
//!
//! The second suite drives the flow that rule unblocks, end to end over the wire: the
//! deployed domain's `DomainArn` is used to create an interface VPC endpoint, access to the
//! domain through it is granted to an account, listed, and revoked — including a revoke
//! naming an account that was never granted anything, which must take nothing away and add
//! nothing.
//!
//! Four services have to agree for this to hold (CloudFormation makes the calls, Amazon EC2
//! owns the VPC, the subnet and the security group, IAM mints nothing here but is mounted as
//! the router mounts it, and Amazon OpenSearch Service owns the domain and the endpoint), so
//! the suite lives here rather than in any one crate.
//!
//! The rows of the first table differ along what the rule generalises over: how many
//! resources the stack declares whose owning service reports them as still coming up (none,
//! one, two), how deep the dependency chain among its resources is, and whether the domain
//! it declares is reached inside a VPC or not. Every expected value is computed from what the
//! row declares or from the identifiers the owning services minted, never read back out of
//! the answer being checked.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element, outputs_of};
use crate::rds_docdb_neptune_signing_name_routing::authorization;

/// The Region every request here is signed for, and so the Region the stack's resources and
/// their ARNs belong to. It is the Region [`authorization`] builds a credential scope for.
const REGION: &str = "us-east-1";

/// The engine release and cluster every row deploys: one the owning service's catalogue
/// offers, so the only thing a row can fail on is the rule under test.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// How long the suite waits before the ONE poll each row of the first table makes.
///
/// Longer than every span that has to fall due for such a deployment to be finished — the
/// owning service's provisioning of a domain and the stack operation's own status transition
/// — with room for a loaded machine, so what that poll reports is what the clock owed and
/// not the outcome of a race. It stands in for the 15-30 s a real client's poll loop sleeps:
/// the rule is about the poll AFTER the spans, whatever the interval is.
const SETTLE_GRACE: Duration = Duration::from_secs(3);

/// The interval and budget of the second suite's poll loops, which wait for a status the
/// owning service owes rather than for a fixed span.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLL_LIMIT: usize = 100;

/// The `StackStatus` values the first rule is stated in.
const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `VpcEndpoint$Status` an endpoint serves traffic in.
const ACTIVE: &str = "ACTIVE";

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_opensearch_amazonopensearchservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// Call one CloudFormation operation over its awsQuery wire (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one Amazon OpenSearch Service operation over its `restJson1` wire (signing name
/// `es`), answered with the status and the parsed document.
async fn opensearch(
    router: &MultiServiceRouter,
    method: &'static str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("es.{REGION}.amazonaws.com"))
        .header("authorization", authorization("es"));
    let payload = match body {
        Some(document) => {
            builder = builder.header("content-type", "application/json");
            Bytes::from(document.to_string())
        }
        None => Bytes::new(),
    };
    let request = builder.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The `StackStatus` of ONE `DescribeStacks` call, with the answer it was read from.
async fn stack_status(router: &MultiServiceRouter, stack: &str, case: &str) -> (String, String) {
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
    );
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Every `ResourceStatusReason` the stack's events carry, which is where a resource the
/// owning service refused reports why — read only to word a failure.
async fn resource_reasons(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, events) = cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    elements(&events, "ResourceStatusReason")
}

/// One row of the first table: a template, and the outputs the deployment must publish with
/// a resolved value.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The template body, built from the stack's own name so each row's resources are its
    /// own.
    template: fn(&str) -> String,
    /// The `Outputs` keys that must each carry a resolved `OutputValue`.
    outputs: &'static [&'static str],
}

/// The recorded template: a VPC, a subnet and a security group in it, and a single-node
/// OpenSearch domain reached inside that VPC whose `AccessPolicies` is declared the way every
/// policy document is — as the policy object itself, with its intrinsics inside it.
///
/// Four dependency levels (VPC -> subnet/security group -> domain), and the domain is the one
/// resource whose owning service answers the create without reporting it ready.
fn vpc_domain_template(stack: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Vpc": {
                "Type": "AWS::EC2::VPC",
                "Properties": {
                    "CidrBlock": "10.91.0.0/16",
                    "EnableDnsSupport": true,
                    "EnableDnsHostnames": true,
                },
            },
            "Subnet": {
                "Type": "AWS::EC2::Subnet",
                "Properties": {
                    "VpcId": { "Ref": "Vpc" },
                    "CidrBlock": "10.91.1.0/24",
                    "AvailabilityZone": { "Fn::Select": [0, { "Fn::GetAZs": "" }] },
                },
            },
            "SecurityGroup": {
                "Type": "AWS::EC2::SecurityGroup",
                "Properties": {
                    "GroupDescription": "https into the domain from inside the vpc",
                    "VpcId": { "Ref": "Vpc" },
                    "SecurityGroupIngress": [{
                        "IpProtocol": "tcp",
                        "FromPort": 443,
                        "ToPort": 443,
                        "CidrIp": "10.91.0.0/16",
                    }],
                },
            },
            "Domain": {
                "Type": "AWS::OpenSearchService::Domain",
                "Properties": {
                    "DomainName": stack,
                    "EngineVersion": ENGINE_VERSION,
                    "ClusterConfig": {
                        "InstanceType": "t3.small.search",
                        "InstanceCount": 1,
                        "DedicatedMasterEnabled": false,
                        "ZoneAwarenessEnabled": false,
                    },
                    "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
                    "VPCOptions": {
                        "SubnetIds": [{ "Ref": "Subnet" }],
                        "SecurityGroupIds": [{ "Ref": "SecurityGroup" }],
                    },
                    "AccessPolicies": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "AWS": { "Fn::Sub": "arn:${AWS::Partition}:iam::${AWS::AccountId}:root" } },
                            "Action": "es:*",
                            "Resource": { "Fn::Sub": "arn:${AWS::Partition}:es:${AWS::Region}:${AWS::AccountId}:domain/*" },
                        }],
                    },
                },
            },
        },
        "Outputs": {
            "DomainArn": { "Value": { "Fn::GetAtt": ["Domain", "Arn"] } },
            "SubnetId": { "Value": { "Ref": "Subnet" } },
            "SecurityGroupId": { "Value": { "Ref": "SecurityGroup" } },
        },
    })
    .to_string()
}

const ROWS: &[Row] = &[
    Row {
        case: "a vpc domain behind a vpc, a subnet and a security group",
        stack: "sim-osve-vpc",
        template: vpc_domain_template,
        outputs: &["DomainArn", "SubnetId", "SecurityGroupId"],
    },
    Row {
        // ONE gated resource and no dependencies at all: the barrier is the whole of what
        // holds the operation.
        case: "a single public domain and nothing else",
        stack: "sim-osve-solo",
        template: |stack| {
            json!({
                "Resources": {
                    "Domain": {
                        "Type": "AWS::OpenSearchService::Domain",
                        "Properties": {
                            "DomainName": stack,
                            "EngineVersion": ENGINE_VERSION,
                            "ClusterConfig": {
                                "InstanceType": "t3.small.search",
                                "InstanceCount": 1,
                            },
                        },
                    },
                },
                "Outputs": {
                    "DomainArn": { "Value": { "Fn::GetAtt": ["Domain", "Arn"] } },
                },
            })
            .to_string()
        },
        outputs: &["DomainArn"],
    },
    Row {
        // TWO gated resources, so the operation waits for both and still concludes on the
        // poll that sees the later of them ready.
        case: "two domains in one stack",
        template: |stack| {
            json!({
                "Resources": {
                    "First": {
                        "Type": "AWS::OpenSearchService::Domain",
                        "Properties": {
                            "DomainName": format!("{stack}-a"),
                            "EngineVersion": ENGINE_VERSION,
                            "ClusterConfig": {
                                "InstanceType": "t3.small.search",
                                "InstanceCount": 1,
                            },
                        },
                    },
                    "Second": {
                        "Type": "AWS::OpenSearchService::Domain",
                        "Properties": {
                            "DomainName": format!("{stack}-b"),
                            "EngineVersion": ENGINE_VERSION,
                            "ClusterConfig": {
                                "InstanceType": "t3.small.search",
                                "InstanceCount": 1,
                            },
                        },
                    },
                },
                "Outputs": {
                    "FirstArn": { "Value": { "Fn::GetAtt": ["First", "Arn"] } },
                    "SecondArn": { "Value": { "Fn::GetAtt": ["Second", "Arn"] } },
                },
            })
            .to_string()
        },
        stack: "sim-osve-pair",
        outputs: &["FirstArn", "SecondArn"],
    },
    Row {
        // NO gated resource: the network alone. The rule is the same one — the clock's own
        // span, paid once — so this row pins that nothing about it depends on a barrier
        // being there.
        case: "a vpc, a subnet and a security group and no domain",
        stack: "sim-osve-network",
        template: |_| {
            json!({
                "Resources": {
                    "Vpc": {
                        "Type": "AWS::EC2::VPC",
                        "Properties": { "CidrBlock": "10.92.0.0/16" },
                    },
                    "Subnet": {
                        "Type": "AWS::EC2::Subnet",
                        "Properties": {
                            "VpcId": { "Ref": "Vpc" },
                            "CidrBlock": "10.92.1.0/24",
                        },
                    },
                    "SecurityGroup": {
                        "Type": "AWS::EC2::SecurityGroup",
                        "Properties": {
                            "GroupDescription": "nothing reaches it",
                            "VpcId": { "Ref": "Vpc" },
                        },
                    },
                },
                "Outputs": {
                    "SubnetId": { "Value": { "Ref": "Subnet" } },
                    "SecurityGroupId": { "Value": { "Ref": "SecurityGroup" } },
                },
            })
            .to_string()
        },
        outputs: &["SubnetId", "SecurityGroupId"],
    },
];

/// Every row: the stack is still in progress on the read that follows the create, and the
/// FIRST poll made once the spans have fallen due reports it complete with every output
/// resolved.
#[tokio::test]
async fn one_poll_after_the_spans_reports_the_stack_complete() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &(row.template)(row.stack)),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
        );

        // The span is never zero: a client reading straight after the create still sees the
        // deployment in progress.
        let (reported, described) = stack_status(&router, row.stack, case).await;
        assert_eq!(
            reported, CREATE_IN_PROGRESS,
            "case {case:?}: expected {CREATE_IN_PROGRESS} straight after the create, got \
             {described}"
        );

        // ...and it is paid ONCE. One poll, made after every span the deployment owes has
        // fallen due, is enough: this is the poll an AWS-sized loop makes next.
        tokio::time::sleep(SETTLE_GRACE).await;
        let (reported, described) = stack_status(&router, row.stack, case).await;
        assert_eq!(
            reported,
            CREATE_COMPLETE,
            "case {case:?}: expected the first poll made after the deployment's spans to \
             report {CREATE_COMPLETE}, got {reported:?} in {described} with reasons {:?}",
            resource_reasons(&router, row.stack).await
        );

        // A stack reported complete has published what it declared, which only a deployment
        // that really created every resource can report.
        let outputs: BTreeMap<String, String> = outputs_of(&described);
        for key in row.outputs {
            assert!(
                outputs.get(*key).is_some_and(|value| !value.is_empty()),
                "case {case:?}: expected {key} to carry a resolved value, got {outputs:?}"
            );
        }
    }
}

/// The `VpcEndpoint$Status` the owning service reports for `endpoint_id`, polled until it is
/// `ACTIVE` — the state the model says an endpoint serves traffic in.
async fn wait_for_active_endpoint(router: &MultiServiceRouter, endpoint_id: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..POLL_LIMIT {
        let (status, body) = opensearch(
            router,
            "POST",
            "/2021-01-01/opensearch/vpcEndpoints/describe",
            Some(json!({ "VpcEndpointIds": [endpoint_id] })),
        )
        .await;
        assert_eq!(
            status, 200,
            "DescribeVpcEndpoints({endpoint_id:?}): expected HTTP 200, got {status} with {body}"
        );
        let reported = body["VpcEndpoints"]
            .get(0)
            .cloned()
            .unwrap_or(Value::Null);
        if reported["Status"].as_str() == Some(ACTIVE) {
            return reported;
        }
        last = body;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("DescribeVpcEndpoints({endpoint_id:?}) never reported {ACTIVE}, last seen {last}");
}

/// Every `AuthorizedPrincipal$Principal` the domain reports as authorized right now.
async fn authorized_principals(router: &MultiServiceRouter, domain: &str, case: &str) -> Vec<String> {
    let (status, body) = opensearch(
        router,
        "GET",
        &format!("/2021-01-01/opensearch/domain/{domain}/listVpcEndpointAccess"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListVpcEndpointAccess, got {status} with {body}"
    );
    body["AuthorizedPrincipalList"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|entry| entry["Principal"].as_str().map(str::to_owned))
        .collect()
}

/// The whole recorded flow over the wire: deploy the VPC domain through CloudFormation, make
/// an interface VPC endpoint for the ARN it published, then grant, list and revoke access to
/// the domain through it.
#[tokio::test]
async fn a_cfn_deployed_vpc_domain_serves_the_endpoint_access_grants() {
    let router = build_router();
    let stack = "sim-osve-flow";
    let case = "the cfn-deployed vpc endpoint access flow";

    let (status, body) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &vpc_domain_template(stack)),
            // "DELETE — delete the stack if stack creation failed", which is what the
            // recorded client asks for: a failed create must therefore be observable as
            // something other than a deployment still in progress.
            ("OnFailure", "DELETE"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );

    tokio::time::sleep(SETTLE_GRACE).await;
    let (reported, described) = stack_status(&router, stack, case).await;
    assert_eq!(
        reported,
        CREATE_COMPLETE,
        "case {case:?}: expected {CREATE_COMPLETE}, got {reported:?} in {described} with \
         reasons {:?}",
        resource_reasons(&router, stack).await
    );
    let outputs: BTreeMap<String, String> = outputs_of(&described);
    let domain_arn = outputs
        .get("DomainArn")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a DomainArn output, got {outputs:?}"));
    let subnet = outputs
        .get("SubnetId")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a SubnetId output, got {outputs:?}"));
    let security_group = outputs.get("SecurityGroupId").cloned().unwrap_or_else(|| {
        panic!("case {case:?}: expected a SecurityGroupId output, got {outputs:?}")
    });

    // The deployed domain is one a client can use: its configuration is settled, so
    // `Processing` is false on the read that follows the completed stack.
    let (status, reported) = opensearch(
        &router,
        "GET",
        &format!("/2021-01-01/opensearch/domain/{stack}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeDomain, got {status} with {reported}"
    );
    assert_eq!(
        reported["DomainStatus"]["Processing"].as_bool(),
        Some(false),
        "case {case:?}: expected the deployed domain to report no configuration change in \
         flight, got {reported}"
    );
    assert_eq!(
        reported["DomainStatus"]["ARN"].as_str(),
        Some(domain_arn.as_str()),
        "case {case:?}: expected the domain to report the ARN the stack published, got \
         {reported}"
    );

    // The endpoint is created for the ARN the stack published, and settles ACTIVE.
    let (status, created) = opensearch(
        &router,
        "POST",
        "/2021-01-01/opensearch/vpcEndpoints",
        Some(json!({
            "DomainArn": domain_arn,
            "VpcOptions": {
                "SubnetIds": [subnet],
                "SecurityGroupIds": [security_group],
            },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpcEndpoint, got {status} with {created}"
    );
    let endpoint_id = created["VpcEndpoint"]["VpcEndpointId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a created VpcEndpointId, got {created}")
        })
        .to_owned();
    let settled = wait_for_active_endpoint(&router, &endpoint_id).await;
    assert_eq!(
        settled["DomainArn"].as_str(),
        Some(domain_arn.as_str()),
        "case {case:?}: expected the endpoint to report the domain it was created for, got \
         {settled}"
    );

    // The account this emulator serves is the principal the grant names, taken from the
    // engine rather than from any answer under test.
    let account = engine::emulator_account_id().to_string();
    let (status, granted) = opensearch(
        &router,
        "POST",
        &format!("/2021-01-01/opensearch/domain/{stack}/authorizeVpcEndpointAccess"),
        Some(json!({ "Account": account })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AuthorizeVpcEndpointAccess, got {status} with \
         {granted}"
    );
    // "The Amazon Web Services account ID to grant access to": the principal is that
    // account, named either by its id or by the root ARN of it.
    let principal = granted["AuthorizedPrincipal"]["Principal"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an AuthorizedPrincipal, got {granted}"))
        .to_owned();
    assert!(
        principal == account || principal == format!("arn:aws:iam::{account}:root"),
        "case {case:?}: expected the grant to name the account {account:?} or its root ARN, \
         got {principal:?}"
    );

    let listed = authorized_principals(&router, stack, case).await;
    assert_eq!(
        listed,
        vec![principal.clone()],
        "case {case:?}: expected the listing to report exactly the granted principal, got \
         {listed:?}"
    );

    // A revoke naming an account that was never granted anything takes nothing away — and
    // adds nothing.
    let never_granted = "111111111111";
    let (status, revoked) = opensearch(
        &router,
        "POST",
        &format!("/2021-01-01/opensearch/domain/{stack}/revokeVpcEndpointAccess"),
        Some(json!({ "Account": never_granted })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 revoking an account that holds no grant, got \
         {status} with {revoked}"
    );
    let listed = authorized_principals(&router, stack, case).await;
    assert_eq!(
        listed,
        vec![principal.clone()],
        "case {case:?}: expected the listing to be unchanged by a revoke of {never_granted}, \
         got {listed:?}"
    );

    // ...and revoking the grant that WAS made removes it.
    let (status, revoked) = opensearch(
        &router,
        "POST",
        &format!("/2021-01-01/opensearch/domain/{stack}/revokeVpcEndpointAccess"),
        Some(json!({ "Account": account })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from RevokeVpcEndpointAccess, got {status} with \
         {revoked}"
    );
    let listed = authorized_principals(&router, stack, case).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: expected the revoked grant to be gone from the listing, got {listed:?}"
    );
}
