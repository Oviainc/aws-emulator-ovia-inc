// SPDX-License-Identifier: CC0-1.0

//! The idempotency rule at the DEEPEST create of the MediaPackageV2 hierarchy — the
//! harvest job — driven over HTTP through the same router the emulator boots.
//!
//! `CreateHarvestJob` is `@idempotent` and binds `ClientToken` with `@idempotencyToken`
//! on the `x-amzn-client-token` header
//! (`smithy_query(query="operation", operation="CreateHarvestJob")`), so the token is the
//! IDENTITY of the attempt rather than a member of it:
//!
//! 1. A repeat carrying the same token and the same other members is the SAME request: it
//!    replays the job that token already created, byte for byte, and creates nothing — so
//!    the endpoint still holds exactly one job and the caller can still read it back.
//! 2. A repeat carrying the same token that differs in ANY other member is refused with
//!    409 `ConflictException` / `ConflictExceptionType` `IDEMPOTENT_PARAMETER_MISMATCH`,
//!    and stores nothing.
//! 3. Without that token every request is its own create, and a name already taken stays
//!    the `RESOURCE_ALREADY_EXISTS` conflict: idempotency is about the token, never about
//!    the name.
//!
//! The same three rules at the channel-group, channel and origin-endpoint levels are
//! covered by `control_plane_validation`; this suite is the harvest-job row of each.

use std::collections::BTreeMap;

use serde_json::{json, Value};

// The crate's shared test harness: the control-plane client and its error-envelope reader
// (`channel_lifecycle`) and the one `CreateHarvestJob` body plus its schedule clock
// (`rejection_rule_and_negative_paths`).
use crate::channel_lifecycle::{error_type, Emulator};
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};

/// The hierarchy every case hangs its jobs off, all matching the model's `ResourceName`
/// pattern `^[a-zA-Z0-9_-]+$`.
const GROUP: &str = "hji-group";
const CHANNEL: &str = "hji-channel";
const ENDPOINT: &str = "hji-endpoint";

/// The manifests the suite's origin endpoint publishes. A harvest job may harvest only a
/// manifest its endpoint publishes, so every manifest a row names is one of these — a
/// changed repeat must be refused for the token it reused, not for an unknown manifest.
const PUBLISHED_MANIFESTS: [&str; 2] = ["index", "backup"];

/// A pair list, as the tables below spell a tag map.
type Pairs = &'static [(&'static str, &'static str)];

/// The `ConflictExceptionType` discriminator of an error envelope.
fn conflict_type(body: &Value) -> &str {
    body.get("ConflictExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The collection `CreateHarvestJob` posts to, and the prefix `GetHarvestJob` reads from.
fn collection() -> String {
    format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/harvestJob")
}

/// A fresh emulator holding the channel group, channel and origin endpoint a harvest job
/// needs, and no harvest job yet — so a job count asserted below counts only what the
/// case itself created.
async fn emulator_with_endpoint(case: &str) -> Emulator {
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateChannelGroup expected 200, got {status} {body}"
    );
    let (status, body) = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateChannel expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .create_endpoint(
            GROUP,
            CHANNEL,
            ENDPOINT,
            json!({
                "ContainerType": "TS",
                "HlsManifests": PUBLISHED_MANIFESTS
                    .iter()
                    .map(|manifest| json!({"ManifestName": manifest}))
                    .collect::<Vec<_>>(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateOriginEndpoint expected 200, got {status} {body}"
    );
    emulator
}

/// `CreateHarvestJob` with `body`, carrying `token` on the member's own
/// `x-amzn-client-token` header when there is one.
async fn create(emulator: &Emulator, body: Value, token: Option<&str>) -> (u16, Value) {
    let headers: Vec<(&str, &str)> = token
        .map(|token| vec![("x-amzn-client-token", token)])
        .unwrap_or_default();
    emulator
        .call_with_headers("POST", &collection(), Some(body), &headers)
        .await
}

/// `GetHarvestJob` on one job of the suite's endpoint.
async fn read_job(emulator: &Emulator, name: &str) -> (u16, Value) {
    emulator
        .call("GET", &format!("{}/{name}", collection()), None)
        .await
}

/// Every harvest job the group holds, through `ListHarvestJobs` — the reader that sees a
/// duplicate a retry minted behind the caller's back.
async fn stored_jobs(emulator: &Emulator, case: &str) -> Vec<Value> {
    let (status, body) = emulator
        .call(
            "GET",
            &format!("/channelGroup/{GROUP}/harvestJob?maxResults=100"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListHarvestJobs expected 200, got {status} {body}"
    );
    body["Items"].as_array().cloned().unwrap_or_default()
}

/// The whole face of a `CreateHarvestJob` request except the `ClientToken` the rule is
/// about: the members a row of [`SHAPES`] varies.
#[derive(Debug, Clone, Copy)]
struct Shape {
    label: &'static str,
    /// `None` for a job the SERVICE names: the model leaves `HarvestJobName` optional, and
    /// a retry that omitted it is the case a server-derived name makes hardest.
    name: Option<&'static str>,
    description: Option<&'static str>,
    manifests: &'static [&'static str],
    /// How long the scheduled window runs, in seconds.
    window: f64,
    destination_path: &'static str,
    tags: Pairs,
}

/// The request `shape` asks for, scheduled from `start` (epoch seconds). Deterministic in
/// its arguments, so a retry built from the same `shape` and `start` is byte-identical.
fn request(shape: &Shape, start: f64) -> Value {
    let mut body = harvest_body(start, start + shape.window, shape.manifests);
    body["Destination"]["S3Destination"]["DestinationPath"] = json!(shape.destination_path);
    if let Some(name) = shape.name {
        body["HarvestJobName"] = json!(name);
    }
    if let Some(description) = shape.description {
        body["Description"] = json!(description);
    }
    if !shape.tags.is_empty() {
        body["Tags"] = json!(shape
            .tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<BTreeMap<String, String>>());
    }
    body
}

/// The requests rule 1 is asserted over. The rows differ in who chose the name, whether a
/// description is carried, how many of the endpoint's manifests are harvested, how long
/// the scheduled window is, where in the bucket the output goes, and how many tags the job
/// carries — everything a create of this resource varies in.
const SHAPES: &[Shape] = &[
    Shape {
        label: "a service-named job, nothing optional carried",
        name: None,
        description: None,
        manifests: &["index"],
        window: 3600.0,
        destination_path: "out/",
        tags: &[],
    },
    Shape {
        label: "a caller-named job with a description",
        name: Some("job-named"),
        description: Some("the nightly export"),
        manifests: &["index"],
        window: 600.0,
        destination_path: "nightly/one",
        tags: &[],
    },
    Shape {
        label: "a service-named job harvesting every published manifest, tagged",
        name: None,
        description: Some("every manifest"),
        manifests: &PUBLISHED_MANIFESTS,
        window: 7200.0,
        destination_path: "exports/full",
        tags: &[("env", "prod"), ("team", "video")],
    },
    Shape {
        label: "a caller-named job with an empty description and one tag",
        name: Some("job_with-punctuation-free-name"),
        description: Some(""),
        manifests: &["backup"],
        window: 1.0,
        destination_path: "a",
        tags: &[("owner", "video")],
    },
];

/// RULE 1 — an unchanged retry under the same token replays the one job it created.
#[tokio::test]
async fn an_unchanged_retry_under_the_same_client_token_replays_the_one_job_it_created() {
    for (index, shape) in SHAPES.iter().enumerate() {
        let label = shape.label;
        let emulator = emulator_with_endpoint(label).await;
        // One window per row, computed once: both attempts send the SAME schedule, which
        // is what makes the second a retry rather than a different request.
        let start = epoch_seconds_from_now(60.0);
        let token = format!("fixed-client-token-{index}");

        let (status, first) = create(&emulator, request(shape, start), Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first create expected 200, got {status} {first}"
        );

        // An SDK retrying the same create on a lost response sends the same token again.
        let (status, replay) = create(&emulator, request(shape, start), Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: an identical retry expected a 200 replay, got {status} {replay}"
        );
        assert_eq!(
            replay, first,
            "case {label:?}: an identical retry must replay the first response verbatim, got \
             {replay} where {first} was answered"
        );

        let name = first["HarvestJobName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: a create must report HarvestJobName: {first}"))
            .to_string();
        if let Some(chosen) = shape.name {
            assert_eq!(
                name, chosen,
                "case {label:?}: a caller-chosen harvest-job name must be kept"
            );
        }

        // One logical create left one job: the retry minted no second job under a second
        // server-derived name, and none that consumes the endpoint's quota.
        let jobs = stored_jobs(&emulator, label).await;
        assert_eq!(
            jobs.len(),
            1,
            "case {label:?}: one create retried once must leave ONE job, got {jobs:?}"
        );
        assert_eq!(
            jobs[0]["HarvestJobName"].as_str(),
            Some(name.as_str()),
            "case {label:?}: the stored job must be the one the create reported, got {jobs:?}"
        );

        // The round trip: the replayed job is the job the reader finds, member for member.
        // `Status`, `ModifiedAt` and `ETag` are left out on purpose — a harvest job's
        // status is the CLOCK's to move, so a read may legitimately report a later one.
        let (status, read) = read_job(&emulator, &name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetHarvestJob on the replayed job expected 200, got {status} {read}"
        );
        for member in [
            "Arn",
            "ChannelGroupName",
            "ChannelName",
            "OriginEndpointName",
            "HarvestJobName",
            "CreatedAt",
            "Description",
            "ScheduleConfiguration",
            "HarvestedManifests",
            "Destination",
            "Tags",
        ] {
            assert_eq!(
                read[member], replay[member],
                "case {label:?}: the replay and the read must agree on {member}, got {} where {} \
                 was replayed",
                read[member], replay[member]
            );
        }

        // Idempotence is stable, not a one-shot: a third attempt under the same token
        // still addresses the same job. The identity members are compared rather than the
        // whole body, because the list above advanced the clock this job's status follows.
        let (status, again) = create(&emulator, request(shape, start), Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a third identical attempt expected 200, got {status} {again}"
        );
        for member in ["HarvestJobName", "Arn", "CreatedAt"] {
            assert_eq!(
                again[member], first[member],
                "case {label:?}: a third attempt must still answer with the first job's \
                 {member}, got {} where {} was answered",
                again[member], first[member]
            );
        }
        assert_eq!(
            stored_jobs(&emulator, label).await.len(),
            1,
            "case {label:?}: no number of retries may mint a second job"
        );
    }
}

/// The base request the mismatch table mutates: it carries every optional member, so a row
/// can change one or take one away.
const BASE: Shape = Shape {
    label: "the base request",
    name: Some("job-base"),
    description: Some("the first attempt"),
    manifests: &["index"],
    window: 3600.0,
    destination_path: "out/",
    tags: &[("env", "prod")],
};

/// One row of the mismatch table: what the retry changed about the request, applied to the
/// base body. Each mutation keeps the request VALID on its own — it would be accepted
/// under a fresh token — so the only reason to refuse it is the token it reused.
type Mutation = (&'static str, fn(&mut Value));

/// Every member of the request the token does NOT cover, one row each: the names, the
/// description, the schedule, the destination, the harvested manifests and the tags.
const MUTATIONS: &[Mutation] = &[
    ("the description changed", |body| {
        body["Description"] = json!("the second attempt");
    }),
    ("the description was taken away", |body| {
        body.as_object_mut()
            .expect("a JSON object body")
            .remove("Description");
    }),
    ("the harvest job name changed", |body| {
        body["HarvestJobName"] = json!("job-other");
    }),
    ("the harvest job name was taken away", |body| {
        body.as_object_mut()
            .expect("a JSON object body")
            .remove("HarvestJobName");
    }),
    ("the window starts later", |body| {
        let start = body["ScheduleConfiguration"]["StartTime"]
            .as_f64()
            .expect("a numeric StartTime");
        body["ScheduleConfiguration"]["StartTime"] = json!(start + 30.0);
    }),
    ("the window ends later", |body| {
        let end = body["ScheduleConfiguration"]["EndTime"]
            .as_f64()
            .expect("a numeric EndTime");
        body["ScheduleConfiguration"]["EndTime"] = json!(end + 30.0);
    }),
    ("the destination bucket changed", |body| {
        body["Destination"]["S3Destination"]["BucketName"] = json!("another-harvest-bucket");
    }),
    ("the destination path changed", |body| {
        body["Destination"]["S3Destination"]["DestinationPath"] = json!("elsewhere/");
    }),
    ("another published manifest is harvested", |body| {
        body["HarvestedManifests"]["HlsManifests"]
            .as_array_mut()
            .expect("a JSON array of manifests")
            .push(json!({"ManifestName": PUBLISHED_MANIFESTS[1]}));
    }),
    ("a tag value changed", |body| {
        body["Tags"]["env"] = json!("staging");
    }),
    ("a tag was added", |body| {
        body["Tags"]["owner"] = json!("video");
    }),
    ("the tags were taken away", |body| {
        body.as_object_mut()
            .expect("a JSON object body")
            .remove("Tags");
    }),
];

/// RULE 2 — a repeat under the same token that changed ANY other member is the modelled
/// parameter mismatch, and stores nothing.
#[tokio::test]
async fn a_repeat_under_the_same_token_that_changed_a_member_is_the_modelled_mismatch() {
    for (label, mutate) in MUTATIONS {
        let emulator = emulator_with_endpoint(label).await;
        let start = epoch_seconds_from_now(60.0);
        let token = "fixed-client-token-mismatch";

        let (status, first) = create(&emulator, request(&BASE, start), Some(token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first create expected 200, got {status} {first}"
        );

        let mut changed = request(&BASE, start);
        mutate(&mut changed);
        assert_ne!(
            changed,
            request(&BASE, start),
            "case {label:?}: the row must actually change the request"
        );
        let (status, refused) = create(&emulator, changed.clone(), Some(token)).await;

        assert_eq!(
            status, 409,
            "case {label:?}: a changed retry expected 409, got {status} {refused}"
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

        // A refused create stores nothing: the one job the first attempt made is still
        // the only one, and it still reports what that attempt asked for.
        let jobs = stored_jobs(&emulator, label).await;
        assert_eq!(
            jobs.len(),
            1,
            "case {label:?}: a refused retry must store nothing, got {jobs:?}"
        );
        let (status, read) = read_job(&emulator, "job-base").await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first job must still be readable, got {status} {read}"
        );
        for member in [
            "Arn",
            "CreatedAt",
            "Description",
            "ScheduleConfiguration",
            "HarvestedManifests",
            "Destination",
            "Tags",
        ] {
            assert_eq!(
                read[member], first[member],
                "case {label:?}: a refused retry must leave {member} alone, got {} where {} was \
                 stored",
                read[member], first[member]
            );
        }

        // The SAME changed request is a perfectly good create on its own — sent first,
        // into a store that holds no job, it is accepted. That is what makes the refusal
        // above about the token it reused rather than about the change it carried.
        let fresh = emulator_with_endpoint(label).await;
        let (status, accepted) = create(&fresh, changed, Some(token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the changed request as a first create expected 200, got {status} \
             {accepted}"
        );
    }
}

/// RULE 3 — without the token every request is its own create, and a taken name stays the
/// already-exists conflict.
#[tokio::test]
async fn without_the_client_token_each_create_is_its_own_request() {
    let label = "no token at all";
    let emulator = emulator_with_endpoint(label).await;
    let start = epoch_seconds_from_now(60.0);
    let unnamed = Shape {
        name: None,
        ..BASE
    };

    // Two byte-identical, service-named creates carrying NO token are two creates.
    let mut names = Vec::new();
    for attempt in 0..2 {
        let (status, body) = create(&emulator, request(&unnamed, start), None).await;
        assert_eq!(
            status, 200,
            "case {label:?} attempt {attempt}: expected 200, got {status} {body}"
        );
        names.push(
            body["HarvestJobName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {label:?}: a create must report HarvestJobName: {body}")
                })
                .to_string(),
        );
    }
    assert_ne!(
        names[0], names[1],
        "case {label:?}: two creates carrying no token must mint two differently named jobs"
    );
    assert_eq!(
        stored_jobs(&emulator, label).await.len(),
        2,
        "case {label:?}: two creates carrying no token must leave two jobs"
    );

    // The same request under a token nothing was created with is a FIRST attempt too.
    let (status, body) = create(&emulator, request(&unnamed, start), Some("unseen-token")).await;
    assert_eq!(
        status, 200,
        "case \"an unseen token\": expected 200, got {status} {body}"
    );
    assert_eq!(
        stored_jobs(&emulator, "an unseen token").await.len(),
        3,
        "case \"an unseen token\": a token no stored job carries starts a new create"
    );

    // A name already taken is the already-exists conflict, whatever token comes with it:
    // under none, and under a token no stored job carries.
    let (status, body) = create(&emulator, request(&BASE, start), Some("first-named-token")).await;
    assert_eq!(
        status, 200,
        "case \"a named job\": expected 200, got {status} {body}"
    );
    for (row, token) in [
        ("no token", None),
        ("another token", Some("yet-another-token")),
    ] {
        let (status, taken) = create(&emulator, request(&BASE, start), token).await;
        assert_eq!(
            status, 409,
            "case \"a taken name\"/{row}: expected 409, got {status} {taken}"
        );
        assert_eq!(
            error_type(&taken),
            "ConflictException",
            "case \"a taken name\"/{row}: expected ConflictException, got {taken}"
        );
        assert_eq!(
            conflict_type(&taken),
            "RESOURCE_ALREADY_EXISTS",
            "case \"a taken name\"/{row}: expected RESOURCE_ALREADY_EXISTS, got {taken}"
        );
    }
    assert_eq!(
        stored_jobs(&emulator, "a taken name").await.len(),
        4,
        "case \"a taken name\": a refused create must store nothing"
    );
}
