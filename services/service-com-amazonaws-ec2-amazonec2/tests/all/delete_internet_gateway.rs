// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteInternetGateway` and for the `DetachInternetGateway` its own
//! doc requires first ("you must detach the internet gateway from the VPC before you can
//! delete it"), driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name
//! in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. A delete takes EXACTLY the one internet gateway its `InternetGatewayId` names out of
//!    the store and leaves every other gateway — its owner, its attachment set and its tags —
//!    as the readers reported it: afterwards the id names nothing
//!    (`InvalidInternetGatewayID.NotFound` from every read of it AND from a second delete of
//!    it), it is absent from the unnarrowed report, and each remaining gateway reads back
//!    unchanged.
//! 2. A delete the operation's own documented blocker forbids — "You must detach the internet
//!    gateway from the VPC before you can delete it" (`DeleteInternetGateway`) — is refused
//!    with `DependencyViolation`, and one whose id this service cannot resolve with
//!    `InvalidInternetGatewayID.NotFound`; the store is left exactly as it was, since a
//!    refused delete removes nothing.
//! 3. A `DryRun` of a delete that would have succeeded answers `DryRunOperation` and removes
//!    nothing either, since the flag asks whether the action WOULD succeed rather than for it
//!    (`DeleteInternetGatewayRequest$DryRun`).
//! 4. A detach takes the ONE attachment between the gateway and the VPC the request names
//!    away: the gateway reads back with a present and empty attachment set ("for an internet
//!    gateway, the state is `available` when attached to a VPC; otherwise, this value is not
//!    returned" — `InternetGatewayAttachment$State`), the VPC is free to take another gateway,
//!    every route of that VPC pointing at the gateway reads back `blackhole` ("the specified
//!    gateway isn't attached to the VPC" — `Route$State`) and `active` again if the same
//!    gateway is re-attached, and the delete rule 2 refused then succeeds.
//! 5. A detach of a pair that is not attached is refused with `Gateway.NotAttached`, one
//!    naming a resource this account does not hold with that resource's own not-found code,
//!    and one whose VPC still holds a running instance with a public address with
//!    `DependencyViolation` ("the VPC must not contain any running instances with Elastic IP
//!    addresses or public IPv4 addresses" — `DetachInternetGateway`); a `DryRun` of a detach
//!    that would have succeeded answers `DryRunOperation`. None of them changes an attachment.
//!
//! Every table row is a distinct input along what those rules generalise over — how many
//! gateways the account holds, which of them the request names, how many tags the named
//! gateway carries, whether it was ever attached to a VPC, how many route tables and routes of
//! the VPC point at the gateway, and which form an unresolvable id takes — so no single
//! recorded request is the specification.

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, InstanceStateName};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, indexed};
use super::wire::{child_text, document, param, set_items};

/// The Region every call here is signed for. An internet gateway carries no Region member, so
/// the Region only picks which account's ambient network the store holds.
const REGION: &str = "us-east-1";

/// One internet gateway as the report carries it, in the members a delete has to leave alone:
/// the gateway's identity, the account that owns it, the VPCs it attaches and its tags.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotGateway {
    internet_gateway_id: String,
    owner_id: String,
    /// Each attachment as `(state, vpcId)`, in the order the report listed them.
    attachments: Vec<(String, String)>,
    /// Each tag as `(key, value)`.
    tags: Vec<(String, String)>,
}

/// One `DeleteInternetGateway` call for `internet_gateway_id`, as its HTTP status and XML
/// body.
async fn delete_gateway(
    router: &MultiServiceRouter,
    internet_gateway_id: &str,
    dry_run: bool,
) -> (u16, String) {
    let mut params = vec![param("InternetGatewayId", internet_gateway_id)];
    if dry_run {
        params.push(param("DryRun", "true"));
    }
    super::ec2_query::call(router, REGION, "DeleteInternetGateway", &params).await
}

/// One `DescribeInternetGateways` call naming `internet_gateway_id`, as its status and body.
async fn describe_one(router: &MultiServiceRouter, internet_gateway_id: &str) -> (u16, String) {
    let params = indexed("InternetGatewayId", &[internet_gateway_id]);
    super::ec2_query::call(router, REGION, "DescribeInternetGateways", &params).await
}

/// Every internet gateway the account holds, read back through the unnarrowed report, in the
/// order it reported them.
async fn snapshot(router: &MultiServiceRouter, case: &str) -> Vec<SnapshotGateway> {
    let (status, body) =
        super::ec2_query::call(router, REGION, "DescribeInternetGateways", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reporting every internet gateway, got {status} with \
         body {body}"
    );
    let parsed = document(case, &body);
    set_items(parsed.root_element(), "internetGatewaySet")
        .into_iter()
        .map(|item| SnapshotGateway {
            internet_gateway_id: child_text(item, "internetGatewayId").unwrap_or_default(),
            owner_id: child_text(item, "ownerId").unwrap_or_default(),
            attachments: set_items(item, "attachmentSet")
                .into_iter()
                .map(|attachment| {
                    (
                        child_text(attachment, "state").unwrap_or_default(),
                        child_text(attachment, "vpcId").unwrap_or_default(),
                    )
                })
                .collect(),
            tags: set_items(item, "tagSet")
                .into_iter()
                .map(|tag| {
                    (
                        child_text(tag, "key").unwrap_or_default(),
                        child_text(tag, "value").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect()
}

/// A VPC created over the wire, by id.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateVpc", &[param("CidrBlock", cidr_block)])
            .await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// An internet gateway created over the wire carrying `tags`, by id.
async fn create_gateway(router: &MultiServiceRouter, tags: &[(&str, &str)]) -> String {
    let mut params = Vec::new();
    if !tags.is_empty() {
        params.push(param("TagSpecification.1.ResourceType", "internet-gateway"));
        for (position, (key, value)) in tags.iter().enumerate() {
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Key", position + 1),
                key,
            ));
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Value", position + 1),
                value,
            ));
        }
    }
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateInternetGateway", &params).await;
    assert_eq!(
        status, 200,
        "CreateInternetGateway carrying {tags:?}: expected HTTP 200, got {status} with body \
         {body}"
    );
    element(&body, "internetGatewayId")
        .unwrap_or_else(|| panic!("CreateInternetGateway answered no internetGatewayId: {body}"))
}

/// Attach `internet_gateway_id` to `vpc_id` over the wire.
async fn attach(router: &MultiServiceRouter, internet_gateway_id: &str, vpc_id: &str) {
    let params = [
        param("InternetGatewayId", internet_gateway_id),
        param("VpcId", vpc_id),
    ];
    let (status, body) =
        super::ec2_query::call(router, REGION, "AttachInternetGateway", &params).await;
    assert_eq!(
        status, 200,
        "AttachInternetGateway {internet_gateway_id} to {vpc_id}: expected HTTP 200, got \
         {status} with body {body}"
    );
}

/// One `DetachInternetGateway` call for the pair, as its HTTP status and XML body.
async fn detach_gateway(
    router: &MultiServiceRouter,
    internet_gateway_id: &str,
    vpc_id: &str,
    dry_run: bool,
) -> (u16, String) {
    let mut params = vec![
        param("InternetGatewayId", internet_gateway_id),
        param("VpcId", vpc_id),
    ];
    if dry_run {
        params.push(param("DryRun", "true"));
    }
    super::ec2_query::call(router, REGION, "DetachInternetGateway", &params).await
}

/// Detach `internet_gateway_id` from `vpc_id` over the wire, which must succeed.
async fn detach(
    router: &MultiServiceRouter,
    case: &str,
    internet_gateway_id: &str,
    vpc_id: &str,
) {
    let (status, body) = detach_gateway(router, internet_gateway_id, vpc_id, false).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 detaching {internet_gateway_id} from {vpc_id}, got \
         {status} with body {body}"
    );
}

/// One row of the delete table: a population of gateways, and which of them the request
/// names.
#[derive(Debug, Clone)]
struct DeleteCase {
    case: &'static str,
    /// The tags each gateway of the population is created with, one entry per gateway.
    population: &'static [&'static [(&'static str, &'static str)]],
    /// Which gateway of the population the delete names, by position.
    target: usize,
    /// Whether the named gateway was attached to a VPC and then detached before the delete —
    /// the state a caller who followed "you must detach the internet gateway from the VPC
    /// before you can delete it" reaches.
    attached_then_detached: bool,
}

/// RULE 1: a delete removes the one gateway its id names and nothing else — the id names
/// nothing afterwards, and every other gateway of the account reads back exactly as it did
/// before.
#[tokio::test]
async fn a_delete_removes_only_the_internet_gateway_its_id_names() {
    // The rows differ along what the rule generalises over: how many gateways the account
    // holds, which one of them is named, how many tags the named gateway carries, and whether
    // it had ever been attached to a VPC.
    let cases = [
        DeleteCase {
            case: "the only gateway the account holds, carrying no tags",
            population: &[&[]],
            target: 0,
            attached_then_detached: false,
        },
        DeleteCase {
            case: "the first of three gateways",
            population: &[&[("Name", "edge")], &[("Name", "spare")], &[]],
            target: 0,
            attached_then_detached: false,
        },
        DeleteCase {
            case: "the last of three gateways, carrying several tags",
            population: &[
                &[],
                &[("Name", "spare")],
                &[("Name", "edge"), ("tier", "public"), ("owner", "network")],
            ],
            target: 2,
            attached_then_detached: false,
        },
        DeleteCase {
            case: "the middle of three gateways",
            population: &[&[("Name", "a")], &[("Name", "b")], &[("Name", "c")]],
            target: 1,
            attached_then_detached: false,
        },
        DeleteCase {
            case: "a gateway that had been attached to a VPC and was detached",
            population: &[&[("Name", "was-attached")], &[]],
            target: 0,
            attached_then_detached: true,
        },
    ];

    for row in cases {
        let case = row.case;
        let (router, _engine) = super::ec2_query::emulator();

        let mut created: Vec<String> = Vec::new();
        for tags in row.population {
            created.push(create_gateway(&router, tags).await);
        }
        let target_id = created[row.target].clone();

        if row.attached_then_detached {
            let vpc_id = create_vpc(&router, "10.30.0.0/16").await;
            attach(&router, &target_id, &vpc_id).await;
            detach(&router, case, &target_id, &vpc_id).await;
        }

        let before = snapshot(&router, case).await;
        assert!(
            before
                .iter()
                .any(|gateway| gateway.internet_gateway_id == target_id),
            "case {case:?}: expected {target_id} among the reported gateways, got {before:?}"
        );

        let (status, body) = delete_gateway(&router, &target_id, false).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting {target_id}, got {status} with body \
             {body}"
        );

        // Exactly the named gateway left the store: the rest of the report is the same rows
        // it carried before.
        let after = snapshot(&router, case).await;
        let expected: Vec<SnapshotGateway> = before
            .iter()
            .filter(|gateway| gateway.internet_gateway_id != target_id)
            .cloned()
            .collect();
        assert_eq!(
            after, expected,
            "case {case:?}: expected every gateway but {target_id} to read back unchanged, got \
             {after:?}"
        );

        // And the id names nothing: every read of it answers the not-found refusal, worded
        // with the id the REQUEST sent.
        let (status, body) = describe_one(&router, &target_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 reading {target_id} back, got {status} with body \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInternetGatewayID.NotFound",
            "case {case:?}: expected InvalidInternetGatewayID.NotFound reading {target_id} \
             back, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The internetGateway ID '{target_id}' does not exist"),
            "case {case:?}: expected the not-found message to name {target_id}, got {body}"
        );

        // A second delete of the same id is refused the same way, so a caller can tell the
        // gateway is gone rather than reading a success for a removal that did nothing.
        let (status, body) = delete_gateway(&router, &target_id, false).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from a second delete of {target_id}, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInternetGatewayID.NotFound",
            "case {case:?}: expected InvalidInternetGatewayID.NotFound from a second delete of \
             {target_id}, got {body}"
        );
    }
}

/// Which id a refusal row sends.
#[derive(Debug, Clone, Copy)]
enum RefusedId {
    /// A literal value, for the ids no gateway of the account could have.
    Literal(&'static str),
    /// The population's gateway that is attached to a VPC.
    Attached,
    /// The population's gateway attached to nothing — deletable, so a dry run of it is a dry
    /// run of a delete that WOULD have succeeded.
    Deletable,
}

/// One row of the refusal table: the id the delete names, whether it is a dry run, and the
/// answer it must get.
#[derive(Debug, Clone)]
struct RefusalCase {
    case: &'static str,
    id: RefusedId,
    dry_run: bool,
    status: u16,
    code: &'static str,
}

/// RULES 2 and 3: every refused delete leaves the store exactly as it was, and is refused
/// with the code that names the reason.
#[tokio::test]
async fn a_refused_delete_leaves_every_internet_gateway_where_it_was() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.40.0.0/16").await;
    let attached_gateway = create_gateway(&router, &[("Name", "attached")]).await;
    attach(&router, &attached_gateway, &vpc_id).await;
    let deletable_gateway = create_gateway(&router, &[]).await;

    let before = snapshot(&router, "the population").await;

    // `InternetGatewayId` publishes no pattern, so there is no spelling that tells a
    // malformed id from an unknown one: every id this service never minted — whatever its
    // form — names no gateway of the account. The rows span those forms: the prefix with no
    // separator, an empty suffix, a non-hexadecimal suffix, a suffix one digit short of the
    // long form, an upper-case prefix, another resource's id, and a value carrying XML
    // metacharacters (the refusal names the value the request sent, so the response must
    // still be a well-formed document whose text decodes back to what was sent).
    let cases = [
        RefusalCase {
            case: "a well-formed long id of no gateway",
            id: RefusedId::Literal("igw-0123456789abcdef0"),
            dry_run: false,
            status: 400,
            code: "InvalidInternetGatewayID.NotFound",
        },
        RefusalCase {
            case: "a well-formed short id of no gateway",
            id: RefusedId::Literal("igw-12345678"),
            dry_run: false,
            status: 400,
            code: "InvalidInternetGatewayID.NotFound",
        },
        RefusalCase {
            case: "the prefix with no separator",
            id: RefusedId::Literal("igw"),
            dry_run: false,
            status: 400,
            code: "InvalidInternetGatewayID.NotFound",
        },
        RefusalCase {
            case: "the prefix with an empty suffix",
            id: RefusedId::Literal("igw-"),
            dry_run: false,
            status: 400,
            code: "InvalidInternetGatewayID.NotFound",
        },
        RefusalCase {
            case: "a suffix that is not hexadecimal",
            id: RefusedId::Literal("igw-0123456789abcdefg"),
            dry_run: false,
            status: 400,
            code: "InvalidInternetGatewayID.NotFound",
        },
        RefusalCase {
            case: "a suffix one digit short of the long id form",
            id: RefusedId::Literal("igw-0123456789abcdef"),
            dry_run: false,
            status: 400,
            code: "InvalidInternetGatewayID.NotFound",
        },
        RefusalCase {
            case: "an upper-case prefix",
            id: RefusedId::Literal("IGW-0123456789abcdef0"),
            dry_run: false,
            status: 400,
            code: "InvalidInternetGatewayID.NotFound",
        },
        RefusalCase {
            case: "another resource's id",
            id: RefusedId::Literal("vpc-0123456789abcdef0"),
            dry_run: false,
            status: 400,
            code: "InvalidInternetGatewayID.NotFound",
        },
        RefusalCase {
            case: "a value carrying XML metacharacters",
            id: RefusedId::Literal("igw-<&\"']]>"),
            dry_run: false,
            status: 400,
            code: "InvalidInternetGatewayID.NotFound",
        },
        RefusalCase {
            case: "an id of no gateway, as a dry run",
            id: RefusedId::Literal("igw-0123456789abcdef1"),
            dry_run: true,
            status: 400,
            code: "InvalidInternetGatewayID.NotFound",
        },
        RefusalCase {
            case: "a gateway still attached to a VPC",
            id: RefusedId::Attached,
            dry_run: false,
            status: 400,
            code: "DependencyViolation",
        },
        RefusalCase {
            case: "a gateway still attached to a VPC, as a dry run",
            id: RefusedId::Attached,
            dry_run: true,
            status: 400,
            code: "DependencyViolation",
        },
        RefusalCase {
            case: "a dry run of a delete that would have succeeded",
            id: RefusedId::Deletable,
            dry_run: true,
            status: 412,
            code: "DryRunOperation",
        },
    ];

    for row in cases {
        let case = row.case;
        let internet_gateway_id = match row.id {
            RefusedId::Literal(value) => value.to_string(),
            RefusedId::Attached => attached_gateway.clone(),
            RefusedId::Deletable => deletable_gateway.clone(),
        };
        let (status, body) = delete_gateway(&router, &internet_gateway_id, row.dry_run).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {} for {internet_gateway_id}, got {status} with body \
             {body}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected {} for {internet_gateway_id}, got {body}",
            row.code
        );

        // The reason is in the message, built from the id the REQUEST sent and from the
        // store's own reading of it.
        let message = error_message(&body);
        let expected_message = match row.code {
            "DependencyViolation" => format!(
                "The internetGateway '{internet_gateway_id}' has dependencies and cannot be \
                 deleted."
            ),
            "DryRunOperation" => "Request would have succeeded, but DryRun flag is set.".to_string(),
            _ => format!("The internetGateway ID '{internet_gateway_id}' does not exist"),
        };
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the refusal to be worded {expected_message:?}, got \
             {message:?} in {body}"
        );

        // A refused delete removes nothing: the whole report is the rows it carried before.
        let after = snapshot(&router, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected every gateway left where it was, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The detach a delete requires first
// ---------------------------------------------------------------------------

/// One route of a route table as the report carries it: where it matches, what it points at
/// and the state its target leaves it in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedRoute {
    destination: String,
    gateway_id: String,
    state: String,
}

/// A route table of `vpc_id` created over the wire, by id.
async fn create_route_table(router: &MultiServiceRouter, vpc_id: &str) -> String {
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateRouteTable", &[param("VpcId", vpc_id)]).await;
    assert_eq!(
        status, 200,
        "CreateRouteTable in {vpc_id}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "routeTableId")
        .unwrap_or_else(|| panic!("CreateRouteTable answered no routeTableId: {body}"))
}

/// One route in `route_table_id` for `destination`, pointing at `gateway_id`.
async fn create_route(
    router: &MultiServiceRouter,
    route_table_id: &str,
    destination: &str,
    gateway_id: &str,
) {
    let params = [
        param("RouteTableId", route_table_id),
        param("DestinationCidrBlock", destination),
        param("GatewayId", gateway_id),
    ];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateRoute", &params).await;
    assert_eq!(
        status, 200,
        "CreateRoute {destination} -> {gateway_id}: expected HTTP 200, got {status} with body \
         {body}"
    );
}

/// Every route the table `route_table_id` holds, as the report carries them.
async fn routes_of(
    router: &MultiServiceRouter,
    case: &str,
    route_table_id: &str,
) -> Vec<ReportedRoute> {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeRouteTables",
        &indexed("RouteTableId", &[route_table_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading {route_table_id}, got {status} with body \
         {body}"
    );
    let parsed = document(case, &body);
    set_items(parsed.root_element(), "routeTableSet")
        .into_iter()
        .flat_map(|table| set_items(table, "routeSet"))
        .map(|route| ReportedRoute {
            destination: child_text(route, "destinationCidrBlock").unwrap_or_default(),
            gateway_id: child_text(route, "gatewayId").unwrap_or_default(),
            state: child_text(route, "state").unwrap_or_default(),
        })
        .collect()
}

/// The attachments the report carries for the one gateway `internet_gateway_id` names, each as
/// `(state, vpcId)`.
async fn attachments_of(
    router: &MultiServiceRouter,
    case: &str,
    internet_gateway_id: &str,
) -> Vec<(String, String)> {
    let (status, body) = describe_one(router, internet_gateway_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading {internet_gateway_id}, got {status} with body \
         {body}"
    );
    let parsed = document(case, &body);
    set_items(parsed.root_element(), "internetGatewaySet")
        .into_iter()
        .flat_map(|item| set_items(item, "attachmentSet"))
        .map(|attachment| {
            (
                child_text(attachment, "state").unwrap_or_default(),
                child_text(attachment, "vpcId").unwrap_or_default(),
            )
        })
        .collect()
}

/// Assert that every route of `route_tables` whose destination one of `destinations` names
/// reads back in `state`, and that each table's own local route is left `active`.
///
/// The state a route's target leaves it in is the one thing rule 4 generalises over, so it is
/// asserted the same way whichever direction the attachment last moved.
async fn assert_routes_to_gateway(
    router: &MultiServiceRouter,
    case: &str,
    route_tables: &[String],
    destinations: &[&[&str]],
    state: &str,
) {
    for (index, route_table_id) in route_tables.iter().enumerate() {
        let reported = routes_of(router, case, route_table_id).await;
        for destination in destinations[index] {
            let route = reported
                .iter()
                .find(|route| route.destination == *destination)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a route for {destination} in {reported:?}")
                });
            assert_eq!(
                route.state, state,
                "case {case:?}: expected the route for {destination} in {state}, got {route:?}"
            );
        }
        let local = reported
            .iter()
            .find(|route| route.gateway_id == "local")
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the table's local route in {reported:?}")
            });
        assert_eq!(
            local.state, "active",
            "case {case:?}: expected the local route left active, got {local:?}"
        );
    }
}

/// One row of the detach table: the VPC's block, and the routes to the gateway each of its
/// route tables holds.
#[derive(Debug, Clone)]
struct DetachCase {
    case: &'static str,
    vpc_cidr: &'static str,
    /// One entry per route table of the VPC, each listing the destinations whose route points
    /// at the gateway.
    tables: &'static [&'static [&'static str]],
}

/// RULE 4: the detach takes the attachment away, frees the VPC, blackholes the routes that
/// pointed at the gateway, and leaves a gateway the delete then accepts.
#[tokio::test]
async fn a_detach_frees_the_vpc_and_blackholes_the_routes_that_pointed_at_the_gateway() {
    // The rows differ along what the rule generalises over: how many route tables of the VPC
    // point at the gateway and how many routes each of them holds — none, one, several.
    let cases = [
        DetachCase {
            case: "a VPC with no route table of its own",
            vpc_cidr: "10.50.0.0/16",
            tables: &[],
        },
        DetachCase {
            case: "one route table with one route to the gateway",
            vpc_cidr: "10.51.0.0/16",
            tables: &[&["0.0.0.0/0"]],
        },
        DetachCase {
            case: "two route tables, each routing to the gateway",
            vpc_cidr: "10.52.0.0/16",
            tables: &[&["0.0.0.0/0"], &["198.51.100.0/24", "203.0.113.0/24"]],
        },
    ];

    for row in cases {
        let case = row.case;
        let (router, _engine) = super::ec2_query::emulator();
        let vpc_id = create_vpc(&router, row.vpc_cidr).await;
        let internet_gateway_id = create_gateway(&router, &[("Name", "edge")]).await;
        attach(&router, &internet_gateway_id, &vpc_id).await;

        let mut route_tables: Vec<String> = Vec::new();
        for destinations in row.tables {
            let route_table_id = create_route_table(&router, &vpc_id).await;
            for destination in *destinations {
                create_route(&router, &route_table_id, destination, &internet_gateway_id).await;
            }
            route_tables.push(route_table_id);
        }

        // While the gateway is attached, the report carries the one attachment and every
        // route to it is reachable.
        assert_eq!(
            attachments_of(&router, case, &internet_gateway_id).await,
            vec![("available".to_string(), vpc_id.clone())],
            "case {case:?}: expected the one attachment to {vpc_id} before the detach"
        );
        assert_routes_to_gateway(&router, case, &route_tables, row.tables, "active").await;

        detach(&router, case, &internet_gateway_id, &vpc_id).await;

        // The attachment set is present and EMPTY: the gateway is attached to nothing, which
        // is not the same answer as a gateway that reports no set at all.
        assert_eq!(
            attachments_of(&router, case, &internet_gateway_id).await,
            Vec::new(),
            "case {case:?}: expected no attachment after detaching {internet_gateway_id}"
        );

        // Every route that pointed at the gateway is a blackhole now, and the local route
        // each table comes with is untouched — the detach makes the GATEWAY's target
        // unavailable, not the VPC's own.
        assert_routes_to_gateway(&router, case, &route_tables, row.tables, "blackhole").await;

        // The state FOLLOWS the attachment in both directions: re-attaching the SAME gateway
        // makes those routes reachable again, since `blackhole` means "the specified gateway
        // isn't attached to the VPC" (`Route$State`) and this one is attached once more.
        attach(&router, &internet_gateway_id, &vpc_id).await;
        assert_eq!(
            attachments_of(&router, case, &internet_gateway_id).await,
            vec![("available".to_string(), vpc_id.clone())],
            "case {case:?}: expected the attachment the re-attach recorded"
        );
        assert_routes_to_gateway(&router, case, &route_tables, row.tables, "active").await;

        // And a second detach takes both away again, so neither direction is a one-off.
        detach(&router, case, &internet_gateway_id, &vpc_id).await;
        assert_routes_to_gateway(&router, case, &route_tables, row.tables, "blackhole").await;

        // The VPC is free to take another gateway, which is what "you can attach only one
        // internet gateway to a VPC at a time" allows once the first one is off.
        let second_gateway = create_gateway(&router, &[]).await;
        attach(&router, &second_gateway, &vpc_id).await;

        // And the detached gateway is one the delete now accepts: the detach is exactly what
        // "you must detach the internet gateway from the VPC before you can delete it" asks
        // for.
        let (status, body) = delete_gateway(&router, &internet_gateway_id, false).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting the detached {internet_gateway_id}, got \
             {status} with body {body}"
        );
        let (status, body) = describe_one(&router, &internet_gateway_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted {internet_gateway_id} to name nothing, got \
             {status} with body {body}"
        );
    }
}

/// Which pair a detach refusal row sends.
#[derive(Debug, Clone, Copy)]
enum DetachPair {
    /// The gateway attached to the first VPC, named with that VPC — the pair that holds.
    Attached,
    /// The gateway attached to the first VPC, named with the OTHER VPC.
    AttachedToAnotherVpc,
    /// A gateway the account holds that is attached to nothing, named with the first VPC.
    Unattached,
    /// The attached gateway, named with a VPC id the account holds no VPC for.
    UnknownVpc,
    /// A gateway id the account holds no gateway for, named with the first VPC.
    UnknownGateway,
    /// The gateway attached to the VPC that still holds a running instance with an Elastic IP
    /// address.
    AttachedToVpcWithMappedAddress,
}

/// One row of the detach refusal table.
#[derive(Debug, Clone)]
struct DetachRefusalCase {
    case: &'static str,
    pair: DetachPair,
    dry_run: bool,
    status: u16,
    code: &'static str,
}

/// RULE 5: every refused detach leaves the attachment where it was, with the code that names
/// the reason.
#[tokio::test]
async fn a_refused_detach_leaves_the_attachment_where_it_was() {
    let (router, engine) = super::ec2_query::emulator();

    let vpc_id = create_vpc(&router, "10.60.0.0/16").await;
    let other_vpc_id = create_vpc(&router, "10.61.0.0/16").await;
    let attached_gateway = create_gateway(&router, &[("Name", "attached")]).await;
    attach(&router, &attached_gateway, &vpc_id).await;
    let unattached_gateway = create_gateway(&router, &[]).await;

    // A third VPC holding one running instance with an Elastic IP address: the dependency the
    // operation's own doc names ("the VPC must not contain any running instances with Elastic
    // IP addresses or public IPv4 addresses"). The address is associated over the wire, so the
    // public address the refusal reads is state the operations wrote; only the instance's
    // RUNNING state is seeded, the clock being what moves a launch out of `pending` and no
    // call being able to hold it there.
    let mapped_vpc_id = create_vpc(&router, "10.62.0.0/16").await;
    let mapped_subnet_id = create_subnet(&router, &mapped_vpc_id, "10.62.1.0/24").await;
    let mapped_gateway = create_gateway(&router, &[("Name", "mapped")]).await;
    attach(&router, &mapped_gateway, &mapped_vpc_id).await;
    let instance_id = launch_instance(&router, &mapped_subnet_id).await;
    park_instance(&engine, &instance_id, InstanceStateName::Running);
    associate_elastic_ip(&router, &instance_id).await;

    let cases = [
        DetachRefusalCase {
            case: "a gateway attached to another VPC",
            pair: DetachPair::AttachedToAnotherVpc,
            dry_run: false,
            status: 400,
            code: "Gateway.NotAttached",
        },
        DetachRefusalCase {
            case: "a gateway attached to nothing",
            pair: DetachPair::Unattached,
            dry_run: false,
            status: 400,
            code: "Gateway.NotAttached",
        },
        DetachRefusalCase {
            case: "a gateway attached to nothing, as a dry run",
            pair: DetachPair::Unattached,
            dry_run: true,
            status: 400,
            code: "Gateway.NotAttached",
        },
        DetachRefusalCase {
            case: "a VPC id the account holds no VPC for",
            pair: DetachPair::UnknownVpc,
            dry_run: false,
            status: 400,
            code: "InvalidVpcID.NotFound",
        },
        DetachRefusalCase {
            case: "a gateway id the account holds no gateway for",
            pair: DetachPair::UnknownGateway,
            dry_run: false,
            status: 400,
            code: "InvalidInternetGatewayID.NotFound",
        },
        DetachRefusalCase {
            case: "a VPC with a running instance holding an Elastic IP address",
            pair: DetachPair::AttachedToVpcWithMappedAddress,
            dry_run: false,
            status: 400,
            code: "DependencyViolation",
        },
        DetachRefusalCase {
            case: "a VPC with a running instance holding an Elastic IP address, as a dry run",
            pair: DetachPair::AttachedToVpcWithMappedAddress,
            dry_run: true,
            status: 400,
            code: "DependencyViolation",
        },
        DetachRefusalCase {
            case: "a dry run of a detach that would have succeeded",
            pair: DetachPair::Attached,
            dry_run: true,
            status: 412,
            code: "DryRunOperation",
        },
    ];

    /// An id of no gateway, in the long EC2 resource id form.
    const UNKNOWN_GATEWAY: &str = "igw-0123456789abcdef0";
    /// An id of no VPC, in the long EC2 resource id form.
    const UNKNOWN_VPC: &str = "vpc-0123456789abcdef0";

    for row in cases {
        let case = row.case;
        let (gateway, vpc) = match row.pair {
            DetachPair::Attached => (attached_gateway.clone(), vpc_id.clone()),
            DetachPair::AttachedToAnotherVpc => (attached_gateway.clone(), other_vpc_id.clone()),
            DetachPair::Unattached => (unattached_gateway.clone(), vpc_id.clone()),
            DetachPair::UnknownVpc => (attached_gateway.clone(), UNKNOWN_VPC.to_string()),
            DetachPair::UnknownGateway => (UNKNOWN_GATEWAY.to_string(), vpc_id.clone()),
            DetachPair::AttachedToVpcWithMappedAddress => {
                (mapped_gateway.clone(), mapped_vpc_id.clone())
            }
        };

        let (status, body) = detach_gateway(&router, &gateway, &vpc, row.dry_run).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {} detaching {gateway} from {vpc}, got {status} with \
             body {body}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected {} detaching {gateway} from {vpc}, got {body}",
            row.code
        );
        let message = error_message(&body);
        let expected_message = match row.code {
            "Gateway.NotAttached" => {
                format!("resource {gateway} is not attached to network {vpc}")
            }
            "InvalidVpcID.NotFound" => format!("The vpc ID '{vpc}' does not exist"),
            "InvalidInternetGatewayID.NotFound" => {
                format!("The internetGateway ID '{gateway}' does not exist")
            }
            "DependencyViolation" => format!(
                "Network {vpc} has some mapped public address(es). Please unmap those public \
                 address(es) before detaching the gateway."
            ),
            _ => "Request would have succeeded, but DryRun flag is set.".to_string(),
        };
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the refusal to be worded {expected_message:?}, got \
             {message:?} in {body}"
        );

        // A refused detach takes nothing away: both attachments the account holds are still
        // the ones the attaches recorded.
        assert_eq!(
            attachments_of(&router, case, &attached_gateway).await,
            vec![("available".to_string(), vpc_id.clone())],
            "case {case:?}: expected {attached_gateway} still attached to {vpc_id}"
        );
        assert_eq!(
            attachments_of(&router, case, &mapped_gateway).await,
            vec![("available".to_string(), mapped_vpc_id.clone())],
            "case {case:?}: expected {mapped_gateway} still attached to {mapped_vpc_id}"
        );
        assert_eq!(
            attachments_of(&router, case, &unattached_gateway).await,
            Vec::new(),
            "case {case:?}: expected {unattached_gateway} still attached to nothing"
        );
    }

    // The dependency is the RUNNING instance with the address, so an instance that is no
    // longer running leaves the detach the doc allows.
    park_instance(&engine, &instance_id, InstanceStateName::Stopped);
    detach(
        &router,
        "the instance is no longer running",
        &mapped_gateway,
        &mapped_vpc_id,
    )
    .await;
    assert_eq!(
        attachments_of(&router, "the instance is no longer running", &mapped_gateway).await,
        Vec::new(),
        "expected {mapped_gateway} detached once no running instance held an address"
    );
}

/// A subnet of `vpc_id` created over the wire, by id.
async fn create_subnet(router: &MultiServiceRouter, vpc_id: &str, cidr_block: &str) -> String {
    let params = [param("VpcId", vpc_id), param("CidrBlock", cidr_block)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateSubnet", &params).await;
    assert_eq!(
        status, 200,
        "CreateSubnet {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "subnetId")
        .unwrap_or_else(|| panic!("CreateSubnet answered no subnetId: {body}"))
}

/// One instance launched into `subnet_id`, by id.
async fn launch_instance(router: &MultiServiceRouter, subnet_id: &str) -> String {
    let params = [
        // Any image of the catalog: which one is launched is nothing this rule turns on.
        param("ImageId", "ami-0123456789abcdef0"),
        param("MinCount", "1"),
        param("MaxCount", "1"),
        param("SubnetId", subnet_id),
    ];
    let (status, body) = super::ec2_query::call(router, REGION, "RunInstances", &params).await;
    assert_eq!(
        status, 200,
        "RunInstances in {subnet_id}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances answered no instanceId: {body}"))
}

/// Hold the instance `instance_id` still in `state`.
///
/// The transient lifecycle states cannot be held across a call through the operations that
/// start them — the clock moves a launched instance on whether or not anybody calls — so the
/// state is written into the store the way the sibling instance suites seed the states no
/// sequence of calls can hold still.
fn park_instance(
    engine: &engine::InMemoryEngine,
    instance_id: &str,
    state: InstanceStateName,
) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let instance = guard
        .instances
        .get_mut(instance_id)
        .unwrap_or_else(|| panic!("the store must hold the launched instance {instance_id}"));
    instance.state = state;
    instance.pending_transition = None;
}

/// Allocate one Elastic IP address and associate it with `instance_id`, so the instance
/// reports a public address (`Instance$PublicIpAddress`).
async fn associate_elastic_ip(router: &MultiServiceRouter, instance_id: &str) {
    let (status, body) = super::ec2_query::call(router, REGION, "AllocateAddress", &[]).await;
    assert_eq!(
        status, 200,
        "AllocateAddress: expected HTTP 200, got {status} with body {body}"
    );
    let allocation_id = element(&body, "allocationId")
        .unwrap_or_else(|| panic!("AllocateAddress answered no allocationId: {body}"));
    let params = [
        param("AllocationId", &allocation_id),
        param("InstanceId", instance_id),
    ];
    let (status, body) = super::ec2_query::call(router, REGION, "AssociateAddress", &params).await;
    assert_eq!(
        status, 200,
        "AssociateAddress {allocation_id} to {instance_id}: expected HTTP 200, got {status} with \
         body {body}"
    );
}
