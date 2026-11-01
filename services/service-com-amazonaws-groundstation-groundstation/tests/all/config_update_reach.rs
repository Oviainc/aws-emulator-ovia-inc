// SPDX-License-Identifier: CC0-1.0

//! How far an `UpdateConfig` (`PUT /config/{configType}/{configId}`) reaches, end to end
//! over HTTP through the same front door as the rest of this target's suites.
//!
//! The RULE under test: "Updating a `Config` will not update the execution parameters for
//! existing future contacts scheduled with this `Config`" (`smithy_query(query="doc",
//! shape="UpdateConfig")`). A config's `configData` IS the execution parameters of every
//! dataflow end its ARN is wired to — the endpoint a `dataflowEndpointConfig` selects and
//! the region that end sits in, the bucket and key template an `s3RecordingConfig` records
//! under — so an update of a config leaves every contact already scheduled with it that is
//! still to be flown on the parameters it was scheduled with, while the config itself, and
//! every contact scheduled after the update, carries the new ones. A contact whose pass is
//! over or whose reservation was released has no future execution to protect and goes on
//! reading the config.
//!
//! The suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * the FORMS the update can take: each `configData` member a contact's dataflow details
//!   are built from, moved one at a time, plus an update that stores the parameters the
//!   config already holds;
//! * the whole `ContactStatus` enum (the one table of it this target keeps,
//!   `crate::mission_profile_deletion::CONTACT_STATUSES`): a contact in each value in turn,
//!   with the parameters kept for every non-terminal status and the config's own read for
//!   every terminal one;
//! * a config updated TWICE, where each contact keeps the parameters in force when IT was
//!   scheduled;
//! * what the kept parameters still DEPEND on: the dataflow endpoint group holding the
//!   endpoint a kept `dataflowEndpointName` selects is still in use by that contact, so its
//!   delete is refused until the contact is released.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_group_in_use_guard::group_holding;
use crate::dataflow_endpoint_groups::{delete_group, get_group};
use crate::minute_usage::{epoch, reserve_pass};
use crate::mission_profile_deletion::{put_in_status, CONTACT_STATUSES};
use crate::mission_profile_update_reach::{dataflows, version_dataflows};
use crate::scheduling_loop::{
    create_config, create_tracking_config, harness_with_engine, send, spectrum_config,
    stock_ground_stations, REGION, STATION,
};
use service_com_amazonaws_groundstation_groundstation::state::TERMINAL_CONTACT_STATUSES;

/// How long each reserved pass runs.
const PASS_SECONDS: i64 = 20 * 60;

/// How far apart two reserved windows sit. An antenna serves one pass at a time, so every
/// contact in a suite is reserved in a window of its own.
const WINDOW_STRIDE: i64 = 86_400;

/// A region other than the one these requests are signed for, for the row that moves an
/// endpoint's `dataflowEndpointRegion`: a dataflow endpoint may sit in a region of its own,
/// which is what the contact's `dataflowDestinationRegion` then reports.
const OTHER_REGION: &str = "us-west-2";

/// An `s3RecordingConfig` recording to `bucket` under `prefix`.
///
/// `prefix` is an `S3KeyPrefix` (`smithy_query(query="shape", shape="S3RecordingConfig")`),
/// and it is the key template a contact's `s3RecordingDetails` reports.
fn s3_recording(bucket: &str, prefix: &str) -> Value {
    json!({"s3RecordingConfig": {
        "bucketArn": format!("arn:aws:s3:::{bucket}"),
        "roleArn": "arn:aws:iam::000000000000:role/GroundStationS3Role",
        "prefix": prefix,
    }})
}

/// A `dataflowEndpointConfig` selecting the endpoint `name` in `region`.
fn endpoint_config(name: &str, region: &str) -> Value {
    json!({"dataflowEndpointConfig": {
        "dataflowEndpointName": name,
        "dataflowEndpointRegion": region,
    }})
}

/// One form an `UpdateConfig` of a dataflow destination can take: the `configData` the
/// config is created with, and the one the update stores for it.
struct Form {
    case: &'static str,
    /// Built from the row's slot, so each row's configs and endpoint names are its own.
    reserved: fn(usize) -> Value,
    updated: fn(usize) -> Value,
    /// Whether the update moves something a contact's dataflow details report at all — an
    /// update that stores the parameters the config already holds moves nothing.
    observable: bool,
}

/// `CreateMissionProfile` wiring ONE dataflow edge from a fresh `antenna-downlink` source
/// into `destination_arn`; answers the `missionProfileArn`.
///
/// `antenna-downlink` into a dataflow endpoint or an S3 recording is a dataflow this
/// service admits (`state::DATAFLOW_EDGE_CAPABILITY_PAIRS`), so a contact reserved against
/// this profile reports one `dataflowList` entry whose destination is the config under test.
async fn profile_into(router: &MultiServiceRouter, name: &str, destination_arn: &str) -> String {
    let (_, _, source_arn) = create_config(
        router,
        &format!("{name} downlink"),
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
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
            "dataflowEdges": [[source_arn, destination_arn]],
            "trackingConfigArn": tracking_arn,
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup {name}: expected CreateMissionProfile 200, got {} {}",
        created.status, created.body
    );
    let profile_id = created.string("missionProfileId");
    let read = send(
        router,
        "GET",
        &format!("/missionprofile/{profile_id}"),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "setup {name}: expected GetMissionProfile 200, got {} {}",
        read.status, read.body
    );
    read.string("missionProfileArn")
}

/// `UpdateConfig` with `config_data`, asserting it was accepted and answered the config it
/// updated.
async fn update_config(
    router: &MultiServiceRouter,
    case: &str,
    config_type: &str,
    config_id: &str,
    config_data: &Value,
) {
    let updated = send(
        router,
        "PUT",
        &format!("/config/{config_type}/{config_id}"),
        Some(json!({"name": format!("updated {case}"), "configData": config_data})),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {case}: expected UpdateConfig 200, got {} {}",
        updated.status, updated.body
    );
    assert_eq!(
        updated.string("configId"),
        config_id,
        "case {case}: the update must answer the config it updated, got {}",
        updated.body
    );
}

/// The `configData` `GetConfig` reports for a config.
async fn stored_config_data(
    router: &MultiServiceRouter,
    case: &str,
    config_type: &str,
    config_id: &str,
) -> Value {
    let read = send(
        router,
        "GET",
        &format!("/config/{config_type}/{config_id}"),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected GetConfig 200, got {} {}",
        read.status, read.body
    );
    read.field("configData").clone()
}

/// The names of the two endpoints row `slot` wires: the one its config selects when the
/// contact is scheduled, and the one the update repoints it to. Both are held by a group of
/// the row's own, so each resolves to an endpoint whichever one a read selects.
fn endpoint_names(slot: usize) -> (String, String) {
    (format!("alpha-{slot}"), format!("beta-{slot}"))
}

#[tokio::test]
async fn an_update_of_a_config_does_not_move_the_parameters_of_contacts_already_scheduled() {
    let (router, engine) = harness_with_engine();
    // The account is served by the station every pass below is booked at.
    stock_ground_stations(&engine, STATION);

    // Rows differ along WHICH `configData` member the update moves — every member a
    // contact's dataflow details are built from, one row each (`dataflow_end` reads the
    // endpoint selection and its region, and an `s3RecordingConfig`'s bucket and prefix) —
    // plus an update that carries the parameters the config already holds.
    let rows = [
        Form {
            case: "the s3 recording key template",
            reserved: |slot| s3_recording(&format!("bucket-{slot}"), "downlink/alpha"),
            updated: |slot| s3_recording(&format!("bucket-{slot}"), "downlink/beta"),
            observable: true,
        },
        Form {
            case: "the s3 recording bucket",
            reserved: |slot| s3_recording(&format!("bucket-{slot}"), "downlink/alpha"),
            updated: |slot| s3_recording(&format!("other-bucket-{slot}"), "downlink/alpha"),
            observable: true,
        },
        Form {
            case: "the dataflow endpoint selected",
            reserved: |slot| endpoint_config(&endpoint_names(slot).0, REGION),
            updated: |slot| endpoint_config(&endpoint_names(slot).1, REGION),
            observable: true,
        },
        Form {
            case: "the region the dataflow endpoint sits in",
            reserved: |slot| endpoint_config(&endpoint_names(slot).0, REGION),
            updated: |slot| endpoint_config(&endpoint_names(slot).0, OTHER_REGION),
            observable: true,
        },
        Form {
            case: "the parameters the config already holds",
            reserved: |slot| s3_recording(&format!("bucket-{slot}"), "downlink/alpha"),
            updated: |slot| s3_recording(&format!("bucket-{slot}"), "downlink/alpha"),
            observable: false,
        },
    ];

    let first_window = epoch(2037, 3, 11, 10, 0);
    for (slot, row) in rows.iter().enumerate() {
        let case = row.case;
        let (reserved_endpoint, updated_endpoint) = endpoint_names(slot);
        // A group of the row's own holds both endpoint names, so an endpoint selection
        // resolves to an endpoint whichever of the two a read resolves.
        group_holding(
            &router,
            case,
            "endpoint",
            &[&reserved_endpoint, &updated_endpoint],
        )
        .await;

        let reserved_data = (row.reserved)(slot);
        let (config_id, config_type, config_arn) =
            create_config(&router, &format!("destination {slot}"), reserved_data.clone(), None)
                .await;
        let profile_arn = profile_into(&router, &format!("config reach {slot}"), &config_arn).await;

        let window = first_window
            + i64::try_from(slot).expect("a row index fits in i64") * 2 * WINDOW_STRIDE;
        let contact_id = reserve_pass(&router, &profile_arn, case, window, PASS_SECONDS).await;
        let before = dataflows(&router, case, &contact_id).await;
        assert_eq!(
            before.as_ref().and_then(Value::as_array).map(Vec::len),
            Some(1),
            "case {case}: the contact must report the one dataflow it was scheduled with, \
             got {before:?}"
        );

        let updated_data = (row.updated)(slot);
        update_config(&router, case, &config_type, &config_id, &updated_data).await;

        // The update landed on the config itself.
        let stored = stored_config_data(&router, case, &config_type, &config_id).await;
        assert_eq!(
            stored, updated_data,
            "case {case}: the updated config must report the parameters the update carried, \
             got {stored}"
        );
        // And a contact scheduled AFTER it is flown with those parameters — the reference
        // for what the kept parameters must NOT have become.
        let fresh_id = reserve_pass(
            &router,
            &profile_arn,
            &format!("{case} fresh"),
            window + WINDOW_STRIDE,
            PASS_SECONDS,
        )
        .await;
        let fresh = dataflows(&router, case, &fresh_id).await;
        if row.observable {
            assert_ne!(
                fresh, before,
                "case {case}: the update must be observable at all, got {fresh:?}"
            );
        } else {
            assert_eq!(
                fresh, before,
                "case {case}: an update that stores the parameters the config already holds \
                 moves nothing, got {fresh:?}"
            );
        }

        let after = dataflows(&router, case, &contact_id).await;
        assert_eq!(
            after, before,
            "case {case}: a contact still to be flown must keep the parameters it was \
             scheduled with, got {after:?}"
        );
        let version_after = version_dataflows(&router, case, &contact_id).await;
        assert_eq!(
            version_after, after,
            "case {case}: both readers of a contact's dataflows must describe one pass, got \
             {version_after:?}"
        );
    }
}

#[tokio::test]
async fn whether_an_update_reaches_a_contact_is_decided_by_the_status_it_holds() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);

    let first_window = epoch(2038, 6, 2, 9, 0);
    // Rows differ along the `ContactStatus` the contact holds when the update lands: the
    // rule is about the contacts still to be flown, which is every non-terminal one, so the
    // table spans the whole enum rather than the two statuses a reservation reaches on its
    // own.
    for (slot, status) in CONTACT_STATUSES.iter().enumerate() {
        let label = status.to_lowercase().replace('_', "-");
        // A config, a profile and a contact of their own per row, in a window of its own,
        // so one row's update cannot be what the next row observes.
        let reserved_data = s3_recording(&format!("bucket-{label}"), "downlink/alpha");
        let (config_id, config_type, config_arn) = create_config(
            &router,
            &format!("recording {label}"),
            reserved_data.clone(),
            None,
        )
        .await;
        let profile_arn = profile_into(&router, &format!("status reach {label}"), &config_arn).await;

        let window = first_window
            + i64::try_from(slot).expect("a row index fits in i64") * 2 * WINDOW_STRIDE;
        let contact_id = reserve_pass(&router, &profile_arn, status, window, PASS_SECONDS).await;
        put_in_status(&engine, &contact_id, status);
        let before = dataflows(&router, status, &contact_id).await;

        let updated_data = s3_recording(&format!("bucket-{label}"), "downlink/beta");
        update_config(&router, status, &config_type, &config_id, &updated_data).await;

        let fresh_id = reserve_pass(
            &router,
            &profile_arn,
            &format!("{status} fresh"),
            window + WINDOW_STRIDE,
            PASS_SECONDS,
        )
        .await;
        let fresh = dataflows(&router, status, &fresh_id).await;
        assert_ne!(
            fresh, before,
            "case {status}: the update must be observable at all, got {fresh:?}"
        );

        let after = dataflows(&router, status, &contact_id).await;
        if TERMINAL_CONTACT_STATUSES.contains(status) {
            assert_eq!(
                after, fresh,
                "case {status}: a contact with no pass left to fly reads the config, so it \
                 must report the config's new parameters, got {after:?}"
            );
        } else {
            assert_eq!(
                after, before,
                "case {status}: a contact still to be flown must keep the parameters it was \
                 scheduled with, got {after:?}"
            );
        }
        let version_after = version_dataflows(&router, status, &contact_id).await;
        assert_eq!(
            version_after, after,
            "case {status}: both readers of a contact's dataflows must describe one pass, got \
             {version_after:?}"
        );
    }
}

#[tokio::test]
async fn each_contact_of_a_twice_updated_config_keeps_the_parameters_of_its_own_reservation() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);

    let first = s3_recording("twice-updated", "downlink/first");
    let (config_id, config_type, config_arn) =
        create_config(&router, "twice updated recording", first, None).await;
    let profile_arn = profile_into(&router, "twice updated", &config_arn).await;

    let window = epoch(2039, 9, 14, 12, 0);
    // One contact scheduled under each set of parameters the config holds in turn, so each
    // row's expectation is what was in force when IT was scheduled.
    let before_any = reserve_pass(&router, &profile_arn, "before-any", window, PASS_SECONDS).await;
    let scheduled_first = dataflows(&router, "before-any", &before_any).await;

    update_config(
        &router,
        "first update",
        &config_type,
        &config_id,
        &s3_recording("twice-updated", "downlink/second"),
    )
    .await;
    let between = reserve_pass(
        &router,
        &profile_arn,
        "between",
        window + WINDOW_STRIDE,
        PASS_SECONDS,
    )
    .await;
    let scheduled_second = dataflows(&router, "between", &between).await;
    assert_ne!(
        scheduled_second, scheduled_first,
        "case between: the first update must be observable at all, got {scheduled_second:?}"
    );

    update_config(
        &router,
        "second update",
        &config_type,
        &config_id,
        &s3_recording("twice-updated-again", "downlink/third"),
    )
    .await;
    let after_all = reserve_pass(
        &router,
        &profile_arn,
        "after-all",
        window + 2 * WINDOW_STRIDE,
        PASS_SECONDS,
    )
    .await;
    let scheduled_third = dataflows(&router, "after-all", &after_all).await;
    assert_ne!(
        scheduled_third, scheduled_second,
        "case after-all: the second update must be observable at all, got {scheduled_third:?}"
    );

    for (case, contact_id, expected) in [
        ("before-any", &before_any, &scheduled_first),
        ("between", &between, &scheduled_second),
        ("after-all", &after_all, &scheduled_third),
    ] {
        let after = dataflows(&router, case, contact_id).await;
        assert_eq!(
            &after, expected,
            "case {case}: the contact must be flown with the parameters in force when it was \
             scheduled, got {after:?}"
        );
    }
}

#[tokio::test]
async fn the_group_holding_an_endpoint_a_kept_selection_names_is_still_in_use_by_that_contact() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);

    // Two groups, each holding the endpoint one of the two selections names: a dataflow
    // graph is flown through the endpoints of ONE group, so the selection the contact keeps
    // and the one the config moves to each have a group of their own.
    let kept_group = group_holding(&router, "kept", "endpoint", &["kept-endpoint"]).await;
    let moved_group = group_holding(&router, "moved", "endpoint", &["moved-endpoint"]).await;
    let (config_id, config_type, config_arn) = create_config(
        &router,
        "endpoint dependency",
        endpoint_config("kept-endpoint", REGION),
        None,
    )
    .await;
    let profile_arn = profile_into(&router, "endpoint dependency", &config_arn).await;

    let window = epoch(2040, 4, 22, 7, 30);
    let contact_id = reserve_pass(&router, &profile_arn, "kept", window, PASS_SECONDS).await;
    update_config(
        &router,
        "kept",
        &config_type,
        &config_id,
        &endpoint_config("moved-endpoint", REGION),
    )
    .await;

    // The contact is still to be flown through the endpoint its kept selection names, so the
    // group holding that endpoint is still in use and its delete is refused naming the
    // contact — the in-use question and the contact's own read answer one pass.
    let refused = delete_group(&router, &kept_group).await;
    assert_eq!(
        refused.status, 431,
        "case kept-endpoint: the group holding the endpoint the kept selection names must \
         still be in use, got {} {}",
        refused.status, refused.body
    );
    assert!(
        refused.string("message").contains(&contact_id),
        "case kept-endpoint: the refusal must name the contact that is still wired to it, got {}",
        refused.body
    );
    assert_eq!(
        get_group(&router, &kept_group).await.status,
        200,
        "case kept-endpoint: a refused delete must leave the group readable"
    );
    // The group the config was repointed TO holds an endpoint no contact is wired to yet,
    // since the only contact of this profile keeps the selection it was scheduled with.
    let dropped = delete_group(&router, &moved_group).await;
    assert_eq!(
        dropped.status, 200,
        "case moved-endpoint: a group no contact is wired to yet must delete, got {} {}",
        dropped.status, dropped.body
    );

    // Released, the contact has no pass left to fly: nothing is wired to the kept endpoint
    // any more and the same delete lands.
    let cancelled = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        cancelled.status, 200,
        "case release: expected CancelContact 200, got {} {}",
        cancelled.status, cancelled.body
    );
    let deleted = delete_group(&router, &kept_group).await;
    assert_eq!(
        deleted.status, 200,
        "case release: with its only holder released the delete must land, got {} {}",
        deleted.status, deleted.body
    );
}
