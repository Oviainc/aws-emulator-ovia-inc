// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateLocalGatewayRouteTableVirtualInterfaceGroupAssociation` and
//! the `DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations` that reads one
//! back, driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! The rule under test is that the create COMPUTES its answer from the store: it joins a
//! local gateway route table the account HOLDS with a virtual interface group the account
//! HOLDS, mints the association id and the owning account itself, reads the route table's
//! ARN and local gateway off the stored route table, answers the non-terminal `associating`
//! state the clock owes `associated`, and refuses a request naming a resource the account
//! does not hold with that resource's own `NotFound` code (and an id that is not one of that
//! resource's kind at all with its `.Malformed`). Every table row below is a distinct input
//! along what that generalises over — which route table, in which Region, under which mode,
//! which virtual interface group, on which local gateway, how many tags, which resource type
//! a tag specification names, which of the two named resources is missing and in what form,
//! and whether the request only asks about permission.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATED,
    LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATING, LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_ID_PREFIX,
    LocalGatewayRouteTableSpec, LocalGatewayVirtualInterfaceGroupSpec, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};

/// The Region every call here is signed for. The stores are per account rather than per
/// Region, so the Region only selects the ec2Query envelope — and the Region a stored route
/// table's ARN names is the one its writer was signed for, which is why one seeded route
/// table carries [`OTHER_REGION`].
const REGION: &str = "us-east-1";

/// The Region of the one route table seeded from another Region, so the ARN a reported
/// association carries is visibly the route table's own rather than this suite's Region.
const OTHER_REGION: &str = "us-west-2";

/// The ec2Query action under test.
const CREATE: &str = "CreateLocalGatewayRouteTableVirtualInterfaceGroupAssociation";

/// The reader this suite proves a created association round-trips through.
const DESCRIBE: &str = "DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations";

/// The XML element each association's id arrives in
/// (`LocalGatewayRouteTableVirtualInterfaceGroupAssociation$LocalGatewayRouteTableVirtualInterfaceGroupAssociationId`'s
/// `xmlName`).
const ID_ELEMENT: &str = "localGatewayRouteTableVirtualInterfaceGroupAssociationId";

/// Long enough past [`TRANSITION_DELAY`] that the move the clock owes a new association is
/// certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `CreateLocalGatewayRouteTableVirtualInterfaceGroupAssociation` call, as its status
/// and body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, CREATE, params).await
}

/// One `DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations` call, as its status
/// and body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, DESCRIBE, params).await
}

/// The form fields naming the route table and the virtual interface group to associate.
fn pair(route_table_id: &str, group_id: &str) -> Vec<(String, String)> {
    vec![
        param("LocalGatewayRouteTableId", route_table_id),
        param("LocalGatewayVirtualInterfaceGroupId", group_id),
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

/// The route tables this suite associates virtual interface groups with: two Regions, both
/// modes, and a local gateway shared by two of them, so an association's reported ARN and
/// local gateway can only have come from the route table it was made in.
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

/// What one seeded virtual interface group is: the local gateway it belongs to, the virtual
/// interfaces in it, its local BGP configuration, the Region its ARN names and its
/// configuration state. Written through the store's own seam for the reason a route table is.
struct GroupRow {
    local_gateway_id: &'static str,
    virtual_interface_ids: &'static [&'static str],
    local_bgp_asn: Option<i32>,
    region: &'static str,
    /// One of `LocalGatewayVirtualInterfaceGroupConfigurationState`'s members.
    configuration_state: &'static str,
}

/// The virtual interface groups this suite associates: two on each of the two local gateways
/// the route tables above belong to, differing in how many virtual interfaces they hold and
/// in their BGP configuration.
fn group_rows() -> Vec<GroupRow> {
    vec![
        GroupRow {
            local_gateway_id: "lgw-00000000000000001",
            virtual_interface_ids: &["lgw-vif-00000000000000001"],
            local_bgp_asn: Some(64512),
            region: REGION,
            configuration_state: "available",
        },
        GroupRow {
            local_gateway_id: "lgw-00000000000000001",
            virtual_interface_ids: &[
                "lgw-vif-00000000000000002",
                "lgw-vif-00000000000000003",
            ],
            local_bgp_asn: None,
            region: REGION,
            configuration_state: "available",
        },
        GroupRow {
            local_gateway_id: "lgw-00000000000000002",
            virtual_interface_ids: &[],
            local_bgp_asn: Some(64513),
            region: OTHER_REGION,
            configuration_state: "available",
        },
        GroupRow {
            local_gateway_id: "lgw-00000000000000002",
            virtual_interface_ids: &["lgw-vif-00000000000000004"],
            local_bgp_asn: None,
            region: OTHER_REGION,
            configuration_state: "pending",
        },
    ]
}

/// Write one route table into the store and answer the id it was given.
fn seed_route_table(engine: &engine::InMemoryEngine, row: &RouteTableRow) -> String {
    let case = row.mode;
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

/// Write one virtual interface group into the store and answer the id it was given.
fn seed_group(engine: &engine::InMemoryEngine, row: &GroupRow) -> String {
    let case = row.configuration_state;
    engine
        .state::<Ec2State>()
        .lock()
        .insert_local_gateway_virtual_interface_group(LocalGatewayVirtualInterfaceGroupSpec {
            local_gateway_id: row.local_gateway_id.to_string(),
            local_gateway_virtual_interface_ids: row
                .virtual_interface_ids
                .iter()
                .map(|id| (*id).to_string())
                .collect(),
            local_bgp_asn: row.local_bgp_asn,
            local_bgp_asn_extended: None,
            region: row.region.to_string(),
            configuration_state: row.configuration_state.to_string(),
            tags: Vec::new(),
        })
        .unwrap_or_else(|| panic!("case {case:?}: the store must take the seeded group"))
}

/// Every route table and every virtual interface group of the tables above, in row order.
fn seed_all(engine: &engine::InMemoryEngine) -> (Vec<String>, Vec<String>) {
    let route_tables = route_table_rows()
        .iter()
        .map(|row| seed_route_table(engine, row))
        .collect();
    let groups = group_rows()
        .iter()
        .map(|row| seed_group(engine, row))
        .collect();
    (route_tables, groups)
}

/// The ARN of a local gateway route table, as
/// `LocalGatewayRouteTableVirtualInterfaceGroupAssociation$LocalGatewayRouteTableArn`
/// carries it: `arn:aws:ec2:<region>:<account>:local-gateway-route-table/<id>`.
fn route_table_arn(region: &str, route_table_id: &str) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:local-gateway-route-table/{route_table_id}",
        engine::emulator_account_id()
    )
}

/// How many associations the store holds.
fn stored_associations(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .local_gateway_route_table_virtual_interface_group_associations
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

/// One row of the create table: the route table to associate in, the virtual interface group
/// to associate, and the tags to assign.
struct CreateRow {
    route_table: usize,
    group: usize,
    tags: Vec<(&'static str, &'static str)>,
}

/// Every member of a created association comes from the store: the minted id, the route
/// table's own id, ARN and local gateway, the group named, the owning account, the transient
/// state and the tags the request assigned — and the association reads back identically
/// through `DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations`.
#[tokio::test]
async fn a_created_association_reports_the_route_table_the_group_and_the_minted_members() {
    let (router, engine) = super::ec2_query::emulator();
    let (route_tables, groups) = seed_all(&engine);
    let rows = route_table_rows();

    // Rows differ along the route table (its Region and mode), the virtual interface group
    // (its local gateway, its interfaces and its BGP configuration) and the number of tags
    // assigned: none, one, and several.
    let cases = vec![
        CreateRow {
            route_table: 0,
            group: 0,
            tags: Vec::new(),
        },
        CreateRow {
            route_table: 1,
            group: 1,
            tags: vec![("Name", "rack-a")],
        },
        CreateRow {
            route_table: 2,
            group: 2,
            tags: vec![("Name", "rack-b"), ("Owner", "TeamA"), ("Tier", "edge")],
        },
    ];

    let mut minted: Vec<String> = Vec::new();
    for case_row in &cases {
        let case = format!(
            "route table {} with group {} and {} tag(s)",
            case_row.route_table,
            case_row.group,
            case_row.tags.len()
        );
        let route_table_id = &route_tables[case_row.route_table];
        let group_id = &groups[case_row.group];
        let row = &rows[case_row.route_table];

        let mut params = pair(route_table_id, group_id);
        if !case_row.tags.is_empty() {
            params.extend(tag_specification(
                "local-gateway-route-table-virtual-interface-group-association",
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
            association_id
                .starts_with(&format!("{LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_ID_PREFIX}-")),
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
            ("localGatewayVirtualInterfaceGroupId", group_id.clone()),
            ("localGatewayId", row.local_gateway_id.to_string()),
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
                "LocalGatewayRouteTableVirtualInterfaceGroupAssociationId",
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
            ("localGatewayVirtualInterfaceGroupId", group_id.clone()),
            ("localGatewayId", row.local_gateway_id.to_string()),
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
    let (route_tables, groups) = seed_all(&engine);

    let (status, body) = create(&router, &pair(&route_tables[0], &groups[0])).await;
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
        "after the transition: the clock owed the association \
         {LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATED:?}, got {read_body}"
    );
}

// ---------------------------------------------------------------------------
// The two named resources must be resources of the account
// ---------------------------------------------------------------------------

/// A `LocalGatewayRouteTableId` that is not a route table id of the account is refused
/// before anything is written: one that is not a local gateway route table id at all with
/// `InvalidLocalGatewayRouteTableID.Malformed`, and a well-formed one the account holds no
/// route table for with `InvalidLocalGatewayRouteTableID.NotFound` — and the route table is
/// resolved FIRST, so a request naming neither resource of the account is refused about its
/// route table.
#[tokio::test]
async fn a_route_table_the_account_does_not_hold_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let (route_tables, groups) = seed_all(&engine);

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
            "lgw-vif-grp-0123456789abcdef0",
            "InvalidLocalGatewayRouteTableID.Malformed",
        ),
        (
            "lgw-vif-grp-assoc-0123456789abcdef0",
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
        // Against a group the account HOLDS and against one it does not, so the order the
        // two resources are resolved in is pinned rather than assumed.
        for named_group in [groups[0].as_str(), "lgw-vif-grp-0123456789abcdef0"] {
            let case = format!("{route_table_id} with group {named_group}");
            let (status, body) = create(&router, &pair(route_table_id, named_group)).await;
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
                assert_eq!(
                    error_message(&body),
                    format!("The LocalGateway Route Table ID '{route_table_id}' does not exist"),
                    "case {case:?}: the refusal must name the id the request carried, got {body}"
                );
            }
        }
    }

    assert_eq!(
        stored_associations(&engine),
        0,
        "a refused create must store nothing"
    );

    // The stored route table is still associable, so the refusals above were about the id.
    let (status, body) = create(&router, &pair(&route_tables[0], &groups[0])).await;
    assert_eq!(
        status, 200,
        "the stored route table: expected HTTP 200, got {status} with body {body}"
    );
}

/// A `LocalGatewayVirtualInterfaceGroupId` that is not a virtual interface group id of the
/// account is refused the same way its route table is: `.Malformed` for an id that is not
/// one of that kind at all, `.NotFound` for a well-formed one the account holds no group for.
#[tokio::test]
async fn a_virtual_interface_group_the_account_does_not_hold_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let (route_tables, groups) = seed_all(&engine);

    // Rows: the long and the short well-formed id form (both unknown), the association id
    // kind this action mints (whose suffix is not hexadecimal under the group's prefix),
    // another EC2 resource kind, a bare prefix, a non-hexadecimal suffix, and free text.
    let cases = [
        (
            "lgw-vif-grp-0123456789abcdef0",
            "InvalidLocalGatewayVirtualInterfaceGroupID.NotFound",
        ),
        (
            "lgw-vif-grp-01234567",
            "InvalidLocalGatewayVirtualInterfaceGroupID.NotFound",
        ),
        (
            "lgw-vif-grp-assoc-0123456789abcdef0",
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
        (
            "lgw-rtb-0123456789abcdef0",
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
        (
            "lgw-vif-grp-",
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
        (
            "lgw-vif-grp-zzzzzzzzzzzzzzzzz",
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
        (
            "not-a-group-id",
            "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed",
        ),
    ];
    for (group_id, expected_code) in cases {
        let case = group_id.to_string();
        let (status, body) = create(&router, &pair(&route_tables[0], group_id)).await;
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
            assert_eq!(
                error_message(&body),
                format!("The local-gateway-virtual-interface-group ID '{group_id}' does not exist"),
                "case {case:?}: the refusal must name the id the request carried, got {body}"
            );
        }
    }

    assert_eq!(
        stored_associations(&engine),
        0,
        "a refused create must store nothing"
    );

    // The stored group is still associable, so the refusals above were about the id.
    let (status, body) = create(&router, &pair(&route_tables[0], &groups[0])).await;
    assert_eq!(
        status, 200,
        "the stored group: expected HTTP 200, got {status} with body {body}"
    );
}

/// A route table and a virtual interface group are resources OF a local gateway, and the
/// association reports the one local gateway it is on, so a pair sitting on two different
/// local gateways is refused with `InvalidParameterValue` naming both.
#[tokio::test]
async fn a_group_on_another_local_gateway_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let (route_tables, groups) = seed_all(&engine);
    let tables = route_table_rows();
    let rows = group_rows();

    // Rows: every (route table, group) pair across the two local gateways — the matching
    // ones are accepted and the crossing ones refused, so the rule is asserted in both
    // directions rather than only where it refuses.
    for (table_index, table_id) in route_tables.iter().enumerate() {
        for (group_index, group_id) in groups.iter().enumerate() {
            let same_gateway =
                tables[table_index].local_gateway_id == rows[group_index].local_gateway_id;
            let case = format!("route table {table_index} with group {group_index}");
            let (status, body) = create(&router, &pair(table_id, group_id)).await;
            let expected_status = if same_gateway { 200 } else { 400 };
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
            );
            if !same_gateway {
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterValue",
                    "case {case:?}: unexpected error code in {body}"
                );
                let message = error_message(&body);
                assert!(
                    message.contains(rows[group_index].local_gateway_id)
                        && message.contains(tables[table_index].local_gateway_id),
                    "case {case:?}: the refusal must name both local gateways, got {message:?}"
                );
            }
        }
    }
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
    let (route_tables, groups) = seed_all(&engine);

    // Rows: each id given as an empty value and both empty (the handler's refusal), then
    // each id omitted entirely (the boundary validator's).
    let cases: Vec<MissingParameterCase> = vec![
        (
            "an empty LocalGatewayRouteTableId",
            pair("", &groups[0]),
            "MissingParameter",
        ),
        (
            "an empty LocalGatewayVirtualInterfaceGroupId",
            pair(&route_tables[0], ""),
            "MissingParameter",
        ),
        ("both ids empty", pair("", ""), "MissingParameter"),
        (
            "no LocalGatewayRouteTableId",
            vec![param("LocalGatewayVirtualInterfaceGroupId", &groups[0])],
            "MalformedInput",
        ),
        (
            "no LocalGatewayVirtualInterfaceGroupId",
            vec![param("LocalGatewayRouteTableId", &route_tables[0])],
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

/// A dry run asks about PERMISSION rather than about the resources named, so it reports
/// `DryRunOperation` whatever the request would have gone on to resolve — and writes nothing.
/// The same request without the flag then takes the resources into account.
#[tokio::test]
async fn a_dry_run_reports_it_would_have_succeeded_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let (route_tables, groups) = seed_all(&engine);

    // Rows: the pair that would have succeeded, pairs naming a route table or a group the
    // account does not hold, and a pair on two different local gateways — a dry run of each
    // reports the same permission answer.
    let cases: Vec<(&str, &str, &str)> = vec![
        (
            "a pair that would succeed",
            route_tables[0].as_str(),
            groups[0].as_str(),
        ),
        (
            "an unknown route table",
            "lgw-rtb-0123456789abcdef0",
            groups[0].as_str(),
        ),
        (
            "an unknown virtual interface group",
            route_tables[0].as_str(),
            "lgw-vif-grp-0123456789abcdef0",
        ),
        (
            "a pair on two local gateways",
            route_tables[0].as_str(),
            groups[2].as_str(),
        ),
    ];
    for (case, route_table, group) in cases {
        let mut params = pair(route_table, group);
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

    let (status, body) = create(&router, &pair(&route_tables[0], &groups[0])).await;
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

/// An association IS the pair it joins: the same (route table, virtual interface group) pair
/// cannot be associated twice, while the same group in another route table of its local
/// gateway and another group in the same route table both can.
#[tokio::test]
async fn one_pair_cannot_be_associated_twice_but_other_pairs_can() {
    let (router, engine) = super::ec2_query::emulator();
    let (route_tables, groups) = seed_all(&engine);

    let (status, body) = create(&router, &pair(&route_tables[0], &groups[0])).await;
    assert_eq!(
        status, 200,
        "the first association: expected HTTP 200, got {status} with body {body}"
    );
    let existing = element(&body, ID_ELEMENT)
        .unwrap_or_else(|| panic!("the first association: no <{ID_ELEMENT}> in {body}"));

    // Rows: the same pair again, the same route table with another group, and another route
    // table of the same local gateway with the same group.
    let cases: Vec<(&str, &str, &str, u16)> = vec![
        (
            "the same pair",
            route_tables[0].as_str(),
            groups[0].as_str(),
            400,
        ),
        (
            "another group in the same route table",
            route_tables[0].as_str(),
            groups[1].as_str(),
            200,
        ),
        (
            "the same group in another route table",
            route_tables[1].as_str(),
            groups[0].as_str(),
            200,
        ),
    ];
    for (case, route_table_id, group_id, expected_status) in cases {
        let (status, body) = create(&router, &pair(route_table_id, group_id)).await;
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
                error_message(&body).contains(&existing),
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
    let (route_tables, groups) = seed_all(&engine);

    // Rows from `ResourceType`'s own members: the resources this request names, the sibling
    // association type, and the type this action actually creates.
    let cases: Vec<(&str, u16)> = vec![
        ("local-gateway-virtual-interface-group", 400),
        ("local-gateway-route-table", 400),
        ("local-gateway-route-table-vpc-association", 400),
        (
            "local-gateway-route-table-virtual-interface-group-association",
            200,
        ),
    ];
    for (resource_type, expected_status) in cases {
        let mut params = pair(&route_tables[0], &groups[0]);
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

    // The one accepted specification's tags are the association's, and they read back
    // through the describe.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the tagged association: expected HTTP 200, got {status} with body {body}"
    );
    for (member, value) in [("key", "Name"), ("value", "tagged")] {
        assert_member("the tagged association", &body, member, value);
    }
}
