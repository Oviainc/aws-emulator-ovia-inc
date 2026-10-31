// SPDX-License-Identifier: CC0-1.0

//! What a channel's CLASS does to the per-pipeline records it already holds, end
//! to end over the restJson1 wire.
//!
//! Rule: the pipelines a channel has are the ones its CURRENT `ChannelClass`
//! gives it — "A standard channel has two encoding pipelines and a single
//! pipeline channel only has one" (`smithy_query(query="shape",
//! shape="ChannelClass")`) — so every reading of a record associated with a
//! pipeline resolves that pipeline against the class the channel has now: an
//! alert whose `PipelineId` ("The ID of the pipeline this alert is associated
//! with", `smithy_query(query="shape", shape="ChannelAlert")`) names a pipeline
//! the class has not got is reported no more than that pipeline's
//! `PipelineDetail` or `ChannelEgressEndpoint` is, whichever class the alert was
//! recorded under; the record itself is kept, so a channel put back into the
//! class that gives it that pipeline reports it again.
//!
//! This is the READ side of the rule `crate::channel_alerts` pins on the write
//! side (an alert is recorded only for a pipeline the class gives the channel),
//! so the two suites share one harness — the channel setup, the alert writer and
//! the `ListAlerts` reader come from there — and this one adds the class change
//! that moves the pipelines under an already-recorded alert.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour. The table spans the `ChannelClass` enum in
//! both directions and each class restated, the pipelines inside and outside
//! each class's range, an alert that names no pipeline at all, both
//! `ChannelAlertState` values and every documented `StateFilter`; the issue's
//! `PIPELINE_1` case is ONE row.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    channel_alert_state, class_pipelines, CHANNEL_ALERT_FILTER_ALL,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_alerts::{
    at, channel, clear, list, logged, raise, reported, reported_ids, Engine,
};
use crate::channel_lifecycle::{describe, wait_for_the_clock};
use crate::wire::{call, medialive_with_engine};

/// `UpdateChannelClass` as a client sends it.
async fn update_class(router: &MultiServiceRouter, id: &str, class: &str) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/prod/channels/{id}/channelClass"),
        Some(json!({ "channelClass": class })),
    )
    .await
}

/// One alert this suite recorded on the channel under test.
#[derive(Debug, Clone)]
struct Seeded {
    /// The `Id` the store minted for it.
    id: String,
    /// The pipeline the observation named, as `PipelineDetail$PipelineId`
    /// numbers them; `None` for an alert about the channel itself.
    pipeline: Option<i32>,
    /// Whether the alert has been cleared, which is what makes its reported
    /// `State` `CLEARED` rather than `SET`.
    cleared: bool,
}

/// Record `pipelines` as alerts on `channel_id`, in the order given, clearing
/// every second one so each row carries both `ChannelAlertState` values.
///
/// The alerts go in through the store's own writer, which is what keeps every
/// recorded alert consistent — and which refuses a pipeline the channel's class
/// has not got, so a row seeds only pipelines the class it CREATED the channel
/// with gives it.
fn seed(engine: &Engine, channel_id: &str, pipelines: &[Option<i32>]) -> Vec<Seeded> {
    let mut seeded: Vec<Seeded> = Vec::new();
    for (index, pipeline) in pipelines.iter().enumerate() {
        let offset = index as u64;
        let id = raise(
            engine,
            channel_id,
            "InputNotDetected",
            Some("the input is not delivering media"),
            *pipeline,
            at(offset),
        );
        let cleared = index % 2 == 1;
        if cleared {
            clear(engine, channel_id, &id, at(offset + 60));
        }
        seeded.push(Seeded {
            id,
            pipeline: *pipeline,
            cleared,
        });
    }
    seeded
}

/// Every documented `StateFilter` value (`ListAlertsRequest$StateFilter`: "SET -
/// Return only alerts with SET state. CLEARED - Return only alerts with CLEARED
/// state. ALL - Return all alerts."), plus the absent filter, which narrows
/// nothing.
fn filters() -> [Option<&'static str>; 4] {
    [
        None,
        Some(CHANNEL_ALERT_FILTER_ALL),
        Some(channel_alert_state::SET),
        Some(channel_alert_state::CLEARED),
    ]
}

/// Whether `filter` admits an alert in the state `cleared` describes.
fn admits(filter: Option<&str>, cleared: bool) -> bool {
    match filter {
        Some(value) if value == channel_alert_state::SET => !cleared,
        Some(value) if value == channel_alert_state::CLEARED => cleared,
        _ => true,
    }
}

/// The ids `ListAlerts` reports for `filter`, read over the wire.
async fn listed(
    router: &MultiServiceRouter,
    channel_id: &str,
    filter: Option<&str>,
    case: &str,
) -> (Vec<String>, Vec<Value>) {
    let query: Vec<(&str, String)> = filter
        .map(|filter| vec![("stateFilter", filter.to_string())])
        .unwrap_or_default();
    let (status, body) = list(router, channel_id, &query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAlerts with the filter {filter:?}: expected 200, got {status}: {body}"
    );
    (reported_ids(&body), reported(&body))
}

/// The ids of `seeded` that a channel of `class` reports under `filter`: the
/// alerts whose pipeline that class gives the channel (an alert naming none is
/// about the channel itself, so every class reports it), narrowed by the filter.
fn expected_ids(seeded: &[Seeded], class: &str, filter: Option<&str>) -> Vec<String> {
    let pipelines = class_pipelines(class);
    seeded
        .iter()
        .filter(|alert| {
            alert
                .pipeline
                .is_none_or(|pipeline| pipelines.contains(&pipeline))
                && admits(filter, alert.cleared)
        })
        .map(|alert| alert.id.clone())
        .collect()
}

/// Rule: a channel reports an alert associated with a pipeline only while its
/// class gives it that pipeline, so a class change re-scopes the alert log along
/// with the pipelines — exactly as it re-scopes `PipelineDetails` and
/// `EgressEndpoints` — and the log itself is left as the history it is.
///
/// The rows differ along the class the channel was created with, the class the
/// update stores (both directions and each class restated), and the pipelines
/// the recorded alerts name: inside the new class's range, past it, and no
/// pipeline at all. Every row is read back under every documented `StateFilter`.
#[tokio::test]
async fn a_channel_reports_only_the_alerts_of_the_pipelines_its_class_gives_it() {
    // (case, class created with, class the update stores, the pipelines the
    //  recorded alerts name — in set order)
    let table: [(&str, &str, &str, &[Option<i32>]); 6] = [
        (
            "standard to single pipeline, alerts on both pipelines and the channel",
            "STANDARD",
            "SINGLE_PIPELINE",
            &[Some(0), Some(1), None, Some(1)],
        ),
        (
            "standard to single pipeline, every alert on the dropped pipeline",
            "STANDARD",
            "SINGLE_PIPELINE",
            &[Some(1), Some(1)],
        ),
        (
            "standard to single pipeline, no alert names a pipeline",
            "STANDARD",
            "SINGLE_PIPELINE",
            &[None, None],
        ),
        (
            "standard restated, every alert survives",
            "STANDARD",
            "STANDARD",
            &[Some(0), Some(1), None],
        ),
        (
            "single pipeline to standard, the class only widens",
            "SINGLE_PIPELINE",
            "STANDARD",
            &[Some(0), None],
        ),
        (
            "single pipeline restated",
            "SINGLE_PIPELINE",
            "SINGLE_PIPELINE",
            &[Some(0), None, Some(0)],
        ),
    ];

    for (case, created_as, updated_to, pipelines) in table {
        let (router, engine) = medialive_with_engine();
        let id = channel(&router, "class-scoped-alerts", Some(created_as)).await;
        let seeded = seed(&engine, &id, pipelines);

        // Before the change the channel has the class every seeded alert was
        // recorded under, so each one is reported: what the row then asserts is
        // the difference the class change makes, not the seeding.
        for filter in filters() {
            let (ids, _) = listed(&router, &id, filter, case).await;
            assert_eq!(
                ids,
                expected_ids(&seeded, created_as, filter),
                "case {case:?}: before the class change, the filter {filter:?} must report the \
                 alerts the created class gives the channel, got {ids:?}"
            );
        }

        wait_for_the_clock();
        let (status, idle) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel: got {status}: {idle}"
        );
        assert_eq!(
            idle["state"],
            json!("IDLE"),
            "case {case:?}: expected the created channel to settle to IDLE, got {:?}",
            idle["state"]
        );

        let (status, response) = update_class(&router, &id, updated_to).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateChannelClass: expected 200, got {status}: {response}"
        );
        assert_eq!(
            response["channel"]["channelClass"],
            json!(updated_to),
            "case {case:?}: expected the stored class {updated_to:?}, got {:?}",
            response["channel"]["channelClass"]
        );

        // The alert log is read against the class the channel now has, under
        // every documented filter.
        for filter in filters() {
            let (ids, alerts) = listed(&router, &id, filter, case).await;
            assert_eq!(
                ids,
                expected_ids(&seeded, updated_to, filter),
                "case {case:?}: after the class change to {updated_to:?}, the filter {filter:?} \
                 must report only the alerts of the pipelines that class gives the channel, got \
                 {ids:?}"
            );
            // Whatever the filter, no reported alert names a pipeline the
            // channel has not got: the same rule the channel's own
            // `EgressEndpoints` are reported by.
            let class_pipelines: Vec<String> = class_pipelines(updated_to)
                .map(|pipeline| pipeline.to_string())
                .collect();
            for alert in &alerts {
                let pipeline = alert["pipelineId"].as_str();
                assert!(
                    pipeline.is_none_or(|pipeline| class_pipelines
                        .iter()
                        .any(|known| known == pipeline)),
                    "case {case:?}: filter {filter:?} reported an alert for a pipeline the class \
                     {updated_to:?} does not give the channel (its pipelines are \
                     {class_pipelines:?}), got {alert}"
                );
            }
        }

        // Nothing was deleted to achieve that: the log still holds every alert
        // that was recorded, as the history it is, and the channel reports one
        // egress endpoint per pipeline of the class it now has.
        assert_eq!(
            logged(&engine, &id),
            seeded.len(),
            "case {case:?}: expected the class change to leave all {} recorded alerts in the log",
            seeded.len()
        );
        let (_, after) = describe(&router, &id).await;
        assert_eq!(
            after["egressEndpoints"].as_array().map(Vec::len),
            Some(class_pipelines(updated_to).count()),
            "case {case:?}: expected one egress endpoint per pipeline of the class {updated_to:?}, \
             got {:?}",
            after["egressEndpoints"]
        );
    }
}

/// Rule: the class decides which alerts a channel REPORTS, not which ones its
/// log holds, so a channel moved back into the class that gives it a pipeline
/// reports that pipeline's alerts again — unchanged, down to the `Id` and the
/// timestamps — exactly as it reports again the address that pipeline egresses
/// from.
///
/// The round trip is the invariant the table above cannot see: it reads the
/// alert back through `ListAlerts` after each of the two class writes.
#[tokio::test]
async fn a_class_that_gives_a_pipeline_back_reports_its_alerts_again() {
    let (router, engine) = medialive_with_engine();
    let id = channel(&router, "class-round-trip", Some("STANDARD")).await;
    let seeded = seed(&engine, &id, &[Some(0), Some(1), None, Some(1)]);
    let case = "standard to single pipeline and back";

    let (_, before) = listed(&router, &id, Some(CHANNEL_ALERT_FILTER_ALL), case).await;
    assert_eq!(
        before.len(),
        seeded.len(),
        "case {case:?}: expected the two-pipeline class to report every recorded alert, got \
         {before:?}"
    );

    wait_for_the_clock();
    let (status, response) = update_class(&router, &id, "SINGLE_PIPELINE").await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateChannelClass to SINGLE_PIPELINE: got {status}: {response}"
    );
    let (ids, _) = listed(&router, &id, Some(CHANNEL_ALERT_FILTER_ALL), case).await;
    assert_eq!(
        ids,
        expected_ids(&seeded, "SINGLE_PIPELINE", Some(CHANNEL_ALERT_FILTER_ALL)),
        "case {case:?}: expected the one-pipeline class to report only the alerts it has the \
         pipeline for, got {ids:?}"
    );

    // Back into the class that gives the channel that pipeline again: the
    // alerts it was reporting before come back as they were recorded.
    wait_for_the_clock();
    let (status, response) = update_class(&router, &id, "STANDARD").await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateChannelClass back to STANDARD: got {status}: {response}"
    );
    let (_, again) = listed(&router, &id, Some(CHANNEL_ALERT_FILTER_ALL), case).await;
    assert_eq!(
        again, before,
        "case {case:?}: expected the restored class to report the same alerts, unchanged, got \
         {again:?}"
    );
}
