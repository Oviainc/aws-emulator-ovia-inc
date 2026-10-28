// SPDX-License-Identifier: CC0-1.0

//! `DescribeScraperLoggingConfiguration`, end to end over HTTP: the logging
//! configuration a scraper holds is reported off the record the store HOLDS — every
//! member read back from it, whatever the configuration looks like — and a request that
//! names no stored configuration is the modelled `ResourceNotFoundException` naming the
//! resource that is missing.
//!
//! The store is seeded directly here, through the same engine the mounted service reads
//! ([`Amp::store`]) and with the builders the deletion suite shares
//! ([`crate::scraper_logging_configuration_deletion`]): this model binds no operation that
//! CREATES a scraper logging configuration, so seeding is how a scraper holds one to
//! describe at all — and it is the same record the handler reads, not a second copy of it.

use crate::scraper_listing::{dataset_destination, eks_source, scraper, seed};
use crate::scraper_logging_configuration_deletion::{
    cloud_watch_logs, components, logging_configuration, seed_logging, seed_pair, seeded_at,
};
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::{
    ScraperComponent, ScraperLoggingDestination, SCRAPER_COMPONENT_TYPE_VALUES,
    SCRAPER_LOGGING_CONFIGURATION_STATUS_CODE_VALUES, SCRAPER_STATUS_CODE_VALUES,
};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    ScraperLoggingConfiguration, SCRAPER_LOGGING_NOT_FOUND_MESSAGE,
    SCRAPER_LOGGING_RESOURCE_TYPE, SCRAPER_LOGGING_STATUS_ACTIVE, SCRAPER_LOGGING_STATUS_DELETING,
    SCRAPER_NOT_FOUND_MESSAGE, SCRAPER_RESOURCE_TYPE, STATUS_ACTIVE, TRANSITION_DELAY,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The members `DescribeScraperLoggingConfigurationResponse` declares, from the model
/// (`smithy_query(query="operation",operation="DescribeScraperLoggingConfiguration")`):
/// all five are `@required`, so a reported body carries exactly these and nothing else.
const DESCRIBED_MEMBERS: &[&str] = &[
    "status",
    "scraperId",
    "loggingDestination",
    "scraperComponents",
    "modifiedAt",
];

/// `GET /scrapers/{scraperId}/logging-configuration`.
async fn describe(amp: &Amp, region: &str, scraper_id: &str) -> Reply {
    amp.call(
        "GET",
        &format!("/scrapers/{scraper_id}/logging-configuration"),
        region,
        "",
    )
    .await
}

/// One instant as the epoch seconds a `Timestamp` is carried as on this protocol.
///
/// `pub(crate)` with the two shape helpers below, because
/// [`crate::scraper_logging_configuration_update`] reads the same members back off the
/// same record: a second copy of them is a second set of rules about the wire form of a
/// stored scraper logging configuration.
pub(crate) fn seconds_since_epoch(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("a stored instant is after the epoch")
        .as_secs_f64()
}

/// The `loggingDestination` a stored destination is reported as — the one variant
/// `ScraperLoggingDestination` models, as the union's JSON form.
pub(crate) fn expected_destination(destination: &ScraperLoggingDestination) -> serde_json::Value {
    let ScraperLoggingDestination::cloudWatchLogs(cloud_watch_logs) = destination;
    serde_json::json!({
        "cloudWatchLogs": { "logGroupArn": cloud_watch_logs.log_group_arn }
    })
}

/// The `scraperComponents` a stored component list is reported as, in the order the
/// record holds them.
pub(crate) fn expected_components(scraper_components: &[ScraperComponent]) -> serde_json::Value {
    serde_json::Value::Array(
        scraper_components
            .iter()
            .map(|component| {
                let mut entry = serde_json::Map::new();
                entry.insert(
                    "type".to_string(),
                    serde_json::Value::String(component.r#type.clone()),
                );
                if let Some(config) = component.config.as_ref() {
                    let mut config_json = serde_json::Map::new();
                    if let Some(options) = config.options.as_ref() {
                        config_json.insert(
                            "options".to_string(),
                            serde_json::Value::Object(
                                options
                                    .iter()
                                    .map(|(key, value)| {
                                        (key.clone(), serde_json::Value::String(value.clone()))
                                    })
                                    .collect(),
                            ),
                        );
                    }
                    entry.insert("config".to_string(), serde_json::Value::Object(config_json));
                }
                serde_json::Value::Object(entry)
            })
            .collect(),
    )
}

/// Every assertion one described configuration makes: each member reported off the
/// stored record, the timestamp as the record holds it, and no member the model does not
/// declare.
fn assert_reports_the_record(case: &str, reply: &Reply, record: &ScraperLoggingConfiguration) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: a stored configuration must be describable, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["scraperId"]),
        record.scraper_id,
        "case {case:?}: scraperId must name the scraper the record belongs to, expected \
         {:?}, got {reply:?}",
        record.scraper_id
    );
    assert_eq!(
        text(&reply.body["status"]["statusCode"]),
        record.status_code,
        "case {case:?}: statusCode must be the stored one, expected {:?}, got {reply:?}",
        record.status_code
    );
    match record.status_reason.as_deref() {
        Some(reason) => assert_eq!(
            text(&reply.body["status"]["statusReason"]),
            reason,
            "case {case:?}: statusReason must be the stored one, expected {reason:?}, got \
             {reply:?}"
        ),
        None => assert!(
            reply.body["status"]["statusReason"].is_null(),
            "case {case:?}: a configuration with no failure reason reports none, got {:?}",
            reply.body["status"]["statusReason"]
        ),
    }
    assert_eq!(
        reply.body["loggingDestination"],
        expected_destination(&record.logging_destination),
        "case {case:?}: loggingDestination must read back off the stored record, expected \
         {:?}, got {reply:?}",
        expected_destination(&record.logging_destination)
    );
    assert_eq!(
        reply.body["scraperComponents"],
        expected_components(&record.scraper_components),
        "case {case:?}: scraperComponents must read back off the stored record, expected \
         {:?}, got {reply:?}",
        expected_components(&record.scraper_components)
    );
    let reported_modified_at = reply.body["modifiedAt"].as_f64().unwrap_or_else(|| {
        panic!(
            "case {case:?}: modifiedAt is a @required Timestamp, got {:?}",
            reply.body["modifiedAt"]
        )
    });
    let expected_modified_at = seconds_since_epoch(record.modified_at);
    assert!(
        (reported_modified_at - expected_modified_at).abs() < 0.001,
        "case {case:?}: modifiedAt must report the stored instant, expected \
         {expected_modified_at:?}, got {reported_modified_at:?}"
    );
    let reported_members: Vec<String> = reply
        .body
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    for member in &reported_members {
        assert!(
            DESCRIBED_MEMBERS.contains(&member.as_str()),
            "case {case:?}: {member:?} is not a member \
             DescribeScraperLoggingConfigurationResponse declares ({DESCRIBED_MEMBERS:?}), \
             got {reply:?}"
        );
    }
}

/// Every assertion a not-found refusal makes: the modelled `ResourceNotFoundException`,
/// the sentence for the kind of resource that is missing, and the id and type the
/// refusal names.
fn assert_not_found(
    case: &str,
    reply: &Reply,
    scraper_id: &str,
    message: &str,
    resource_type: &str,
) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: a configuration the store holds nothing for is the modelled 404, \
         got {reply:?}"
    );
    assert_eq!(
        reply.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected the modelled ResourceNotFoundException, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["message"]),
        message,
        "case {case:?}: expected {message:?}, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["resourceId"]),
        scraper_id,
        "case {case:?}: the refusal must name the id addressed, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["resourceType"]),
        resource_type,
        "case {case:?}: the refusal must name the resource type, got {reply:?}"
    );
    for member in DESCRIBED_MEMBERS {
        assert!(
            reply.body[*member].is_null(),
            "case {case:?}: a refused describe reports no {member:?}, got {reply:?}"
        );
    }
}

/// A describe reports the STORED record: every member of
/// `DescribeScraperLoggingConfigurationResponse` read back off the configuration the
/// store holds.
///
/// One row per thing the rule generalises over — the destination log group, how many
/// components of which types the configuration covers and whether they carry options,
/// a failure reason or none, the length of the `scraperId` addressing it, and the
/// instant the record was last modified.
#[tokio::test]
async fn a_describe_reports_every_member_of_the_stored_record() {
    let region = "us-west-2";

    let mut with_reason = logging_configuration(
        "s-22222222-2222-2222-2222-222222222222",
        SCRAPER_LOGGING_STATUS_ACTIVE,
        cloud_watch_logs("aws/prometheus/reasoned"),
        components(SCRAPER_COMPONENT_TYPE_VALUES),
    );
    with_reason.status_reason = Some("the delivery role is missing a permission".to_string());

    let mut no_component_config = logging_configuration(
        "s",
        SCRAPER_LOGGING_STATUS_ACTIVE,
        cloud_watch_logs("aws/prometheus/tiny"),
        // `ScraperComponent$config` is optional, so a component carrying none is
        // reported without it.
        vec![ScraperComponent {
            r#type: "EXPORTER".to_string(),
            config: None,
        }],
    );
    no_component_config.modified_at = seeded_at() + Duration::from_secs(86_400);

    let cases = vec![
        (
            "one COLLECTOR component logging to its own group",
            logging_configuration(
                "s-11111111-1111-1111-1111-111111111111",
                SCRAPER_LOGGING_STATUS_ACTIVE,
                cloud_watch_logs("aws/prometheus/collector"),
                components(&["COLLECTOR"]),
            ),
        ),
        (
            "every component type the model lists, and a failure reason",
            with_reason,
        ),
        ("a one-character scraperId, no component config", no_component_config),
        (
            // `ScraperId` is `len 1..64, pattern ^[0-9A-Za-z][-.0-9A-Z_a-z]*$`: an id at
            // either end of that length is addressed the same way.
            "a scraperId of the maximum modelled length",
            logging_configuration(
                &"s".repeat(64),
                SCRAPER_LOGGING_STATUS_ACTIVE,
                cloud_watch_logs("aws/prometheus/long"),
                components(&["SERVICE_DISCOVERY", "COLLECTOR"]),
            ),
        ),
    ];

    for (case, seeded) in cases {
        let amp = Amp::new();
        seed_pair(&amp, region, &seeded, "ws-11111111-1111-1111-1111-111111111111");

        let reply = describe(&amp, region, &seeded.scraper_id).await;
        assert_reports_the_record(case, &reply, &seeded);
    }
}

/// Whatever status the stored configuration is in, the describe reports THAT status: the
/// catalog is the model's own value list
/// (`crate::ops::types::SCRAPER_LOGGING_CONFIGURATION_STATUS_CODE_VALUES`), so a status
/// this test never names by hand is covered too. Nothing is owed on these records, so
/// nothing settles them under the read.
#[tokio::test]
async fn a_describe_reports_whatever_status_the_record_is_in() {
    let region = "us-west-2";
    let scraper_id = "s-33333333-3333-3333-3333-333333333333";

    for status in SCRAPER_LOGGING_CONFIGURATION_STATUS_CODE_VALUES {
        let amp = Amp::new();
        let seeded = logging_configuration(
            scraper_id,
            status,
            cloud_watch_logs("aws/prometheus/statuses"),
            components(&["COLLECTOR"]),
        );
        seed_pair(&amp, region, &seeded, "ws-33333333-3333-3333-3333-333333333333");

        let reply = describe(&amp, region, scraper_id).await;
        assert_reports_the_record(status, &reply, &seeded);
    }
}

/// A describe is a report, so it must not change what a later caller sees: repeating it
/// answers the same body, and the stored record keeps its status, its `modifiedAt` and
/// the fact that nothing is owed on it.
#[tokio::test]
async fn a_describe_changes_nothing_a_later_caller_reads() {
    let region = "us-west-2";
    let amp = Amp::new();
    let seeded = logging_configuration(
        "s-44444444-4444-4444-4444-444444444444",
        SCRAPER_LOGGING_STATUS_ACTIVE,
        cloud_watch_logs("aws/prometheus/repeatable"),
        components(&["COLLECTOR", "EXPORTER"]),
    );
    seed_pair(&amp, region, &seeded, "ws-44444444-4444-4444-4444-444444444444");

    let first = describe(&amp, region, &seeded.scraper_id).await;
    assert_reports_the_record("first read", &first, &seeded);

    for attempt in ["second read", "third read"] {
        let again = describe(&amp, region, &seeded.scraper_id).await;
        assert_eq!(
            again.body, first.body,
            "case {attempt:?}: a read must report what the first read did, expected {:?}, \
             got {again:?}",
            first.body
        );
    }

    let stored = amp
        .store()
        .lock()
        .region(region)
        .scraper_logging
        .get(&seeded.scraper_id)
        .cloned()
        .unwrap_or_else(|| panic!("the store must still hold the configuration"));
    assert_eq!(
        stored.status_code, seeded.status_code,
        "a read must leave the stored status alone, expected {:?}, got {:?}",
        seeded.status_code, stored.status_code
    );
    assert_eq!(
        stored.modified_at, seeded.modified_at,
        "a read is not a modification, so modifiedAt must stand, expected {:?}, got {:?}",
        seeded.modified_at, stored.modified_at
    );
    assert_eq!(
        stored.transition_due, None,
        "a read must owe nothing on a settled record, got {:?}",
        stored.transition_due
    );
}

/// A `scraperId` the store does not hold is the modelled `ResourceNotFoundException`
/// (404) naming the SCRAPER, whatever the id looks like: the configuration is a child of
/// the scraper, so the scraper is what is missing.
#[tokio::test]
async fn a_scraper_the_store_does_not_hold_is_not_found() {
    let region = "us-west-2";
    let amp = Amp::new();
    let held = "s-55555555-5555-5555-5555-555555555555";
    seed_pair(
        &amp,
        region,
        &logging_configuration(
            held,
            SCRAPER_LOGGING_STATUS_ACTIVE,
            cloud_watch_logs("aws/prometheus/bystander"),
            components(&["COLLECTOR"]),
        ),
        "ws-55555555-5555-5555-5555-555555555555",
    );

    let absent = [
        (
            "a minted-looking id nothing holds",
            "s-99999999-9999-9999-9999-999999999999".to_string(),
        ),
        ("a one-character id", "z".to_string()),
        ("an id of the maximum modelled length", "s".repeat(64)),
        (
            "an id that differs only in case",
            "S-55555555-5555-5555-5555-555555555555".to_string(),
        ),
    ];

    for (case, scraper_id) in absent {
        let reply = describe(&amp, region, &scraper_id).await;
        assert_not_found(
            case,
            &reply,
            &scraper_id,
            SCRAPER_NOT_FOUND_MESSAGE,
            SCRAPER_RESOURCE_TYPE,
        );
    }

    // A describe that named another scraper reports this configuration unchanged.
    let still_there = describe(&amp, region, held).await;
    assert_eq!(
        text(&still_there.body["status"]["statusCode"]),
        SCRAPER_LOGGING_STATUS_ACTIVE,
        "the configuration nobody asked for is still reported as it was, got {still_there:?}"
    );
}

/// A scraper that holds NO logging configuration is the modelled
/// `ResourceNotFoundException` (404) naming the CONFIGURATION, whatever status the
/// scraper itself is in — there is nothing to report. The catalog of scraper statuses is
/// the model's own value list (`crate::ops::types::SCRAPER_STATUS_CODE_VALUES`).
#[tokio::test]
async fn a_scraper_that_holds_no_configuration_is_not_found() {
    let region = "us-west-2";

    for status in SCRAPER_STATUS_CODE_VALUES {
        let amp = Amp::new();
        let scraper_id = "s-66666666-6666-6666-6666-666666666666";
        seed(
            &amp,
            region,
            &scraper(
                scraper_id,
                Some("unlogged"),
                status,
                eks_source("unlogged-cluster"),
                dataset_destination("unlogged-metrics"),
            ),
        );

        let reply = describe(&amp, region, scraper_id).await;
        assert_not_found(
            status,
            &reply,
            scraper_id,
            SCRAPER_LOGGING_NOT_FOUND_MESSAGE,
            SCRAPER_LOGGING_RESOURCE_TYPE,
        );
    }
}

/// A describe reports the record as of NOW, so a transition the clock already owed is
/// materialised before the answer: a configuration whose removal has come due is gone —
/// the describe is the configuration's not-found — and one whose other pending
/// transition has come due reports `ACTIVE`.
#[tokio::test]
async fn a_describe_reports_the_transition_the_clock_already_owed() {
    let region = "us-west-2";

    // A removal the clock owes: the record is still reported while the span runs, and is
    // gone once it has passed.
    let amp = Amp::new();
    let deleting_id = "s-77777777-7777-7777-7777-777777777777";
    let mut deleting = logging_configuration(
        deleting_id,
        SCRAPER_LOGGING_STATUS_DELETING,
        cloud_watch_logs("aws/prometheus/doomed"),
        components(&["COLLECTOR"]),
    );
    deleting.transition_due = Some(SystemTime::now() + TRANSITION_DELAY);
    seed_pair(&amp, region, &deleting, "ws-77777777-7777-7777-7777-777777777777");

    let pending = describe(&amp, region, deleting_id).await;
    assert_eq!(
        text(&pending.body["status"]["statusCode"]),
        SCRAPER_LOGGING_STATUS_DELETING,
        "a removal that is not yet due still reports the transient status, got {pending:?}"
    );

    wait_for_due_transition();

    let removed = describe(&amp, region, deleting_id).await;
    assert_not_found(
        "the removal has come due",
        &removed,
        deleting_id,
        SCRAPER_LOGGING_NOT_FOUND_MESSAGE,
        SCRAPER_LOGGING_RESOURCE_TYPE,
    );

    // Every other pending transition settles to ACTIVE, whatever transient status the
    // record was in: the catalog is the model's own value list, minus the one the clock
    // removes.
    for status in SCRAPER_LOGGING_CONFIGURATION_STATUS_CODE_VALUES {
        if *status == SCRAPER_LOGGING_STATUS_DELETING {
            continue;
        }
        let amp = Amp::new();
        let scraper_id = "s-88888888-8888-8888-8888-888888888888";
        let mut pending = logging_configuration(
            scraper_id,
            status,
            cloud_watch_logs("aws/prometheus/settling"),
            components(&["COLLECTOR"]),
        );
        pending.transition_due = Some(SystemTime::now());
        seed_logging(&amp, region, &pending);
        seed(
            &amp,
            region,
            &scraper(
                scraper_id,
                Some("settling"),
                STATUS_ACTIVE,
                eks_source("settling-cluster"),
                dataset_destination("settling-metrics"),
            ),
        );

        let reply = describe(&amp, region, scraper_id).await;
        assert_eq!(
            text(&reply.body["status"]["statusCode"]),
            SCRAPER_LOGGING_STATUS_ACTIVE,
            "case {status:?}: a transition the clock already owed must be reported \
             settled, got {reply:?}"
        );
        assert_eq!(
            amp.store()
                .lock()
                .region(region)
                .scraper_logging
                .get(scraper_id)
                .and_then(|record| record.transition_due),
            None,
            "case {status:?}: the settled record must owe nothing more"
        );
    }
}

/// The deletion a caller just started is what the next describe reports — the stored
/// record moved to `DELETING` — and once the clock has dropped it, the describe is the
/// configuration's not-found. The two operations read and write ONE record.
#[tokio::test]
async fn a_describe_reports_the_deletion_a_caller_just_started() {
    let region = "us-west-2";
    let amp = Amp::new();
    let scraper_id = "s-99999999-9999-9999-9999-999999999999";
    let seeded = logging_configuration(
        scraper_id,
        SCRAPER_LOGGING_STATUS_ACTIVE,
        cloud_watch_logs("aws/prometheus/round-trip"),
        components(&["COLLECTOR", "SERVICE_DISCOVERY"]),
    );
    seed_pair(&amp, region, &seeded, "ws-99999999-9999-9999-9999-999999999999");

    let before = describe(&amp, region, scraper_id).await;
    assert_reports_the_record("before the delete", &before, &seeded);

    let deleted = amp
        .call(
            "DELETE",
            &format!("/scrapers/{scraper_id}/logging-configuration"),
            region,
            "",
        )
        .await;
    assert_eq!(
        deleted.status, 202,
        "the delete must be accepted, got {deleted:?}"
    );

    let during = describe(&amp, region, scraper_id).await;
    assert_eq!(
        text(&during.body["status"]["statusCode"]),
        SCRAPER_LOGGING_STATUS_DELETING,
        "the describe must report the status the delete wrote, got {during:?}"
    );
    let moved = during.body["modifiedAt"].as_f64();
    assert!(
        moved.is_some_and(|instant| instant > seconds_since_epoch(seeded.modified_at)),
        "the delete changed the record, so modifiedAt must have moved past {:?}, got \
         {during:?}",
        seconds_since_epoch(seeded.modified_at)
    );
    assert_eq!(
        during.body["loggingDestination"],
        before.body["loggingDestination"],
        "a deletion in progress reports the destination it was logging to, got {during:?}"
    );

    wait_for_due_transition();

    let after = describe(&amp, region, scraper_id).await;
    assert_not_found(
        "after the clock dropped the record",
        &after,
        scraper_id,
        SCRAPER_LOGGING_NOT_FOUND_MESSAGE,
        SCRAPER_LOGGING_RESOURCE_TYPE,
    );
}
