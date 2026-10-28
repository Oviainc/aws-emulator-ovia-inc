// SPDX-License-Identifier: CC0-1.0

//! `UpdateScraperLoggingConfiguration`, end to end over HTTP: an accepted update REPLACES
//! the members the request carries on the configuration the store HOLDS, moves that
//! record to `UPDATING` with its settling owed on the clock, and is read back through
//! `DescribeScraperLoggingConfiguration` — so the write is observed through a later read
//! rather than through the response alone.
//!
//! The store is seeded directly here, through the same engine the mounted service reads
//! ([`Amp::store`]) and with the builders the deletion suite shares
//! ([`crate::scraper_logging_configuration_deletion`]): this model binds no operation that
//! CREATES a scraper logging configuration, so seeding is how a scraper holds one to
//! update at all — and it is the same record the handler writes, not a second copy of it.

use crate::scraper_listing::{eks_source, scraper, seed, workspace_destination};
use crate::scraper_logging_configuration_deletion::{
    cloud_watch_logs, components, logging_configuration, seed_pair, seeded_at,
};
use crate::scraper_logging_configuration_describe::{
    expected_components, expected_destination, seconds_since_epoch,
};
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::{
    ComponentConfig, ScraperComponent, ScraperLoggingDestination, SCRAPER_COMPONENT_TYPE_VALUES,
};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    ScraperLoggingConfiguration, SCRAPER_LOGGING_NOT_FOUND_MESSAGE,
    SCRAPER_LOGGING_RESOURCE_TYPE, SCRAPER_LOGGING_STATUS_ACTIVE, SCRAPER_LOGGING_STATUS_DELETING,
    SCRAPER_LOGGING_STATUS_UPDATING, SCRAPER_NOT_FOUND_MESSAGE, SCRAPER_RESOURCE_TYPE,
    STATUS_ACTIVE,
};
use std::collections::HashMap;

/// The members `UpdateScraperLoggingConfigurationResponse` declares, from the model
/// (`smithy_query(query="operation",operation="UpdateScraperLoggingConfiguration")`):
/// `status` alone, and it is `@required`.
const RESPONSE_MEMBERS: &[&str] = &["status"];

/// The workspace a seeded scraper sends its metrics to. Nothing here reads it back; it is
/// the destination every scraper record needs to exist.
const DESTINATION_WORKSPACE: &str = "ws-11111111-1111-1111-1111-111111111111";

/// `PUT /scrapers/{scraperId}/logging-configuration`, carrying the `@required`
/// `loggingDestination` and the optional `scraperComponents` when one is given.
async fn update(
    amp: &Amp,
    region: &str,
    scraper_id: &str,
    destination: &ScraperLoggingDestination,
    scraper_components: Option<&[ScraperComponent]>,
) -> Reply {
    let mut body = serde_json::Map::new();
    body.insert(
        "loggingDestination".to_string(),
        expected_destination(destination),
    );
    if let Some(scraper_components) = scraper_components {
        body.insert(
            "scraperComponents".to_string(),
            expected_components(scraper_components),
        );
    }
    amp.call(
        "PUT",
        &format!("/scrapers/{scraper_id}/logging-configuration"),
        region,
        &serde_json::Value::Object(body).to_string(),
    )
    .await
}

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

/// The stored configuration of one scraper in `region`, or `None` when that store holds
/// none for it.
fn stored(amp: &Amp, region: &str, scraper_id: &str) -> Option<ScraperLoggingConfiguration> {
    amp.store()
        .lock()
        .region(region)
        .scraper_logging
        .get(scraper_id)
        .cloned()
}

/// Every assertion an accepted update's own response makes: the modelled 202, the status
/// of the record it wrote, and no member the response does not declare.
fn assert_accepted(case: &str, reply: &Reply, expected_status_code: &str) {
    assert_eq!(
        reply.status, 202,
        "case {case:?}: an accepted update is the modelled 202, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["status"]["statusCode"]),
        expected_status_code,
        "case {case:?}: expected statusCode {expected_status_code:?}, got {reply:?}"
    );
    assert!(
        reply.body["status"]["statusReason"].is_null(),
        "case {case:?}: an update nothing failed about carries no statusReason, got {:?}",
        reply.body["status"]["statusReason"]
    );
    let reported: Vec<String> = reply
        .body
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    for member in &reported {
        assert!(
            RESPONSE_MEMBERS.contains(&member.as_str()),
            "case {case:?}: {member:?} is not a member \
             UpdateScraperLoggingConfigurationResponse declares ({RESPONSE_MEMBERS:?}), got \
             {reply:?}"
        );
    }
}

/// Every assertion a refusal makes: the status and code the model declares, the message
/// for the kind of resource the refusal is about, and the resource it names.
fn assert_refused(
    case: &str,
    reply: &Reply,
    status: u16,
    code: &str,
    message: &str,
    resource_id: &str,
    resource_type: &str,
) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected the modelled {status}, got {reply:?}"
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case:?}: expected the modelled {code}, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["message"]),
        message,
        "case {case:?}: expected {message:?}, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["resourceId"]),
        resource_id,
        "case {case:?}: the refusal must name the id addressed, expected {resource_id:?}, got \
         {reply:?}"
    );
    assert_eq!(
        text(&reply.body["resourceType"]),
        resource_type,
        "case {case:?}: the refusal must name the resource type, expected {resource_type:?}, \
         got {reply:?}"
    );
    assert!(
        reply.body["status"].is_null(),
        "case {case:?}: a refused update reports no status, got {reply:?}"
    );
}

/// An accepted update writes the members it carries onto the STORED record, and the
/// describe of that record then reports them: `UPDATING` while the clock owes the
/// settling, `ACTIVE` once it has passed, with the new destination and components either
/// way.
///
/// One row per thing the rule generalises over — the destination log group, how many
/// components of which types the update covers and whether they carry options, whether
/// the request carries `scraperComponents` at all (the one optional member), and the
/// length of the `scraperId` addressing the record.
#[tokio::test]
async fn an_update_replaces_the_members_it_carries_on_the_stored_record() {
    let region = "us-west-2";
    let cases: Vec<(
        &str,
        ScraperLoggingConfiguration,
        ScraperLoggingDestination,
        Option<Vec<ScraperComponent>>,
    )> = vec![
        (
            "a new log group and one COLLECTOR component",
            logging_configuration(
                "s-11111111-1111-1111-1111-111111111111",
                SCRAPER_LOGGING_STATUS_ACTIVE,
                cloud_watch_logs("aws/prometheus/before"),
                components(&["SERVICE_DISCOVERY"]),
            ),
            cloud_watch_logs("aws/prometheus/after"),
            Some(components(&["COLLECTOR"])),
        ),
        (
            // The catalog of component types is the model's own value list
            // (`crate::ops::types::SCRAPER_COMPONENT_TYPE_VALUES`), so a type this test
            // never names by hand is covered too.
            "every component type the model lists",
            logging_configuration(
                "s-22222222-2222-2222-2222-222222222222",
                SCRAPER_LOGGING_STATUS_ACTIVE,
                cloud_watch_logs("aws/prometheus/one"),
                components(&["EXPORTER"]),
            ),
            cloud_watch_logs("aws/prometheus/all-components"),
            Some(components(SCRAPER_COMPONENT_TYPE_VALUES)),
        ),
        (
            // `ScraperComponent$config` is optional, so a component carrying none is
            // stored and reported without it.
            "a component carrying no config",
            logging_configuration(
                "s-33333333-3333-3333-3333-333333333333",
                SCRAPER_LOGGING_STATUS_ACTIVE,
                cloud_watch_logs("aws/prometheus/configured"),
                components(&["COLLECTOR", "EXPORTER"]),
            ),
            cloud_watch_logs("aws/prometheus/bare"),
            Some(vec![ScraperComponent {
                r#type: "COLLECTOR".to_string(),
                config: None,
            }]),
        ),
        (
            // `scraperComponents` is the one optional input member, and the described
            // record carries it `@required` at `len 1..`: an update that omits it leaves
            // the stored list standing rather than emptying it.
            "no scraperComponents at all, so the stored list stands",
            logging_configuration(
                "s",
                SCRAPER_LOGGING_STATUS_ACTIVE,
                cloud_watch_logs("aws/prometheus/tiny"),
                components(&["SERVICE_DISCOVERY", "COLLECTOR"]),
            ),
            cloud_watch_logs("aws/prometheus/tiny-moved"),
            None,
        ),
        (
            // `ScraperId` is `len 1..64, pattern ^[0-9A-Za-z][-.0-9A-Z_a-z]*$`: an id at
            // either end of that length addresses its record the same way.
            "a scraperId of the maximum modelled length",
            logging_configuration(
                &"s".repeat(64),
                SCRAPER_LOGGING_STATUS_ACTIVE,
                cloud_watch_logs("aws/prometheus/long"),
                components(&["EXPORTER"]),
            ),
            cloud_watch_logs("aws/prometheus/long-moved"),
            Some(vec![ScraperComponent {
                r#type: "SERVICE_DISCOVERY".to_string(),
                config: Some(ComponentConfig {
                    options: Some(HashMap::from([(
                        "level".to_string(),
                        "info".to_string(),
                    )])),
                }),
            }]),
        ),
    ];

    for (case, seeded, destination, scraper_components) in cases {
        let amp = Amp::new();
        seed_pair(&amp, region, &seeded, DESTINATION_WORKSPACE);

        let reply = update(
            &amp,
            region,
            &seeded.scraper_id,
            &destination,
            scraper_components.as_deref(),
        )
        .await;
        assert_accepted(case, &reply, SCRAPER_LOGGING_STATUS_UPDATING);

        // A member the request carried replaces the stored one; one it omitted leaves the
        // record alone.
        let expected_components_value = expected_components(
            scraper_components
                .as_deref()
                .unwrap_or(&seeded.scraper_components),
        );

        // The record is still settling, so the caller reading straight after the call
        // sees it being updated — with the new members already in place.
        let described = describe(&amp, region, &seeded.scraper_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the updated configuration must be describable, got {described:?}"
        );
        assert_eq!(
            text(&described.body["status"]["statusCode"]),
            SCRAPER_LOGGING_STATUS_UPDATING,
            "case {case:?}: an accepted update is still settling, got {described:?}"
        );
        assert_eq!(
            described.body["loggingDestination"],
            expected_destination(&destination),
            "case {case:?}: loggingDestination must be the one this update carried, expected \
             {:?}, got {described:?}",
            expected_destination(&destination)
        );
        assert_eq!(
            described.body["scraperComponents"], expected_components_value,
            "case {case:?}: scraperComponents must be {expected_components_value:?}, got \
             {described:?}"
        );
        let reported_modified_at = described.body["modifiedAt"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: modifiedAt is a @required Timestamp, got {:?}",
                described.body["modifiedAt"]
            )
        });
        let seeded_modified_at = seconds_since_epoch(seeded_at());
        assert!(
            reported_modified_at > seeded_modified_at,
            "case {case:?}: an update moves modifiedAt past the seeded {seeded_modified_at:?}, \
             got {reported_modified_at:?}"
        );

        // The settling is owed on the clock, not applied by the read: once the span has
        // passed the very same describe reports the terminal status, and the members this
        // update wrote are still the stored ones.
        wait_for_due_transition();
        let settled = describe(&amp, region, &seeded.scraper_id).await;
        assert_eq!(
            text(&settled.body["status"]["statusCode"]),
            SCRAPER_LOGGING_STATUS_ACTIVE,
            "case {case:?}: the clock must settle an accepted update, got {settled:?}"
        );
        assert_eq!(
            settled.body["loggingDestination"],
            expected_destination(&destination),
            "case {case:?}: a settled configuration keeps the destination it was given, got \
             {settled:?}"
        );
        assert_eq!(
            settled.body["scraperComponents"], expected_components_value,
            "case {case:?}: a settled configuration keeps the components it was given, got \
             {settled:?}"
        );
        let record = stored(&amp, region, &seeded.scraper_id)
            .unwrap_or_else(|| panic!("case {case:?}: the store must still hold the record"));
        assert!(
            record.transition_due.is_none(),
            "case {case:?}: a settled record owes the clock nothing, got {record:?}"
        );
    }
}

/// An update that addresses no stored record is the modelled `ResourceNotFoundException`,
/// naming the resource that is MISSING: the scraper when this account holds none, the
/// configuration when the scraper holds none. Nothing is written either way.
///
/// One row per way the record can be absent, including the Region: this service is
/// regional, so a scraper of another Region is not reachable from this endpoint.
#[tokio::test]
async fn an_update_addressing_no_stored_record_is_the_modelled_not_found() {
    let region = "us-west-2";
    let absent_scraper = "s-00000000-0000-0000-0000-000000000000";
    let seeded = logging_configuration(
        "s-44444444-4444-4444-4444-444444444444",
        SCRAPER_LOGGING_STATUS_ACTIVE,
        cloud_watch_logs("aws/prometheus/elsewhere"),
        components(&["COLLECTOR"]),
    );
    // A scraper this Region's store HOLDS, with no logging configuration on it.
    let unconfigured = "s-55555555-5555-5555-5555-555555555555";

    let cases: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "a scraper no store holds",
            region,
            absent_scraper,
            SCRAPER_NOT_FOUND_MESSAGE,
            SCRAPER_RESOURCE_TYPE,
        ),
        (
            "a scraper of another Region",
            "eu-west-1",
            &seeded.scraper_id,
            SCRAPER_NOT_FOUND_MESSAGE,
            SCRAPER_RESOURCE_TYPE,
        ),
        (
            "a scraper that holds no logging configuration",
            region,
            unconfigured,
            SCRAPER_LOGGING_NOT_FOUND_MESSAGE,
            SCRAPER_LOGGING_RESOURCE_TYPE,
        ),
    ];

    for (case, addressed_region, scraper_id, message, resource_type) in cases {
        let amp = Amp::new();
        seed_pair(&amp, region, &seeded, DESTINATION_WORKSPACE);
        seed(
            &amp,
            region,
            &scraper(
                unconfigured,
                Some("unconfigured"),
                STATUS_ACTIVE,
                eks_source("logging-cluster"),
                workspace_destination(DESTINATION_WORKSPACE),
            ),
        );

        let reply = update(
            &amp,
            addressed_region,
            scraper_id,
            &cloud_watch_logs("aws/prometheus/refused"),
            Some(&components(&["EXPORTER"])),
        )
        .await;
        assert_refused(
            case,
            &reply,
            404,
            "ResourceNotFoundException",
            message,
            scraper_id,
            resource_type,
        );

        // A refused update writes nothing: the configuration this account does hold is
        // exactly as it was seeded.
        let record = stored(&amp, region, &seeded.scraper_id)
            .unwrap_or_else(|| panic!("case {case:?}: the seeded record must still be held"));
        assert_eq!(
            record.status_code, SCRAPER_LOGGING_STATUS_ACTIVE,
            "case {case:?}: a refused update moves no status, got {record:?}"
        );
        assert_eq!(
            record.modified_at,
            seeded_at(),
            "case {case:?}: a refused update moves no modifiedAt, got {record:?}"
        );
        assert_eq!(
            expected_destination(&record.logging_destination),
            expected_destination(&seeded.logging_destination),
            "case {case:?}: a refused update replaces no destination, got {record:?}"
        );
        assert!(
            stored(&amp, region, unconfigured).is_none(),
            "case {case:?}: a refused update creates no configuration"
        );
    }
}

/// A configuration whose deletion has already been accepted is on its way out, so
/// updating it would leave an inconsistent state: the modelled `ConflictException` (409),
/// naming the configuration and the status it is in, and the stored record is left as the
/// deletion wrote it.
#[tokio::test]
async fn an_update_of_a_configuration_being_deleted_is_the_modelled_conflict() {
    let region = "us-west-2";
    let seeded = logging_configuration(
        "s-66666666-6666-6666-6666-666666666666",
        SCRAPER_LOGGING_STATUS_ACTIVE,
        cloud_watch_logs("aws/prometheus/going"),
        components(&["COLLECTOR"]),
    );
    let amp = Amp::new();
    seed_pair(&amp, region, &seeded, DESTINATION_WORKSPACE);

    let deleted = amp
        .call(
            "DELETE",
            &format!("/scrapers/{}/logging-configuration", seeded.scraper_id),
            region,
            "",
        )
        .await;
    assert_eq!(
        deleted.status, 202,
        "the deletion must be accepted, got {deleted:?}"
    );
    let pending = stored(&amp, region, &seeded.scraper_id)
        .expect("the record is still held while its removal is owed");

    let reply = update(
        &amp,
        region,
        &seeded.scraper_id,
        &cloud_watch_logs("aws/prometheus/too-late"),
        Some(&components(&["EXPORTER"])),
    )
    .await;
    assert_refused(
        "a configuration whose deletion is owed",
        &reply,
        409,
        "ConflictException",
        &format!("The scraper logging configuration is {SCRAPER_LOGGING_STATUS_DELETING}"),
        &seeded.scraper_id,
        SCRAPER_LOGGING_RESOURCE_TYPE,
    );

    let after = stored(&amp, region, &seeded.scraper_id)
        .expect("a refused update removes nothing from the store");
    assert_eq!(
        after.status_code, SCRAPER_LOGGING_STATUS_DELETING,
        "a refused update leaves the pending deletion in place, got {after:?}"
    );
    assert_eq!(
        after.modified_at, pending.modified_at,
        "a refused update moves no modifiedAt, got {after:?}"
    );
    assert_eq!(
        expected_destination(&after.logging_destination),
        expected_destination(&seeded.logging_destination),
        "a refused update replaces no destination, got {after:?}"
    );
}
