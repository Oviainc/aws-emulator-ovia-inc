// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeServiceLinkVirtualInterfaces`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! ONE rule, asserted over tables of inputs: the operation reports THE STORE — the account's
//! service link virtual interfaces
//! (`ec2::state::ComAmazonawsEc2Amazonec2State::service_link_virtual_interfaces`) — narrowed
//! by the `ServiceLinkVirtualInterfaceIds` and `Filters` the request names and cut into
//! `MaxResults`-sized pages, writing nothing. Six of the filters
//! `DescribeServiceLinkVirtualInterfacesRequest$Filters` documents select on a fact of the
//! interface itself; the seventh names a LOCAL GATEWAY virtual interface, which an interface is
//! selected on through the Outpost LAG they both run over.
//!
//! The rows differ along which filter, how many values, how many filters, which ids, which
//! members an interface holds a value for, which LAG it runs over (including none at all), and
//! which page size; an account with no Outpost (the empty store) is one row among them rather
//! than the specification.
//!
//! Nothing in the EC2 API this emulator serves racks an Outpost, so the seeded LAGs and
//! interfaces are written straight through the store's own seams — the ones an Outpost
//! provisioning path writes through.

use ec2::ops::types::{SERVICE_LINK_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES, Tag};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, OutpostLagSpec, SERVICE_LINK_VIF_ID_PREFIX,
    ServiceLinkVirtualInterfaceSpec as InterfaceSpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The ec2Query action under test.
const ACTION: &str = "DescribeServiceLinkVirtualInterfaces";

/// The XML element each reported interface's id arrives in
/// (`ServiceLinkVirtualInterface$ServiceLinkVirtualInterfaceId`'s `xmlName`).
const ID_ELEMENT: &str = "serviceLinkVirtualInterfaceId";

/// The Region every call here is signed for. The interface store is per account rather than
/// per Region, so the Region only selects the ec2Query envelope — and the Region a stored
/// interface's ARN names is the one its writer was signed for, which is why one seeded Outpost
/// carries [`OTHER_REGION`].
const REGION: &str = "us-east-1";

/// The Region of the one seeded Outpost written from another Region, so the reported ARNs have
/// two Region namespaces to tell apart.
const OTHER_REGION: &str = "us-west-2";

/// One call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The interface ids a successful response reports, in the order it reported them.
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

/// A tag, as the seeded interfaces carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// What one seeded Outpost LAG holds: the Outpost whose links it aggregates, the Region that
/// Outpost's ARN names, and the local gateway virtual interfaces associated with it — the ids
/// the documented `local-gateway-virtual-interface-id` filter selects an interface over this
/// LAG on.
struct LagRow {
    /// "The Outpost ID" the LAG's own `OutpostArn` names.
    outpost_id: &'static str,
    region: &'static str,
    /// "The IDs of the local gateway virtual interfaces associated with the Outpost LAG"
    /// (`OutpostLag$LocalGatewayVirtualInterfaceIds`).
    local_gateway_virtual_interface_ids: &'static [&'static str],
}

/// The LAGs the interfaces below run over. Two LAGs share an Outpost, so a filter on the
/// Outpost ARN selects interfaces over both; two share a local gateway virtual interface, so
/// that filter does too; and one has no local gateway virtual interface at all, so the
/// interfaces over it hold no value for that filter.
fn lag_rows() -> Vec<LagRow> {
    vec![
        LagRow {
            outpost_id: "op-00000000000000001",
            region: REGION,
            local_gateway_virtual_interface_ids: &[
                "lgw-vif-00000000000000001",
                "lgw-vif-00000000000000002",
            ],
        },
        LagRow {
            outpost_id: "op-00000000000000002",
            region: OTHER_REGION,
            local_gateway_virtual_interface_ids: &["lgw-vif-00000000000000003"],
        },
        LagRow {
            outpost_id: "op-00000000000000003",
            region: REGION,
            local_gateway_virtual_interface_ids: &[],
        },
        LagRow {
            outpost_id: "op-00000000000000001",
            region: REGION,
            local_gateway_virtual_interface_ids: &["lgw-vif-00000000000000001"],
        },
    ]
}

/// What one seeded service link virtual interface holds.
struct InterfaceRow {
    /// The index into [`lag_rows`] of the LAG this interface runs over — `None` for the one
    /// that runs over no LAG, which therefore holds no value for the LAG filter or for the
    /// local gateway virtual interfaces of a LAG.
    lag: Option<usize>,
    /// The Outpost whose ARN the interface reports, and the Region that ARN names. An interface
    /// over a LAG reports that LAG's Outpost; the one over no LAG names its own.
    outpost: Option<(&'static str, &'static str)>,
    vlan: Option<i32>,
    local_address: Option<&'static str>,
    peer_address: Option<&'static str>,
    /// "The ASN for the Border Gateway Protocol (BGP) associated with the service link virtual
    /// interface" — a `Long`, so one row carries a 32-bit ASN past the 16-bit ids.
    peer_bgp_asn: Option<i64>,
    /// "The current state of the service link virtual interface"
    /// (`ServiceLinkVirtualInterface$ConfigurationState`), one of the values
    /// `ServiceLinkVirtualInterfaceConfigurationState` publishes — the generated catalog
    /// [`SERVICE_LINK_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES`], which
    /// [`every_seeded_state_is_a_modelled_configuration_state`] checks these against.
    configuration_state: &'static str,
    tags: Vec<Tag>,
}

/// The interfaces the filter and paging tables are written against. They differ along every
/// attribute the documented filters select on — the LAG, the Outpost ARN, the state, the VLAN
/// and the local gateway virtual interfaces of their LAG — and along every member that is
/// merely reported: the addresses, the peer ASN, the Region the ARN names and the tags. Two
/// share a VLAN, three share a state and two share a LAG, so a filter that selects on one
/// reports more than a single row; one row runs over no LAG and holds no value for most
/// attributes at all.
fn interface_rows() -> Vec<InterfaceRow> {
    vec![
        InterfaceRow {
            lag: Some(0),
            outpost: Some(("op-00000000000000001", REGION)),
            vlan: Some(2001),
            local_address: Some("10.0.0.1"),
            peer_address: Some("10.0.0.2"),
            peer_bgp_asn: Some(65_001),
            configuration_state: "available",
            tags: vec![tag("Name", "rack-a"), tag("Owner", "TeamA")],
        },
        InterfaceRow {
            lag: Some(0),
            outpost: Some(("op-00000000000000001", REGION)),
            vlan: Some(2002),
            local_address: Some("10.0.0.5"),
            peer_address: Some("10.0.0.6"),
            peer_bgp_asn: Some(65_002),
            configuration_state: "pending",
            tags: vec![tag("Name", "rack-a")],
        },
        // Runs over no LAG and holds no value for most attributes: no VLAN, no address and no
        // ASN — so no value of those filters can select it.
        InterfaceRow {
            lag: None,
            outpost: Some(("op-00000000000000004", REGION)),
            vlan: None,
            local_address: None,
            peer_address: None,
            peer_bgp_asn: None,
            configuration_state: "deleting",
            tags: Vec::new(),
        },
        InterfaceRow {
            lag: Some(1),
            outpost: Some(("op-00000000000000002", OTHER_REGION)),
            vlan: Some(2003),
            local_address: Some("10.1.0.1"),
            peer_address: Some("10.1.0.2"),
            // The 32-bit ASN space `PeerBgpAsn`'s `Long` carries.
            peer_bgp_asn: Some(4_200_000_000),
            configuration_state: "available",
            tags: vec![tag("Name", "rack-b")],
        },
        InterfaceRow {
            lag: Some(2),
            outpost: Some(("op-00000000000000003", REGION)),
            // The same VLAN as the first row, so that filter selects two rows.
            vlan: Some(2001),
            local_address: Some("10.2.0.1"),
            peer_address: Some("10.2.0.2"),
            peer_bgp_asn: Some(65_003),
            configuration_state: "deleted",
            tags: vec![tag("Env", "test")],
        },
        InterfaceRow {
            lag: Some(2),
            outpost: Some(("op-00000000000000003", REGION)),
            vlan: Some(2004),
            local_address: Some("10.2.0.5"),
            peer_address: Some("10.2.0.6"),
            peer_bgp_asn: Some(65_004),
            configuration_state: "pending",
            tags: Vec::new(),
        },
        InterfaceRow {
            lag: Some(3),
            outpost: Some(("op-00000000000000001", REGION)),
            vlan: Some(2005),
            local_address: Some("10.3.0.1"),
            peer_address: Some("10.3.0.2"),
            peer_bgp_asn: None,
            configuration_state: "available",
            tags: vec![tag("Name", "rack-c")],
        },
    ]
}

/// The ARN of an Outpost, as `ServiceLinkVirtualInterface$OutpostArn` carries it:
/// `arn:aws:outposts:<region>:<account>:outpost/<outpost-id>`.
fn outpost_arn(region: &str, outpost_id: &str) -> String {
    format!(
        "arn:aws:outposts:{region}:{}:outpost/{outpost_id}",
        engine::emulator_account_id()
    )
}

/// The ARN of a service link virtual interface, as
/// `ServiceLinkVirtualInterface$ServiceLinkVirtualInterfaceArn` carries it:
/// `arn:aws:ec2:<region>:<account>:service-link-virtual-interface/<id>`.
fn interface_arn(region: &str, interface_id: &str) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:service-link-virtual-interface/{interface_id}",
        engine::emulator_account_id()
    )
}

/// What one seeding wrote: the LAG ids and the interface ids, each in store order.
struct Seeded {
    lags: Vec<String>,
    interfaces: Vec<String>,
}

/// Write the seeded LAGs and the interfaces over them into the engine's store — the seams a
/// second-generation Outposts rack is placed through, nothing in the EC2 API this emulator
/// serves racking an Outpost — and report the ids they were given, in store order.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut lags = Vec::new();
    for row in lag_rows() {
        let case = row.outpost_id;
        lags.push(
            guard
                .insert_outpost_lag(OutpostLagSpec {
                    outpost_arn: outpost_arn(row.region, row.outpost_id),
                    state: "available".to_string(),
                    local_gateway_virtual_interface_ids: row
                        .local_gateway_virtual_interface_ids
                        .iter()
                        .map(|id| (*id).to_string())
                        .collect(),
                    // The LAGs start with no service link virtual interface on them, so every
                    // id one reports later can only have been added by a write that put one
                    // there.
                    service_link_virtual_interface_ids: Vec::new(),
                    tags: Vec::new(),
                })
                .unwrap_or_else(|| panic!("case {case:?}: the store must take the seeded LAG")),
        );
    }
    let mut interfaces = Vec::new();
    for row in interface_rows() {
        let case = row.configuration_state;
        interfaces.push(
            guard
                .insert_service_link_virtual_interface(spec_of(&row, &lags))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the store must take the seeded interface")
                }),
        );
    }
    Seeded { lags, interfaces }
}

/// The spec one seeded interface row is written through, against the LAG ids `lags` the seeding
/// above minted.
fn spec_of(row: &InterfaceRow, lags: &[String]) -> InterfaceSpec {
    let (outpost_id, region) = row.outpost.unwrap_or(("", REGION));
    InterfaceSpec {
        outpost_arn: outpost_arn(region, outpost_id),
        outpost_lag_id: row.lag.map(|lag| lags[lag].clone()),
        local_address: row.local_address.map(str::to_string),
        peer_address: row.peer_address.map(str::to_string),
        peer_bgp_asn: row.peer_bgp_asn,
        vlan: row.vlan,
        region: region.to_string(),
        configuration_state: row.configuration_state.to_string(),
        tags: row.tags.clone(),
    }
}

/// The ids at `indices`, in store order.
fn ids_at(seeded: &[String], indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded[*index].clone()).collect()
}

/// How many service link virtual interfaces the store holds.
fn stored_interfaces(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .service_link_virtual_interfaces
        .len()
}

// ---------------------------------------------------------------------------
// The report is the store
// ---------------------------------------------------------------------------

/// Every state the table seeds is one the model publishes, so the rows below are written
/// against the modelled enum rather than against free text.
#[test]
fn every_seeded_state_is_a_modelled_configuration_state() {
    for row in interface_rows() {
        let state = row.configuration_state;
        assert!(
            SERVICE_LINK_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES.contains(&state),
            "case {state:?}: expected one of \
             {SERVICE_LINK_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES:?}"
        );
    }
}

/// An account with no Outpost holds no service link virtual interface — one comes with a
/// second-generation Outposts rack rather than from any EC2 call — so the operation answers
/// HTTP 200 with an empty set, and describing is a READ: two calls report the same thing and
/// the store is untouched.
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
        stored_interfaces(&engine),
        0,
        "describing service link virtual interfaces must not write to the store"
    );
}

/// Every interface the store holds is reported, in store order, with every member of the
/// modelled shape — and one written after a call shows up in the next one, so the answer is
/// computed from the store rather than fixed.
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
        reported, seeded.interfaces,
        "all interfaces: expected every stored virtual interface in store order, got \
         {reported:?}"
    );

    // Every member of the reported shape comes from the stored interface: the id this service
    // minted, the ARN built from the Region its writer was signed for, the owning account, the
    // Outpost ARN and the Outpost id that ARN names, the LAG, the VLAN, the addresses, the peer
    // ASN, the configuration state and the tags.
    for (index, row) in interface_rows().iter().enumerate() {
        let case = format!("row {index}");
        let interface_id = &seeded.interfaces[index];
        assert!(
            ec2::state::service_link_virtual_interface_id_is_well_formed(interface_id),
            "case {case:?}: expected a well-formed virtual interface id, got {interface_id:?}"
        );
        let (outpost_id, region) = row.outpost.expect("every seeded row names an Outpost");
        let mut expected: Vec<(&str, String)> = vec![
            (ID_ELEMENT, interface_id.clone()),
            (
                "serviceLinkVirtualInterfaceArn",
                interface_arn(region, interface_id),
            ),
            ("outpostId", outpost_id.to_string()),
            ("outpostArn", outpost_arn(region, outpost_id)),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("configurationState", row.configuration_state.to_string()),
        ];
        if let Some(lag) = row.lag {
            expected.push(("outpostLagId", seeded.lags[lag].clone()));
        }
        for (member, value) in [
            ("localAddress", row.local_address),
            ("peerAddress", row.peer_address),
        ] {
            if let Some(value) = value {
                expected.push((member, value.to_string()));
            }
        }
        if let Some(vlan) = row.vlan {
            expected.push(("vlan", vlan.to_string()));
        }
        if let Some(asn) = row.peer_bgp_asn {
            expected.push(("peerBgpAsn", asn.to_string()));
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

    // The local gateway virtual interfaces of the LAG an interface runs over are something a
    // filter selects on, not a member of the reported shape, so none is reported.
    assert!(
        !body.contains("localGatewayVirtualInterfaceId"),
        "all interfaces: the modelled shape carries no local gateway virtual interface \
         member, got {body}"
    );

    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_service_link_virtual_interface(InterfaceSpec {
            outpost_arn: outpost_arn(REGION, "op-00000000000000009"),
            outpost_lag_id: Some(seeded.lags[0].clone()),
            local_address: Some("10.9.0.1".to_string()),
            peer_address: Some("10.9.0.2".to_string()),
            peer_bgp_asn: Some(65_009),
            vlan: Some(2009),
            region: REGION.to_string(),
            configuration_state: "available".to_string(),
            tags: vec![tag("Name", "late")],
        })
        .expect("after a later write: the store must take the virtual interface");
    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.interfaces.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );

    // Each minted id carries this resource's own prefix, so an id of this kind is told apart
    // from a sibling Outpost resource's by its prefix alone.
    for id in &after {
        assert!(
            id.starts_with(&format!("{SERVICE_LINK_VIF_ID_PREFIX}-")),
            "expected the id {id:?} to carry the prefix {SERVICE_LINK_VIF_ID_PREFIX:?}"
        );
    }
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
    let all: Vec<usize> = (0..seeded.interfaces.len()).collect();

    // One row per filter name `DescribeServiceLinkVirtualInterfacesRequest$Filters` documents,
    // each asserted on an interface the issue never cited, plus rows for several values, a
    // value nothing holds, the wrong case, and the row that holds no value for the attribute at
    // all.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        // The LAG the interface runs over: one LAG, two LAGs, and an id no LAG of the account
        // has — the interface over no LAG holding no value for it.
        ("outpost-lag-id", vec![seeded.lags[2].clone()], vec![4, 5]),
        (
            "outpost-lag-id",
            vec![seeded.lags[0].clone(), seeded.lags[3].clone()],
            vec![0, 1, 6],
        ),
        (
            "outpost-lag-id",
            vec!["ola-00000000000000099".to_string()],
            Vec::new(),
        ),
        // The Outpost ARN: one Outpost carries two LAGs, so it selects interfaces over both.
        (
            "outpost-arn",
            vec![outpost_arn(REGION, "op-00000000000000001")],
            vec![0, 1, 6],
        ),
        (
            "outpost-arn",
            vec![outpost_arn(OTHER_REGION, "op-00000000000000002")],
            vec![3],
        ),
        (
            "outpost-arn",
            vec![outpost_arn(REGION, "op-00000000000000099")],
            Vec::new(),
        ),
        ("owner-id", vec![account.clone()], all.clone()),
        // An account id the emulator does not use (its own is
        // `engine::emulator_account_id()`), so no interface is owned by it.
        ("owner-id", vec!["123456789012".to_string()], Vec::new()),
        ("state", vec!["available".to_string()], vec![0, 3, 6]),
        // `state` is the described interface's own `ConfigurationState`, not the state of the
        // LAG it runs over: every seeded LAG is `available`, so these three rows would report
        // every interface over a LAG, no interface at all, and nothing, if the filter were read
        // against the LAG instead.
        (
            "state",
            vec!["pending".to_string(), "deleting".to_string()],
            vec![1, 2, 5],
        ),
        ("state", vec!["deleted".to_string()], vec![4]),
        // The comparison is case-sensitive, so the state spelled another way selects nothing.
        ("state", vec!["AVAILABLE".to_string()], Vec::new()),
        ("vlan", vec!["2001".to_string()], vec![0, 4]),
        ("vlan", vec!["2004".to_string()], vec![5]),
        // The interface with no VLAN holds no value for this attribute, so no value can
        // select it.
        ("vlan", vec!["0".to_string()], Vec::new()),
        (
            "service-link-virtual-interface-id",
            vec![seeded.interfaces[3].clone()],
            vec![3],
        ),
        (
            "service-link-virtual-interface-id",
            vec![
                seeded.interfaces[6].clone(),
                seeded.interfaces[1].clone(),
            ],
            vec![1, 6],
        ),
        (
            "service-link-virtual-interface-id",
            vec![seeded.interfaces[0].to_uppercase()],
            Vec::new(),
        ),
        // The local gateway virtual interface: shared with the LAGs the interface runs over, so
        // one id can select interfaces over two LAGs, and the interfaces over the LAG that has
        // none hold no value for it.
        (
            "local-gateway-virtual-interface-id",
            vec!["lgw-vif-00000000000000001".to_string()],
            vec![0, 1, 6],
        ),
        (
            "local-gateway-virtual-interface-id",
            vec!["lgw-vif-00000000000000002".to_string()],
            vec![0, 1],
        ),
        (
            "local-gateway-virtual-interface-id",
            vec![
                "lgw-vif-00000000000000002".to_string(),
                "lgw-vif-00000000000000003".to_string(),
            ],
            vec![0, 1, 3],
        ),
        (
            "local-gateway-virtual-interface-id",
            vec!["lgw-vif-00000000000000099".to_string()],
            Vec::new(),
        ),
    ];

    for (name, values, expected_indices) in cases {
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let case = format!("{name}={values:?}");
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded.interfaces, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
    assert_eq!(
        stored_interfaces(&engine),
        seeded.interfaces.len(),
        "filtering must not write to the store"
    );
}

/// One row of the AND table: the filters to send, and the indices of the interfaces that must
/// come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the interfaces matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let owner: [&str; 1] = [account.as_str()];
    let first_lag: [&str; 1] = [seeded.lags[0].as_str()];
    let third_lag: [&str; 1] = [seeded.lags[2].as_str()];
    let shared_outpost = outpost_arn(REGION, "op-00000000000000001");
    let shared_outpost_values: [&str; 1] = [shared_outpost.as_str()];

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("outpost-lag-id", &first_lag[..]),
                ("state", &["pending"][..]),
            ],
            vec![1],
        ),
        (
            vec![
                ("outpost-lag-id", &first_lag[..]),
                ("vlan", &["2003"][..]),
            ],
            Vec::new(),
        ),
        (
            vec![
                ("outpost-arn", &shared_outpost_values[..]),
                ("owner-id", &owner[..]),
            ],
            vec![0, 1, 6],
        ),
        (
            vec![
                ("state", &["available", "pending"][..]),
                ("vlan", &["2001", "2004"][..]),
                ("owner-id", &owner[..]),
            ],
            vec![0, 5],
        ),
        (
            vec![
                (
                    "local-gateway-virtual-interface-id",
                    &["lgw-vif-00000000000000001"][..],
                ),
                ("state", &["available"][..]),
            ],
            vec![0, 6],
        ),
        (
            vec![
                ("outpost-lag-id", &third_lag[..]),
                ("state", &["deleted"][..]),
            ],
            vec![4],
        ),
    ];

    for (entries, expected_indices) in cases {
        let case = format!("{entries:?}");
        let reported = described_ids(&router, &case, &filters(&entries)).await;
        let expected = ids_at(&seeded.interfaces, &expected_indices);
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

    // Rows: a member this operation's `Filters` doc does NOT name a filter for even though the
    // shape carries it (the Outpost id, the two addresses, the peer ASN), two names the SIBLING
    // `DescribeOutpostLags` documents but this operation does not, the underscore spelling, a
    // documented name with the wrong case, the plural, a near-miss, the tag filters it does not
    // name either (even though the interface carries tags), and an empty name.
    let cases = [
        "outpost-id",
        "local-address",
        "peer-address",
        "peer-bgp-asn",
        "configuration-state",
        "service-link-virtual-interface-arn",
        "outpost_lag_id",
        "Outpost-Lag-Id",
        "outpost-lag-ids",
        "service-link-virtual-interface",
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

/// One row of the bounds table: what the case is called, the form fields to send, and the HTTP
/// status the request must answer with.
type BoundsCase = (String, Vec<(String, String)>, u16);

/// The request-controlled factors of the store scan are bounded: each of the crate's caps is
/// accepted at its published value and refused one past it, so no request can ask for
/// unbounded filtering work.
#[tokio::test]
async fn the_filter_and_id_factors_are_accepted_at_their_caps_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: the number of filters, the values in one filter, the bytes in one value and the
    // number of interface ids — each at the crate's cap (`ec2::state`) and one past it.
    let many_filters = |count: usize| -> Vec<(String, String)> {
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| ("state", &["available"][..]))
            .collect();
        filters(&entries)
    };
    let many_values = |count: usize| -> Vec<(String, String)> {
        let values: Vec<String> = (0..count).map(|index| format!("ola-{index}")).collect();
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        filters(&[("outpost-lag-id", &borrowed)])
    };
    let one_value_of = |bytes: usize| -> Vec<(String, String)> {
        let value = "a".repeat(bytes);
        filters(&[("outpost-lag-id", &[value.as_str()])])
    };
    let many_ids = |count: usize| -> Vec<(String, String)> {
        let ids: Vec<String> = (0..count)
            .map(|index| format!("{SERVICE_LINK_VIF_ID_PREFIX}-{index:017x}"))
            .collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        indexed("ServiceLinkVirtualInterfaceId", &borrowed)
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
// ServiceLinkVirtualInterfaceIds
// ---------------------------------------------------------------------------

/// The ids narrow the report to the interfaces named, in store order whatever order they were
/// asked for; an id that names no interface of the account contributes no row.
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
        (0..seeded.interfaces.len()).collect(),
    ];
    for asked in cases {
        let ids = ids_at(&seeded.interfaces, &asked);
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("ids {asked:?}");
        let reported = described_ids(
            &router,
            &case,
            &indexed("ServiceLinkVirtualInterfaceId", &borrowed),
        )
        .await;
        let mut expected = asked.clone();
        expected.sort_unstable();
        assert_eq!(
            reported,
            ids_at(&seeded.interfaces, &expected),
            "case {case:?}: expected the interfaces named, in store order, got {reported:?}"
        );
    }

    // An id of the right shape that names no interface of the account, alone and beside a
    // stored one.
    let unknown = format!("{SERVICE_LINK_VIF_ID_PREFIX}-0000000000000beef");
    for (case, asked) in [
        ("an unknown id alone", vec![unknown.clone()]),
        (
            "an unknown id beside a stored one",
            vec![unknown.clone(), seeded.interfaces[1].clone()],
        ),
    ] {
        let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
        let reported = described_ids(
            &router,
            case,
            &indexed("ServiceLinkVirtualInterfaceId", &borrowed),
        )
        .await;
        let expected: Vec<String> = asked.iter().filter(|id| *id != &unknown).cloned().collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every interface exactly once, in store
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_interface_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `ServiceLinkMaxResults` is modelled `range 5..1000`, so the rows are its published lower
    // bound, a size that divides the store exactly, one that does not, and the upper bound,
    // which holds everything.
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
                "case {case:?}: expected at most {max_results} interfaces in a page, got \
                 {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.interfaces.len() + 1,
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
            walked, seeded.interfaces,
            "case {case:?}: expected every interface exactly once in store order, got {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Six of the seven seeded interfaces are in one of these three states (the seventh is
    // `deleting`), so the model's minimum page size cuts the filtered set into two pages.
    let selected_states: &[&str] = &["available", "pending", "deleted"];
    let page_params = || {
        let mut params = filters(&[("state", selected_states)]);
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
        ids_at(&seeded.interfaces, &[0, 1, 3, 4, 5]),
        "filtered first page: expected the first five interfaces in those states, got {first:?}"
    );
    let token = element(&body, "nextToken")
        .expect("filtered first page: a sixth interface follows, so a token must be issued");

    let mut next = page_params();
    next.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &next).await;
    assert_eq!(
        second,
        ids_at(&seeded.interfaces, &[6]),
        "filtered second page: expected the remaining interface in those states, got {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`ServiceLinkMaxResults`, `range 5..1000`) and one past each.
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

    // Rows: free text, punctuation, a hex string this service never minted, the bare resource
    // id a caller might guess the token is, and that id hex-encoded (the shape the token
    // takes, without the marker and checksum a minted token carries).
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.interfaces[0].clone(),
        ::hex::encode(&seeded.interfaces[0]),
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
        assert!(
            elements(&body, ID_ELEMENT).is_empty(),
            "case {token:?}: a refused request reports no virtual interface, got {body}"
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
        reported, seeded.interfaces,
        "DryRun=false: expected every stored interface, got {reported:?}"
    );
    assert_eq!(
        stored_interfaces(&engine),
        seeded.interfaces.len(),
        "a dry run must not write to the store"
    );
}
