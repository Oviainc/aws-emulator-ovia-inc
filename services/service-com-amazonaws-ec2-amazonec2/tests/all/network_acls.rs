// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateNetworkAcl`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=CreateNetworkAcl`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope, so
//! every call here carries one.
//!
//! The create side of the network ACL slice; what `DescribeNetworkAcls` reports out of the
//! same store — the filters, the id list and the pages — is covered by
//! [`super::describe_network_acls`], and the describe is used here only to read a created ACL
//! back.
//!
//! The rules under test:
//!
//! * `CreateNetworkAcl` STORES ONE ACL in the VPC the request names, beside the default ACL
//!   that VPC already came with, answers it with the catch-all deny entry in each direction
//!   and no subnet association, and echoes the client token only when the request carried
//!   one. The rows differ in the VPC, the number of tags and the client token.
//! * A `VpcId` the account holds no VPC for is refused with one code and one message whatever
//!   the value's shape, and the create stores nothing.
//! * A `ClientToken` is idempotent for the REQUEST it was first given with, and refused with
//!   `IdempotentParameterMismatch` for any other.
//! * A created ACL belongs to its VPC: deleting the VPC takes it away.

use ec2::state::{
    ALL_IPV4_ADDRESSES, ALL_IP_PROTOCOLS, ComAmazonawsEc2Amazonec2State as Ec2State,
    DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER, IDEMPOTENT_PARAMETER_MISMATCH,
    INVALID_VPC_ID_NOT_FOUND, MAX_CLIENT_TOKEN_CHARS, NETWORK_ACL_ID_PREFIX, RULE_ACTION_DENY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};
use super::wire::{child_text, document, is_long_id, param, set_items};

/// The Region every call here is signed for. The network ACL store holds the account's ACLs
/// rather than one Region's, and the default VPC a fresh account already has is seeded for
/// the emulator's default Region, so calls are signed for that Region.
const REGION: &str = "us-east-1";

/// One `CreateNetworkAcl` call carrying `params`, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateNetworkAcl", params).await
}

/// One `DescribeNetworkAcls` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeNetworkAcls", params).await
}

/// The network ACL ids a body reports, in the order it reported them.
///
/// Read off the TREE rather than by scanning for `<networkAclId>`: an association inside an
/// ACL carries a `networkAclId` of its own, so only the direct child of a reported ACL is
/// that ACL's id.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "networkAclSet")
        .into_iter()
        .map(|item| child_text(item, "networkAclId").unwrap_or_default())
        .collect()
}

/// The XML of the one network ACL `network_acl_id` names.
async fn describe_one(router: &MultiServiceRouter, case: &str, network_acl_id: &str) -> String {
    let params = indexed("NetworkAclId", &[network_acl_id]);
    let (status, body) = describe(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// The form fields that ask `CreateNetworkAcl` for `tags` on the ACL it creates, as one
/// `TagSpecification` naming the network ACL resource type. No tags asks for no specification
/// at all.
fn tag_params(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param("TagSpecification.1.ResourceType", "network-acl")];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Key"),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    params
}

/// A VPC created over the wire, by id — so a create can name a VPC the account really has
/// without reaching into the store.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let params = [param("CidrBlock", cidr_block)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateVpc", &params).await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// The account's default VPC id, read back through `DescribeVpcs`.
async fn default_vpc_id(router: &MultiServiceRouter) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeVpcs: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("DescribeVpcs answered no vpcId: {body}"))
}

/// How many network ACLs the store holds that no VPC came with — the ACLs a create made.
fn created_acls_held(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .network_acls
        .values()
        .filter(|acl| acl.is_default != Some(true))
        .count()
}

/// One entry of a reported network ACL, as the five members the catch-all assertions read:
/// the rule number, the protocol, the action, the direction and the IPv4 range.
type ReportedEntry = (String, String, String, String, String);

/// The entries of the ONE network ACL a response carries, in the order it reported them.
///
/// Read through the XML tree rather than by substring, so an entry's members stay inside the
/// entry they belong to: the ACL is the element carrying both a `networkAclId` and an
/// `entrySet`, which is how both a create's `<networkAcl>` and a describe's `<item>` are
/// written. Every call here names ONE ACL, so the first such element is that ACL.
fn reported_entries(case: &str, body: &str) -> Vec<ReportedEntry> {
    let parsed = document(case, body);
    let acl = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && super::wire::child(*node, "networkAclId").is_some()
                && super::wire::child(*node, "entrySet").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no network ACL in {body}"));
    set_items(acl, "entrySet")
        .into_iter()
        .map(|entry| {
            (
                child_text(entry, "ruleNumber").unwrap_or_default(),
                child_text(entry, "protocol").unwrap_or_default(),
                child_text(entry, "ruleAction").unwrap_or_default(),
                child_text(entry, "egress").unwrap_or_default(),
                child_text(entry, "cidrBlock").unwrap_or_default(),
            )
        })
        .collect()
}

/// The entries AWS gives a network ACL a caller just created: the catch-all deny in each
/// direction, EGRESS first, as the wire renders them — the order an ACL's `entrySet` comes
/// back in (the egress rules, then the ingress rules, each by ascending rule number).
fn catch_all_entries() -> Vec<ReportedEntry> {
    ["true", "false"]
        .into_iter()
        .map(|egress| {
            (
                DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER.to_string(),
                ALL_IP_PROTOCOLS.to_string(),
                RULE_ACTION_DENY.to_string(),
                egress.to_string(),
                ALL_IPV4_ADDRESSES.to_string(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// CreateNetworkAcl, and the round trip through the describe
// ---------------------------------------------------------------------------

/// One row of the create table: the VPC to create in, the tags to ask for, and a client token
/// or none.
struct CreateCase {
    name: &'static str,
    /// Which VPC of the two this test makes the ACL in.
    vpc_index: usize,
    tags: Vec<(&'static str, &'static str)>,
    client_token: Option<&'static str>,
}

/// Each create stores exactly one ACL, in the VPC it named, with the tags it asked for and
/// the entries AWS gives a caller-created ACL, and `DescribeNetworkAcls` reads every one of
/// them back. The rows differ in the VPC, in how many tags they carry, and in whether they
/// carry a client token.
#[tokio::test]
async fn a_created_network_acl_round_trips_through_the_describe() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_ids = [
        default_vpc_id(&router).await,
        create_vpc(&router, "10.1.0.0/16").await,
    ];

    let cases = vec![
        CreateCase {
            name: "no tags, default VPC",
            vpc_index: 0,
            tags: Vec::new(),
            client_token: None,
        },
        CreateCase {
            name: "one tag",
            vpc_index: 0,
            tags: vec![("Name", "web")],
            client_token: None,
        },
        CreateCase {
            name: "three tags, second VPC, a client token",
            vpc_index: 1,
            tags: vec![("Name", "edge"), ("Owner", "TeamA"), ("Tier", "prod")],
            client_token: Some("token-three-tags"),
        },
        CreateCase {
            name: "a tag value with spaces and punctuation",
            vpc_index: 1,
            tags: vec![("Note", "zone A/B, primary")],
            client_token: Some("token-note"),
        },
    ];

    let mut created: Vec<String> = Vec::new();
    for case in &cases {
        let vpc_id = &vpc_ids[case.vpc_index];
        let mut params = vec![param("VpcId", vpc_id)];
        params.extend(tag_params(&case.tags));
        if let Some(token) = case.client_token {
            params.push(param("ClientToken", token));
        }

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with body {body}",
            name = case.name
        );
        let id = element(&body, "networkAclId")
            .unwrap_or_else(|| panic!("case {:?}: no networkAclId in {body}", case.name));
        assert!(
            is_long_id(NETWORK_ACL_ID_PREFIX, &id),
            "case {name:?}: expected a {NETWORK_ACL_ID_PREFIX}-<17 hex digits> id, got {id}",
            name = case.name
        );
        assert_eq!(
            element(&body, "default").as_deref(),
            Some("false"),
            "case {name:?}: an ACL a create mints is not the VPC's default, got {body}",
            name = case.name
        );
        assert_eq!(
            reported_entries(case.name, &body),
            catch_all_entries(),
            "case {name:?}: expected the catch-all deny in each direction, got {body}",
            name = case.name
        );
        assert_eq!(
            element(&body, "clientToken").as_deref(),
            case.client_token,
            "case {name:?}: the client token is echoed only when the request carried one, \
             got {body}",
            name = case.name
        );

        // The round trip: the ACL the create answered is the ACL the describe reports,
        // member for member.
        let read_back = describe_one(&router, case.name, &id).await;
        for (member, value) in [
            ("networkAclId", id.as_str()),
            ("vpcId", vpc_id.as_str()),
            ("ownerId", engine::emulator_account_id()),
            ("default", "false"),
        ] {
            assert!(
                read_back.contains(&format!("<{member}>{value}</{member}>")),
                "case {name:?}: expected <{member}>{value}</{member}>, got {read_back}",
                name = case.name
            );
        }
        assert_eq!(
            reported_entries(case.name, &read_back),
            catch_all_entries(),
            "case {name:?}: the describe reports the entries the create stored, got {read_back}",
            name = case.name
        );
        let reported_associations = elements(&read_back, "networkAclAssociationId");
        assert!(
            reported_associations.is_empty(),
            "case {name:?}: a created ACL is associated with no subnet, got \
             {reported_associations:?}",
            name = case.name
        );
        let keys = elements(&read_back, "key");
        let values = elements(&read_back, "value");
        let expected_keys: Vec<String> = case.tags.iter().map(|(key, _)| key.to_string()).collect();
        let expected_values: Vec<String> = case
            .tags
            .iter()
            .map(|(_, value)| value.to_string())
            .collect();
        assert_eq!(
            (keys, values),
            (expected_keys, expected_values),
            "case {name:?}: expected the tags the create asked for, got {read_back}",
            name = case.name
        );
        created.push(id);
    }

    // Every created ACL is reported, and the ACL each VPC came with is still there beside
    // them: a create adds an ACL to the VPC, it does not replace its default one.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "every ACL: expected HTTP 200, got {status} with body {body}"
    );
    let reported = reported_ids("every ACL", &body);
    for id in &created {
        assert!(
            reported.contains(id),
            "every ACL: expected the created {id} among {reported:?}"
        );
    }
    for vpc_id in &vpc_ids {
        let params = filters(&[("vpc-id", &[vpc_id.as_str()]), ("default", &["true"])]);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "the default ACL of {vpc_id}: expected HTTP 200, got {status} with body {body}"
        );
        let defaults = reported_ids("the default ACL", &body);
        assert_eq!(
            defaults.len(),
            1,
            "the default ACL of {vpc_id}: expected the one ACL the VPC came with, got \
             {defaults:?}"
        );
        assert!(
            !created.contains(&defaults[0]),
            "the default ACL of {vpc_id}: a created ACL must not be the VPC's default, got \
             {defaults:?}"
        );
    }
    assert_eq!(
        created_acls_held(&engine),
        cases.len(),
        "each create stores exactly one network ACL"
    );
}

/// One client token names the one ACL it minted: a repeat of the same request reports that
/// ACL and creates nothing, and a different token mints a second ACL.
#[tokio::test]
async fn a_client_token_makes_the_create_idempotent() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = default_vpc_id(&router).await;
    let params = vec![param("VpcId", &vpc_id), param("ClientToken", "once-only")];

    let (status, first) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "first create: expected HTTP 200, got {status} with body {first}"
    );
    let first_id = element(&first, "networkAclId").expect("a created ACL has an id");

    let (status, second) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "replay: expected HTTP 200, got {status} with body {second}"
    );
    assert_eq!(
        element(&second, "networkAclId").as_deref(),
        Some(first_id.as_str()),
        "replay: expected the ACL the token minted, got {second}"
    );
    assert_eq!(
        created_acls_held(&engine),
        1,
        "a replayed client token must not create a second network ACL"
    );

    let (status, other) = create(
        &router,
        &[param("VpcId", &vpc_id), param("ClientToken", "another")],
    )
    .await;
    assert_eq!(
        status, 200,
        "a second token: expected HTTP 200, got {status} with body {other}"
    );
    assert_ne!(
        element(&other, "networkAclId").as_deref(),
        Some(first_id.as_str()),
        "a second token: expected a new ACL, got {other}"
    );
}

/// One row of the idempotency-mismatch table: the case name, which VPC the replay names, and
/// the tags it asks for.
type ReplayCase = (&'static str, usize, Vec<(&'static str, &'static str)>);

/// A client token is idempotent only for the REQUEST it was first given with: replayed with
/// any different parameter it is refused with `IdempotentParameterMismatch` rather than
/// answered with an ACL the caller did not ask for. The rows differ along each parameter the
/// request decides — the VPC, and how many tags it carries, with which keys and which values.
#[tokio::test]
async fn a_client_token_replayed_with_a_different_request_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_ids = [
        default_vpc_id(&router).await,
        create_vpc(&router, "10.4.0.0/16").await,
    ];
    const TOKEN: &str = "shared-token";
    let first_tags = [("Name", "web"), ("Owner", "TeamA")];

    let mut params = vec![param("VpcId", &vpc_ids[0]), param("ClientToken", TOKEN)];
    params.extend(tag_params(&first_tags));
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "the first create: expected HTTP 200, got {status} with body {body}"
    );
    let first_id = element(&body, "networkAclId").expect("a created ACL has an id");

    let cases: Vec<ReplayCase> = vec![
        ("a different VPC", 1, first_tags.to_vec()),
        (
            "one tag more",
            0,
            vec![("Name", "web"), ("Owner", "TeamA"), ("Tier", "prod")],
        ),
        ("one tag fewer", 0, vec![("Name", "web")]),
        (
            "a different tag value",
            0,
            vec![("Name", "edge"), ("Owner", "TeamA")],
        ),
        (
            "a different tag key",
            0,
            vec![("Name", "web"), ("Team", "TeamA")],
        ),
        ("no tags at all", 0, Vec::new()),
    ];

    for (case, vpc_index, tags) in &cases {
        let mut params = vec![
            param("VpcId", &vpc_ids[*vpc_index]),
            param("ClientToken", TOKEN),
        ];
        params.extend(tag_params(tags));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected {IDEMPOTENT_PARAMETER_MISMATCH}, got {body}"
        );
    }

    // The tags a create asks for are a SET, so naming the same ones in another order is the
    // SAME request: it reports the ACL the token already minted.
    let mut params = vec![param("VpcId", &vpc_ids[0]), param("ClientToken", TOKEN)];
    params.extend(tag_params(&[("Owner", "TeamA"), ("Name", "web")]));
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "the same tags in another order: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "networkAclId").as_deref(),
        Some(first_id.as_str()),
        "the same tags in another order: expected the ACL the token minted, got {body}"
    );

    assert_eq!(
        created_acls_held(&engine),
        1,
        "a refused or replayed create must not store a second network ACL"
    );
}

/// One row of the refusal table: the case name, the form fields to send, the EC2 error code
/// the refusal must carry and the message it must carry (empty for a row whose wording this
/// table does not pin).
type RefusalCase = (
    &'static str,
    Vec<(String, String)>,
    &'static str,
    &'static str,
);

/// A create this emulator cannot honour is refused with the EC2 code for what is wrong, and
/// stores nothing. A `VpcId` the account holds no VPC for is ONE refusal whatever the value's
/// shape — a well-formed id of a VPC that does not exist, a short-form id, and text that is
/// no id at all.
#[tokio::test]
async fn a_create_that_cannot_be_honoured_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = default_vpc_id(&router).await;

    let long_token = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);
    let cases: Vec<RefusalCase> = vec![
        (
            "a VPC the account does not have",
            vec![param("VpcId", "vpc-0ffffffffffffffff")],
            INVALID_VPC_ID_NOT_FOUND,
            "The vpc ID 'vpc-0ffffffffffffffff' does not exist",
        ),
        (
            "a short-form VPC id nothing holds",
            vec![param("VpcId", "vpc-1a2b3c4d")],
            INVALID_VPC_ID_NOT_FOUND,
            "The vpc ID 'vpc-1a2b3c4d' does not exist",
        ),
        (
            "a VPC id that is not an id at all",
            vec![param("VpcId", "not-a-vpc")],
            INVALID_VPC_ID_NOT_FOUND,
            "The vpc ID 'not-a-vpc' does not exist",
        ),
        (
            "a tag specification for another resource type",
            vec![
                param("VpcId", &vpc_id),
                param("TagSpecification.1.ResourceType", "subnet"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            "InvalidParameterValue",
            "",
        ),
        (
            "a tag specification with no resource type",
            vec![
                param("VpcId", &vpc_id),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            "MissingParameter",
            "",
        ),
        (
            "a reserved tag key",
            vec![
                param("VpcId", &vpc_id),
                param("TagSpecification.1.ResourceType", "network-acl"),
                param("TagSpecification.1.Tag.1.Key", "aws:created-by"),
                param("TagSpecification.1.Tag.1.Value", "me"),
            ],
            "InvalidParameterValue",
            "",
        ),
        (
            "a client token past the maximum",
            vec![param("VpcId", &vpc_id), param("ClientToken", &long_token)],
            "InvalidParameterValue",
            "",
        ),
    ];

    for (case, params, expected_code, expected_message) in cases {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        if !expected_message.is_empty() {
            assert_eq!(
                error_message(&body),
                expected_message,
                "case {case:?}: expected the message EC2 words for {expected_code}, got {body}"
            );
        }
    }

    assert_eq!(
        created_acls_held(&engine),
        0,
        "a refused create must store no network ACL"
    );
}

/// A dry run of a create that would have succeeded reports that outcome and writes nothing:
/// "If you have the required permissions, the error response is DryRunOperation"
/// (`CreateNetworkAclRequest$DryRun`).
#[tokio::test]
async fn a_dry_run_create_performs_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = default_vpc_id(&router).await;

    let (status, body) = create(&router, &[param("VpcId", &vpc_id), param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "a dry run: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run: expected DryRunOperation, got {body}"
    );
    assert_eq!(
        created_acls_held(&engine),
        0,
        "a dry run must store no network ACL"
    );

    // A dry run of a request that would have been REFUSED is refused with that reason
    // instead, so the flag never reports a success the real call would not have had.
    let (status, body) = create(
        &router,
        &[
            param("VpcId", "vpc-0ffffffffffffffff"),
            param("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a dry run of a doomed create: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_VPC_ID_NOT_FOUND,
        "a dry run of a doomed create: expected {INVALID_VPC_ID_NOT_FOUND}, got {body}"
    );
}

/// A created ACL belongs to its VPC, so deleting the VPC takes it away — and leaves the ACLs
/// of every other VPC alone.
#[tokio::test]
async fn deleting_a_vpc_takes_the_network_acls_created_in_it_with_it() {
    let (router, engine) = super::ec2_query::emulator();
    let doomed_vpc_id = create_vpc(&router, "10.7.0.0/16").await;
    let kept_vpc_id = create_vpc(&router, "10.8.0.0/16").await;

    let mut doomed: Vec<String> = Vec::new();
    for _ in 0..2 {
        let (status, body) = create(&router, &[param("VpcId", &doomed_vpc_id)]).await;
        assert_eq!(
            status, 200,
            "a create in the doomed VPC: expected HTTP 200, got {status} with body {body}"
        );
        doomed.push(element(&body, "networkAclId").expect("a created ACL has an id"));
    }
    let (status, body) = create(&router, &[param("VpcId", &kept_vpc_id)]).await;
    assert_eq!(
        status, 200,
        "a create in the kept VPC: expected HTTP 200, got {status} with body {body}"
    );
    let kept = element(&body, "networkAclId").expect("a created ACL has an id");

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "DeleteVpc",
        &[param("VpcId", &doomed_vpc_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteVpc: expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "after the delete: expected HTTP 200, got {status} with body {body}"
    );
    let reported = reported_ids("after the delete", &body);
    for gone in &doomed {
        assert!(
            !reported.contains(gone),
            "after the delete: expected {gone} to be gone, got {reported:?}"
        );
    }
    assert!(
        reported.contains(&kept),
        "after the delete: expected the kept VPC's ACL {kept} among {reported:?}"
    );

    let held: Vec<String> = engine
        .state::<Ec2State>()
        .lock()
        .network_acls
        .values()
        .filter(|acl| acl.is_default != Some(true))
        .filter_map(|acl| acl.network_acl_id.clone())
        .collect();
    assert_eq!(
        held,
        vec![kept],
        "after the delete: expected the store to hold only the ACL created in the kept VPC, \
         got {held:?}"
    );
}
