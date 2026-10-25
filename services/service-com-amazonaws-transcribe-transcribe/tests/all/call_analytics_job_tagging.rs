// SPDX-License-Identifier: CC0-1.0

//! The Call Analytics JOB family through the standard tagging API, end to end over the
//! wire.
//!
//! The rule these tests pin: a taggable resource of this service is addressed by the
//! ARN `<type>` segment the service publishes for its family together with the
//! `<name>` of one record of it, so `ListTagsForResource` reports that record's CURRENT
//! tag set, `TagResource` merges into it and `UntagResource` removes from it — a tag
//! set a create stored is never write-only, and the family's own read reports the same
//! set the tagging API does. A `<type>` segment the service publishes for no family
//! names nothing, however much it resembles one that does.
//!
//! The rows differ along what the rule generalises over: how many tags a call carries
//! (none, one, several, the modelled maximum, and one past each published bound), the
//! shape of a tag value (including the empty string the modelled `TagValue` admits),
//! whether the tags were supplied at start time or added afterwards, and which
//! `<type>` segment the ARN carries.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) rather than standing up a second emulator, so these
//! tests exercise the awsJson1_1 envelope and the boundary constraint validation too.

use serde_json::{json, Value};

use service_com_amazonaws_transcribe_transcribe::state::DEFAULT_REGION;

use crate::tagging_resource_families::pairs;
use crate::transcription_jobs::Transcribe;

/// The modelled client error this service answers for an ARN that names no stored
/// resource, and the status its awsJson1_1 wire protocol puts on every client error.
const ABSENT_CODE: &str = "BadRequestException";
/// The refusal the boundary validator answers for a request outside a modelled
/// constraint, before any handler runs.
const VALIDATION_CODE: &str = "ValidationException";
/// See [`ABSENT_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// The ARN resource type this service publishes for a Call Analytics job:
/// `arn:${Partition}:transcribe:${Region}:${Account}:analytics-job/${JobName}`.
const ANALYTICS_JOB_TYPE: &str = "analytics-job";

/// A valid `DataAccessRoleArn`, whose `@pattern` pins it to an IAM role ARN
/// (`smithy_query(query="operation", operation="StartCallAnalyticsJob")`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/TranscribeAccess";

/// The most tags one `TagList` may carry, from that shape's own
/// `@length(min: 1, max: 200)` (`smithy_query(query="shape", shape="TagList")`).
const MAX_TAGS: usize = 200;

/// `tags` as the modelled `TagList` reaches the wire.
fn tags_json(tags: &[(String, String)]) -> Value {
    tags.iter()
        .map(|(key, value)| json!({"Key": key, "Value": value}))
        .collect()
}

/// A `(key, value)` table as owned pairs, sorted by key: a comparison is about the SET
/// a resource holds, not the order a merge left it in.
fn owned(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    sorted(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    )
}

/// The same pairs, sorted.
fn sorted(mut tags: Vec<(String, String)>) -> Vec<(String, String)> {
    tags.sort();
    tags
}

/// `count` distinct tags, for the rows that test the count axis at its bounds.
fn generated(count: usize) -> Vec<(String, String)> {
    sorted(
        (0..count)
            .map(|index| (format!("key-{index:03}"), format!("value-{index}")))
            .collect(),
    )
}

/// A `StartCallAnalyticsJob` request for `name`, carrying `tags` when the row supplies
/// a `Tags` member at all.
fn start_request(name: &str, tags: Option<&[(String, String)]>) -> Value {
    let mut request = json!({
        "CallAnalyticsJobName": name,
        "Media": {"MediaFileUri": "s3://example-media/call.wav"},
        "DataAccessRoleArn": ROLE_ARN
    });
    if let Some(tags) = tags {
        request["Tags"] = tags_json(tags);
    }
    request
}

/// The ARN this service hands out for the resource `resource_type` names `name` in the
/// Region a request with no credential scope is served in — built through the
/// framework's own ARN helper, so no test names a partition or an account literal.
fn arn_of(resource_type: &str, name: &str) -> String {
    engine::emulator_arn(
        "transcribe",
        DEFAULT_REGION,
        &format!("{resource_type}/{name}"),
    )
}

/// `ListTagsForResource` for `arn`, asserting it answered and echoed the ARN asked
/// about, and returning the tag set it reported.
async fn listed(transcribe: &Transcribe, arn: &str, case: &str) -> Vec<(String, String)> {
    let reply = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": arn}))
        .await;
    let body = reply.ok(case);
    assert_eq!(
        body.get("ResourceArn").and_then(Value::as_str),
        Some(arn),
        "case {case:?}: the answer must name the resource asked about, got {body:?}"
    );
    let tags = body
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags member, got {body:?}"));
    pairs(tags, case)
}

/// The tag set `GetCallAnalyticsJob` reports for the job named `name` — the family's
/// OWN view of the same stored set, empty where it leaves `Tags` out (which is how a
/// job carrying none reaches the wire: `TagList` is `@length(min: 1, max: 200)`, so an
/// empty list is not a value of it).
async fn job_tags(transcribe: &Transcribe, name: &str, case: &str) -> Vec<(String, String)> {
    let reply = transcribe
        .call("GetCallAnalyticsJob", json!({"CallAnalyticsJobName": name}))
        .await;
    let body = reply.ok(case);
    let job = body.get("CallAnalyticsJob").unwrap_or_else(|| {
        panic!("case {case:?}: expected a CallAnalyticsJob, got {body:?}")
    });
    job.get("Tags")
        .filter(|tags| !tags.is_null())
        .map_or_else(Vec::new, |tags| pairs(tags, case))
}

/// One row of [`start_time_tags`]: the case, and the `Tags` member the start carries
/// (`None` for a start that carries none at all).
type StartRow = (&'static str, Option<Vec<(String, String)>>);

/// The tag sets a start may store, each with the set the tagging API must then report.
fn start_time_tags() -> Vec<StartRow> {
    vec![
        ("no Tags member at all", None),
        ("one tag", Some(owned(&[("team", "speech")]))),
        (
            "three tags",
            Some(owned(&[("team", "speech"), ("env", "test"), ("cost", "7")])),
        ),
        // `TagValue` is `@length(min: 0, max: 256)`, so the empty string is a value of
        // it (`smithy_query(query="shape", shape="TagValue")`).
        ("a tag whose value is the empty string", Some(owned(&[("env", "")]))),
        ("the modelled maximum of tags", Some(generated(MAX_TAGS))),
    ]
}

/// RULE: the tag set a `StartCallAnalyticsJob` stored is what `ListTagsForResource`
/// reports for that job's ARN — and the same set the job's own read reports.
#[tokio::test]
async fn the_tag_set_a_start_stored_is_what_the_tagging_api_reports() {
    let transcribe = Transcribe::new();

    for (index, (case, tags)) in start_time_tags().into_iter().enumerate() {
        let name = format!("started{index}-analytics-job");
        transcribe
            .call(
                "StartCallAnalyticsJob",
                start_request(&name, tags.as_deref()),
            )
            .await
            .ok(case);

        let expected = tags.unwrap_or_default();
        let arn = arn_of(ANALYTICS_JOB_TYPE, &name);
        let found = listed(&transcribe, &arn, case).await;
        assert_eq!(
            found, expected,
            "case {case:?}: the tagging API must report the tags the start stored, got \
             {found:?}"
        );
        let reported = job_tags(&transcribe, &name, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: GetCallAnalyticsJob must report the same stored set, got \
             {reported:?}"
        );
    }
}

/// RULE: `TagResource` merges into the job's stored set — a key it does not carry is
/// added, a key it does carry takes the new value — and `UntagResource` removes exactly
/// the named keys; both are observable through `ListTagsForResource` and through the
/// job's own read.
#[tokio::test]
async fn tagging_a_job_after_the_start_reaches_its_stored_set() {
    let transcribe = Transcribe::new();
    let case = "tags written after the start";
    let name = "merged-analytics-job";
    transcribe
        .call(
            "StartCallAnalyticsJob",
            start_request(name, Some(&owned(&[("team", "speech"), ("env", "test")]))),
        )
        .await
        .ok(case);
    let arn = arn_of(ANALYTICS_JOB_TYPE, name);

    // One added key and one key already on the job, whose value the merge replaces
    // rather than storing twice.
    transcribe
        .call(
            "TagResource",
            json!({
                "ResourceArn": arn,
                "Tags": tags_json(&owned(&[("env", "prod"), ("cost", "7")]))
            }),
        )
        .await
        .ok(case);
    let merged = owned(&[("team", "speech"), ("env", "prod"), ("cost", "7")]);
    for (reader, found) in [
        ("ListTagsForResource", listed(&transcribe, &arn, case).await),
        ("GetCallAnalyticsJob", job_tags(&transcribe, name, case).await),
    ] {
        assert_eq!(
            found, merged,
            "case {case:?}: {reader} must report the merged set, got {found:?}"
        );
    }

    // A second merge of the same pairs is idempotent: one value per key.
    transcribe
        .call(
            "TagResource",
            json!({"ResourceArn": arn, "Tags": tags_json(&merged)}),
        )
        .await
        .ok(case);
    let found = listed(&transcribe, &arn, case).await;
    assert_eq!(
        found, merged,
        "case {case:?}: re-tagging with the stored pairs must change nothing, got \
         {found:?}"
    );

    // One key the job carries and two it does not: the outcome is "these keys are not
    // on the job", already true for the absent ones.
    transcribe
        .call(
            "UntagResource",
            json!({
                "ResourceArn": arn,
                "TagKeys": ["env", "not-a-key", "another-absent-key"]
            }),
        )
        .await
        .ok_status(case, 200);
    let kept = owned(&[("team", "speech"), ("cost", "7")]);
    for (reader, found) in [
        ("ListTagsForResource", listed(&transcribe, &arn, case).await),
        ("GetCallAnalyticsJob", job_tags(&transcribe, name, case).await),
    ] {
        assert_eq!(
            found, kept,
            "case {case:?}: {reader} must report only the keys the untag kept, got \
             {found:?}"
        );
    }

    // Removing the rest empties the set, and the emptied set is still the job's CURRENT
    // set rather than a constant: a tag added afterwards is reported again.
    transcribe
        .call(
            "UntagResource",
            json!({"ResourceArn": arn, "TagKeys": ["team", "cost"]}),
        )
        .await
        .ok_status(case, 200);
    let found = listed(&transcribe, &arn, case).await;
    assert_eq!(
        found,
        Vec::<(String, String)>::new(),
        "case {case:?}: a job whose last tag was removed must report an empty set, got \
         {found:?}"
    );
    transcribe
        .call(
            "TagResource",
            json!({"ResourceArn": arn, "Tags": tags_json(&owned(&[("re-tagged", "yes")]))}),
        )
        .await
        .ok(case);
    let found = listed(&transcribe, &arn, case).await;
    assert_eq!(
        found,
        owned(&[("re-tagged", "yes")]),
        "case {case:?}: a tag added after the set emptied must be reported, got {found:?}"
    );
}

/// RULE: `TagList` is `@length(min: 1, max: 200)`, so a `Tags` list outside that bound
/// is refused by the boundary validator — at the start and at `TagResource` alike — and
/// a refused call stores nothing.
#[tokio::test]
async fn a_tag_list_outside_the_modelled_bound_is_refused() {
    let transcribe = Transcribe::new();

    // One past each published bound, at the start.
    for (case, tags) in [
        ("an empty Tags list at the start", generated(0)),
        ("one tag past the maximum at the start", generated(MAX_TAGS + 1)),
    ] {
        let name = "refused-analytics-job";
        transcribe
            .call("StartCallAnalyticsJob", start_request(name, Some(&tags)))
            .await
            .refused(case, CLIENT_ERROR_STATUS, VALIDATION_CODE);
        // The refused start recorded no job, so its ARN still names nothing.
        transcribe
            .call(
                "ListTagsForResource",
                json!({"ResourceArn": arn_of(ANALYTICS_JOB_TYPE, name)}),
            )
            .await
            .refused(case, CLIENT_ERROR_STATUS, ABSENT_CODE);
    }

    // And at `TagResource`, against a job that does exist: the bound is the shape's,
    // not the start's.
    let name = "bounded-analytics-job";
    transcribe
        .call("StartCallAnalyticsJob", start_request(name, None))
        .await
        .ok("a job to tag past the bound");
    let arn = arn_of(ANALYTICS_JOB_TYPE, name);
    for (case, tags) in [
        ("an empty Tags list at TagResource", generated(0)),
        (
            "one tag past the maximum at TagResource",
            generated(MAX_TAGS + 1),
        ),
    ] {
        transcribe
            .call(
                "TagResource",
                json!({"ResourceArn": arn, "Tags": tags_json(&tags)}),
            )
            .await
            .refused(case, CLIENT_ERROR_STATUS, VALIDATION_CODE);
        let found = listed(&transcribe, &arn, case).await;
        assert_eq!(
            found,
            Vec::<(String, String)>::new(),
            "case {case:?}: a refused TagResource must store nothing, got {found:?}"
        );
    }
}

/// RULE, the other side: the ARN's `<type>` segment selects the family, so the tags of
/// a job and of a same-named resource of another family are independent sets, and a
/// `<type>` this service publishes for no family — however much it resembles one it
/// does — names nothing at all. A `<name>` no job was started under names nothing
/// either, and the refusal words the KIND the type segment denotes.
#[tokio::test]
async fn only_the_published_type_segment_reaches_a_job_s_tags() {
    let transcribe = Transcribe::new();
    let shared = "shared.name";
    let case = "one name across two families";

    transcribe
        .call("StartCallAnalyticsJob", start_request(shared, None))
        .await
        .ok(case);
    transcribe
        .call(
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": shared,
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.wav"}
            }),
        )
        .await
        .ok(case);

    // Each family's ARN is tagged with a key naming that family, so a leak between the
    // two cannot pass unnoticed.
    for resource_type in [ANALYTICS_JOB_TYPE, "transcription-job"] {
        transcribe
            .call(
                "TagResource",
                json!({
                    "ResourceArn": arn_of(resource_type, shared),
                    "Tags": tags_json(&owned(&[("family", resource_type)]))
                }),
            )
            .await
            .ok(case);
    }
    for resource_type in [ANALYTICS_JOB_TYPE, "transcription-job"] {
        let case = format!("{case} / {resource_type}");
        let found = listed(&transcribe, &arn_of(resource_type, shared), &case).await;
        assert_eq!(
            found,
            owned(&[("family", resource_type)]),
            "case {case:?}: a tagging call must reach only its own family's record, got \
             {found:?}"
        );
    }

    // A `<name>` no job of this family was started under: the refusal names the kind
    // the type segment denotes, which for every job family of this service is "job".
    let absent = tagging_calls(&arn_of(ANALYTICS_JOB_TYPE, "never-started"));
    for (operation, request) in absent {
        let case = format!("{operation} / a job name nothing was started under");
        let reply = transcribe.call(operation, request).await;
        reply.refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);
        let message = reply.error_message(&case);
        assert_eq!(
            message,
            "The requested job couldn't be found. Check the job name and try your \
             request again.",
            "case {case:?}: the refusal must word the kind the type segment denotes, \
             got {message:?}"
        );
    }

    // Spellings the `TranscribeArn` pattern's `<type>` segment admits that this service
    // publishes for no family: each names nothing, even though a job of the shared name
    // exists under the published spelling.
    for resource_type in [
        "analytics-jobs",
        "analyticsjob",
        "call-analytics-job",
        "analytics",
    ] {
        for (operation, request) in tagging_calls(&arn_of(resource_type, shared)) {
            let case = format!("{operation} / {resource_type} naming no family");
            let reply = transcribe.call(operation, request).await;
            reply.refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);
            let message = reply.error_message(&case);
            assert_eq!(
                message,
                "The requested resource couldn't be found. Check the resource name and \
                 try your request again.",
                "case {case:?}: a type segment naming no family must be refused \
                 neutrally, got {message:?}"
            );
        }
    }

    // None of those refusals touched the stored set the published ARN reaches.
    let found = listed(&transcribe, &arn_of(ANALYTICS_JOB_TYPE, shared), case).await;
    assert_eq!(
        found,
        owned(&[("family", ANALYTICS_JOB_TYPE)]),
        "case {case:?}: a refused tagging call must leave the stored set alone, got \
         {found:?}"
    );
}

/// All three tagging operations against one ARN.
fn tagging_calls(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("ListTagsForResource", json!({"ResourceArn": arn})),
        (
            "TagResource",
            json!({"ResourceArn": arn, "Tags": [{"Key": "team", "Value": "speech"}]}),
        ),
        (
            "UntagResource",
            json!({"ResourceArn": arn, "TagKeys": ["team"]}),
        ),
    ]
}
