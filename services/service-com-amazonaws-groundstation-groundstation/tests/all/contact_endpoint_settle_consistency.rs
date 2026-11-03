// SPDX-License-Identifier: CC0-1.0

//! What the operations that report a stored dataflow endpoint answer about it, and whether
//! that answer depends on which of them read first.
//!
//! RULE under test: a reporting operation materialises every status move the clock has
//! already made due for EVERY resource its response describes — not only the resource it is
//! named after — so one stored resource reads the same through every operation that reports
//! it and no read's answer depends on another read having been taken.
//!
//! The table spans the two dimensions the rule generalises over:
//!
//! * which of the three operations that report a stored dataflow endpoint takes the FIRST
//!   read after the move fell due: `DescribeContact` and `DescribeContactVersion`, which
//!   embed the endpoint in a contact's `dataflowList`, and `GetDataflowEndpointGroup`,
//!   which reports the group holding it;
//! * which kind of endpoint is stored: the dataflow endpoint, which the create hands back
//!   already attached, and the three AWS Ground Station Agent kinds, whose health is the
//!   health of the agent registered for their group.
//!
//! Every request goes through the real front door — the router, deserialization, constraint
//! validation, the handler and serialization — on the harness `scheduling_loop.rs` stands
//! up. Each row gets a STORE OF ITS OWN, because "the first read of this endpoint" is a
//! property of the store the endpoint lives in.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    ENDPOINT_CREATED, FIRST_VERSION_ID, HEALTH_HEALTHY, HEALTH_UNHEALTHY, REASON_HEALTHY,
    REASON_NO_REGISTERED_AGENT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_groups::{create_group, details, endpoint_kinds, get_group};
use crate::scheduling_loop::{
    create_config, create_tracking_config, harness_serving, send, spectrum_config, AFTER_SETTLE,
};

/// The window every row's contact is reserved over: antenna time far enough ahead that no
/// row's reservation is refused for having already elapsed.
const CONTACT_START: i64 = 1_900_000_000;

/// The ground station every row reserves at; each row has a store of its own, so no two
/// reservations compete for the same antenna.
const GROUND_STATION: &str = "Ohio-1";

/// One operation that reports a stored dataflow endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reader {
    /// `GET /contact/{contactId}` — the endpoint inside the contact's `dataflowList`.
    DescribeContact,
    /// `GET /contact/{contactId}/versions/{versionId}` — the same list, version-scoped.
    DescribeContactVersion,
    /// `GET /dataflowEndpointGroup/{dataflowEndpointGroupId}` — the group that holds it.
    GetDataflowEndpointGroup,
}

/// Every operation of this service that reports a stored dataflow endpoint.
const READERS: [Reader; 3] = [
    Reader::DescribeContact,
    Reader::DescribeContactVersion,
    Reader::GetDataflowEndpointGroup,
];

/// What a read must report for a stored endpoint once every move the clock owed it is due.
struct Settled {
    /// `endpoint.status`, for the kind that carries a status of its own; `None` for a kind
    /// that carries none.
    status: Option<&'static str>,
    health_status: &'static str,
    health_reason: &'static str,
}

/// The settled form of an endpoint described by the `EndpointDetails` member `member`.
///
/// Source: the create's own rule — a dataflow endpoint (the `endpoint` member) is handed
/// back attached, `created` / `HEALTHY` / `HEALTHY`, whose health is the dataplane's own,
/// while an agent endpoint carries no `status` and its health is that of the agent
/// registered for its group, so one no agent has registered for stays `UNHEALTHY` /
/// `NO_REGISTERED_AGENT` however long the clock runs.
fn settled(member: &str) -> Settled {
    if member == "endpoint" {
        Settled {
            status: Some(ENDPOINT_CREATED),
            health_status: HEALTH_HEALTHY,
            health_reason: REASON_HEALTHY,
        }
    } else {
        Settled {
            status: None,
            health_status: HEALTH_UNHEALTHY,
            health_reason: REASON_NO_REGISTERED_AGENT,
        }
    }
}

/// One row of the table, provisioned: a store holding one dataflow endpoint group, one
/// mission profile whose single dataflow edge ends at that group's endpoint, and one contact
/// reserved against that profile.
struct Case {
    label: String,
    /// The reader that takes the first read of this store's endpoint.
    first: Reader,
    expected: Settled,
    router: MultiServiceRouter,
    contact_id: String,
    group_id: String,
}

/// Stand up a store holding the endpoint `member` describes, a dataflow wired to it and a
/// contact reserved against the profile carrying that dataflow; answer the router, the
/// `contactId` and the `dataflowEndpointGroupId`.
async fn provision(label: &str, member: &str, body: &Value) -> (MultiServiceRouter, String, String) {
    let router = harness_serving(GROUND_STATION);
    let endpoint_name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label}: the endpoint-kind row carries a name, got {body}"))
        .to_string();

    // Only a DATAFLOW endpoint is created with the security details Ground Station connects
    // its stream with; an agent endpoint's stream is carried by the customer's own agent.
    let created = create_group(
        &router,
        json!({"endpointDetails": [details(member, body, member == "endpoint")]}),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {label}: CreateDataflowEndpointGroup expected 200, got {} {}",
        created.status, created.body
    );
    let group_id = created.string("dataflowEndpointGroupId");

    let (_, _, downlink_arn) = create_config(
        &router,
        &format!("{label} downlink"),
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
        None,
    )
    .await;
    let (_, _, endpoint_config_arn) = create_config(
        &router,
        &format!("{label} endpoint"),
        json!({"dataflowEndpointConfig": {"dataflowEndpointName": endpoint_name}}),
        None,
    )
    .await;
    let tracking_arn = create_tracking_config(&router, &format!("{label} tracking")).await;

    let profile = send(
        &router,
        "POST",
        "/missionprofile",
        Some(json!({
            "name": label,
            "minimumViableContactDurationSeconds": 60,
            "dataflowEdges": [[downlink_arn, endpoint_config_arn]],
            "trackingConfigArn": tracking_arn,
        })),
    )
    .await;
    assert_eq!(
        profile.status, 200,
        "case {label}: CreateMissionProfile expected 200, got {} {}",
        profile.status, profile.body
    );
    let profile_id = profile.string("missionProfileId");
    let read_profile = send(&router, "GET", &format!("/missionprofile/{profile_id}"), None).await;
    assert_eq!(
        read_profile.status, 200,
        "case {label}: GetMissionProfile expected 200, got {} {}",
        read_profile.status, read_profile.body
    );
    let profile_arn = read_profile.string("missionProfileArn");

    let reserved = send(
        &router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": GROUND_STATION,
            "startTime": CONTACT_START,
            "endTime": CONTACT_START + 600,
        })),
    )
    .await;
    assert_eq!(
        reserved.status, 200,
        "case {label}: ReserveContact expected 200, got {} {}",
        reserved.status, reserved.body
    );

    (router, reserved.string("contactId"), group_id)
}

/// The stored endpoint as `reader` reports it: the `endpointDetails` a contact's one
/// dataflow carries at its destination, or the one endpoint of the group itself.
async fn reported_endpoint(case: &Case, reader: Reader) -> Value {
    let label = &case.label;
    match reader {
        Reader::DescribeContact => {
            let reply = send(
                &case.router,
                "GET",
                &format!("/contact/{}", case.contact_id),
                None,
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {label}: DescribeContact expected 200, got {} {}",
                reply.status, reply.body
            );
            endpoint_of_dataflow(&reply.body, label, "DescribeContact")
        }
        Reader::DescribeContactVersion => {
            let reply = send(
                &case.router,
                "GET",
                &format!("/contact/{}/versions/{FIRST_VERSION_ID}", case.contact_id),
                None,
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {label}: DescribeContactVersion expected 200, got {} {}",
                reply.status, reply.body
            );
            endpoint_of_dataflow(&reply.body, label, "DescribeContactVersion")
        }
        Reader::GetDataflowEndpointGroup => {
            let reply = get_group(&case.router, &case.group_id).await;
            assert_eq!(
                reply.status, 200,
                "case {label}: GetDataflowEndpointGroup expected 200, got {} {}",
                reply.status, reply.body
            );
            reply
                .field("endpointsDetails")
                .as_array()
                .and_then(|items| items.first())
                .unwrap_or_else(|| {
                    panic!(
                        "case {label}: GetDataflowEndpointGroup reports the group's one \
                         endpoint, got {}",
                        reply.body
                    )
                })
                .clone()
        }
    }
}

/// The `endpointDetails` the one dataflow of a described contact carries at its destination.
fn endpoint_of_dataflow(body: &Value, label: &str, reader: &str) -> Value {
    body.get("dataflowList")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(|detail| detail.get("destination"))
        .and_then(|destination| destination.get("configDetails"))
        .and_then(|config_details| config_details.get("endpointDetails"))
        .unwrap_or_else(|| {
            panic!(
                "case {label}: {reader} reports the dataflow endpoint its one dataflow ends \
                 at, got {body}"
            )
        })
        .clone()
}

#[tokio::test]
async fn every_reader_of_a_stored_dataflow_endpoint_reports_the_moves_the_clock_already_owed() {
    // Rows differ along which reader reads first and which kind of endpoint is stored.
    let mut cases: Vec<Case> = Vec::new();
    for kind in endpoint_kinds() {
        for first in READERS {
            let label = format!("{} read first by {first:?}", kind.label);
            let (router, contact_id, group_id) = provision(&label, kind.member, &kind.body).await;
            cases.push(Case {
                label,
                first,
                expected: settled(kind.member),
                router,
                contact_id,
                group_id,
            });
        }
    }

    // Past every span this service owes a status move, so every row's store has had any
    // owed move fall due before ANY read of it is taken.
    std::thread::sleep(AFTER_SETTLE);

    for case in &cases {
        let label = &case.label;

        // The first read of this store: whichever reader it is, it reports the endpoint as
        // the clock has already left it.
        let first = reported_endpoint(case, case.first).await;
        assert_eq!(
            first.get("endpoint").and_then(|endpoint| endpoint.get("status")),
            case.expected.status.map(|status| json!(status)).as_ref(),
            "case {label}: the first read must report the endpoint status the clock already \
             owed ({:?}), got {first:?}",
            case.expected.status
        );
        assert_eq!(
            first.get("healthStatus"),
            Some(&json!(case.expected.health_status)),
            "case {label}: the first read must report healthStatus {}, got {first:?}",
            case.expected.health_status
        );
        assert_eq!(
            first.get("healthReasons"),
            Some(&json!([case.expected.health_reason])),
            "case {label}: the first read must report healthReasons [{}], got {first:?}",
            case.expected.health_reason
        );

        // And every reader of that one stored endpoint — including the first one again,
        // after the others have read — describes it identically: no reader's answer was
        // produced, or changed, by another read having been taken.
        for reader in READERS {
            let also = reported_endpoint(case, reader).await;
            assert_eq!(
                also, first,
                "case {label}: {reader:?} must describe the same stored endpoint as \
                 {:?}, got {also:?}",
                case.first
            );
        }
        let again = reported_endpoint(case, case.first).await;
        assert_eq!(
            again, first,
            "case {label}: reading again through {:?} must answer the same, got {again:?}",
            case.first
        );
    }
}
