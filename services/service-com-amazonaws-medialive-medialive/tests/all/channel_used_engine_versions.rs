// SPDX-License-Identifier: CC0-1.0

//! The engine version a channel's RUNNING pipelines are using, as
//! `ListChannels` reports it, end to end over the restJson1 wire.
//!
//! The rule under test: `ChannelSummary$UsedChannelEngineVersions` is "The engine
//! version that the running pipelines are using", so a channel reports a used
//! engine version exactly while it has a healthy pipeline
//! (`PipelinesRunningCount` is "The number of currently healthy pipelines"), and
//! the version those pipelines run is the build identifier the channel is pinned
//! to — or the account's default version, the newest the catalogue still offers,
//! for a channel pinned to none — carrying the expiry that catalogue stores for
//! that identifier. A channel with no pipeline up is using no engine version and
//! reports none.
//!
//! The rows differ along what the rule generalises over: how many pipelines the
//! channel's class gives it, which catalogue entry it is pinned to (a different
//! stored expiry per row, one of them already past its expiry), an identifier the
//! catalogue does not hold, the documented reset value, no engine version member
//! at all, and the lifecycle state the channel is read in (creating, idle,
//! starting, running, stopping, recovering with a pipeline still up and
//! recovering with none).
//!
//! Nothing here invents an engine version: an engine version is published by the
//! SERVICE and no operation of this one creates, changes or deletes one, so the
//! catalogue the table installs is built through the store's own type, and the
//! account the last suite uses has none published to it at all.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    ChannelEngineVersionCatalog, ChannelEngineVersionRecord,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::engine_version_catalog::{install, shifted};
use crate::support::{epoch_seconds, timestamp, unix_seconds};
use crate::wire::{call, medialive, medialive_with_engine};

/// How far two reported instants may be apart and still name the same instant:
/// the wire form carries sub-second precision, so the comparison is a tolerance
/// rather than an equality.
const TOLERANCE_SECONDS: f64 = 0.5;

/// The one `Version` value that is a RESET rather than a build identifier,
/// spelled as `ChannelEngineVersionRequest$Version` documents it: "Specify
/// 'DEFAULT' to reset to the default version".
const RESET: &str = "DEFAULT";

/// The catalogue every row of the first table installs, as `(build identifier,
/// seconds from the base instant it was released, seconds from it that it
/// expires)`: three entries with three DIFFERENT expiries, one of them already
/// past — so the newest version still available to the account (its DEFAULT
/// version) is the last of the three, and a row that reports the wrong entry
/// cannot pass.
const CATALOGUE: &[(&str, i64, i64)] = &[
    ("9.0.0", -300, 1_800),
    ("9.1.0", -200, -60),
    ("9.2.0", -100, 7_200),
];

/// The build identifier of the newest UNEXPIRED entry of [`CATALOGUE`], which is
/// the default version that catalogue offers.
const DEFAULT_VERSION: &str = "9.2.0";

/// One row: the channel to create, and the engine version its running pipelines
/// must then be reported as using.
struct Row {
    case: &'static str,
    /// The `ChannelClass` the channel is created with, or `None` to name none.
    class: Option<&'static str>,
    /// The `channelEngineVersion.version` the create names, or `None` when it
    /// names no engine version member at all.
    pinned: Option<&'static str>,
    /// The build identifier the running channel must report as in use.
    used: &'static str,
    /// Seconds from the base instant the reported `expirationDate` must name, or
    /// `None` when the catalogue holds no expiry for that identifier.
    expires: Option<i64>,
}

/// The catalogue of [`CATALOGUE`], dated from `base`.
fn catalogue(base: SystemTime) -> ChannelEngineVersionCatalog {
    ChannelEngineVersionCatalog::of(CATALOGUE.iter().map(|(version, released, expires)| {
        ChannelEngineVersionRecord::expiring(
            (*version).to_string(),
            shifted(base, *released),
            shifted(base, *expires),
        )
    }))
}

/// `CreateChannel`, answering the channel it reports.
async fn create_channel(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let body = crate::support::resolved_channel_body(router, body).await;
    let (status, response) = call(router, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateChannel to answer 201, got {status}: {response}"
    );
    response["channel"].clone()
}

/// `StartChannel`.
async fn start_channel(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = call(router, "POST", &format!("/prod/channels/{id}/start"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartChannel to answer 200, got {status}: {body}"
    );
    body
}

/// `StopChannel`.
async fn stop_channel(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, body) = call(router, "POST", &format!("/prod/channels/{id}/stop"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StopChannel to answer 200, got {status}: {body}"
    );
}

/// `RestartChannelPipelines`, naming `pipeline_ids` when the row carries them.
async fn restart_pipelines(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    pipeline_ids: Option<Value>,
) {
    let body = pipeline_ids.map(|ids| json!({ "pipelineIds": ids }));
    let (status, response) = call(
        router,
        "POST",
        &format!("/prod/channels/{id}/restartChannelPipelines"),
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RestartChannelPipelines to answer 200, got {status}: {response}"
    );
}

/// The `ListChannels` summary of the channel `id`.
async fn listed_channel(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = call(router, "GET", "/prod/channels", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListChannels to answer 200, got {status}: {body}"
    );
    body["channels"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|channel| channel["id"] == json!(id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: ListChannels reported no channel {id}: {body}"))
}

/// The `usedChannelEngineVersions` entries a summary carries. The member targets
/// a list, so it is always reported AS a list: a channel using no engine version
/// reports the empty one rather than omitting the member.
fn used_versions(case: &str, stage: &str, summary: &Value) -> Vec<Value> {
    match summary.get("usedChannelEngineVersions") {
        Some(Value::Array(entries)) => entries.clone(),
        other => panic!(
            "case {case:?}/{stage}: expected a usedChannelEngineVersions array, got {:?}",
            other.cloned().unwrap_or(Value::Null)
        ),
    }
}

/// Assert the channel `id` is in `state` and reports no used engine version,
/// because no pipeline of it is up.
async fn assert_uses_nothing(
    router: &MultiServiceRouter,
    case: &str,
    stage: &str,
    id: &str,
    state: &str,
) {
    let summary = listed_channel(router, case, id).await;
    assert_eq!(
        summary["state"],
        json!(state),
        "case {case:?}/{stage}: expected the channel in state {state}, got {:?}",
        summary["state"]
    );
    assert_eq!(
        summary["pipelinesRunningCount"],
        json!(0),
        "case {case:?}/{stage}: expected no pipeline running, got {:?}",
        summary["pipelinesRunningCount"]
    );
    let reported = used_versions(case, stage, &summary);
    assert!(
        reported.is_empty(),
        "case {case:?}/{stage}: expected a channel with no pipeline up to use no engine \
         version, got {reported:?}"
    );
}

/// Assert `summary` reports exactly the one engine version `version`, with the
/// expiry `expires` seconds from `base` when the catalogue holds one for it.
fn assert_uses(
    case: &str,
    stage: &str,
    summary: &Value,
    version: &str,
    expires: Option<i64>,
    base: SystemTime,
) {
    let reported = used_versions(case, stage, summary);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}/{stage}: every pipeline of one channel runs one engine, so expected \
         exactly the version {version:?} in use, got {reported:?}"
    );
    let entry = &reported[0];
    assert_eq!(
        entry["version"],
        json!(version),
        "case {case:?}/{stage}: expected the used build identifier {version:?}, got {entry:?}"
    );
    match expires {
        Some(expires) => {
            let reported_expiry = epoch_seconds(timestamp(case, "expirationDate", entry));
            let want = unix_seconds(shifted(base, expires));
            assert!(
                (reported_expiry - want).abs() < TOLERANCE_SECONDS,
                "case {case:?}/{stage}: expected the catalogue expiry {want} for {version:?}, \
                 got {reported_expiry}"
            );
        }
        None => assert_eq!(
            entry.get("expirationDate").cloned().unwrap_or(Value::Null),
            Value::Null,
            "case {case:?}/{stage}: expected no expirationDate for an identifier the catalogue \
             does not hold, got {entry:?}"
        ),
    }
}

/// Rule: a channel reports a used engine version exactly while a pipeline of it
/// is up, and that version is its pin — or the account's default version when it
/// pinned none — with the expiry the catalogue stores for that identifier.
#[tokio::test]
async fn a_running_channel_reports_the_engine_version_its_pipelines_run() {
    let table = [
        Row {
            case: "standard, pinned to the newest catalogue entry",
            class: Some("STANDARD"),
            pinned: Some("9.2.0"),
            used: "9.2.0",
            expires: Some(7_200),
        },
        Row {
            case: "single pipeline, pinned to an older entry, which expires sooner",
            class: Some("SINGLE_PIPELINE"),
            pinned: Some("9.0.0"),
            used: "9.0.0",
            expires: Some(1_800),
        },
        Row {
            case: "standard, pinned to an entry already past its expiry",
            class: Some("STANDARD"),
            pinned: Some("9.1.0"),
            used: "9.1.0",
            expires: Some(-60),
        },
        Row {
            case: "single pipeline, pinned to an identifier the catalogue does not hold",
            class: Some("SINGLE_PIPELINE"),
            pinned: Some("7.5.0"),
            used: "7.5.0",
            expires: None,
        },
        Row {
            case: "standard, pinned with the documented reset value",
            class: Some("STANDARD"),
            pinned: Some(RESET),
            used: DEFAULT_VERSION,
            expires: Some(7_200),
        },
        Row {
            case: "class and engine version both omitted",
            class: None,
            pinned: None,
            used: DEFAULT_VERSION,
            expires: Some(7_200),
        },
    ];

    for row in table {
        let case = row.case;
        let (router, engine) = medialive_with_engine();
        let base = SystemTime::now();
        install(&engine, catalogue(base));

        let mut request = json!({ "name": "used-engine-versions" });
        if let Some(class) = row.class {
            request["channelClass"] = json!(class);
        }
        if let Some(version) = row.pinned {
            request["channelEngineVersion"] = json!({ "version": version });
        }
        let created = create_channel(&router, case, request).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        // A channel that is still being created runs nothing yet.
        assert_uses_nothing(&router, case, "CREATING", &id, "CREATING").await;

        wait_for_the_clock();
        assert_uses_nothing(&router, case, "IDLE", &id, "IDLE").await;

        // A start has been accepted but the pipelines are not up yet.
        start_channel(&router, case, &id).await;
        assert_uses_nothing(&router, case, "STARTING", &id, "STARTING").await;

        wait_for_the_clock();
        let running = listed_channel(&router, case, &id).await;
        assert_eq!(
            running["state"],
            json!("RUNNING"),
            "case {case:?}: the row did not reach RUNNING, it reached {:?}",
            running["state"]
        );
        assert_uses(case, "RUNNING", &running, row.used, row.expires, base);

        // The listing is a read: a second one reports the same thing.
        let again = listed_channel(&router, case, &id).await;
        assert_eq!(
            used_versions(case, "the second read", &again),
            used_versions(case, "RUNNING", &running),
            "case {case:?}: a read must not change the engine version a later caller reads"
        );

        // Coming down, the pipelines are no longer up, so there is no engine
        // version in use again.
        stop_channel(&router, case, &id).await;
        assert_uses_nothing(&router, case, "STOPPING", &id, "STOPPING").await;
    }
}

/// Rule, read through the pipelines a restart takes down: a channel reports the
/// engine version in use while a pipeline of it is up — including a channel that
/// is recovering with one pipeline still up — and none while every pipeline is
/// down.
#[tokio::test]
async fn a_recovering_channel_uses_an_engine_version_only_while_a_pipeline_is_up() {
    // (case, the `pipelineIds` the restart names, how many pipelines stay up)
    let table = [
        (
            "every pipeline of a standard channel restarts",
            None,
            0_usize,
        ),
        (
            "one pipeline of a standard channel restarts",
            Some(json!(["PIPELINE_0"])),
            1,
        ),
    ];

    for (case, pipeline_ids, still_up) in table {
        let (router, engine) = medialive_with_engine();
        let base = SystemTime::now();
        install(&engine, catalogue(base));

        let created = create_channel(
            &router,
            case,
            json!({
                "name": "recovering-engine-version",
                "channelClass": "STANDARD",
                "channelEngineVersion": { "version": "9.0.0" },
            }),
        )
        .await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        wait_for_the_clock();
        start_channel(&router, case, &id).await;
        wait_for_the_clock();

        let running = listed_channel(&router, case, &id).await;
        assert_eq!(
            running["state"],
            json!("RUNNING"),
            "case {case:?}: the row did not reach RUNNING, it reached {:?}",
            running["state"]
        );
        assert_uses(case, "RUNNING", &running, "9.0.0", Some(1_800), base);

        restart_pipelines(&router, case, &id, pipeline_ids).await;
        let recovering = listed_channel(&router, case, &id).await;
        assert_eq!(
            recovering["state"],
            json!("RECOVERING"),
            "case {case:?}: expected a restart to leave the channel RECOVERING, got {:?}",
            recovering["state"]
        );
        assert_eq!(
            recovering["pipelinesRunningCount"],
            json!(still_up),
            "case {case:?}: expected {still_up} pipeline(s) still up while recovering, got {:?}",
            recovering["pipelinesRunningCount"]
        );
        if still_up == 0 {
            let reported = used_versions(case, "RECOVERING", &recovering);
            assert!(
                reported.is_empty(),
                "case {case:?}/RECOVERING: expected a channel with every pipeline down to use \
                 no engine version, got {reported:?}"
            );
        } else {
            assert_uses(case, "RECOVERING", &recovering, "9.0.0", Some(1_800), base);
        }

        // The restart completes with every pipeline of the class up again, on
        // the engine the channel is pinned to.
        wait_for_the_clock();
        let restarted = listed_channel(&router, case, &id).await;
        assert_eq!(
            restarted["state"],
            json!("RUNNING"),
            "case {case:?}: expected the restart to complete into RUNNING, got {:?}",
            restarted["state"]
        );
        assert_uses(case, "restarted", &restarted, "9.0.0", Some(1_800), base);
    }
}

/// Rule, on the account no test published an engine version to: the default
/// version is the newest the account's catalogue offers, so an account with an
/// EMPTY catalogue has no default version and a running channel pinned to none
/// names no engine version — while a channel pinned to a build identifier reports
/// that identifier, which is a fact of the channel rather than of the catalogue.
#[tokio::test]
async fn an_account_with_no_published_version_names_one_only_where_a_channel_pinned_it() {
    let case = "an account with no published engine version";
    let router = medialive();

    let unpinned = create_channel(&router, case, json!({ "name": "unpinned" })).await;
    let unpinned_id = unpinned["id"].as_str().unwrap_or_default().to_string();
    let pinned = create_channel(
        &router,
        case,
        json!({
            "name": "pinned",
            "channelEngineVersion": { "version": "3.1.0" },
        }),
    )
    .await;
    let pinned_id = pinned["id"].as_str().unwrap_or_default().to_string();

    wait_for_the_clock();
    start_channel(&router, case, &unpinned_id).await;
    start_channel(&router, case, &pinned_id).await;
    wait_for_the_clock();

    let unpinned = listed_channel(&router, case, &unpinned_id).await;
    assert_eq!(
        unpinned["state"],
        json!("RUNNING"),
        "case {case:?}: the unpinned channel did not reach RUNNING, it reached {:?}",
        unpinned["state"]
    );
    let reported = used_versions(case, "unpinned", &unpinned);
    assert!(
        reported.is_empty(),
        "case {case:?}: expected no used engine version where the account publishes none and \
         the channel pinned none, got {reported:?}"
    );

    let pinned = listed_channel(&router, case, &pinned_id).await;
    assert_eq!(
        pinned["state"],
        json!("RUNNING"),
        "case {case:?}: the pinned channel did not reach RUNNING, it reached {:?}",
        pinned["state"]
    );
    assert_uses(
        case,
        "pinned",
        &pinned,
        "3.1.0",
        None,
        SystemTime::now(),
    );
}
