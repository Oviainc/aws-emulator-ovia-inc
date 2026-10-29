// SPDX-License-Identifier: CC0-1.0

//! The READER half of the rule `crate::empty_optional_reference` pins on the writers,
//! driven end to end over HTTP: a listing that SELECTS on an OPTIONAL REFERENCE member
//! reads the empty string as the ABSENCE of that reference, so it narrows nothing and the
//! listing answers exactly what omitting the member answers.
//!
//! Why the empty string reaches a handler at all: each member below is modelled over
//! `^[\s\S]*$` with a minimum length of 0 (`smithy_query query="operation"
//! operation="GetDevices,GetLinks,GetConnections,GetLinkAssociations,GetNetworkResources"`:
//! `SiteId` / `DeviceId` / `LinkId` len 0..50, `ResourceArn` len 0..1500), so it satisfies
//! the constraint walk. A member modelled with an id pattern (`CoreNetworkId`,
//! `AttachmentId`) or a non-zero minimum (`AwsRegion` len 1..63, `AccountId` len 12..12) is
//! refused by that walk before any handler runs, which is why no such member is a row here.
//!
//! The table spans the class the rule generalises over: five listings, each with the
//! reference members its query string carries, and for every one of those members three
//! value classes — the empty string (which names no resource), an id or ARN of the right
//! shape nothing is stored under (which still names a resource, so it selects nothing), and
//! a value an entry carries (which still narrows the listing to the entries carrying it) —
//! plus the combinations: a narrowing filter beside the other members spelled empty, and
//! every member of one request spelled empty at once.
//!
//! Only the telemetry rows SEED the store (`crate::network_telemetry` explains why: the
//! operations that register a connection through a GATEWAY are still scaffold stubs); every
//! other resource here is created through its own operation and read back over the wire.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_networkmanager_networkmanager::state::{
    self, TelemetryResourceRecord, CONNECT_PEER_TYPE, SETTLE_DELAY, VPN_CONNECTION_TYPE,
};

use crate::connection_lifecycle::create_connection_ok;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::link_association_lifecycle::{associate_ok, get_associations, pairs_of};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::resource_tagging::escape;

/// An id of the shape this service mints that nothing is stored under, so a filter naming
/// it is a reference to nothing rather than the absence of a reference.
const UNSTORED_SITE: &str = "site-00000000000000099";
/// See [`UNSTORED_SITE`].
const UNSTORED_DEVICE: &str = "device-00000000000000099";
/// See [`UNSTORED_SITE`].
const UNSTORED_LINK: &str = "link-00000000000000099";
/// See [`UNSTORED_SITE`]: an ARN no resource of the store carries.
const UNSTORED_RESOURCE_ARN: &str =
    "arn:aws:ec2:us-west-2:123456789012:vpn-connection/vpn-00000000000000099";
/// See [`UNSTORED_SITE`]: the ARN of a gateway nothing was registered through.
const UNSTORED_GATEWAY_ARN: &str =
    "arn:aws:ec2:us-west-2:123456789012:transit-gateway/tgw-00000000000000099";

/// The account the seeded telemetry rows' Amazon VPC resources belong to.
const FOREIGN_ACCOUNT: &str = "123456789012";
/// The Region those resources sit in.
const TELEMETRY_REGION: &str = "us-west-2";
/// The transit gateway the two seeded VPN connections were registered through, so a
/// `registeredGatewayArn` naming it selects those two and not the Cloud WAN row.
const SEEDED_GATEWAY_ARN: &str =
    "arn:aws:ec2:us-west-2:123456789012:transit-gateway/tgw-0123456789abcdef0";
/// The core network the seeded Cloud WAN row belongs to; its id satisfies the modelled
/// `CoreNetworkId` pattern `^core-network-([0-9a-f]{8,17})$` (`smithy_query
/// shape=CoreNetworkId`).
const SEEDED_CORE_NETWORK: &str = "core-network-0123456789abcdef0";

/// One optional reference member a listing selects on.
struct Reference {
    /// The query parameter the member is bound to.
    query: &'static str,
    /// The response members an entry carries that reference in. A selector matching an
    /// entry at EITHER end names more than one (a connection names a device at both ends; a
    /// relationship is selected through either of the two resources it joins).
    reported: &'static [&'static str],
    /// A value of the right shape nothing is stored under.
    unstored: &'static str,
}

/// One listing under test.
struct Listing {
    /// Names the listing in every assertion message.
    case: &'static str,
    /// The URI it is addressed at, with no query string.
    base: String,
    /// The response member its page sits in.
    list: &'static str,
    /// The members an entry is identified by in an assertion.
    key: &'static [&'static str],
    /// The optional reference members its query string narrows it by.
    references: &'static [Reference],
}

/// `GetDevices` selects a device by the site it sits at.
const DEVICE_REFERENCES: &[Reference] = &[Reference {
    query: "siteId",
    reported: &["SiteId"],
    unstored: UNSTORED_SITE,
}];

/// `GetLinks` selects a link by the site it belongs to.
const LINK_REFERENCES: &[Reference] = &[Reference {
    query: "siteId",
    reported: &["SiteId"],
    unstored: UNSTORED_SITE,
}];

/// `GetConnections` selects a connection by a device at either of its ends.
const CONNECTION_REFERENCES: &[Reference] = &[Reference {
    query: "deviceId",
    reported: &["DeviceId", "ConnectedDeviceId"],
    unstored: UNSTORED_DEVICE,
}];

/// `GetNetworkResources` and `GetNetworkTelemetry` select an entry by the resource it
/// reports and by the gateway that resource was registered through.
const INVENTORY_REFERENCES: &[Reference] = &[
    Reference {
        query: "resourceArn",
        reported: &["ResourceArn"],
        unstored: UNSTORED_RESOURCE_ARN,
    },
    Reference {
        query: "registeredGatewayArn",
        reported: &["RegisteredGatewayArn"],
        unstored: UNSTORED_GATEWAY_ARN,
    },
];

/// `GetNetworkResourceRelationships` selects a tie through either resource it joins.
const RELATIONSHIP_REFERENCES: &[Reference] = &[
    Reference {
        query: "resourceArn",
        reported: &["From", "To"],
        unstored: UNSTORED_RESOURCE_ARN,
    },
    Reference {
        query: "registeredGatewayArn",
        reported: &["RegisteredGatewayArn"],
        unstored: UNSTORED_GATEWAY_ARN,
    },
];

/// What the listings below read: a global network holding sites, devices (sited and
/// unsited), links, connections and link associations, and a second one holding the seeded
/// telemetry registrations.
struct Fixture {
    network: String,
    telemetry_network: String,
    /// The `(device, link)` pairs the fixture tied together, in the order it made them.
    associations: Vec<(String, String)>,
}

/// Build the fixture. No two connections share a device and no two associations share one,
/// so a filter naming a stored resource always selects FEWER entries than the unfiltered
/// listing — which is what makes the narrowing assertions below bite.
async fn seed(emulator: &Emulator, engine: &engine::InMemoryEngine) -> Fixture {
    let network = emulator.create_global_network("empty reference filters").await;
    let hq = emulator.create_site(&network, "hq").await;
    let branch = emulator.create_site(&network, "branch").await;

    let at_hq = create_device_ok(emulator, &network, json!({ "SiteId": hq.clone() })).await;
    let at_branch =
        create_device_ok(emulator, &network, json!({ "SiteId": branch.clone() })).await;
    let spare = create_device_ok(emulator, &network, json!({ "SiteId": branch.clone() })).await;
    // A device that was created without a site: the record the writers hold as none.
    let unsited = create_device_ok(emulator, &network, json!({})).await;

    let hq_link = create_link_ok(emulator, &network, &hq, &plain_link()).await;
    let branch_link = create_link_ok(emulator, &network, &branch, &plain_link()).await;

    create_connection_ok(
        emulator,
        &network,
        json!({ "DeviceId": at_hq.clone(), "ConnectedDeviceId": at_branch.clone() }),
    )
    .await;
    create_connection_ok(
        emulator,
        &network,
        json!({ "DeviceId": spare.clone(), "ConnectedDeviceId": unsited.clone() }),
    )
    .await;

    associate_ok(emulator, &network, &at_hq, &hq_link).await;
    associate_ok(emulator, &network, &at_branch, &branch_link).await;
    let associations = vec![
        (at_hq.clone(), hq_link.clone()),
        (at_branch.clone(), branch_link.clone()),
    ];

    let telemetry_network = emulator.create_global_network("registered connections").await;
    let registered_at = SystemTime::now() - PAST_SETTLE;
    let rows = vec![
        TelemetryResourceRecord {
            global_network_id: telemetry_network.clone(),
            resource_type: CONNECT_PEER_TYPE.to_string(),
            resource_arn: format!(
                "arn:aws:networkmanager::{}:connect-peer/connect-peer-0123456789abcdef0",
                engine::emulator_account_id()
            ),
            aws_region: TELEMETRY_REGION.to_string(),
            core_network_id: Some(SEEDED_CORE_NETWORK.to_string()),
            registered_gateway_arn: None,
            address: None,
            registered_at,
            connected_at: registered_at + SETTLE_DELAY,
        },
        TelemetryResourceRecord {
            global_network_id: telemetry_network.clone(),
            resource_type: VPN_CONNECTION_TYPE.to_string(),
            resource_arn: format!(
                "arn:aws:ec2:{TELEMETRY_REGION}:{FOREIGN_ACCOUNT}:vpn-connection/vpn-0123456789abcdef0"
            ),
            aws_region: TELEMETRY_REGION.to_string(),
            core_network_id: None,
            registered_gateway_arn: Some(SEEDED_GATEWAY_ARN.to_string()),
            address: None,
            registered_at,
            connected_at: registered_at + SETTLE_DELAY,
        },
        TelemetryResourceRecord {
            global_network_id: telemetry_network.clone(),
            resource_type: VPN_CONNECTION_TYPE.to_string(),
            resource_arn: format!(
                "arn:aws:ec2:{TELEMETRY_REGION}:{FOREIGN_ACCOUNT}:vpn-connection/vpn-0123456789abcdef1"
            ),
            aws_region: TELEMETRY_REGION.to_string(),
            core_network_id: None,
            registered_gateway_arn: Some(SEEDED_GATEWAY_ARN.to_string()),
            address: None,
            registered_at,
            connected_at: registered_at + SETTLE_DELAY,
        },
    ];
    let store = state::service_state(engine).expect("the networkmanager store must be reachable");
    let mut guard = store.lock();
    for row in rows {
        guard.register_telemetry_resource(row);
    }
    drop(guard);

    Fixture {
        network,
        telemetry_network,
        associations,
    }
}

/// The listings the fixture feeds, one per reporting operation whose query string carries an
/// optional reference member the modelled shape lets arrive empty.
fn listings(fixture: &Fixture) -> Vec<Listing> {
    let network = &fixture.network;
    vec![
        Listing {
            case: "GetDevices",
            base: format!("/global-networks/{network}/devices"),
            list: "Devices",
            key: &["DeviceId"],
            references: DEVICE_REFERENCES,
        },
        Listing {
            case: "GetLinks",
            base: format!("/global-networks/{network}/links"),
            list: "Links",
            key: &["LinkId"],
            references: LINK_REFERENCES,
        },
        Listing {
            case: "GetConnections",
            base: format!("/global-networks/{network}/connections"),
            list: "Connections",
            key: &["ConnectionId"],
            references: CONNECTION_REFERENCES,
        },
        Listing {
            case: "GetNetworkResources",
            base: format!("/global-networks/{network}/network-resources"),
            list: "NetworkResources",
            key: &["ResourceType", "ResourceId"],
            references: INVENTORY_REFERENCES,
        },
        Listing {
            case: "GetNetworkResourceRelationships",
            base: format!("/global-networks/{network}/network-resource-relationships"),
            list: "Relationships",
            key: &["From", "To"],
            references: RELATIONSHIP_REFERENCES,
        },
        Listing {
            case: "GetNetworkTelemetry",
            base: format!(
                "/global-networks/{}/network-telemetry",
                fixture.telemetry_network
            ),
            list: "NetworkTelemetry",
            key: &["ResourceArn"],
            references: INVENTORY_REFERENCES,
        },
    ]
}

/// One page of `listing` for `query`, asserted to be served whole.
async fn fetch(emulator: &Emulator, listing: &Listing, case: &str, query: &str) -> Vec<Value> {
    let uri = if query.is_empty() {
        listing.base.clone()
    } else {
        format!("{}?{query}", listing.base)
    };
    let (status, body) = emulator.get(&uri).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GET {uri} expected 200, got {status} with {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case {case:?}: GET {uri} must answer every entry on one page, got {body}"
    );
    body[listing.list]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no {} array in {body}", listing.list))
        .clone()
}

/// The key one entry is identified by: the listing's key members, in order.
fn key_of(listing: &Listing, entry: &Value) -> String {
    listing
        .key
        .iter()
        .map(|member| entry[*member].as_str().unwrap_or("<absent>").to_string())
        .collect::<Vec<String>>()
        .join("/")
}

/// The keys of `entries`, in the order the listing reported them.
fn keys_of(listing: &Listing, entries: &[Value]) -> Vec<String> {
    entries.iter().map(|entry| key_of(listing, entry)).collect()
}

/// Whether `entry` carries `value` in any member `reference` is reported in.
fn carries(entry: &Value, reference: &Reference, value: &str) -> bool {
    reference
        .reported
        .iter()
        .any(|member| entry[*member].as_str() == Some(value))
}

/// A value `entries` actually carry for `reference`, or `None` when no entry of this
/// listing carries that reference at all.
fn stored_value(entries: &[Value], reference: &Reference) -> Option<String> {
    entries.iter().find_map(|entry| {
        reference
            .reported
            .iter()
            .find_map(|member| entry[*member].as_str().map(str::to_string))
    })
}

#[tokio::test]
async fn an_empty_reference_filter_names_no_resource_and_narrows_no_listing() {
    let (emulator, engine) = Emulator::with_engine();
    let fixture = seed(&emulator, &engine).await;
    // Past the settle span, so every resource has reached its terminal state and two reads
    // of one listing report the same entries.
    tokio::time::sleep(PAST_SETTLE).await;

    for listing in listings(&fixture) {
        let case = listing.case;
        let unfiltered = fetch(&emulator, &listing, case, "").await;
        let expected_all = keys_of(&listing, &unfiltered);
        assert!(
            expected_all.len() > 1,
            "case {case:?}: the fixture must hold more than one entry for a filter to be \
             able to narrow, got {expected_all:?}"
        );

        for reference in listing.references {
            let query = reference.query;

            // The rule: the empty string names no resource, so it is the absence of the
            // filter and the listing answers what omitting the member answers.
            let empty_case = format!("{case} with an empty {query}");
            let empty = keys_of(
                &listing,
                &fetch(&emulator, &listing, &empty_case, &format!("{query}=")).await,
            );
            assert_eq!(
                empty, expected_all,
                "case {empty_case:?}: expected the unfiltered listing {expected_all:?}, got {empty:?}"
            );

            // A NON-empty value is still a reference: one nothing is stored under selects
            // nothing rather than everything.
            let unstored_case = format!("{case} with a {query} nothing is stored under");
            let unstored = keys_of(
                &listing,
                &fetch(
                    &emulator,
                    &listing,
                    &unstored_case,
                    &format!("{query}={}", escape(reference.unstored)),
                )
                .await,
            );
            assert!(
                unstored.is_empty(),
                "case {unstored_case:?}: expected no entry, got {unstored:?}"
            );

            // A value an entry carries still narrows the listing to the entries carrying
            // it. A reference no entry of this listing carries has no such row.
            let Some(value) = stored_value(&unfiltered, reference) else {
                continue;
            };
            let expected_some: Vec<String> = unfiltered
                .iter()
                .filter(|entry| carries(entry, reference, &value))
                .map(|entry| key_of(&listing, entry))
                .collect();
            assert!(
                !expected_some.is_empty() && expected_some.len() < expected_all.len(),
                "case {case:?} with {query}={value:?}: the fixture must make this filter \
                 select some but not all of {expected_all:?}, got {expected_some:?}"
            );
            let narrowed_case = format!("{case} with a {query} naming a stored resource");
            let narrowed = keys_of(
                &listing,
                &fetch(
                    &emulator,
                    &listing,
                    &narrowed_case,
                    &format!("{query}={}", escape(&value)),
                )
                .await,
            );
            assert_eq!(
                narrowed, expected_some,
                "case {narrowed_case:?}: expected {expected_some:?}, got {narrowed:?}"
            );

            // The empty spelling adds no narrowing to a filter that does narrow.
            let mut combined = format!("{query}={}", escape(&value));
            for other in listing
                .references
                .iter()
                .filter(|other| other.query != query)
            {
                combined.push_str(&format!("&{}=", other.query));
            }
            let combined_case =
                format!("{case} with a {query} naming a stored resource and every other \
                         reference member spelled empty");
            let with_empties =
                keys_of(&listing, &fetch(&emulator, &listing, &combined_case, &combined).await);
            assert_eq!(
                with_empties, expected_some,
                "case {combined_case:?}: expected {expected_some:?}, got {with_empties:?}"
            );
        }

        // Every reference member of one request spelled empty is still the unfiltered
        // listing.
        let all_empty: String = listing
            .references
            .iter()
            .map(|reference| format!("{}=", reference.query))
            .collect::<Vec<String>>()
            .join("&");
        let all_case = format!("{case} with every reference member spelled empty");
        let all = keys_of(
            &listing,
            &fetch(&emulator, &listing, &all_case, &all_empty).await,
        );
        assert_eq!(
            all, expected_all,
            "case {all_case:?}: expected the unfiltered listing {expected_all:?}, got {all:?}"
        );
    }
}

/// One row of the `GetLinkAssociations` table: the case, the query string the request
/// carries, and the device/link pairs the answer must report for it.
type SelectorRow = (&'static str, String, Vec<(String, String)>);

#[tokio::test]
async fn an_empty_selector_of_get_link_associations_is_the_absence_of_that_selector() {
    let (emulator, engine) = Emulator::with_engine();
    let fixture = seed(&emulator, &engine).await;
    let network = fixture.network.clone();
    let (device, link) = fixture.associations[0].clone();
    let (other_device, other_link) = fixture.associations[1].clone();

    // "Gets the link associations for a device or a link. Either the device ID or the link
    // ID must be specified." (`smithy_query query="doc" shape="GetLinkAssociations"`) — the
    // empty string names no resource, so a request whose only selectors are empty specifies
    // NEITHER and is refused exactly as the request carrying no selector at all is. The rows
    // span the spellings of "neither": no member, each one alone, and both.
    let refused: Vec<(&str, String)> = vec![
        ("no selector at all", String::new()),
        ("an empty deviceId", "deviceId=".to_string()),
        ("an empty linkId", "linkId=".to_string()),
        ("both selectors spelled empty", "deviceId=&linkId=".to_string()),
    ];
    for (case, query) in refused {
        let (status, body) = get_associations(&emulator, &network, &query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request naming neither a device nor a link must be refused, \
             got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some("either DeviceId or LinkId must be specified"),
            "case {case:?}: expected the modelled refusal, got {body}"
        );
    }

    // A selector that DOES name a resource is answered, and the OTHER one spelled empty
    // narrows nothing: the two spellings report the same pairs. The rows differ along which
    // selector names a resource, whether the other is empty or absent, whether the value
    // names anything stored, and whether the two name the same pair.
    let rows: Vec<SelectorRow> = vec![
        (
            "a stored deviceId",
            format!("deviceId={device}"),
            vec![(device.clone(), link.clone())],
        ),
        (
            "a stored deviceId beside an empty linkId",
            format!("deviceId={device}&linkId="),
            vec![(device.clone(), link.clone())],
        ),
        (
            "a stored linkId",
            format!("linkId={other_link}"),
            vec![(other_device.clone(), other_link.clone())],
        ),
        (
            "a stored linkId beside an empty deviceId",
            format!("deviceId=&linkId={other_link}"),
            vec![(other_device.clone(), other_link.clone())],
        ),
        (
            "both selectors naming the one associated pair",
            format!("deviceId={device}&linkId={link}"),
            vec![(device.clone(), link.clone())],
        ),
        (
            "a stored deviceId and the other pair's linkId",
            format!("deviceId={device}&linkId={other_link}"),
            Vec::new(),
        ),
        (
            "a deviceId nothing is stored under",
            format!("deviceId={UNSTORED_DEVICE}"),
            Vec::new(),
        ),
        (
            "a deviceId nothing is stored under beside an empty linkId",
            format!("deviceId={UNSTORED_DEVICE}&linkId="),
            Vec::new(),
        ),
        (
            "a linkId nothing is stored under beside an empty deviceId",
            format!("deviceId=&linkId={UNSTORED_LINK}"),
            Vec::new(),
        ),
    ];
    for (case, query, expected) in rows {
        let (status, body) = get_associations(&emulator, &network, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetLinkAssociations({query}) expected 200, got {status} with {body}"
        );
        let reported = pairs_of(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}
