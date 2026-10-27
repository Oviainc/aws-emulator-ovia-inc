// SPDX-License-Identifier: CC0-1.0

//! ONE rule, over every create in this service that grows a bounded collection: the 402
//! `ServiceQuotaExceededException` a create at its ceiling answers is worded only from
//! WHAT was refused — the KIND of resource whose quota is reached — so the sentence is
//! the service's single quota form for every create path, and carries neither an
//! identifier the caller sent nor the ceiling's own value.
//!
//! The rows are the four collections a create grows: channel groups per Region, channels
//! per channel group, origin endpoints per channel, harvest jobs per origin endpoint.
//! Each row fills its collection to the ceiling, asks for one more, and is asserted
//! against the same sentence template with its own kind's plural — which is what catches
//! a handler that words a refusal of its own for one kind only. The accepted side is
//! asserted too: every create up to the ceiling is served, and the refused one stores
//! nothing.

use serde_json::json;

use crate::channel_lifecycle::error_type;
use crate::rejection_rule_and_negative_paths::{
    epoch_seconds_from_now, harvest_body, Answer, Emulator,
};

/// The quota sentence this service answers a create at its ceiling with, as a function of
/// the refused KIND's plural noun — the only part of it that varies.
///
/// Written out here (rather than imported) so the test pins the wording a client matches
/// on; the plurals the rows pass in are the nouns `state::ResourceKind::plural` yields for
/// the four kinds the model's `ResourceTypeNotFound` enumerates (`CHANNEL_GROUP`,
/// `CHANNEL`, `ORIGIN_ENDPOINT`, `HARVEST_JOB`).
fn quota_sentence(plural: &str) -> String {
    format!(
        "MediaPackage can't create the resource you requested because you've reached the \
         quota for {plural}. Delete {plural} you no longer need, or request a quota \
         increase, and then try again"
    )
}

/// Assert the rule on one refusal: 402, the modelled error code, the service's one quota
/// sentence for `plural`, no digit in it (the ceiling's value is a fact about the account,
/// not about the refusal) and none of `sent` — the identifiers the refused request
/// carried.
fn assert_quota_form(case: &str, plural: &str, answer: &Answer, sent: &[&str]) {
    assert_eq!(
        answer.status, 402,
        "case {case:?}: expected HTTP 402, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        error_type(&answer.json),
        "ServiceQuotaExceededException",
        "case {case:?}: expected a ServiceQuotaExceededException envelope, got {}",
        answer.body
    );
    let message = answer.string("Message");
    assert_eq!(
        message,
        quota_sentence(plural),
        "case {case:?}: expected the service's one quota sentence, got {message:?}"
    );
    assert!(
        !message.chars().any(|c| c.is_ascii_digit()),
        "case {case:?}: the quota's value must not be in the refusal, got {message:?}"
    );
    for value in sent {
        assert!(
            !message.contains(value),
            "case {case:?}: the message must not echo the request's {value:?}, got \
             {message:?}"
        );
    }
}

/// Fill one Region's channel-group collection to its ceiling and ask for one more.
async fn channel_groups_past_the_ceiling() -> Answer {
    use service_com_amazonaws_mediapackagev2_mediapackagev2::state::MAX_CHANNEL_GROUPS;

    let emulator = Emulator::new();
    for index in 0..MAX_CHANNEL_GROUPS {
        let created = emulator
            .create_group(&format!("quota-grp-{index}"), json!({}))
            .await;
        assert_eq!(
            created.status, 200,
            "channel group {index} of {MAX_CHANNEL_GROUPS} must be created, got {} with \
             body {}",
            created.status, created.body
        );
    }
    let refused = emulator.create_group("grp-one-too-many", json!({})).await;
    let read = emulator
        .call("GET", "/channelGroup/grp-one-too-many", None)
        .await;
    assert_eq!(
        read.status, 404,
        "a refused channel-group create must store nothing, got {} with body {}",
        read.status, read.body
    );
    refused
}

/// Fill one channel group's channel collection to its ceiling and ask for one more.
async fn channels_past_the_ceiling() -> Answer {
    use service_com_amazonaws_mediapackagev2_mediapackagev2::state::MAX_CHANNELS_PER_GROUP;

    let emulator = Emulator::new();
    let created = emulator.create_group("quota-chan-grp", json!({})).await;
    assert_eq!(
        created.status, 200,
        "the fixture channel group must be created, got {} with body {}",
        created.status, created.body
    );
    for index in 0..MAX_CHANNELS_PER_GROUP {
        let created = emulator
            .create_channel("quota-chan-grp", &format!("quota-chan-{index}"), json!({}))
            .await;
        assert_eq!(
            created.status, 200,
            "channel {index} of {MAX_CHANNELS_PER_GROUP} must be created, got {} with \
             body {}",
            created.status, created.body
        );
    }
    let refused = emulator
        .create_channel("quota-chan-grp", "chan-one-too-many", json!({}))
        .await;
    let read = emulator
        .call(
            "GET",
            "/channelGroup/quota-chan-grp/channel/chan-one-too-many",
            None,
        )
        .await;
    assert_eq!(
        read.status, 404,
        "a refused channel create must store nothing, got {} with body {}",
        read.status, read.body
    );
    refused
}

/// Fill one channel's origin-endpoint collection to its ceiling and ask for one more.
async fn origin_endpoints_past_the_ceiling() -> Answer {
    use service_com_amazonaws_mediapackagev2_mediapackagev2::state::MAX_ORIGIN_ENDPOINTS_PER_CHANNEL;

    let emulator = Emulator::new();
    let created = emulator.create_group("quota-ep-grp", json!({})).await;
    assert_eq!(
        created.status, 200,
        "the fixture channel group must be created, got {} with body {}",
        created.status, created.body
    );
    let created = emulator
        .create_channel("quota-ep-grp", "quota-ep-chan", json!({}))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture channel must be created, got {} with body {}",
        created.status, created.body
    );
    let endpoint_body = json!({
        "ContainerType": "TS",
        "HlsManifests": [{"ManifestName": "index"}],
    });
    for index in 0..MAX_ORIGIN_ENDPOINTS_PER_CHANNEL {
        let created = emulator
            .create_endpoint(
                "quota-ep-grp",
                "quota-ep-chan",
                &format!("quota-ep-{index}"),
                endpoint_body.clone(),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "origin endpoint {index} of {MAX_ORIGIN_ENDPOINTS_PER_CHANNEL} must be \
             created, got {} with body {}",
            created.status, created.body
        );
    }
    let refused = emulator
        .create_endpoint(
            "quota-ep-grp",
            "quota-ep-chan",
            "ep-one-too-many",
            endpoint_body,
        )
        .await;
    let read = emulator
        .call(
            "GET",
            "/channelGroup/quota-ep-grp/channel/quota-ep-chan/originEndpoint/\
             ep-one-too-many",
            None,
        )
        .await;
    assert_eq!(
        read.status, 404,
        "a refused origin-endpoint create must store nothing, got {} with body {}",
        read.status, read.body
    );
    refused
}

/// Fill one origin endpoint's harvest-job collection to its ceiling and ask for one more.
async fn harvest_jobs_past_the_ceiling() -> Answer {
    use service_com_amazonaws_mediapackagev2_mediapackagev2::state::MAX_HARVEST_JOBS_PER_ORIGIN_ENDPOINT;

    let emulator = Emulator::new();
    let created = emulator.create_group("quota-job-grp", json!({})).await;
    assert_eq!(
        created.status, 200,
        "the fixture channel group must be created, got {} with body {}",
        created.status, created.body
    );
    let created = emulator
        .create_channel("quota-job-grp", "quota-job-chan", json!({}))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture channel must be created, got {} with body {}",
        created.status, created.body
    );
    let created = emulator
        .create_endpoint(
            "quota-job-grp",
            "quota-job-chan",
            "quota-job-ep",
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture origin endpoint must be created, got {} with body {}",
        created.status, created.body
    );

    let jobs_path = "/channelGroup/quota-job-grp/channel/quota-job-chan/originEndpoint\
                     /quota-job-ep/harvestJob";
    // A window that opens in the future and never closes during the test, so every job
    // stays in the status the create gave it.
    let start = epoch_seconds_from_now(600.0);
    for index in 0..MAX_HARVEST_JOBS_PER_ORIGIN_ENDPOINT {
        let mut body = harvest_body(start, start + 600.0, &["index"]);
        body["HarvestJobName"] = json!(format!("quota-job-{index}"));
        let created = emulator.call("POST", jobs_path, Some(body)).await;
        assert_eq!(
            created.status, 200,
            "harvest job {index} of {MAX_HARVEST_JOBS_PER_ORIGIN_ENDPOINT} must be \
             created, got {} with body {}",
            created.status, created.body
        );
    }
    let mut body = harvest_body(start, start + 600.0, &["index"]);
    body["HarvestJobName"] = json!("job-one-too-many");
    let refused = emulator.call("POST", jobs_path, Some(body)).await;
    let read = emulator
        .call("GET", &format!("{jobs_path}/job-one-too-many"), None)
        .await;
    assert_eq!(
        read.status, 404,
        "a refused harvest-job create must store nothing, got {} with body {}",
        read.status, read.body
    );
    refused
}

#[tokio::test]
async fn every_create_at_its_ceiling_is_refused_in_the_services_one_quota_form() {
    // (the case, the refused kind's plural, the refusal, the identifiers the refused
    // request carried). One row per collection a create grows, so the sentence is held to
    // the same template at every level of the hierarchy.
    let rows: Vec<(&str, &str, Answer, Vec<&str>)> = vec![
        (
            "a channel group past the per-Region ceiling",
            "channel groups",
            channel_groups_past_the_ceiling().await,
            vec!["grp-one-too-many", "quota-grp-0"],
        ),
        (
            "a channel past its group's ceiling",
            "channels",
            channels_past_the_ceiling().await,
            vec!["chan-one-too-many", "quota-chan-grp"],
        ),
        (
            "an origin endpoint past its channel's ceiling",
            "origin endpoints",
            origin_endpoints_past_the_ceiling().await,
            vec!["ep-one-too-many", "quota-ep-grp", "quota-ep-chan"],
        ),
        (
            "a harvest job past its origin endpoint's ceiling",
            "harvest jobs",
            harvest_jobs_past_the_ceiling().await,
            vec![
                "job-one-too-many",
                "quota-job-grp",
                "quota-job-chan",
                "quota-job-ep",
            ],
        ),
    ];

    for (case, plural, answer, sent) in &rows {
        assert_quota_form(case, plural, answer, sent);
    }

    // The rule's own consequence: one sentence per KIND and nothing else varying, so two
    // rows read the same sentence exactly when they refused the same kind. With four
    // distinct kinds here, the four sentences are pairwise different, and each differs
    // from the others only in the plural noun.
    for (case, _, answer, _) in &rows {
        for (other_case, other_plural, other, _) in &rows {
            if case == other_case {
                continue;
            }
            assert_ne!(
                answer.string("Message"),
                quota_sentence(other_plural),
                "case {case:?}: must not read as the refusal of {other_case:?}, got {}",
                other.body
            );
        }
    }
}
