// SPDX-License-Identifier: CC0-1.0

//! Braket resource tagging, end to end over HTTP.
//!
//! The rules these suites pin:
//!
//! * The three tagging doors act on the tag set stored on the Braket resource
//!   `resourceArn` names, whatever kind of resource that is: `TagResource` MERGES the
//!   submitted keys into it (a new key is added, a key already there takes the submitted
//!   value, a key the request does not name is untouched), `UntagResource` removes the
//!   named keys and only those, and `ListTagsForResource` reports exactly the set those
//!   writes left — as a map, empty when the resource carries no tag.
//! * A resource's own read door reports the same set: the tags `GetQuantumTask`,
//!   `GetJob` and `SearchSpendingLimits` report are the ones the tagging doors wrote.
//! * `resourceArn` is the only thing that resolves the resource: an ARN naming no stored
//!   resource — never minted, or a device AWS publishes rather than this API creates, so
//!   it holds no tag set — is HTTP 404 `NotFoundException` with a fixed sentence, and a
//!   value that is not a Braket resource ARN is `ValidationException` (400). Neither
//!   touches the store.
//!
//! Each suite asserts its rule over a TABLE spanning the class rather than one recorded
//! call: the rows differ along the KIND of resource tagged (quantum task, hybrid job,
//! spending limit), whether the create already carried tags, and the write's shape (one
//! key, several keys, none, a key that overwrites, an empty value, a key the resource
//! does not carry, every key at once, the same call repeated). Everything goes through
//! the production router, reusing the harness in `quantum_task_loop.rs`, so what is
//! pinned is what a Braket client sees.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::hybrid_job_loop::job_body;
use crate::quantum_task_loop::{create_body, encode, member, Braket, Reply};
use crate::spending_limits::{a_device, create_limit, limit_body, limit_by_arn, CREATED};

/// The fixed sentence the tagging doors word for a `resourceArn` that resolves to no
/// resource they hold the tags of. It interpolates nothing from the request.
const RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The error code the tagging doors answer a `resourceArn` resolving to nothing with.
const NOT_FOUND_CODE: &str = "NotFoundException";

/// The sentence the tagging doors word for a `resourceArn` that is not a Braket resource
/// ARN at all, naming the member as the model spells it.
const INVALID_ARN: &str = "resourceArn is not valid.";

/// `GET /tags/{resourceArn}`.
async fn list_tags(braket: &Braket, arn: &str) -> Reply {
    braket
        .call("GET", &format!("/tags/{}", encode(arn)), None)
        .await
}

/// `POST /tags/{resourceArn}`.
async fn tag_resource(braket: &Braket, arn: &str, tags: &[(&str, &str)]) -> Reply {
    let mut submitted = json!({});
    for (key, value) in tags {
        submitted[*key] = json!(*value);
    }
    braket
        .call(
            "POST",
            &format!("/tags/{}", encode(arn)),
            Some(json!({ "tags": submitted })),
        )
        .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…`, one query entry per key, as an SDK serializes
/// an `@httpQuery` list.
async fn untag_resource(braket: &Braket, arn: &str, keys: &[&str]) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode(key)))
        .collect::<Vec<String>>()
        .join("&");
    braket
        .call(
            "DELETE",
            &format!("/tags/{}?{query}", encode(arn)),
            None,
        )
        .await
}

/// The tag set `ListTagsForResource` reports, asserting the call was served.
async fn reported_tags(braket: &Braket, arn: &str, case: &str) -> BTreeMap<String, String> {
    let reply = list_tags(braket, arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {} {}",
        reply.status, reply.body
    );
    tags_of(&reply.body, case)
}

/// The `tags` member of a response body, as a map. A served `ListTagsForResource` always
/// carries the member, even when the resource holds no tag.
fn tags_of(body: &Value, case: &str) -> BTreeMap<String, String> {
    let map = body
        .get("tags")
        .unwrap_or_else(|| panic!("case {case:?}: the response carries no tags member: {body}"))
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: tags is not a map: {body}"));
    map.iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: tag {key} is not a string: {body}"))
                    .to_string(),
            )
        })
        .collect()
}

/// An expected tag set, written as the pairs a row states.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One taggable resource under test: what it is, the ARN that resolves it, and the tags
/// its create already put on it.
struct Resource {
    case: &'static str,
    arn: String,
    created_with: Vec<(&'static str, &'static str)>,
}

/// A quantum task, a hybrid job and a spending limit, each created twice — once bare and
/// once with tags already on it — so every rule below is asserted over every KIND of
/// resource this service holds tags for and over both starting points.
async fn taggable_resources(braket: &Braket) -> Vec<Resource> {
    let device = braket.an_online_device().await;
    let qpu = a_device(braket, "QPU", "ONLINE").await;
    let created_with: Vec<(&'static str, &'static str)> =
        vec![("env", "test"), ("owner", "ada")];
    let mut resources: Vec<Resource> = Vec::new();

    for (index, carries) in [Vec::new(), created_with.clone()].into_iter().enumerate() {
        // A quantum task.
        let mut body = create_body(&format!("tok-task-{index}"), &device, 10);
        if !carries.is_empty() {
            let mut tags = json!({});
            for (key, value) in &carries {
                tags[*key] = json!(*value);
            }
            body["tags"] = tags;
        }
        let reply = braket.create_task(body).await;
        assert_eq!(
            reply.status, 201,
            "creating the quantum task to tag: expected 201, got {} {}",
            reply.status, reply.body
        );
        resources.push(Resource {
            case: if carries.is_empty() {
                "a quantum task created with no tags"
            } else {
                "a quantum task created with tags"
            },
            arn: reply.str_member("quantumTaskArn"),
            created_with: carries.clone(),
        });

        // A hybrid job.
        let mut body = job_body(
            &format!("tok-job-{index}"),
            &format!("job-to-tag-{index}"),
            &device,
        );
        if !carries.is_empty() {
            let mut tags = json!({});
            for (key, value) in &carries {
                tags[*key] = json!(*value);
            }
            body["tags"] = tags;
        }
        let reply = braket.create_job(body).await;
        assert_eq!(
            reply.status, 201,
            "creating the hybrid job to tag: expected 201, got {} {}",
            reply.status, reply.body
        );
        resources.push(Resource {
            case: if carries.is_empty() {
                "a hybrid job created with no tags"
            } else {
                "a hybrid job created with tags"
            },
            arn: reply.str_member("jobArn"),
            created_with: carries.clone(),
        });

        // A spending limit.
        let mut body = limit_body(&format!("tok-limit-{index}"), &qpu, "100.50");
        if !carries.is_empty() {
            let mut tags = json!({});
            for (key, value) in &carries {
                tags[*key] = json!(*value);
            }
            body["tags"] = tags;
        }
        let reply = create_limit(braket, body).await;
        assert_eq!(
            reply.status, CREATED,
            "creating the spending limit to tag: expected {CREATED}, got {} {}",
            reply.status, reply.body
        );
        resources.push(Resource {
            case: if carries.is_empty() {
                "a spending limit created with no tags"
            } else {
                "a spending limit created with tags"
            },
            arn: reply.str_member("spendingLimitArn"),
            created_with: carries.clone(),
        });
    }
    resources
}

/// One write in the table, and what the rule says the resource's tag set holds after it.
enum Step {
    /// `TagResource` with these pairs: the set gains them, a key already there taking the
    /// submitted value.
    Tag(&'static [(&'static str, &'static str)]),
    /// `UntagResource` with these keys: the set loses exactly the ones it carried.
    Untag(&'static [&'static str]),
}

/// The writes every resource is driven through, spanning the shapes a tagging call takes:
/// one key, several at once, none at all, a key that overwrites a value, an empty value, a
/// key the resource does not carry, several keys at once, and the same call repeated.
const STEPS: &[(&str, Step)] = &[
    ("one new key", Step::Tag(&[("team", "quantum")])),
    (
        "several new keys at once",
        Step::Tag(&[("project", "shor"), ("stage", "alpha")]),
    ),
    ("no keys at all", Step::Tag(&[])),
    ("a key that overwrites", Step::Tag(&[("team", "optics")])),
    ("an empty value", Step::Tag(&[("note", "")])),
    ("one key removed", Step::Untag(&["stage"])),
    (
        "a key the resource does not carry",
        Step::Untag(&["never-set"]),
    ),
    (
        "several keys at once, one of them absent",
        Step::Untag(&["project", "never-set", "note"]),
    ),
    ("the same removal again", Step::Untag(&["project"])),
    ("the last key this suite added", Step::Untag(&["team"])),
];

/// `TagResource` and `UntagResource` leave a set `ListTagsForResource` reports exactly,
/// for every kind of taggable resource and every shape of write.
#[tokio::test]
async fn the_tag_doors_write_the_set_list_tags_for_resource_reports() {
    let braket = Braket::new();
    let resources = taggable_resources(&braket).await;

    for resource in &resources {
        let case = resource.case;
        // The starting point is what the create put on the resource — which the tagging
        // doors neither need nor disturb.
        let mut held = expected(&resource.created_with);
        assert_eq!(
            reported_tags(&braket, &resource.arn, case).await,
            held,
            "case {case:?}: the tags the create supplied are the set reported before any \
             tagging call"
        );

        for (step_case, step) in STEPS {
            let full = format!("{case} / {step_case}");
            let reply = match step {
                Step::Tag(pairs) => {
                    for (key, value) in *pairs {
                        held.insert((*key).to_string(), (*value).to_string());
                    }
                    tag_resource(&braket, &resource.arn, pairs).await
                }
                Step::Untag(keys) => {
                    held.retain(|key, _| !keys.contains(&key.as_str()));
                    untag_resource(&braket, &resource.arn, keys).await
                }
            };
            assert_eq!(
                reply.status, 200,
                "case {full:?}: expected 200, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error, None,
                "case {full:?}: a served tagging call carries no error, got {:?}",
                reply.error
            );
            let reported = reported_tags(&braket, &resource.arn, &full).await;
            assert_eq!(
                reported, held,
                "case {full:?}: expected the tag set the write leaves, {held:?}, got \
                 {reported:?}"
            );
        }

        // Every key this suite added is gone, so the set is the create's again: a write
        // removed nothing it was not asked to remove.
        let reported = reported_tags(&braket, &resource.arn, case).await;
        assert_eq!(
            reported,
            expected(&resource.created_with),
            "case {case:?}: expected the create's own tags to have survived the whole \
             table, got {reported:?}"
        );
    }

    // Tagging is per RESOURCE: the keys one resource carried never appeared on another.
    // Each resource above ends on its create's tags, which is what this re-reads.
    for resource in &resources {
        let reported = reported_tags(&braket, &resource.arn, resource.case).await;
        assert_eq!(
            reported,
            expected(&resource.created_with),
            "case {:?}: the writes aimed at the other resources left this one alone, got \
             {reported:?}",
            resource.case
        );
    }
}

/// A resource's own read door reports the tags the tagging doors wrote: the tag set is
/// held once per resource, not once per door.
#[tokio::test]
async fn a_resources_own_read_reports_the_tags_the_tag_doors_wrote() {
    let braket = Braket::new();
    let device = braket.an_online_device().await;
    let qpu = a_device(&braket, "QPU", "ONLINE").await;

    let task = braket
        .create_task(create_body("tok-task-read", &device, 10))
        .await;
    assert_eq!(
        task.status, 201,
        "creating the quantum task: got {} {}",
        task.status, task.body
    );
    let task_arn = task.str_member("quantumTaskArn");

    let job = braket
        .create_job(job_body("tok-job-read", "job-read-back", &device))
        .await;
    assert_eq!(
        job.status, 201,
        "creating the hybrid job: got {} {}",
        job.status, job.body
    );
    let job_arn = job.str_member("jobArn");

    let limit = create_limit(&braket, limit_body("tok-limit-read", &qpu, "42")).await;
    assert_eq!(
        limit.status, CREATED,
        "creating the spending limit: got {} {}",
        limit.status, limit.body
    );
    let limit_arn = limit.str_member("spendingLimitArn");

    // (case, the ARN, the tags to write, the keys to remove afterwards)
    let written: &[(&str, &str)] = &[("team", "quantum"), ("env", "test")];
    let rows: Vec<(&str, &str)> = vec![
        ("a quantum task", task_arn.as_str()),
        ("a hybrid job", job_arn.as_str()),
        ("a spending limit", limit_arn.as_str()),
    ];
    for (case, arn) in &rows {
        let reply = tag_resource(&braket, arn, written).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: tagging expected 200, got {} {}",
            reply.status, reply.body
        );
    }

    // The resource's OWN door: `GetQuantumTask`, `GetJob` and the spending-limit search
    // each report the set the tagging door wrote.
    let read = braket.get_task(&task_arn).await;
    assert_eq!(
        tags_of(&read.body, "a quantum task"),
        expected(written),
        "case {:?}: GetQuantumTask reports the tags TagResource wrote, got {}",
        "a quantum task",
        read.body
    );
    let read = braket.get_job(&job_arn).await;
    assert_eq!(
        tags_of(&read.body, "a hybrid job"),
        expected(written),
        "case {:?}: GetJob reports the tags TagResource wrote, got {}",
        "a hybrid job",
        read.body
    );
    let summary = limit_by_arn(&braket, &qpu, &limit_arn).await;
    assert_eq!(
        tags_of(&summary, "a spending limit"),
        expected(written),
        "case {:?}: SearchSpendingLimits reports the tags TagResource wrote, got {summary}",
        "a spending limit"
    );

    // And a removal is just as visible there: the resource's own read reports no tag, or
    // no tags member at all, once the set is empty.
    for (case, arn) in &rows {
        let reply = untag_resource(&braket, arn, &["team", "env"]).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: untagging expected 200, got {} {}",
            reply.status, reply.body
        );
        let reported = reported_tags(&braket, arn, case).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected no tags left, got {reported:?}"
        );
    }
    let read = braket.get_task(&task_arn).await;
    assert_eq!(
        read.body
            .get("tags")
            .and_then(Value::as_object)
            .map_or(0, serde_json::Map::len),
        0,
        "case {:?}: GetQuantumTask reports no tag once the set is empty, got {}",
        "a quantum task",
        read.body
    );
    let summary = limit_by_arn(&braket, &qpu, &limit_arn).await;
    assert_eq!(
        summary
            .get("tags")
            .and_then(Value::as_object)
            .map_or(0, serde_json::Map::len),
        0,
        "case {:?}: SearchSpendingLimits reports no tag once the set is empty, got {summary}",
        "a spending limit"
    );
}

/// What `resourceArn` resolves to decides the answer, for all three doors: a stored
/// resource is served, an ARN naming none is a 404 with the fixed sentence, and a value
/// that is not a Braket resource ARN is a 400. A refused call changes nothing.
#[tokio::test]
async fn the_arn_decides_which_resource_the_tag_doors_resolve() {
    let braket = Braket::new();
    let device = braket.an_online_device().await;
    let region = "eu-west-1";
    let account = engine::EMULATOR_ACCOUNT_ID;

    // The resource a refused call must not touch.
    let created = braket
        .create_task(create_body("tok-task-refusals", &device, 10))
        .await;
    assert_eq!(
        created.status, 201,
        "creating the quantum task: got {} {}",
        created.status, created.body
    );
    let arn = created.str_member("quantumTaskArn");
    let written: &[(&str, &str)] = &[("team", "quantum")];
    let tagged = tag_resource(&braket, &arn, written).await;
    assert_eq!(
        tagged.status, 200,
        "tagging the task the refusals must not touch: got {} {}",
        tagged.status, tagged.body
    );

    // (case, the ARN the request names, expected status, expected error code) — a
    // well-formed ARN of each taggable kind naming nothing stored, the same under another
    // region and account, a PUBLISHED device ARN (a device holds no tag set), a resource
    // field with no id, an ARN with no resource type at all, an ARN of another service,
    // and values that are no ARN at all.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a quantum-task ARN naming no stored task",
            format!("arn:aws:braket:{region}:{account}:quantum-task/ghost-1"),
            404,
            NOT_FOUND_CODE,
        ),
        (
            "a job ARN naming no stored job",
            format!("arn:aws:braket:{region}:{account}:job/ghost-2"),
            404,
            NOT_FOUND_CODE,
        ),
        (
            "a spending-limit ARN naming no stored limit",
            format!("arn:aws:braket:{region}:{account}:spending-limit/ghost-3"),
            404,
            NOT_FOUND_CODE,
        ),
        (
            "a well-formed ARN in another region",
            format!("arn:aws:braket:us-east-1:{account}:quantum-task/ghost-4"),
            404,
            NOT_FOUND_CODE,
        ),
        (
            "a well-formed ARN under another account",
            format!("arn:aws:braket:{region}:123456789012:quantum-task/ghost-5"),
            404,
            NOT_FOUND_CODE,
        ),
        (
            "a published device ARN, which holds no tag set",
            device.clone(),
            404,
            NOT_FOUND_CODE,
        ),
        (
            "an ARN of a Braket resource kind nothing stores",
            format!("arn:aws:braket:{region}:{account}:reservation/ghost-6"),
            404,
            NOT_FOUND_CODE,
        ),
        (
            "an empty resource id",
            format!("arn:aws:braket:{region}:{account}:quantum-task/"),
            400,
            "ValidationException",
        ),
        (
            "a resource field with no type at all",
            format!("arn:aws:braket:{region}:{account}:quantum-task"),
            400,
            "ValidationException",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{region}:{account}:quantum-task/t1"),
            400,
            "ValidationException",
        ),
        (
            "an account that is not 12 digits",
            format!("arn:aws:braket:{region}:1234:quantum-task/t1"),
            400,
            "ValidationException",
        ),
        (
            "no ARN at all",
            "not-an-arn".to_string(),
            400,
            "ValidationException",
        ),
    ];

    for (case, named, status, code) in &rows {
        // Every door answers the same way: what the ARN resolves to is a property of the
        // resource, not of the call.
        let replies: Vec<(&str, Reply)> = vec![
            ("ListTagsForResource", list_tags(&braket, named).await),
            ("TagResource", tag_resource(&braket, named, written).await),
            (
                "UntagResource",
                untag_resource(&braket, named, &["team"]).await,
            ),
        ];
        for (door, reply) in replies {
            let full = format!("{door} / {case}");
            assert_eq!(
                reply.status, *status,
                "case {full:?}: expected {status}, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some(*code),
                "case {full:?}: expected {code}, got {:?} in {}",
                reply.error,
                reply.body
            );
            let sentence = if *status == 404 {
                RESOURCE_NOT_FOUND
            } else {
                INVALID_ARN
            };
            assert_eq!(
                reply.message(),
                sentence,
                "case {full:?}: expected the sentence this class is worded with"
            );
        }

        // The stored task is untouched by any of the refusals.
        let reported = reported_tags(&braket, &arn, case).await;
        assert_eq!(
            reported,
            expected(written),
            "case {case:?}: a refused tagging call changes nothing, got {reported:?}"
        );
    }

    // The control: the ARN the create minted does resolve, for every door.
    let served = list_tags(&braket, &arn).await;
    assert_eq!(
        served.status, 200,
        "the ARN the create minted resolves: got {} {}",
        served.status, served.body
    );
    let served = untag_resource(&braket, &arn, &["team"]).await;
    assert_eq!(
        served.status, 200,
        "the ARN the create minted resolves for the removal too: got {} {}",
        served.status, served.body
    );
    assert!(
        reported_tags(&braket, &arn, "the control").await.is_empty(),
        "the served removal emptied the set"
    );
}

/// `ListTagsForResource` reports, it does not write: reading a resource's tags over and
/// over leaves the set, and everything else about the resource, exactly as it was.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let braket = Braket::new();
    let device = braket.an_online_device().await;
    let created = braket
        .create_task(create_body("tok-task-reads", &device, 25))
        .await;
    assert_eq!(
        created.status, 201,
        "creating the quantum task: got {} {}",
        created.status, created.body
    );
    let arn = created.str_member("quantumTaskArn");
    let written: &[(&str, &str)] = &[("team", "quantum"), ("note", "")];
    assert_eq!(
        tag_resource(&braket, &arn, written).await.status,
        200,
        "tagging the task to read"
    );

    for attempt in 1..=5 {
        let reported = reported_tags(&braket, &arn, "a repeated read").await;
        assert_eq!(
            reported,
            expected(written),
            "read {attempt}: expected the same set every time, got {reported:?}"
        );
    }

    // Nor did the reads change the resource itself: its own door still reports the
    // members the create supplied.
    let read = braket.get_task(&arn).await;
    assert_eq!(
        read.status, 200,
        "GetQuantumTask after the reads: got {} {}",
        read.status, read.body
    );
    assert_eq!(
        member(&read.body, "quantumTaskArn"),
        arn,
        "the reads left the task's identity alone, got {}",
        read.body
    );
    assert_eq!(
        read.body.get("shots").and_then(Value::as_i64),
        Some(25),
        "the reads left the task's shots alone, got {}",
        read.body
    );
}

/// The tag set one resource carries is bounded by COUNT: a call that would take it past
/// the limit is refused and writes nothing, while a call that only overwrites a key the
/// resource already carries is served at the limit.
#[tokio::test]
async fn the_tag_set_a_resource_carries_is_bounded_by_count() {
    use service_com_amazonaws_braket_braket::state::MAX_TAGS_PER_RESOURCE;

    let braket = Braket::new();
    let device = braket.an_online_device().await;
    let created = braket
        .create_task(create_body("tok-task-bound", &device, 10))
        .await;
    assert_eq!(
        created.status, 201,
        "creating the quantum task: got {} {}",
        created.status, created.body
    );
    let arn = created.str_member("quantumTaskArn");

    // Fill the set to one short of the limit in one call, then reach the limit with the
    // next: how the keys are spread over calls does not change what the bound counts.
    let keys: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|n| (format!("key-{n}"), format!("value-{n}")))
        .collect();
    let pairs: Vec<(&str, &str)> = keys
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    let (all_but_last, last) = pairs.split_at(MAX_TAGS_PER_RESOURCE - 1);
    let reply = tag_resource(&braket, &arn, all_but_last).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected 200, got {} {}",
        "one short of the limit", reply.status, reply.body
    );
    let reply = tag_resource(&braket, &arn, last).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected 200, got {} {}",
        "the key that reaches the limit", reply.status, reply.body
    );
    let at_limit = reported_tags(&braket, &arn, "at the limit").await;
    assert_eq!(
        at_limit.len(),
        MAX_TAGS_PER_RESOURCE,
        "case {:?}: expected {MAX_TAGS_PER_RESOURCE} tags, got {}",
        "at the limit",
        at_limit.len()
    );

    // (case, the pairs the call submits, expected status) — one new key past the limit,
    // several new keys at once, a key the resource already carries (which does not grow
    // the set), and a batch mixing a carried key with a new one.
    struct Over<'p> {
        case: &'static str,
        submitted: Vec<(&'p str, &'p str)>,
        status: u16,
    }
    let over: Vec<Over<'_>> = vec![
        Over {
            case: "one new key past the limit",
            submitted: vec![("one-too-many", "x")],
            status: 400,
        },
        Over {
            case: "several new keys past the limit",
            submitted: vec![("a", "1"), ("b", "2")],
            status: 400,
        },
        Over {
            case: "a key the resource already carries",
            submitted: vec![(pairs[0].0, "rewritten")],
            status: 200,
        },
        Over {
            case: "a carried key beside a new one",
            submitted: vec![(pairs[1].0, "rewritten"), ("also-new", "y")],
            status: 400,
        },
    ];
    for Over {
        case,
        submitted,
        status,
    } in &over
    {
        let reply = tag_resource(&braket, &arn, submitted).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected {status}, got {} {}",
            reply.status, reply.body
        );
        if *status == 400 {
            assert_eq!(
                reply.error.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {:?} in {}",
                reply.error,
                reply.body
            );
            assert_eq!(
                reply.message(),
                format!("A resource cannot have more than {MAX_TAGS_PER_RESOURCE} tags"),
                "case {case:?}: the refusal states the limit"
            );
        }
        // A refused call writes nothing, and a served one at the limit only rewrote a
        // value: either way the set still holds exactly the limit's worth of keys.
        let reported = reported_tags(&braket, &arn, case).await;
        assert_eq!(
            reported.len(),
            MAX_TAGS_PER_RESOURCE,
            "case {case:?}: expected the set to still hold {MAX_TAGS_PER_RESOURCE} keys, \
             got {reported:?}"
        );
        assert!(
            reported.keys().all(|key| key.starts_with("key-")),
            "case {case:?}: no key from a refused call reached the set, got {reported:?}"
        );
    }

    // Room freed by a removal is room a new key can take: the bound is on the set's
    // size, not on how many calls have been made.
    let reply = untag_resource(&braket, &arn, &[pairs[0].0]).await;
    assert_eq!(
        reply.status, 200,
        "freeing one slot: expected 200, got {} {}",
        reply.status, reply.body
    );
    let reply = tag_resource(&braket, &arn, &[("after-the-removal", "z")]).await;
    assert_eq!(
        reply.status, 200,
        "the key that takes the freed slot: expected 200, got {} {}",
        reply.status, reply.body
    );
    let reported = reported_tags(&braket, &arn, "after the removal").await;
    assert_eq!(
        reported.get("after-the-removal").map(String::as_str),
        Some("z"),
        "the freed slot took the new key, got {reported:?}"
    );
    assert_eq!(
        reported.len(),
        MAX_TAGS_PER_RESOURCE,
        "the set is full again, got {reported:?}"
    );
}
