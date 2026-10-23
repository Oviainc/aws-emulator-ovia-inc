// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations`,
//! driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations`, answered with
//! XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2`
//! signing name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the account's local gateway route table / virtual interface group associations, narrowed
//! by the `LocalGatewayRouteTableVirtualInterfaceGroupAssociationIds` and `Filters` the
//! request names and cut into `MaxResults`-sized pages. Every table row is a distinct input
//! along what that generalises over — which filter, how many values, how many filters,
//! which ids, which page size — and an account with no Outpost (the empty store) is one row
//! among them rather than the specification.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State,
    LocalGatewayRouteTableVirtualInterfaceGroupAssociationSpec as AssociationSpec,
};
use ec2::ops::types::Tag;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. The association store is per account rather
/// than per Region, so the Region only selects the ec2Query envelope — and the Region a
/// stored association's route-table ARN names is the one its writer was signed for, which
/// is why one seeded row carries [`OTHER_REGION`].
const REGION: &str = "us-east-1";

/// The Region of the one seeded association written from another Region, so the
/// `local-gateway-route-table-arn` filter has two ARN namespaces to tell apart.
const OTHER_REGION: &str = "us-west-2";

/// The ec2Query action under test.
const ACTION: &str = "DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations";

/// The XML element each reported association's id arrives in
/// (`LocalGatewayRouteTableVirtualInterfaceGroupAssociation$LocalGatewayRouteTableVirtualInterfaceGroupAssociationId`'s
/// `xmlName`).
const ID_ELEMENT: &str = "localGatewayRouteTableVirtualInterfaceGroupAssociationId";

/// One call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The association ids a successful response reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, ID_ELEMENT)
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// A tag, as the seeded associations carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// What one seeded association joins, and in what state.
struct AssociationRow {
    local_gateway_id: &'static str,
    route_table_id: &'static str,
    virtual_interface_group_id: &'static str,
    region: &'static str,
    /// "The state of the association"
    /// (`LocalGatewayRouteTableVirtualInterfaceGroupAssociation$State`) — an
    /// unconstrained string in the model, carrying the lifecycle states an Outpost local
    /// gateway association moves through.
    state: &'static str,
    tags: Vec<Tag>,
}

/// The associations the filter and paging tables are written against. They differ along
/// every attribute the documented filters select on: the local gateway, the route table,
/// the virtual interface group, the Region the route-table ARN names, and the state — and
/// two rows share a local gateway and two share a route table, so a filter that selects on
/// one of those reports more than a single row.
fn seeded_rows() -> Vec<AssociationRow> {
    vec![
        AssociationRow {
            local_gateway_id: "lgw-00000000000000001",
            route_table_id: "lgw-rtb-00000000000000001",
            virtual_interface_group_id: "lgw-vif-grp-00000000000000001",
            region: REGION,
            state: "available",
            tags: vec![tag("Name", "rack-a"), tag("Owner", "TeamA")],
        },
        AssociationRow {
            local_gateway_id: "lgw-00000000000000001",
            route_table_id: "lgw-rtb-00000000000000001",
            virtual_interface_group_id: "lgw-vif-grp-00000000000000002",
            region: REGION,
            state: "pending",
            tags: vec![tag("Name", "rack-a")],
        },
        AssociationRow {
            local_gateway_id: "lgw-00000000000000001",
            route_table_id: "lgw-rtb-00000000000000002",
            virtual_interface_group_id: "lgw-vif-grp-00000000000000003",
            region: REGION,
            state: "available",
            tags: Vec::new(),
        },
        AssociationRow {
            local_gateway_id: "lgw-00000000000000002",
            route_table_id: "lgw-rtb-00000000000000003",
            virtual_interface_group_id: "lgw-vif-grp-00000000000000004",
            region: REGION,
            state: "deleting",
            tags: vec![tag("Owner", "TeamB")],
        },
        AssociationRow {
            local_gateway_id: "lgw-00000000000000002",
            route_table_id: "lgw-rtb-00000000000000004",
            virtual_interface_group_id: "lgw-vif-grp-00000000000000005",
            region: REGION,
            state: "deleted",
            tags: Vec::new(),
        },
        AssociationRow {
            local_gateway_id: "lgw-00000000000000003",
            route_table_id: "lgw-rtb-00000000000000005",
            virtual_interface_group_id: "lgw-vif-grp-00000000000000006",
            region: OTHER_REGION,
            state: "available",
            tags: vec![tag("Project", "atlas")],
        },
        AssociationRow {
            local_gateway_id: "lgw-00000000000000004",
            route_table_id: "lgw-rtb-00000000000000006",
            virtual_interface_group_id: "lgw-vif-grp-00000000000000007",
            region: REGION,
            state: "pending",
            tags: vec![tag("Name", "rack-b")],
        },
    ]
}

/// Write the seeded associations into the engine's store, the way the operation that
/// associates a virtual interface group with a local gateway route table would, and report
/// the ids they were given in store order.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut ids = Vec::new();
    for row in seeded_rows() {
        ids.push(
            guard.insert_local_gateway_route_table_virtual_interface_group_association(
                AssociationSpec {
                    local_gateway_route_table_id: row.route_table_id.to_string(),
                    local_gateway_virtual_interface_group_id: row
                        .virtual_interface_group_id
                        .to_string(),
                    local_gateway_id: row.local_gateway_id.to_string(),
                    region: row.region.to_string(),
                    state: row.state.to_string(),
                    tags: row.tags,
                },
            ),
        );
    }
    ids
}

/// The ids at `indices`, in store order.
fn ids_at(seeded: &[String], indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded[*index].clone()).collect()
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

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// An account with no Outpost holds no association — a local gateway, its route tables and
/// its virtual interface groups come with an Outpost rather than from any EC2 call — so the
/// operation answers HTTP 200 with an empty set, and describing is a READ: two calls report
/// the same thing and the store is untouched.
#[tokio::test]
async fn an_account_with_no_outpost_reports_an_empty_set_and_describing_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert!(
        reported.is_empty(),
        "fresh account: expected no association, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "fresh account: a complete report carries no NextToken, got {body}"
    );

    let second = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second, reported,
        "a read must not change what a later caller sees: {reported:?} then {second:?}"
    );
    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .local_gateway_route_table_virtual_interface_group_associations
            .len(),
        0,
        "describing associations must not write to the store"
    );
}

/// Every association the store holds is reported, in store order, with every member of the
/// modelled shape — and one written after a call shows up in the next one, so the answer is
/// computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_association_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "all associations: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert_eq!(
        reported, seeded,
        "all associations: expected every stored association in store order, got {reported:?}"
    );

    // Every member of the reported shape comes from the stored association: the ids it
    // joins, the route table ARN built from the Region its writer was signed for, the
    // owning account, the state and the tags.
    let rows = seeded_rows();
    for (index, row) in rows.iter().enumerate() {
        let case = format!("row {index}");
        for (member, value) in [
            ("localGatewayId", row.local_gateway_id.to_string()),
            (
                "localGatewayRouteTableId",
                row.route_table_id.to_string(),
            ),
            (
                "localGatewayVirtualInterfaceGroupId",
                row.virtual_interface_group_id.to_string(),
            ),
            (
                "localGatewayRouteTableArn",
                route_table_arn(row.region, row.route_table_id),
            ),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("state", row.state.to_string()),
        ] {
            assert!(
                body.contains(&format!("<{member}>{value}</{member}>")),
                "case {case:?}: expected <{member}>{value}</{member}>, got {body}"
            );
        }
        for stored_tag in &row.tags {
            let key = stored_tag.key.clone().unwrap_or_default();
            let value = stored_tag.value.clone().unwrap_or_default();
            assert!(
                body.contains(&format!("<key>{key}</key>")),
                "case {case:?}: expected the tag key {key:?} to be reported, got {body}"
            );
            assert!(
                body.contains(&format!("<value>{value}</value>")),
                "case {case:?}: expected the tag value {value:?} to be reported, got {body}"
            );
        }
    }

    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_local_gateway_route_table_virtual_interface_group_association(AssociationSpec {
            local_gateway_route_table_id: "lgw-rtb-00000000000000009".to_string(),
            local_gateway_virtual_interface_group_id: "lgw-vif-grp-00000000000000009".to_string(),
            local_gateway_id: "lgw-00000000000000009".to_string(),
            region: REGION.to_string(),
            state: "available".to_string(),
            tags: vec![tag("Name", "late")],
        });
    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute, values within one filter are OR'd,
/// and the comparison is exact and case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_associations_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let rows = seeded_rows();
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.len()).collect();

    // One row per filter name
    // `DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociationsRequest$Filters`
    // documents, each asserted on an association the issue never cited.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "local-gateway-id",
            vec![rows[0].local_gateway_id.to_string()],
            vec![0, 1, 2],
        ),
        // Two values for one filter: joined with OR.
        (
            "local-gateway-id",
            vec![
                rows[3].local_gateway_id.to_string(),
                rows[5].local_gateway_id.to_string(),
            ],
            vec![3, 4, 5],
        ),
        // A local gateway nothing is associated with: an empty set, not an error.
        (
            "local-gateway-id",
            vec!["lgw-0ffffffffffffffff".to_string()],
            vec![],
        ),
        (
            "local-gateway-route-table-id",
            vec![rows[0].route_table_id.to_string()],
            vec![0, 1],
        ),
        (
            "local-gateway-route-table-id",
            vec![rows[6].route_table_id.to_string()],
            vec![6],
        ),
        (
            "local-gateway-route-table-arn",
            vec![route_table_arn(rows[2].region, rows[2].route_table_id)],
            vec![2],
        ),
        // The one association written from another Region: its ARN names that Region, so
        // the same route table id under this Region's namespace selects nothing.
        (
            "local-gateway-route-table-arn",
            vec![route_table_arn(OTHER_REGION, rows[5].route_table_id)],
            vec![5],
        ),
        (
            "local-gateway-route-table-arn",
            vec![route_table_arn(REGION, rows[5].route_table_id)],
            vec![],
        ),
        (
            "local-gateway-route-table-virtual-interface-group-id",
            vec![rows[4].virtual_interface_group_id.to_string()],
            vec![4],
        ),
        (
            "local-gateway-route-table-virtual-interface-group-association-id",
            vec![seeded[3].clone()],
            vec![3],
        ),
        (
            "local-gateway-route-table-virtual-interface-group-association-id",
            vec![seeded[1].clone(), seeded[6].clone()],
            vec![1, 6],
        ),
        ("owner-id", vec![account], all.clone()),
        ("owner-id", vec!["111122223333".to_string()], vec![]),
        ("state", vec!["available".to_string()], vec![0, 2, 5]),
        ("state", vec!["pending".to_string()], vec![1, 6]),
        ("state", vec!["deleting".to_string()], vec![3]),
        ("state", vec!["deleted".to_string()], vec![4]),
        (
            "state",
            vec!["deleting".to_string(), "deleted".to_string()],
            vec![3, 4],
        ),
        // "Filter values are case-sensitive" (`Filter$Values`).
        ("state", vec!["AVAILABLE".to_string()], vec![]),
        // Filter values carry EC2's wildcards: `*` for zero or more characters, `?` for
        // exactly one ("List and filter using the CLI and API", the guide the modelled
        // `Filter` shape links to).
        ("state", vec!["delet*".to_string()], vec![3, 4]),
        ("state", vec!["pendin?".to_string()], vec![1, 6]),
        ("state", vec!["pendin??".to_string()], vec![]),
        (
            "local-gateway-route-table-arn",
            vec![format!("arn:aws:ec2:{OTHER_REGION}:*")],
            vec![5],
        ),
        ("local-gateway-id", vec!["lgw-*".to_string()], all.clone()),
        // A documented filter carrying no value at all: there is no value for the
        // attribute to match, so it selects nothing rather than everything.
        ("state", vec![], vec![]),
        ("local-gateway-id", vec![], vec![]),
    ];

    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the associations that
/// must come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the associations matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000001"][..]),
                ("state", &["available"][..]),
            ],
            vec![0, 2],
        ),
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000001"][..]),
                (
                    "local-gateway-route-table-id",
                    &["lgw-rtb-00000000000000001"][..],
                ),
                ("state", &["pending"][..]),
            ],
            vec![1],
        ),
        // Each filter matches something, but no association matches both.
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000003"][..]),
                ("state", &["deleted"][..]),
            ],
            vec![],
        ),
    ];

    for (entries, expected_indices) in cases {
        let case = format!("{entries:?}");
        let reported = described_ids(&router, &case, &filters(&entries)).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: a near-miss of a documented name, the underscore spelling, a documented name
    // with the wrong case, the plural, the tag filters this operation's `Filters` doc does
    // NOT name (even though the association carries tags), and an empty name.
    let cases = [
        "local-gateway-route-table-virtual-interface-group",
        "local_gateway_id",
        "Local-Gateway-Id",
        "local-gateway-ids",
        "tag-key",
        "tag:Name",
        "tag:",
        "",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &["x"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        // The EC2 error envelope is XML, so the message's apostrophes arrive escaped.
        assert!(
            unescaped(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: the refusal must name the filter, got {body}"
        );
        assert!(
            elements(&body, ID_ELEMENT).is_empty(),
            "case {name:?}: a refused request reports no association, got {body}"
        );
    }
}

/// One row of the bounds table: what the case is called, the form fields to send, and the
/// HTTP status the request must answer with.
type BoundsCase = (String, Vec<(String, String)>, u16);

/// The request-controlled factors of the store scan are bounded: each of the crate's caps
/// is accepted at its published value and refused one past it, so no request can ask for
/// unbounded filtering work.
#[tokio::test]
async fn the_filter_and_id_factors_are_accepted_at_their_caps_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: the number of filters, the values in one filter, the bytes in one value and the
    // number of association ids — each at the crate's cap (`ec2::state`) and one past it.
    let many_filters = |count: usize| -> Vec<(String, String)> {
        let entries: Vec<(&str, &[&str])> = (0..count).map(|_| ("state", &["available"][..])).collect();
        filters(&entries)
    };
    let many_values = |count: usize| -> Vec<(String, String)> {
        let values: Vec<String> = (0..count).map(|index| format!("state-{index}")).collect();
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        filters(&[("state", &borrowed)])
    };
    let one_value_of = |bytes: usize| -> Vec<(String, String)> {
        let value = "a".repeat(bytes);
        filters(&[("state", &[value.as_str()])])
    };
    let many_ids = |count: usize| -> Vec<(String, String)> {
        let ids: Vec<String> = (0..count)
            .map(|index| format!("lgw-vif-grp-assoc-{index:017x}"))
            .collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        indexed(
            "LocalGatewayRouteTableVirtualInterfaceGroupAssociationId",
            &borrowed,
        )
    };

    let cases: Vec<BoundsCase> = vec![
        (
            format!("{} filters", ec2::state::MAX_FILTERS),
            many_filters(ec2::state::MAX_FILTERS),
            200,
        ),
        (
            format!("{} filters", ec2::state::MAX_FILTERS + 1),
            many_filters(ec2::state::MAX_FILTERS + 1),
            400,
        ),
        (
            format!("{} filter values", ec2::state::MAX_FILTER_VALUES),
            many_values(ec2::state::MAX_FILTER_VALUES),
            200,
        ),
        (
            format!("{} filter values", ec2::state::MAX_FILTER_VALUES + 1),
            many_values(ec2::state::MAX_FILTER_VALUES + 1),
            400,
        ),
        (
            format!("a {}-byte filter value", ec2::state::MAX_FILTER_VALUE_BYTES),
            one_value_of(ec2::state::MAX_FILTER_VALUE_BYTES),
            200,
        ),
        (
            format!(
                "a {}-byte filter value",
                ec2::state::MAX_FILTER_VALUE_BYTES + 1
            ),
            one_value_of(ec2::state::MAX_FILTER_VALUE_BYTES + 1),
            400,
        ),
        (
            format!("{} ids", ec2::state::MAX_RESOURCE_IDS),
            many_ids(ec2::state::MAX_RESOURCE_IDS),
            200,
        ),
        (
            format!("{} ids", ec2::state::MAX_RESOURCE_IDS + 1),
            many_ids(ec2::state::MAX_RESOURCE_IDS + 1),
            400,
        ),
    ];

    for (case, params, expected_status) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            assert!(
                elements(&body, ID_ELEMENT).is_empty(),
                "case {case:?}: a refused request reports no association, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// LocalGatewayRouteTableVirtualInterfaceGroupAssociationIds
// ---------------------------------------------------------------------------

/// The ids narrow the report to the associations named, in store order whatever order they
/// were asked for; an id that names no association of the account contributes no row.
#[tokio::test]
async fn the_ids_narrow_the_report_to_the_associations_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows differ in count and in the order the ids are asked for.
    let cases: Vec<Vec<usize>> = vec![
        vec![0],
        vec![6],
        vec![2, 4],
        vec![4, 2],
        vec![5, 1, 3],
        (0..seeded.len()).collect(),
    ];
    for asked in cases {
        let ids = ids_at(&seeded, &asked);
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("ids={borrowed:?}");
        let reported = described_ids(
            &router,
            &case,
            &indexed(
                "LocalGatewayRouteTableVirtualInterfaceGroupAssociationId",
                &borrowed,
            ),
        )
        .await;
        let mut expected_indices = asked.clone();
        expected_indices.sort_unstable();
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // An id this account holds no association for: no row, and the known ids beside it are
    // still reported.
    let unknown = "lgw-vif-grp-assoc-0ffffffffffffffff";
    let known = seeded[2].clone();
    let unknown_cases: Vec<(Vec<String>, Vec<usize>)> = vec![
        (vec![unknown.to_string()], vec![]),
        (vec!["not-an-association-id".to_string()], vec![]),
        (vec![known.clone(), unknown.to_string()], vec![2]),
        (vec![unknown.to_string(), known], vec![2]),
    ];
    for (ids, expected_indices) in unknown_cases {
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("ids={borrowed:?}");
        let reported = described_ids(
            &router,
            &case,
            &indexed(
                "LocalGatewayRouteTableVirtualInterfaceGroupAssociationId",
                &borrowed,
            ),
        )
        .await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every association exactly once, in
/// store order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_association_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `LocalGatewayMaxResults` is modelled `range 5..1000`, so the rows are its published
    // lower bound, a size that divides the store exactly, one that does not, and the upper
    // bound, which holds everything.
    for max_results in [5, 6, 7, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
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
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token.clone()));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded,
            "case {case:?}: expected every association exactly once in store order, got \
             {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Six of the seven seeded associations belong to these three local gateways, so the
    // model's minimum page size cuts the filtered set into two pages.
    let selected_gateways: &[&str] = &[
        "lgw-00000000000000001",
        "lgw-00000000000000002",
        "lgw-00000000000000003",
    ];
    let page_params = || {
        let mut params = filters(&[("local-gateway-id", selected_gateways)]);
        params.extend(max_results_param(5));
        params
    };

    let (status, body) = describe(&router, &page_params()).await;
    assert_eq!(
        status, 200,
        "filtered first page: expected HTTP 200, got {status} with body {body}"
    );
    let first = elements(&body, ID_ELEMENT);
    assert_eq!(
        first,
        ids_at(&seeded, &[0, 1, 2, 3, 4]),
        "filtered first page: expected the first five associations of those gateways, got \
         {first:?}"
    );
    let token = element(&body, "nextToken")
        .unwrap_or_else(|| panic!("filtered first page: expected a NextToken in {body}"));

    let mut next = page_params();
    next.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &next).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[5]),
        "filtered second page: expected the remaining association of those gateways, got \
         {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`LocalGatewayMaxResults`, `range 5..1000`) and one past each.
    let cases = [(4, 400), (5, 200), (1000, 200), (1001, 400)];
    for (max_results, expected_status) in cases {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`,
/// rather than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, and that id hex-encoded (the shape the
    // token takes, without the marker and checksum a minted token carries).
    let forged_marker = ::hex::encode(format!("ec2-1:0000000000000000:{}", seeded[0]));
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded[0].clone(),
        ::hex::encode(&seeded[0]),
        forged_marker,
    ];
    for token in cases {
        let params = vec![("NextToken".to_string(), token.clone())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[("DryRun".to_string(), "true".to_string())]).await;
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

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded,
        "DryRun=false: expected the ordinary report, got {reported:?}"
    );
}
