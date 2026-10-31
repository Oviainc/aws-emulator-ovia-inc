// SPDX-License-Identifier: CC0-1.0

//! The CHANGE-FREE update made while the addressed resource's own CREATE is still
//! outstanding, over every resource of this service whose create answers a non-terminal
//! state, driven end-to-end over HTTP through the real router.
//!
//! The rule pinned here is the one `crate::update_no_op` pins for a settled resource: an
//! update that asks for the state the addressed resource is ALREADY in leaves the service no
//! change to apply, so it writes nothing. While the creation's own transition is still
//! outstanding that rule has a sharper edge, because the state a resource reports gives an
//! outstanding reconfiguration precedence over an outstanding creation: an update that wrote
//! a reconfiguration for a request asking for nothing would replace the reported creating
//! state with a reconfiguring one no request asked for, AND push the instant the creation
//! settles out to that call plus the span. Neither may happen — the resource keeps reporting
//! that it is being created, every member of it is left as the call found it, and its
//! creation settles at the instant it was always going to.
//!
//! The rows are the resources of this service whose create owes a transition
//! (`smithy_query shape=GlobalNetworkState,SiteState,LinkState,DeviceState,ConnectionState`;
//! a core network is answered ready, so it has no creating state to preserve), and they
//! differ along the kind of resource, the members its update takes (one, several, a nested
//! structure, a numeric one, a reference) and the reader it is read back through. Each row
//! carries the requests that ask for NO change — none of its mutable members at all, the
//! value the create wrote, a nested structure equal member for member, a reference naming
//! the resource the record already names, and the empty string where the record names none —
//! and ONE request that DOES ask for a change, so a row cannot pass against an updater that
//! records no reconfiguration at all.
//!
//! Nothing reaches into `crate::state`: every resource is created, updated and read back
//! through operations. The harness is the one `global_network_lifecycle` defines and the
//! readers are `crate::resource_state_machine`'s.

use std::time::Duration;

use serde_json::{json, Value};

use crate::connection_lifecycle::{create_connection_ok, fixture, id_of};
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{Emulator, PAST_SETTLE};
use crate::link_lifecycle::create_link_ok;
use crate::resource_state_machine::{read_back, state_of, Reader, AVAILABLE, UPDATING};

/// The state a resource reports while the transition its create owes is still outstanding
/// (`smithy_query shape=GlobalNetworkState`).
const PENDING: &str = "PENDING";

/// Short of the span the clock owes a creation, so a request made this long after the create
/// still reaches a resource that is being created — and far enough into it that a span
/// started here would fall due measurably later than the creation's own.
const WITHIN_SPAN: Duration = Duration::from_millis(250);

/// Waited from inside that span, this carries the clock past the instant the creation's own
/// transition fell due while staying short of where a span started at [`WITHIN_SPAN`] would
/// have pushed it — so a resource read here says which of the two happened.
const PAST_ORIGINAL_SPAN: Duration = Duration::from_millis(400);

/// The `Description` every seeded resource of this suite is created with, so a request can
/// supply the value the record already holds.
const SEEDED_DESCRIPTION: &str = "seeded description";

/// The `Location` the seeded site holds.
fn seeded_location() -> Value {
    json!({ "Address": "1 Main St", "Latitude": "47.6062", "Longitude": "-122.3321" })
}

/// The `AWSLocation` the seeded device holds.
fn seeded_aws_location() -> Value {
    json!({ "Zone": "us-west-2a" })
}

/// The `Bandwidth` the seeded link holds.
fn seeded_bandwidth() -> Value {
    json!({ "UploadSpeed": 10, "DownloadSpeed": 20 })
}

/// The kinds of resource this suite drives, one row each.
#[derive(Clone, Copy, Debug)]
enum Kind {
    GlobalNetwork,
    Site,
    Link,
    Device,
    Connection,
}

/// Every row of the table.
const KINDS: [Kind; 5] = [
    Kind::GlobalNetwork,
    Kind::Site,
    Kind::Link,
    Kind::Device,
    Kind::Connection,
];

/// One row: a resource whose create is still outstanding, the requests that ask it for NO
/// change, and one that does.
struct CreatingCase {
    case: &'static str,
    /// The `PATCH` URI the row's updater is addressed by.
    uri: String,
    /// The member of the update's own answer the resource arrives under.
    answer_member: &'static str,
    /// The id the row's resource was minted with.
    id: String,
    /// How the row reads its resource back.
    reader: Reader,
    /// The requests that ask for the state the resource is already in, each labelled.
    change_free: Vec<(&'static str, Value)>,
    /// A request that DOES ask for a change, with the member it has to write and the value
    /// that member must then hold.
    change: (Value, &'static str, Value),
}

/// `CreateSite` that must succeed, answering the minted `SiteId`; the shared harness helper
/// carries a `Description` only, and this row's site also holds a `Location`.
async fn create_site_ok(emulator: &Emulator, network: &str, body: Value) -> String {
    let (status, created) = emulator
        .post(&format!("/global-networks/{network}/sites"), body)
        .await;
    assert_eq!(
        status, 200,
        "CreateSite: expected 200, got {status} with {created}"
    );
    created["Site"]["SiteId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateSite: no SiteId in {created}"))
        .to_string()
}

/// Create the row's resource, LAST, so the span the clock owes its creation starts here and
/// the requests the caller makes next reach a resource that is still being created.
async fn seed(emulator: &Emulator, kind: Kind) -> CreatingCase {
    match kind {
        Kind::GlobalNetwork => {
            let network = emulator.create_global_network(SEEDED_DESCRIPTION).await;
            CreatingCase {
                case: "a global network being created, the one member it takes",
                uri: format!("/global-networks/{network}"),
                answer_member: "GlobalNetwork",
                reader: Reader::Listed {
                    uri: "/global-networks".to_string(),
                    member: "GlobalNetworks",
                    id_member: "GlobalNetworkId",
                },
                id: network,
                change_free: vec![
                    ("no mutable member at all", json!({})),
                    (
                        "the Description the create wrote",
                        json!({ "Description": SEEDED_DESCRIPTION }),
                    ),
                ],
                change: (
                    json!({ "Description": "reconfigured" }),
                    "Description",
                    json!("reconfigured"),
                ),
            }
        }
        Kind::Site => {
            let network = emulator.create_global_network("creating-site").await;
            let site = create_site_ok(
                emulator,
                &network,
                json!({ "Description": SEEDED_DESCRIPTION, "Location": seeded_location() }),
            )
            .await;
            CreatingCase {
                case: "a site being created, a member and a nested structure",
                uri: format!("/global-networks/{network}/sites/{site}"),
                answer_member: "Site",
                reader: Reader::Listed {
                    uri: format!("/global-networks/{network}/sites"),
                    member: "Sites",
                    id_member: "SiteId",
                },
                id: site,
                change_free: vec![
                    ("no mutable member at all", json!({})),
                    (
                        "the Location the create wrote, member for member",
                        json!({ "Location": seeded_location() }),
                    ),
                    (
                        "both members, each the value the record holds",
                        json!({
                            "Description": SEEDED_DESCRIPTION,
                            "Location": seeded_location(),
                        }),
                    ),
                ],
                change: (
                    json!({ "Location": { "Address": "9 Elm St" } }),
                    "Location",
                    json!({ "Address": "9 Elm St" }),
                ),
            }
        }
        Kind::Link => {
            let network = emulator.create_global_network("creating-link").await;
            let site = emulator.create_site(&network, "creating-link-site").await;
            let link = create_link_ok(
                emulator,
                &network,
                &site,
                &json!({
                    "Description": SEEDED_DESCRIPTION,
                    "Type": "broadband",
                    "Provider": "AnyCompany Networks",
                    "Bandwidth": seeded_bandwidth(),
                }),
            )
            .await;
            CreatingCase {
                case: "a link being created, three descriptors and a numeric structure",
                uri: format!("/global-networks/{network}/links/{link}"),
                answer_member: "Link",
                reader: Reader::Listed {
                    uri: format!("/global-networks/{network}/links"),
                    member: "Links",
                    id_member: "LinkId",
                },
                id: link,
                change_free: vec![
                    ("no mutable member at all", json!({})),
                    (
                        "the Bandwidth the create wrote, speed for speed",
                        json!({ "Bandwidth": seeded_bandwidth() }),
                    ),
                    (
                        "every member, each the value the record holds",
                        json!({
                            "Description": SEEDED_DESCRIPTION,
                            "Type": "broadband",
                            "Provider": "AnyCompany Networks",
                            "Bandwidth": seeded_bandwidth(),
                        }),
                    ),
                ],
                change: (
                    json!({ "Bandwidth": { "UploadSpeed": 50, "DownloadSpeed": 20 } }),
                    "Bandwidth",
                    json!({ "UploadSpeed": 50, "DownloadSpeed": 20 }),
                ),
            }
        }
        Kind::Device => {
            let network = emulator.create_global_network("creating-device").await;
            let site = emulator.create_site(&network, "creating-device-site").await;
            let device = create_device_ok(
                emulator,
                &network,
                json!({
                    "SiteId": site,
                    "Description": SEEDED_DESCRIPTION,
                    "Type": "router",
                    "AWSLocation": seeded_aws_location(),
                }),
            )
            .await;
            CreatingCase {
                case: "a device being created, descriptors, a structure and a reference",
                uri: format!("/global-networks/{network}/devices/{device}"),
                answer_member: "Device",
                reader: Reader::Listed {
                    uri: format!("/global-networks/{network}/devices"),
                    member: "Devices",
                    id_member: "DeviceId",
                },
                id: device,
                change_free: vec![
                    ("no mutable member at all", json!({})),
                    (
                        "the site the device already sits at",
                        json!({ "SiteId": site }),
                    ),
                    (
                        "the AWSLocation the create wrote",
                        json!({ "AWSLocation": seeded_aws_location() }),
                    ),
                ],
                change: (
                    json!({ "Vendor": "Example Networks" }),
                    "Vendor",
                    json!("Example Networks"),
                ),
            }
        }
        Kind::Connection => {
            // A connection needs two devices and the links to tie them over, which the
            // connection suite's fixture builds in a global network of its own.
            let seed = fixture(emulator, "creating-connection").await;
            let connection = id_of(
                &create_connection_ok(
                    emulator,
                    &seed.network,
                    json!({
                        "DeviceId": seed.device,
                        "ConnectedDeviceId": seed.connected_device,
                        "Description": SEEDED_DESCRIPTION,
                        "LinkId": seed.link,
                    }),
                )
                .await,
            );
            CreatingCase {
                case: "a connection being created, a member and two references",
                uri: format!(
                    "/global-networks/{}/connections/{connection}",
                    seed.network
                ),
                answer_member: "Connection",
                reader: Reader::Listed {
                    uri: format!("/global-networks/{}/connections", seed.network),
                    member: "Connections",
                    id_member: "ConnectionId",
                },
                id: connection,
                change_free: vec![
                    ("no mutable member at all", json!({})),
                    (
                        "the link the connection already names",
                        json!({ "LinkId": seed.link.clone() }),
                    ),
                    (
                        "an emptied reference the record already names none for",
                        json!({ "ConnectedLinkId": "" }),
                    ),
                ],
                change: (
                    json!({ "ConnectedLinkId": seed.connected_link.clone() }),
                    "ConnectedLinkId",
                    json!(seed.connected_link),
                ),
            }
        }
    }
}

#[tokio::test]
async fn a_change_free_update_leaves_a_creating_resource_creating_and_settling_on_its_own_clock() {
    let emulator = Emulator::new();

    // One row at a time, each on its own timeline: the span the clock owes a row's creation
    // starts where that row is seeded.
    for kind in KINDS {
        let row = seed(&emulator, kind).await;
        let case = row.case;

        let creating = read_back(&emulator, case, &row.id, &row.reader).await;
        assert_eq!(
            state_of(case, &creating),
            PENDING,
            "case {case:?}: expected the creation's own transition to still be outstanding, got {creating}"
        );

        for (label, body) in &row.change_free {
            let (status, answer) = emulator.patch(&row.uri, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?} / {label:?}: expected 200, got {status} with {answer}"
            );
            let resource = &answer[row.answer_member];
            assert_eq!(
                state_of(case, resource),
                PENDING,
                "case {case:?} / {label:?}: a change-free update must leave the creation the only transition outstanding, got {resource}"
            );
            // The whole record, member for member: a write anywhere in it — a moved state, a
            // restamped instant, a rewritten member — fails here.
            assert_eq!(
                *resource, creating,
                "case {case:?} / {label:?}: expected the record untouched ({creating}), got {resource}"
            );

            // And the next caller sees the same record: nothing was written to the store
            // either.
            let reread = read_back(&emulator, case, &row.id, &row.reader).await;
            assert_eq!(
                reread, creating,
                "case {case:?} / {label:?}: expected the stored record untouched ({creating}), got {reread}"
            );
        }

        // Still inside the creation's span, and far enough into it that a span started by
        // this call would fall due measurably later than the creation's own.
        tokio::time::sleep(WITHIN_SPAN).await;
        let label = "no mutable member at all, later in the creation's span";
        let (status, answer) = emulator.patch(&row.uri, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?} / {label:?}: expected 200, got {status} with {answer}"
        );
        let resource = &answer[row.answer_member];
        assert_eq!(
            *resource, creating,
            "case {case:?} / {label:?}: expected the record untouched ({creating}), got {resource}"
        );

        // Past the instant the creation was always going to settle at, and short of where a
        // span started by the call above would have pushed it: nothing was called while the
        // clock ran, so the resource is settled, holding exactly what it was created with.
        tokio::time::sleep(PAST_ORIGINAL_SPAN).await;
        let settled = read_back(&emulator, case, &row.id, &row.reader).await;
        let mut expected = creating.clone();
        expected["State"] = json!(AVAILABLE);
        assert_eq!(
            settled, expected,
            "case {case:?}: expected the creation to settle at the instant it was always due, with nothing else written ({expected}), got {settled}"
        );
    }
}

#[tokio::test]
async fn a_change_asked_of_a_creating_resource_is_recorded_as_a_reconfiguration() {
    let emulator = Emulator::new();

    // The control for the rows above: the SAME updaters, reached while the creation is still
    // outstanding, do record a reconfiguration for a request that asks for one — so a row
    // above cannot pass against an updater that never writes a transition at all.
    let mut rows = Vec::new();
    for kind in KINDS {
        rows.push(seed(&emulator, kind).await);
    }

    for row in &rows {
        let case = row.case;
        let (body, member, expected) = &row.change;
        let (status, answer) = emulator.patch(&row.uri, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the changing request expected 200, got {status} with {answer}"
        );
        let resource = &answer[row.answer_member];
        assert_eq!(
            state_of(case, resource),
            UPDATING,
            "case {case:?}: an accepted change owes the transition out of {UPDATING}, got {resource}"
        );
        assert_eq!(
            resource[*member], *expected,
            "case {case:?}: expected the changing request to write {member} {expected}, got {resource}"
        );
    }

    // Both transitions fall due by themselves, and the record holds what the change wrote.
    tokio::time::sleep(PAST_SETTLE).await;
    for row in &rows {
        let case = row.case;
        let (_, member, expected) = &row.change;
        let settled = read_back(&emulator, case, &row.id, &row.reader).await;
        assert_eq!(
            state_of(case, &settled),
            AVAILABLE,
            "case {case:?}: expected {AVAILABLE} once both transitions are due, got {settled}"
        );
        assert_eq!(
            settled[*member], *expected,
            "case {case:?}: expected the written {member} {expected} to survive, got {settled}"
        );
    }
}
