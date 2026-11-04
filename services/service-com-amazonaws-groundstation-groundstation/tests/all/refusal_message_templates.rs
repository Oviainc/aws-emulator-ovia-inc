// SPDX-License-Identifier: CC0-1.0

//! The TEXT this service refuses with, end to end over HTTP.
//!
//! The RULE under test: a refusal carries the sentence the SERVICE emits for the error
//! class the operation raised — its words fixed for every request that reaches that
//! branch, and its only variable parts the identifiers the request itself carried, where
//! that sentence has a slot for one. So nothing else may vary with the request or with the
//! environment: not the index of the list item at fault, not the account the call was
//! scoped to, not the wording of a neighbouring resource's refusal, and a sentence with no
//! slot echoes nothing of the request back at all.
//!
//! The suites assert that over a SET of inputs per operation, the rows differing along
//! what the sentence is claimed to be independent of:
//!
//! * `DescribeContactVersion` answers one text for an unresolvable `(contactId,
//!   versionId)` pair, whichever of the two labels is the one that does not resolve;
//! * `UpdateContact` names the `{contactId}` it was given and nothing else, over ids that
//!   name no contact crossed with every member an update can carry;
//! * the `securityDetails` gate of `CreateDataflowEndpointGroup` answers one sentence
//!   whichever item of `endpointDetails` omitted the member;
//! * `GetDataflowEndpointGroup` and `DeleteDataflowEndpointGroup` answer one sentence for
//!   a group id that was deleted and for one that never existed;
//! * `DeleteMissionProfile` names the `{missionProfileId}` it could not find;
//! * every operation that resolves a ground station by name answers one sentence, over
//!   names differing in length, case and spelling.
//!
//! Every request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — on the harness
//! `scheduling_loop.rs` stands up.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::contact_satellite_reference_integrity::onboard_satellite_arns;
use crate::dataflow_endpoint_groups::{create_group, delete_group, get_group, security_details};
use crate::minute_usage::reserve_pass;
// The not-found shape (HTTP 434 + `ResourceNotFoundException`) and the ids that name
// nothing are asserted through the suite that already owns them, so this one adds texts
// rather than a second notion of what a refusal is.
use crate::not_found_messages::{not_found_message, ABSENT_UUIDS};
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, stock_ground_stations, Reply, ABSENT_UUID,
    AFTER_SETTLE, STATION,
};

/// The window the reservations here book: far enough ahead that a contact stays scheduled
/// for the whole run.
const CONTACT_START: i64 = 2_100_000_000;
const CONTACT_SECONDS: i64 = 600;

/// Read `reply` as a bad-parameter refusal of this service — `InvalidParameterException`,
/// HTTP 431 — and answer its `message`.
///
/// `pub(crate)` for the same reason `crate::not_found_messages::not_found_message` is: every
/// suite that reads a refusal TEXT asserts the status and the error shape through this ONE
/// helper (`crate::identifier_parameter_refusals` and `crate::satellite_reads` do), so the
/// target cannot drift into two notions of what a bad-parameter answer is.
pub(crate) fn bad_parameter_message(case: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, 431,
        "case {case}: expected the modelled InvalidParameterException status 431, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "InvalidParameterException",
        "case {case}: wrong error shape, got {}",
        reply.body
    );
    reply.string("message")
}

/// `GET /contact/{contactId}/versions/{versionId}`.
async fn describe_version(router: &MultiServiceRouter, contact_id: &str, version_id: i64) -> Reply {
    send(
        router,
        "GET",
        &format!("/contact/{contact_id}/versions/{version_id}"),
        None,
    )
    .await
}

/// `POST /contact/{contactId}/versions`.
async fn update_contact(router: &MultiServiceRouter, contact_id: &str, body: Value) -> Reply {
    send(
        router,
        "POST",
        &format!("/contact/{contact_id}/versions"),
        Some(body),
    )
    .await
}

/// `DELETE /missionprofile/{missionProfileId}`.
async fn delete_profile(router: &MultiServiceRouter, mission_profile_id: &str) -> Reply {
    send(
        router,
        "DELETE",
        &format!("/missionprofile/{mission_profile_id}"),
        None,
    )
    .await
}

/// The `{missionProfileId}` an ARN this service minted addresses.
fn id_of(arn: &str) -> String {
    arn.split('/')
        .next_back()
        .expect("an ARN this service minted ends in the resource id")
        .to_string()
}

/// One `EndpointDetails` carrying a `DataflowEndpoint` — the endpoint kind whose streams
/// Ground Station places network interfaces for, so the only kind `securityDetails` is
/// demanded of (`crate::dataflow_endpoint_groups`) — with the member when `with_security`.
fn dataflow_endpoint(name: &str, port: i64, with_security: bool) -> Value {
    let mut item = json!({
        "endpoint": {
            "name": name,
            "address": {"name": "172.31.0.10", "port": port},
            "mtu": 1500,
        },
    });
    if with_security {
        item["securityDetails"] = security_details();
    }
    item
}

#[tokio::test]
async fn describe_contact_version_answers_one_text_for_either_unresolvable_label() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);
    let profile_arn = create_mission_profile(&router, "version refusals", 180, None, None).await;
    let contact_id = reserve_pass(
        &router,
        &profile_arn,
        "the contact the rows address",
        CONTACT_START,
        CONTACT_SECONDS,
    )
    .await;
    std::thread::sleep(AFTER_SETTLE);

    // The rows: a `versionId` inside the modelled range (1..128 —
    // `smithy_query(query="shape", shape="VersionId")`) that this one-version contact
    // holds no version at, and a `contactId` naming no contact at all. Both label the
    // SAME resource — the contact version this route addresses — so both answer the one
    // text, and neither answers the missing-CONTACT text `DescribeContact` carries.
    let first = i64::from(state::FIRST_VERSION_ID);
    let last = i64::from(state::MAX_CONTACT_VERSIONS);
    let mut rows: Vec<(String, String, i64)> = vec![];
    for version_id in [first + 1, 7, 64, last] {
        rows.push((
            format!("the contact holds no version {version_id}"),
            contact_id.clone(),
            version_id,
        ));
    }
    for absent in ABSENT_UUIDS {
        assert_ne!(
            *absent, contact_id,
            "case the contact {absent}: this row must name no contact"
        );
        rows.push((
            format!("no contact {absent}"),
            (*absent).to_string(),
            first,
        ));
    }

    for (case, addressed_contact, version_id) in rows {
        let reply = describe_version(&router, &addressed_contact, version_id).await;
        let message = not_found_message(&case, &reply);
        assert_eq!(
            message, "Contact version not found",
            "case {case}: the refusal names the contact version this operation addresses, \
             got {message:?}"
        );
        assert!(
            !message.contains(&addressed_contact) && !message.contains(&version_id.to_string()),
            "case {case}: this sentence has no slot, so it echoes neither label back, got \
             {message:?}"
        );
    }

    // The pair that DOES resolve is described, so the rows above are refused for the
    // version they named and not because every read of this route is.
    let described = describe_version(&router, &contact_id, first).await;
    assert_eq!(
        described.status, 200,
        "case the version the contact holds: expected 200, got {} {}",
        described.status, described.body
    );
}

#[tokio::test]
async fn update_contact_names_the_contact_id_and_nothing_of_the_environment() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);
    let satellite_arn = onboard_satellite_arns(&engine, 1)[0].clone();

    // Rows cross the ids that name no contact with every member an update may carry: the
    // contact is resolved before anything about the request is judged, so each answers the
    // same text.
    let bodies: Vec<(&str, Value)> = vec![
        ("satelliteArn alone", json!({"satelliteArn": satellite_arn})),
        (
            "trackingOverrides alone",
            json!({"trackingOverrides": {
                "programTrackSettings": {"azEl": {"ephemerisId": ABSENT_UUID}},
            }}),
        ),
        (
            "both members",
            json!({
                "satelliteArn": satellite_arn,
                "trackingOverrides": {
                    "programTrackSettings": {"tle": {"ephemerisId": ABSENT_UUID}},
                },
            }),
        ),
        ("neither member", json!({})),
    ];

    for absent in ABSENT_UUIDS {
        for (label, body) in &bodies {
            let case = format!("{absent} with {label}");
            let reply = update_contact(&router, absent, body.clone()).await;
            let message = not_found_message(&case, &reply);
            assert_eq!(
                message,
                format!("Contact not found: {absent}"),
                "case {case}: the refusal is the fixed text plus the id the path carried, \
                 got {message:?}"
            );
            assert!(
                !message.replace(absent, "").contains(engine::EMULATOR_ACCOUNT_ID),
                "case {case}: outside the id slot nothing of the environment belongs in the \
                 text, got {message:?}"
            );
        }
    }

    // A contact that DOES exist takes the same update, so the rows above are refused for
    // the id they named and not because every update is.
    let profile_arn = create_mission_profile(&router, "update refusals", 180, None, None).await;
    let contact_id = reserve_pass(
        &router,
        &profile_arn,
        "the contact the control updates",
        CONTACT_START,
        CONTACT_SECONDS,
    )
    .await;
    std::thread::sleep(AFTER_SETTLE);
    let accepted = update_contact(&router, &contact_id, bodies[0].1.clone()).await;
    assert_eq!(
        accepted.status, 200,
        "case the contact the account holds: expected 200, got {} {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.string("contactId"),
        contact_id,
        "case the contact the account holds: the accepted update answers that contact, got {}",
        accepted.body
    );
}

#[tokio::test]
async fn the_security_details_gate_answers_one_sentence_whichever_item_omits_it() {
    let (router, _engine) = harness_with_engine();

    // Rows differ in WHERE in `endpointDetails` the omission is and how long the list is:
    // the sentence names the shape and the member, so it is the same for all of them.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the only item",
            json!([dataflow_endpoint("downlink-0", 55_888, false)]),
        ),
        (
            "the second of two",
            json!([
                dataflow_endpoint("downlink-0", 55_888, true),
                dataflow_endpoint("downlink-1", 55_889, false),
            ]),
        ),
        (
            "the last of three",
            json!([
                dataflow_endpoint("downlink-0", 55_888, true),
                dataflow_endpoint("downlink-1", 55_889, true),
                dataflow_endpoint("downlink-2", 55_890, false),
            ]),
        ),
    ];

    for (case, endpoint_details) in rows {
        let reply = create_group(&router, json!({"endpointDetails": endpoint_details})).await;
        let message = bad_parameter_message(case, &reply);
        assert_eq!(
            message, "EndpointDetails securityDetails cannot be null",
            "case {case}: the refusal is this error class's fixed sentence, got {message:?}"
        );
        assert!(
            !message.contains('['),
            "case {case}: the item's index is not part of the sentence, got {message:?}"
        );
        assert_eq!(
            reply.field("parameterName"),
            &json!("securityDetails"),
            "case {case}: the refusal must name the member at fault, got {}",
            reply.body
        );
    }

    // The same list WITH the member is created, so the rows above are refused for the
    // omission and not because the shape of the request is rejected.
    let accepted = create_group(
        &router,
        json!({"endpointDetails": [dataflow_endpoint("downlink-0", 55_888, true)]}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case the member is present: expected 200, got {} {}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn both_dataflow_endpoint_group_operations_answer_one_sentence_for_a_group_that_is_gone() {
    let (router, _engine) = harness_with_engine();

    // A group created and then deleted, so the rows cover a group that USED to exist as
    // well as ids that never named one.
    let created = create_group(
        &router,
        json!({"endpointDetails": [dataflow_endpoint("downlink-0", 55_888, true)]}),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the group to delete: expected CreateDataflowEndpointGroup 200, got {} {}",
        created.status, created.body
    );
    let deleted_id = created.string("dataflowEndpointGroupId");
    let removed = delete_group(&router, &deleted_id).await;
    assert_eq!(
        removed.status, 200,
        "case the group to delete: expected DeleteDataflowEndpointGroup 200, got {} {}",
        removed.status, removed.body
    );

    let mut ids: Vec<String> = ABSENT_UUIDS.iter().map(|id| (*id).to_string()).collect();
    ids.push(deleted_id);

    for id in &ids {
        for (operation, reply) in [
            ("GetDataflowEndpointGroup", get_group(&router, id).await),
            (
                "DeleteDataflowEndpointGroup",
                delete_group(&router, id).await,
            ),
        ] {
            let case = format!("{operation} on {id}");
            let message = not_found_message(&case, &reply);
            assert_eq!(
                message, "The specified group does not exist.",
                "case {case}: the refusal is this service's fixed sentence for a group it \
                 does not hold, got {message:?}"
            );
            assert!(
                !message.contains(id),
                "case {case}: this sentence has no slot, so it echoes no id back, got \
                 {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn delete_mission_profile_names_the_profile_id_it_could_not_find() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);

    // A profile created and then deleted, beside ids that never named one: the delete of a
    // profile that is already gone reads exactly as the delete of one that never was.
    let profile_arn = create_mission_profile(&router, "delete refusals", 180, None, None).await;
    let deleted_id = id_of(&profile_arn);
    let removed = delete_profile(&router, &deleted_id).await;
    assert_eq!(
        removed.status, 200,
        "case the profile to delete: expected DeleteMissionProfile 200, got {} {}",
        removed.status, removed.body
    );

    let mut ids: Vec<String> = ABSENT_UUIDS.iter().map(|id| (*id).to_string()).collect();
    ids.push(deleted_id);

    for id in &ids {
        let reply = delete_profile(&router, id).await;
        let message = not_found_message(id, &reply);
        assert_eq!(
            message,
            format!("Mission Profile {id} not found"),
            "case {id}: the refusal is the fixed words around the id the path carried, got \
             {message:?}"
        );
        assert!(
            !message.replace(id.as_str(), "").contains(engine::EMULATOR_ACCOUNT_ID),
            "case {id}: outside the id slot nothing of the environment belongs in the text, \
             got {message:?}"
        );
    }

    // A profile that IS held is deleted instead, so the rows above are refused for the id
    // they named and not because every delete is.
    let held = create_mission_profile(&router, "held profile", 180, None, None).await;
    let accepted = delete_profile(&router, &id_of(&held)).await;
    assert_eq!(
        accepted.status, 200,
        "case the profile the account holds: expected 200, got {} {}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn every_operation_that_resolves_a_ground_station_answers_one_sentence_for_an_unknown_one() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);
    // An availability search resolves the satellite BEFORE the ground station, so the row
    // that goes through `ListContacts` names one this account is on-boarded to.
    let satellite_arn = onboard_satellite_arns(&engine, 1)[0].clone();
    let profile_arn = create_mission_profile(&router, "station refusals", 180, None, None).await;

    // `GroundStationName`s (length 4..97, pattern `^[ a-zA-Z0-9-._:=]{4,97}$` —
    // `smithy_query(query="operation", operation="ListAntennas")`) that name no ground
    // station this account is served by: one nothing records, each published length bound,
    // a known name in another case, and one carrying a space (so the row also travels
    // percent-encoded through the two route-bound listings).
    let longest = "Z".repeat(97);
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a name nothing records",
            "Bahrain-1".to_string(),
            "Bahrain-1".to_string(),
        ),
        (
            "the shortest name the model admits",
            "Abcd".to_string(),
            "Abcd".to_string(),
        ),
        (
            "the longest name the model admits",
            longest.clone(),
            longest,
        ),
        (
            "a known name in another case",
            STATION.to_lowercase(),
            STATION.to_lowercase(),
        ),
        (
            "a name carrying a space",
            "Punta%20Arenas%202".to_string(),
            "Punta Arenas 2".to_string(),
        ),
    ];

    for (case, segment, name) in rows {
        // The four ways a request names a ground station: the two listings bound under the
        // `{groundStationId}` label, the reservation that holds antenna time at one, and
        // the availability search that offers windows at one.
        let window = (CONTACT_START, CONTACT_START + CONTACT_SECONDS);
        let attempts: Vec<(&str, &str, Reply)> = vec![
            (
                "ListAntennas",
                "groundStationId",
                send(&router, "GET", &format!("/groundstation/{segment}/antenna"), None).await,
            ),
            (
                "ListGroundStationReservations",
                "groundStationId",
                send(
                    &router,
                    "GET",
                    &format!(
                        "/groundstation/{segment}/reservation?startTime={}&endTime={}",
                        window.0, window.1
                    ),
                    None,
                )
                .await,
            ),
            (
                "ReserveContact",
                "groundStation",
                send(
                    &router,
                    "POST",
                    "/contact",
                    Some(json!({
                        "missionProfileArn": profile_arn,
                        "groundStation": name,
                        "startTime": window.0,
                        "endTime": window.1,
                    })),
                )
                .await,
            ),
            (
                "ListContacts",
                "groundStation",
                send(
                    &router,
                    "POST",
                    "/contacts",
                    Some(json!({
                        "statusList": ["AVAILABLE"],
                        "missionProfileArn": profile_arn,
                        "satelliteArn": satellite_arn,
                        "groundStation": name,
                        "startTime": window.0,
                        "endTime": window.1,
                    })),
                )
                .await,
            ),
        ];

        for (operation, parameter, reply) in attempts {
            let case = format!("{operation} for {case} ({name})");
            let message = bad_parameter_message(&case, &reply);
            assert_eq!(
                message,
                "Invalid request: Ground station is invalid, does not exist, or you do not \
                 have access to it",
                "case {case}: the refusal is this service's fixed sentence for a station it \
                 cannot resolve, got {message:?}"
            );
            assert!(
                !message.contains(&name),
                "case {case}: this sentence has no slot, so it echoes no name back, got \
                 {message:?}"
            );
            assert_eq!(
                reply.string("parameterName"),
                parameter,
                "case {case}: the refusal must name the member at fault, got {}",
                reply.body
            );
        }
    }

    // The station this account IS served by is answered, so the rows above are refused for
    // the name they carried and not because every such request is.
    let served = send(
        &router,
        "GET",
        &format!("/groundstation/{STATION}/antenna"),
        None,
    )
    .await;
    assert_eq!(
        served.status, 200,
        "case the station the account is served by: expected 200, got {} {}",
        served.status, served.body
    );
}
