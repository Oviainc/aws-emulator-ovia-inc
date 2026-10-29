// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH channels `CreateHarvestJob` exports content from, driven
//! over HTTP through the same router the emulator boots.
//!
//! The rules under test, each asserted over a SET of channels and request bodies rather
//! than one recorded request:
//!
//! 1. A harvest job exports the content the addressed channel ingested, so a channel that
//!    receives no ingest of its own — the state the model's `InputType` doc describes for
//!    `MULTIVIEW` ("The channel receives no ingest of its own. Instead, it composites
//!    video from the source channels in its MultiviewConfiguration") — has nothing to
//!    export and is refused with `ValidationException` (400) and
//!    `ValidationExceptionType` `MULTIVIEW_INPUT_TYPE_WITH_HARVEST_JOB`, the type the
//!    model names for this case, storing no job; an endpoint of a channel of every other
//!    input type the model declares is harvested.
//! 2. The refusal is keyed on the ADDRESSED channel's own input type and on nothing else:
//!    it holds whatever the request's other members carry, and the source channels of a
//!    refused multiview channel are harvested normally.

use serde_json::{json, Value};

use crate::channel_lifecycle::error_type;
use crate::rejection_rule_and_negative_paths::{
    epoch_seconds_from_now, harvest_body, Answer, Emulator,
};

/// The channel group every row below is created in.
const GROUP: &str = "harvest-input-types";

/// The CMAF channels the multiview rows list as tile sources:
/// `MultiviewConfiguration$AvailableSources`' doc requires each source to be in the same
/// channel group and to have an `InputType` of `CMAF`.
const SOURCES: [&str; 2] = ["harvest-source-one", "harvest-source-two"];

/// Layout values, from the model's `MultiviewLayoutType` enum (LAYOUT_2EH | LAYOUT_2PL |
/// LAYOUT_3EL | LAYOUT_3PL | LAYOUT_4E | LAYOUT_4PL). The rows use entries the reported
/// case did not cite as well as the whole catalogue, which `AvailableLayouts`' `@length`
/// of 1..6 also makes the longest legal list.
const LAYOUTS: [&str; 6] = [
    "LAYOUT_2EH",
    "LAYOUT_2PL",
    "LAYOUT_3EL",
    "LAYOUT_3PL",
    "LAYOUT_4E",
    "LAYOUT_4PL",
];

/// The manifest every endpoint in this suite publishes, and the one every harvest request
/// asks for: the requested manifests are not what these rows vary.
const PUBLISHED_HLS: &str = "index";

/// The fixed envelope every refused request is reported in
/// (`state::validation_message`).
const ENVELOPE: &str = "Validation error encountered at request";

/// The `ValidationExceptionType` of an error envelope.
fn validation_type(answer: &Answer) -> &str {
    answer
        .json
        .get("ValidationExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The `Message` of an error envelope.
fn message(answer: &Answer) -> &str {
    answer
        .json
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// One row: a channel to create, and whether a harvest job against an endpoint of it is
/// refused for the channel having no ingest of its own.
struct Row {
    label: &'static str,
    channel: &'static str,
    body: fn() -> Value,
    refused: bool,
}

/// The rows span the model's whole `InputType` enum (CMAF | HLS | MULTIVIEW) plus the
/// absent case the member's doc defaults to HLS, and vary the multiview channels along
/// source count, layout count and the members a harvest job is indifferent to.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "input type absent (defaults to HLS)",
            channel: "harvest-plain",
            body: || json!({}),
            refused: false,
        },
        Row {
            label: "HLS input, described and tagged",
            channel: "harvest-hls",
            body: || {
                json!({"InputType": "HLS", "Description": "ingests HLS", "tags": {"k": "v"}})
            },
            refused: false,
        },
        Row {
            label: "CMAF input",
            channel: "harvest-cmaf",
            body: || json!({"InputType": "CMAF"}),
            refused: false,
        },
        Row {
            label: "MULTIVIEW: one source, one layout the report did not cite",
            channel: "harvest-mv-one",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": {
                        "AvailableSources": [SOURCES[1]],
                        "AvailableLayouts": [LAYOUTS[4]],
                    },
                })
            },
            refused: true,
        },
        Row {
            label: "MULTIVIEW: every source, the whole layout catalogue",
            channel: "harvest-mv-every",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": {
                        "AvailableSources": SOURCES,
                        "AvailableLayouts": LAYOUTS,
                    },
                })
            },
            refused: true,
        },
        Row {
            label: "MULTIVIEW: described and tagged",
            channel: "harvest-mv-described",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "Description": "composites two sources",
                    "tags": {"a": "1", "b": "2"},
                    "MultiviewConfiguration": {
                        "AvailableSources": SOURCES,
                        "AvailableLayouts": [LAYOUTS[1], LAYOUTS[5]],
                    },
                })
            },
            refused: true,
        },
    ]
}

/// The harvest-request shapes each row is driven with: the rule is about the channel, so
/// it must hold whether the job is named by the caller or by the service, and whatever
/// description and tags the request carries.
fn request_variants(start: f64, suffix: &str) -> Vec<(String, Value)> {
    let window = |label: &str, extra: Value| {
        let mut body = harvest_body(start, start + 600.0, &[PUBLISHED_HLS]);
        if let Value::Object(members) = extra {
            for (member, value) in members {
                body[member] = value;
            }
        }
        (format!("{label} ({suffix})"), body)
    };
    vec![
        window(
            "named by the caller",
            json!({"HarvestJobName": format!("job-{suffix}")}),
        ),
        window("named by the service", json!({})),
        window(
            "described and tagged",
            json!({
                "HarvestJobName": format!("job-{suffix}-tagged"),
                "Description": "an export of the last ten minutes",
                "Tags": {"team": "video"},
            }),
        ),
        window(
            "a long window far ahead",
            json!({
                "HarvestJobName": format!("job-{suffix}-long"),
                "ScheduleConfiguration": {
                    "StartTime": start + 86_400.0,
                    "EndTime": start + 86_400.0 + 7_200.0,
                },
            }),
        ),
    ]
}

/// [`GROUP`] with its [`SOURCES`] in place, each with an origin endpoint of its own, so
/// the multiview rows resolve and the sources are themselves harvestable.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let created = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        created.status, 200,
        "setup: creating group {GROUP} must answer 200, got {} with body {}",
        created.status, created.body
    );
    for source in SOURCES {
        let created = emulator
            .create_channel(GROUP, source, json!({"InputType": "CMAF"}))
            .await;
        assert_eq!(
            created.status, 200,
            "setup: creating source {source} must answer 200, got {} with body {}",
            created.status, created.body
        );
        create_endpoint(&emulator, source).await;
    }
    emulator
}

/// The origin endpoint of one channel: `CMAF` container with a single published HLS
/// manifest, the endpoint shape every row harvests from.
async fn create_endpoint(emulator: &Emulator, channel: &str) -> String {
    let endpoint = format!("{channel}-ep");
    let created = emulator
        .create_endpoint(
            GROUP,
            channel,
            &endpoint,
            json!({
                "ContainerType": "CMAF",
                "HlsManifests": [{"ManifestName": PUBLISHED_HLS}],
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "setup: creating endpoint {endpoint} must answer 200, got {} with body {}",
        created.status, created.body
    );
    endpoint
}

/// `POST .../harvestJob` under one endpoint.
async fn create_job(emulator: &Emulator, channel: &str, endpoint: &str, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!(
                "/channelGroup/{GROUP}/channel/{channel}/originEndpoint/{endpoint}/harvestJob"
            ),
            Some(body),
        )
        .await
}

/// The harvest jobs `ListHarvestJobs` reports for one channel.
async fn jobs_of(emulator: &Emulator, channel: &str) -> usize {
    let listed = emulator
        .call(
            "GET",
            &format!("/channelGroup/{GROUP}/harvestJob?channelName={channel}"),
            None,
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "ListHarvestJobs for {channel} must answer 200, got {} with body {}",
        listed.status, listed.body
    );
    listed.json["Items"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

#[tokio::test]
async fn a_harvest_job_is_refused_on_a_channel_that_receives_no_ingest_of_its_own() {
    let emulator = seeded().await;
    let start = epoch_seconds_from_now(600.0);

    for row in rows() {
        let label = row.label;
        let created = emulator
            .create_channel(GROUP, row.channel, (row.body)())
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: creating {} must answer 200, got {} with body {}",
            row.channel, created.status, created.body
        );
        let endpoint = create_endpoint(&emulator, row.channel).await;

        let mut served = 0usize;
        for (variant, body) in request_variants(start, row.channel) {
            let answer = create_job(&emulator, row.channel, &endpoint, body).await;
            let case = format!("{label} / {variant}");
            if row.refused {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with body {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    error_type(&answer.json),
                    "ValidationException",
                    "case {case:?}: expected a ValidationException envelope, got {}",
                    answer.body
                );
                assert_eq!(
                    validation_type(&answer),
                    "MULTIVIEW_INPUT_TYPE_WITH_HARVEST_JOB",
                    "case {case:?}: expected the multiview harvest-job validation type, \
                     got {}",
                    answer.body
                );
                // The service states every refusal in one form: the member path the rule
                // is about, then the requirement it failed.
                let prefix = format!("{ENVELOPE}.channelName: ");
                assert!(
                    message(&answer).starts_with(&prefix),
                    "case {case:?}: expected a message starting {prefix:?}, got {:?}",
                    message(&answer)
                );
                assert!(
                    message(&answer).len() > prefix.len(),
                    "case {case:?}: the message must state the rule after the member \
                     path, got {:?}",
                    message(&answer)
                );
                // The text states the RULE, so it carries none of the caller's values.
                for sent in [GROUP, row.channel, endpoint.as_str()] {
                    assert!(
                        !message(&answer).contains(sent),
                        "case {case:?}: the message must not echo the request's {sent:?}, \
                         got {:?}",
                        message(&answer)
                    );
                }
            } else {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected HTTP 200, got {} with body {}",
                    answer.status, answer.body
                );
                served += 1;
                // What the create reported reads back through `GetHarvestJob`, so the
                // served rows are served out of persisted state.
                let name = answer.string("HarvestJobName").to_string();
                let read = emulator
                    .call(
                        "GET",
                        &format!(
                            "/channelGroup/{GROUP}/channel/{}/originEndpoint/{endpoint}\
                             /harvestJob/{name}",
                            row.channel
                        ),
                        None,
                    )
                    .await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: GetHarvestJob must find the job, got {} with body {}",
                    read.status, read.body
                );
                assert_eq!(
                    read.string("Arn"),
                    answer.string("Arn"),
                    "case {case:?}: expected the stored job the create reported {:?}, got {}",
                    answer.string("Arn"),
                    read.body
                );
            }
        }

        // A refused create stores nothing, and a served one stores exactly one job per
        // accepted request.
        let stored = jobs_of(&emulator, row.channel).await;
        assert_eq!(
            stored, served,
            "case {label:?}: expected {served} stored harvest job(s), got {stored}"
        );
    }
}

#[tokio::test]
async fn the_sources_of_a_refused_multiview_channel_are_still_harvested() {
    let emulator = seeded().await;
    let start = epoch_seconds_from_now(600.0);
    let multiview = "harvest-mv-over-sources";
    let created = emulator
        .create_channel(
            GROUP,
            multiview,
            json!({
                "InputType": "MULTIVIEW",
                "MultiviewConfiguration": {
                    "AvailableSources": SOURCES,
                    "AvailableLayouts": [LAYOUTS[3]],
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "setup: creating {multiview} must answer 200, got {} with body {}",
        created.status, created.body
    );
    let endpoint = create_endpoint(&emulator, multiview).await;

    let body = harvest_body(start, start + 600.0, &[PUBLISHED_HLS]);
    let answer = create_job(&emulator, multiview, &endpoint, body).await;
    assert_eq!(
        answer.status, 400,
        "case {multiview:?}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        jobs_of(&emulator, multiview).await,
        0,
        "case {multiview:?}: a refused create must store nothing"
    );

    // Each source ingests content of its own, so each is harvested — the refusal is about
    // the channel the request addresses, not about the group or the multiview
    // relationship.
    for source in SOURCES {
        let mut body = harvest_body(start, start + 600.0, &[PUBLISHED_HLS]);
        body["HarvestJobName"] = json!(format!("{source}-job"));
        let answer = create_job(&emulator, source, &format!("{source}-ep"), body).await;
        assert_eq!(
            answer.status, 200,
            "case {source:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            jobs_of(&emulator, source).await,
            1,
            "case {source:?}: the served create must store exactly one job"
        );
    }
}
