// SPDX-License-Identifier: CC0-1.0

//! The namespace a harvest job create's `ClientToken` is resolved in, driven over HTTP
//! through the same router the emulator boots.
//!
//! `CreateHarvestJob` is `@idempotent` and binds `ClientToken` with `@idempotencyToken` on
//! the `x-amzn-client-token` header
//! (`smithy_query(query="operation", operation="CreateHarvestJob")`), so the token is the
//! identity of the ATTEMPT. A harvest job's own identity lives in the CHANNEL
//! (`smithy_query(query="doc", shape="CreateHarvestJobRequest", member="HarvestJobName")`:
//! "A name for the harvest job. This name must be unique within the channel"), and the
//! token is resolved over exactly that namespace — the same collection the job's name is
//! judged against. The rules this suite asserts, each over a SET of inputs:
//!
//! 1. A token already spent on a job under ANY origin endpoint of the channel is never
//!    read as fresh: a repeat under it that addresses a sibling endpoint — a changed
//!    request, because the endpoint is a member of it — is refused with 409
//!    `ConflictException` / `IDEMPOTENT_PARAMETER_MISMATCH` and stores nothing.
//! 2. Widening the scope to the channel does not cost the replay: an unchanged repeat
//!    under the token, addressing the endpoint the first attempt did, still replays that
//!    job verbatim while sibling endpoints of the same channel hold jobs of their own.
//! 3. The scope is the channel and no wider: the same token in another channel of the
//!    group, or in another channel group, is a first attempt and mints its own job.

use serde_json::{json, Value};

// The crate's shared test harness: the control-plane client and its error-envelope reader
// (`channel_lifecycle`) and the one `CreateHarvestJob` body plus its schedule clock
// (`rejection_rule_and_negative_paths`).
use crate::channel_lifecycle::{error_type, Emulator};
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};

/// The hierarchy every case hangs its jobs off. Every name matches the model's
/// `ResourceName` pattern `^[a-zA-Z0-9_-]+$`.
const GROUP: &str = "hjt-group";
const OTHER_GROUP: &str = "hjt-other-group";
const CHANNEL: &str = "hjt-channel";
const OTHER_CHANNEL: &str = "hjt-other-channel";

/// Three origin endpoints under EACH channel, so a row can spend a token on one of them
/// and reuse it against a sibling.
const ENDPOINTS: [&str; 3] = ["hjt-endpoint-a", "hjt-endpoint-b", "hjt-endpoint-c"];

/// The manifests every origin endpoint of the hierarchy publishes. A harvest job may
/// harvest only a manifest its endpoint publishes, so every manifest a row names is one of
/// these — a refusal below is about the token it reused, never about an unknown manifest.
const PUBLISHED_MANIFESTS: [&str; 2] = ["index", "backup"];

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

/// The endpoint at `index` of `GROUP`/`CHANNEL` — the one channel whose token namespace
/// rules 1 and 2 are about.
fn under_channel(index: usize) -> At {
    At {
        group: GROUP,
        channel: CHANNEL,
        endpoint: ENDPOINTS[index],
    }
}

/// What one create carries besides the token: the members the token does NOT identify.
#[derive(Debug, Clone, Copy)]
struct Body {
    /// `None` for a job the SERVICE names — the hardest repeat, since the first attempt's
    /// name was never in the request at all.
    name: Option<&'static str>,
    description: Option<&'static str>,
    manifests: &'static [&'static str],
    /// How long the scheduled window runs, in seconds.
    window: f64,
}

/// The request `body` asks for, scheduled from `start` (epoch seconds). Deterministic in
/// its arguments, so a repeat built from the same `body` and `start` is byte-identical.
fn request(body: &Body, start: f64) -> Value {
    let mut request = harvest_body(start, start + body.window, body.manifests);
    if let Some(name) = body.name {
        request["HarvestJobName"] = json!(name);
    }
    if let Some(description) = body.description {
        request["Description"] = json!(description);
    }
    request
}

/// `CreateHarvestJob` against one origin endpoint, carrying `token` on the member's own
/// `x-amzn-client-token` header when there is one.
async fn create(
    emulator: &Emulator,
    at: At,
    body: Value,
    token: Option<&str>,
) -> (u16, Value) {
    let headers: Vec<(&str, &str)> = token
        .map(|token| vec![("x-amzn-client-token", token)])
        .unwrap_or_default();
    emulator
        .call_with_headers("POST", &at.collection(), Some(body), &headers)
        .await
}

/// `GetHarvestJob` on one job of one origin endpoint.
async fn read_job(emulator: &Emulator, at: At, name: &str) -> (u16, Value) {
    emulator
        .call("GET", &format!("{}/{name}", at.collection()), None)
        .await
}

/// Every harvest job one channel group holds, through `ListHarvestJobs` — the reader that
/// sees a second job a reused token minted behind the caller's back.
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

/// How many jobs of `channel` the group reports.
fn in_channel(jobs: &[Value], channel: &str) -> usize {
    jobs.iter()
        .filter(|job| job["ChannelName"].as_str() == Some(channel))
        .count()
}

/// The job name a create reported.
fn job_name(body: &Value, case: &str) -> String {
    body["HarvestJobName"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a create must report HarvestJobName: {body}"))
        .to_string()
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

/// One row of the sibling-reuse table: a job created on the endpoint at `first` under a
/// token, then that token reused against the endpoint at `second` with `repeat`.
#[derive(Debug, Clone, Copy)]
struct Reuse {
    label: &'static str,
    /// Index into [`ENDPOINTS`] of the endpoint the token is spent on.
    first: usize,
    /// Index into [`ENDPOINTS`] of the SIBLING endpoint the repeat addresses.
    second: usize,
    /// What the first attempt carried.
    first_body: Body,
    /// What the repeat carries — identical to `first_body`, or differing in further
    /// members on top of the endpoint it addresses.
    repeat: Body,
}

/// The base request most rows repeat: a caller-named job with a description.
const NAMED: Body = Body {
    name: Some("hjt-job"),
    description: Some("the first attempt"),
    manifests: &["index"],
    window: 3600.0,
};

/// A job the service names: the repeat carries no name either, so only the endpoint it
/// addresses tells the two requests apart.
const UNNAMED: Body = Body {
    name: None,
    description: None,
    manifests: &["index"],
    window: 600.0,
};

/// The rows rule 1 is asserted over: which endpoint of the channel spends the token, which
/// sibling reuses it, and how much else the repeat changes.
const REUSES: &[Reuse] = &[
    Reuse {
        label: "a sibling endpoint, every other member identical",
        first: 0,
        second: 1,
        first_body: NAMED,
        repeat: NAMED,
    },
    Reuse {
        label: "the LAST endpoint of the channel spends the token, the first reuses it",
        first: 2,
        second: 0,
        first_body: NAMED,
        repeat: NAMED,
    },
    Reuse {
        label: "a sibling endpoint, a service-named job on both sides",
        first: 1,
        second: 2,
        first_body: UNNAMED,
        repeat: UNNAMED,
    },
    Reuse {
        label: "a sibling endpoint, harvesting every published manifest over a longer window",
        first: 0,
        second: 2,
        first_body: NAMED,
        repeat: Body {
            manifests: &PUBLISHED_MANIFESTS,
            window: 7200.0,
            ..NAMED
        },
    },
    Reuse {
        label: "a sibling endpoint, the description taken away",
        first: 1,
        second: 0,
        first_body: NAMED,
        repeat: Body {
            description: None,
            ..NAMED
        },
    },
    Reuse {
        label: "a sibling endpoint, the first attempt service-named and the repeat named",
        first: 2,
        second: 1,
        first_body: UNNAMED,
        repeat: Body {
            name: Some("hjt-other-job"),
            ..UNNAMED
        },
    },
];

/// RULE 1 — a token spent anywhere in the channel is never fresh: a repeat under it that
/// addresses a sibling endpoint is the modelled parameter mismatch, and stores nothing.
#[tokio::test]
async fn a_token_spent_under_a_sibling_endpoint_of_the_channel_is_the_modelled_mismatch() {
    for row in REUSES {
        let label = row.label;
        let emulator = emulator_with_hierarchy(label).await;
        let start = epoch_seconds_from_now(60.0);
        let token = "hjt-shared-token";
        let (first_at, second_at) = (under_channel(row.first), under_channel(row.second));

        let (status, first) =
            create(&emulator, first_at, request(&row.first_body, start), Some(token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first create expected 200, got {status} {first}"
        );
        let first_name = job_name(&first, label);

        let (status, refused) =
            create(&emulator, second_at, request(&row.repeat, start), Some(token)).await;
        assert_eq!(
            status, 409,
            "case {label:?}: a repeat under a token already spent in the channel expected 409, \
             got {status} {refused}"
        );
        assert_eq!(
            error_type(&refused),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {refused}"
        );
        assert_eq!(
            conflict_type(&refused),
            "IDEMPOTENT_PARAMETER_MISMATCH",
            "case {label:?}: expected IDEMPOTENT_PARAMETER_MISMATCH, got {refused}"
        );

        // The refused create stored nothing: the channel still holds the one job the token
        // minted, the sibling that reused it holds none, and the first job is untouched.
        let jobs = stored_jobs(&emulator, label, GROUP).await;
        assert_eq!(
            in_channel(&jobs, CHANNEL),
            1,
            "case {label:?}: a token may mint ONE job in the channel, got {jobs:?}"
        );
        let (status, read) = read_job(&emulator, first_at, &first_name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first job must still be readable, got {status} {read}"
        );
        for member in [
            "Arn",
            "OriginEndpointName",
            "CreatedAt",
            "Description",
            "ScheduleConfiguration",
            "HarvestedManifests",
            "Destination",
        ] {
            assert_eq!(
                read[member], first[member],
                "case {label:?}: a refused repeat must leave {member} alone, got {} where {} was \
                 stored",
                read[member], first[member]
            );
        }
        let (status, absent) = read_job(&emulator, second_at, &first_name).await;
        assert_eq!(
            status, 404,
            "case {label:?}: the sibling endpoint must hold no job of that name, got {status} \
             {absent}"
        );

        // The SAME repeat is a perfectly good create on its own — sent into a store that
        // holds no job, under the same token, it is accepted. That is what makes the
        // refusal above about the token it reused rather than about what it asked for.
        let fresh = emulator_with_hierarchy(label).await;
        let (status, accepted) =
            create(&fresh, second_at, request(&row.repeat, start), Some(token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the repeat as a FIRST create expected 200, got {status} {accepted}"
        );
    }
}

/// RULE 2 — the replay survives the wider scope: an unchanged repeat against the endpoint
/// the first attempt addressed still replays its job, with sibling endpoints holding jobs.
#[tokio::test]
async fn an_unchanged_repeat_still_replays_while_siblings_hold_their_own_jobs() {
    for (index, body) in [NAMED, UNNAMED].iter().enumerate() {
        let label = if body.name.is_some() {
            "a caller-named job, siblings busy"
        } else {
            "a service-named job, siblings busy"
        };
        let emulator = emulator_with_hierarchy(label).await;
        let start = epoch_seconds_from_now(60.0);
        let token = format!("hjt-replay-token-{index}");
        let at = under_channel(0);

        // Every sibling endpoint of the channel already holds a job of its own, each under
        // its own token: the candidate set the token is searched in is not empty, and none
        // of its entries carries this request's token.
        for sibling in 1..ENDPOINTS.len() {
            let (status, body) = create(
                &emulator,
                under_channel(sibling),
                request(
                    &Body {
                        name: None,
                        ..UNNAMED
                    },
                    start,
                ),
                Some(&format!("hjt-sibling-token-{sibling}")),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: a sibling's own create expected 200, got {status} {body}"
            );
        }

        let (status, first) = create(&emulator, at, request(body, start), Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first create expected 200, got {status} {first}"
        );

        let (status, replay) = create(&emulator, at, request(body, start), Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: an identical repeat expected a 200 replay, got {status} {replay}"
        );
        assert_eq!(
            replay, first,
            "case {label:?}: an identical repeat must replay the first response verbatim, got \
             {replay} where {first} was answered"
        );

        // The replay created nothing: the channel holds one job per sibling plus this one.
        let jobs = stored_jobs(&emulator, label, GROUP).await;
        assert_eq!(
            in_channel(&jobs, CHANNEL),
            ENDPOINTS.len(),
            "case {label:?}: the repeat must have minted no second job, got {jobs:?}"
        );

        // The round trip: the replayed job is the job the reader finds.
        let name = job_name(&first, label);
        let (status, read) = read_job(&emulator, at, &name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetHarvestJob on the replayed job expected 200, got {status} {read}"
        );
        for member in [
            "Arn",
            "OriginEndpointName",
            "HarvestJobName",
            "CreatedAt",
            "ScheduleConfiguration",
            "HarvestedManifests",
            "Destination",
        ] {
            assert_eq!(
                read[member], replay[member],
                "case {label:?}: the replay and the read must agree on {member}, got {} where {} \
                 was replayed",
                read[member], replay[member]
            );
        }
    }
}

/// One row of the other-namespace table: where the second create under the same token
/// goes.
#[derive(Debug, Clone, Copy)]
struct Elsewhere {
    label: &'static str,
    at: At,
}

/// The rows rule 3 is asserted over: one channel over, one channel over on another
/// endpoint, and one channel GROUP over.
const ELSEWHERE: &[Elsewhere] = &[
    Elsewhere {
        label: "another channel of the same group, the same endpoint name",
        at: At {
            group: GROUP,
            channel: OTHER_CHANNEL,
            endpoint: ENDPOINTS[0],
        },
    },
    Elsewhere {
        label: "another channel of the same group, another endpoint",
        at: At {
            group: GROUP,
            channel: OTHER_CHANNEL,
            endpoint: ENDPOINTS[2],
        },
    },
    Elsewhere {
        label: "another channel group",
        at: At {
            group: OTHER_GROUP,
            channel: CHANNEL,
            endpoint: ENDPOINTS[0],
        },
    },
];

/// RULE 3 — the scope is the channel and no wider: the same token one channel or one
/// channel group over is a first attempt.
#[tokio::test]
async fn the_same_token_is_fresh_outside_the_channel_that_spent_it() {
    for row in ELSEWHERE {
        let label = row.label;
        let emulator = emulator_with_hierarchy(label).await;
        let start = epoch_seconds_from_now(60.0);
        let token = "hjt-cross-namespace-token";
        let first_at = under_channel(0);

        let (status, first) = create(&emulator, first_at, request(&NAMED, start), Some(token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first create expected 200, got {status} {first}"
        );

        let (status, accepted) = create(&emulator, row.at, request(&NAMED, start), Some(token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a token no job of THAT channel carries expected 200, got {status} \
             {accepted}"
        );
        assert_ne!(
            accepted["Arn"], first["Arn"],
            "case {label:?}: the two jobs are different resources, so their ARNs must differ, got \
             {} twice",
            accepted["Arn"]
        );
        assert_eq!(
            accepted["ChannelName"].as_str(),
            Some(row.at.channel),
            "case {label:?}: the accepted job must belong to the channel it was created in, got \
             {accepted}"
        );

        // Both jobs are stored, each once in its own channel, and each reads back.
        for (owner, at, body) in [
            ("the first job", first_at, &first),
            ("the accepted job", row.at, &accepted),
        ] {
            let name = job_name(body, label);
            let (status, read) = read_job(&emulator, at, &name).await;
            assert_eq!(
                status, 200,
                "case {label:?}: GetHarvestJob on {owner} expected 200, got {status} {read}"
            );
            assert_eq!(
                read["Arn"], body["Arn"],
                "case {label:?}: {owner} must read back as it was created, got {} where {} was \
                 answered",
                read["Arn"], body["Arn"]
            );
        }
        let jobs = stored_jobs(&emulator, label, row.at.group).await;
        assert_eq!(
            in_channel(&jobs, row.at.channel),
            1,
            "case {label:?}: the accepted job must be stored once, got {jobs:?}"
        );
        let jobs = stored_jobs(&emulator, label, GROUP).await;
        assert_eq!(
            in_channel(&jobs, CHANNEL),
            1,
            "case {label:?}: the first job must still be stored once, got {jobs:?}"
        );
    }
}
