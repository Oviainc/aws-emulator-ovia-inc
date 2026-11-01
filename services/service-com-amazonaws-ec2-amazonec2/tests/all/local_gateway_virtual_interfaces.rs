// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeLocalGatewayVirtualInterfaces`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=DescribeLocalGatewayVirtualInterfaces`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing
//! name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the account's local gateway virtual interfaces, narrowed by the
//! `LocalGatewayVirtualInterfaceIds` and `Filters` the request names and cut into
//! `MaxResults`-sized pages. Every table row is a distinct input along what that
//! generalises over — which filter, how many values, how many filters, which ids, which
//! members a virtual interface holds a value for, which page size — and an account with no
//! Outpost (the empty store) is one row among them rather than the specification.

use ec2::ops::types::Tag;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LocalGatewayVirtualInterfaceSpec as InterfaceSpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. The virtual interface store is per account
/// rather than per Region, so the Region only selects the ec2Query envelope — and the
/// Region a stored interface's ARN names is the one its writer was signed for, which is why
/// one seeded row carries [`OTHER_REGION`].
const REGION: &str = "us-east-1";

/// The Region of the one seeded virtual interface written from another Region, so the
/// reported ARNs have two Region namespaces to tell apart.
const OTHER_REGION: &str = "us-west-2";

/// The ec2Query action under test.
const ACTION: &str = "DescribeLocalGatewayVirtualInterfaces";

/// The XML element each reported virtual interface's id arrives in
/// (`LocalGatewayVirtualInterface$LocalGatewayVirtualInterfaceId`'s `xmlName`).
const ID_ELEMENT: &str = "localGatewayVirtualInterfaceId";

/// One call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The virtual interface ids a successful response reports, in the order it reported them.
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

/// A tag, as the seeded virtual interfaces carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// What one seeded local gateway virtual interface holds.
struct InterfaceRow {
    local_gateway_id: &'static str,
    /// "The ID of the local gateway virtual interface group" — absent on the row that
    /// belongs to no group yet, so a report has a member with no value in it.
    group_id: Option<&'static str>,
    outpost_lag_id: Option<&'static str>,
    vlan: Option<i32>,
    local_address: Option<&'static str>,
    peer_address: Option<&'static str>,
    local_bgp_asn: Option<i32>,
    peer_bgp_asn: Option<i32>,
    peer_bgp_asn_extended: Option<i64>,
    region: &'static str,
    /// "The current state of the local gateway virtual interface"
    /// (`LocalGatewayVirtualInterface$ConfigurationState`), one of the values
    /// `LocalGatewayVirtualInterfaceConfigurationState` publishes — the generated catalog
    /// `ec2::ops::types::LOCAL_GATEWAY_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES`, which
    /// [`every_seeded_state_is_a_modelled_configuration_state`] checks these against.
    configuration_state: &'static str,
    tags: Vec<Tag>,
}

/// The virtual interfaces the filter and paging tables are written against. They differ
/// along every attribute the documented filters select on — the local gateway, the local and
/// peer addresses, both BGP ASNs and the VLAN — and along every member that is merely
/// reported: the group, the Outpost LAG, the extended peer ASN, the Region the ARN names,
/// the configuration state and the tags. Three rows share a local gateway and two more share
/// another, two share a VLAN and two share a peer ASN, so a filter that selects on one
/// reports more than a single row; one row holds no value for most attributes at all.
fn seeded_rows() -> Vec<InterfaceRow> {
    vec![
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000001",
            group_id: Some("lgw-vif-grp-00000000000000001"),
            outpost_lag_id: Some("ola-00000000000000001"),
            vlan: Some(101),
            local_address: Some("10.0.0.1"),
            peer_address: Some("10.0.0.2"),
            local_bgp_asn: Some(64512),
            peer_bgp_asn: Some(65001),
            peer_bgp_asn_extended: None,
            region: REGION,
            configuration_state: "available",
            tags: vec![tag("Name", "rack-a"), tag("Owner", "TeamA")],
        },
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000001",
            group_id: Some("lgw-vif-grp-00000000000000001"),
            outpost_lag_id: Some("ola-00000000000000001"),
            vlan: Some(102),
            local_address: Some("10.0.0.5"),
            peer_address: Some("10.0.0.6"),
            local_bgp_asn: Some(64512),
            peer_bgp_asn: Some(65002),
            peer_bgp_asn_extended: Some(4_200_000_000),
            region: REGION,
            configuration_state: "pending",
            tags: vec![tag("Name", "rack-a")],
        },
        // Holds no value for most attributes: no group, no LAG, no VLAN, no address and no
        // ASN — so no value of those filters can select it.
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000001",
            group_id: None,
            outpost_lag_id: None,
            vlan: None,
            local_address: None,
            peer_address: None,
            local_bgp_asn: None,
            peer_bgp_asn: None,
            peer_bgp_asn_extended: None,
            region: REGION,
            configuration_state: "deleting",
            tags: Vec::new(),
        },
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000002",
            group_id: Some("lgw-vif-grp-00000000000000002"),
            outpost_lag_id: Some("ola-00000000000000002"),
            vlan: Some(201),
            local_address: Some("10.1.0.1"),
            peer_address: Some("10.1.0.2"),
            local_bgp_asn: Some(64513),
            peer_bgp_asn: Some(65003),
            peer_bgp_asn_extended: None,
            region: OTHER_REGION,
            configuration_state: "available",
            tags: vec![tag("Name", "rack-b")],
        },
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000003",
            group_id: Some("lgw-vif-grp-00000000000000003"),
            outpost_lag_id: None,
            vlan: Some(301),
            local_address: Some("10.2.0.1"),
            peer_address: Some("10.2.0.2"),
            local_bgp_asn: Some(64514),
            // The same peer ASN as the first row, so that filter selects two rows.
            peer_bgp_asn: Some(65001),
            peer_bgp_asn_extended: None,
            region: REGION,
            configuration_state: "deleted",
            tags: vec![tag("Env", "test")],
        },
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000003",
            group_id: Some("lgw-vif-grp-00000000000000003"),
            outpost_lag_id: Some("ola-00000000000000003"),
            // The same VLAN as the first row, so that filter selects two rows.
            vlan: Some(101),
            local_address: Some("10.2.0.5"),
            peer_address: Some("10.2.0.6"),
            local_bgp_asn: Some(64514),
            peer_bgp_asn: Some(65004),
            peer_bgp_asn_extended: None,
            region: REGION,
            configuration_state: "available",
            tags: Vec::new(),
        },
        InterfaceRow {
            local_gateway_id: "lgw-00000000000000004",
            group_id: Some("lgw-vif-grp-00000000000000004"),
            outpost_lag_id: Some("ola-00000000000000004"),
            vlan: Some(401),
            local_address: Some("10.3.0.1"),
            peer_address: Some("10.3.0.2"),
            local_bgp_asn: None,
            peer_bgp_asn: None,
            peer_bgp_asn_extended: Some(4_200_000_001),
            region: REGION,
            configuration_state: "pending",
            tags: vec![tag("Name", "rack-c")],
        },
    ]
}

/// The spec one seeded row is written through.
fn spec_of(row: &InterfaceRow) -> InterfaceSpec {
    InterfaceSpec {
        local_gateway_id: row.local_gateway_id.to_string(),
        local_gateway_virtual_interface_group_id: row.group_id.map(str::to_string),
        outpost_lag_id: row.outpost_lag_id.map(str::to_string),
        vlan: row.vlan,
        local_address: row.local_address.map(str::to_string),
        peer_address: row.peer_address.map(str::to_string),
        local_bgp_asn: row.local_bgp_asn,
        peer_bgp_asn: row.peer_bgp_asn,
        peer_bgp_asn_extended: row.peer_bgp_asn_extended,
        region: row.region.to_string(),
        configuration_state: row.configuration_state.to_string(),
        tags: row.tags.clone(),
    }
}

/// Write the seeded virtual interfaces into the engine's store, the way the operation that
/// creates a local gateway virtual interface would, and report the ids they were given in
/// store order.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut ids = Vec::new();
    for row in seeded_rows() {
        let case = row.configuration_state;
        ids.push(
            guard
                .insert_local_gateway_virtual_interface(spec_of(&row))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the store must take the seeded virtual interface")
                }),
        );
    }
    ids
}

/// The ids at `indices`, in store order.
fn ids_at(seeded: &[String], indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded[*index].clone()).collect()
}

/// The ARN of a local gateway virtual interface, as
/// `LocalGatewayVirtualInterface$LocalGatewayVirtualInterfaceArn` carries it:
/// `arn:aws:ec2:<region>:<account>:local-gateway-virtual-interface/<id>`.
fn interface_arn(region: &str, interface_id: &str) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:local-gateway-virtual-interface/{interface_id}",
        engine::emulator_account_id()
    )
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// Every state the seeded rows pin is a value the model publishes, so the table above is
/// written against the modelled enum rather than against prose.
#[test]
fn every_seeded_state_is_a_modelled_configuration_state() {
    for row in seeded_rows() {
        let state = row.configuration_state;
        assert!(
            ec2::ops::types::LOCAL_GATEWAY_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES
                .contains(&state),
            "case {state:?}: expected a value of \
             LocalGatewayVirtualInterfaceConfigurationState, got {state:?}"
        );
    }
}

/// An account with no Outpost holds no virtual interface — a local gateway and the
/// interfaces on it come with an Outpost rather than from any EC2 call — so the operation
/// answers HTTP 200 with an empty set, and describing is a READ: two calls report the same
/// thing and the store is untouched.
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
        "fresh account: expected no virtual interface, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "fresh account: expected no pagination token, got {body}"
    );

    let again = described_ids(&router, "a second call", &[]).await;
    assert!(
        again.is_empty(),
        "a second call: expected no virtual interface, got {again:?}"
    );
    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .local_gateway_virtual_interfaces
            .len(),
        0,
        "describing virtual interfaces must not write to the store"
    );
}

/// Every virtual interface the store holds is reported, in store order, with every member of
/// the modelled shape — and one written after a call shows up in the next one, so the answer
/// is computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_interface_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "all interfaces: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert_eq!(
        reported, seeded,
        "all interfaces: expected every stored virtual interface in store order, got \
         {reported:?}"
    );

    // Every member of the reported shape comes from the stored virtual interface: the local
    // gateway and group, the ARN built from the Region its writer was signed for, the owning
    // account, the Outpost LAG, the VLAN, the addresses, the ASNs, the configuration state
    // and the tags.
    for (index, row) in seeded_rows().iter().enumerate() {
        let case = format!("row {index}");
        assert!(
            ec2::state::local_gateway_virtual_interface_id_is_well_formed(&seeded[index]),
            "case {case:?}: expected a well-formed virtual interface id, got {:?}",
            seeded[index]
        );
        let mut expected: Vec<(&str, String)> = vec![
            ("localGatewayId", row.local_gateway_id.to_string()),
            (ID_ELEMENT, seeded[index].clone()),
            (
                "localGatewayVirtualInterfaceArn",
                interface_arn(row.region, &seeded[index]),
            ),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("configurationState", row.configuration_state.to_string()),
        ];
        for (member, value) in [
            ("localGatewayVirtualInterfaceGroupId", row.group_id),
            ("outpostLagId", row.outpost_lag_id),
            ("localAddress", row.local_address),
            ("peerAddress", row.peer_address),
        ] {
            if let Some(value) = value {
                expected.push((member, value.to_string()));
            }
        }
        for (member, value) in [
            ("vlan", row.vlan),
            ("localBgpAsn", row.local_bgp_asn),
            ("peerBgpAsn", row.peer_bgp_asn),
        ] {
            if let Some(value) = value {
                expected.push((member, value.to_string()));
            }
        }
        if let Some(asn) = row.peer_bgp_asn_extended {
            expected.push(("peerBgpAsnExtended", asn.to_string()));
        }
        for (member, value) in expected {
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
        .insert_local_gateway_virtual_interface(InterfaceSpec {
            local_gateway_id: "lgw-00000000000000009".to_string(),
            local_gateway_virtual_interface_group_id: Some(
                "lgw-vif-grp-00000000000000009".to_string(),
            ),
            outpost_lag_id: Some("ola-00000000000000009".to_string()),
            vlan: Some(909),
            local_address: Some("10.9.0.1".to_string()),
            peer_address: Some("10.9.0.2".to_string()),
            local_bgp_asn: Some(65010),
            peer_bgp_asn: Some(65011),
            peer_bgp_asn_extended: None,
            region: REGION.to_string(),
            configuration_state: "available".to_string(),
            tags: vec![tag("Name", "late")],
        })
        .expect("after a later write: the store must take the virtual interface");
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
async fn each_documented_filter_selects_the_interfaces_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.len()).collect();

    // One row per filter name `DescribeLocalGatewayVirtualInterfacesRequest$Filters`
    // documents, each asserted on a virtual interface the issue never cited, plus rows for
    // several values, a value nothing holds, the wrong case, and the row that holds no value
    // for the attribute at all.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        ("local-address", vec!["10.0.0.1".to_string()], vec![0]),
        (
            "local-address",
            vec!["10.1.0.1".to_string(), "10.2.0.5".to_string()],
            vec![3, 5],
        ),
        ("local-address", vec!["10.9.9.9".to_string()], Vec::new()),
        ("local-bgp-asn", vec!["64514".to_string()], vec![4, 5]),
        ("local-bgp-asn", vec!["64512".to_string()], vec![0, 1]),
        ("local-bgp-asn", vec!["99999".to_string()], Vec::new()),
        (
            "local-gateway-id",
            vec!["lgw-00000000000000001".to_string()],
            vec![0, 1, 2],
        ),
        (
            "local-gateway-id",
            vec![
                "lgw-00000000000000003".to_string(),
                "lgw-00000000000000004".to_string(),
            ],
            vec![4, 5, 6],
        ),
        (
            "local-gateway-id",
            vec!["lgw-00000000000000099".to_string()],
            Vec::new(),
        ),
        (
            "local-gateway-virtual-interface-id",
            vec![seeded[3].clone()],
            vec![3],
        ),
        (
            "local-gateway-virtual-interface-id",
            vec![seeded[6].clone(), seeded[1].clone()],
            vec![1, 6],
        ),
        (
            "local-gateway-virtual-interface-id",
            vec![seeded[0].to_uppercase()],
            Vec::new(),
        ),
        ("owner-id", vec![account.clone()], all.clone()),
        // An account id the emulator does not use (its own is
        // `engine::emulator_account_id()`), so no virtual interface is owned by it.
        ("owner-id", vec!["123456789012".to_string()], Vec::new()),
        ("peer-address", vec!["10.0.0.6".to_string()], vec![1]),
        (
            "peer-address",
            vec!["10.2.0.2".to_string(), "10.3.0.2".to_string()],
            vec![4, 6],
        ),
        ("peer-bgp-asn", vec!["65001".to_string()], vec![0, 4]),
        ("peer-bgp-asn", vec!["65004".to_string()], vec![5]),
        ("vlan", vec!["101".to_string()], vec![0, 5]),
        ("vlan", vec!["102".to_string()], vec![1]),
        // The interface with no VLAN holds no value for this attribute, so no value can
        // select it.
        ("vlan", vec!["0".to_string()], Vec::new()),
    ];

    for (name, values, expected_indices) in cases {
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let case = format!("{name}={values:?}");
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the virtual interfaces
/// that must come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the virtual interfaces matching every one
/// are reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let owner: [&str; 1] = [account.as_str()];

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000001"][..]),
                ("vlan", &["102"][..]),
            ],
            vec![1],
        ),
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000001"][..]),
                ("vlan", &["201"][..]),
            ],
            Vec::new(),
        ),
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000003"][..]),
                ("owner-id", &owner[..]),
            ],
            vec![4, 5],
        ),
        (
            vec![
                ("local-bgp-asn", &["64514"][..]),
                ("peer-bgp-asn", &["65001", "65004"][..]),
                ("owner-id", &owner[..]),
            ],
            vec![4, 5],
        ),
        (
            vec![
                ("local-gateway-id", &["lgw-00000000000000003"][..]),
                ("peer-address", &["10.2.0.6"][..]),
                ("vlan", &["101"][..]),
            ],
            vec![5],
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

    // Rows: a member this operation's `Filters` doc does NOT name a filter for even though
    // the shape carries it (the virtual interface group, the Outpost LAG, the configuration
    // state), the underscore spelling, a documented name with the wrong case, the plural, a
    // near-miss, the tag filters it does not name either (even though the interface carries
    // tags), and an empty name.
    let cases = [
        "local-gateway-virtual-interface-group-id",
        "outpost-lag-id",
        "configuration-state",
        "local_address",
        "Local-Address",
        "local-addresses",
        "local-gateway-virtual-interface",
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
            "case {name:?}: a refused request reports no virtual interface, got {body}"
        );
    }
}

/// One row of the bounds table: what the case is called, the form fields to send, and the
/// HTTP status the request must answer with.
type BoundsCase = (String, Vec<(String, String)>, u16);

/// The request-controlled factors of the store scan are bounded: each of the crate's caps is
/// accepted at its published value and refused one past it, so no request can ask for
/// unbounded filtering work.
#[tokio::test]
async fn the_filter_and_id_factors_are_accepted_at_their_caps_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: the number of filters, the values in one filter, the bytes in one value and the
    // number of virtual interface ids — each at the crate's cap (`ec2::state`) and one past
    // it.
    let many_filters = |count: usize| -> Vec<(String, String)> {
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| ("local-gateway-id", &["lgw-00000000000000001"][..]))
            .collect();
        filters(&entries)
    };
    let many_values = |count: usize| -> Vec<(String, String)> {
        let values: Vec<String> = (0..count).map(|index| format!("lgw-{index}")).collect();
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        filters(&[("local-gateway-id", &borrowed)])
    };
    let one_value_of = |bytes: usize| -> Vec<(String, String)> {
        let value = "a".repeat(bytes);
        filters(&[("local-gateway-id", &[value.as_str()])])
    };
    let many_ids = |count: usize| -> Vec<(String, String)> {
        let ids: Vec<String> = (0..count)
            .map(|index| format!("lgw-vif-{index:017x}"))
            .collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        indexed("LocalGatewayVirtualInterfaceId", &borrowed)
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
                "case {case:?}: a refused request reports no virtual interface, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// LocalGatewayVirtualInterfaceIds
// ---------------------------------------------------------------------------

/// The ids narrow the report to the virtual interfaces named, in store order whatever order
/// they were asked for; an id that names no virtual interface of the account contributes no
/// row.
#[tokio::test]
async fn the_ids_narrow_the_report_to_the_interfaces_named() {
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
        let case = format!("ids {asked:?}");
        let reported = described_ids(
            &router,
            &case,
            &indexed("LocalGatewayVirtualInterfaceId", &borrowed),
        )
        .await;
        let mut expected = asked.clone();
        expected.sort_unstable();
        assert_eq!(
            reported,
            ids_at(&seeded, &expected),
            "case {case:?}: expected the virtual interfaces named, in store order, got \
             {reported:?}"
        );
    }

    // An id of the right shape that names no virtual interface of the account, alone and
    // beside a stored one.
    let unknown = "lgw-vif-0000000000000beef";
    for (case, asked) in [
        ("an unknown id alone", vec![unknown.to_string()]),
        (
            "an unknown id beside a stored one",
            vec![unknown.to_string(), seeded[1].clone()],
        ),
    ] {
        let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
        let reported = described_ids(
            &router,
            case,
            &indexed("LocalGatewayVirtualInterfaceId", &borrowed),
        )
        .await;
        let expected: Vec<String> = asked
            .iter()
            .filter(|id| id.as_str() != unknown)
            .cloned()
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every virtual interface exactly once, in
/// store order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_interface_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `LocalGatewayMaxResults` is modelled `range 5..1000`, so the rows are its published
    // lower bound, a size that divides the store exactly, one that does not, and the upper
    // bound, which holds everything.
    for max_results in [5, 7, 6, 1000] {
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
                "case {case:?}: expected at most {max_results} virtual interfaces in a page, \
                 got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.len() + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            match element(&body, "nextToken") {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded,
            "case {case:?}: expected every virtual interface exactly once in store order, got \
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

    // Six of the seven seeded virtual interfaces belong to these three local gateways, so
    // the model's minimum page size cuts the filtered set into two pages.
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
        "filtered first page: expected the first five interfaces of those gateways, got \
         {first:?}"
    );
    let token = element(&body, "nextToken")
        .expect("filtered first page: a sixth virtual interface follows, so a token must be issued");

    let mut next = page_params();
    next.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &next).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[5]),
        "filtered second page: expected the remaining interface of those gateways, got \
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

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a cursor.
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
        let (status, body) = describe(&router, &[("NextToken".to_string(), token.clone())]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
        // EC2's message for this refusal is built from the request — it names the token that
        // was refused — so a caller holding several can tell which one is bad.
        assert!(
            unescaped(&body).contains(&format!("Next token '{token}' is invalid")),
            "case {token:?}: the refusal must name the refused token, got {body}"
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
        "DryRun=true: a dry run reports no virtual interface, got {body}"
    );

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded,
        "DryRun=false: expected every stored virtual interface, got {reported:?}"
    );
}
