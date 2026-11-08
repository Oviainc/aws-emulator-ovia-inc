// SPDX-License-Identifier: CC0-1.0

//! What a stack publishes for the `Fn::GetAtt` attributes of an `AWS::EC2::Instance` it
//! launched: the values Amazon EC2 itself reports for that instance.
//!
//! The rule under test: **an attribute a stack publishes for a resource is the value the
//! OWNING SERVICE reports for it, never a value CloudFormation computed.** The published
//! attributes of an instance are the zone it was placed in and its private and public
//! addresses and DNS names, every one of which only Amazon EC2 can produce — the zone is the
//! one it placed the instance in, the private address is one it chose out of the subnet, and a
//! public one exists only where it assigned one. So each `Fn::GetAtt` output of the stack has
//! to equal the member Amazon EC2 reports for that instance, which is what this suite reads
//! back: one row per published attribute, against the element of `DescribeInstances` that
//! carries it — including the two whose wire name is not the attribute's (`ipAddress`,
//! `dnsName`).
//!
//! This is the half of that type's deployment the CloudFormation crate's own suite does not
//! reach: that one owns WHICH stack status a launch Amazon EC2 refuses leads to
//! (`cloudformation_ec2_instance_rollback.rs` in `services/service-com-amazonaws-
//! cloudformation-cloudformation/tests/all/`), and this one owns what the stack publishes when
//! the launch goes through. Both need Amazon EC2 and AWS Systems Manager beside
//! CloudFormation; this one runs them on the ASSEMBLED router, as the emulator serves them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{block_of, form_encode, opt_element, outputs_of};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The Availability Zone the stack's subnet is chosen in: the Region's first, which offers
/// every instance type the catalog publishes.
const ZONE: &str = "us-east-1a";

/// The instance type the stack declares — one Amazon EC2 publishes, so the launch goes
/// through and there are attributes to read at all.
const INSTANCE_TYPE: &str = "t3.micro";

/// The public Systems Manager parameter the template resolves its AMI from.
const IMAGE_PARAMETER: &str =
    "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64";

/// How long a poll of a stack's status waits between reads, and how many times it reads.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLL_ATTEMPTS: usize = 60;

/// Each `Fn::GetAtt` attribute `AWS::EC2::Instance` publishes, with the element of an
/// `ec2Query` answer that carries the member Amazon EC2 reports it as.
///
/// Source: the type's "Return values" section read against the `Instance` shape's own wire
/// names (`smithy_query(query="shape",shape="Instance")`). The zone is the one member Amazon
/// EC2 reports INSIDE the instance's `placement` rather than directly, which is why its row
/// carries that parent.
const PUBLISHED_ATTRIBUTES: &[(&str, &str, Option<&str>)] = &[
    ("AvailabilityZone", "availabilityZone", Some("placement")),
    ("PrivateDnsName", "privateDnsName", None),
    ("PrivateIp", "privateIpAddress", None),
    ("PublicDnsName", "dnsName", None),
    ("PublicIp", "ipAddress", None),
];

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One form-encoded call on a query wire, as its HTTP status and body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon EC2 operation over its ec2Query wire.
async fn ec2(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "ec2", "2016-11-15", action, members).await
}

/// A subnet of the account's default VPC in [`ZONE`], read out of Amazon EC2 itself: the
/// template cannot invent a subnet id.
async fn subnet_in_the_first_zone(router: &MultiServiceRouter) -> String {
    let (status, body) = ec2(
        router,
        "DescribeSubnets",
        &[
            ("Filter.1.Name", "availability-zone"),
            ("Filter.1.Value.1", ZONE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeSubnets must report the default VPC's subnets, got {status} with body {body}"
    );
    opt_element(&body, "subnetId")
        .unwrap_or_else(|| panic!("a fresh account must hold a subnet in {ZONE}: {body}"))
}

/// The template the stack deploys: one instance, with an output per published attribute.
fn template(subnet_id: &str) -> Value {
    let mut outputs = serde_json::Map::new();
    outputs.insert(
        "InstanceId".to_string(),
        json!({ "Value": { "Ref": "TheInstance" } }),
    );
    for (attribute, _, _) in PUBLISHED_ATTRIBUTES {
        outputs.insert(
            (*attribute).to_string(),
            json!({ "Value": { "Fn::GetAtt": ["TheInstance", attribute] } }),
        );
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Parameters": {
            "ImageId": {
                "Type": "AWS::SSM::Parameter::Value<AWS::EC2::Image::Id>",
                "Default": IMAGE_PARAMETER,
            }
        },
        "Resources": {
            "TheInstance": {
                "Type": "AWS::EC2::Instance",
                "Properties": {
                    "ImageId": { "Ref": "ImageId" },
                    "InstanceType": INSTANCE_TYPE,
                    "SubnetId": subnet_id,
                }
            }
        },
        "Outputs": outputs,
    })
}

/// Every `Fn::GetAtt` output of a stack whose `AWS::EC2::Instance` launched carries the member
/// Amazon EC2 reports for that instance — and an address that service assigned none of is
/// published as no value rather than as something CloudFormation made up.
#[tokio::test]
async fn the_published_instance_attributes_are_what_amazon_ec2_reports() {
    let case = "an instance the stack launched";
    let stack_name = "instance-attributes";
    let router = build_router();
    let subnet_id = subnet_in_the_first_zone(&router).await;

    let body = template(&subnet_id).to_string();
    let (status, created) = cloudformation(
        &router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &body)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {created}"
    );

    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        tokio::time::sleep(POLL_INTERVAL).await;
        let (status, body) =
            cloudformation(&router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
        );
        described = body;
        if !opt_element(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
    }
    let stack_status = opt_element(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        stack_status, "CREATE_COMPLETE",
        "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {stack_status:?} in \
         {described}"
    );

    let outputs: BTreeMap<String, String> = outputs_of(&described);
    let instance_id = outputs.get("InstanceId").cloned().unwrap_or_default();
    let (status, reported) =
        ec2(&router, "DescribeInstances", &[("InstanceId.1", &instance_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Amazon EC2 to hold {instance_id}, got {status} with body \
         {reported}"
    );

    for (attribute, element, parent) in PUBLISHED_ATTRIBUTES {
        // What Amazon EC2 reports for the member, read inside its own parent where the answer
        // nests it — so an element of the same name elsewhere cannot stand in for it. A member
        // that service reports nothing for is the empty value.
        let scoped = match parent {
            Some(parent) => block_of(&reported, parent).unwrap_or_default().to_string(),
            None => reported.clone(),
        };
        let reported_value = opt_element(&scoped, element).unwrap_or_default();
        let published = outputs.get(*attribute).cloned().unwrap_or_default();
        assert_eq!(
            published, reported_value,
            "case {case:?}: expected the stack's {attribute} output to be the <{element}> \
             Amazon EC2 reports ({reported_value:?}), got {published:?} in {outputs:?}"
        );
    }

    // The zone is the one the declared SUBNET is in, so the attribute is a value that service
    // decided about this instance rather than one that could have come from anywhere.
    let zone = outputs.get("AvailabilityZone").cloned().unwrap_or_default();
    assert_eq!(
        zone, ZONE,
        "case {case:?}: expected the instance's zone to be the declared subnet's ({ZONE}), got \
         {zone:?}"
    );
    // And the two members Amazon EC2 does report are published as the values it reports rather
    // than as empty ones, so the comparisons above are not all passing on emptiness.
    for attribute in ["PrivateIp", "PrivateDnsName"] {
        let published = outputs.get(attribute).cloned().unwrap_or_default();
        assert!(
            !published.is_empty(),
            "case {case:?}: expected the stack to publish the {attribute} Amazon EC2 assigned, \
             got {published:?} in {outputs:?}"
        );
    }
}
