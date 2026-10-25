// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the connection lifecycle (`CreateConnection`,
//! `UpdateConnection`, `GetConnections`), driven over HTTP through the real router.
//!
//! The rule these suites pin for the UPDATE: an update of an existing resource writes ONLY
//! the members the request carries — an absent member leaves the stored value as it was, a
//! supplied one replaces it (an empty string included, which is how the service documents
//! removing information: "To remove information for any of the parameters, specify an
//! empty string", `smithy_query query="doc" shape="UpdateConnection"`) — and rewrites
//! nothing else: ids, ARN, the two devices, `CreatedAt`, tags, the clock-derived lifecycle
//! state and every other record stay as they were. Addressing a resource the store does
//! not hold under the path's parent is the missing-resource answer (404
//! `ResourceNotFoundException`), and a reference member naming a resource the global
//! network does not hold is a field failure (400 `ValidationException`); both store
//! nothing.
//!
//! The CREATE and the listing are covered here too, because they are what makes a stored
//! connection observable: nothing reaches into `crate::state`, a connection is only ever
//! read back through `GetConnections`. The harness is the one `global_network_lifecycle`
//! defines.

use serde_json::{json, Value};

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{
    error_code, expected_scoped_arn, is_minted_id, Emulator, PAST_SETTLE,
};
use crate::link_lifecycle::{create_link_ok, plain_link};

/// The `Description` every seeded connection starts with, so a row that supplies no
/// `Description` has a value to leave alone.
pub(crate) const BASE_DESCRIPTION: &str = "baseline description";

/// The text every `ResourceNotFoundException` of this service carries
/// (`crate::state::RESOURCE_NOT_FOUND_MESSAGE`, pinned by `resource_not_found_message`).
pub(crate) const NOT_FOUND_MESSAGE: &str = "Resource not found.";

/// The ARN resource types a refusal or an ARN of this suite reports.
pub(crate) const GLOBAL_NETWORK: &str = "global-network";
/// See [`GLOBAL_NETWORK`].
pub(crate) const CONNECTION: &str = "connection";

/// The `Tags` every seeded connection carries, which no update may rewrite.
pub(crate) fn base_tags() -> Value {
    json!([{ "Key": "env", "Value": "test" }])
}

/// The resources a connection needs: a global network, a site to hang links off, two
/// devices to tie together and two links to tie them over.
pub(crate) struct Fixture {
    pub(crate) network: String,
    pub(crate) device: String,
    pub(crate) connected_device: String,
    pub(crate) link: String,
    pub(crate) connected_link: String,
}

/// Create the resources of [`Fixture`] in one global network of `emulator`.
pub(crate) async fn fixture(emulator: &Emulator, label: &str) -> Fixture {
    let network = emulator.create_global_network(label).await;
    let site = emulator.create_site(&network, &format!("{label}-site")).await;
    Fixture {
        device: create_device_ok(emulator, &network, json!({ "Description": "first" })).await,
        connected_device: create_device_ok(emulator, &network, json!({ "Description": "second" }))
            .await,
        link: create_link_ok(emulator, &network, &site, &plain_link()).await,
        connected_link: create_link_ok(emulator, &network, &site, &plain_link()).await,
        network,
    }
}

/// `CreateConnection` against `network`.
async fn create_connection(emulator: &Emulator, network: &str, body: Value) -> (u16, Value) {
    emulator
        .post(&format!("/global-networks/{network}/connections"), body)
        .await
}

/// `CreateConnection` that must succeed, answering the whole minted `Connection`.
pub(crate) async fn create_connection_ok(emulator: &Emulator, network: &str, body: Value) -> Value {
    let (status, created) = create_connection(emulator, network, body.clone()).await;
    assert_eq!(
        status, 200,
        "CreateConnection({body}): expected 200, got {status} with {created}"
    );
    created["Connection"].clone()
}

/// The `ConnectionId` of a `Connection`.
pub(crate) fn id_of(connection: &Value) -> String {
    connection["ConnectionId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a ConnectionId in {connection}"))
        .to_string()
}

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

/// `GetConnections` for `query` (appended to the connections URI), with the status.
pub(crate) async fn get_connections(
    emulator: &Emulator,
    network: &str,
    query: &str,
) -> (u16, Value) {
    let uri = if query.is_empty() {
        format!("/global-networks/{network}/connections")
    } else {
        format!("/global-networks/{network}/connections?{query}")
    };
    emulator.get(&uri).await
}

/// The `ConnectionId`s one `GetConnections` answer carries, in the order returned.
pub(crate) fn ids_in(body: &Value) -> Vec<String> {
    body["Connections"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Connections array in {body}"))
        .iter()
        .map(id_of)
        .collect()
}

/// Read one connection back through `GetConnections`, the implemented reader for this
/// resource.
pub(crate) async fn read_connection(emulator: &Emulator, network: &str, connection: &str) -> Value {
    let (status, body) =
        get_connections(emulator, network, &format!("connectionIds={connection}")).await;
    assert_eq!(
        status, 200,
        "GetConnections({connection}): expected 200, got {status} with {body}"
    );
    let listed = body["Connections"]
        .as_array()
        .unwrap_or_else(|| panic!("GetConnections({connection}): no Connections array in {body}"));
    assert_eq!(
        listed.len(),
        1,
        "GetConnections({connection}): expected exactly that connection, got {body}"
    );
    listed[0].clone()
}

/// A `Connection` without its `State`, so a create answer and a later read can be compared
/// member for member without racing the `PENDING` -> `AVAILABLE` transition the clock owes.
pub(crate) fn without_state(connection: &Value) -> Value {
    let mut copy = connection.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("State");
    }
    copy
}

/// One row of the create table: the request body, and the members the stored record must
/// hold afterwards (`Value::Null` standing for "the member is absent").
struct CreateCase {
    case: &'static str,
    body: Value,
    description: Value,
    link: Value,
    connected_link: Value,
    tags: Value,
}

/// The rows of the create table. They differ along WHICH optional members the request
/// carries (both links, one, neither; a description, tags) and along the value's form — an
/// empty string, the modelled maximum length of `ConstrainedString` (256), and a
/// multi-line, non-ASCII value.
fn create_cases(seed: &Fixture) -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "both links, a description and tags",
            body: json!({
                "Description": BASE_DESCRIPTION,
                "LinkId": seed.link,
                "ConnectedLinkId": seed.connected_link,
                "Tags": base_tags(),
            }),
            description: json!(BASE_DESCRIPTION),
            link: json!(seed.link),
            connected_link: json!(seed.connected_link),
            tags: base_tags(),
        },
        CreateCase {
            case: "neither link, so both stay absent",
            body: json!({ "Description": "over no recorded link" }),
            description: json!("over no recorded link"),
            link: Value::Null,
            connected_link: Value::Null,
            tags: json!([]),
        },
        CreateCase {
            case: "the first link only",
            body: json!({ "LinkId": seed.link }),
            description: Value::Null,
            link: json!(seed.link),
            connected_link: Value::Null,
            tags: json!([]),
        },
        CreateCase {
            case: "the second link only",
            body: json!({ "ConnectedLinkId": seed.connected_link }),
            description: Value::Null,
            link: Value::Null,
            connected_link: json!(seed.connected_link),
            tags: json!([]),
        },
        CreateCase {
            case: "both ends over the same link",
            body: json!({ "LinkId": seed.link, "ConnectedLinkId": seed.link }),
            description: Value::Null,
            link: json!(seed.link),
            connected_link: json!(seed.link),
            tags: json!([]),
        },
        CreateCase {
            case: "an empty description",
            body: json!({ "Description": "" }),
            description: json!(""),
            link: Value::Null,
            connected_link: Value::Null,
            tags: json!([]),
        },
        CreateCase {
            case: "a description at the modelled maximum of 256 characters",
            body: json!({ "Description": "d".repeat(256) }),
            description: json!("d".repeat(256)),
            link: Value::Null,
            connected_link: Value::Null,
            tags: json!([]),
        },
        CreateCase {
            case: "a multi-line, non-ascii description",
            body: json!({ "Description": "line one\nline two \u{1f310}" }),
            description: json!("line one\nline two \u{1f310}"),
            link: Value::Null,
            connected_link: Value::Null,
            tags: json!([]),
        },
    ]
}

#[tokio::test]
async fn create_connection_stores_every_member_and_get_connections_answers_from_the_stored_record()
{
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "create-connection").await;

    for row in create_cases(&seed) {
        let mut body = row.body.clone();
        let object = body
            .as_object_mut()
            .unwrap_or_else(|| panic!("case {}: the row body must be an object", row.case));
        object.insert("DeviceId".to_string(), json!(seed.device));
        object.insert(
            "ConnectedDeviceId".to_string(),
            json!(seed.connected_device),
        );

        let connection = create_connection_ok(&emulator, &seed.network, body).await;
        let connection_id = id_of(&connection);

        assert!(
            is_minted_id(&connection_id, CONNECTION),
            "case {}: expected a minted connection id, got {connection_id}",
            row.case
        );
        assert_eq!(
            connection["ConnectionArn"],
            json!(expected_scoped_arn(CONNECTION, &seed.network, &connection_id)),
            "case {}: wrong ARN in {connection}",
            row.case
        );
        assert_eq!(
            connection["GlobalNetworkId"],
            json!(seed.network.clone()),
            "case {}: wrong parent back-reference in {connection}",
            row.case
        );
        assert_eq!(
            connection["DeviceId"],
            json!(seed.device.clone()),
            "case {}: wrong DeviceId in {connection}",
            row.case
        );
        assert_eq!(
            connection["ConnectedDeviceId"],
            json!(seed.connected_device.clone()),
            "case {}: wrong ConnectedDeviceId in {connection}",
            row.case
        );
        assert_eq!(
            connection["Description"], row.description,
            "case {}: expected Description {}, got {connection}",
            row.case, row.description
        );
        assert_eq!(
            connection["LinkId"], row.link,
            "case {}: expected LinkId {}, got {connection}",
            row.case, row.link
        );
        assert_eq!(
            connection["ConnectedLinkId"], row.connected_link,
            "case {}: expected ConnectedLinkId {}, got {connection}",
            row.case, row.connected_link
        );
        assert_eq!(
            connection["Tags"], row.tags,
            "case {}: expected Tags {}, got {connection}",
            row.case, row.tags
        );
        assert!(
            connection["CreatedAt"].is_number(),
            "case {}: expected a minted CreatedAt, got {connection}",
            row.case
        );

        // The answer is the STORED record, not a view of the request.
        let stored = read_connection(&emulator, &seed.network, &connection_id).await;
        assert_eq!(
            without_state(&stored),
            without_state(&connection),
            "case {}: GetConnections must answer the record the create returned, got {stored}",
            row.case
        );
    }
}

#[tokio::test]
async fn create_connection_refuses_a_reference_the_global_network_does_not_hold() {
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "create-refusal").await;
    let other = fixture(&emulator, "create-refusal-other").await;

    // Well-formed ids the store never minted.
    let unknown_device = "device-00000000000000099";
    let unknown_link = "link-00000000000000099";

    // Each row is a different reference member naming a resource this global network does
    // not hold: one that never existed, and a live one belonging to another global network.
    let rows = vec![
        (
            "a DeviceId that never existed",
            json!({ "DeviceId": unknown_device, "ConnectedDeviceId": seed.connected_device }),
            "deviceId",
            "Device not found.",
        ),
        (
            "a ConnectedDeviceId that never existed",
            json!({ "DeviceId": seed.device, "ConnectedDeviceId": unknown_device }),
            "connectedDeviceId",
            "Device not found.",
        ),
        (
            "a DeviceId of another global network",
            json!({ "DeviceId": other.device, "ConnectedDeviceId": seed.connected_device }),
            "deviceId",
            "Device not found.",
        ),
        (
            "a LinkId that never existed",
            json!({
                "DeviceId": seed.device,
                "ConnectedDeviceId": seed.connected_device,
                "LinkId": unknown_link,
            }),
            "linkId",
            "Link not found.",
        ),
        (
            "a ConnectedLinkId of another global network",
            json!({
                "DeviceId": seed.device,
                "ConnectedDeviceId": seed.connected_device,
                "ConnectedLinkId": other.link,
            }),
            "connectedLinkId",
            "Link not found.",
        ),
    ];

    for (case, body, field, message) in rows {
        let (status, answer) = create_connection(&emulator, &seed.network, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: wrong error code in {answer}"
        );
        assert_eq!(
            answer["Message"].as_str(),
            Some(message),
            "case {case}: expected the message {message:?}, got {answer}"
        );
        assert_eq!(
            answer["Reason"].as_str(),
            Some("FieldValidationFailed"),
            "case {case}: expected a field failure, got {answer}"
        );
        assert_eq!(
            answer["Fields"][0]["Name"].as_str(),
            Some(field),
            "case {case}: expected the refusal to name {field:?}, got {answer}"
        );
    }

    // A global network the store does not hold is a field failure too: this create models
    // no `ResourceNotFoundException`.
    let (status, answer) = create_connection(
        &emulator,
        "global-network-00000000000000099",
        json!({ "DeviceId": seed.device, "ConnectedDeviceId": seed.connected_device }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case an unknown global network: expected 400, got {status} with {answer}"
    );
    assert_eq!(
        answer["Fields"][0]["Name"].as_str(),
        Some("globalNetworkId"),
        "case an unknown global network: expected the refusal to name globalNetworkId, got {answer}"
    );

    // Every refusal above stored nothing.
    let (status, listed) = get_connections(&emulator, &seed.network, "").await;
    assert_eq!(
        status, 200,
        "GetConnections after the refusals: expected 200, got {status} with {listed}"
    );
    assert_eq!(
        ids_in(&listed),
        Vec::<String>::new(),
        "case refused creates: nothing may be stored, got {listed}"
    );
}

/// One row of the update table: the request body, and the three mutable members the stored
/// record must hold afterwards (`Value::Null` standing for "the member is absent").
struct UpdateCase {
    case: &'static str,
    body: Value,
    description: Value,
    link: Value,
    connected_link: Value,
}

/// The rows of the update table. They differ along WHICH mutable members the request
/// carries (all three, each one alone, none) and along the value's form — another stored
/// link, an empty string (the documented removal), the modelled maximum length of
/// `ConstrainedString` (256), and a multi-line, non-ASCII value.
fn update_cases(seed: &Fixture) -> Vec<UpdateCase> {
    let link = &seed.link;
    let connected_link = &seed.connected_link;
    vec![
        UpdateCase {
            case: "all three members supplied, the two links swapped",
            body: json!({
                "Description": "updated description",
                "LinkId": connected_link,
                "ConnectedLinkId": link,
            }),
            description: json!("updated description"),
            link: json!(connected_link),
            connected_link: json!(link),
        },
        UpdateCase {
            case: "Description only, so both stored links are left alone",
            body: json!({ "Description": "description only" }),
            description: json!("description only"),
            link: json!(link),
            connected_link: json!(connected_link),
        },
        UpdateCase {
            case: "LinkId only, so the Description and the other link are left alone",
            body: json!({ "LinkId": connected_link }),
            description: json!(BASE_DESCRIPTION),
            link: json!(connected_link),
            connected_link: json!(connected_link),
        },
        UpdateCase {
            case: "ConnectedLinkId only",
            body: json!({ "ConnectedLinkId": link }),
            description: json!(BASE_DESCRIPTION),
            link: json!(link),
            connected_link: json!(link),
        },
        UpdateCase {
            case: "no mutable member at all, so nothing changes",
            body: json!({}),
            description: json!(BASE_DESCRIPTION),
            link: json!(link),
            connected_link: json!(connected_link),
        },
        UpdateCase {
            case: "an empty Description removes the information it held",
            body: json!({ "Description": "" }),
            description: json!(""),
            link: json!(link),
            connected_link: json!(connected_link),
        },
        UpdateCase {
            case: "an empty LinkId removes the link it held",
            body: json!({ "LinkId": "" }),
            description: json!(BASE_DESCRIPTION),
            link: json!(""),
            connected_link: json!(connected_link),
        },
        UpdateCase {
            case: "an empty ConnectedLinkId removes the link it held",
            body: json!({ "ConnectedLinkId": "" }),
            description: json!(BASE_DESCRIPTION),
            link: json!(link),
            connected_link: json!(""),
        },
        UpdateCase {
            case: "both links removed at once",
            body: json!({ "LinkId": "", "ConnectedLinkId": "" }),
            description: json!(BASE_DESCRIPTION),
            link: json!(""),
            connected_link: json!(""),
        },
        UpdateCase {
            case: "a Description at the modelled maximum of 256 characters",
            body: json!({ "Description": "d".repeat(256) }),
            description: json!("d".repeat(256)),
            link: json!(link),
            connected_link: json!(connected_link),
        },
        UpdateCase {
            case: "a multi-line, non-ascii Description",
            body: json!({ "Description": "zeile eins\nzeile zwei \u{1f310}" }),
            description: json!("zeile eins\nzeile zwei \u{1f310}"),
            link: json!(link),
            connected_link: json!(connected_link),
        },
    ]
}

/// Create a connection seeded with [`BASE_DESCRIPTION`], both of the fixture's links and
/// [`base_tags`], and answer its minted id.
pub(crate) async fn create_seeded_connection(emulator: &Emulator, seed: &Fixture) -> String {
    let connection = create_connection_ok(
        emulator,
        &seed.network,
        json!({
            "DeviceId": seed.device,
            "ConnectedDeviceId": seed.connected_device,
            "LinkId": seed.link,
            "ConnectedLinkId": seed.connected_link,
            "Description": BASE_DESCRIPTION,
            "Tags": base_tags(),
        }),
    )
    .await;
    id_of(&connection)
}

#[tokio::test]
async fn update_connection_writes_only_the_supplied_members_and_round_trips_through_get_connections()
{
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "update-connection").await;
    let rows = update_cases(&seed);

    // One identically seeded connection per row, plus a sibling no row names.
    let mut connections = Vec::new();
    for _ in &rows {
        connections.push(create_seeded_connection(&emulator, &seed).await);
    }
    let sibling = create_seeded_connection(&emulator, &seed).await;

    // The PENDING -> AVAILABLE transition the create owes is past before the first update,
    // so an update that restarted that clock would be visible here.
    std::thread::sleep(PAST_SETTLE);

    for (row, connection_id) in rows.iter().zip(&connections) {
        let before = read_connection(&emulator, &seed.network, connection_id).await;
        let created_at = before["CreatedAt"].clone();

        let (status, answer) =
            update_connection(&emulator, &seed.network, connection_id, row.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} with {answer}",
            row.case
        );
        let connection = &answer["Connection"];

        assert_eq!(
            connection["Description"], row.description,
            "case {}: expected Description {}, got {connection}",
            row.case, row.description
        );
        assert_eq!(
            connection["LinkId"], row.link,
            "case {}: expected LinkId {}, got {connection}",
            row.case, row.link
        );
        assert_eq!(
            connection["ConnectedLinkId"], row.connected_link,
            "case {}: expected ConnectedLinkId {}, got {connection}",
            row.case, row.connected_link
        );

        // Everything the update does not take as input is the record it already was.
        assert_eq!(
            connection["ConnectionId"],
            json!(connection_id),
            "case {}: ConnectionId must be the addressed connection, got {connection}",
            row.case
        );
        assert_eq!(
            connection["ConnectionArn"],
            json!(expected_scoped_arn(CONNECTION, &seed.network, connection_id)),
            "case {}: the ARN must not be rewritten, got {connection}",
            row.case
        );
        assert_eq!(
            connection["GlobalNetworkId"],
            json!(seed.network.clone()),
            "case {}: the parent back-reference must not be rewritten, got {connection}",
            row.case
        );
        assert_eq!(
            connection["DeviceId"],
            json!(seed.device.clone()),
            "case {}: the first device must not be rewritten, got {connection}",
            row.case
        );
        assert_eq!(
            connection["ConnectedDeviceId"],
            json!(seed.connected_device.clone()),
            "case {}: the second device must not be rewritten, got {connection}",
            row.case
        );
        assert_eq!(
            connection["CreatedAt"], created_at,
            "case {}: CreatedAt must stay the create's instant, got {connection}",
            row.case
        );
        assert_eq!(
            connection["Tags"],
            base_tags(),
            "case {}: Tags must not be rewritten, got {connection}",
            row.case
        );
        assert_eq!(
            connection["State"],
            json!("AVAILABLE"),
            "case {}: an update must not restart the create's settle clock, got {connection}",
            row.case
        );

        // The answer is the STORED record, not a view of the request.
        let stored = read_connection(&emulator, &seed.network, connection_id).await;
        assert_eq!(
            stored, *connection,
            "case {}: GetConnections must answer the record the update returned, got {stored}",
            row.case
        );
    }

    // No update reached a connection it did not name.
    let untouched = read_connection(&emulator, &seed.network, &sibling).await;
    assert_eq!(
        untouched["Description"],
        json!(BASE_DESCRIPTION),
        "case sibling connection: an update of another connection must leave it alone, got {untouched}"
    );
    assert_eq!(
        untouched["LinkId"],
        json!(seed.link.clone()),
        "case sibling connection: its LinkId must be left alone, got {untouched}"
    );
}

#[tokio::test]
async fn successive_updates_accumulate_on_the_stored_record() {
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "update-connection-sequence").await;

    // Seeded with NO mutable member, so a member nothing has supplied stays absent.
    let created = create_connection_ok(
        &emulator,
        &seed.network,
        json!({ "DeviceId": seed.device, "ConnectedDeviceId": seed.connected_device }),
    )
    .await;
    let connection_id = id_of(&created);

    // Each step applies to the record the step before it left behind.
    let steps = vec![
        UpdateCase {
            case: "step 1: an empty update of an empty record",
            body: json!({}),
            description: Value::Null,
            link: Value::Null,
            connected_link: Value::Null,
        },
        UpdateCase {
            case: "step 2: a Description where there was none",
            body: json!({ "Description": "first" }),
            description: json!("first"),
            link: Value::Null,
            connected_link: Value::Null,
        },
        UpdateCase {
            case: "step 3: a LinkId where there was none, keeping the Description",
            body: json!({ "LinkId": seed.link }),
            description: json!("first"),
            link: json!(seed.link),
            connected_link: Value::Null,
        },
        UpdateCase {
            case: "step 4: the other link, keeping the first",
            body: json!({ "ConnectedLinkId": seed.connected_link }),
            description: json!("first"),
            link: json!(seed.link),
            connected_link: json!(seed.connected_link),
        },
        UpdateCase {
            case: "step 5: emptying the Description keeps both links",
            body: json!({ "Description": "" }),
            description: json!(""),
            link: json!(seed.link),
            connected_link: json!(seed.connected_link),
        },
        UpdateCase {
            case: "step 6: a new LinkId replaces the one stored",
            body: json!({ "LinkId": seed.connected_link }),
            description: json!(""),
            link: json!(seed.connected_link),
            connected_link: json!(seed.connected_link),
        },
        UpdateCase {
            case: "step 7: every member at once",
            body: json!({
                "Description": "second",
                "LinkId": seed.link,
                "ConnectedLinkId": "",
            }),
            description: json!("second"),
            link: json!(seed.link),
            connected_link: json!(""),
        },
    ];

    for step in &steps {
        let (status, answer) =
            update_connection(&emulator, &seed.network, &connection_id, step.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} with {answer}",
            step.case
        );
        let stored = read_connection(&emulator, &seed.network, &connection_id).await;
        assert_eq!(
            stored["Description"], step.description,
            "case {}: expected the stored Description {}, got {stored}",
            step.case, step.description
        );
        assert_eq!(
            stored["LinkId"], step.link,
            "case {}: expected the stored LinkId {}, got {stored}",
            step.case, step.link
        );
        assert_eq!(
            stored["ConnectedLinkId"], step.connected_link,
            "case {}: expected the stored ConnectedLinkId {}, got {stored}",
            step.case, step.connected_link
        );
    }
}

#[tokio::test]
async fn update_connection_refuses_a_resource_the_store_does_not_hold_and_stores_nothing() {
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "update-not-found").await;
    let other = fixture(&emulator, "update-not-found-other").await;
    let connection_id = create_seeded_connection(&emulator, &seed).await;

    // Well-formed ids the store never minted.
    let unknown_network = "global-network-00000000000000099".to_string();
    let unknown_connection = "connection-00000000000000099".to_string();

    let rows = vec![
        (
            "a global network that never existed",
            unknown_network.clone(),
            connection_id.clone(),
            unknown_network.clone(),
            GLOBAL_NETWORK,
        ),
        (
            "a live connection addressed through another global network",
            other.network.clone(),
            connection_id.clone(),
            connection_id.clone(),
            CONNECTION,
        ),
        (
            "a connection that never existed",
            seed.network.clone(),
            unknown_connection.clone(),
            unknown_connection.clone(),
            CONNECTION,
        ),
    ];

    for (case, network, connection, resource_id, resource_type) in rows {
        let (status, answer) = update_connection(
            &emulator,
            &network,
            &connection,
            json!({ "Description": "must not be stored" }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: wrong error code in {answer}"
        );
        assert_eq!(
            answer["Message"].as_str(),
            Some(NOT_FOUND_MESSAGE),
            "case {case}: expected the constant message {NOT_FOUND_MESSAGE:?}, got {answer}"
        );
        assert_eq!(
            answer["ResourceId"].as_str(),
            Some(resource_id.as_str()),
            "case {case}: expected ResourceId {resource_id:?}, got {answer}"
        );
        assert_eq!(
            answer["ResourceType"].as_str(),
            Some(resource_type),
            "case {case}: expected ResourceType {resource_type:?}, got {answer}"
        );
    }

    // Every refusal above stored nothing: the live connection still holds what it was
    // seeded with, read back through its own global network.
    let stored = read_connection(&emulator, &seed.network, &connection_id).await;
    assert_eq!(
        stored["Description"],
        json!(BASE_DESCRIPTION),
        "case refused updates: the connection must keep its stored Description, got {stored}"
    );
}

#[tokio::test]
async fn update_connection_refuses_a_link_reference_the_global_network_does_not_hold() {
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "update-unknown-link").await;
    let other = fixture(&emulator, "update-unknown-link-other").await;
    let connection_id = create_seeded_connection(&emulator, &seed).await;

    // Each row supplies a link reference this global network does not hold: one that never
    // existed, and a live link of another global network, on each of the two members.
    let rows = vec![
        (
            "a LinkId that never existed",
            json!({ "LinkId": "link-00000000000000099" }),
            "linkId",
        ),
        (
            "a ConnectedLinkId that never existed",
            json!({ "ConnectedLinkId": "link-00000000000000099" }),
            "connectedLinkId",
        ),
        (
            "a LinkId of another global network",
            json!({ "LinkId": other.link }),
            "linkId",
        ),
        (
            "a ConnectedLinkId of another global network",
            json!({ "ConnectedLinkId": other.connected_link }),
            "connectedLinkId",
        ),
        (
            "an unresolvable link beside a valid Description",
            json!({ "Description": "must not be stored", "LinkId": other.link }),
            "linkId",
        ),
    ];

    for (case, body, field) in rows {
        let (status, answer) =
            update_connection(&emulator, &seed.network, &connection_id, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: wrong error code in {answer}"
        );
        assert_eq!(
            answer["Reason"].as_str(),
            Some("FieldValidationFailed"),
            "case {case}: expected a field failure, got {answer}"
        );
        assert_eq!(
            answer["Fields"][0]["Name"].as_str(),
            Some(field),
            "case {case}: expected the refusal to name {field:?}, got {answer}"
        );
    }

    // Every refusal above stored nothing.
    let stored = read_connection(&emulator, &seed.network, &connection_id).await;
    assert_eq!(
        stored["Description"],
        json!(BASE_DESCRIPTION),
        "case refused updates: the connection must keep its stored Description, got {stored}"
    );
    assert_eq!(
        stored["LinkId"],
        json!(seed.link.clone()),
        "case refused updates: the connection must keep its stored LinkId, got {stored}"
    );
    assert_eq!(
        stored["ConnectedLinkId"],
        json!(seed.connected_link.clone()),
        "case refused updates: the connection must keep its stored ConnectedLinkId, got {stored}"
    );
}

#[tokio::test]
async fn get_connections_filters_on_either_end_and_pages_through_the_global_networks_records() {
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "get-connections").await;
    let third = create_device_ok(&emulator, &seed.network, json!({ "Description": "third" })).await;

    // Three connections over three devices, so each device sits at the first end of one
    // connection and the second end of another.
    let first_to_second = id_of(
        &create_connection_ok(
            &emulator,
            &seed.network,
            json!({ "DeviceId": seed.device, "ConnectedDeviceId": seed.connected_device }),
        )
        .await,
    );
    let second_to_third = id_of(
        &create_connection_ok(
            &emulator,
            &seed.network,
            json!({ "DeviceId": seed.connected_device, "ConnectedDeviceId": third }),
        )
        .await,
    );
    let third_to_first = id_of(
        &create_connection_ok(
            &emulator,
            &seed.network,
            json!({ "DeviceId": third, "ConnectedDeviceId": seed.device }),
        )
        .await,
    );

    // A connection of another global network, which no listing of this one may reach.
    let elsewhere = fixture(&emulator, "get-connections-other").await;
    create_connection_ok(
        &emulator,
        &elsewhere.network,
        json!({ "DeviceId": elsewhere.device, "ConnectedDeviceId": elsewhere.connected_device }),
    )
    .await;

    let mut every = vec![
        first_to_second.clone(),
        second_to_third.clone(),
        third_to_first.clone(),
    ];
    every.sort();

    // The rows differ along WHICH filter the query carries and how many records it leaves:
    // none, each device (matched at either end), an id list of one, of two, and one naming
    // a connection of another global network.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        ("no filter at all", String::new(), every.clone()),
        (
            "the first device, at the first end of one connection and the second of another",
            format!("deviceId={}", seed.device),
            sorted(vec![first_to_second.clone(), third_to_first.clone()]),
        ),
        (
            "the second device",
            format!("deviceId={}", seed.connected_device),
            sorted(vec![first_to_second.clone(), second_to_third.clone()]),
        ),
        (
            "the third device",
            format!("deviceId={third}"),
            sorted(vec![second_to_third.clone(), third_to_first.clone()]),
        ),
        (
            "a device of another global network",
            format!("deviceId={}", elsewhere.device),
            Vec::new(),
        ),
        (
            "one id",
            format!("connectionIds={second_to_third}"),
            vec![second_to_third.clone()],
        ),
        (
            "two ids",
            format!("connectionIds={first_to_second}&connectionIds={third_to_first}"),
            sorted(vec![first_to_second.clone(), third_to_first.clone()]),
        ),
        (
            "an id this global network does not hold",
            "connectionIds=connection-00000000000000099".to_string(),
            Vec::new(),
        ),
        (
            "an id list and a device that disagree",
            format!("connectionIds={second_to_third}&deviceId={}", seed.device),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in rows {
        let (status, body) = get_connections(&emulator, &seed.network, &query).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            ids_in(&body),
            expected,
            "case {case}: expected {expected:?}, got {body}"
        );
    }

    // `MaxResults` is modelled `range 1..500`, so a page size either side of the record
    // count and each published bound walks every record exactly once, in key order.
    for page_size in [1, 2, 3, 4, 500] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = format!("maxResults={page_size}");
            if let Some(next) = &token {
                query.push_str(&format!("&nextToken={next}"));
            }
            let (status, body) = get_connections(&emulator, &seed.network, &query).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size}: expected 200, got {status} with {body}"
            );
            let page = ids_in(&body);
            assert!(
                !page.is_empty(),
                "case maxResults={page_size}: a page while records remain is never empty, got {body}"
            );
            assert!(
                page.len() <= page_size,
                "case maxResults={page_size}: page of {} exceeds the requested size, got {body}",
                page.len()
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= every.len() + 1,
                "case maxResults={page_size}: pagination did not terminate after {pages} pages"
            );
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, every,
            "case maxResults={page_size}: expected every record exactly once in key order, got {collected:?}"
        );
    }

    // A global network the store does not hold is the missing-resource answer, not an
    // empty page.
    let (status, answer) =
        get_connections(&emulator, "global-network-00000000000000099", "").await;
    assert_eq!(
        status, 404,
        "case an unknown global network: expected 404, got {status} with {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "case an unknown global network: wrong error code in {answer}"
    );

    // Listing is a read: a later caller sees the same records.
    let (_, again) = get_connections(&emulator, &seed.network, "").await;
    assert_eq!(
        ids_in(&again),
        every,
        "case repeated listing: a read must not change what a later caller sees, got {again}"
    );
}

/// `ids` sorted, which is the key order `GetConnections` lists in.
fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}
