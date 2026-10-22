// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateSecurityGroup`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=CreateSecurityGroup`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the operation CREATES THE GROUP THE REQUEST DESCRIBES in
//! the VPC it names: the group it stores carries the requested name, description and tags,
//! lives in the named VPC (the account's default VPC when the request names none), is
//! minted with a service-owned id and ARN, starts with no inbound rules and one
//! allow-all-outbound rule, and reads back through `DescribeSecurityGroups`; a name the
//! same VPC already holds — case-insensitively — is refused, while the same name in
//! another VPC is not. Every table row is a distinct input along what that generalises
//! over — the name and description value, their length and character set, the tag count,
//! which VPC, the signed Region — so the one case a recording happens to contain is one
//! row among many rather than the specification.
//!
//! What `DescribeSecurityGroups` selects and pages over is its own suite's subject
//! ([`super::describe_security_groups`]); this suite reads it only to prove the round trip.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region most calls here are signed for.
const REGION: &str = "us-east-1";

/// The account every ARN this emulator mints names.
fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// One row of a refusal table: the case, the request's form fields, the EC2 error code the
/// service answers and the HTTP status it answers with.
type RefusalCase = (&'static str, Vec<Param>, &'static str, u16);

/// One row of the create table: the case, the group's name, its description, and the tags
/// the request assigns to it.
type CreateCase = (
    &'static str,
    String,
    String,
    Vec<(&'static str, &'static str)>,
);

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `CreateSecurityGroup` call carrying `params`, signed for `region`.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    params: &[Param],
) -> (u16, String) {
    super::ec2_query::call(router, region, "CreateSecurityGroup", params).await
}

/// One `CreateSecurityGroup` call carrying `params`, signed for [`REGION`].
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    create_in(router, REGION, params).await
}

/// `CreateSecurityGroup` requiring success, and the body it answered.
async fn create_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateSecurityGroup, got {status} \
         with body {body}"
    );
    body
}

/// The form fields naming a group's name and description.
fn named(group_name: &str, description: &str) -> Vec<Param> {
    vec![
        param("GroupName", group_name),
        param("GroupDescription", description),
    ]
}

/// The `GroupId` a create answered with, or a panic naming the case.
fn created_id(case: &str, body: &str) -> String {
    element(body, "groupId")
        .unwrap_or_else(|| panic!("case {case:?}: no <groupId> in {body}"))
}

/// One `DescribeSecurityGroups` call narrowed to `group_id`, requiring success.
async fn describe_by_id(case: &str, router: &MultiServiceRouter, group_id: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeSecurityGroups",
        &[param("GroupId.1", group_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSecurityGroups for {group_id}, \
         got {status} with body {body}"
    );
    body
}

/// One `DescribeSecurityGroups` call narrowed to `group_name`, as its status and body.
async fn describe_by_name(router: &MultiServiceRouter, group_name: &str) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "DescribeSecurityGroups",
        &[param("GroupName.1", group_name)],
    )
    .await
}

/// `CreateVpc` for `cidr`, answering the `VpcId` it minted.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr: &str) -> String {
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateVpc", &[param("CidrBlock", cidr)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpc, got {status} with body {body}"
    );
    element(&body, "vpcId")
        .unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// The `VpcId` of the account's default VPC, read off the `default` group that VPC comes
/// with — the one group a fresh account already holds.
async fn default_vpc_id(router: &MultiServiceRouter) -> String {
    let (status, body) = describe_by_name(
        router,
        ec2::state::DEFAULT_SECURITY_GROUP_NAME,
    )
    .await;
    assert_eq!(
        status, 200,
        "the default security group must be describable, got {status} with body {body}"
    );
    group_member("the default group", &body, "vpcId")
}

// ---------------------------------------------------------------------------
// Reading one security group back off the ec2Query wire
// ---------------------------------------------------------------------------

/// The `<item>` children of `node`, in document order.
fn items<'a, 'i>(node: roxmltree::Node<'a, 'i>) -> Vec<roxmltree::Node<'a, 'i>> {
    node.children()
        .filter(|inner| inner.is_element() && inner.tag_name().name() == "item")
        .collect()
}

/// The one security group a response carries: the element that has a `groupName` child.
/// A `CreateSecurityGroup` response carries no group row at all, so this is only used on a
/// `DescribeSecurityGroups` body narrowed to one group.
fn group_member(case: &str, body: &str, name: &str) -> String {
    let document = document(case, body);
    let group = document
        .descendants()
        .find(|node| node.is_element() && child(*node, "groupName").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no security group (no <groupName>) in {body}"));
    child_text(group, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the group in {body}"))
}

/// The tags of the first `tagSet` a response carries, in the order it reported them.
fn tag_set(case: &str, body: &str) -> Vec<(String, String)> {
    let document = document(case, body);
    let Some(set) = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "tagSet")
    else {
        return Vec::new();
    };
    items(set)
        .into_iter()
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// Every rule of the `set` (`ipPermissions` / `ipPermissionsEgress`) a response carries,
/// as `(ipProtocol, the IPv4 CIDR blocks it names)`.
fn rules(case: &str, body: &str, set: &str) -> Vec<(String, Vec<String>)> {
    let document = document(case, body);
    let Some(node) = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == set)
    else {
        panic!("case {case:?}: no <{set}> in {body}");
    };
    items(node)
        .into_iter()
        .map(|rule| {
            let protocol = child_text(rule, "ipProtocol").unwrap_or_default();
            let blocks = child(rule, "ipRanges")
                .map(|ranges| {
                    items(ranges)
                        .into_iter()
                        .map(|range| child_text(range, "cidrIp").unwrap_or_default())
                        .collect()
                })
                .unwrap_or_default();
            (protocol, blocks)
        })
        .collect()
}

/// The form fields assigning `tags` to the group a create mints.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        ec2::state::RESOURCE_TYPE_SECURITY_GROUP,
    )];
    for (position, (key, value)) in tags.iter().enumerate() {
        let index = position + 1;
        params.push(param(&format!("TagSpecification.1.Tag.{index}.Key"), key));
        params.push(param(
            &format!("TagSpecification.1.Tag.{index}.Value"),
            value,
        ));
    }
    params
}

// ---------------------------------------------------------------------------
// The group the request describes
// ---------------------------------------------------------------------------

/// The rule: `CreateSecurityGroup` stores the group the request describes — its name,
/// description and tags as given, in the VPC the request names — mints the id and the ARN
/// itself, gives the group no inbound rules and one rule allowing all outbound IPv4
/// traffic, and every member of it reads back through `DescribeSecurityGroups`.
///
/// The rows differ in the name and description value, their length (including the
/// published 255-character maximum), whether the name carries spaces, and how many tags
/// the request assigns.
#[tokio::test]
async fn create_security_group_stores_the_group_and_round_trips_through_describe() {
    let router = emulator();
    let vpc_id = default_vpc_id(&router).await;
    let longest = ec2::state::MAX_SECURITY_GROUP_TEXT_CHARS;
    // (case, group name, description, the tags the request assigns)
    let cases: Vec<CreateCase> = vec![
        (
            "a plain name",
            "web-tier".to_string(),
            "HTTP from the load balancer".to_string(),
            Vec::new(),
        ),
        (
            "a name with spaces",
            "edge group 7".to_string(),
            "the edge".to_string(),
            Vec::new(),
        ),
        (
            "one tag",
            "tagged-once".to_string(),
            "one tag".to_string(),
            vec![("Name", "alpha")],
        ),
        (
            "three tags",
            "tagged-thrice".to_string(),
            "three tags".to_string(),
            vec![("Name", "beta"), ("env", "prod"), ("tier", "web")],
        ),
        (
            "a name of the maximum length",
            "n".repeat(longest),
            "the longest name".to_string(),
            Vec::new(),
        ),
        (
            "a description of the maximum length",
            "longest-description".to_string(),
            "d".repeat(longest),
            Vec::new(),
        ),
        (
            "a one-character name and description",
            "a".to_string(),
            "b".to_string(),
            Vec::new(),
        ),
    ];

    for (case, group_name, description, tags) in cases {
        let mut params = named(&group_name, &description);
        params.extend(tag_params(&tags));
        let created = create_ok(case, &router, &params).await;

        let group_id = created_id(case, &created);
        assert!(
            is_long_id("sg", &group_id),
            "case {case:?}: expected a long sg- id, got {group_id:?}"
        );
        let arn = element(&created, "securityGroupArn").unwrap_or_default();
        let expected_arn = format!(
            "arn:aws:ec2:{REGION}:{account}:security-group/{group_id}",
            account = account_id(),
        );
        assert_eq!(
            arn, expected_arn,
            "case {case:?}: expected the ARN {expected_arn:?}, got {arn:?}"
        );
        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            tag_set(case, &created),
            expected_tags,
            "case {case:?}: expected CreateSecurityGroup to report {expected_tags:?}, \
             got {:?} in {created}",
            tag_set(case, &created)
        );

        let described = describe_by_id(case, &router, &group_id).await;
        for (member, expected) in [
            ("groupId", group_id.as_str()),
            ("groupName", group_name.as_str()),
            ("groupDescription", description.as_str()),
            ("vpcId", vpc_id.as_str()),
            ("ownerId", account_id()),
            ("securityGroupArn", expected_arn.as_str()),
        ] {
            let actual = group_member(case, &described, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected <{member}> {expected:?} after the round trip, \
                 got {actual:?}"
            );
        }
        assert_eq!(
            tag_set(case, &described),
            expected_tags,
            "case {case:?}: expected DescribeSecurityGroups to report {expected_tags:?}, \
             got {:?}",
            tag_set(case, &described)
        );
        // "When you create a security group, it has no inbound rules" and "an outbound
        // rule that allows all outbound traffic" (Amazon VPC User Guide, "Security group
        // rules").
        let inbound = rules(case, &described, "ipPermissions");
        assert!(
            inbound.is_empty(),
            "case {case:?}: expected no inbound rules on a new group, got {inbound:?}"
        );
        let outbound = rules(case, &described, "ipPermissionsEgress");
        let expected_outbound = vec![(
            ec2::state::ALL_IP_PROTOCOLS.to_string(),
            vec![ec2::state::ALL_IPV4_ADDRESSES.to_string()],
        )];
        assert_eq!(
            outbound, expected_outbound,
            "case {case:?}: expected the one allow-all outbound rule \
             {expected_outbound:?}, got {outbound:?}"
        );
    }
}

/// The rule: every character the model documents for a group's name and description is
/// accepted and stored as given, and a character outside that set is refused with
/// `InvalidParameterValue`.
///
/// The accepted set is the punctuation `CreateSecurityGroupRequest$GroupName` /
/// `$Description` publish — "a-z, A-Z, 0-9, spaces, and `._-:/()#,@[]+=&;{}!$*`", carried
/// in the crate as [`ec2::state::SECURITY_GROUP_PUNCTUATION`] — exercised one
/// character at a time, so every entry of the published list is a row and none of them is
/// the one the issue cited.
#[tokio::test]
async fn create_security_group_accepts_every_documented_character_and_refuses_the_rest() {
    let router = emulator();
    for (index, punctuation) in ec2::state::SECURITY_GROUP_PUNCTUATION
        .chars()
        .enumerate()
    {
        let case = format!("the character {punctuation:?}");
        let group_name = format!("ok{index}{punctuation}name");
        let description = format!("desc{punctuation}text");
        let created = create_ok(&case, &router, &named(&group_name, &description)).await;
        let group_id = created_id(&case, &created);
        let described = describe_by_id(&case, &router, &group_id).await;
        let stored_name = group_member(&case, &described, "groupName");
        assert_eq!(
            stored_name, group_name,
            "case {case:?}: expected the name {group_name:?} to round-trip, \
             got {stored_name:?}"
        );
        let stored_description = group_member(&case, &described, "groupDescription");
        assert_eq!(
            stored_description, description,
            "case {case:?}: expected the description {description:?} to round-trip, \
             got {stored_description:?}"
        );
    }

    // Characters the published list does NOT carry, in the name and in the description.
    for refused in ['~', '"', '\'', '%', '^', '<', '>', '?', '|', '`', '\\'] {
        for member in ["GroupName", "GroupDescription"] {
            let case = format!("{refused:?} in {member}");
            let value = format!("bad{refused}value");
            let params = match member {
                "GroupName" => named(&value, "a description"),
                _ => named("a-name", &value),
            };
            let (status, body) = create(&router, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code,
                ec2::state::INVALID_PARAMETER_VALUE,
                "case {case:?}: expected InvalidParameterValue, got {code:?} in {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Uniqueness within a VPC
// ---------------------------------------------------------------------------

/// The rule: a group name is unique WITHIN ITS VPC and compared case-insensitively ("you
/// can't have two security groups for the same VPC with the same name";
/// "names are case-insensitive and must be unique within the VPC"), so a clashing create
/// is refused with `InvalidGroup.Duplicate` while the same name in another VPC succeeds.
///
/// The rows differ in how the second name is spelled relative to the first — identical,
/// upper-cased, mixed-case — and one row is the `default` name every VPC already holds.
#[tokio::test]
async fn create_security_group_refuses_a_name_its_vpc_already_holds() {
    let router = emulator();
    let other_vpc = create_vpc("another VPC", &router, "10.40.0.0/16").await;

    // (case, the name created first, the name the clashing create asks for)
    let clashes = [
        ("the same spelling", "payments", "payments"),
        ("upper-cased", "billing", "BILLING"),
        ("mixed case", "search-tier", "Search-Tier"),
    ];
    for (case, first, second) in clashes {
        create_ok(case, &router, &named(first, "the first group")).await;
        let (status, body) = create(&router, &named(second, "the clashing group")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a clashing name, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_GROUP_DUPLICATE,
            "case {case:?}: expected InvalidGroup.Duplicate, got {code:?} in {body}"
        );

        // The very same name in ANOTHER VPC is a different group, so it is accepted.
        let mut params = named(second, "the same name, another VPC");
        params.push(param("VpcId", &other_vpc));
        let created = create_ok(case, &router, &params).await;
        let group_id = created_id(case, &created);
        let described = describe_by_id(case, &router, &group_id).await;
        let stored_vpc = group_member(case, &described, "vpcId");
        assert_eq!(
            stored_vpc, other_vpc,
            "case {case:?}: expected the group in VPC {other_vpc:?}, got {stored_vpc:?}"
        );
    }

    // The group every VPC comes with holds the `default` name already.
    let (status, body) = create(
        &router,
        &named(
            ec2::state::DEFAULT_SECURITY_GROUP_NAME,
            "a second default group",
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"the default group's name\": expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code,
        ec2::state::INVALID_GROUP_DUPLICATE,
        "case \"the default group's name\": expected InvalidGroup.Duplicate, \
         got {code:?} in {body}"
    );
}

// ---------------------------------------------------------------------------
// The VPC the group goes in
// ---------------------------------------------------------------------------

/// The rule: "The ID of the VPC. Required for a nondefault VPC"
/// (`CreateSecurityGroupRequest$VpcId`) — a request that names a VPC creates the group
/// there, and one that names none creates it in the account's default VPC.
///
/// The rows differ in which VPC the request names: none, the default VPC by id, and two
/// VPCs `CreateVpc` minted.
#[tokio::test]
async fn create_security_group_puts_the_group_in_the_vpc_the_request_names() {
    let router = emulator();
    let default_vpc = default_vpc_id(&router).await;
    let first = create_vpc("the first created VPC", &router, "10.41.0.0/16").await;
    let second = create_vpc("the second created VPC", &router, "10.42.0.0/16").await;

    // (case, the VpcId the request names, the VPC the group must end up in)
    let cases = [
        ("no VpcId at all", None, default_vpc.clone()),
        (
            "the default VPC by id",
            Some(default_vpc.clone()),
            default_vpc.clone(),
        ),
        ("a created VPC", Some(first.clone()), first.clone()),
        ("another created VPC", Some(second.clone()), second.clone()),
    ];
    for (index, (case, requested, expected)) in cases.into_iter().enumerate() {
        let mut params = named(&format!("placed-{index}"), "placed somewhere");
        if let Some(vpc_id) = &requested {
            params.push(param("VpcId", vpc_id));
        }
        let created = create_ok(case, &router, &params).await;
        let group_id = created_id(case, &created);
        let described = describe_by_id(case, &router, &group_id).await;
        let actual = group_member(case, &described, "vpcId");
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the group in VPC {expected:?}, got {actual:?}"
        );
    }
}

/// The rule: the ARN the service mints names the Region the request was signed for.
///
/// The rows are distinct Regions, so the Region in the ARN cannot be a constant.
#[tokio::test]
async fn create_security_group_mints_an_arn_naming_the_signed_region() {
    let router = emulator();
    for (index, region) in ["us-east-1", "us-west-2", "eu-central-1", "ap-south-1"]
        .into_iter()
        .enumerate()
    {
        let case = format!("signed for {region}");
        let params = named(&format!("regional-{index}"), "one Region");
        let (status, body) = create_in(&router, region, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let group_id = created_id(&case, &body);
        let expected = format!(
            "arn:aws:ec2:{region}:{account}:security-group/{group_id}",
            account = account_id(),
        );
        let actual = element(&body, "securityGroupArn").unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the ARN {expected:?}, got {actual:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The rule: a request the service cannot satisfy is refused with the EC2 error code and
/// status that names what is wrong, and nothing is stored.
///
/// The rows span the published constraints and their boundaries: the 255-character maximum
/// of each text member and one character past it, the `sg-` prefix a name may not take, a
/// `VpcId` the account holds no VPC for, a tag specification naming another resource type
/// or no type at all, and the tag rules the crate enforces for every create.
#[tokio::test]
async fn create_security_group_refuses_what_it_cannot_satisfy() {
    let router = emulator();
    let longest = ec2::state::MAX_SECURITY_GROUP_TEXT_CHARS;
    let max_tags = ec2::state::MAX_TAGS_PER_RESOURCE;
    let too_long = "x".repeat(longest + 1);
    let reserved_key = format!("{}owner", ec2::state::RESERVED_TAG_PREFIX);
    let mut too_many_tags = named("too-many-tags", "past the tag ceiling");
    too_many_tags.push(param(
        "TagSpecification.1.ResourceType",
        ec2::state::RESOURCE_TYPE_SECURITY_GROUP,
    ));
    for index in 1..=(max_tags + 1) {
        too_many_tags.push(param(
            &format!("TagSpecification.1.Tag.{index}.Key"),
            &format!("k{index}"),
        ));
        too_many_tags.push(param(&format!("TagSpecification.1.Tag.{index}.Value"), "v"));
    }

    let refused: Vec<RefusalCase> = vec![
        (
            "an empty group name",
            named("", "a description"),
            ec2::state::MISSING_PARAMETER,
            400,
        ),
        (
            "an empty description",
            named("empty-description", ""),
            ec2::state::MISSING_PARAMETER,
            400,
        ),
        (
            "a name spelled like a group id",
            named("sg-0123456789abcdef0", "a description"),
            ec2::state::INVALID_PARAMETER_VALUE,
            400,
        ),
        (
            "a name one character past the maximum",
            named(&too_long, "a description"),
            ec2::state::INVALID_PARAMETER_VALUE,
            400,
        ),
        (
            "a description one character past the maximum",
            named("long-description", &too_long),
            ec2::state::INVALID_PARAMETER_VALUE,
            400,
        ),
        (
            "a VpcId the account holds no VPC for",
            {
                let mut params = named("absent-vpc", "a description");
                params.push(param("VpcId", "vpc-0fffffffffffffff0"));
                params
            },
            ec2::state::INVALID_VPC_ID_NOT_FOUND,
            400,
        ),
        (
            "a tag specification for another resource type",
            {
                let mut params = named("wrong-tag-type", "a description");
                params.push(param(
                    "TagSpecification.1.ResourceType",
                    ec2::state::RESOURCE_TYPE_VPC,
                ));
                params.push(param("TagSpecification.1.Tag.1.Key", "Name"));
                params.push(param("TagSpecification.1.Tag.1.Value", "nope"));
                params
            },
            ec2::state::INVALID_PARAMETER_VALUE,
            400,
        ),
        (
            "a tag specification with no resource type",
            {
                let mut params = named("no-tag-type", "a description");
                params.push(param("TagSpecification.1.Tag.1.Key", "Name"));
                params.push(param("TagSpecification.1.Tag.1.Value", "nope"));
                params
            },
            ec2::state::MISSING_PARAMETER,
            400,
        ),
        (
            "a tag key carrying the reserved prefix",
            {
                let mut params = named("reserved-tag", "a description");
                params.extend(tag_params(&[(reserved_key.as_str(), "v")]));
                params
            },
            ec2::state::INVALID_PARAMETER_VALUE,
            400,
        ),
        (
            "the same tag key twice",
            {
                let mut params = named("repeated-tag", "a description");
                params.extend(tag_params(&[("Name", "first"), ("Name", "second")]));
                params
            },
            ec2::state::INVALID_PARAMETER_VALUE,
            400,
        ),
        (
            "one tag past the ceiling",
            too_many_tags,
            "TagLimitExceeded",
            400,
        ),
        (
            "a dry run that would have succeeded",
            {
                let mut params = named("dry-run-group", "a description");
                params.push(param("DryRun", "true"));
                params
            },
            ec2::state::DRY_RUN_OPERATION,
            412,
        ),
    ];

    for (case, params, expected_code, expected_status) in refused {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got {code:?} \
             in {body}"
        );
    }

    // A refused create stores NOTHING: every name above is still unknown to the describe.
    for name in [
        "empty-description",
        "long-description",
        "absent-vpc",
        "wrong-tag-type",
        "no-tag-type",
        "reserved-tag",
        "repeated-tag",
        "too-many-tags",
        "dry-run-group",
    ] {
        let (status, body) = describe_by_name(&router, name).await;
        assert_eq!(
            status, 400,
            "case {name:?}: a refused create must store nothing, but the group was \
             describable: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_GROUP_NOT_FOUND,
            "case {name:?}: expected InvalidGroup.NotFound for a group no create stored, \
             got {code:?} in {body}"
        );
    }
}

/// The rule: each group a create mints gets its OWN id, so two creates never report the
/// same group.
///
/// The rows differ only in count: the ids of a run of creates are all distinct and all
/// well-formed.
#[tokio::test]
async fn create_security_group_mints_a_distinct_id_per_group() {
    let router = emulator();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for index in 0..8 {
        let case = format!("group {index}");
        let created = create_ok(
            &case,
            &router,
            &named(&format!("distinct-{index}"), "one of many"),
        )
        .await;
        let group_id = created_id(&case, &created);
        assert!(
            is_long_id("sg", &group_id),
            "case {case:?}: expected a long sg- id, got {group_id:?}"
        );
        assert!(
            seen.insert(group_id.clone()),
            "case {case:?}: the id {group_id:?} was already minted for another group"
        );
    }
}
