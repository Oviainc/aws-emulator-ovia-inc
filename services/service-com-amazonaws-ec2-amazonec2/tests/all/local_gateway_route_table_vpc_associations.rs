// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateLocalGatewayRouteTableVpcAssociation` and the
//! `DescribeLocalGatewayRouteTableVpcAssociations` that reads one back, driven over the wire
//! the service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST
//! whose body carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md
//! "Wire protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the create COMPUTES its answer from the store: it joins a
//! local gateway route table the account HOLDS with a VPC the account HOLDS, mints the
//! association id and the owning account itself, reads the route table's ARN and local
//! gateway off the stored route table, answers the non-terminal `associating` state the
//! clock owes `associated`, and refuses a request naming a resource the account does not
//! hold with that resource's own `NotFound` code. Every table row below is a distinct input
//! along what that generalises over — which route table, in which Region, under which mode,
//! which VPC, how many tags, which resource type a tag specification names, which of the
//! two named resources is missing, and whether the request only asks about permission.

use std::sync::Arc;
use std::time::Duration;

use ec2::ops::types::Tag;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATED,
    LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATING,
    LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATION_ID_PREFIX, LocalGatewayRouteTableSpec,
    LocalGatewayRouteTableVpcAssociationSpec, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. The stores are per account rather than per
/// Region, so the Region only selects the ec2Query envelope — and the Region a stored route
/// table's ARN names is the one its writer was signed for, which is why one seeded route
/// table carries [`OTHER_REGION`].
const REGION: &str = "us-east-1";

/// The Region of the one route table seeded from another Region, so the ARN a reported
/// association carries is visibly the route table's own rather than this suite's Region.
const OTHER_REGION: &str = "us-west-2";

/// The ec2Query action under test.
const CREATE: &str = "CreateLocalGatewayRouteTableVpcAssociation";

/// The reader this suite proves a created association round-trips through.
const DESCRIBE: &str = "DescribeLocalGatewayRouteTableVpcAssociations";

/// The XML element each association's id arrives in
/// (`LocalGatewayRouteTableVpcAssociation$LocalGatewayRouteTableVpcAssociationId`'s
/// `xmlName`).
const ID_ELEMENT: &str = "localGatewayRouteTableVpcAssociationId";

/// Long enough past [`TRANSITION_DELAY`] that the move the clock owes a new association is
/// certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `CreateLocalGatewayRouteTableVpcAssociation` call, as its status and body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, CREATE, params).await
}

/// One `DescribeLocalGatewayRouteTableVpcAssociations` call, as its status and body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, DESCRIBE, params).await
}

/// The association ids a successful describe reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {DESCRIBE}, got {status} with body {body}"
    );
    elements(&body, ID_ELEMENT)
}

/// The form fields naming the route table and the VPC to associate.
fn pair(route_table_id: &str, vpc_id: &str) -> Vec<(String, String)> {
    vec![
        param("LocalGatewayRouteTableId", route_table_id),
        param("VpcId", vpc_id),
    ]
}

/// The form fields for one `TagSpecification` naming `resource_type` and `tags`.
fn tag_specification(resource_type: &str, tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut fields = vec![param("TagSpecification.1.ResourceType", resource_type)];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        fields.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
        fields.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    fields
}

/// What one seeded local gateway route table is: the gateway and Outpost it belongs to, the
/// Region its ARN names, its mode and its state. Nothing in the EC2 API this emulator serves
/// creates an Outpost, so a route table is written straight through the store's own seam —
/// the one `CreateLocalGatewayRouteTable` will write through.
struct RouteTableRow {
    local_gateway_id: &'static str,
    outpost_arn: &'static str,
    region: &'static str,
    /// One of `LocalGatewayRouteTableMode`'s members (`coip`, `direct-vpc-routing`).
    mode: &'static str,
    state: &'static str,
}

/// The route tables this suite associates VPCs with: two Regions, both modes, and a local
/// gateway shared by two of them, so an association's reported ARN and local gateway can
/// only have come from the route table it was made in.
fn route_table_rows() -> Vec<RouteTableRow> {
    vec![
        RouteTableRow {
            local_gateway_id: "lgw-00000000000000001",
            outpost_arn: "arn:aws:outposts:us-east-1:111122223333:outpost/op-00000000000000001",
            region: REGION,
            mode: "direct-vpc-routing",
            state: "available",
        },
        RouteTableRow {
            local_gateway_id: "lgw-00000000000000001",
            outpost_arn: "arn:aws:outposts:us-east-1:111122223333:outpost/op-00000000000000001",
            region: REGION,
            mode: "coip",
            state: "available",
        },
        RouteTableRow {
            local_gateway_id: "lgw-00000000000000002",
            outpost_arn: "arn:aws:outposts:us-west-2:111122223333:outpost/op-00000000000000002",
            region: OTHER_REGION,
            mode: "coip",
            state: "available",
        },
    ]
}

/// Write one route table into the store and answer the id it was given.
fn seed_route_table(engine: &engine::InMemoryEngine, row: &RouteTableRow) -> String {
    let case = row.local_gateway_id;
    engine
        .state::<Ec2State>()
        .lock()
        .insert_local_gateway_route_table(LocalGatewayRouteTableSpec {
            local_gateway_id: row.local_gateway_id.to_string(),
            outpost_arn: row.outpost_arn.to_string(),
            region: row.region.to_string(),
            mode: row.mode.to_string(),
            state: row.state.to_string(),
            tags: Vec::new(),
        })
        .unwrap_or_else(|| panic!("case {case:?}: the store must take the seeded route table"))
}

/// The ARN of a local gateway route table, as
/// `LocalGatewayRouteTableVpcAssociation$LocalGatewayRouteTableArn` carries it:
/// `arn:aws:ec2:<region>:<account>:local-gateway-route-table/<id>`.
fn route_table_arn(region: &str, route_table_id: &str) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:local-gateway-route-table/{route_table_id}",
        engine::emulator_account_id()
    )
}

/// `CreateVpc` for `cidr`, answering the `VpcId` it minted.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr: &str) -> String {
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateVpc", &[param("CidrBlock", cidr)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpc, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// How many associations the store holds.
fn stored_associations(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .local_gateway_route_table_vpc_associations
        .len()
}

/// Assert that `body` carries `<member>value</member>`.
fn assert_member(case: &str, body: &str, member: &str, value: &str) {
    assert!(
        body.contains(&format!("<{member}>{value}</{member}>")),
        "case {case:?}: expected <{member}>{value}</{member}>, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The create computes its answer from the store
// ---------------------------------------------------------------------------

/// One row of the create table: the route table to associate in, the CIDR of the VPC to
/// associate, and the tags to assign.
struct CreateRow {
    route_table: usize,
    cidr: &'static str,
    tags: Vec<(&'static str, &'static str)>,
}

/// Every member of a created association comes from the store: the minted id, the route
/// table's own id, ARN and local gateway, the VPC named, the owning account, the transient
/// state and the tags the request assigned — and the association reads back identically
/// through `DescribeLocalGatewayRouteTableVpcAssociations`.
#[tokio::test]
async fn a_created_association_reports_the_route_table_the_vpc_and_the_minted_members() {
    let (router, engine) = super::ec2_query::emulator();
    let rows = route_table_rows();
    let route_tables: Vec<String> = rows
        .iter()
        .map(|row| seed_route_table(&engine, row))
        .collect();

    // Rows differ along the route table (its Region and mode), the VPC and the number of
    // tags assigned: none, one, and several.
    let cases = vec![
        CreateRow {
            route_table: 0,
            cidr: "10.10.0.0/16",
            tags: Vec::new(),
        },
        CreateRow {
            route_table: 1,
            cidr: "10.11.0.0/16",
            tags: vec![("Name", "rack-a")],
        },
        CreateRow {
            route_table: 2,
            cidr: "10.12.0.0/16",
            tags: vec![("Name", "rack-b"), ("Owner", "TeamA"), ("Tier", "edge")],
        },
    ];

    let mut minted: Vec<String> = Vec::new();
    for case_row in &cases {
        let case = format!(
            "route table {} with {} tag(s)",
            case_row.route_table,
            case_row.tags.len()
        );
        let route_table_id = &route_tables[case_row.route_table];
        let row = &rows[case_row.route_table];
        let vpc_id = create_vpc(&case, &router, case_row.cidr).await;

        let mut params = pair(route_table_id, &vpc_id);
        if !case_row.tags.is_empty() {
            params.extend(tag_specification(
                "local-gateway-route-table-vpc-association",
                &case_row.tags,
            ));
        }
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {CREATE}, got {status} with body {body}"
        );

        let association_id = element(&body, ID_ELEMENT)
            .unwrap_or_else(|| panic!("case {case:?}: no <{ID_ELEMENT}> in {body}"));
        assert!(
            association_id.starts_with(&format!("{LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATION_ID_PREFIX}-")),
            "case {case:?}: expected an id minted with the association prefix, got \
             {association_id:?}"
        );
        assert!(
            !minted.contains(&association_id),
            "case {case:?}: expected a fresh association id, got the already-minted \
             {association_id:?}"
        );

        for (member, value) in [
            ("localGatewayRouteTableId", route_table_id.clone()),
            (
                "localGatewayRouteTableArn",
                route_table_arn(row.region, route_table_id),
            ),
            ("localGatewayId", row.local_gateway_id.to_string()),
            ("vpcId", vpc_id.clone()),
            ("ownerId", engine::emulator_account_id().to_string()),
            (
                "state",
                LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATING.to_string(),
            ),
        ] {
            assert_member(&case, &body, member, &value);
        }
        for (key, value) in &case_row.tags {
            assert_member(&case, &body, "key", key);
            assert_member(&case, &body, "value", value);
        }

        // The round trip: the association reads back through the describe with the same
        // members, so the create answered the record it stored rather than its request.
        let (read_status, read_body) = describe(
            &router,
            &indexed(
                "LocalGatewayRouteTableVpcAssociationId",
                &[association_id.as_str()],
            ),
        )
        .await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected HTTP 200 from {DESCRIBE}, got {read_status} with body \
             {read_body}"
        );
        assert_eq!(
            elements(&read_body, ID_ELEMENT),
            vec![association_id.clone()],
            "case {case:?}: expected {DESCRIBE} to report exactly the created association, \
             got {read_body}"
        );
        for (member, value) in [
            ("localGatewayRouteTableId", route_table_id.clone()),
            (
                "localGatewayRouteTableArn",
                route_table_arn(row.region, route_table_id),
            ),
            ("localGatewayId", row.local_gateway_id.to_string()),
            ("vpcId", vpc_id.clone()),
            ("ownerId", engine::emulator_account_id().to_string()),
        ] {
            assert_member(&case, &read_body, member, &value);
        }
        for (key, value) in &case_row.tags {
            assert_member(&case, &read_body, "key", key);
            assert_member(&case, &read_body, "value", value);
        }

        minted.push(association_id);
    }

    assert_eq!(
        stored_associations(&engine),
        cases.len(),
        "every created association must be stored once"
    );
}

/// A new association is answered in the non-terminal `associating` state, and the clock owes
/// it `associated` one `TRANSITION_DELAY` later whether or not anybody calls again.
#[tokio::test]
async fn an_association_starts_associating_and_the_clock_owes_it_associated() {
    let (router, engine) = super::ec2_query::emulator();
    let rows = route_table_rows();
    let route_table_id = seed_route_table(&engine, &rows[0]);
    let vpc_id = create_vpc("the clock", &router, "10.20.0.0/16").await;

    let (status, body) = create(&router, &pair(&route_table_id, &vpc_id)).await;
    assert_eq!(
        status, 200,
        "the clock: expected HTTP 200 from {CREATE}, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATING),
        "the clock: a new association starts in the transient state, got {body}"
    );
    let association_id = element(&body, ID_ELEMENT)
        .unwrap_or_else(|| panic!("the clock: no <{ID_ELEMENT}> in {body}"));

    tokio::time::sleep(past_the_transition()).await;

    let (read_status, read_body) = describe(&router, &[]).await;
    assert_eq!(
        read_status, 200,
        "after the transition: expected HTTP 200, got {read_status} with body {read_body}"
    );
    assert_eq!(
        elements(&read_body, ID_ELEMENT),
        vec![association_id],
        "after the transition: expected the one association, got {read_body}"
    );
    assert_eq!(
        element(&read_body, "state").as_deref(),
        Some(LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATED),
        "after the transition: the clock owed the association {LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATED:?}, got {read_body}"
    );
}

// ---------------------------------------------------------------------------
// The two named resources must be resources of the account
// ---------------------------------------------------------------------------

/// A `LocalGatewayRouteTableId` that is not a route table id of the account is refused
/// before anything is written: one that is not a local gateway route table id at all with
/// `InvalidLocalGatewayRouteTableID.Malformed`, and a well-formed one the account holds no
/// route table for with `InvalidLocalGatewayRouteTableID.NotFound`.
#[tokio::test]
async fn a_route_table_the_account_does_not_hold_is_refused_not_found() {
    let (router, engine) = super::ec2_query::emulator();
    let rows = route_table_rows();
    // One route table IS stored, so the refusals below are about the id named rather than
    // about an empty store.
    let stored = seed_route_table(&engine, &rows[0]);
    let vpc_id = create_vpc("not found", &router, "10.30.0.0/16").await;

    // Rows: the long and the short well-formed id form (both unknown), then ids that are not
    // a local gateway route table id at all — another EC2 resource kind, the association kind
    // this action mints, a bare prefix, a suffix that is not hexadecimal, and free text.
    let cases = [
        (
            "lgw-rtb-0123456789abcdef0",
            "InvalidLocalGatewayRouteTableID.NotFound",
        ),
        ("lgw-rtb-01234567", "InvalidLocalGatewayRouteTableID.NotFound"),
        (
            "vpc-0123456789abcdef0",
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        (
            "lgw-vpc-assoc-0123456789abcdef0",
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        ("lgw-rtb-", "InvalidLocalGatewayRouteTableID.Malformed"),
        (
            "lgw-rtb-zzzzzzzzzzzzzzzzz",
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        (
            "not-a-route-table-id",
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
    ];
    for (route_table_id, expected_code) in cases {
        let case = route_table_id.to_string();
        let (status, body) = create(&router, &pair(route_table_id, &vpc_id)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code:?}, got {body}"
        );
        if expected_code.ends_with(".NotFound") {
            let message = unescaped(&element(&body, "Message").unwrap_or_default());
            assert_eq!(
                message,
                format!("The LocalGateway Route Table ID '{route_table_id}' does not exist"),
                "case {case:?}: the refusal must name the id the request carried, got {body}"
            );
        }
    }

    assert_eq!(
        stored_associations(&engine),
        0,
        "a refused create must store nothing"
    );

    // The stored route table is still associable, so the refusals above were about the id.
    let (status, body) = create(&router, &pair(&stored, &vpc_id)).await;
    assert_eq!(
        status, 200,
        "the stored route table: expected HTTP 200, got {status} with body {body}"
    );
}

/// One row of the missing-parameter table: what the case is called, the form fields to send,
/// and the error code the request must be refused with.
type MissingParameterCase = (&'static str, Vec<(String, String)>, &'static str);

/// A required parameter the request carried as an EMPTY value is refused with EC2's
/// `MissingParameter` naming it — the two ids are modelled `clientOptional`, so an empty one
/// reaches the handler rather than the boundary validator — and that refusal is answered
/// whether or not the request only asks about permission. A parameter left out ALTOGETHER is
/// a null the boundary validator refuses before the handler, with the framework's own
/// `MalformedInput`, as every other operation of this service answers it.
#[tokio::test]
async fn a_required_parameter_the_request_left_out_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let rows = route_table_rows();
    let route_table_id = seed_route_table(&engine, &rows[0]);
    let vpc_id = create_vpc("missing parameter", &router, "10.80.0.0/16").await;

    // Rows: each id given as an empty value and both empty (the handler's refusal), then
    // each id omitted entirely (the boundary validator's).
    let cases: Vec<MissingParameterCase> = vec![
        (
            "an empty LocalGatewayRouteTableId",
            pair("", &vpc_id),
            "MissingParameter",
        ),
        ("an empty VpcId", pair(&route_table_id, ""), "MissingParameter"),
        ("both ids empty", pair("", ""), "MissingParameter"),
        (
            "no LocalGatewayRouteTableId",
            vec![param("VpcId", &vpc_id)],
            "MalformedInput",
        ),
        (
            "no VpcId",
            vec![param("LocalGatewayRouteTableId", &route_table_id)],
            "MalformedInput",
        ),
        ("neither id", Vec::new(), "MalformedInput"),
    ];
    for (case, params, expected_code) in cases {
        for dry_run in [false, true] {
            let case = format!("{case} (DryRun={dry_run})");
            let mut params = params.clone();
            if dry_run {
                params.push(param("DryRun", "true"));
            }
            let (status, body) = create(&router, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected the code {expected_code:?}, got {body}"
            );
        }
    }
    assert_eq!(
        stored_associations(&engine),
        0,
        "a refused create must store nothing"
    );
}

/// A `VpcId` the account holds no VPC for is refused with `InvalidVpcID.NotFound` — and it
/// is the VPC that is resolved FIRST, so a request naming neither a VPC nor a route table of
/// the account is refused about its VPC.
#[tokio::test]
async fn a_vpc_the_account_does_not_hold_is_refused_not_found() {
    let (router, engine) = super::ec2_query::emulator();
    let rows = route_table_rows();
    let route_table_id = seed_route_table(&engine, &rows[2]);

    // Rows: the long and the short VPC id form, an id of another kind, and free text — each
    // against the stored route table and against an id the account holds no route table for,
    // so the order the two are resolved in is pinned rather than assumed.
    let unknown = [
        "vpc-0123456789abcdef0",
        "vpc-01234567",
        "subnet-0123456789abcdef0",
        "not-a-vpc-id",
    ];
    for vpc_id in unknown {
        for named_route_table in [route_table_id.as_str(), "lgw-rtb-0123456789abcdef0"] {
            let case = format!("vpc {vpc_id} in {named_route_table}");
            let (status, body) = create(&router, &pair(named_route_table, vpc_id)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidVpcID.NotFound",
                "case {case:?}: unexpected error code in {body}"
            );
        }
    }
    assert_eq!(
        stored_associations(&engine),
        0,
        "a refused create must store nothing"
    );
}

/// A dry run asks about PERMISSION rather than about the resources named, so it reports
/// `DryRunOperation` whatever the request would have gone on to resolve — and writes nothing.
/// The same request without the flag then takes the resources into account.
#[tokio::test]
async fn a_dry_run_reports_it_would_have_succeeded_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let rows = route_table_rows();
    let route_table_id = seed_route_table(&engine, &rows[0]);
    let vpc_id = create_vpc("dry run", &router, "10.40.0.0/16").await;

    // Rows: the pair that would have succeeded, and pairs naming a route table or a VPC the
    // account does not hold — a dry run of each reports the same permission answer.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a pair that would succeed", route_table_id.as_str(), vpc_id.as_str()),
        (
            "an unknown route table",
            "lgw-rtb-0123456789abcdef0",
            vpc_id.as_str(),
        ),
        (
            "an unknown VPC",
            route_table_id.as_str(),
            "vpc-0123456789abcdef0",
        ),
    ];
    for (case, route_table, vpc) in cases {
        let mut params = pair(route_table, vpc);
        params.push(param("DryRun", "true"));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            stored_associations(&engine),
            0,
            "case {case:?}: a dry run may write nothing"
        );
    }

    let (status, body) = create(&router, &pair(&route_table_id, &vpc_id)).await;
    assert_eq!(
        status, 200,
        "after the dry run: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        stored_associations(&engine),
        1,
        "after the dry run: the association must be stored exactly once"
    );
}

/// An association IS the pair it joins: the same (route table, VPC) pair cannot be
/// associated twice, while the same VPC in another route table and another VPC in the same
/// route table both can.
#[tokio::test]
async fn one_pair_cannot_be_associated_twice_but_other_pairs_can() {
    let (router, engine) = super::ec2_query::emulator();
    let rows = route_table_rows();
    let first_table = seed_route_table(&engine, &rows[0]);
    let second_table = seed_route_table(&engine, &rows[1]);
    let first_vpc = create_vpc("pairs", &router, "10.50.0.0/16").await;
    let second_vpc = create_vpc("pairs", &router, "10.51.0.0/16").await;

    let (status, body) = create(&router, &pair(&first_table, &first_vpc)).await;
    assert_eq!(
        status, 200,
        "the first association: expected HTTP 200, got {status} with body {body}"
    );
    let existing = element(&body, ID_ELEMENT)
        .unwrap_or_else(|| panic!("the first association: no <{ID_ELEMENT}> in {body}"));

    // Rows: the same pair again, the same route table with another VPC, and another route
    // table with the same VPC.
    let cases: Vec<(&str, &str, &str, u16)> = vec![
        ("the same pair", first_table.as_str(), first_vpc.as_str(), 400),
        (
            "another VPC in the same route table",
            first_table.as_str(),
            second_vpc.as_str(),
            200,
        ),
        (
            "the same VPC in another route table",
            second_table.as_str(),
            first_vpc.as_str(),
            200,
        ),
    ];
    for (case, route_table_id, vpc_id, expected_status) in cases {
        let (status, body) = create(&router, &pair(route_table_id, vpc_id)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: unexpected error code in {body}"
            );
            assert!(
                unescaped(&element(&body, "Message").unwrap_or_default()).contains(&existing),
                "case {case:?}: the refusal must name the association already held \
                 ({existing}), got {body}"
            );
        }
    }

    assert_eq!(
        stored_associations(&engine),
        3,
        "only the repeat of one pair may be refused"
    );
}

/// A `TagSpecification` may name only the resource type this action creates; any other is
/// refused with `InvalidParameterValue` rather than accepted and discarded.
#[tokio::test]
async fn a_tag_specification_naming_another_resource_type_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let rows = route_table_rows();
    let route_table_id = seed_route_table(&engine, &rows[0]);
    let vpc_id = create_vpc("tag specs", &router, "10.60.0.0/16").await;

    // Rows from `ResourceType`'s own members: the resources this request names, the sibling
    // association type, and the type this action actually creates.
    let cases: Vec<(&str, u16)> = vec![
        ("vpc", 400),
        ("local-gateway-route-table", 400),
        (
            "local-gateway-route-table-virtual-interface-group-association",
            400,
        ),
        ("local-gateway-route-table-vpc-association", 200),
    ];
    for (resource_type, expected_status) in cases {
        let mut params = pair(&route_table_id, &vpc_id);
        params.extend(tag_specification(resource_type, &[("Name", "tagged")]));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {resource_type:?}: expected HTTP {expected_status}, got {status} with body \
             {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {resource_type:?}: unexpected error code in {body}"
            );
        }
    }
    assert_eq!(
        stored_associations(&engine),
        1,
        "only the specification naming this action's own resource type may be stored"
    );
}

// ---------------------------------------------------------------------------
// Reading the store back
// ---------------------------------------------------------------------------

/// What one directly seeded association joins, and in what state: the states an association
/// moves through are the `associating`/`associated`/`disassociating`/`disassociated`
/// vocabulary every association-state enum of this model spells, and only a create can leave
/// one `associating`, so the terminal ones are written through the store's own seam.
struct SeededAssociation {
    route_table: usize,
    vpc_id: &'static str,
    state: &'static str,
}

/// The ids every documented filter is asserted against: three associations made over the
/// wire and four written straight into the store, so the set spans two route tables, two
/// Regions' ARNs, several VPCs and four states — and is bigger than the model's smallest page.
async fn seeded_store(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    route_tables: &[String],
    rows: &[RouteTableRow],
) -> (Vec<String>, Vec<String>) {
    let mut ids: Vec<String> = Vec::new();
    let mut vpcs: Vec<String> = Vec::new();
    for (index, cidr) in ["10.70.0.0/16", "10.71.0.0/16", "10.72.0.0/16"]
        .iter()
        .enumerate()
    {
        let vpc_id = create_vpc("seeded store", router, cidr).await;
        let route_table_id = &route_tables[usize::from(index > 1)];
        let (status, body) = create(router, &pair(route_table_id, &vpc_id)).await;
        assert_eq!(
            status, 200,
            "seeding {cidr}: expected HTTP 200, got {status} with body {body}"
        );
        ids.push(
            element(&body, ID_ELEMENT)
                .unwrap_or_else(|| panic!("seeding {cidr}: no <{ID_ELEMENT}> in {body}")),
        );
        vpcs.push(vpc_id);
    }

    let seeded = [
        SeededAssociation {
            route_table: 0,
            vpc_id: "vpc-00000000000000011",
            state: "associated",
        },
        SeededAssociation {
            route_table: 1,
            vpc_id: "vpc-00000000000000012",
            state: "disassociating",
        },
        SeededAssociation {
            route_table: 2,
            vpc_id: "vpc-00000000000000013",
            state: "disassociated",
        },
        SeededAssociation {
            route_table: 2,
            vpc_id: "vpc-00000000000000014",
            state: "associated",
        },
    ];
    for association in &seeded {
        let case = association.vpc_id;
        let route_table_id = route_tables[association.route_table].clone();
        let row = &rows[association.route_table];
        let id = engine
            .state::<Ec2State>()
            .lock()
            .insert_local_gateway_route_table_vpc_association(
                LocalGatewayRouteTableVpcAssociationSpec {
                    local_gateway_route_table_arn: route_table_arn(row.region, &route_table_id),
                    local_gateway_route_table_id: route_table_id,
                    local_gateway_id: row.local_gateway_id.to_string(),
                    vpc_id: association.vpc_id.to_string(),
                    state: association.state.to_string(),
                    becomes: None,
                    tags: vec![Tag {
                        key: Some("Name".to_string()),
                        value: Some(association.vpc_id.to_string()),
                    }],
                },
            )
            .unwrap_or_else(|| panic!("case {case:?}: the store must take the association"));
        ids.push(id);
        vpcs.push(association.vpc_id.to_string());
    }
    (ids, vpcs)
}

/// Each documented filter selects on its own attribute, values within one filter are OR'd,
/// several filters are AND'd, and an undocumented name is refused rather than reported as an
/// empty set.
#[tokio::test]
async fn each_documented_filter_selects_the_associations_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let rows = route_table_rows();
    let route_tables: Vec<String> = rows
        .iter()
        .map(|row| seed_route_table(&engine, row))
        .collect();
    let (ids, vpcs) = seeded_store(&router, &engine, &route_tables, &rows).await;
    // The created associations have had time to reach `associated` only if the clock owed it;
    // the filter rows below that select on `state` name the terminal states the seeded rows
    // carry, so they do not depend on where inside `TRANSITION_DELAY` this test is running.
    tokio::time::sleep(past_the_transition()).await;

    let ids_at = |indices: &[usize]| -> Vec<String> {
        indices.iter().map(|index| ids[*index].clone()).collect()
    };
    let all: Vec<usize> = (0..ids.len()).collect();

    // One row per filter name `DescribeLocalGatewayRouteTableVpcAssociationsRequest$Filters`
    // documents, each asserted on an association the issue never cited.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "local-gateway-route-table-id",
            vec![route_tables[0].clone()],
            vec![0, 1, 3],
        ),
        (
            "local-gateway-route-table-id",
            vec![route_tables[1].clone(), route_tables[2].clone()],
            vec![2, 4, 5, 6],
        ),
        (
            "local-gateway-route-table-arn",
            vec![route_table_arn(OTHER_REGION, &route_tables[2])],
            vec![5, 6],
        ),
        (
            "local-gateway-id",
            vec![rows[0].local_gateway_id.to_string()],
            vec![0, 1, 2, 3, 4],
        ),
        (
            "local-gateway-id",
            vec![rows[2].local_gateway_id.to_string()],
            vec![5, 6],
        ),
        ("vpc-id", vec![vpcs[1].clone()], vec![1]),
        (
            "vpc-id",
            vec![vpcs[4].clone(), vpcs[6].clone()],
            vec![4, 6],
        ),
        (
            "local-gateway-route-table-vpc-association-id",
            vec![ids[2].clone()],
            vec![2],
        ),
        (
            "owner-id",
            vec![engine::emulator_account_id().to_string()],
            all.clone(),
        ),
        ("owner-id", vec!["111122223333".to_string()], vec![]),
        ("state", vec!["disassociating".to_string()], vec![4]),
        ("state", vec!["disassociated".to_string()], vec![5]),
        (
            "state",
            vec!["disassociating".to_string(), "disassociated".to_string()],
            vec![4, 5],
        ),
        (
            "state",
            vec![LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATED.to_string()],
            vec![0, 1, 2, 3, 6],
        ),
        // EC2's own wildcards, and a documented filter carrying no value at all: there is no
        // value for the attribute to match, so it selects nothing rather than everything.
        ("vpc-id", vec!["vpc-*".to_string()], all.clone()),
        ("state", vec![], vec![]),
    ];
    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // Several filters are joined with an AND.
    let combined = filters(&[
        ("local-gateway-route-table-id", &[route_tables[0].as_str()]),
        ("state", &[LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATED]),
    ]);
    let reported = described_ids(&router, "AND", &combined).await;
    assert_eq!(
        reported,
        ids_at(&[0, 1, 3]),
        "case AND: expected the associated associations of the first route table, got \
         {reported:?}"
    );

    // A filter name this operation does not document is refused, rather than silently
    // reported as an empty set.
    for name in ["tag-key", "tag:Name", "local_gateway_id", "Vpc-Id", "mode"] {
        let (status, body) = describe(&router, &filters(&[(name, &["x"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: unexpected error code in {body}"
        );
    }
}

/// Whatever the page size, walking the pages covers every association exactly once in store
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_association_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let rows = route_table_rows();
    let route_tables: Vec<String> = rows
        .iter()
        .map(|row| seed_route_table(&engine, row))
        .collect();
    let (ids, _vpcs) = seeded_store(&router, &engine, &route_tables, &rows).await;

    // `LocalGatewayMaxResults` is modelled `range 5..1000`, so the rows are its published
    // lower bound, a size that divides the store exactly, one that does not, and the upper
    // bound, which holds everything.
    for max_results in [5, 7, 6, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = vec![param("MaxResults", &max_results.to_string())];
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, ID_ELEMENT);
            assert!(
                page.len() <= max_results,
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= ids.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = vec![
                        param("MaxResults", &max_results.to_string()),
                        param("NextToken", token),
                    ];
                }
                None => break,
            }
        }
        assert_eq!(
            walked, ids,
            "case {case:?}: expected every association exactly once in store order, got \
             {walked:?}"
        );
    }

    // The published bounds of `LocalGatewayMaxResults` and one past each.
    for (max_results, expected_status) in [(4, 400), (5, 200), (1000, 200), (1001, 400)] {
        let (status, body) =
            describe(&router, &[param("MaxResults", &max_results.to_string())]).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }

    // A `NextToken` this service did not issue is refused rather than read as a cursor.
    for token in ["not-a-token", "###", "6161"] {
        let (status, body) = describe(&router, &[param("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: unexpected error code in {body}"
        );
    }
}

/// An account with no Outpost holds no route table and so no association: the describe
/// answers an empty set, and describing is a READ — two calls report the same thing and the
/// store is untouched.
#[tokio::test]
async fn an_account_with_no_outpost_reports_an_empty_set_and_describing_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let first = described_ids(&router, "fresh account", &[]).await;
    assert!(
        first.is_empty(),
        "fresh account: expected no association, got {first:?}"
    );
    let second = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second, first,
        "a read must not change what a later caller sees: {first:?} then {second:?}"
    );
    assert_eq!(
        stored_associations(&engine),
        0,
        "describing associations must not write to the store"
    );
}

/// `LocalGatewayRouteTableVpcAssociationIds` narrows the report to the ids named — in store
/// order whatever order they were asked for, and with an id the account holds no association
/// for contributing no row rather than widening the answer — and a describe that only asks
/// about permission reports that outcome instead of describing anything.
#[tokio::test]
async fn the_id_list_narrows_the_report_and_a_dry_run_describes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let rows = route_table_rows();
    let route_tables: Vec<String> = rows
        .iter()
        .map(|row| seed_route_table(&engine, row))
        .collect();
    let (ids, _vpcs) = seeded_store(&router, &engine, &route_tables, &rows).await;

    let ids_at = |indices: &[usize]| -> Vec<String> {
        indices.iter().map(|index| ids[*index].clone()).collect()
    };
    let all: Vec<usize> = (0..ids.len()).collect();

    // An id nothing in this account holds: the prefix every association id carries
    // ([`LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATION_ID_PREFIX`]) and 17 hexadecimal digits the
    // store's own mint, which counts from one, can never have produced.
    let unheld = format!("{LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATION_ID_PREFIX}-0ffffffffffffffff");

    // Rows differ in how many ids are named, in the order they are named, and in whether one
    // of them names an association the account holds.
    let cases: Vec<(Vec<String>, Vec<usize>)> = vec![
        (ids_at(&[0]), vec![0]),
        (ids_at(&[6]), vec![6]),
        (ids_at(&[1, 4]), vec![1, 4]),
        (ids_at(&[4, 1]), vec![1, 4]),
        (ids_at(&[5, 2, 3]), vec![2, 3, 5]),
        (ids_at(&all), all.clone()),
        (vec![unheld.clone()], Vec::new()),
        (vec![unheld.clone(), ids[2].clone()], vec![2]),
    ];
    for (asked, expected_indices) in cases {
        let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
        let case = format!("LocalGatewayRouteTableVpcAssociationIds={borrowed:?}");
        let reported = described_ids(
            &router,
            &case,
            &indexed("LocalGatewayRouteTableVpcAssociationId", &borrowed),
        )
        .await;
        let expected = ids_at(&expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, ID_ELEMENT).is_empty(),
        "DryRun=true: a dry run reports no association, got {body}"
    );

    let reported = described_ids(&router, "DryRun=false", &[param("DryRun", "false")]).await;
    assert_eq!(
        reported,
        ids_at(&all),
        "DryRun=false: expected the ordinary report, got {reported:?}"
    );
}
