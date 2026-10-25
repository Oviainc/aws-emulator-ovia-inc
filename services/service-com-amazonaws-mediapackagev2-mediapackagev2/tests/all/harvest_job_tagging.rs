// SPDX-License-Identifier: CC0-1.0

//! Tagging the DEEPEST taggable level of the MediaPackageV2 hierarchy — the harvest job —
//! driven over HTTP through the same router the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. The tagging operations resolve the resource path of EVERY level this service mints
//!    an ARN for and stores a tag map on, the harvest job included: `TagResource` merges
//!    into the job's own stored map, `UntagResource` removes exactly the keys it names,
//!    and both `ListTagsForResource` and `GetHarvestJob` then report that one map.
//! 2. A write through the tagging API is a commit on exactly the job addressed: it
//!    rotates that job's entity tag, leaves its other members alone, and leaves the tag
//!    maps of its endpoint, channel and group untouched.
//! 3. A well-formed ARN of a level this service serves is judged by LOOKUP, so a
//!    harvest-job ARN naming a job (or a parent of one) that is not stored is the
//!    not-found class (404) and not the malformed-ARN class (400). A string whose
//!    resource path is not one this service builds stays malformed (400).
//!
//! The same three rules at the group, channel and origin-endpoint levels are covered by
//! `resource_tags` and `resource_tagging`; this suite is the harvest-job row of each.

use std::collections::BTreeMap;

use serde_json::{json, Value};

// The crate's shared test harness: the control-plane client and its error-envelope
// reader (`channel_lifecycle`), the three tagging calls and the tag-map readers
// (`resource_tags`), and the `CreateHarvestJob` body plus its schedule clock
// (`rejection_rule_and_negative_paths`). This suite adds only what is specific to
// harvest-job tagging.
use crate::channel_lifecycle::{error_type, Emulator};
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};
use crate::resource_tags::{map_of, tag_map};

/// A pair list, as the tables below spell a tag map.
type Pairs = &'static [(&'static str, &'static str)];

/// The manifests the suite's origin endpoint publishes. A harvest job may only harvest a
/// manifest its endpoint publishes, so the rows that harvest two need both to exist.
const PUBLISHED_MANIFESTS: [&str; 2] = ["index", "backup"];

/// The names of the hierarchy a case builds, all matching the model's `ResourceName`
/// pattern `^[a-zA-Z0-9_-]+$`.
fn names(suffix: &str) -> (String, String, String) {
    (
        format!("hjt-group-{suffix}"),
        format!("hjt-channel-{suffix}"),
        format!("hjt-endpoint-{suffix}"),
    )
}

/// One harvest job, with everything a case needs to address it again.
struct Job {
    group: String,
    channel: String,
    endpoint: String,
    name: String,
    /// The ARN the service minted for the job — the string the tagging operations take.
    arn: String,
}

impl Job {
    /// `GetHarvestJob` on this job.
    async fn get(&self, emulator: &Emulator) -> (u16, Value) {
        let collection = harvest_job_path(&self.group, &self.channel, &self.endpoint);
        emulator
            .call("GET", &format!("{collection}/{}", self.name), None)
            .await
    }
}

/// `CreateHarvestJob` / `ListHarvestJobs` bind the endpoint's `harvestJob` collection.
fn harvest_job_path(group: &str, channel: &str, endpoint: &str) -> String {
    format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}/harvestJob")
}

/// Build a channel group, a channel, an origin endpoint publishing
/// [`PUBLISHED_MANIFESTS`], and one harvest job inside it — the job carrying `tags` and
/// every level above it carrying none, so an assertion about the job's tag map cannot be
/// satisfied by a parent's.
///
/// `name` is `None` for a job the SERVICE names, which is a legal `CreateHarvestJob` (the
/// model leaves `HarvestJobName` optional), and the ARN is read back from the response
/// either way: the point of the suite is that the tagging operations accept the ARN this
/// service minted, whoever chose the name in it.
async fn create_job(
    emulator: &Emulator,
    case: &str,
    suffix: &str,
    name: Option<&str>,
    manifests: &[&str],
    tags: &BTreeMap<String, String>,
) -> Job {
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
    let (status, body) = emulator
        .create_endpoint(
            &group,
            &channel,
            &endpoint,
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

    // A window that opens in a minute and runs for an hour: the job stays inside its
    // scheduled window for the whole test, so nothing the clock owes it ends it early.
    let start = epoch_seconds_from_now(60.0);
    let mut request = harvest_body(start, start + 3600.0, manifests);
    // A description, so the "a tag write left the other members alone" assertions below
    // compare real values rather than two absences.
    request["Description"] = json!(format!("harvest job for {case}"));
    // `CreateHarvestJobRequest$Tags` carries no `@jsonName`
    // (`smithy_query(query="operation", operation="CreateHarvestJob")`).
    request["Tags"] = json!(tags);
    if let Some(name) = name {
        request["HarvestJobName"] = json!(name);
    }
    let (status, created) = emulator
        .call(
            "POST",
            &harvest_job_path(&group, &channel, &endpoint),
            Some(request),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateHarvestJob expected 200, got {status} {created}"
    );
    let job = created["HarvestJobName"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a create must report HarvestJobName: {created}"))
        .to_string();
    let arn = created["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a create must report Arn: {created}"))
        .to_string();
    if let Some(name) = name {
        assert_eq!(
            job, name,
            "case {case:?}: a caller-chosen harvest-job name must be kept"
        );
    }

    Job {
        group,
        channel,
        endpoint,
        name: job,
        arn,
    }
}

/// An ARN of this service with `path` as its resource field, built from the prefix of an
/// ARN the service itself minted — so the partition, Region and account of every ARN this
/// suite submits are the service's own rather than values written out here.
fn arn_with_path(minted: &str, path: &str) -> String {
    let (prefix, _) = minted
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("a minted ARN has six colon-separated fields: {minted}"));
    format!("{prefix}:{path}")
}

/// One row of the merge table: the tags the job is created with, the `TagResource` writes
/// applied to it, the keys then removed with `UntagResource`, and the map it must carry
/// afterwards.
struct TagCase {
    label: &'static str,
    created_with: Pairs,
    writes: &'static [Pairs],
    untag: &'static [&'static str],
    expected: Pairs,
}

/// The merge table. Rows differ in whether a written key is new or already present, in
/// how many writes arrive, in how many pairs each carries, in whether keys are then
/// removed, and in the form of the keys and values (ASCII, punctuation, non-ASCII, empty
/// value) — `TagKey` and `TagValue` carry no `@length` or `@pattern`
/// (`smithy_query(query="shape", shape="TagKey,TagValue")`), so any string is a legal tag.
const TAG_CASES: &[TagCase] = &[
    TagCase {
        label: "new keys on a job created untagged",
        created_with: &[],
        writes: &[&[("env", "prod"), ("team", "video")]],
        untag: &[],
        expected: &[("env", "prod"), ("team", "video")],
    },
    TagCase {
        label: "the create-time tags are reachable and an existing key is replaced",
        created_with: &[("env", "prod"), ("team", "video")],
        writes: &[&[("env", "staging")]],
        untag: &[],
        expected: &[("env", "staging"), ("team", "video")],
    },
    TagCase {
        label: "a new key is appended to the create-time tags",
        created_with: &[("env", "prod")],
        writes: &[&[("costCenter", "1234")]],
        untag: &[],
        expected: &[("costCenter", "1234"), ("env", "prod")],
    },
    TagCase {
        label: "successive writes accumulate",
        created_with: &[],
        writes: &[&[("a", "1")], &[("b", "2")], &[("c", "3")]],
        untag: &[],
        expected: &[("a", "1"), ("b", "2"), ("c", "3")],
    },
    TagCase {
        label: "the last write wins for a repeated key",
        created_with: &[("k", "0")],
        writes: &[&[("k", "1")], &[("k", "2")]],
        untag: &[],
        expected: &[("k", "2")],
    },
    TagCase {
        label: "an empty tag map leaves the job unchanged",
        created_with: &[("env", "prod")],
        writes: &[&[]],
        untag: &[],
        expected: &[("env", "prod")],
    },
    TagCase {
        label: "keys and values of every form",
        created_with: &[("plain", "value")],
        writes: &[&[
            ("dotted.key:with/punctuation", "a b c"),
            ("ключ", "значение"),
            ("empty-value", ""),
        ]],
        untag: &[],
        expected: &[
            ("dotted.key:with/punctuation", "a b c"),
            ("empty-value", ""),
            ("plain", "value"),
            ("ключ", "значение"),
        ],
    },
    TagCase {
        label: "untagging removes exactly the named keys and ignores an absent one",
        created_with: &[("a", "1"), ("b", "2")],
        writes: &[&[("c", "3")]],
        untag: &["a", "never-set"],
        expected: &[("b", "2"), ("c", "3")],
    },
    TagCase {
        label: "untagging every key leaves the job carrying none",
        created_with: &[("a", "1")],
        writes: &[&[("b", "2")]],
        untag: &["a", "b"],
        expected: &[],
    },
];

#[tokio::test]
async fn the_tagging_operations_reach_a_harvest_jobs_own_tag_map() {
    let emulator = Emulator::new();
    for (index, case) in TAG_CASES.iter().enumerate() {
        // The jobs differ along the two things a harvest job's identity varies in and the
        // rule is indifferent to: who chose the name (caller or service) and how many of
        // the endpoint's manifests it harvests.
        let chosen = format!("job_{index}-named");
        let name = (index % 2 == 0).then_some(chosen.as_str());
        let manifests: &[&str] = if index % 3 == 0 {
            &PUBLISHED_MANIFESTS
        } else {
            &PUBLISHED_MANIFESTS[..1]
        };
        let job = create_job(
            &emulator,
            case.label,
            &index.to_string(),
            name,
            manifests,
            &map_of(case.created_with),
        )
        .await;
        let label = case.label;

        // Before any write: the tags the create stored are reported through the tagging
        // API, so the merge below is measured against a known starting map.
        let (status, listed) = emulator.list_tags(&job.arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListTagsForResource on a harvest-job ARN expected 200, got {status} \
             {listed}"
        );
        assert_eq!(
            tag_map(&listed, "tags"),
            map_of(case.created_with),
            "case {label:?}: the create-time tags must be reported, got {listed}"
        );

        for (write, tags) in case.writes.iter().enumerate() {
            let (status, body) = emulator.tag(&job.arn, &map_of(tags)).await;
            assert_eq!(
                status, 204,
                "case {label:?} write {write}: TagResource expected its modelled 204, got \
                 {status} {body}"
            );
            assert!(
                body.is_null(),
                "case {label:?} write {write}: a 204 carries no body, got {body}"
            );
        }

        if !case.untag.is_empty() {
            let (status, body) = emulator.untag(&job.arn, case.untag).await;
            assert_eq!(
                status, 204,
                "case {label:?}: UntagResource expected its modelled 204, got {status} {body}"
            );
        }

        let expected = map_of(case.expected);
        let (status, listed) = emulator.list_tags(&job.arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListTagsForResource expected 200, got {status} {listed}"
        );
        assert_eq!(
            tag_map(&listed, "tags"),
            expected,
            "case {label:?}: ListTagsForResource reported {listed}"
        );

        // The round trip through the job's own reader: the tagging API writes to the one
        // record `GetHarvestJob` reads, not to a map beside it. `GetHarvestJobResponse$Tags`
        // carries no `@jsonName`.
        let (status, read) = job.get(&emulator).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetHarvestJob must still find the job, got {status} {read}"
        );
        assert_eq!(
            tag_map(&read, "Tags"),
            expected,
            "case {label:?}: GetHarvestJob reported {read}"
        );
    }
}

#[tokio::test]
async fn a_tag_write_commits_the_harvest_job_and_nothing_else() {
    let emulator = Emulator::new();
    let job = create_job(
        &emulator,
        "a tag write commits the job",
        "commit",
        Some("job-commit"),
        &PUBLISHED_MANIFESTS,
        &map_of(&[("env", "prod")]),
    )
    .await;

    let (_, before) = job.get(&emulator).await;
    let (status, body) = emulator
        .tag(&job.arn, &map_of(&[("env", "staging"), ("owner", "video")]))
        .await;
    assert_eq!(
        status, 204,
        "case \"commit\": TagResource expected 204, got {status} {body}"
    );

    let (_, after) = job.get(&emulator).await;
    assert_ne!(
        after["ETag"], before["ETag"],
        "case \"commit\": a tag write is a commit, so it must rotate the job's entity tag; got \
         {} both times",
        after["ETag"]
    );
    // Everything the job's identity and request are made of is untouched by a tag write.
    // `Status` is left out on purpose: a harvest job's status is the CLOCK's to move, so
    // it may legitimately differ between the two reads.
    for member in [
        "Arn",
        "CreatedAt",
        "Description",
        "HarvestJobName",
        "ScheduleConfiguration",
        "HarvestedManifests",
        "Destination",
    ] {
        assert_eq!(
            after[member], before[member],
            "case \"commit\": a tag write must not change {member}, got {} where {} was stored",
            after[member], before[member]
        );
    }
    assert!(
        after["Description"].is_string(),
        "case \"commit\": the comparison above needs a description to compare, got {after}"
    );

    // Tagging the job is not tagging anything above it.
    let (_, endpoint) = emulator
        .get_endpoint(&job.group, &job.channel, &job.endpoint)
        .await;
    assert!(
        tag_map(&endpoint, "Tags").is_empty(),
        "case \"commit\": tagging the job must leave its endpoint untagged, got {endpoint}"
    );
    let (_, channel) = emulator.get_channel(&job.group, &job.channel).await;
    assert!(
        tag_map(&channel, "Tags").is_empty(),
        "case \"commit\": tagging the job must leave its channel untagged, got {channel}"
    );
    let (_, group) = emulator.get_group(&job.group).await;
    assert!(
        tag_map(&group, "tags").is_empty(),
        "case \"commit\": tagging the job must leave its group untagged, got {group}"
    );
}

#[tokio::test]
async fn a_harvest_job_arn_naming_nothing_stored_is_the_not_found_class() {
    let emulator = Emulator::new();
    let job = create_job(
        &emulator,
        "the not-found class",
        "absent",
        Some("job-present"),
        &PUBLISHED_MANIFESTS[..1],
        &map_of(&[("env", "prod")]),
    )
    .await;
    let (group, channel, endpoint) = (&job.group, &job.channel, &job.endpoint);

    // Well-formed harvest-job ARNs whose resource is not stored. The rows differ in WHICH
    // level of the path is missing — the job itself, then each ancestor in turn.
    let absent: &[(&str, String)] = &[
        (
            "an absent job under a stored endpoint",
            arn_with_path(
                &job.arn,
                &format!(
                    "channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}\
                     /harvestJob/no-such-job"
                ),
            ),
        ),
        (
            "a job under an absent endpoint",
            arn_with_path(
                &job.arn,
                &format!(
                    "channelGroup/{group}/channel/{channel}/originEndpoint/no-such-endpoint\
                     /harvestJob/job-present"
                ),
            ),
        ),
        (
            "a job under an absent channel",
            arn_with_path(
                &job.arn,
                &format!(
                    "channelGroup/{group}/channel/no-such-channel/originEndpoint/{endpoint}\
                     /harvestJob/job-present"
                ),
            ),
        ),
        (
            "a job under an absent channel group",
            arn_with_path(
                &job.arn,
                "channelGroup/no-such-group/channel/c/originEndpoint/e/harvestJob/j",
            ),
        ),
    ];
    for (label, arn) in absent {
        for (operation, status, body) in refusals(&emulator, arn).await {
            assert_eq!(
                status, 404,
                "case {operation:?} with {label:?}: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_type(&body),
                "NotFoundException",
                "case {operation:?} with {label:?}: expected NotFoundException, got {body}"
            );
        }
    }

    // Strings whose resource path is not one this service builds stay malformed. The rows
    // differ in HOW the harvest-job path is wrong: the type with no name after it, an
    // empty name, a level deeper than the hierarchy goes, and a mis-spelled type.
    let malformed: &[(&str, String)] = &[
        (
            "a harvest-job path with no job name",
            arn_with_path(
                &job.arn,
                &format!(
                    "channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}/harvestJob"
                ),
            ),
        ),
        (
            "a harvest-job path with an empty job name",
            arn_with_path(
                &job.arn,
                &format!(
                    "channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}\
                     /harvestJob/"
                ),
            ),
        ),
        (
            "a level deeper than the hierarchy goes",
            arn_with_path(
                &job.arn,
                &format!(
                    "channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}\
                     /harvestJob/job-present/output/o"
                ),
            ),
        ),
        (
            "a mis-spelled harvest-job type",
            arn_with_path(
                &job.arn,
                &format!(
                    "channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}\
                     /harvestjob/job-present"
                ),
            ),
        ),
    ];
    for (label, arn) in malformed {
        for (operation, status, body) in refusals(&emulator, arn).await {
            assert_eq!(
                status, 400,
                "case {operation:?} with {label:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_type(&body),
                "BadRequestException",
                "case {operation:?} with {label:?}: expected BadRequestException, got {body}"
            );
        }
    }

    // None of those refusals touched the job that IS stored.
    let (status, listed) = emulator.list_tags(&job.arn).await;
    assert_eq!(
        status, 200,
        "case \"the stored job\": ListTagsForResource expected 200, got {status} {listed}"
    );
    assert_eq!(
        tag_map(&listed, "tags"),
        map_of(&[("env", "prod")]),
        "case \"the stored job\": a refused call must change no job's tags, got {listed}"
    );
}

/// Every tagging operation's answer for `arn`, as `(operation, status, body)` — the three
/// share one ARN rule, so each table row above is asserted over all three.
async fn refusals(emulator: &Emulator, arn: &str) -> Vec<(&'static str, u16, Value)> {
    let (status, body) = emulator.list_tags(arn).await;
    let mut answers = vec![("ListTagsForResource", status, body)];
    let (status, body) = emulator.tag(arn, &map_of(&[("k", "v")])).await;
    answers.push(("TagResource", status, body));
    let (status, body) = emulator.untag(arn, &["k"]).await;
    answers.push(("UntagResource", status, body));
    answers
}
