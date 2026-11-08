// SPDX-License-Identifier: CC0-1.0

//! ONE rule, over every level of the hierarchy and every operation that can miss one of
//! them: a `ResourceNotFoundException` states the sentence this service words for the
//! KIND of resource that is missing — the kind's own noun, the word it calls that kind's
//! identifier, and the verb it makes one with — and carries nothing the caller sent.
//!
//! The harvest job is the kind whose sentence differs from the three named levels above
//! it: it is addressed by an `id` the caller `create`s, where a channel group, a channel
//! and an origin endpoint are addressed by a `name` the caller `add`s. One wording per
//! kind, whichever operation reports it. The sentences themselves, and the assertion that
//! pins them, are the crate's shared ones
//! ([`crate::rejection_rule_and_negative_paths`]); this suite adds the rule they are a
//! property of the KIND rather than of the operation.
//!
//! The rule is asserted over a SET of inputs: both operations that address one harvest
//! job (`GetHarvestJob`, `CancelHarvestJob`), differently shaped absent job names, an
//! endpoint that already holds other jobs, a job name that exists under a SIBLING
//! channel, and — on those same two operations — each of the three levels above the job
//! missing in turn, so the kind that is reported is the kind the lookup actually missed.

use serde_json::{json, Value};

use crate::rejection_rule_and_negative_paths::{
    assert_fixed_refusal, epoch_seconds_from_now, harvest_body, Emulator,
    CHANNEL_GROUP_NOT_FOUND, CHANNEL_NOT_FOUND, HARVEST_JOB_NOT_FOUND,
    ORIGIN_ENDPOINT_NOT_FOUND,
};

/// The hierarchy every row addresses, and the sibling channel a row borrows a job name
/// from.
const GROUP: &str = "nf-wording-grp";
const CHANNEL: &str = "nf-wording-chan";
const ENDPOINT: &str = "nf-wording-ep";
const SIBLING_CHANNEL: &str = "nf-wording-sib";
const SIBLING_ENDPOINT: &str = "nf-wording-sib-ep";

/// The manifest both endpoints publish, and so the one a harvest job may harvest.
const MANIFEST: &str = "index";

/// The job the sibling channel holds, and the one the fixture endpoint holds: an absent
/// job is looked up beside jobs that DO exist, so the lookup misses the job rather than
/// an empty collection.
const SIBLING_JOB: &str = "sibling-job";
const PRESENT_JOB: &str = "present-job";

/// The modelled code every row is refused with, and the discriminator member that carries
/// the kind (shape `ResourceNotFoundException`: `Message`, `ResourceTypeNotFound`).
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";
const KIND_MEMBER: &str = "ResourceTypeNotFound";

/// One level of the hierarchy, as a 404 about it reads: the `ResourceTypeNotFound` value,
/// the noun a case label names it by, and the crate's fixed sentence for that kind.
///
/// The rows are the model's whole `ResourceTypeNotFound` enum (`smithy_query query=shape
/// shape=ResourceTypeNotFound`: `CHANNEL_GROUP`, `CHANNEL`, `ORIGIN_ENDPOINT`,
/// `HARVEST_JOB`), so no kind is left untested.
struct Kind {
    resource_type: &'static str,
    noun: &'static str,
    message: &'static str,
}

const CHANNEL_GROUP_KIND: Kind = Kind {
    resource_type: "CHANNEL_GROUP",
    noun: "channel group",
    message: CHANNEL_GROUP_NOT_FOUND,
};
const CHANNEL_KIND: Kind = Kind {
    resource_type: "CHANNEL",
    noun: "channel",
    message: CHANNEL_NOT_FOUND,
};
const ORIGIN_ENDPOINT_KIND: Kind = Kind {
    resource_type: "ORIGIN_ENDPOINT",
    noun: "origin endpoint",
    message: ORIGIN_ENDPOINT_NOT_FOUND,
};
const HARVEST_JOB_KIND: Kind = Kind {
    resource_type: "HARVEST_JOB",
    noun: "harvest job",
    message: HARVEST_JOB_NOT_FOUND,
};

/// The group, two channels, an endpoint on each, and one harvest job on each endpoint —
/// so every absent-job row is answered beside jobs that exist.
async fn hierarchy() -> Emulator {
    let emulator = Emulator::new();
    let created = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        created.status, 200,
        "the fixture channel group must be created, got {} with body {}",
        created.status, created.body
    );
    for (channel, endpoint, job) in [
        (CHANNEL, ENDPOINT, PRESENT_JOB),
        (SIBLING_CHANNEL, SIBLING_ENDPOINT, SIBLING_JOB),
    ] {
        let created = emulator.create_channel(GROUP, channel, json!({})).await;
        assert_eq!(
            created.status, 200,
            "the fixture channel {channel} must be created, got {} with body {}",
            created.status, created.body
        );
        let created = emulator
            .create_endpoint(
                GROUP,
                channel,
                endpoint,
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": [{"ManifestName": MANIFEST}],
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "the fixture endpoint {endpoint} must be created, got {} with body {}",
            created.status, created.body
        );
        let start = epoch_seconds_from_now(600.0);
        let mut body = harvest_body(start, start + 3_600.0, &[MANIFEST]);
        body["HarvestJobName"] = json!(job);
        let created = emulator
            .call(
                "POST",
                &format!(
                    "/channelGroup/{GROUP}/channel/{channel}/originEndpoint/{endpoint}\
                     /harvestJob"
                ),
                Some(body),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "the fixture harvest job {job} must be created, got {} with body {}",
            created.status, created.body
        );
    }
    emulator
}

/// The path one harvest job is addressed at.
fn job_path(group: &str, channel: &str, endpoint: &str, job: &str) -> String {
    format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}/harvestJob/{job}")
}

/// The two operations that address ONE harvest job, with the method each uses:
/// `GetHarvestJob` reads it, `CancelHarvestJob` cancels it (model: `GET` and `PUT` on
/// `.../harvestJob/{HarvestJobName}`).
const JOB_OPERATIONS: &[(&str, &str)] = &[("GetHarvestJob", "GET"), ("CancelHarvestJob", "PUT")];

#[tokio::test]
async fn an_absent_harvest_job_is_reported_in_the_harvest_job_wording() {
    let emulator = hierarchy().await;
    // (label, the absent job name, whether the name can be echo-checked). Job names no job
    // of this channel holds, differing in shape — length, case, digits, separators — plus
    // one held by a job of the SIBLING channel and one differing from a stored job only in
    // case, since a message echoing the caller's value would differ on each of them.
    //
    // A one-character name is the shortest `ResourceName` the model admits (length 1..256)
    // and is kept as that boundary, but it is not handed to the echo check: that check is a
    // substring search, and a single letter occurs inside ordinary English, so it could not
    // tell an echo from the sentence (the rule this crate's identifier rows record).
    let absent: &[(&str, &str, bool)] = &[
        ("a one-character name", "x", false),
        ("a dashed name", "ghost-job", true),
        ("an underscored, mixed-case name", "Ghost_Job-2", true),
        ("a digit run", "0123456789", true),
        (
            "a long name",
            "a-very-long-harvest-job-name-that-was-never-created-0001",
            true,
        ),
        (
            "a name differing from a stored job only in case",
            "PRESENT-JOB",
            true,
        ),
        ("a name held by a job of a sibling channel", SIBLING_JOB, true),
    ];

    for (operation, method) in JOB_OPERATIONS {
        for (label, job, echo_checked) in absent {
            let case = format!("{operation} on {label}");
            let answer = emulator
                .call(method, &job_path(GROUP, CHANNEL, ENDPOINT, job), None)
                .await;
            let mut identifiers = vec![GROUP, CHANNEL, ENDPOINT];
            if *echo_checked {
                identifiers.push(job);
            }
            assert_fixed_refusal(
                &case,
                &answer,
                404,
                NOT_FOUND_CODE,
                (KIND_MEMBER, HARVEST_JOB_KIND.resource_type),
                HARVEST_JOB_KIND.message,
                &identifiers,
            );
        }
    }

    // The rule's other side: the job that DOES exist is still served by both operations,
    // so the wording above is a refusal of the lookup and not of the path.
    let path = job_path(GROUP, CHANNEL, ENDPOINT, PRESENT_JOB);
    let read = emulator.call("GET", &path, None).await;
    assert_eq!(
        read.status, 200,
        "case \"GetHarvestJob on the stored job\": expected HTTP 200, got {} with body {}",
        read.status, read.body
    );
    assert_eq!(
        read.string("HarvestJobName"),
        PRESENT_JOB,
        "case \"GetHarvestJob on the stored job\": wrong job reported, got {}",
        read.body
    );
    let cancelled = emulator.call("PUT", &path, None).await;
    assert_eq!(
        cancelled.status, 200,
        "case \"CancelHarvestJob on the stored job\": expected HTTP 200, got {} with body {}",
        cancelled.status, cancelled.body
    );
    let read = emulator.call("GET", &path, None).await;
    assert_eq!(
        read.string("Status"),
        "CANCELLED",
        "case \"CancelHarvestJob on the stored job\": the cancellation must be stored, got {}",
        read.body
    );
}

#[tokio::test]
async fn an_absent_level_above_the_job_is_reported_in_that_levels_wording() {
    let emulator = hierarchy().await;
    // (the kind the lookup misses, the path's group / channel / endpoint). The job name is
    // one that exists, so each row's 404 can only be about the level named absent.
    let rows: &[(&Kind, &str, &str, &str)] = &[
        (&CHANNEL_GROUP_KIND, "ghost-grp", CHANNEL, ENDPOINT),
        (&CHANNEL_KIND, GROUP, "ghost-chan", ENDPOINT),
        (&ORIGIN_ENDPOINT_KIND, GROUP, CHANNEL, "ghost-ep"),
    ];

    for (operation, method) in JOB_OPERATIONS {
        for (kind, group, channel, endpoint) in rows {
            let case = format!("{operation} on an absent {}", kind.noun);
            let answer = emulator
                .call(
                    method,
                    &job_path(group, channel, endpoint, PRESENT_JOB),
                    None,
                )
                .await;
            assert_fixed_refusal(
                &case,
                &answer,
                404,
                NOT_FOUND_CODE,
                (KIND_MEMBER, kind.resource_type),
                kind.message,
                &[group, channel, endpoint, PRESENT_JOB],
            );
        }
    }
}

#[tokio::test]
async fn every_kinds_wording_is_reported_by_the_operations_that_address_it_directly() {
    let emulator = hierarchy().await;
    // (label, method, path, the kind the lookup misses, what the request carried) — one
    // row per level, through an operation that addresses that level itself rather than
    // through the harvest-job path, so a kind's sentence is one sentence service-wide.
    let rows: Vec<(&str, &str, String, &Kind, Vec<&str>)> = vec![
        (
            "GetChannelGroup on an absent group",
            "GET",
            "/channelGroup/ghost-grp".to_string(),
            &CHANNEL_GROUP_KIND,
            vec!["ghost-grp"],
        ),
        (
            "ListHarvestJobs under an absent group",
            "GET",
            "/channelGroup/ghost-grp/harvestJob".to_string(),
            &CHANNEL_GROUP_KIND,
            vec!["ghost-grp"],
        ),
        (
            "GetChannel on an absent channel",
            "GET",
            format!("/channelGroup/{GROUP}/channel/ghost-chan"),
            &CHANNEL_KIND,
            vec![GROUP, "ghost-chan"],
        ),
        (
            "GetOriginEndpoint on an absent endpoint",
            "GET",
            format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/ghost-ep"),
            &ORIGIN_ENDPOINT_KIND,
            vec![GROUP, CHANNEL, "ghost-ep"],
        ),
        (
            "CreateHarvestJob under an absent endpoint",
            "POST",
            format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/ghost-ep/harvestJob"),
            &ORIGIN_ENDPOINT_KIND,
            vec![GROUP, CHANNEL, "ghost-ep"],
        ),
    ];

    for (label, method, path, kind, identifiers) in rows {
        let body: Option<Value> = match method {
            "POST" => {
                let start = epoch_seconds_from_now(600.0);
                let mut body = harvest_body(start, start + 3_600.0, &[MANIFEST]);
                body["HarvestJobName"] = json!("ghost-job");
                Some(body)
            }
            _ => None,
        };
        let answer = emulator.call(method, &path, body).await;
        assert_fixed_refusal(
            label,
            &answer,
            404,
            NOT_FOUND_CODE,
            (KIND_MEMBER, kind.resource_type),
            kind.message,
            &identifiers,
        );
    }
}
