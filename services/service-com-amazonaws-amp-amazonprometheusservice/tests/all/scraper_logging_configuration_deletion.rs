// SPDX-License-Identifier: CC0-1.0

//! `DeleteScraperLoggingConfiguration`, end to end over HTTP: the deletion of the
//! logging configuration a scraper holds is accepted, moves the STORED record to
//! `DELETING`, and lets the clock drop it — after which the very same call is a
//! not-found rather than a second deletion.
//!
//! The store is seeded directly here, through the same engine the mounted service reads
//! ([`Amp::store`]) and with the same scraper builder the listing and deletion suites use
//! ([`crate::scraper_listing::scraper`]): `UpdateScraperLoggingConfiguration` is still
//! scaffolded (its own stub issue), so seeding is how a scraper with a logging
//! configuration exists to delete at all — and it is the same record the handler reads,
//! not a second copy of it.

use crate::scraper_listing::{dataset_destination, eks_source, scraper, seed, workspace_destination};
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::{
    CloudWatchLogDestination, ComponentConfig, ScraperComponent, ScraperLoggingDestination,
    SCRAPER_COMPONENT_TYPE_VALUES, SCRAPER_LOGGING_CONFIGURATION_STATUS_CODE_VALUES,
    SCRAPER_STATUS_CODE_VALUES,
};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    Scraper, ScraperLoggingConfiguration, SCRAPER_LOGGING_NOT_FOUND_MESSAGE,
    SCRAPER_LOGGING_RESOURCE_TYPE, SCRAPER_LOGGING_STATUS_ACTIVE, SCRAPER_LOGGING_STATUS_DELETING,
    SCRAPER_NOT_FOUND_MESSAGE, SCRAPER_RESOURCE_TYPE, STATUS_ACTIVE,
};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

/// The instant a seeded configuration was last modified: a fixed past one, so a call
/// that writes the record is visible as a `modifiedAt` that MOVED.
fn seeded_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// A CloudWatch Logs destination holding `log_group` — the one variant
/// `ScraperLoggingDestination` models. The ARN satisfies `LogGroupArn`'s published
/// pattern (`^arn:aws[a-z0-9-]*:logs:[a-z0-9-]+:[0-9]{12}:log-group:...\:\*$`).
fn cloud_watch_logs(log_group: &str) -> ScraperLoggingDestination {
    ScraperLoggingDestination::cloudWatchLogs(CloudWatchLogDestination {
        log_group_arn: format!(
            "arn:aws:logs:us-west-2:{}:log-group:{log_group}:*",
            engine::emulator_account_id()
        ),
    })
}

/// One scraper component of each `type` named, every one carrying the options map
/// `ComponentConfig` models. The catalog of types is the model's own value list
/// (`crate::ops::types::SCRAPER_COMPONENT_TYPE_VALUES`).
fn components(types: &[&str]) -> Vec<ScraperComponent> {
    types
        .iter()
        .map(|component_type| ScraperComponent {
            r#type: (*component_type).to_string(),
            config: Some(ComponentConfig {
                options: Some(HashMap::from([(
                    "level".to_string(),
                    "debug".to_string(),
                )])),
            }),
        })
        .collect()
}

/// One seeded scraper logging configuration, in `status_code`, with nothing owed by the
/// clock yet.
fn logging_configuration(
    scraper_id: &str,
    status_code: &str,
    destination: ScraperLoggingDestination,
    scraper_components: Vec<ScraperComponent>,
) -> ScraperLoggingConfiguration {
    ScraperLoggingConfiguration {
        scraper_id: scraper_id.to_string(),
        status_code: status_code.to_string(),
        status_reason: None,
        logging_destination: destination,
        scraper_components,
        modified_at: seeded_at(),
        transition_due: None,
        client_token: None,
    }
}

/// Seed one logging configuration into the store the mounted service reads — the SAME
/// record the handler reads, not a second copy of it.
fn seed_logging(amp: &Amp, record: &ScraperLoggingConfiguration) {
    amp.store()
        .lock()
        .scraper_logging
        .insert(record.scraper_id.clone(), record.clone());
}

/// `DELETE /scrapers/{scraperId}/logging-configuration`, with the optional `clientToken`
/// query member when one is given.
async fn delete(
    amp: &Amp,
    region: &str,
    scraper_id: &str,
    client_token: Option<&str>,
) -> Reply {
    let uri = match client_token {
        Some(token) => {
            format!("/scrapers/{scraper_id}/logging-configuration?clientToken={token}")
        }
        None => format!("/scrapers/{scraper_id}/logging-configuration"),
    };
    amp.call("DELETE", &uri, region, "").await
}

/// The stored configuration of one scraper, or `None` once the store no longer holds it.
fn stored(amp: &Amp, scraper_id: &str) -> Option<ScraperLoggingConfiguration> {
    amp.store().lock().scraper_logging.get(scraper_id).cloned()
}

/// An `ACTIVE` scraper holding a logging configuration, seeded together.
fn seed_pair(amp: &Amp, record: &ScraperLoggingConfiguration, destination_workspace: &str) -> Scraper {
    let host = scraper(
        &record.scraper_id,
        Some("logging"),
        STATUS_ACTIVE,
        eks_source("logging-cluster"),
        workspace_destination(destination_workspace),
    );
    seed(amp, &host);
    seed_logging(amp, record);
    host
}

/// Deleting the logging configuration a scraper holds is accepted (the modelled 202),
/// moves the stored record to `DELETING` with the removal owed on the clock, and the
/// clock then drops it — whatever the configuration looks like. One row per shape a
/// stored configuration can take: the destination it logs to, how many components of
/// which types it covers, the length of the `scraperId` addressing it, and whether the
/// request carries a `clientToken`.
#[tokio::test]
async fn deleting_a_configuration_moves_it_to_deleting_then_the_clock_removes_it() {
    let region = "us-west-2";
    // A `clientToken` at the top of its modelled length (`IdempotencyToken`, len 1..64).
    let long_token = "t".repeat(64);
    let cases = vec![
        (
            "one COLLECTOR component logging to its own group",
            logging_configuration(
                "s-11111111-1111-1111-1111-111111111111",
                SCRAPER_LOGGING_STATUS_ACTIVE,
                cloud_watch_logs("aws/prometheus/collector"),
                components(&["COLLECTOR"]),
            ),
            Some("delete-logging-1"),
        ),
        (
            "every component type the model lists, and no clientToken",
            logging_configuration(
                "s-22222222-2222-2222-2222-222222222222",
                SCRAPER_LOGGING_STATUS_ACTIVE,
                cloud_watch_logs("aws/prometheus/all-components"),
                components(SCRAPER_COMPONENT_TYPE_VALUES),
            ),
            None,
        ),
        (
            // `ScraperId` is `len 1..64, pattern ^[0-9A-Za-z][-.0-9A-Z_a-z]*$`: an id at
            // either end of that length is addressed the same way.
            "a one-character scraperId",
            logging_configuration(
                "s",
                SCRAPER_LOGGING_STATUS_ACTIVE,
                cloud_watch_logs("aws/prometheus/tiny"),
                components(&["EXPORTER"]),
            ),
            Some("delete-logging-3"),
        ),
        (
            "a scraperId of the maximum modelled length",
            logging_configuration(
                &"s".repeat(64),
                SCRAPER_LOGGING_STATUS_ACTIVE,
                cloud_watch_logs("aws/prometheus/long"),
                components(&["SERVICE_DISCOVERY", "COLLECTOR"]),
            ),
            Some(long_token.as_str()),
        ),
    ];

    for (case, seeded, client_token) in cases {
        let amp = Amp::new();
        let host = seed_pair(&amp, &seeded, "ws-11111111-1111-1111-1111-111111111111");
        let scraper_id = seeded.scraper_id.as_str();

        let reply = delete(&amp, region, scraper_id, client_token).await;
        assert_eq!(
            reply.status, 202,
            "case {case:?}: DeleteScraperLoggingConfiguration is modelled 202, got {reply:?}"
        );

        let pending = stored(&amp, scraper_id);
        let pending = pending.unwrap_or_else(|| {
            panic!("case {case:?}: the record must still be held while the removal is owed")
        });
        assert_eq!(
            pending.status_code, SCRAPER_LOGGING_STATUS_DELETING,
            "case {case:?}: expected the status the call wrote, got {:?}",
            pending.status_code
        );
        assert!(
            pending.transition_due.is_some(),
            "case {case:?}: the delete must leave a removal owed, got {:?}",
            pending.transition_due
        );
        assert!(
            pending.modified_at > seeded_at(),
            "case {case:?}: the delete must move modifiedAt past {:?}, got {:?}",
            seeded_at(),
            pending.modified_at
        );
        assert_eq!(
            pending.client_token.as_deref(),
            client_token,
            "case {case:?}: the record must carry the clientToken of the call that \
             started the deletion, got {:?}",
            pending.client_token
        );
        // The configuration is being deleted; the scraper itself is not.
        assert_eq!(
            amp.store()
                .lock()
                .scrapers
                .get(scraper_id)
                .map(|record| record.status_code.clone())
                .as_deref(),
            Some(host.status_code.as_str()),
            "case {case:?}: deleting a logging configuration must leave the scraper in \
             {:?}",
            host.status_code
        );

        wait_for_due_transition();

        // The removal the clock owed has come due, so this very same call is now a
        // not-found rather than a second deletion.
        let repeated = delete(&amp, region, scraper_id, client_token).await;
        assert_eq!(
            repeated.status, 404,
            "case {case:?}: deleting a configuration the store no longer holds must \
             answer 404, got {repeated:?}"
        );
        assert_eq!(
            text(&repeated.body["message"]),
            SCRAPER_LOGGING_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the service's not-found sentence, got {repeated:?}"
        );
        assert_eq!(
            stored(&amp, scraper_id).map(|record| record.status_code),
            None,
            "case {case:?}: the store must no longer hold the configuration, got {:?}",
            stored(&amp, scraper_id)
        );
    }
}

/// The deletion is accepted whatever status the stored configuration is in — the model
/// states no status rule for it, so none is invented — and the operation is
/// `@idempotent`: replaying it while the removal is still owed is the SAME request, so it
/// is accepted again and does NOT push the owed removal out. The catalog is the model's
/// own value list (`crate::ops::types::SCRAPER_LOGGING_CONFIGURATION_STATUS_CODE_VALUES`),
/// so a status this test never names by hand is covered too.
#[tokio::test]
async fn a_delete_is_accepted_from_every_status_and_a_replay_is_the_same_request() {
    let region = "us-west-2";
    let scraper_id = "s-33333333-3333-3333-3333-333333333333";
    let token = "idem-33333333";

    for status in SCRAPER_LOGGING_CONFIGURATION_STATUS_CODE_VALUES {
        let amp = Amp::new();
        seed_pair(
            &amp,
            &logging_configuration(
                scraper_id,
                status,
                cloud_watch_logs("aws/prometheus/statuses"),
                components(&["COLLECTOR"]),
            ),
            "ws-33333333-3333-3333-3333-333333333333",
        );

        let first = delete(&amp, region, scraper_id, Some(token)).await;
        assert_eq!(
            first.status, 202,
            "case {status:?}: the delete must be accepted, got {first:?}"
        );
        let owed = stored(&amp, scraper_id).and_then(|record| record.transition_due);
        assert!(
            owed.is_some(),
            "case {status:?}: the delete must leave a removal owed, got {owed:?}"
        );
        assert_eq!(
            stored(&amp, scraper_id).map(|record| record.status_code),
            Some(SCRAPER_LOGGING_STATUS_DELETING.to_string()),
            "case {status:?}: expected the status the call wrote, got {:?}",
            stored(&amp, scraper_id).map(|record| record.status_code)
        );

        for repeat_token in [Some(token), None, Some("idem-other")] {
            let repeat = delete(&amp, region, scraper_id, repeat_token).await;
            assert_eq!(
                repeat.status, 202,
                "case {status:?}/{repeat_token:?}: a deletion already under way must \
                 still be accepted, got {repeat:?}"
            );
            let still_owed = stored(&amp, scraper_id).and_then(|record| record.transition_due);
            assert_eq!(
                still_owed, owed,
                "case {status:?}/{repeat_token:?}: the repeat must not move the instant \
                 the removal is owed, expected {owed:?}, got {still_owed:?}"
            );
        }
    }
}

/// A `scraperId` the store does not hold is the modelled `ResourceNotFoundException`
/// (404) naming the SCRAPER, whatever the id looks like: the configuration is a child of
/// the scraper, so the scraper is what is missing.
#[tokio::test]
async fn a_scraper_the_store_does_not_hold_is_not_found() {
    let region = "us-west-2";
    let amp = Amp::new();
    let held = "s-44444444-4444-4444-4444-444444444444";
    seed_pair(
        &amp,
        &logging_configuration(
            held,
            SCRAPER_LOGGING_STATUS_ACTIVE,
            cloud_watch_logs("aws/prometheus/bystander"),
            components(&["COLLECTOR"]),
        ),
        "ws-44444444-4444-4444-4444-444444444444",
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
            "S-44444444-4444-4444-4444-444444444444".to_string(),
        ),
    ];

    for (case, scraper_id) in absent {
        let reply = delete(&amp, region, &scraper_id, None).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an absent scraper must answer 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled ResourceNotFoundException, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["message"]),
            SCRAPER_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the scraper not-found sentence, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceId"]),
            scraper_id,
            "case {case:?}: the refusal must name the id asked for, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            SCRAPER_RESOURCE_TYPE,
            "case {case:?}: the refusal must name the resource type, got {reply:?}"
        );
    }

    assert_eq!(
        stored(&amp, held).map(|record| record.status_code),
        Some(SCRAPER_LOGGING_STATUS_ACTIVE.to_string()),
        "a delete that named another scraper must leave this configuration where it was, \
         got {:?}",
        stored(&amp, held)
    );
}

/// A scraper that holds NO logging configuration is the modelled
/// `ResourceNotFoundException` (404) naming the configuration, whatever status the
/// scraper itself is in — there is nothing to delete, and the scraper is left alone. The
/// catalog of scraper statuses is the model's own value list
/// (`crate::ops::types::SCRAPER_STATUS_CODE_VALUES`).
#[tokio::test]
async fn a_scraper_that_holds_no_configuration_is_not_found() {
    let region = "us-west-2";

    for status in SCRAPER_STATUS_CODE_VALUES {
        let amp = Amp::new();
        let scraper_id = "s-55555555-5555-5555-5555-555555555555";
        seed(
            &amp,
            &scraper(
                scraper_id,
                Some("unlogged"),
                status,
                eks_source("unlogged-cluster"),
                dataset_destination("unlogged-metrics"),
            ),
        );

        let reply = delete(&amp, region, scraper_id, Some("token-unlogged")).await;
        assert_eq!(
            reply.status, 404,
            "case {status:?}: a scraper with no configuration must answer 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {status:?}: expected the modelled ResourceNotFoundException, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["message"]),
            SCRAPER_LOGGING_NOT_FOUND_MESSAGE,
            "case {status:?}: expected the configuration not-found sentence, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceId"]),
            scraper_id,
            "case {status:?}: the refusal must name the scraper asked for, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            SCRAPER_LOGGING_RESOURCE_TYPE,
            "case {status:?}: the refusal must name the resource type, got {reply:?}"
        );
        assert_eq!(
            amp.store()
                .lock()
                .scrapers
                .get(scraper_id)
                .map(|record| record.status_code.clone()),
            Some((*status).to_string()),
            "case {status:?}: a refused delete must leave the scraper where it was"
        );
    }
}

/// Deleting one configuration is a change to that record only: every other scraper's
/// configuration keeps its status, its `modifiedAt` and the fact that nothing is owed on
/// it.
#[tokio::test]
async fn a_delete_touches_only_the_configuration_it_names() {
    let region = "us-west-2";
    let amp = Amp::new();
    let ids = [
        "s-aaaa1111-1111-1111-1111-111111111111",
        "s-bbbb2222-2222-2222-2222-222222222222",
        "s-cccc3333-3333-3333-3333-333333333333",
    ];
    for id in ids {
        seed_pair(
            &amp,
            &logging_configuration(
                id,
                SCRAPER_LOGGING_STATUS_ACTIVE,
                cloud_watch_logs("aws/prometheus/fleet"),
                components(&["COLLECTOR"]),
            ),
            "ws-66666666-6666-6666-6666-666666666666",
        );
    }

    let reply = delete(&amp, region, ids[1], None).await;
    assert_eq!(
        reply.status, 202,
        "case {:?}: the delete must be accepted, got {reply:?}",
        ids[1]
    );

    for id in ids {
        let record = stored(&amp, id)
            .unwrap_or_else(|| panic!("case {id:?}: every configuration is still held"));
        if id == ids[1] {
            assert_eq!(
                record.status_code, SCRAPER_LOGGING_STATUS_DELETING,
                "case {id:?}: expected the status the call wrote, got {:?}",
                record.status_code
            );
            continue;
        }
        assert_eq!(
            record.status_code, SCRAPER_LOGGING_STATUS_ACTIVE,
            "case {id:?}: a configuration nobody deleted keeps its status, got {:?}",
            record.status_code
        );
        assert_eq!(
            record.modified_at,
            seeded_at(),
            "case {id:?}: a configuration nobody deleted keeps its modifiedAt, got {:?}",
            record.modified_at
        );
        assert_eq!(
            record.transition_due, None,
            "case {id:?}: a configuration nobody deleted owes nothing, got {:?}",
            record.transition_due
        );
    }
}

/// A logging configuration belongs to its scraper, so the scraper's own removal takes it
/// with it: once `DeleteScraper`'s owed removal has come due, the configuration is gone
/// too and a delete that names it answers for the missing SCRAPER.
#[tokio::test]
async fn a_scraper_the_clock_removes_takes_its_configuration_with_it() {
    let region = "us-west-2";
    let amp = Amp::new();
    let scraper_id = "s-77777777-7777-7777-7777-777777777777";
    seed_pair(
        &amp,
        &logging_configuration(
            scraper_id,
            SCRAPER_LOGGING_STATUS_ACTIVE,
            cloud_watch_logs("aws/prometheus/doomed"),
            components(&["COLLECTOR", "EXPORTER"]),
        ),
        "ws-77777777-7777-7777-7777-777777777777",
    );

    let deleted = amp
        .call("DELETE", &format!("/scrapers/{scraper_id}"), region, "")
        .await;
    assert_eq!(
        deleted.status, 202,
        "deleting the scraper itself must be accepted, got {deleted:?}"
    );

    wait_for_due_transition();

    let reply = delete(&amp, region, scraper_id, None).await;
    assert_eq!(
        reply.status, 404,
        "the scraper is gone, so its configuration is too, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["message"]),
        SCRAPER_NOT_FOUND_MESSAGE,
        "expected the scraper not-found sentence, got {reply:?}"
    );
    assert_eq!(
        stored(&amp, scraper_id).map(|record| record.status_code),
        None,
        "the store must no longer hold the configuration, got {:?}",
        stored(&amp, scraper_id)
    );
}
