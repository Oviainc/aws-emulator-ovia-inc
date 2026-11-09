// SPDX-License-Identifier: CC0-1.0

//! What the origin-endpoint delete removes, and which levels of this hierarchy refuse a
//! delete while children remain — driven over HTTP through the same router the emulator
//! boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. The origin-endpoint delete is UNCONDITIONAL and idempotent: whatever the endpoint
//!    holds when the call arrives — no harvest job, one, several, a job the caller named,
//!    one the service named, a cancelled job, a job whose scheduled window has already
//!    opened, an attached endpoint policy — it answers its modelled 200 with an empty
//!    structure, and a repeat answers 200 too. It has no children-attached refusal to make:
//!    the model declares `AccessDeniedException`, `InternalServerException`,
//!    `ThrottlingException`
//!    and `ValidationException` for it and NO `ConflictException`
//!    (`smithy_query(query="operation", operation="DeleteOriginEndpoint")`), while the two
//!    deletes above it — `DeleteChannel` and `DeleteChannelGroup` — do declare it, which is
//!    the 409 they answer while children remain.
//! 2. What the delete removes is exactly the endpoint and the records persisted ON it: its
//!    policy and its harvest jobs. Afterwards the endpoint, its policy and each of its jobs
//!    answer 404 naming `ORIGIN_ENDPOINT` — the level of the address that is missing — the
//!    group's own harvest-job list reports none of them, and an endpoint created again
//!    under the same name starts with no policy and no harvest job. Everything beside it is
//!    untouched: the channel, the group, and a sibling endpoint with its own jobs.
//! 3. The children-attached 409 is the rule of the two levels that model it: while the
//!    endpoint is stored, `DeleteChannel` and `DeleteChannelGroup` are refused 409 /
//!    `RESOURCE_IN_USE` and leave their resource stored; once the endpoint delete has run,
//!    both succeed.

use serde_json::{json, Value};

// The crate's shared test harness: the control-plane client, its error-envelope reader and
// the smallest acceptable `CreateOriginEndpoint` body (`channel_lifecycle`); the
// `CreateHarvestJob` body with its schedule clock and the endpoint-policy document
// (`rejection_rule_and_negative_paths`); the tagging client, which is how a harvest job is
// addressed by the ARN the service minted for it (`resource_tags`).
use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};
use crate::rejection_rule_and_negative_paths::{
    epoch_seconds_from_now, grant_policy, harvest_body,
};

/// The one manifest [`minimal_endpoint`] publishes, and therefore the only one a harvest
/// job on one of this suite's endpoints may harvest.
const MANIFEST: &str = "index";

/// The `ResourceTypeNotFound` value a 404 carries when the ORIGIN ENDPOINT of the address
/// is what is missing (model shape `ResourceTypeNotFound`).
const ORIGIN_ENDPOINT_MISSING: &str = "ORIGIN_ENDPOINT";

/// The names of the hierarchy a case builds, all matching the model's `ResourceName`
/// pattern `^[a-zA-Z0-9_-]+$`.
fn names(suffix: &str) -> (String, String, String) {
    (
        format!("doe-group-{suffix}"),
        format!("doe-channel-{suffix}"),
        format!("doe-endpoint-{suffix}"),
    )
}

/// The origin endpoint's own path — what `DeleteOriginEndpoint` and `GetOriginEndpoint`
/// bind, and the prefix of its policy and harvest-job collections.
fn endpoint_path(group: &str, channel: &str, endpoint: &str) -> String {
    format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}")
}

/// A harvest job stored on an endpoint: the name it is addressed by under its endpoint, and
/// the ARN the service minted for it (what the tagging operations take).
struct Job {
    name: String,
    arn: String,
}

/// One level of the hierarchy, as this suite addresses it.
struct Hierarchy {
    group: String,
    channel: String,
    endpoint: String,
}

impl Hierarchy {
    /// Build channel group -> channel -> origin endpoint under `suffix`.
    async fn create(emulator: &Emulator, case: &str, suffix: &str) -> Self {
        let (group, channel, endpoint) = names(suffix);
        let (status, body) = emulator.create_group(&group, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateChannelGroup expected 200, got {status} {body}"
        );
        let (status, body) = emulator.create_channel(&group, &channel, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateChannel expected 200, got {status} {body}"
        );
        let hierarchy = Self {
            group,
            channel,
            endpoint,
        };
        hierarchy.create_endpoint(emulator, case, None).await;
        hierarchy
    }

    /// `CreateOriginEndpoint` on this hierarchy's endpoint, or on `named` when a case needs
    /// a SIBLING endpoint under the same channel.
    async fn create_endpoint(&self, emulator: &Emulator, case: &str, named: Option<&str>) {
        let endpoint = named.unwrap_or(&self.endpoint);
        let (status, body) = emulator
            .create_endpoint(&self.group, &self.channel, endpoint, minimal_endpoint("TS"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateOriginEndpoint expected 200, got {status} {body}"
        );
    }

    /// The path of `endpoint` under this hierarchy's channel.
    fn path(&self, endpoint: &str) -> String {
        endpoint_path(&self.group, &self.channel, endpoint)
    }

    /// `CreateHarvestJob` on `endpoint`, harvesting the one manifest it publishes.
    ///
    /// `name` is `None` for a job the SERVICE names, which the model allows
    /// (`CreateHarvestJobRequest$HarvestJobName` is optional). `window_already_open` picks a
    /// schedule whose start is in the past rather than a minute ahead, so a row can hold a
    /// job the clock has already taken out of `QUEUED`.
    async fn create_job(
        &self,
        emulator: &Emulator,
        case: &str,
        endpoint: &str,
        name: Option<&str>,
        window_already_open: bool,
    ) -> Job {
        let start = epoch_seconds_from_now(if window_already_open { -600.0 } else { 60.0 });
        let mut request = harvest_body(start, start + 3600.0, &[MANIFEST]);
        if let Some(name) = name {
            request["HarvestJobName"] = json!(name);
        }
        let (status, created) = emulator
            .call(
                "POST",
                &format!("{}/harvestJob", self.path(endpoint)),
                Some(request),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateHarvestJob expected 200, got {status} {created}"
        );
        let name = created["HarvestJobName"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a create must report HarvestJobName: {created}")
            })
            .to_string();
        let arn = created["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a create must report Arn: {created}"))
            .to_string();
        Job { name, arn }
    }

    /// `PutOriginEndpointPolicy` on `endpoint`.
    async fn attach_policy(&self, emulator: &Emulator, case: &str, endpoint: &str) {
        let (status, body) = emulator
            .call(
                "POST",
                &format!("{}/policy", self.path(endpoint)),
                Some(json!({"Policy": grant_policy("mediapackagev2:GetObject")})),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutOriginEndpointPolicy expected 200, got {status} {body}"
        );
    }

    /// `GetOriginEndpointPolicy` on `endpoint`.
    async fn read_policy(&self, emulator: &Emulator, endpoint: &str) -> (u16, Value) {
        emulator
            .call("GET", &format!("{}/policy", self.path(endpoint)), None)
            .await
    }

    /// `DeleteOriginEndpoint` on `endpoint`.
    async fn delete_endpoint(&self, emulator: &Emulator, endpoint: &str) -> (u16, Value) {
        emulator.call("DELETE", &self.path(endpoint), None).await
    }

    /// `GetHarvestJob` on a job of `endpoint`.
    async fn read_job(&self, emulator: &Emulator, endpoint: &str, job: &str) -> (u16, Value) {
        emulator
            .call("GET", &format!("{}/harvestJob/{job}", self.path(endpoint)), None)
            .await
    }

    /// `ListHarvestJobs` over the whole channel group — the group-scoped collection the
    /// model binds (`GET /channelGroup/{ChannelGroupName}/harvestJob`), read at the biggest
    /// page the model allows so one page holds everything a case created.
    async fn list_jobs(&self, emulator: &Emulator, query: &str) -> (u16, Value) {
        emulator
            .call(
                "GET",
                &format!(
                    "/channelGroup/{}/harvestJob?maxResults=100{query}",
                    self.group
                ),
                None,
            )
            .await
    }
}

/// The `HarvestJobName`s a `ListHarvestJobs` page reports, in the order it reported them.
fn listed_job_names(case: &str, body: &Value) -> Vec<String> {
    body.get("Items")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: a list reports Items, got {body}"))
        .iter()
        .map(|item| {
            item.get("HarvestJobName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every listed job names itself, got {item}")
                })
                .to_string()
        })
        .collect()
}

/// One row of the delete table: what the endpoint holds when the delete arrives.
struct Case {
    label: &'static str,
    /// How many harvest jobs the endpoint holds.
    jobs: usize,
    /// Whether the caller names the jobs, or the service does.
    caller_named: bool,
    /// Whether the first job is cancelled before the delete, so the row carries a job in a
    /// TERMINAL status (`CANCELLED`) beside the live ones.
    cancel_first: bool,
    /// Whether the jobs are scheduled for a window that is already open, so the row carries
    /// a job the clock has taken past `QUEUED` rather than one still waiting.
    window_already_open: bool,
    /// Whether an endpoint policy is attached — the sub-resource persisted on the endpoint
    /// beside its harvest jobs.
    policy: bool,
}

/// The table: rows differ in how many harvest jobs the endpoint holds, who named them, what
/// status they are in, whether their scheduled window has opened, and whether the endpoint
/// also carries a policy. Rule 1 holds for every one of them.
const CASES: &[Case] = &[
    Case {
        label: "an endpoint holding nothing",
        jobs: 0,
        caller_named: true,
        cancel_first: false,
        window_already_open: false,
        policy: false,
    },
    Case {
        label: "an endpoint holding only a policy",
        jobs: 0,
        caller_named: true,
        cancel_first: false,
        window_already_open: false,
        policy: true,
    },
    Case {
        label: "one caller-named queued harvest job",
        jobs: 1,
        caller_named: true,
        cancel_first: false,
        window_already_open: false,
        policy: false,
    },
    Case {
        label: "one service-named harvest job",
        jobs: 1,
        caller_named: false,
        cancel_first: false,
        window_already_open: false,
        policy: false,
    },
    Case {
        label: "one harvest job whose scheduled window is already open",
        jobs: 1,
        caller_named: true,
        cancel_first: false,
        window_already_open: true,
        policy: false,
    },
    Case {
        label: "three harvest jobs, one of them cancelled",
        jobs: 3,
        caller_named: true,
        cancel_first: true,
        window_already_open: false,
        policy: false,
    },
    Case {
        label: "two harvest jobs and a policy",
        jobs: 2,
        caller_named: true,
        cancel_first: false,
        window_already_open: false,
        policy: true,
    },
    Case {
        label: "five harvest jobs the service named, and a policy",
        jobs: 5,
        caller_named: false,
        cancel_first: false,
        window_already_open: false,
        policy: true,
    },
];

/// RULE 1 and RULE 2 — the delete answers 200 whatever the endpoint holds, and takes the
/// endpoint and the records persisted on it with it.
#[tokio::test]
async fn the_endpoint_delete_answers_two_hundred_whatever_the_endpoint_holds() {
    let emulator = Emulator::new();
    for (index, case) in CASES.iter().enumerate() {
        let label = case.label;
        let hierarchy = Hierarchy::create(&emulator, label, &index.to_string()).await;
        let endpoint = hierarchy.endpoint.clone();

        let mut jobs = Vec::new();
        for job in 0..case.jobs {
            let chosen = format!("doe-job-{index}-{job}");
            let name = case.caller_named.then_some(chosen.as_str());
            jobs.push(
                hierarchy
                    .create_job(&emulator, label, &endpoint, name, case.window_already_open)
                    .await,
            );
        }
        if case.cancel_first {
            let job = &jobs[0].name;
            let (status, body) = emulator
                .call(
                    "PUT",
                    &format!("{}/harvestJob/{job}", hierarchy.path(&endpoint)),
                    None,
                )
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: CancelHarvestJob expected 200, got {status} {body}"
            );
            let (_, read) = hierarchy.read_job(&emulator, &endpoint, job).await;
            assert_eq!(
                read["Status"].as_str(),
                Some("CANCELLED"),
                "case {label:?}: the cancelled job must report CANCELLED, got {read}"
            );
        }
        if case.policy {
            hierarchy.attach_policy(&emulator, label, &endpoint).await;
            let (status, body) = hierarchy.read_policy(&emulator, &endpoint).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the attached policy must be readable before the delete, got \
                 {status} {body}"
            );
        }

        // The group's harvest-job list holds exactly what this row created, so the "none
        // left" assertion below is measured against a known starting set.
        let (status, listed) = hierarchy.list_jobs(&emulator, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListHarvestJobs expected 200, got {status} {listed}"
        );
        assert_eq!(
            listed_job_names(label, &listed).len(),
            case.jobs,
            "case {label:?}: the group must list the {} jobs the row created, got {listed}",
            case.jobs
        );

        // RULE 1 — the delete is answered, not refused, and answers with an empty structure.
        let (status, body) = hierarchy.delete_endpoint(&emulator, &endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DeleteOriginEndpoint expected its modelled 200, got {status} {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {label:?}: DeleteOriginEndpointResponse carries no member, so the answer is an \
             empty structure, got {body}"
        );
        // `@idempotent`: the endpoint is gone, so the same call again is already that state.
        let (status, body) = hierarchy.delete_endpoint(&emulator, &endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a repeated DeleteOriginEndpoint expected 200, got {status} {body}"
        );

        // RULE 2 — the endpoint, its policy and its harvest jobs are gone, and the 404 names
        // the ORIGIN ENDPOINT as the level of the address that is missing.
        let (status, body) = emulator
            .get_endpoint(&hierarchy.group, &hierarchy.channel, &endpoint)
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: GetOriginEndpoint expected 404 after the delete, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected a ResourceNotFoundException envelope, got {body}"
        );
        assert_eq!(
            body["ResourceTypeNotFound"].as_str(),
            Some(ORIGIN_ENDPOINT_MISSING),
            "case {label:?}: expected ResourceTypeNotFound {ORIGIN_ENDPOINT_MISSING}, got {body}"
        );

        let (status, body) = hierarchy.read_policy(&emulator, &endpoint).await;
        assert_eq!(
            status, 404,
            "case {label:?}: GetOriginEndpointPolicy expected 404 after the delete, got {status} \
             {body}"
        );
        assert_eq!(
            body["ResourceTypeNotFound"].as_str(),
            Some(ORIGIN_ENDPOINT_MISSING),
            "case {label:?}: the policy's 404 must name the missing endpoint, got {body}"
        );

        for job in &jobs {
            let (status, body) = hierarchy.read_job(&emulator, &endpoint, &job.name).await;
            assert_eq!(
                status, 404,
                "case {label:?}: GetHarvestJob on {:?} expected 404 after the delete, got {status} \
                 {body}",
                job.name
            );
            assert_eq!(
                body["ResourceTypeNotFound"].as_str(),
                Some(ORIGIN_ENDPOINT_MISSING),
                "case {label:?}: the job's 404 must name the missing endpoint, got {body}"
            );
            // The job is no longer addressable by the ARN the service minted for it either.
            let (status, body) = emulator.list_tags(&job.arn).await;
            assert_eq!(
                status, 404,
                "case {label:?}: ListTagsForResource on {:?} expected 404 after the delete, got \
                 {status} {body}",
                job.arn
            );
            assert_eq!(
                error_type(&body),
                "NotFoundException",
                "case {label:?}: expected the tagging front end's not-found class, got {body}"
            );
        }

        let (status, listed) = hierarchy.list_jobs(&emulator, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListHarvestJobs expected 200 after the delete, got {status} {listed}"
        );
        assert_eq!(
            listed_job_names(label, &listed),
            Vec::<String>::new(),
            "case {label:?}: the group must list no job of the deleted endpoint, got {listed}"
        );

        // Everything beside the endpoint is untouched.
        let (status, body) = emulator.get_channel(&hierarchy.group, &hierarchy.channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the channel must survive the endpoint's delete, got {status} {body}"
        );
        let (status, body) = emulator.get_group(&hierarchy.group).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the group must survive the endpoint's delete, got {status} {body}"
        );

        // Nothing of the deleted endpoint lingers for the next endpoint of that name: it
        // starts with no policy — a 404 that names no resource type, the one absence no
        // value names — and with no harvest job.
        hierarchy.create_endpoint(&emulator, label, None).await;
        let (status, body) = hierarchy.read_policy(&emulator, &endpoint).await;
        assert_eq!(
            status, 404,
            "case {label:?}: the re-created endpoint must carry no policy, got {status} {body}"
        );
        assert_eq!(
            body["ResourceTypeNotFound"].as_str(),
            None,
            "case {label:?}: the endpoint is present, so only the policy is missing, got {body}"
        );
        let (status, listed) = hierarchy
            .list_jobs(&emulator, &format!("&originEndpointName={endpoint}"))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListHarvestJobs expected 200 for the re-created endpoint, got \
             {status} {listed}"
        );
        assert_eq!(
            listed_job_names(label, &listed),
            Vec::<String>::new(),
            "case {label:?}: the re-created endpoint must hold no harvest job, got {listed}"
        );
    }
}

/// RULE 2 — the delete removes exactly the endpoint it names: a SIBLING endpoint under the
/// same channel keeps its harvest jobs and its policy, which is what makes the removal a
/// containment rule rather than a sweep of the channel.
#[tokio::test]
async fn a_sibling_endpoint_keeps_its_harvest_jobs_and_policy() {
    let emulator = Emulator::new();
    let case = "a sibling endpoint";
    let hierarchy = Hierarchy::create(&emulator, case, "siblings").await;
    let doomed = hierarchy.endpoint.clone();
    let kept = format!("{doomed}-kept");
    hierarchy
        .create_endpoint(&emulator, case, Some(&kept))
        .await;

    let doomed_job = hierarchy
        .create_job(&emulator, case, &doomed, Some("doomed-job"), false)
        .await;
    let kept_job = hierarchy
        .create_job(&emulator, case, &kept, Some("kept-job"), false)
        .await;
    hierarchy.attach_policy(&emulator, case, &kept).await;

    let (status, body) = hierarchy.delete_endpoint(&emulator, &doomed).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteOriginEndpoint expected 200, got {status} {body}"
    );

    // The deleted endpoint's job is gone; the sibling's reads back unchanged.
    let (status, body) = hierarchy
        .read_job(&emulator, &doomed, &doomed_job.name)
        .await;
    assert_eq!(
        status, 404,
        "case {case:?}: the deleted endpoint's job expected 404, got {status} {body}"
    );
    let (status, body) = hierarchy.read_job(&emulator, &kept, &kept_job.name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the sibling's job must survive, got {status} {body}"
    );
    assert_eq!(
        body["Arn"].as_str(),
        Some(kept_job.arn.as_str()),
        "case {case:?}: the surviving job must keep the ARN its create minted, got {body}"
    );
    assert_eq!(
        body["OriginEndpointName"].as_str(),
        Some(kept.as_str()),
        "case {case:?}: the surviving job must still name its endpoint, got {body}"
    );
    let (status, body) = hierarchy.read_policy(&emulator, &kept).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the sibling's policy must survive, got {status} {body}"
    );

    // The group's list is the other reader of the same store, and it agrees.
    let (status, listed) = hierarchy.list_jobs(&emulator, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListHarvestJobs expected 200, got {status} {listed}"
    );
    assert_eq!(
        listed_job_names(case, &listed),
        vec![kept_job.name.clone()],
        "case {case:?}: the group must list exactly the surviving job, got {listed}"
    );
}

/// One row of the refusal table: a delete, and whether the level it addresses refuses while
/// the level below it is stored.
struct DeleteLevel {
    label: &'static str,
    /// The child kind the refusal has to name, for the levels that refuse.
    children: Option<&'static str>,
}

/// The three deletes of this hierarchy, outermost first. `DeleteChannelGroup` and
/// `DeleteChannel` declare `ConflictException` and refuse while children remain;
/// `DeleteOriginEndpoint` declares none and never refuses
/// (`smithy_query(query="operation", operation="DeleteChannelGroup,DeleteChannel,\
/// DeleteOriginEndpoint")`).
const DELETE_LEVELS: &[DeleteLevel] = &[
    DeleteLevel {
        label: "DeleteChannelGroup",
        children: Some("channels"),
    },
    DeleteLevel {
        label: "DeleteChannel",
        children: Some("origin endpoints"),
    },
    DeleteLevel {
        label: "DeleteOriginEndpoint",
        children: None,
    },
];

/// RULE 3 — the children-attached 409 is the rule of the levels that model it, and the
/// endpoint delete is the level that answers instead of refusing.
#[tokio::test]
async fn only_the_levels_that_model_a_conflict_refuse_a_delete_with_children() {
    let emulator = Emulator::new();
    let case = "a full hierarchy with a harvest job";
    let hierarchy = Hierarchy::create(&emulator, case, "levels").await;
    let endpoint = hierarchy.endpoint.clone();
    let job = hierarchy
        .create_job(&emulator, case, &endpoint, Some("levels-job"), false)
        .await;

    for level in DELETE_LEVELS {
        let label = level.label;
        let (status, body) = match label {
            "DeleteChannelGroup" => emulator.delete_group(&hierarchy.group).await,
            "DeleteChannel" => {
                emulator
                    .delete_channel(&hierarchy.group, &hierarchy.channel)
                    .await
            }
            _ => hierarchy.delete_endpoint(&emulator, &endpoint).await,
        };
        match level.children {
            Some(children) => {
                assert_eq!(
                    status, 409,
                    "case {label:?}: a delete with {children} attached expected 409, got {status} \
                     {body}"
                );
                assert_eq!(
                    error_type(&body),
                    "ConflictException",
                    "case {label:?}: expected a ConflictException envelope, got {body}"
                );
                assert_eq!(
                    body["ConflictExceptionType"].as_str(),
                    Some("RESOURCE_IN_USE"),
                    "case {label:?}: expected ConflictExceptionType RESOURCE_IN_USE, got {body}"
                );
                assert!(
                    body["Message"].as_str().unwrap_or_default().contains(children),
                    "case {label:?}: the refusal must name the {children} that have to go first, \
                     got {body}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {label:?}: the endpoint delete models no conflict, so it expected 200, got \
                 {status} {body}"
            ),
        }
    }

    // The two refusals stored nothing: the resources they named are still there, and the
    // harvest job is gone with the endpoint that held it.
    let (status, body) = emulator.get_channel(&hierarchy.group, &hierarchy.channel).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a refused DeleteChannel must leave the channel stored, got {status} {body}"
    );
    let (status, body) = hierarchy.read_job(&emulator, &endpoint, &job.name).await;
    assert_eq!(
        status, 404,
        "case {case:?}: the deleted endpoint's job expected 404, got {status} {body}"
    );

    // With the endpoint gone, each level above it deletes in turn — the same two calls that
    // were refused while a child was left.
    let (status, body) = emulator
        .delete_channel(&hierarchy.group, &hierarchy.channel)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteChannel expected 200 once no endpoint is left, got {status} {body}"
    );
    let (status, body) = emulator.delete_group(&hierarchy.group).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteChannelGroup expected 200 once no channel is left, got {status} \
         {body}"
    );
    let (status, body) = emulator.get_group(&hierarchy.group).await;
    assert_eq!(
        status, 404,
        "case {case:?}: the deleted group expected 404, got {status} {body}"
    );
}
