// SPDX-License-Identifier: CC0-1.0

//! WHICH code Elastic Load Balancing refuses a request under, and in whose words.
//!
//! The error code is what a client branches on, so a refusal of the wrong CLASS sends it
//! down the wrong path: a client matching the validation class retries a request that can
//! never be accepted, and a client told to rename never learns that its security groups are
//! what the service would not take. Two classes, and what decides between them:
//!
//! * A rule the REQUEST'S OWN VALUES have to satisfy — the number of subnets an Application
//!   Load Balancer needs, an attribute key the service defines no attribute under, the IPv6
//!   ranges a placement asking for IPv6 addresses needs of its subnets — is answered with
//!   HTTP 400 `ValidationError`, in the words the service words that rule, whatever error
//!   the operation's own model happens to declare.
//! * A configuration the resources the request NAMES cannot hold — two subnets of one
//!   Availability Zone, a security group this account has none of or that governs traffic in
//!   another VPC — is the modelled `InvalidConfigurationRequest`, and those resource checks
//!   run BEFORE the name is judged for uniqueness.
//!
//! Each rule is tabled over a set of inputs rather than the one case that showed it: the
//! three resources that carry attributes against several shapes of unrecognised key, both
//! operations that place a load balancer against several placements, every `IpAddressType`
//! against every shape of subnet EC2 can report, and every shape of security group value
//! against what EC2 says about it.
//!
//! The two rules that are decided on what EC2 reports are tabled directly over the readings
//! in `crate::state` — the wire halves of them need EC2 on the same engine and live in
//! `core/router/tests/all/elbv2_dualstack_and_security_group_classes.rs` — and everything
//! else goes through the router exactly as a client's request would: awsQuery form body in,
//! XML out. The harness is `all/provisioning_loop.rs`'s, reused rather than written twice.

use crate::listener_rules::create_listener;
use crate::load_balancer_attributes::create_load_balancer;
use crate::provisioning_loop::{
    action, call, create_http_target_group, emulator, members, p, Reply,
};
use crate::target_group_attributes_and_modification::{create_target_group, unescaped};
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::{
    check_security_groups, check_subnet_ipv6_cidrs,
    network::{NetworkRefusal, SecurityGroupFacts, SecurityGroupLookup, SubnetFacts},
    Refusal, SecurityGroupRefusal, INVALID_SECURITY_GROUPS, IPV6_CIDR_REQUIRED,
    SECURITY_GROUP_NOT_FOUND, SUBNETS_IN_TWO_ZONES_REQUIRED,
};
use smithy_server_runtime::MultiServiceRouter;

/// The code Elastic Load Balancing answers a value it cannot accept with, for a rule its
/// model documents in prose and declares no error shape for.
const VALIDATION_ERROR: &str = "ValidationError";

/// The awsQuery code of `InvalidConfigurationRequestException`
/// (`smithy_query(query="operation",operation="CreateLoadBalancer")`).
const INVALID_CONFIGURATION: &str = "InvalidConfigurationRequest";

/// The awsQuery code of `InvalidSecurityGroupException`, which the same operation models.
const INVALID_SECURITY_GROUP: &str = "InvalidSecurityGroup";

/// The awsQuery code of `DuplicateLoadBalancerNameException`.
const DUPLICATE_NAME: &str = "DuplicateLoadBalancerName";

/// The refusal `reply` carries: its code and its message, with the serializer's escaping
/// undone, so one assertion names both.
fn refusal(reply: &Reply) -> (u16, String, String) {
    (
        reply.status,
        reply.error_code(),
        unescaped(&reply.text("Message").unwrap_or_default()),
    )
}

// ---------------------------------------------------------------------------
// An attribute key the service defines no attribute under.
// ---------------------------------------------------------------------------

/// Which resource's attributes a write names, with the operation that writes them, the
/// member naming the resource, and the noun the refusal states.
struct AttributeOwner {
    case: &'static str,
    operation: &'static str,
    arn_member: &'static str,
    noun: &'static str,
    /// Keys the resource defines no attribute under: a plain word, one dotted like a real
    /// key, one that IS a real key of another of the three resources, and one differing
    /// from a real key of this resource only in case — the key set is matched exactly.
    unrecognized: [&'static str; 4],
    /// A key this resource does carry, with a value inside its domain: the control that
    /// shows the same request shape is accepted.
    recognized: (&'static str, &'static str),
}

/// The three resources that carry attributes, each with the keys the table drives.
const ATTRIBUTE_OWNERS: [AttributeOwner; 3] = [
    AttributeOwner {
        case: "a load balancer",
        operation: "ModifyLoadBalancerAttributes",
        arn_member: "LoadBalancerArn",
        noun: "Load balancer",
        unrecognized: [
            "not_an_attribute",
            "routing.http.invented_mode",
            // A listener attribute, which a load balancer does not carry.
            "tcp.idle_timeout.seconds",
            "Deletion_Protection.Enabled",
        ],
        recognized: ("deletion_protection.enabled", "true"),
    },
    AttributeOwner {
        case: "a listener",
        operation: "ModifyListenerAttributes",
        arn_member: "ListenerArn",
        noun: "Listener",
        unrecognized: [
            "not_an_attribute",
            "routing.http.invented_mode",
            // A load balancer attribute, which a listener does not carry.
            "deletion_protection.enabled",
            "Routing.Http.Response.Server.Enabled",
        ],
        recognized: ("routing.http.response.server.enabled", "false"),
    },
    AttributeOwner {
        case: "a target group",
        operation: "ModifyTargetGroupAttributes",
        arn_member: "TargetGroupArn",
        noun: "Target group",
        unrecognized: [
            "not_an_attribute",
            "stickiness.invented_cookie.name",
            // A load balancer attribute, which a target group does not carry.
            "deletion_protection.enabled",
            "Deregistration_Delay.Timeout_Seconds",
        ],
        recognized: ("deregistration_delay.timeout_seconds", "60"),
    },
];

/// Write one attribute through `owner`'s operation.
async fn write_attribute(
    router: &MultiServiceRouter,
    owner: &AttributeOwner,
    arn: &str,
    key: &str,
    value: &str,
) -> Reply {
    let mut form = action(owner.operation);
    form.push(p(owner.arn_member, arn));
    form.push(p("Attributes.member.1.Key", key));
    form.push(p("Attributes.member.1.Value", value));
    call(router, &form).await
}

#[tokio::test]
async fn an_attribute_key_the_service_does_not_define_is_refused_as_a_validation_error() {
    let (router, _engine) = emulator();

    // One resource of each kind, created the way the sibling suites create them.
    let lb_arn = create_load_balancer(&router, "attr-class-lb", &[]).await;
    let tg_arn = create_http_target_group(&router, "attr-class-tg").await;
    let listener_arn = create_listener(&router, &lb_arn, &tg_arn, 80).await;
    // The target group the attribute rows write to is its own, so a write never depends on
    // the listener that forwards to the one above.
    let plain_tg_arn = create_target_group(
        &router,
        "attr-class-tg2",
        &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
    )
    .await;

    for owner in &ATTRIBUTE_OWNERS {
        let arn = match owner.arn_member {
            "LoadBalancerArn" => lb_arn.as_str(),
            "ListenerArn" => listener_arn.as_str(),
            _ => plain_tg_arn.as_str(),
        };
        for key in owner.unrecognized {
            let case = format!("{} setting {key:?}", owner.case);
            let reply = write_attribute(&router, owner, arn, key, "1").await;
            let expected = format!("{} attribute key '{key}' is not recognized", owner.noun);
            assert_eq!(
                refusal(&reply),
                (400, VALIDATION_ERROR.to_string(), expected.clone()),
                "case {case:?}: expected 400 {VALIDATION_ERROR} {expected:?}, got {}",
                reply.body
            );
        }
        // The control: a key the resource carries is stored, so the rows above are refused
        // for the key and not for the request shape.
        let (key, value) = owner.recognized;
        let case = format!("{} setting {key:?}", owner.case);
        let reply = write_attribute(&router, owner, arn, key, value).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the attribute to be stored, got {} body {}",
            reply.status, reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// The subnets an Application Load Balancer must be placed in.
// ---------------------------------------------------------------------------

/// One placement row: the case, the load balancer type, the placement parameters, and the
/// code the request is refused under (empty for a placement that is served).
type PlacementRow = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
);

/// Every placement the table drives, against both the create and the replacement.
///
/// The rows differ along how many subnets the placement names, how it SPELLS them
/// (`Subnets` or `SubnetMappings`) and the family it is for: an Application Load Balancer's
/// subnets must come "from at least two Availability Zones", while a Network or Gateway
/// Load Balancer "can specify subnets from one or more".
const PLACEMENT_ROWS: &[PlacementRow] = &[
    (
        "one subnet on an application load balancer",
        "application",
        &[("Subnets.member.1", "subnet-0aaa0001")],
        VALIDATION_ERROR,
    ),
    (
        "one subnet mapping on an application load balancer",
        "application",
        &[("SubnetMappings.member.1.SubnetId", "subnet-0aaa0002")],
        VALIDATION_ERROR,
    ),
    (
        "two subnets on an application load balancer",
        "application",
        &[
            ("Subnets.member.1", "subnet-0aaa0003"),
            ("Subnets.member.2", "subnet-0aaa0004"),
        ],
        "",
    ),
    (
        "two subnet mappings on an application load balancer",
        "application",
        &[
            ("SubnetMappings.member.1.SubnetId", "subnet-0aaa0005"),
            ("SubnetMappings.member.2.SubnetId", "subnet-0aaa0006"),
        ],
        "",
    ),
    (
        "one subnet on a network load balancer",
        "network",
        &[("Subnets.member.1", "subnet-0aaa0007")],
        "",
    ),
    (
        "one subnet on a gateway load balancer",
        "gateway",
        &[("Subnets.member.1", "subnet-0aaa0008")],
        "",
    ),
];

#[tokio::test]
async fn an_application_load_balancer_placement_short_of_two_zones_is_a_validation_error() {
    let (router, _engine) = emulator();

    for (index, (case, lb_type, placement, code)) in PLACEMENT_ROWS.iter().enumerate() {
        // --- the create, which places a load balancer that does not exist yet ---
        let name = format!("placement-class-{index}");
        let mut form = action("CreateLoadBalancer");
        form.push(p("Name", &name));
        form.push(p("Type", lb_type));
        for (key, value) in placement.iter() {
            form.push(p(key, value));
        }
        let created = call(&router, &form).await;
        if code.is_empty() {
            assert_eq!(
                created.status, 200,
                "case {case:?} on CreateLoadBalancer: expected the placement to be \
                 accepted, got {} body {}",
                created.status, created.body
            );
        } else {
            assert_eq!(
                refusal(&created),
                (
                    400,
                    (*code).to_string(),
                    SUBNETS_IN_TWO_ZONES_REQUIRED.to_string()
                ),
                "case {case:?} on CreateLoadBalancer: expected 400 {code} \
                 {SUBNETS_IN_TWO_ZONES_REQUIRED:?}, got {}",
                created.body
            );
            // And the refused create stored nothing.
            let mut read = action("DescribeLoadBalancers");
            read.push(p("Names.member.1", &name));
            let described = call(&router, &read).await;
            assert_eq!(
                described.error_code(),
                "LoadBalancerNotFound",
                "case {case:?} on CreateLoadBalancer: a refused create must store \
                 nothing, got {}",
                described.body
            );
        }

        // --- the replacement, which places one that already exists ---
        let settled = format!("placement-class-settled-{index}");
        let arn = create_load_balancer(&router, &settled, &[("Type", lb_type)]).await;
        let mut replace = action("SetSubnets");
        replace.push(p("LoadBalancerArn", &arn));
        for (key, value) in placement.iter() {
            replace.push(p(key, value));
        }
        let replaced = call(&router, &replace).await;
        if code.is_empty() {
            assert_eq!(
                replaced.status, 200,
                "case {case:?} on SetSubnets: expected the placement to be accepted, got \
                 {} body {}",
                replaced.status, replaced.body
            );
        } else {
            assert_eq!(
                refusal(&replaced),
                (
                    400,
                    (*code).to_string(),
                    SUBNETS_IN_TWO_ZONES_REQUIRED.to_string()
                ),
                "case {case:?} on SetSubnets: expected 400 {code} \
                 {SUBNETS_IN_TWO_ZONES_REQUIRED:?}, got {}",
                replaced.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The IPv6 ranges an address type needs of the subnets.
// ---------------------------------------------------------------------------

/// One subnet as EC2 reports it, carrying the CIDR blocks `cidrs` names.
fn subnet(subnet_id: &str, cidrs: &[&str]) -> SubnetFacts {
    SubnetFacts {
        subnet_id: subnet_id.to_string(),
        vpc_id: Some("vpc-1".to_string()),
        availability_zone: Some("us-west-2a".to_string()),
        cidrs: cidrs
            .iter()
            .map(|cidr| cidr.parse().expect("a CIDR block"))
            .collect(),
    }
}

#[tokio::test]
async fn an_ipv6_address_type_needs_subnets_with_an_ipv6_range() {
    // (case, the IP address type, the subnets EC2 reported — `None` when there is no EC2
    // to ask, whether the placement is accepted)
    //
    // The rows differ along the address type (every value of `IpAddressType`), along what
    // EC2 reports for the subnets (IPv4 only, IPv6 beside IPv4, IPv6 alone, one of each,
    // and nothing at all) and along how many subnets the placement names, because every
    // node the load balancer places needs an IPv6 range where it sits.
    type Row<'a> = (&'a str, &'a str, Option<Vec<SubnetFacts>>, bool);
    let ipv4_only = || {
        vec![
            subnet("subnet-1", &["10.0.1.0/24"]),
            subnet("subnet-2", &["10.0.2.0/24"]),
        ]
    };
    let dual = || {
        vec![
            subnet("subnet-1", &["10.0.1.0/24", "2001:db8:1::/64"]),
            subnet("subnet-2", &["10.0.2.0/24", "2001:db8:2::/64"]),
        ]
    };
    let rows: Vec<Row<'_>> = vec![
        ("ipv4 on subnets with no IPv6 range", "ipv4", Some(ipv4_only()), true),
        ("ipv4 on subnets with an IPv6 range", "ipv4", Some(dual()), true),
        ("dualstack on subnets with an IPv6 range", "dualstack", Some(dual()), true),
        (
            "dualstack on subnets with no IPv6 range",
            "dualstack",
            Some(ipv4_only()),
            false,
        ),
        (
            "dualstack where only one subnet has an IPv6 range",
            "dualstack",
            Some(vec![
                subnet("subnet-1", &["10.0.1.0/24", "2001:db8:1::/64"]),
                subnet("subnet-2", &["10.0.2.0/24"]),
            ]),
            false,
        ),
        (
            "dualstack on a single subnet with an IPv6 range",
            "dualstack",
            Some(vec![subnet("subnet-1", &["10.0.1.0/24", "2001:db8:1::/64"])]),
            true,
        ),
        (
            "dualstack on a subnet that holds IPv6 alone",
            "dualstack",
            Some(vec![subnet("subnet-1", &["2001:db8:1::/64"])]),
            true,
        ),
        (
            "the application-only type on subnets with an IPv6 range",
            "dualstack-without-public-ipv4",
            Some(dual()),
            true,
        ),
        (
            "the application-only type on subnets with no IPv6 range",
            "dualstack-without-public-ipv4",
            Some(ipv4_only()),
            false,
        ),
        // Which ranges a subnet holds is EC2's to say: with no EC2 to ask nothing is known
        // about them, so nothing is refused on them.
        ("dualstack with no EC2 to ask", "dualstack", None, true),
        ("ipv4 with no EC2 to ask", "ipv4", None, true),
    ];

    for (case, ip_address_type, resolved, accepted) in rows {
        let judged = check_subnet_ipv6_cidrs(ip_address_type, resolved.as_deref());
        let expected = if accepted {
            Ok(())
        } else {
            Err(Refusal::Validation(IPV6_CIDR_REQUIRED.to_string()))
        };
        assert_eq!(
            judged, expected,
            "case {case:?}: expected {expected:?}, got {judged:?}"
        );
    }

    // And the scheme decides nothing: an internal load balancer takes an address type its
    // family carries, which the old emulator rule refused outright. With no EC2 mounted
    // here the subnets say nothing, so the create is served and the type reads back.
    let (router, _engine) = emulator();
    for (case, scheme, lb_type, ip_address_type) in [
        ("an internal application load balancer", "internal", "application", "dualstack"),
        (
            "an internet-facing application load balancer",
            "internet-facing",
            "application",
            "dualstack",
        ),
        ("an internal network load balancer", "internal", "network", "dualstack"),
    ] {
        let name = format!(
            "ds-{}-{lb_type}",
            if scheme == "internal" { "int" } else { "ext" }
        );
        let arn = create_load_balancer(
            &router,
            &name,
            &[
                ("Scheme", scheme),
                ("Type", lb_type),
                ("IpAddressType", ip_address_type),
            ],
        )
        .await;
        let mut read = action("DescribeLoadBalancers");
        read.push(p("LoadBalancerArns.member.1", &arn));
        let described = call(&router, &read).await;
        assert_eq!(
            described.text("IpAddressType").as_deref(),
            Some(ip_address_type),
            "case {case:?}: expected the stored IP address type, got {}",
            described.body
        );
    }
}

// ---------------------------------------------------------------------------
// The security groups a load balancer can be given.
// ---------------------------------------------------------------------------

/// One security group as EC2 reports it, governing traffic in `vpc_id`.
fn group(group_id: &str, vpc_id: &str) -> SecurityGroupFacts {
    SecurityGroupFacts {
        group_id: group_id.to_string(),
        group_name: Some("probe".to_string()),
        owner_id: None,
        vpc_id: Some(vpc_id.to_string()),
    }
}

#[tokio::test]
async fn which_refusal_a_security_group_earns_follows_its_form_the_account_and_the_vpc() {
    // (case, the VPC the load balancer is in, the ids the request named, what EC2 answered
    // about them, the refusal expected)
    //
    // The rows differ along the FORM of the value (both id lengths EC2 mints, and a value
    // that is no id at all in each way it can fail), along what EC2 says about it (reported,
    // absent, nothing known) and along the VPC the group governs traffic in.
    type Row<'a> = (
        &'a str,
        Option<&'a str>,
        Vec<String>,
        SecurityGroupLookup,
        Result<(), SecurityGroupRefusal>,
    );
    let ids = |ids: &[&str]| ids.iter().map(|id| (*id).to_string()).collect::<Vec<String>>();
    let rows: Vec<Row<'_>> = vec![
        (
            "a short-form group of the load balancer's own VPC",
            Some("vpc-1"),
            ids(&["sg-11111111"]),
            SecurityGroupLookup::Resolved(vec![group("sg-11111111", "vpc-1")]),
            Ok(()),
        ),
        (
            "a long-form group of the load balancer's own VPC",
            Some("vpc-1"),
            ids(&["sg-0123456789abcdef0"]),
            SecurityGroupLookup::Resolved(vec![group("sg-0123456789abcdef0", "vpc-1")]),
            Ok(()),
        ),
        (
            "a group of another VPC",
            Some("vpc-1"),
            ids(&["sg-22222222"]),
            SecurityGroupLookup::Resolved(vec![group("sg-22222222", "vpc-2")]),
            Err(SecurityGroupRefusal::Invalid),
        ),
        (
            "a group of each VPC",
            Some("vpc-1"),
            ids(&["sg-11111111", "sg-22222222"]),
            SecurityGroupLookup::Resolved(vec![
                group("sg-11111111", "vpc-1"),
                group("sg-22222222", "vpc-2"),
            ]),
            Err(SecurityGroupRefusal::Invalid),
        ),
        (
            "a group of another VPC, on a load balancer whose own VPC is not known",
            None,
            ids(&["sg-22222222"]),
            SecurityGroupLookup::Resolved(vec![group("sg-22222222", "vpc-2")]),
            Ok(()),
        ),
        (
            "an id the account holds no group for",
            Some("vpc-1"),
            ids(&["sg-33333333"]),
            SecurityGroupLookup::Absent(NetworkRefusal::new("InvalidGroup.NotFound")),
            Err(SecurityGroupRefusal::Invalid),
        ),
        (
            "an id the account holds no group for, with no VPC known either",
            None,
            ids(&["sg-0123456789abcdef1"]),
            SecurityGroupLookup::Absent(NetworkRefusal::new("InvalidGroup.NotFound")),
            Err(SecurityGroupRefusal::Invalid),
        ),
        (
            "a value with no group prefix",
            Some("vpc-1"),
            ids(&["not-a-group"]),
            SecurityGroupLookup::Unknown,
            Err(SecurityGroupRefusal::NotFound),
        ),
        (
            "a value whose suffix is too short",
            Some("vpc-1"),
            ids(&["sg-1234567"]),
            SecurityGroupLookup::Unknown,
            Err(SecurityGroupRefusal::NotFound),
        ),
        (
            "a value whose suffix is between the two id lengths",
            Some("vpc-1"),
            ids(&["sg-0123456789abcdef"]),
            SecurityGroupLookup::Unknown,
            Err(SecurityGroupRefusal::NotFound),
        ),
        (
            "a value whose suffix is not hexadecimal",
            Some("vpc-1"),
            ids(&["sg-0123456789abcdefz"]),
            SecurityGroupLookup::Unknown,
            Err(SecurityGroupRefusal::NotFound),
        ),
        (
            "one good id beside a value that is no id at all",
            Some("vpc-1"),
            ids(&["sg-11111111", "sg-"]),
            SecurityGroupLookup::Unknown,
            Err(SecurityGroupRefusal::NotFound),
        ),
        // Whether a group exists is EC2's to say: with no EC2 to ask nothing is known, so
        // a well-formed id is not refused on silence.
        (
            "a well-formed id with no EC2 to ask",
            Some("vpc-1"),
            ids(&["sg-44444444"]),
            SecurityGroupLookup::Unknown,
            Ok(()),
        ),
        (
            "no security group at all",
            Some("vpc-1"),
            ids(&[]),
            SecurityGroupLookup::Unknown,
            Ok(()),
        ),
    ];

    for (case, vpc_id, named, lookup, expected) in rows {
        let judged = check_security_groups(vpc_id, &named, &lookup);
        assert_eq!(
            judged, expected,
            "case {case:?}: expected {expected:?}, got {judged:?}"
        );
    }
}

#[tokio::test]
async fn create_load_balancer_judges_its_security_groups_before_the_name_it_reuses() {
    let (router, _engine) = emulator();

    // The name every row below re-uses, created with a group that is usable.
    let taken = "name-and-groups";
    let arn = create_load_balancer(
        &router,
        taken,
        &[("SecurityGroups.member.1", "sg-11111111")],
    )
    .await;

    // (case, the security groups the repeat create names, the code it earns, the message it
    // states) — the repeat also differs in its scheme, so the name alone would be refused
    // as a duplicate: WHICH refusal it earns is the behaviour under test.
    let rows: Vec<(&str, Vec<&str>, &str, String)> = vec![
        (
            "a value that is no security group id at all",
            vec!["sg-not-an-id"],
            INVALID_SECURITY_GROUP,
            SECURITY_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a good group beside a value that is no id at all",
            vec!["sg-11111111", "invented"],
            INVALID_SECURITY_GROUP,
            SECURITY_GROUP_NOT_FOUND.to_string(),
        ),
        // The control: with groups nothing can be said against, the name is what is left
        // to refuse.
        (
            "groups this emulator cannot fault",
            vec!["sg-11111111"],
            DUPLICATE_NAME,
            format!("A load balancer with the same name '{taken}' exists, but with different settings"),
        ),
    ];

    for (case, groups, code, message) in rows {
        let mut form = action("CreateLoadBalancer");
        form.push(p("Name", taken));
        form.push(p("Scheme", "internal"));
        form.push(p("Subnets.member.1", "subnet-0bbb0001"));
        form.push(p("Subnets.member.2", "subnet-0bbb0002"));
        for (index, id) in groups.iter().enumerate() {
            form.push(p(&format!("SecurityGroups.member.{}", index + 1), id));
        }
        let reply = call(&router, &form).await;
        assert_eq!(
            refusal(&reply),
            (400, code.to_string(), message.clone()),
            "case {case:?}: expected 400 {code} {message:?}, got {}",
            reply.body
        );
    }

    // None of the refusals touched the load balancer the name belongs to.
    let mut read = action("DescribeLoadBalancers");
    read.push(p("LoadBalancerArns.member.1", &arn));
    let described = call(&router, &read).await;
    assert_eq!(
        (
            described.status,
            described.text("Scheme"),
            members(&described.body, "SecurityGroups")
        ),
        (
            200,
            Some("internet-facing".to_string()),
            vec!["sg-11111111".to_string()]
        ),
        "case after the refusals: expected the stored load balancer unchanged, got {}",
        described.body
    );
}

/// `SetSecurityGroups` reads the SAME rule: the groups that replace the ones a load
/// balancer carries are judged by form, existence and VPC, and a refused call replaces
/// nothing.
#[tokio::test]
async fn set_security_groups_refuses_the_values_the_create_refuses() {
    let (router, _engine) = emulator();
    let arn = create_load_balancer(
        &router,
        "set-groups-class",
        &[("SecurityGroups.member.1", "sg-11111111")],
    )
    .await;

    // (case, the groups the request names, the code it earns — empty for a replacement
    // that is served)
    let rows: Vec<(&str, Vec<&str>, &str)> = vec![
        ("a value that is no group id at all", vec!["sg-nope"], INVALID_SECURITY_GROUP),
        (
            "a good id beside one that is no id at all",
            vec!["sg-22222222", "sg-0123456789abcdef"],
            INVALID_SECURITY_GROUP,
        ),
        ("two well-formed ids", vec!["sg-22222222", "sg-0123456789abcdef0"], ""),
    ];

    for (case, groups, code) in rows {
        let mut form = action("SetSecurityGroups");
        form.push(p("LoadBalancerArn", &arn));
        for (index, id) in groups.iter().enumerate() {
            form.push(p(&format!("SecurityGroups.member.{}", index + 1), id));
        }
        let reply = call(&router, &form).await;
        if code.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the replacement to be accepted, got {} body {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                refusal(&reply),
                (
                    400,
                    code.to_string(),
                    SECURITY_GROUP_NOT_FOUND.to_string()
                ),
                "case {case:?}: expected 400 {code}, got {}",
                reply.body
            );
        }
    }
}

/// The two classes do not collapse into one: the placement ceiling — two subnets in one
/// Availability Zone — stays the modelled `InvalidConfigurationRequest`, which is what
/// keeps `ValidationError` meaning "fix the values you sent".
#[tokio::test]
async fn a_placement_no_load_balancer_can_hold_stays_the_modelled_refusal() {
    let (router, _engine) = emulator();

    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", "ceiling-class"));
    for member in 1..=27 {
        form.push(p(
            &format!("Subnets.member.{member}"),
            &format!("subnet-{member:017x}"),
        ));
    }
    let reply = call(&router, &form).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, INVALID_CONFIGURATION.to_string()),
        "case a placement past the zones a Region has: expected 400 \
         {INVALID_CONFIGURATION}, got {}",
        reply.body
    );
    assert_ne!(
        unescaped(&reply.text("Message").unwrap_or_default()),
        INVALID_SECURITY_GROUPS,
        "case a placement past the zones a Region has: the refusal must state the rule it \
         broke, got {}",
        reply.body
    );
}
