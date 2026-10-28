// SPDX-License-Identifier: CC0-1.0

//! The UPDATE half of the rule `empty_optional_reference` pins for the creates: an OPTIONAL
//! reference member a request supplies is WRITTEN as what it names, and the empty string
//! names no resource, so the record afterwards holds none and the member is absent from
//! every payload — the state a create carrying the same spelling leaves behind.
//!
//! Every id shape of this service is modelled `len 0..50` over `^[\s\S]*$`
//! (`smithy_query query="operation" operation="UpdateConnection"`: `LinkId` len 0..50), so
//! the empty string satisfies the constraint walk and reaches the handler, and the service
//! documents it as the spelling for removing the information a parameter holds ("To remove
//! information for any of the parameters, specify an empty string", `smithy_query
//! query="doc" shape="UpdateConnection"`). The tables below therefore require the emptied
//! member to be ABSENT afterwards — not stored as an id nothing can ever be stored under —
//! and require the update path and the create path to agree member for member on the same
//! spelling.
//!
//! The refusal rows are the other half of the rule: a NON-empty id nothing is stored under
//! — a blank one included, which is a value rather than the absence of one — is still the
//! unresolvable-reference answer and writes nothing.
//!
//! Nothing reaches into `crate::state`: every record is read back through its implemented
//! reader. The harness is the one `global_network_lifecycle` defines.

use serde_json::{json, Value};

use crate::connection_lifecycle::{
    create_connection_ok, fixture, id_of, read_connection, without_state, Fixture,
};
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator};

/// An id of the right shape that nothing was ever stored under, so a reference carrying it
/// names nothing.
const UNSTORED_LINK: &str = "link-00000000000000099";
/// See [`UNSTORED_LINK`].
const UNSTORED_SITE: &str = "site-00000000000000099";

/// `UpdateConnection` on `connection` of `network`.
async fn update_connection(
    emulator: &Emulator,
    network: &str,
    connection: &str,
    body: Value,
) -> (u16, Value) {
    emulator
        .patch(
            &format!("/global-networks/{network}/connections/{connection}"),
            body,
        )
        .await
}

/// `UpdateDevice` on `device` of `network`.
async fn update_device(
    emulator: &Emulator,
    network: &str,
    device: &str,
    body: Value,
) -> (u16, Value) {
    emulator
        .patch(
            &format!("/global-networks/{network}/devices/{device}"),
            body,
        )
        .await
}

/// Read one device back through `GetDevices`, the implemented reader for the resource.
async fn read_device(emulator: &Emulator, case: &str, network: &str, device: &str) -> Value {
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/devices?deviceIds={device}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetDevices({device}) expected 200, got {status} with {body}"
    );
    let listed = body["Devices"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Devices array in {body}"));
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: GetDevices({device}) must answer exactly that device, got {body}"
    );
    listed[0].clone()
}

/// The two link members of a `Connection` — what this rule is about — so the record one
/// path stores can be compared against the record another path stores.
fn link_members(connection: &Value) -> Value {
    json!({
        "LinkId": connection["LinkId"].clone(),
        "ConnectedLinkId": connection["ConnectedLinkId"].clone(),
    })
}

/// The body of a `CreateConnection` that ties the fixture's two devices together.
fn both_devices(seed: &Fixture) -> Value {
    json!({
        "DeviceId": seed.device.clone(),
        "ConnectedDeviceId": seed.connected_device.clone(),
    })
}

/// `body` with `member` set to the reference `expected` stands for: the id itself, or the
/// empty string where the record is to name nothing.
fn with_reference(body: &mut Value, member: &str, expected: &Value) {
    body[member] = match expected.as_str() {
        Some(id) => json!(id),
        None => json!(""),
    };
}

/// One row of the update table: what the connection is seeded with, the update body, and
/// the two link members the stored record must hold afterwards (`Value::Null` standing for
/// "the member is absent").
struct Row {
    case: &'static str,
    seeded: Value,
    update: Value,
    link: Value,
    connected_link: Value,
}

/// The rows differ along WHICH member the update empties (one, the other, both at once),
/// along what the record held before (both links, one, neither — so the emptied member can
/// already be absent), and along what else the same request carries (nothing, a free-form
/// `Description`, the other member naming a stored link).
fn rows(seed: &Fixture) -> Vec<Row> {
    let link = seed.link.clone();
    let connected = seed.connected_link.clone();
    vec![
        Row {
            case: "both links stored, LinkId emptied",
            seeded: json!({ "LinkId": link, "ConnectedLinkId": connected }),
            update: json!({ "LinkId": "" }),
            link: Value::Null,
            connected_link: json!(connected),
        },
        Row {
            case: "both links stored, ConnectedLinkId emptied",
            seeded: json!({ "LinkId": link, "ConnectedLinkId": connected }),
            update: json!({ "ConnectedLinkId": "" }),
            link: json!(link),
            connected_link: Value::Null,
        },
        Row {
            case: "both links stored, both emptied in one request",
            seeded: json!({ "LinkId": link, "ConnectedLinkId": connected }),
            update: json!({ "LinkId": "", "ConnectedLinkId": "" }),
            link: Value::Null,
            connected_link: Value::Null,
        },
        Row {
            case: "LinkId emptied beside a free-form Description",
            seeded: json!({ "LinkId": link, "ConnectedLinkId": connected }),
            update: json!({ "LinkId": "", "Description": "over no recorded link" }),
            link: Value::Null,
            connected_link: json!(connected),
        },
        Row {
            case: "one member emptied while the other names a stored link",
            seeded: json!({ "LinkId": link, "ConnectedLinkId": connected }),
            update: json!({ "LinkId": "", "ConnectedLinkId": link }),
            link: Value::Null,
            connected_link: json!(link),
        },
        Row {
            case: "a member emptied that was already absent",
            seeded: json!({}),
            update: json!({ "LinkId": "" }),
            link: Value::Null,
            connected_link: Value::Null,
        },
        Row {
            case: "only one link stored, the other member emptied",
            seeded: json!({ "LinkId": link }),
            update: json!({ "ConnectedLinkId": "" }),
            link: json!(link),
            connected_link: Value::Null,
        },
        Row {
            case: "the stored link emptied while the other was never set",
            seeded: json!({ "ConnectedLinkId": connected }),
            update: json!({ "ConnectedLinkId": "" }),
            link: Value::Null,
            connected_link: Value::Null,
        },
    ]
}

/// Create a connection of `seed` carrying `extra` beside the two required devices.
async fn seeded_connection(emulator: &Emulator, seed: &Fixture, extra: &Value) -> String {
    let mut body = both_devices(seed);
    for (member, value) in extra.as_object().expect("the row's seed is an object") {
        body[member] = value.clone();
    }
    id_of(&create_connection_ok(emulator, &seed.network, body).await)
}

#[tokio::test]
async fn update_connection_writes_an_emptied_link_member_as_no_link() {
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "update-empty-link").await;

    for row in rows(&seed) {
        let case = row.case;
        let connection = seeded_connection(&emulator, &seed, &row.seeded).await;

        let (status, answer) =
            update_connection(&emulator, &seed.network, &connection, row.update.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {} must be served, got {status} with {answer}",
            row.update
        );
        let updated = answer["Connection"].clone();
        assert_eq!(
            updated["LinkId"], row.link,
            "case {case:?}: expected LinkId {}, got {updated}",
            row.link
        );
        assert_eq!(
            updated["ConnectedLinkId"], row.connected_link,
            "case {case:?}: expected ConnectedLinkId {}, got {updated}",
            row.connected_link
        );

        // ROUND TRIP: the answer is the STORED record, so `GetConnections` reports the
        // emptied member as absent too rather than as an id nothing is stored under.
        let stored = read_connection(&emulator, &seed.network, &connection).await;
        assert_eq!(
            without_state(&stored),
            without_state(&updated),
            "case {case:?}: GetConnections must answer the record the update returned, got \
             {stored} against {updated}"
        );

        // The CREATE path, given the same spelling for the same two members, stores the same
        // record: one input spelling cannot leave two different stored states.
        let mut create_body = both_devices(&seed);
        with_reference(&mut create_body, "LinkId", &row.link);
        with_reference(&mut create_body, "ConnectedLinkId", &row.connected_link);
        let created = create_connection_ok(&emulator, &seed.network, create_body.clone()).await;
        assert_eq!(
            link_members(&created),
            link_members(&stored),
            "case {case:?}: {create_body} must store what the update stored, got {created} \
             against {stored}"
        );

        // Emptying a member is idempotent, and a later non-empty value still names a link,
        // so nothing is permanently wiped.
        let (status, answer) =
            update_connection(&emulator, &seed.network, &connection, row.update.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeated update must be served, got {status} with {answer}"
        );
        assert_eq!(
            link_members(&answer["Connection"]),
            link_members(&stored),
            "case {case:?}: the repeated update must store the same record, got {answer}"
        );

        let (status, answer) = update_connection(
            &emulator,
            &seed.network,
            &connection,
            json!({ "LinkId": seed.link.clone(), "ConnectedLinkId": seed.connected_link.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: re-naming both links must be served, got {status} with {answer}"
        );
        let renamed = read_connection(&emulator, &seed.network, &connection).await;
        assert_eq!(
            link_members(&renamed),
            json!({
                "LinkId": seed.link.clone(),
                "ConnectedLinkId": seed.connected_link.clone(),
            }),
            "case {case:?}: an emptied member must take a link again, got {renamed}"
        );
    }
}

#[tokio::test]
async fn update_connection_still_refuses_a_link_member_that_names_nothing() {
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "update-unresolvable-link").await;
    let elsewhere = fixture(&emulator, "update-unresolvable-link-other").await;
    let connection = seeded_connection(
        &emulator,
        &seed,
        &json!({ "LinkId": seed.link.clone(), "ConnectedLinkId": seed.connected_link.clone() }),
    )
    .await;

    // The rows span the ways a NON-empty id names nothing — never stored, stored under
    // another global network, and a blank id, which is a value rather than the absence of
    // one — on each of the two members.
    let values: Vec<(&str, String)> = vec![
        ("a link id nothing was stored under", UNSTORED_LINK.to_string()),
        ("a link of another global network", elsewhere.link.clone()),
        (
            "a blank id, which is a value and not the absence of one",
            " ".to_string(),
        ),
    ];

    for (member, field) in [("LinkId", "linkId"), ("ConnectedLinkId", "connectedLinkId")] {
        for (case, value) in &values {
            let body = json!({ member: value });
            let (status, answer) =
                update_connection(&emulator, &seed.network, &connection, body.clone()).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {member}: expected 400, got {status} with {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case:?} on {member}: expected ValidationException, got {answer}"
            );
            assert_eq!(
                answer["Fields"][0]["Name"].as_str(),
                Some(field),
                "case {case:?} on {member}: the refusal must name {field:?}, got {answer}"
            );
        }
    }

    // Every refusal above wrote nothing: the connection still names the links it was
    // seeded with.
    let stored = read_connection(&emulator, &seed.network, &connection).await;
    assert_eq!(
        link_members(&stored),
        json!({
            "LinkId": seed.link.clone(),
            "ConnectedLinkId": seed.connected_link.clone(),
        }),
        "case the refused updates: the stored links must be left alone, got {stored}"
    );
}

#[tokio::test]
async fn update_device_writes_an_emptied_site_id_as_no_site() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update empty site").await;
    let site = emulator.create_site(&network, "hq").await;
    let elsewhere = emulator.create_global_network("another network").await;
    let foreign_site = emulator.create_site(&elsewhere, "foreign").await;

    // The same rule on the other reference member this crate updates: the rows differ along
    // what the device held before (a site, no site) and along what else the request carries.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a sited device unsited",
            json!({ "SiteId": site.clone() }),
            json!({ "SiteId": "" }),
        ),
        (
            "a device that was never sited",
            json!({}),
            json!({ "SiteId": "" }),
        ),
        (
            "unsited beside a free-form Description",
            json!({ "SiteId": site.clone() }),
            json!({ "SiteId": "", "Description": "unsited" }),
        ),
    ];

    for (case, seeded, update) in rows {
        let device = create_device_ok(&emulator, &network, seeded).await;
        let (status, answer) = update_device(&emulator, &network, &device, update.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {update} must be served, got {status} with {answer}"
        );
        assert_eq!(
            answer["Device"]["SiteId"],
            Value::Null,
            "case {case:?}: an emptied SiteId must name nothing, got {answer}"
        );

        // ROUND TRIP, and the create path's agreement on the same spelling.
        let stored = read_device(&emulator, case, &network, &device).await;
        assert_eq!(
            stored["SiteId"],
            Value::Null,
            "case {case:?}: the stored device must be unsited, got {stored}"
        );
        let created = create_device_ok(&emulator, &network, json!({ "SiteId": "" })).await;
        let created = read_device(&emulator, case, &network, &created).await;
        assert_eq!(
            created["SiteId"], stored["SiteId"],
            "case {case:?}: the create path must store what the update stored, got {created} \
             against {stored}"
        );

        // A site the request DOES name is still resolved, so the empty spelling is the only
        // value that names nothing.
        let (status, answer) = update_device(
            &emulator,
            &network,
            &device,
            json!({ "SiteId": site.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: naming a site of this global network must be served, got {status} \
             with {answer}"
        );
        let resited = read_device(&emulator, case, &network, &device).await;
        assert_eq!(
            resited["SiteId"],
            json!(site.clone()),
            "case {case:?}: an unsited device must take a site again, got {resited}"
        );
    }

    // A non-empty id that names nothing is still the unresolvable-reference refusal, and
    // writes nothing.
    let device = create_device_ok(&emulator, &network, json!({ "SiteId": site.clone() })).await;
    for (case, site_id) in [
        ("a site id nothing was stored under", UNSTORED_SITE.to_string()),
        ("a site of another global network", foreign_site),
        (
            "a blank SiteId, which is a value and not the absence of one",
            " ".to_string(),
        ),
    ] {
        let (status, answer) =
            update_device(&emulator, &network, &device, json!({ "SiteId": site_id })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert_eq!(
            answer["Fields"][0]["Name"].as_str(),
            Some("siteId"),
            "case {case:?}: the refusal must name siteId, got {answer}"
        );
    }
    let stored = read_device(&emulator, "after the refusals", &network, &device).await;
    assert_eq!(
        stored["SiteId"],
        json!(site),
        "case the refused updates: the stored site must be left alone, got {stored}"
    );
}
