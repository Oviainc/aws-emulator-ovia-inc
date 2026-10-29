// SPDX-License-Identifier: CC0-1.0

//! The namespace a harvest job's NAME lives in, driven over HTTP through the same router
//! the emulator boots.
//!
//! A harvest job record hangs off the origin endpoint whose content it exports, but the
//! model gives its name the CHANNEL's namespace
//! (`smithy_query(query="doc", shape="CreateHarvestJobRequest", member="HarvestJobName")`:
//! "A name for the harvest job. This name must be unique within the channel"). The rules
//! this suite asserts, each over a SET of inputs:
//!
//! 1. A create naming a job that ANY origin endpoint of the same channel already holds is
//!    refused with 409 `ConflictException` / `RESOURCE_ALREADY_EXISTS` and stores nothing —
//!    whether the name is taken on the addressed endpoint or on a sibling of it.
//! 2. A name is free again one channel over: the same name is accepted in another channel
//!    of the same channel group, and in another channel group, and so is a name that
//!    differs from a taken one at all — in case, or by one character.
//! 3. The name the SERVICE chooses for a create that carried none is free within the whole
//!    channel too, so creates spread over the channel's endpoints never collide, and a
//!    caller naming a job after one of them is refused by rule 1.

use serde_json::{json, Value};

// The crate's shared test harness: the control-plane client and its error-envelope reader
// (`channel_lifecycle`) and the one `CreateHarvestJob` body plus its schedule clock
// (`rejection_rule_and_negative_paths`).
use crate::channel_lifecycle::{error_type, Emulator};
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};

/// The hierarchy every case hangs its jobs off. Every name matches the model's
/// `ResourceName` pattern `^[a-zA-Z0-9_-]+$`.
const GROUP: &str = "hjn-group";
const OTHER_GROUP: &str = "hjn-other-group";
const CHANNEL: &str = "hjn-channel";
const OTHER_CHANNEL: &str = "hjn-other-channel";

/// Three origin endpoints under EACH channel, so a row can take a name on one of them and
/// ask for it again on a sibling.
const ENDPOINTS: [&str; 3] = ["hjn-endpoint-a", "hjn-endpoint-b", "hjn-endpoint-c"];

/// The manifests every origin endpoint of the hierarchy publishes. A harvest job may
/// harvest only a manifest its endpoint publishes, so every manifest a row names is one of
/// these — a refusal below is about the job's NAME, never about an unknown manifest.
const PUBLISHED_MANIFESTS: [&str; 2] = ["index", "backup"];

/// The fixed sentence this service answers a taken name with
/// (`state::resource_already_exists`).
const RESOURCE_ALREADY_EXISTS: &str = "MediaPackage can't create the resource you requested \
     because a resource with that name already exists. Verify the name of the resource and \
     then try again";

/// The published maximum of `HarvestJobName`
/// (`smithy_query(query="operation", operation="CreateHarvestJob")`: `ResourceName`,
/// length 1..256).
const LONGEST_NAME_CHARS: usize = 256;

/// The `ConflictExceptionType` discriminator of an error envelope.
fn conflict_type(body: &Value) -> &str {
    body.get("ConflictExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The origin endpoint one create or read addresses: the whole path above the job.
#[derive(Debug, Clone, Copy)]
struct At {
    group: &'static str,
    channel: &'static str,
    endpoint: &'static str,
}

impl At {
    /// The `/harvestJob` collection of this origin endpoint.
    fn collection(&self) -> String {
        format!(
            "/channelGroup/{}/channel/{}/originEndpoint/{}/harvestJob",
            self.group, self.channel, self.endpoint
        )
    }
}

/// The endpoint at `index` of `GROUP`/`CHANNEL` — the one channel whose namespace rules 1
/// and 3 are about.
fn under_channel(index: usize) -> At {
    At {
        group: GROUP,
        channel: CHANNEL,
        endpoint: ENDPOINTS[index],
    }
}

/// `CreateHarvestJob` against one origin endpoint: `name` when the row names its job,
/// harvesting `manifests` over a window of `window` seconds starting at `start`.
async fn create_job(
    emulator: &Emulator,
    at: At,
    name: Option<&str>,
    manifests: &[&str],
    start: f64,
    window: f64,
) -> (u16, Value) {
    let mut body = harvest_body(start, start + window, manifests);
    if let Some(name) = name {
        body["HarvestJobName"] = json!(name);
    }
    emulator.call("POST", &at.collection(), Some(body)).await
}

/// `GetHarvestJob` on one job of one origin endpoint.
async fn read_job(emulator: &Emulator, at: At, name: &str) -> (u16, Value) {
    emulator
        .call("GET", &format!("{}/{name}", at.collection()), None)
        .await
}

/// Every harvest job one channel group holds, through `ListHarvestJobs` — the reader that
/// sees a duplicate name a create slipped into the channel under another endpoint.
async fn stored_jobs(emulator: &Emulator, case: &str, group: &str) -> Vec<Value> {
    let (status, body) = emulator
        .call(
            "GET",
            &format!("/channelGroup/{group}/harvestJob?maxResults=100"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListHarvestJobs expected 200, got {status} {body}"
    );
    body["Items"].as_array().cloned().unwrap_or_default()
}

/// How many jobs of `channel` the group reports under `name`.
fn named(jobs: &[Value], channel: &str, name: &str) -> usize {
    jobs.iter()
        .filter(|job| {
            job["ChannelName"].as_str() == Some(channel)
                && job["HarvestJobName"].as_str() == Some(name)
        })
        .count()
}

/// A fresh emulator holding two channel groups, each with the channels and the three
/// origin endpoints the rows address, and no harvest job yet — so every job a case counts
/// is one the case itself created.
async fn emulator_with_hierarchy(case: &str) -> Emulator {
    let emulator = Emulator::new();
    let manifests: Vec<Value> = PUBLISHED_MANIFESTS
        .iter()
        .map(|manifest| json!({"ManifestName": manifest}))
        .collect();
    for group in [GROUP, OTHER_GROUP] {
        let (status, body) = emulator.create_group(group, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateChannelGroup {group} expected 200, got {status} {body}"
        );
        for channel in [CHANNEL, OTHER_CHANNEL] {
            let (status, body) = emulator.create_channel(group, channel, json!({})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateChannel {group}/{channel} expected 200, got {status} {body}"
            );
            for endpoint in ENDPOINTS {
                let (status, body) = emulator
                    .create_endpoint(
                        group,
                        channel,
                        endpoint,
                        json!({"ContainerType": "TS", "HlsManifests": manifests}),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: CreateOriginEndpoint {group}/{channel}/{endpoint} expected \
                     200, got {status} {body}"
                );
            }
        }
    }
    emulator
}

/// How a row spells the name both of its creates carry.
#[derive(Debug, Clone, Copy)]
enum NameForm {
    /// The name exactly as written.
    Literal(&'static str),
    /// A name at the published `ResourceName` maximum (see [`LONGEST_NAME_CHARS`]).
    Longest,
    /// A name of one character — the published minimum.
    Shortest,
}

impl NameForm {
    fn spell(self) -> String {
        match self {
            NameForm::Literal(name) => name.to_string(),
            NameForm::Longest => "j".repeat(LONGEST_NAME_CHARS),
            NameForm::Shortest => "j".to_string(),
        }
    }
}

/// One row of the collision table: a job created on the endpoint at `first`, then the same
/// name asked for on the endpoint at `second`, with a request that may differ from the
/// first in every other member.
#[derive(Debug, Clone, Copy)]
struct Collision {
    label: &'static str,
    /// Index into [`ENDPOINTS`] of the endpoint that takes the name.
    first: usize,
    /// Index into [`ENDPOINTS`] of the endpoint the second create addresses — a sibling
    /// under the same channel, or the first endpoint itself.
    second: usize,
    name: NameForm,
    /// What the second create would harvest, and how long its window runs: a row changes
    /// them to show the refusal is about the name alone.
    manifests: &'static [&'static str],
    window: f64,
}

/// The rows rule 1 is asserted over: which endpoint of the channel holds the name, which
/// one asks for it, how the name is spelled, and how different the second request is.
const COLLISIONS: &[Collision] = &[
    Collision {
        label: "the same endpoint, the same body",
        first: 0,
        second: 0,
        name: NameForm::Literal("job-a"),
        manifests: &["index"],
        window: 600.0,
    },
    Collision {
        label: "a sibling endpoint of the same channel",
        first: 0,
        second: 1,
        name: NameForm::Literal("job-a"),
        manifests: &["index"],
        window: 600.0,
    },
    Collision {
        label: "a sibling endpoint, harvesting another manifest over a longer window",
        first: 1,
        second: 2,
        name: NameForm::Literal("job_b-2"),
        manifests: &PUBLISHED_MANIFESTS,
        window: 7200.0,
    },
    Collision {
        label: "the name taken on the LAST endpoint of the channel, asked for on the first",
        first: 2,
        second: 0,
        name: NameForm::Literal("Job-C"),
        manifests: &["backup"],
        window: 1.0,
    },
    Collision {
        label: "a sibling endpoint, a name at the published maximum length",
        first: 0,
        second: 2,
        name: NameForm::Longest,
        manifests: &["index"],
        window: 60.0,
    },
    Collision {
        label: "a sibling endpoint, a name of one character",
        first: 1,
        second: 0,
        name: NameForm::Shortest,
        manifests: &["backup"],
        window: 3600.0,
    },
];

/// RULE 1 — a name any endpoint of the channel already holds is the duplicate-create
/// refusal, and the refused create stores nothing.
#[tokio::test]
async fn a_harvest_job_name_taken_anywhere_in_the_channel_is_refused() {
    for row in COLLISIONS {
        let label = row.label;
        let emulator = emulator_with_hierarchy(label).await;
        let start = epoch_seconds_from_now(60.0);
        let name = row.name.spell();
        let (first_at, second_at) = (under_channel(row.first), under_channel(row.second));

        let (status, first) =
            create_job(&emulator, first_at, Some(&name), &["index"], start, 600.0).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first create expected 200, got {status} {first}"
        );

        let (status, refused) = create_job(
            &emulator,
            second_at,
            Some(&name),
            row.manifests,
            start,
            row.window,
        )
        .await;
        assert_eq!(
            status, 409,
            "case {label:?}: a name already taken in the channel expected 409, got {status} \
             {refused}"
        );
        assert_eq!(
            error_type(&refused),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {refused}"
        );
        assert_eq!(
            conflict_type(&refused),
            "RESOURCE_ALREADY_EXISTS",
            "case {label:?}: expected RESOURCE_ALREADY_EXISTS, got {refused}"
        );
        assert_eq!(
            refused["Message"].as_str().unwrap_or_default(),
            RESOURCE_ALREADY_EXISTS,
            "case {label:?}: expected the service's fixed sentence, got {refused}"
        );

        // Nothing was stored: the channel still holds exactly one job of that name, it is
        // still the one the first create made, and the endpoint that asked for the name
        // holds no job under it.
        let jobs = stored_jobs(&emulator, label, GROUP).await;
        assert_eq!(
            named(&jobs, CHANNEL, &name),
            1,
            "case {label:?}: the channel must hold ONE job of that name, got {jobs:?}"
        );
        let (status, read) = read_job(&emulator, first_at, &name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first job must still be readable, got {status} {read}"
        );
        for member in [
            "Arn",
            "CreatedAt",
            "HarvestedManifests",
            "ScheduleConfiguration",
        ] {
            assert_eq!(
                read[member], first[member],
                "case {label:?}: a refused create must leave {member} alone, got {} where {} was \
                 stored",
                read[member], first[member]
            );
        }
        if row.second != row.first {
            let (status, absent) = read_job(&emulator, second_at, &name).await;
            assert_eq!(
                status, 404,
                "case {label:?}: the endpoint that was refused must hold no such job, got \
                 {status} {absent}"
            );
        }
    }
}

/// The name the first create of rule 2 takes, under `GROUP`/`CHANNEL`/`ENDPOINTS[0]`.
const TAKEN: &str = "shared-name";

/// One row of the free-name table: where the second create goes, and the name it carries.
#[derive(Debug, Clone, Copy)]
struct FreeName {
    label: &'static str,
    at: At,
    /// `None` reuses [`TAKEN`] verbatim; a name here is one that differs from it.
    name: Option<&'static str>,
}

/// The rows rule 2 is asserted over: one channel over, one channel GROUP over, and — on a
/// sibling endpoint of the very channel that holds the name — a name differing from it
/// only in case, and one differing by a single character.
const FREE_NAMES: &[FreeName] = &[
    FreeName {
        label: "the same name in another channel of the same group",
        at: At {
            group: GROUP,
            channel: OTHER_CHANNEL,
            endpoint: ENDPOINTS[0],
        },
        name: None,
    },
    FreeName {
        label: "the same name on another endpoint of another channel",
        at: At {
            group: GROUP,
            channel: OTHER_CHANNEL,
            endpoint: ENDPOINTS[2],
        },
        name: None,
    },
    FreeName {
        label: "the same name in another channel group",
        at: At {
            group: OTHER_GROUP,
            channel: CHANNEL,
            endpoint: ENDPOINTS[0],
        },
        name: None,
    },
    FreeName {
        label: "a sibling endpoint, a name differing only in case",
        at: At {
            group: GROUP,
            channel: CHANNEL,
            endpoint: ENDPOINTS[1],
        },
        name: Some("Shared-Name"),
    },
    FreeName {
        label: "a sibling endpoint, a name differing by one character",
        at: At {
            group: GROUP,
            channel: CHANNEL,
            endpoint: ENDPOINTS[1],
        },
        name: Some("shared-name2"),
    },
];

/// RULE 2 — the channel is the whole scope: one channel over, or under a name that differs
/// at all, the create is accepted and the job reads back.
#[tokio::test]
async fn a_harvest_job_name_is_free_outside_the_channel_that_holds_it() {
    for row in FREE_NAMES {
        let label = row.label;
        let emulator = emulator_with_hierarchy(label).await;
        let start = epoch_seconds_from_now(60.0);
        let first_at = under_channel(0);

        let (status, first) =
            create_job(&emulator, first_at, Some(TAKEN), &["index"], start, 600.0).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first create expected 200, got {status} {first}"
        );

        let name = row.name.unwrap_or(TAKEN);
        let (status, accepted) =
            create_job(&emulator, row.at, Some(name), &["backup"], start, 1800.0).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a name no job of that channel holds expected 200, got {status} \
             {accepted}"
        );
        assert_eq!(
            accepted["HarvestJobName"].as_str(),
            Some(name),
            "case {label:?}: the create must keep the name it was given, got {accepted}"
        );
        assert_ne!(
            accepted["Arn"], first["Arn"],
            "case {label:?}: the two jobs are different resources, so their ARNs must differ, \
             got {} twice",
            accepted["Arn"]
        );

        // The round trip: both jobs read back at their own paths.
        for (owner, at, job) in [
            ("the first job", first_at, TAKEN),
            ("the accepted job", row.at, name),
        ] {
            let (status, read) = read_job(&emulator, at, job).await;
            assert_eq!(
                status, 200,
                "case {label:?}: GetHarvestJob on {owner} expected 200, got {status} {read}"
            );
            assert_eq!(
                read["HarvestJobName"].as_str(),
                Some(job),
                "case {label:?}: {owner} must report its own name, got {read}"
            );
        }

        // Both jobs are stored, each once under its own channel.
        let jobs = stored_jobs(&emulator, label, row.at.group).await;
        assert_eq!(
            named(&jobs, row.at.channel, name),
            1,
            "case {label:?}: the accepted job must be stored once, got {jobs:?}"
        );
        let jobs = stored_jobs(&emulator, label, GROUP).await;
        assert_eq!(
            named(&jobs, CHANNEL, TAKEN),
            1,
            "case {label:?}: the first job must still be stored once, got {jobs:?}"
        );
    }
}

/// RULE 3 — the name the service chooses is free within the whole channel.
///
/// Creates carrying no `HarvestJobName` are spread over every endpoint of one channel, so
/// a name derived per endpoint would be free to repeat across them; every reported name
/// must still be distinct, addressable, and taken for the whole channel afterwards.
#[tokio::test]
async fn a_service_chosen_harvest_job_name_is_unique_within_the_channel() {
    let label = "service-chosen names across the endpoints of one channel";
    let emulator = emulator_with_hierarchy(label).await;
    let start = epoch_seconds_from_now(60.0);

    // Two per endpoint, so the rule is asserted both across endpoints and within one.
    let mut minted: Vec<(usize, String)> = Vec::new();
    for round in 0..2 {
        for index in 0..ENDPOINTS.len() {
            let at = under_channel(index);
            let endpoint = at.endpoint;
            let (status, body) = create_job(&emulator, at, None, &["index"], start, 600.0).await;
            assert_eq!(
                status, 200,
                "case {label:?} ({endpoint}, round {round}): expected 200, got {status} {body}"
            );
            let name = body["HarvestJobName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {label:?}: a create must report HarvestJobName: {body}")
                })
                .to_string();
            // The name has to be addressable, so it is a `ResourceName`: 1..256 characters
            // of the model's `^[a-zA-Z0-9_-]+$`.
            assert!(
                (1..=LONGEST_NAME_CHARS).contains(&name.chars().count())
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "case {label:?} ({endpoint}, round {round}): a service-chosen name must be an \
                 addressable ResourceName, got {name:?}"
            );
            assert!(
                !minted.iter().any(|(_, taken)| *taken == name),
                "case {label:?} ({endpoint}, round {round}): the channel already holds a job \
                 named {name:?}, minted as {minted:?}"
            );
            minted.push((index, name));
        }
    }

    // Every minted job is readable at the endpoint it was created against.
    for (index, name) in &minted {
        let (status, read) = read_job(&emulator, under_channel(*index), name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetHarvestJob on {name:?} expected 200, got {status} {read}"
        );
    }
    let jobs = stored_jobs(&emulator, label, GROUP).await;
    assert_eq!(
        jobs.len(),
        minted.len(),
        "case {label:?}: every create must have left exactly one job, got {jobs:?}"
    );

    // A service-chosen name is taken for the whole channel: a caller asking for it on a
    // sibling endpoint is refused by rule 1.
    let (first_index, taken) = minted[0].clone();
    let sibling = under_channel((first_index + 1) % ENDPOINTS.len());
    let (status, refused) =
        create_job(&emulator, sibling, Some(&taken), &["index"], start, 600.0).await;
    assert_eq!(
        status, 409,
        "case {label:?}: a service-chosen name asked for on a sibling expected 409, got {status} \
         {refused}"
    );
    assert_eq!(
        conflict_type(&refused),
        "RESOURCE_ALREADY_EXISTS",
        "case {label:?}: expected RESOURCE_ALREADY_EXISTS, got {refused}"
    );

    // The same name is still free one channel over.
    let next_channel = At {
        group: GROUP,
        channel: OTHER_CHANNEL,
        endpoint: ENDPOINTS[0],
    };
    let (status, accepted) =
        create_job(&emulator, next_channel, Some(&taken), &["index"], start, 600.0).await;
    assert_eq!(
        status, 200,
        "case {label:?}: a service-chosen name of another channel expected 200, got {status} \
         {accepted}"
    );
}
