// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateRouteTable`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=CreateRouteTable`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! * `CreateRouteTable` STORES one route table for the VPC the request names, carrying the
//!   local route every table comes with for EACH CIDR block that VPC has associated — "every
//!   route table contains a local route for communication within the VPC", and "routes to
//!   IPv4 and IPv6 addresses within a VPC" are both such routes (Amazon VPC User Guide,
//!   "Route tables") — with no association of its own, and answers that table exactly as
//!   `DescribeRouteTables` reports it. The rows differ in the VPC, in that VPC's block value
//!   and size, in how many tags the request asks for and in whether it carries a client
//!   token.
//! * A `ClientToken` names the one table it minted: the SAME request replayed with it
//!   reports that table and stores nothing, any OTHER request carrying it is refused with
//!   `IdempotentParameterMismatch`, and a different token mints a second table.
//! * A create this service cannot honour is refused with the EC2 code for what is wrong and
//!   stores nothing — a `VpcId` the account holds no VPC for whatever the value's shape, a
//!   tag specification naming another resource type, a client token past the bound this
//!   crate holds one to — and a `DryRun` of a request that WOULD have succeeded answers
//!   `DryRunOperation` without creating the table.
//! * Every bound this create holds a request to is served AT the bound and refused ONE past
//!   it: the tag count, the tag key and value lengths, and the client token length.
//! * Caller text carrying the characters that delimit this protocol's XML comes back as it
//!   was written, from the success envelope and from the message of a refusal quoting it.
//!
//! The create side of the route table slice. [`super::vpc_internet_path`] drives the same
//! operation as one step of a VPC's path to the internet; here it is the subject, with the
//! blocks a VPC has associated, the client token and the refusals as the table rows.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, IDEMPOTENT_PARAMETER_MISMATCH,
    INVALID_VPC_ID_NOT_FOUND, LOCAL_ROUTE_GATEWAY_ID, MAX_CLIENT_TOKEN_CHARS,
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS, RESOURCE_TYPE_ROUTE_TABLE,
    ROUTE_ORIGIN_CREATE_ROUTE_TABLE, ROUTE_STATE_ACTIVE, ROUTE_TABLE_ID_PREFIX,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, filters, indexed};
use super::wire::{child, child_text, document, is_long_id, param, set_items, Param};

/// The Region every call here is signed for — the Region the default VPC a fresh account
/// already has is seeded for.
const REGION: &str = "us-east-1";

/// One `CreateRouteTable` call carrying `params`, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateRouteTable", params).await
}

/// One call of `action` that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = super::ec2_query::call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// A member of a response body, or a panic naming the case.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// A VPC with `cidr_block`, as its id.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    member(case, &body, "vpcId")
}

/// The account's default VPC, as its id and the IPv4 block it is associated with.
async fn default_vpc(router: &MultiServiceRouter) -> (String, String) {
    let case = "the default VPC";
    let body = ok(
        case,
        router,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"])]),
    )
    .await;
    (
        member(case, &body, "vpcId"),
        member(case, &body, "cidrBlock"),
    )
}

/// The form fields that ask `CreateRouteTable` for `tags` on the table it creates, as one
/// `TagSpecification` naming the route table resource type
/// (`CreateRouteTableRequest$TagSpecifications`, whose resource type this crate spells
/// [`RESOURCE_TYPE_ROUTE_TABLE`]). No tags asks for no specification at all.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_ROUTE_TABLE,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    params
}

/// The ONE route table a response carries: the element holding both a `routeTableId` and a
/// `routeSet`, which is how a create's `<routeTable>` and a describe's `<item>` are both
/// written. Read off the TREE, so a `routeTableId` nested inside an association is never
/// taken for a table.
fn reported_table<'a, 'i>(
    case: &str,
    parsed: &'a roxmltree::Document<'i>,
) -> roxmltree::Node<'a, 'i> {
    parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child(*node, "routeTableId").is_some()
                && child(*node, "routeSet").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no route table in {}", parsed.input_text()))
}

/// One reported route, as the four members these assertions read: the destination it
/// matches (whichever member carries it), its target, its origin and its state.
type ReportedRoute = (String, String, String, String);

/// The routes of the one route table a response carries, in the order it reported them.
fn reported_routes(case: &str, body: &str) -> Vec<ReportedRoute> {
    let parsed = document(case, body);
    set_items(reported_table(case, &parsed), "routeSet")
        .into_iter()
        .map(|route| {
            (
                child_text(route, "destinationCidrBlock")
                    .or_else(|| child_text(route, "destinationIpv6CidrBlock"))
                    .unwrap_or_default(),
                child_text(route, "gatewayId").unwrap_or_default(),
                child_text(route, "origin").unwrap_or_default(),
                child_text(route, "state").unwrap_or_default(),
            )
        })
        .collect()
}

/// The local routes a table of a VPC with these blocks comes with: one per IPv4 block, then
/// one per IPv6 block, each targeting `local` with the create's own origin, active.
fn local_routes(ipv4_blocks: &[&str], ipv6_blocks: &[&str]) -> Vec<ReportedRoute> {
    ipv4_blocks
        .iter()
        .chain(ipv6_blocks.iter())
        .map(|block| {
            (
                (*block).to_string(),
                LOCAL_ROUTE_GATEWAY_ID.to_string(),
                ROUTE_ORIGIN_CREATE_ROUTE_TABLE.to_string(),
                ROUTE_STATE_ACTIVE.to_string(),
            )
        })
        .collect()
}

/// The tags of the one route table a response carries, in the order it reported them.
fn reported_tags(case: &str, body: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    set_items(reported_table(case, &parsed), "tagSet")
        .into_iter()
        .map(|tag| {
            (
                child_text(tag, "key").unwrap_or_default(),
                child_text(tag, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// How many associations the one route table a response carries reports.
fn reported_associations(case: &str, body: &str) -> usize {
    let parsed = document(case, body);
    set_items(reported_table(case, &parsed), "associationSet").len()
}

/// How many propagating virtual private gateways the one table a response carries reports.
fn reported_propagating_vgws(case: &str, body: &str) -> usize {
    let parsed = document(case, body);
    set_items(reported_table(case, &parsed), "propagatingVgwSet").len()
}

/// The `DescribeRouteTables` body for the one table `route_table_id` names.
async fn describe_one(case: &str, router: &MultiServiceRouter, route_table_id: &str) -> String {
    ok(
        case,
        router,
        "DescribeRouteTables",
        &indexed("RouteTableId", &[route_table_id]),
    )
    .await
}

/// How many route tables the store holds that no VPC came with — the tables a create made.
fn created_tables_held(engine: &engine::InMemoryEngine) -> usize {
    let handle = engine.state::<Ec2State>();
    let store = handle.lock();
    store
        .route_tables
        .keys()
        .filter(|id| !store.route_table_is_main(id))
        .count()
}

// ---------------------------------------------------------------------------
// The create, and the round trip through the describe
// ---------------------------------------------------------------------------

/// One row of the create table: which VPC the table is made for, the tags to ask for, and a
/// client token or none.
struct CreateCase {
    name: &'static str,
    /// Which of the VPCs this test has the table is created for.
    vpc_index: usize,
    tags: Vec<(&'static str, &'static str)>,
    client_token: Option<&'static str>,
}

/// Each create stores exactly one route table for the VPC it named, carrying the local route
/// that VPC's block gets and the tags it asked for, with no association of its own and no
/// propagating gateway, and `DescribeRouteTables` reads every one of them back unchanged.
/// The rows differ in the VPC and its block's value and size, in how many tags they carry,
/// and in whether they carry a client token.
#[tokio::test]
async fn a_created_route_table_round_trips_through_the_describe() {
    let (router, engine) = super::ec2_query::emulator();
    let (default_id, default_cidr) = default_vpc(&router).await;
    let vpcs: Vec<(String, String)> = vec![
        (default_id, default_cidr),
        (
            create_vpc("a /16", &router, "10.1.0.0/16").await,
            "10.1.0.0/16".to_string(),
        ),
        (
            create_vpc("a /24 outside the private 10 block", &router, "192.168.20.0/24").await,
            "192.168.20.0/24".to_string(),
        ),
    ];

    let cases = vec![
        CreateCase {
            name: "no tags, the default VPC",
            vpc_index: 0,
            tags: Vec::new(),
            client_token: None,
        },
        CreateCase {
            name: "one tag, a /16",
            vpc_index: 1,
            tags: vec![("Name", "web")],
            client_token: None,
        },
        CreateCase {
            name: "three tags and a client token, a /24",
            vpc_index: 2,
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

    for case in &cases {
        let (vpc_id, vpc_cidr) = &vpcs[case.vpc_index];
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
        let id = member(case.name, &body, "routeTableId");
        assert!(
            is_long_id(ROUTE_TABLE_ID_PREFIX, &id),
            "case {name:?}: expected a {ROUTE_TABLE_ID_PREFIX}-<17 hex digits> id, got {id:?}",
            name = case.name
        );
        assert_eq!(
            element(&body, "clientToken").as_deref(),
            case.client_token,
            "case {name:?}: the client token is echoed only when the request carried one, got \
             {body}",
            name = case.name
        );

        // The round trip: the table the create answered is the table the describe reports.
        let read_back = describe_one(case.name, &router, &id).await;
        for (reported, source) in [(&body, "the create"), (&read_back, "the describe")] {
            assert_eq!(
                element(reported, "vpcId").as_deref(),
                Some(vpc_id.as_str()),
                "case {name:?}: {source} must name the VPC the table is for, got {reported}",
                name = case.name
            );
            assert_eq!(
                element(reported, "ownerId").as_deref(),
                Some(engine::emulator_account_id()),
                "case {name:?}: {source} must name the owning account, got {reported}",
                name = case.name
            );
            assert_eq!(
                reported_routes(case.name, reported),
                local_routes(&[vpc_cidr.as_str()], &[]),
                "case {name:?}: {source} must report the local route for {vpc_cidr}, got \
                 {reported}",
                name = case.name
            );
            let expected_tags: Vec<(String, String)> = case
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                reported_tags(case.name, reported),
                expected_tags,
                "case {name:?}: {source} must report the tags the create asked for, got \
                 {reported}",
                name = case.name
            );
            // A table a caller created is not the VPC's main one, so it carries no
            // association until one is made, and no gateway propagates into it.
            assert_eq!(
                reported_associations(case.name, reported),
                0,
                "case {name:?}: {source} must report no association on a fresh table, got \
                 {reported}",
                name = case.name
            );
            assert_eq!(
                reported_propagating_vgws(case.name, reported),
                0,
                "case {name:?}: {source} must report no propagating gateway, got {reported}",
                name = case.name
            );
        }
    }

    assert_eq!(
        created_tables_held(&engine),
        cases.len(),
        "each create stores exactly one route table, so {} must be held",
        cases.len()
    );

    // The table each VPC came with is still there beside the created ones: a create adds a
    // table to the VPC, it does not replace its main one.
    for (vpc_id, _) in &vpcs {
        let case = "the main table";
        let body = ok(
            case,
            &router,
            "DescribeRouteTables",
            &filters(&[
                ("vpc-id", &[vpc_id.as_str()]),
                ("association.main", &["true"]),
            ]),
        )
        .await;
        assert!(
            element(&body, "routeTableId").is_some(),
            "case {case:?}: expected {vpc_id} to still have its main table, got {body}"
        );
    }
}

/// The same rule over the blocks a VPC has ASSOCIATED rather than the one it was created
/// with: a table comes with one local route per block of each family the VPC holds, so a VPC
/// with a secondary IPv4 block and an Amazon-provided IPv6 block routes all three locally.
#[tokio::test]
async fn a_route_table_comes_with_a_local_route_per_block_its_vpc_has_associated() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "a secondary IPv4 block and an IPv6 block";
    let vpc_id = create_vpc(case, &router, "10.2.0.0/16").await;

    ok(
        case,
        &router,
        "AssociateVpcCidrBlock",
        &[param("VpcId", &vpc_id), param("CidrBlock", "10.3.0.0/16")],
    )
    .await;
    let associated = ok(
        case,
        &router,
        "AssociateVpcCidrBlock",
        &[
            param("VpcId", &vpc_id),
            param("AmazonProvidedIpv6CidrBlock", "true"),
        ],
    )
    .await;
    let ipv6_block = member(case, &associated, "ipv6CidrBlock");

    let body = ok(
        case,
        &router,
        "CreateRouteTable",
        &[param("VpcId", &vpc_id)],
    )
    .await;
    let route_table_id = member(case, &body, "routeTableId");
    let expected = local_routes(&["10.2.0.0/16", "10.3.0.0/16"], &[ipv6_block.as_str()]);
    assert_eq!(
        reported_routes(case, &body),
        expected,
        "case {case:?}: expected a local route per associated block, got {body}"
    );

    let read_back = describe_one(case, &router, &route_table_id).await;
    assert_eq!(
        reported_routes(case, &read_back),
        expected,
        "case {case:?}: the describe must report the local routes the create stored, got \
         {read_back}"
    );
}

// ---------------------------------------------------------------------------
// The client token
// ---------------------------------------------------------------------------

/// One client token names the one table it minted: a repeat of the same request reports that
/// table and creates nothing, and a different token mints a second table.
#[tokio::test]
async fn a_client_token_makes_the_create_idempotent() {
    let (router, engine) = super::ec2_query::emulator();
    let (vpc_id, _) = default_vpc(&router).await;
    let params = vec![param("VpcId", &vpc_id), param("ClientToken", "once-only")];

    let (status, first) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "the first create: expected HTTP 200, got {status} with body {first}"
    );
    let first_id = member("the first create", &first, "routeTableId");

    let (status, replay) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "the replay: expected HTTP 200, got {status} with body {replay}"
    );
    assert_eq!(
        element(&replay, "routeTableId").as_deref(),
        Some(first_id.as_str()),
        "the replay: expected the table the token minted, got {replay}"
    );
    assert_eq!(
        created_tables_held(&engine),
        1,
        "a replayed client token must not create a second route table"
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
        element(&other, "routeTableId").as_deref(),
        Some(first_id.as_str()),
        "a second token: expected a new table, got {other}"
    );
    assert_eq!(
        created_tables_held(&engine),
        2,
        "a second token must mint a second route table"
    );
}

/// One row of the idempotency-mismatch table: the case name, which VPC the replay names, and
/// the tags it asks for.
type ReplayCase = (&'static str, usize, Vec<(&'static str, &'static str)>);

/// A client token is idempotent only for the REQUEST it was first given with: replayed with
/// any different parameter it is refused with `IdempotentParameterMismatch` rather than
/// answered with a table the caller did not ask for. The rows differ along each parameter the
/// request decides — the VPC, and how many tags it carries, with which keys and which values.
#[tokio::test]
async fn a_client_token_replayed_with_a_different_request_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_ids = [
        default_vpc(&router).await.0,
        create_vpc("the second VPC", &router, "10.4.0.0/16").await,
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
    let first_id = member("the first create", &body, "routeTableId");

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
    // SAME request: it reports the table the token already minted.
    let mut params = vec![param("VpcId", &vpc_ids[0]), param("ClientToken", TOKEN)];
    params.extend(tag_params(&[("Owner", "TeamA"), ("Name", "web")]));
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "the same tags in another order: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "routeTableId").as_deref(),
        Some(first_id.as_str()),
        "the same tags in another order: expected the table the token minted, got {body}"
    );

    assert_eq!(
        created_tables_held(&engine),
        1,
        "a refused or replayed create must not store a second route table"
    );
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// One row of the refusal table: the case name, the form fields to send, the status and the
/// EC2 error code the refusal must carry, and a fragment its message must carry (empty for a
/// row whose wording this table does not pin).
type RefusalCase = (&'static str, Vec<Param>, u16, &'static str, String);

/// A create this service cannot honour is refused with the EC2 code for what is wrong, and
/// stores nothing. A `VpcId` the account holds no VPC for is ONE refusal whatever the value's
/// shape; a tag specification for another resource type and a client token past the bound
/// this crate holds one to are refused for the parameter that is wrong; and a `DryRun` of a
/// request that WOULD have succeeded answers `DryRunOperation` without creating a table.
#[tokio::test]
async fn a_create_that_cannot_be_honoured_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let (vpc_id, _) = default_vpc(&router).await;
    let long_token = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);

    let cases: Vec<RefusalCase> = vec![
        (
            "a VPC the account does not have",
            vec![param("VpcId", "vpc-0ffffffffffffffff")],
            400,
            INVALID_VPC_ID_NOT_FOUND,
            "The vpc ID 'vpc-0ffffffffffffffff' does not exist".to_string(),
        ),
        (
            "a short-form VPC id nothing holds",
            vec![param("VpcId", "vpc-1a2b3c4d")],
            400,
            INVALID_VPC_ID_NOT_FOUND,
            "The vpc ID 'vpc-1a2b3c4d' does not exist".to_string(),
        ),
        (
            "a VPC id that is not an id at all",
            vec![param("VpcId", "not-a-vpc")],
            400,
            INVALID_VPC_ID_NOT_FOUND,
            "The vpc ID 'not-a-vpc' does not exist".to_string(),
        ),
        (
            "a tag specification for another resource type",
            vec![
                param("VpcId", &vpc_id),
                param("TagSpecification.1.ResourceType", "subnet"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            400,
            "InvalidParameterValue",
            String::new(),
        ),
        (
            "a client token past the bound",
            vec![param("VpcId", &vpc_id), param("ClientToken", &long_token)],
            400,
            "InvalidParameterValue",
            format!("clientToken accepts at most {MAX_CLIENT_TOKEN_CHARS} characters"),
        ),
        (
            "a dry run of a request that would have succeeded",
            vec![param("VpcId", &vpc_id), param("DryRun", "true")],
            412,
            DRY_RUN_OPERATION,
            "Request would have succeeded, but DryRun flag is set.".to_string(),
        ),
    ];

    for (case, params, expected_status, expected_code, expected_message) in &cases {
        let (status, body) = create(&router, params).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        if !expected_message.is_empty() {
            assert_eq!(
                error_message(&body),
                *expected_message,
                "case {case:?}: expected the message {expected_message:?}, got {}",
                error_message(&body)
            );
        }
    }

    assert_eq!(
        created_tables_held(&engine),
        0,
        "a refused create must store no route table at all"
    );
}

// ---------------------------------------------------------------------------
// The published bounds, at the bound and one past it
// ---------------------------------------------------------------------------

/// The form fields asking for `tags` as ONE route-table tag specification, for keys and
/// values this test builds at runtime (the sibling [`tag_params`] takes the literal pairs a
/// row writes out).
fn owned_tag_params(tags: &[(String, String)]) -> Vec<Param> {
    let borrowed: Vec<(&str, &str)> = tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    tag_params(&borrowed)
}

/// `count` tags, keyed so the order a request names them in is also their sorted order.
fn many_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("Key{index:03}"), format!("value-{index}")))
        .collect()
}

/// One row of the at-the-bound table: the tag payload to ask for and the client token to
/// carry, each row sitting on ONE of the bounds this create holds a caller to.
struct BoundCase {
    case: String,
    tags: Vec<(String, String)>,
    client_token: Option<String>,
}

/// One row of the one-past-the-bound table: the extra form fields that overshoot a bound, and
/// the EC2 code and message the refusal must carry.
struct PastBoundCase {
    case: String,
    extra: Vec<Param>,
    code: &'static str,
    message: String,
}

/// A request sitting exactly ON a bound this create holds a caller to is SERVED, and the
/// table it stores carries what the request asked for.
///
/// The bounds are this crate's shared ones, named here rather than written out: the number of
/// tags one resource holds ([`MAX_TAGS_PER_RESOURCE`]), the length of a tag key and of a tag
/// value ([`MAX_TAG_KEY_CHARS`] / [`MAX_TAG_VALUE_CHARS`], from `Tag$Key` / `Tag$Value`), and
/// the length this crate holds a `ClientToken` to ([`MAX_CLIENT_TOKEN_CHARS`]). The rows
/// differ along which bound they sit on, so each is exercised at its own published value.
#[tokio::test]
async fn a_create_is_served_at_each_published_bound() {
    let (router, engine) = super::ec2_query::emulator();
    let (vpc_id, vpc_cidr) = default_vpc(&router).await;

    let rows = vec![
        BoundCase {
            case: format!("{MAX_TAGS_PER_RESOURCE} tags, as many as one resource holds"),
            tags: many_tags(MAX_TAGS_PER_RESOURCE),
            client_token: None,
        },
        BoundCase {
            case: format!("a tag key of {MAX_TAG_KEY_CHARS} characters"),
            tags: vec![("k".repeat(MAX_TAG_KEY_CHARS), "at the key bound".to_string())],
            client_token: None,
        },
        BoundCase {
            case: format!("a tag value of {MAX_TAG_VALUE_CHARS} characters"),
            tags: vec![(
                "AtValueBound".to_string(),
                "v".repeat(MAX_TAG_VALUE_CHARS),
            )],
            client_token: None,
        },
        BoundCase {
            case: format!("a client token of {MAX_CLIENT_TOKEN_CHARS} characters"),
            tags: Vec::new(),
            client_token: Some("t".repeat(MAX_CLIENT_TOKEN_CHARS)),
        },
    ];

    for row in &rows {
        let case = row.case.as_str();
        let mut params = vec![param("VpcId", &vpc_id)];
        params.extend(owned_tag_params(&row.tags));
        if let Some(token) = &row.client_token {
            params.push(param("ClientToken", token));
        }

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 at the bound, got {status} with body {body}"
        );
        let route_table_id = member(case, &body, "routeTableId");
        assert!(
            is_long_id(ROUTE_TABLE_ID_PREFIX, &route_table_id),
            "case {case:?}: expected a {ROUTE_TABLE_ID_PREFIX}-<17 hex digits> id, got \
             {route_table_id:?}"
        );
        assert_eq!(
            element(&body, "clientToken").as_deref(),
            row.client_token.as_deref(),
            "case {case:?}: the client token is echoed only when the request carried one, got \
             {body}"
        );

        let read_back = describe_one(case, &router, &route_table_id).await;
        for (reported, source) in [(&body, "the create"), (&read_back, "the describe")] {
            assert_eq!(
                reported_tags(case, reported),
                row.tags,
                "case {case:?}: {source} must report the tags the create asked for, got \
                 {reported}"
            );
            assert_eq!(
                reported_routes(case, reported),
                local_routes(&[vpc_cidr.as_str()], &[]),
                "case {case:?}: {source} must report the local route for {vpc_cidr}, got \
                 {reported}"
            );
        }

        // A token at the bound is still a token: replayed with the same request it reports
        // the table it minted rather than minting a second one.
        if row.client_token.is_some() {
            let (status, replay) = create(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the replay to be served, got {status} with body {replay}"
            );
            assert_eq!(
                element(&replay, "routeTableId").as_deref(),
                Some(route_table_id.as_str()),
                "case {case:?}: expected the replay to report {route_table_id}, got {replay}"
            );
        }
    }

    assert_eq!(
        created_tables_held(&engine),
        rows.len(),
        "each create at a bound stores exactly one route table, so {} must be held, got {}",
        rows.len(),
        created_tables_held(&engine)
    );
}

/// A request ONE past a bound is refused with the EC2 code for the parameter that is wrong,
/// and stores nothing — the same bounds as [`a_create_is_served_at_each_published_bound`],
/// each overshot by one.
#[tokio::test]
async fn a_create_one_past_a_published_bound_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let (vpc_id, _) = default_vpc(&router).await;

    let over_count = many_tags(MAX_TAGS_PER_RESOURCE + 1);
    let rows = vec![
        PastBoundCase {
            case: format!("{} tags", MAX_TAGS_PER_RESOURCE + 1),
            extra: owned_tag_params(&over_count),
            code: "TagLimitExceeded",
            message: format!(
                "The request carries {count} tags, above the maximum of \
                 {MAX_TAGS_PER_RESOURCE} per resource",
                count = MAX_TAGS_PER_RESOURCE + 1,
            ),
        },
        PastBoundCase {
            case: format!("a tag key of {} characters", MAX_TAG_KEY_CHARS + 1),
            extra: owned_tag_params(&[(
                "k".repeat(MAX_TAG_KEY_CHARS + 1),
                "past the key bound".to_string(),
            )]),
            code: "InvalidParameterValue",
            message: format!("A tag key accepts at most {MAX_TAG_KEY_CHARS} characters"),
        },
        PastBoundCase {
            case: format!("a tag value of {} characters", MAX_TAG_VALUE_CHARS + 1),
            extra: owned_tag_params(&[(
                "PastValueBound".to_string(),
                "v".repeat(MAX_TAG_VALUE_CHARS + 1),
            )]),
            code: "InvalidParameterValue",
            message: format!(
                "The value of tag key 'PastValueBound' accepts at most \
                 {MAX_TAG_VALUE_CHARS} characters"
            ),
        },
        PastBoundCase {
            case: format!("a client token of {} characters", MAX_CLIENT_TOKEN_CHARS + 1),
            extra: vec![param(
                "ClientToken",
                &"t".repeat(MAX_CLIENT_TOKEN_CHARS + 1),
            )],
            code: "InvalidParameterValue",
            message: format!("clientToken accepts at most {MAX_CLIENT_TOKEN_CHARS} characters"),
        },
    ];

    for row in &rows {
        let case = row.case.as_str();
        let mut params = vec![param("VpcId", &vpc_id)];
        params.extend(row.extra.iter().cloned());
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 one past the bound, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected {code}, got {body}",
            code = row.code
        );
        assert_eq!(
            error_message(&body),
            row.message,
            "case {case:?}: expected the message {message:?}, got {actual:?}",
            message = row.message,
            actual = error_message(&body)
        );
    }

    assert_eq!(
        created_tables_held(&engine),
        0,
        "a create refused past a bound must store no route table at all, got {}",
        created_tables_held(&engine)
    );
}

// ---------------------------------------------------------------------------
// Caller text in the XML envelope
// ---------------------------------------------------------------------------

/// A tag whose key or value carries the characters that delimit the XML this protocol answers
/// in comes back as the caller wrote it, both from the SUCCESS envelope (the create and the
/// describe that reads the table back) and from the MESSAGE of a refusal that quotes the
/// caller's own tag key: the envelope escapes the text it carries rather than letting it end
/// an element.
#[tokio::test]
async fn a_tag_carrying_xml_delimiters_round_trips_and_is_quoted_back_in_a_refusal() {
    let (router, _engine) = super::ec2_query::emulator();
    let (vpc_id, _) = default_vpc(&router).await;
    let key = "Note<&>\"'".to_string();
    let value = "a < b & c > d \" e ' f".to_string();
    let tags = vec![(key.clone(), value)];

    let case = "a tag key and value carrying XML delimiters";
    let mut params = vec![param("VpcId", &vpc_id)];
    params.extend(owned_tag_params(&tags));
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let route_table_id = member(case, &body, "routeTableId");

    let read_back = describe_one(case, &router, &route_table_id).await;
    for (reported, source) in [(&body, "the create"), (&read_back, "the describe")] {
        assert_eq!(
            reported_tags(case, reported),
            tags,
            "case {case:?}: {source} must report the tag exactly as it was asked for, got \
             {reported}"
        );
    }

    // The same key in a refusal that quotes it: one character past the value bound, so the
    // message is built from the caller's key and the bound rather than from a literal.
    let refused_case = "a quoted tag key carrying XML delimiters";
    let mut params = vec![param("VpcId", &vpc_id)];
    params.extend(owned_tag_params(&[(
        key.clone(),
        "v".repeat(MAX_TAG_VALUE_CHARS + 1),
    )]));
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 400,
        "case {refused_case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("The value of tag key '{key}' accepts at most {MAX_TAG_VALUE_CHARS} characters"),
        "case {refused_case:?}: the message must quote the caller's own key, got {actual:?}",
        actual = error_message(&body)
    );
}
