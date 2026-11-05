// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an Amazon EC2 estate — a VPC with its gateway,
//! subnet, route table and security group, an IAM role with its instance profile, and an
//! Amazon EBS volume — settles at `CREATE_COMPLETE`, and each resource it publishes an
//! identifier for is one the owning service really holds.
//!
//! The rule under test: **a resource type CloudFormation provisions is created in the
//! service that owns it, carrying exactly the configuration its declaration states — so the
//! physical resource id the stack publishes resolves in that service, and a property the
//! owning API has no member for is applied through the call that owns it rather than left
//! unasked for or added BESIDE what the service already gave the resource.** Two
//! consequences of that rule are what this suite drives:
//!
//! * a new security group comes with one outbound rule of Amazon EC2's own — the one that
//!   allows all outbound traffic — so a declaration that states outbound rules REPLACES it:
//!   the group ends up carrying the declared rules and nothing else. Authorizing beside the
//!   default rule instead is not only a group the template did not declare, it is a create
//!   Amazon EC2 refuses outright when a declared rule repeats the default one, since a
//!   security group cannot hold the same rule twice;
//! * an `AWS::EC2::Volume` is a volume of Amazon EC2, created through `CreateVolume`, whose
//!   `vol-…` identifier that service assigns — "Ref returns the volume ID" — so the value
//!   the stack publishes for it is one `DescribeVolumes`, `AttachVolume` and every other
//!   call that takes a volume id accepts.
//!
//! The rows of the first table differ along what the rule generalises over for a group's
//! outbound rules: how many the declaration states (none, one, two), whether the one it
//! states is the all-traffic rule the service already gave the group or a narrower one, how
//! the list is written (a sequence, or a single rule without one), and whether inbound rules
//! are declared beside them. The rows of the second differ along the volume's size, type,
//! provisioned IOPS and throughput, its encryption and its zone.
//!
//! The last test is the end-to-end estate the whole suite exists for, and the one that needs
//! three crates to agree (CloudFormation makes the calls; Amazon EC2 and IAM serve them):
//! one stack carrying every resource type of that estate, whose outputs each resolve in
//! the service that owns the resource, and whose deletion takes the group and the volume
//! away again.
//!
//! That stack's internet gateway, its gateway attachment and its default route are
//! DECLARED-ONLY here: the Amazon EC2 operations that would create them
//! (`CreateInternetGateway`, `AttachInternetGateway`, `CreateRoute`) are not reached for a
//! declared resource of those types yet, each with an open issue of its own, so the stack
//! records those resources as the template declared them and asks nothing of any service for
//! them. Reaching `CREATE_COMPLETE` with them in the template is therefore not a claim that
//! they exist in Amazon EC2 — nothing below reads one back — and the day they are provisioned
//! this suite is where the reads for them belong. Its route table and its subnet association
//! ARE created in Amazon EC2, as is every other resource of the estate, and the last test
//! reads each one back there.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{block_of, elements, form_encode, opt_element, outputs_of};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// Long enough for the stack's own clock-owed span and for the span Amazon EC2 gives a
/// volume it answers `creating` to come due, so the stack reports its outputs and the volume
/// is reported in the state a usable one is in.
const PAST_PROVISIONING_SPANS: Duration = Duration::from_millis(1_500);

/// How long a poll of a stack's status waits between reads, and how many times it reads: a
/// stack is owed its terminal status once the services that own its resources report them
/// ready, which only a read asks them about, so a client polls — which is what this is.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLL_ATTEMPTS: usize = 40;

/// How many hexadecimal characters an Amazon EC2 resource identifier carries after its
/// prefix: the current (long) id form is 17 (`vol-0123456789abcdef0`).
const EC2_ID_HEX_DIGITS: usize = 17;

/// The CIDR block every VPC here is declared with, and the subnet block inside it.
const VPC_CIDR: &str = "10.0.0.0/16";
const SUBNET_CIDR: &str = "10.0.0.0/24";

/// The zone the resources that take one are declared in, and a second one, so a row can
/// place a volume somewhere else.
const ZONE: &str = "us-east-1a";
const OTHER_ZONE: &str = "us-east-1b";

/// The outbound rule Amazon EC2 gives every new security group: the all-protocols rule over
/// every IPv4 address.
const ALL_PROTOCOLS: &str = "-1";
const ALL_IPV4: &str = "0.0.0.0/0";

/// The three tag keys CloudFormation records on every resource it creates, naming the stack
/// and the resource's place in it.
const STACK_TAG_KEYS: [&str; 3] = [
    "aws:cloudformation:stack-name",
    "aws:cloudformation:stack-id",
    "aws:cloudformation:logical-id",
];

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
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
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request and what each handler reads its Region from. The signature is never
/// checked: this router runs with request authentication off.
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
    host: &str,
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
        .header("host", host)
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
    query(
        router,
        "cloudformation",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "2010-05-15",
        action,
        members,
    )
    .await
}

/// Call one Amazon EC2 operation over its ec2Query wire.
async fn ec2(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(
        router,
        "ec2",
        &format!("ec2.{REGION}.amazonaws.com"),
        "2016-11-15",
        action,
        members,
    )
    .await
}

/// Call one IAM operation over its awsQuery wire. IAM is global, so the request is signed
/// for this suite's Region only because that is what routes it.
async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(
        router,
        "iam",
        "iam.amazonaws.com",
        "2010-05-08",
        action,
        members,
    )
    .await
}

/// Whether `value` is an Amazon EC2 identifier of the published form: `<prefix>-` plus
/// [`EC2_ID_HEX_DIGITS`] lowercase hexadecimal characters.
fn is_ec2_identifier(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(&format!("{prefix}-"))
        .is_some_and(|digits| {
            digits.len() == EC2_ID_HEX_DIGITS
                && digits
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
}

/// Create a stack, poll it to a terminal status the way a client does, and answer its
/// `DescribeStacks` body.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &Value,
    extra: &[(&str, &str)],
) -> String {
    let template = template.to_string();
    let mut members: Vec<(&str, &str)> =
        vec![("StackName", stack_name), ("TemplateBody", &template)];
    members.extend_from_slice(extra);
    let (status, body) = cloudformation(router, "CreateStack", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    poll_to_terminal(router, case, stack_name).await
}

/// Read a stack until it reports a status no longer in progress, and answer that read's
/// body. The last read stands where the stack is still in progress after the bound, so the
/// caller's own assertion reports what it was waiting for.
async fn poll_to_terminal(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        tokio::time::sleep(POLL_INTERVAL).await;
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {body}"
        );
        described = body;
        if !opt_element(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
    }
    described
}

/// The reason the stack's own events carry for every resource that failed, so a stack that
/// did not reach the status a row expects says WHICH resource its owning service refused.
async fn failures(router: &MultiServiceRouter, stack_name: &str) -> String {
    let (_, events) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    super::query_wire::members_of(&events)
        .into_iter()
        .filter(|member| member.contains("_FAILED"))
        .map(|member| {
            format!(
                "{:?} {:?} {:?}",
                opt_element(member, "LogicalResourceId").unwrap_or_default(),
                opt_element(member, "ResourceStatus").unwrap_or_default(),
                opt_element(member, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Assert the stack settled at `CREATE_COMPLETE`, quoting what its events say failed when it
/// did not.
async fn assert_create_complete(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    described: &str,
) {
    let status = opt_element(described, "StackStatus").unwrap_or_default();
    if status != "CREATE_COMPLETE" {
        panic!(
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {status:?}; its \
             failed resources: {}",
            failures(router, stack_name).await
        );
    }
}

// ---------------------------------------------------------------------------
// A security group's outbound rules
// ---------------------------------------------------------------------------

/// One row of the security-group table: the rule properties the group is declared with, the
/// outbound rules Amazon EC2 must end up reporting for it as `(protocol, CIDR)` pairs in the
/// order the group holds them, the port range each of those rules carries (only the rules
/// that have one, in the same order), and the inbound rules it must report the same way.
struct RuleRow {
    case: &'static str,
    /// The `SecurityGroupIngress` / `SecurityGroupEgress` properties, as they are written
    /// into the declaration.
    rules: fn() -> Value,
    egress: &'static [(&'static str, &'static str)],
    egress_ports: &'static [&'static str],
    ingress: &'static [(&'static str, &'static str)],
}

const RULE_ROWS: &[RuleRow] = &[
    RuleRow {
        case: "no outbound rule declared: the group keeps the one Amazon EC2 gave it",
        rules: || json!({}),
        egress: &[(ALL_PROTOCOLS, ALL_IPV4)],
        egress_ports: &[],
        ingress: &[],
    },
    RuleRow {
        case: "the all-traffic rule declared again: the group carries it once",
        rules: || {
            json!({
                "SecurityGroupEgress": [{ "IpProtocol": ALL_PROTOCOLS, "CidrIp": ALL_IPV4 }]
            })
        },
        egress: &[(ALL_PROTOCOLS, ALL_IPV4)],
        egress_ports: &[],
        ingress: &[],
    },
    RuleRow {
        case: "one narrower outbound rule: the default rule is gone",
        rules: || {
            json!({
                "SecurityGroupEgress": [{
                    "IpProtocol": "tcp",
                    "FromPort": 443,
                    "ToPort": 443,
                    "CidrIp": VPC_CIDR,
                }]
            })
        },
        egress: &[("tcp", VPC_CIDR)],
        egress_ports: &["443"],
        ingress: &[],
    },
    RuleRow {
        case: "two outbound rules of different protocols",
        rules: || {
            json!({
                "SecurityGroupEgress": [
                    { "IpProtocol": "tcp", "FromPort": 443, "ToPort": 443, "CidrIp": VPC_CIDR },
                    { "IpProtocol": "udp", "FromPort": 53, "ToPort": 53, "CidrIp": ALL_IPV4 },
                ]
            })
        },
        egress: &[("tcp", VPC_CIDR), ("udp", ALL_IPV4)],
        egress_ports: &["443", "53"],
        ingress: &[],
    },
    RuleRow {
        case: "one outbound rule written without the sequence syntax",
        rules: || {
            json!({
                "SecurityGroupEgress": {
                    "IpProtocol": "tcp",
                    "FromPort": 80,
                    "ToPort": 80,
                    "CidrIp": VPC_CIDR,
                }
            })
        },
        egress: &[("tcp", VPC_CIDR)],
        egress_ports: &["80"],
        ingress: &[],
    },
    RuleRow {
        case: "an inbound rule declared beside an outbound one",
        rules: || {
            json!({
                "SecurityGroupIngress": [{
                    "IpProtocol": "tcp",
                    "FromPort": 22,
                    "ToPort": 22,
                    "CidrIp": ALL_IPV4,
                }],
                "SecurityGroupEgress": [{
                    "IpProtocol": "tcp",
                    "FromPort": 443,
                    "ToPort": 443,
                    "CidrIp": VPC_CIDR,
                }],
            })
        },
        egress: &[("tcp", VPC_CIDR)],
        egress_ports: &["443"],
        ingress: &[("tcp", ALL_IPV4)],
    },
];

/// The `(protocol, CIDR)` pairs one direction of a reported group carries, in the order the
/// answer reports them.
///
/// `direction` is the member of `SecurityGroup` that holds that side's permissions
/// (`ipPermissions` / `ipPermissionsEgress`). Each row declares one IPv4 range per rule, so
/// the protocols and the ranges of that block pair up by position.
fn permissions_of(reported: &str, direction: &str) -> Vec<(String, String)> {
    let Some(block) = block_of(reported, direction) else {
        return Vec::new();
    };
    elements(block, "ipProtocol")
        .into_iter()
        .zip(elements(block, "cidrIp"))
        .collect()
}

/// The port ranges one direction of a reported group carries, in the same order — read as
/// the `fromPort` of each rule that has one.
fn ports_of(reported: &str, direction: &str) -> Vec<String> {
    block_of(reported, direction)
        .map(|block| elements(block, "fromPort"))
        .unwrap_or_default()
}

/// A security group a stack declares carries exactly the rules its declaration states in
/// each direction: the all-traffic outbound rule Amazon EC2 gives a new group is kept where
/// the declaration states no outbound rule and REPLACED where it states its own, and the
/// group resolves in Amazon EC2 under the id the stack published for it.
#[tokio::test]
async fn a_declared_security_group_carries_exactly_the_rules_it_declares() {
    let router = build_router();

    for (index, row) in RULE_ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("group-rules-{index}");
        let mut properties = json!({
            "GroupDescription": "Security group for EC2 instance",
            "VpcId": { "Ref": "Vpc" },
        });
        if let (Some(properties), Some(rules)) =
            (properties.as_object_mut(), (row.rules)().as_object())
        {
            for (property, value) in rules {
                properties.insert(property.clone(), value.clone());
            }
        }
        let template = json!({
            "Resources": {
                "Vpc": { "Type": "AWS::EC2::VPC", "Properties": { "CidrBlock": VPC_CIDR } },
                "SecurityGroup": {
                    "Type": "AWS::EC2::SecurityGroup",
                    "Properties": properties,
                },
            },
            "Outputs": {
                "SecurityGroupId": { "Value": { "Fn::GetAtt": ["SecurityGroup", "GroupId"] } },
            },
        });

        let described = deploy(&router, case, &stack_name, &template, &[]).await;
        assert_create_complete(&router, case, &stack_name, &described).await;
        let group_id = outputs_of(&described)
            .get("SecurityGroupId")
            .cloned()
            .unwrap_or_default();
        assert!(
            is_ec2_identifier(&group_id, "sg"),
            "case {case:?}: expected the stack to publish an sg-<17 hexadecimal characters> \
             id, got {group_id:?}"
        );

        let (status, reported) = ec2(
            &router,
            "DescribeSecurityGroups",
            &[("GroupId.1", &group_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Amazon EC2 to hold the group the stack published \
             ({group_id}), got HTTP {status} with body {reported}"
        );

        let expected_egress: Vec<(String, String)> = row
            .egress
            .iter()
            .map(|(protocol, cidr)| (protocol.to_string(), cidr.to_string()))
            .collect();
        let egress = permissions_of(&reported, "ipPermissionsEgress");
        assert_eq!(
            egress, expected_egress,
            "case {case:?}: expected the group's outbound rules to be {expected_egress:?}, \
             got {egress:?} from {reported}"
        );
        let expected_ports: Vec<String> =
            row.egress_ports.iter().map(ToString::to_string).collect();
        let ports = ports_of(&reported, "ipPermissionsEgress");
        assert_eq!(
            ports, expected_ports,
            "case {case:?}: expected the outbound rules' port ranges to be \
             {expected_ports:?}, got {ports:?}"
        );
        let expected_ingress: Vec<(String, String)> = row
            .ingress
            .iter()
            .map(|(protocol, cidr)| (protocol.to_string(), cidr.to_string()))
            .collect();
        let ingress = permissions_of(&reported, "ipPermissions");
        assert_eq!(
            ingress, expected_ingress,
            "case {case:?}: expected the group's inbound rules to be {expected_ingress:?}, \
             got {ingress:?} from {reported}"
        );
    }
}

// ---------------------------------------------------------------------------
// An Amazon EBS volume
// ---------------------------------------------------------------------------

/// One row of the volume table: the properties the volume is declared with, the three
/// members Amazon EC2 must report every volume with, and the further members a row that
/// declares them must see reported — a property dropped on the way to `CreateVolume` is a
/// volume the template did not declare.
struct VolumeRow {
    case: &'static str,
    properties: fn() -> Value,
    zone: &'static str,
    size: &'static str,
    volume_type: &'static str,
    /// `(the member Amazon EC2 reports it as, the value it must carry)` for each further
    /// property this row declares.
    declared: &'static [(&'static str, &'static str)],
}

const VOLUME_ROWS: &[VolumeRow] = &[
    VolumeRow {
        case: "a 50 GiB gp3 volume",
        properties: || {
            json!({ "AvailabilityZone": ZONE, "Size": 50, "VolumeType": "gp3" })
        },
        zone: ZONE,
        size: "50",
        volume_type: "gp3",
        declared: &[],
    },
    VolumeRow {
        case: "the smallest gp2 volume",
        properties: || {
            json!({ "AvailabilityZone": ZONE, "Size": 1, "VolumeType": "gp2" })
        },
        zone: ZONE,
        size: "1",
        volume_type: "gp2",
        declared: &[],
    },
    VolumeRow {
        case: "an io1 volume with provisioned IOPS",
        properties: || {
            json!({
                "AvailabilityZone": ZONE,
                "Size": 4,
                "VolumeType": "io1",
                "Iops": 100,
            })
        },
        zone: ZONE,
        size: "4",
        volume_type: "io1",
        declared: &[("iops", "100")],
    },
    VolumeRow {
        case: "an encrypted gp3 volume with its own throughput",
        properties: || {
            json!({
                "AvailabilityZone": ZONE,
                "Size": 125,
                "VolumeType": "gp3",
                "Throughput": 200,
                "Encrypted": true,
            })
        },
        zone: ZONE,
        size: "125",
        volume_type: "gp3",
        declared: &[("throughput", "200"), ("encrypted", "true")],
    },
    VolumeRow {
        case: "a magnetic volume in another zone",
        properties: || {
            json!({
                "AvailabilityZone": OTHER_ZONE,
                "Size": 20,
                "VolumeType": "standard",
            })
        },
        zone: OTHER_ZONE,
        size: "20",
        volume_type: "standard",
        declared: &[],
    },
];

/// An `AWS::EC2::Volume` a stack declares is a volume of Amazon EC2: the stack publishes the
/// `vol-…` id that service assigned it, the volume resolves there carrying the zone, size
/// and type the template declared, it is reported in a state a usable volume is in, and it
/// carries the tags CloudFormation gives a resource it created.
#[tokio::test]
async fn a_declared_ebs_volume_is_created_in_amazon_ec2() {
    let router = build_router();

    for (index, row) in VOLUME_ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("ebs-estate-volume-{index}");
        let template = json!({
            "Resources": {
                "EbsVolume": { "Type": "AWS::EC2::Volume", "Properties": (row.properties)() },
            },
            "Outputs": {
                "EBSVolumeId": { "Value": { "Ref": "EbsVolume" } },
                "VolumeIdAttribute": { "Value": { "Fn::GetAtt": ["EbsVolume", "VolumeId"] } },
            },
        });

        let described = deploy(&router, case, &stack_name, &template, &[]).await;
        assert_create_complete(&router, case, &stack_name, &described).await;
        let outputs = outputs_of(&described);
        let volume_id = outputs.get("EBSVolumeId").cloned().unwrap_or_default();
        assert!(
            is_ec2_identifier(&volume_id, "vol"),
            "case {case:?}: expected the stack to publish a vol-<17 hexadecimal characters> \
             id, got {volume_id:?}"
        );
        assert_eq!(
            outputs.get("VolumeIdAttribute").map(String::as_str),
            Some(volume_id.as_str()),
            "case {case:?}: expected Fn::GetAtt VolumeId to carry the id Ref answered \
             ({volume_id:?}), got {:?}",
            outputs.get("VolumeIdAttribute")
        );

        let (status, reported) = ec2(&router, "DescribeVolumes", &[("VolumeId.1", &volume_id)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Amazon EC2 to hold the volume the stack published \
             ({volume_id}), got HTTP {status} with body {reported}"
        );
        for (member, expected) in [
            ("availabilityZone", row.zone),
            ("size", row.size),
            ("volumeType", row.volume_type),
        ]
        .into_iter()
        .chain(row.declared.iter().copied())
        {
            let read = opt_element(&reported, member).unwrap_or_default();
            assert_eq!(
                read, expected,
                "case {case:?}: expected Amazon EC2 to report {member} {expected:?}, got \
                 {read:?} from {reported}"
            );
        }
        // The stack holds a resource's creation open until its owning service reports it
        // ready, so a volume a CREATE_COMPLETE stack published is one nothing is still
        // creating.
        let state = opt_element(&reported, "status").unwrap_or_default();
        assert_eq!(
            state, "available",
            "case {case:?}: expected the volume of a completed stack to be available, got \
             {state:?}"
        );
        let keys: BTreeSet<String> = elements(&reported, "key").into_iter().collect();
        for key in STACK_TAG_KEYS {
            assert!(
                keys.contains(key),
                "case {case:?}: expected the volume to carry the {key} tag, got {keys:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The whole estate
// ---------------------------------------------------------------------------

/// The template of the estate: the network, the security group, the IAM role with its
/// instance profile, and the EBS volume — the shape a client deploys an EC2 instance and its
/// storage with.
fn estate_template() -> Value {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Vpc": {
                "Type": "AWS::EC2::VPC",
                "Properties": {
                    "CidrBlock": VPC_CIDR,
                    "EnableDnsSupport": true,
                    "EnableDnsHostnames": true,
                },
            },
            "Igw": { "Type": "AWS::EC2::InternetGateway" },
            "IgwAttachment": {
                "Type": "AWS::EC2::VPCGatewayAttachment",
                "Properties": {
                    "VpcId": { "Ref": "Vpc" },
                    "InternetGatewayId": { "Ref": "Igw" },
                },
            },
            "PublicSubnet": {
                "Type": "AWS::EC2::Subnet",
                "Properties": {
                    "VpcId": { "Ref": "Vpc" },
                    "CidrBlock": SUBNET_CIDR,
                    "AvailabilityZone": ZONE,
                    "MapPublicIpOnLaunch": true,
                },
            },
            "PublicRouteTable": {
                "Type": "AWS::EC2::RouteTable",
                "Properties": { "VpcId": { "Ref": "Vpc" } },
            },
            "PublicDefaultRoute": {
                "Type": "AWS::EC2::Route",
                "DependsOn": "IgwAttachment",
                "Properties": {
                    "RouteTableId": { "Ref": "PublicRouteTable" },
                    "DestinationCidrBlock": ALL_IPV4,
                    "GatewayId": { "Ref": "Igw" },
                },
            },
            "PublicSubnetAssociation": {
                "Type": "AWS::EC2::SubnetRouteTableAssociation",
                "Properties": {
                    "SubnetId": { "Ref": "PublicSubnet" },
                    "RouteTableId": { "Ref": "PublicRouteTable" },
                },
            },
            "SecurityGroup": {
                "Type": "AWS::EC2::SecurityGroup",
                "Properties": {
                    "GroupDescription": "Security group for EC2 instance",
                    "VpcId": { "Ref": "Vpc" },
                    "SecurityGroupEgress": [
                        { "IpProtocol": ALL_PROTOCOLS, "CidrIp": ALL_IPV4 }
                    ],
                },
            },
            "PowerUserRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "ec2.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "describe-volumes",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": "ec2:DescribeVolumes",
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "PowerUserInstanceProfile": {
                "Type": "AWS::IAM::InstanceProfile",
                "Properties": { "Roles": [{ "Ref": "PowerUserRole" }] },
            },
            "EbsVolume": {
                "Type": "AWS::EC2::Volume",
                "Properties": {
                    "AvailabilityZone": ZONE,
                    "Size": 50,
                    "VolumeType": "gp3",
                },
            },
        },
        "Outputs": {
            "VpcId": { "Value": { "Ref": "Vpc" } },
            "SubnetId": { "Value": { "Ref": "PublicSubnet" } },
            "SecurityGroupId": { "Value": { "Fn::GetAtt": ["SecurityGroup", "GroupId"] } },
            "EBSVolumeId": { "Value": { "Ref": "EbsVolume" } },
            "IAMRoleName": { "Value": { "Ref": "PowerUserRole" } },
            "InstanceProfileName": { "Value": { "Ref": "PowerUserInstanceProfile" } },
        },
    })
}

/// The estate stack settles at `CREATE_COMPLETE`, each of its outputs resolves in the
/// service that owns the resource, and its deletion takes the resources away again.
#[tokio::test]
async fn the_ec2_and_iam_estate_stack_settles_at_create_complete() {
    let router = build_router();
    let case = "the EC2-and-IAM estate";
    let stack_name = "ec2-ebs-estate";

    let described = deploy(
        &router,
        case,
        stack_name,
        &estate_template(),
        &[("Capabilities.member.1", "CAPABILITY_IAM")],
    )
    .await;
    assert_create_complete(&router, case, stack_name, &described).await;

    let outputs = outputs_of(&described);
    for key in [
        "VpcId",
        "SubnetId",
        "SecurityGroupId",
        "EBSVolumeId",
        "IAMRoleName",
        "InstanceProfileName",
    ] {
        assert!(
            outputs.get(key).is_some_and(|value| !value.is_empty()),
            "case {case:?}: expected the stack to publish a non-empty {key}, got {outputs:?}"
        );
    }
    let vpc_id = outputs.get("VpcId").cloned().unwrap_or_default();
    let subnet_id = outputs.get("SubnetId").cloned().unwrap_or_default();
    let group_id = outputs.get("SecurityGroupId").cloned().unwrap_or_default();
    let volume_id = outputs.get("EBSVolumeId").cloned().unwrap_or_default();
    let role_name = outputs.get("IAMRoleName").cloned().unwrap_or_default();

    // Every identifier the stack published is one the owning service holds the resource
    // under: that is what a client does next with it.
    let (status, reported) = ec2(&router, "DescribeVpcs", &[("VpcId.1", &vpc_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Amazon EC2 to hold the VPC {vpc_id}, got HTTP {status} with \
         body {reported}"
    );
    let (status, reported) = ec2(&router, "DescribeSubnets", &[("SubnetId.1", &subnet_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Amazon EC2 to hold the subnet {subnet_id}, got HTTP {status} \
         with body {reported}"
    );
    let reported_vpc = opt_element(&reported, "vpcId").unwrap_or_default();
    assert_eq!(
        reported_vpc, vpc_id,
        "case {case:?}: expected the subnet to sit in the stack's VPC ({vpc_id}), got \
         {reported_vpc:?}"
    );
    let (status, reported) = ec2(
        &router,
        "DescribeSecurityGroups",
        &[("GroupId.1", &group_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Amazon EC2 to hold the security group {group_id}, got HTTP \
         {status} with body {reported}"
    );
    let egress = permissions_of(&reported, "ipPermissionsEgress");
    let expected_egress = vec![(ALL_PROTOCOLS.to_string(), ALL_IPV4.to_string())];
    assert_eq!(
        egress, expected_egress,
        "case {case:?}: expected the group to carry the one outbound rule the template \
         declared, got {egress:?}"
    );
    let (status, reported) = ec2(&router, "DescribeVolumes", &[("VolumeId.1", &volume_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Amazon EC2 to hold the volume {volume_id}, got HTTP {status} \
         with body {reported}"
    );
    let (status, reported) = iam(&router, "GetRole", &[("RoleName", &role_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected IAM to hold the role {role_name}, got HTTP {status} with \
         body {reported}"
    );
    // The instance profile is what lets an instance assume that role, so it has to exist in
    // IAM holding it — the profile's own `Roles` is what a client reads to find out.
    let profile_name = outputs
        .get("InstanceProfileName")
        .cloned()
        .unwrap_or_default();
    let (status, reported) = iam(
        &router,
        "GetInstanceProfile",
        &[("InstanceProfileName", &profile_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected IAM to hold the instance profile {profile_name}, got HTTP \
         {status} with body {reported}"
    );
    let profile_roles = elements(&reported, "RoleName");
    assert_eq!(
        profile_roles,
        vec![role_name.clone()],
        "case {case:?}: expected the profile to hold the stack's role ({role_name}), got \
         {profile_roles:?} from {reported}"
    );

    // The stack's deletion removes what it created, in the services that own it: the group
    // and the volume are gone, which is the other half of provisioning them.
    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_PROVISIONING_SPANS).await;
    for (action, member, id, code) in [
        (
            "DescribeSecurityGroups",
            "GroupId.1",
            &group_id,
            "InvalidGroup.NotFound",
        ),
        (
            "DescribeVolumes",
            "VolumeId.1",
            &volume_id,
            "InvalidVolume.NotFound",
        ),
    ] {
        let (status, reported) = ec2(&router, action, &[(member, id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {action} on the deleted {id} to be refused, got HTTP \
             {status} with body {reported}"
        );
        let reported_code = opt_element(&reported, "Code").unwrap_or_default();
        assert_eq!(
            reported_code, code,
            "case {case:?}: expected {action} on the deleted {id} to be refused with {code}, \
             got {reported_code:?}"
        );
    }
    // The IAM half of the same teardown: the profile the stack created is gone from IAM,
    // which is what its own not-found refusal says.
    let (status, reported) = iam(
        &router,
        "GetInstanceProfile",
        &[("InstanceProfileName", &profile_name)],
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected GetInstanceProfile on the deleted {profile_name} to be \
         refused, got HTTP {status} with body {reported}"
    );
    let reported_code = opt_element(&reported, "Code").unwrap_or_default();
    assert_eq!(
        reported_code, "NoSuchEntity",
        "case {case:?}: expected the deleted profile to be refused with NoSuchEntity, got \
         {reported_code:?} from {reported}"
    );
}
