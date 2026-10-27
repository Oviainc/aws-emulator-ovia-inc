// SPDX-License-Identifier: CC0-1.0

//! `DeleteMissionProfile` (`DELETE /missionprofile/{missionProfileId}`) end to end over
//! HTTP, through the same front door as the rest of this target's suites.
//!
//! The RULE under test: a resource of this service is REMOVED only while nothing else it
//! holds still names it, because a read of the referring resource resolves that reference
//! — so a mission profile a contact still to be flown was reserved against is not
//! deleted. The delete is then refused with the operation's modelled in-use failure
//! (`InvalidParameterException`, HTTP 431 — the operation models no
//! `ResourceInUseException`), naming the contacts that hold it, and the store is left
//! exactly as it was; once every one of them is terminal the delete goes through and the
//! profile is gone from every read.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * the refusal over the WHOLE `ContactStatus` enum (`smithy_query(query="shape",
//!   shape="ContactStatus")`): a contact in each value in turn, reserved against a profile
//!   of its own, with the delete refused for every non-terminal status and accepted for
//!   every terminal one;
//! * the holders the refusal names, over a profile several contacts hold at once, released
//!   one at a time, with a profile no contact names deleting while they stand;
//! * what a refused delete leaves behind: the profile read back unchanged, the contact
//!   still reporting the dataflows of its pass, and the availability search for the same
//!   ARN still answered — plus the refusal of an id that names no profile at all.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    mission_profile_in_use_message, service_state, MISSION_PROFILE_NOT_FOUND,
    TERMINAL_CONTACT_STATUSES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::minute_usage::{epoch, reserve_pass};
use crate::scheduling_loop::{
    create_config, create_tracking_config, harness_with_engine, send, spectrum_config, Reply,
    ABSENT_UUID, REGION,
};

/// Every value of the model's `ContactStatus`.
///
/// Source: `smithy_query(query="shape", shape="ContactStatus",
/// model_path="services/service-com-amazonaws-groundstation-groundstation/smithy-model/\
/// com-amazonaws-groundstation-groundstation.json")` — all thirteen values, not only the
/// ones a request can reach today, because the rule is over the whole enum. Which of them
/// are the ends of the lifecycle is `state::TERMINAL_CONTACT_STATUSES`, asserted below to
/// be a subset of this list so the two cannot drift apart.
const CONTACT_STATUSES: &[&str] = &[
    "AVAILABLE",
    "AWS_CANCELLED",
    "AWS_FAILED",
    "CANCELLED",
    "CANCELLING",
    "COMPLETED",
    "FAILED",
    "FAILED_TO_SCHEDULE",
    "PASS",
    "POSTPASS",
    "PREPASS",
    "SCHEDULED",
    "SCHEDULING",
];

/// The ground station every reservation in this suite is held at.
const GROUND_STATION: &str = "Ohio-1";

/// A mission profile carrying ONE dataflow edge, as `(missionProfileId,
/// missionProfileArn)`.
///
/// The edge is a real dataflow — an `antenna-downlink` source into a `dataflow-endpoint`
/// destination — so a contact reserved against this profile reports one `dataflowList`
/// entry, which is exactly what a profile dropped out from under it would take away.
async fn profile_with_one_dataflow(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (_, _, downlink_arn) = create_config(
        router,
        &format!("{name} downlink"),
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
        None,
    )
    .await;
    let (_, _, endpoint_arn) = create_config(
        router,
        &format!("{name} endpoint"),
        json!({"dataflowEndpointConfig": {
            "dataflowEndpointName": "downlink-endpoint",
            "dataflowEndpointRegion": REGION,
        }}),
        None,
    )
    .await;
    let tracking_arn = create_tracking_config(router, &format!("{name} tracking")).await;

    let created = send(
        router,
        "POST",
        "/missionprofile",
        Some(json!({
            "name": name,
            "minimumViableContactDurationSeconds": 60,
            "dataflowEdges": [[downlink_arn, endpoint_arn]],
            "trackingConfigArn": tracking_arn,
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup {name}: expected CreateMissionProfile 200, got {} {}",
        created.status, created.body
    );
    let id = created.string("missionProfileId");
    let read = read_profile(router, &id).await;
    assert_eq!(
        read.status, 200,
        "setup {name}: expected GetMissionProfile 200, got {} {}",
        read.status, read.body
    );
    (id, read.string("missionProfileArn"))
}

/// `DeleteMissionProfile` for `mission_profile_id`.
async fn delete_profile(router: &MultiServiceRouter, mission_profile_id: &str) -> Reply {
    send(
        router,
        "DELETE",
        &format!("/missionprofile/{mission_profile_id}"),
        None,
    )
    .await
}

/// `GetMissionProfile` for `mission_profile_id`.
async fn read_profile(router: &MultiServiceRouter, mission_profile_id: &str) -> Reply {
    send(
        router,
        "GET",
        &format!("/missionprofile/{mission_profile_id}"),
        None,
    )
    .await
}

/// `DescribeContact` for `contact_id`, asserting it answered.
async fn describe_contact(router: &MultiServiceRouter, case: &str, contact_id: &str) -> Reply {
    let described = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case {case}: expected DescribeContact 200, got {} {}",
        described.status, described.body
    );
    described
}

/// How many dataflows a contact read reports, or `None` when it reports no `dataflowList`
/// at all — which is what a contact whose profile is gone can say about its wiring.
fn dataflow_count(described: &Reply) -> Option<usize> {
    described
        .body
        .get("dataflowList")
        .and_then(Value::as_array)
        .map(Vec::len)
}

/// Put the stored contact into `status`.
///
/// Most `ContactStatus` values are reached by the scheduler and by the pass itself, which
/// no request drives, so the stored contact's status is set directly — as
/// `tests/all/contact_cancellation.rs` does for the same reason.
fn put_in_status(engine: &engine::InMemoryEngine, contact_id: &str, status: &str) {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .contacts
        .get_mut(contact_id)
        .expect("the reserved contact")
        .contact_status = status.to_string();
}

#[tokio::test]
async fn a_mission_profile_is_deleted_only_when_no_contact_still_to_be_flown_names_it() {
    let (router, engine) = harness_with_engine();

    for terminal in TERMINAL_CONTACT_STATUSES {
        assert!(
            CONTACT_STATUSES.contains(terminal),
            "case enum: {terminal:?} is a terminal status this table does not span, got \
             {CONTACT_STATUSES:?}"
        );
    }

    let first_window = epoch(2033, 5, 17, 9, 0);
    for (slot, status) in CONTACT_STATUSES.iter().enumerate() {
        // A profile and a contact of their own per row, in a window of its own — an
        // antenna serves one pass at a time, so two rows cannot hold the same window at
        // one ground station — so one row's delete cannot be what the next row observes.
        let (profile_id, profile_arn) =
            profile_with_one_dataflow(&router, &format!("delete guard {status}")).await;
        let start = first_window + i64::try_from(slot).expect("a row index fits in i64") * 86_400;
        let contact_id = reserve_pass(&router, &profile_arn, status, start, 20 * 60).await;
        put_in_status(&engine, &contact_id, status);

        let deleted = delete_profile(&router, &profile_id).await;

        if TERMINAL_CONTACT_STATUSES.contains(status) {
            assert_eq!(
                deleted.status, 200,
                "case {status}: a profile only a terminal contact names must delete, got {} {}",
                deleted.status, deleted.body
            );
            assert_eq!(
                deleted.string("missionProfileId"),
                profile_id,
                "case {status}: the delete must answer the profile it removed, got {}",
                deleted.body
            );
            let gone = read_profile(&router, &profile_id).await;
            assert_eq!(
                gone.status, 434,
                "case {status}: a deleted profile must read 434, got {} {}",
                gone.status, gone.body
            );
            // The contact survives the profile it no longer depends on, and says nothing
            // about a wiring that is no longer known.
            let described = describe_contact(&router, status, &contact_id).await;
            assert_eq!(
                dataflow_count(&described),
                None,
                "case {status}: a contact whose profile is gone must report no dataflowList, \
                 got {}",
                described.body
            );
            continue;
        }

        assert_eq!(
            deleted.status, 431,
            "case {status}: a profile a contact still to be flown names must answer the \
             modelled InvalidParameterException status 431, got {} {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.error_code(),
            "InvalidParameterException",
            "case {status}: wrong error shape, got {}",
            deleted.body
        );
        assert_eq!(
            deleted.string("message"),
            mission_profile_in_use_message(std::slice::from_ref(&contact_id)),
            "case {status}: the refusal must name the contact that holds the profile, got {}",
            deleted.body
        );

        // A refused delete writes nothing: the profile is still there, and the contact
        // still reads the dataflows of its pass out of it.
        let kept = read_profile(&router, &profile_id).await;
        assert_eq!(
            kept.status, 200,
            "case {status}: a refused delete must leave the profile readable, got {} {}",
            kept.status, kept.body
        );
        assert_eq!(
            kept.string("missionProfileArn"),
            profile_arn,
            "case {status}: the kept profile must still answer its own ARN, got {}",
            kept.body
        );
        let described = describe_contact(&router, status, &contact_id).await;
        assert_eq!(
            dataflow_count(&described),
            Some(1),
            "case {status}: the contact must still report the one dataflow of its pass, got {}",
            described.body
        );
        assert_eq!(
            described.string("missionProfileArn"),
            profile_arn,
            "case {status}: the contact must still name the profile it was reserved against, \
             got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn the_refusal_names_every_holding_contact_and_the_delete_lands_once_they_are_released() {
    let (router, _engine) = harness_with_engine();
    let (profile_id, profile_arn) = profile_with_one_dataflow(&router, "held profile").await;
    // A second profile no contact is reserved against: the guard counts the contacts that
    // name THIS profile, not every contact the store holds.
    let (spare_id, _) = profile_with_one_dataflow(&router, "spare profile").await;

    let first_window = epoch(2034, 2, 3, 11, 0);
    let mut holders: Vec<String> = Vec::new();
    for slot in 0..3_i64 {
        let case = format!("holder-{slot}");
        holders.push(
            reserve_pass(
                &router,
                &profile_arn,
                &case,
                first_window + slot * 86_400,
                15 * 60,
            )
            .await,
        );
    }
    // The refusal lists the holders in a stable order, so it reads the same on every call.
    let mut expected = holders.clone();
    expected.sort();

    let refused = delete_profile(&router, &profile_id).await;
    assert_eq!(
        refused.status, 431,
        "case three-holders: expected 431, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.string("message"),
        mission_profile_in_use_message(&expected),
        "case three-holders: the refusal must name every contact that holds the profile, got {}",
        refused.body
    );

    let spare = delete_profile(&router, &spare_id).await;
    assert_eq!(
        spare.status, 200,
        "case spare: a profile no contact names must delete while other contacts stand, got {} {}",
        spare.status, spare.body
    );

    // Released one at a time: a cancel of a pass that has yet to start leaves the contact
    // terminal, so it stops holding the profile while the others still do.
    let mut still_held = expected.clone();
    for (released, holder) in holders.iter().enumerate() {
        let cancelled = send(&router, "DELETE", &format!("/contact/{holder}"), None).await;
        assert_eq!(
            cancelled.status, 200,
            "case release-{released}: expected CancelContact 200, got {} {}",
            cancelled.status, cancelled.body
        );
        still_held.retain(|contact_id| contact_id != holder);

        let attempt = delete_profile(&router, &profile_id).await;
        if still_held.is_empty() {
            assert_eq!(
                attempt.status, 200,
                "case release-{released}: with every holder released the delete must land, \
                 got {} {}",
                attempt.status, attempt.body
            );
            assert_eq!(
                attempt.string("missionProfileId"),
                profile_id,
                "case release-{released}: the delete must answer the profile it removed, got {}",
                attempt.body
            );
        } else {
            assert_eq!(
                attempt.status, 431,
                "case release-{released}: {} holders are left, so the delete must still be \
                 refused, got {} {}",
                still_held.len(),
                attempt.status,
                attempt.body
            );
            assert_eq!(
                attempt.string("message"),
                mission_profile_in_use_message(&still_held),
                "case release-{released}: the refusal must name exactly the holders that are \
                 left, got {}",
                attempt.body
            );
        }
    }

    let gone = read_profile(&router, &profile_id).await;
    assert_eq!(
        gone.status, 434,
        "case gone: the deleted profile must read 434, got {} {}",
        gone.status, gone.body
    );
    assert_eq!(
        gone.string("message"),
        MISSION_PROFILE_NOT_FOUND,
        "case gone: wrong text for a profile that is no longer held, got {}",
        gone.body
    );
}

#[tokio::test]
async fn a_refused_delete_leaves_the_profile_and_the_contact_loop_exactly_as_they_were() {
    let (router, _engine) = harness_with_engine();
    let (profile_id, profile_arn) = profile_with_one_dataflow(&router, "untouched profile").await;
    let start = epoch(2035, 7, 9, 13, 30);
    let contact_id = reserve_pass(&router, &profile_arn, "untouched", start, 10 * 60).await;

    let before = read_profile(&router, &profile_id).await;
    assert_eq!(
        before.status, 200,
        "case before: expected GetMissionProfile 200, got {} {}",
        before.status, before.body
    );

    // Repeating a refused delete is still a refusal, and still writes nothing.
    for attempt in 0..2 {
        let refused = delete_profile(&router, &profile_id).await;
        assert_eq!(
            refused.status, 431,
            "case attempt-{attempt}: expected 431, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case attempt-{attempt}: wrong error shape, got {}",
            refused.body
        );
    }

    let after = read_profile(&router, &profile_id).await;
    assert_eq!(
        after.body, before.body,
        "case untouched: a refused delete must leave the profile exactly as it was, got {}",
        after.body
    );

    let described = describe_contact(&router, "untouched", &contact_id).await;
    assert_eq!(
        dataflow_count(&described),
        Some(1),
        "case untouched: the contact must still report the one dataflow of its pass, got {}",
        described.body
    );

    // The discovery half of the scheduling loop still resolves the ARN the contact listing
    // hands back: an availability search for that profile is answered rather than refused.
    let search = send(
        &router,
        "POST",
        "/contacts",
        Some(json!({
            "statusList": ["AVAILABLE"],
            "startTime": start - 3_600,
            "endTime": start + 3_600,
            "groundStation": GROUND_STATION,
            "satelliteArn": format!(
                "arn:aws:groundstation:{REGION}:{}:satellite/\
                 11111111-1111-1111-1111-111111111111",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "missionProfileArn": profile_arn,
        })),
    )
    .await;
    assert_eq!(
        search.status, 200,
        "case availability: the profile a live contact names must still resolve for a search, \
         got {} {}",
        search.status, search.body
    );

    // An id that names no profile at all is the other refusal of this operation, and it
    // disturbs nothing either.
    let absent = delete_profile(&router, ABSENT_UUID).await;
    assert_eq!(
        absent.status, 434,
        "case absent: an id naming no profile must answer 434, got {} {}",
        absent.status, absent.body
    );
    assert_eq!(
        absent.error_code(),
        "ResourceNotFoundException",
        "case absent: wrong error shape, got {}",
        absent.body
    );
    assert_eq!(
        absent.string("message"),
        MISSION_PROFILE_NOT_FOUND,
        "case absent: wrong text for an id naming no profile, got {}",
        absent.body
    );
    let still = read_profile(&router, &profile_id).await;
    assert_eq!(
        still.status, 200,
        "case absent: the stored profile must be untouched, got {} {}",
        still.status, still.body
    );
}
