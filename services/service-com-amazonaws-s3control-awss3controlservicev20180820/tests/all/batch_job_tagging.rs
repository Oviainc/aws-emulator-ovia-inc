// SPDX-License-Identifier: CC0-1.0

//! The tagging sub-resource of an S3 Batch Operations job, driven end to end
//! over HTTP.
//!
//! The rule under test: a job's tag set is ONE sub-resource of the job, keyed
//! by the account and the job id. `GetJobTagging` reports exactly the set last
//! written to it — the set `CreateJob` recorded until a write replaces it, and
//! the empty set for a job that carries none; `PutJobTagging` replaces the
//! WHOLE set, so a key the request omits is gone rather than merged forward and
//! an empty submitted set leaves no tags; `DeleteJobTagging` empties it. None of
//! the three changes anything else about the job, each names a job the request's
//! own account owns, and a request that submits a set the service does not store
//! — more than the 50 tags per job AWS documents, or one key twice — is refused
//! with the stored set left alone.
//!
//! Every row below differs along something that rule generalises over: how many
//! tags the set carries (none, one, several, and the maximum a job admits),
//! the order the keys are submitted in, the length of a key and a value
//! (including each published bound of `TagKeyString` and `TagValueString` and
//! one past each), which character classes the modelled tag pattern admits,
//! whether the set replaces one that overlaps it, is disjoint from it or is a
//! subset of it, where the set under test came from (a create or a put), and
//! which account owns the job.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes
//! it per the Smithy model, runs the handler and serializes the restXml
//! response. The router, the request builder, the XML escaping and the job
//! create and describe helpers are the suites' beside this one, reused rather
//! than copied.

use std::time::Duration;

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    JOB_PREPARATION, MAX_JOB_TAGS,
};

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, xml_escape, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, XMLNS,
};
use crate::batch_job_lifecycle::{
    create_job, describe_job, JobCase, ACTION_LAMBDA_INVOKE, ACTION_PUT_OBJECT_COPY,
    ACTION_PUT_OBJECT_TAGGING, GENERATED_MANIFEST, MANIFEST_CSV, MANIFEST_INVENTORY,
};
// The reader of a reported tag set is the Storage Lens suite's, which matches a
// leaf by its path's last segment and so reads an `S3TagSet`'s `<member>`
// wrapping too: one parser for both tagging surfaces.
use crate::storage_lens_configuration::reported_tags;

/// The published bounds of a tag key and a tag value
/// (`smithy_query(query="shape",shape="TagKeyString,TagValueString")`:
/// `TagKeyString` len 1..128, `TagValueString` len 0..256).
const KEY_MAX_LENGTH: usize = 128;
const VALUE_MAX_LENGTH: usize = 256;

/// One tag set, owned so a row can be built from a published bound rather than
/// written out by hand.
type TagSet = Vec<(String, String)>;

/// One row: the case name and the tag set it submits.
type TagSetRow = (&'static str, TagSet);

/// One row of a refusal table: the case name, the tag set it submits and the
/// error code the service answers it with.
type RefusalRow = (&'static str, TagSet, &'static str);

/// A tag pair.
fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// `count` distinct tags, for the rows that span how many tags a set carries.
fn numbered_tags(count: usize) -> TagSet {
    (0..count)
        .map(|index| tag(&format!("key-{index:02}"), &format!("value-{index:02}")))
        .collect()
}

/// The `<Tags>` element a `PutJobTagging` body carries: `S3TagSet` is a plain
/// Smithy list, so each tag is one `<member>`.
fn tag_set_xml(tags: &[(String, String)]) -> String {
    let mut xml = String::from("<Tags>");
    for (key, value) in tags {
        xml.push_str(&format!(
            "<member><Key>{}</Key><Value>{}</Value></member>",
            xml_escape(key),
            xml_escape(value)
        ));
    }
    xml.push_str("</Tags>");
    xml
}

async fn put_job_tagging(
    emulator: &Emulator,
    account_id: &str,
    job_id: &str,
    tags: &[(String, String)],
) -> Reply {
    let body = format!(
        "<PutJobTaggingRequest xmlns=\"{XMLNS}\">{}</PutJobTaggingRequest>",
        tag_set_xml(tags)
    );
    emulator
        .call(
            "PUT",
            &format!("/v20180820/jobs/{job_id}/tagging"),
            account_id,
            &body,
        )
        .await
}

async fn get_job_tagging(emulator: &Emulator, account_id: &str, job_id: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v20180820/jobs/{job_id}/tagging"),
            account_id,
            "",
        )
        .await
}

async fn delete_job_tagging(emulator: &Emulator, account_id: &str, job_id: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/v20180820/jobs/{job_id}/tagging"),
            account_id,
            "",
        )
        .await
}

/// The tag set `GetJobTagging` reports for `job_id`, asserting the read itself
/// succeeded.
async fn stored_tags(
    emulator: &Emulator,
    account_id: &str,
    job_id: &str,
    case: &str,
) -> TagSet {
    let reply = get_job_tagging(emulator, account_id, job_id).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected GetJobTagging of {job_id:?} to answer 200, got {} with body {}",
        reply.status, reply.body
    );
    reported_tags(&reply.body)
}

/// Create one job from `case` and answer the job id the service assigned, with
/// the clock's own `New` -> `Ready` transition already made due, so a later
/// `DescribeJob` comparison is not one the clock was about to change under it.
async fn create_prepared_job(
    emulator: &Emulator,
    account_id: &str,
    case: &JobCase,
    token: &str,
) -> String {
    let created = create_job(emulator, account_id, case, token).await;
    assert_eq!(
        created.status, 200,
        "case {token:?}: expected CreateJob to answer 200, got {} with body {}",
        created.status, created.body
    );
    let job_id = created
        .text("JobId")
        .unwrap_or_else(|| panic!("case {token:?}: CreateJob answers a JobId: {}", created.body));
    tokio::time::sleep(JOB_PREPARATION + Duration::from_millis(50)).await;
    job_id
}

/// The create shapes the rows below are run over: the tag set is a
/// sub-resource of the job whatever action the job runs and wherever its object
/// list comes from.
fn job_shapes() -> Vec<JobCase> {
    vec![
        JobCase::new(
            "copy from a CSV manifest",
            ACTION_PUT_OBJECT_COPY,
            MANIFEST_CSV,
        ),
        JobCase::new(
            "tagging from an inventory manifest",
            ACTION_PUT_OBJECT_TAGGING,
            MANIFEST_INVENTORY,
        ),
        JobCase::new(
            "lambda invoke from a generated manifest",
            ACTION_LAMBDA_INVOKE,
            GENERATED_MANIFEST,
        ),
    ]
}

/// The tag sets a put submits. No row is merely a rename of another: each
/// differs in how many tags it carries, how long its key or value is, which
/// characters it uses or the order it submits its keys in.
fn tag_set_rows() -> Vec<TagSetRow> {
    vec![
        (
            "the empty set, which leaves the job with no tags",
            Vec::new(),
        ),
        ("one tag", vec![tag("owner", "data-platform")]),
        (
            "two tags, submitted with their keys out of alphabetical order",
            vec![tag("stage", "prod"), tag("owner", "analytics")],
        ),
        (
            "a value at the length minimum the model admits",
            vec![tag("owner", "")],
        ),
        (
            "the longest key and value the model admits",
            vec![tag(
                &"k".repeat(KEY_MAX_LENGTH),
                &"v".repeat(VALUE_MAX_LENGTH),
            )],
        ),
        (
            // Every class the modelled tag pattern admits
            // (^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$): letters, separators, numbers
            // and the punctuation it lists.
            "keys and values over every character class the modelled pattern admits",
            vec![
                tag("cost.center/team:east=1+2-3_4@aws", "ops team 42"),
                tag("Ünïcødé", "vàlüé wîth spaces"),
            ],
        ),
        (
            "two keys that differ only in case, which are two tags",
            vec![tag("Owner", "upper"), tag("owner", "lower")],
        ),
        (
            "the most tags a job admits",
            numbered_tags(MAX_JOB_TAGS),
        ),
    ]
}

#[tokio::test]
async fn a_put_replaces_the_whole_tag_set_and_a_get_reports_exactly_what_was_stored() {
    let emulator = Emulator::start();

    for (shape_row, shape) in job_shapes().into_iter().enumerate() {
        for (row, (case, tags)) in tag_set_rows().into_iter().enumerate() {
            let token = format!("put-{shape_row}-{row}");
            let job_id = create_prepared_job(&emulator, ACCOUNT, &shape, &token).await;

            // The job exactly as it reads before the tag set is written, so
            // what the write changed about the job itself can be compared.
            let before = describe_job(&emulator, ACCOUNT, &job_id).await;

            let put = put_job_tagging(&emulator, ACCOUNT, &job_id, &tags).await;
            assert_eq!(
                put.status, 200,
                "case {case:?}: expected PutJobTagging to answer 200, got {} with body {}",
                put.status, put.body
            );

            let reported = stored_tags(&emulator, ACCOUNT, &job_id, case).await;
            assert_eq!(
                reported, tags,
                "case {case:?}: expected GetJobTagging to report the submitted set, got \
                 {reported:?}"
            );

            // The job itself is the job the create recorded: `JobDescriptor`
            // declares no tag member, so the described document is unchanged.
            let after = describe_job(&emulator, ACCOUNT, &job_id).await;
            assert_eq!(
                after.body, before.body,
                "case {case:?}: expected the tag write to change nothing else about the job, \
                 got {} from {}",
                after.body, before.body
            );
        }
    }
}

#[tokio::test]
async fn a_get_reports_the_set_the_create_recorded_until_a_put_replaces_it() {
    let emulator = Emulator::start();

    // A job created with no tag set carries none: the empty set, not a refusal.
    let untagged = create_prepared_job(
        &emulator,
        ACCOUNT,
        &JobCase::new("copy from a CSV manifest", ACTION_PUT_OBJECT_COPY, MANIFEST_CSV),
        "created-untagged",
    )
    .await;
    let reported = stored_tags(&emulator, ACCOUNT, &untagged, "a job nobody tagged").await;
    assert!(
        reported.is_empty(),
        "case {:?}: expected the empty set for a job created with no tags, got {reported:?}",
        "a job nobody tagged"
    );

    // A job created WITH a tag set carries exactly that set before any put:
    // the create and the tagging operations share one stored set.
    let created_tags: &'static [(&'static str, &'static str)] =
        &[("owner", "data-platform"), ("stage", "prod")];
    let tagged = create_prepared_job(
        &emulator,
        ACCOUNT,
        &JobCase::new(
            "tagging from a CSV manifest",
            ACTION_PUT_OBJECT_TAGGING,
            MANIFEST_CSV,
        )
        .with_tags(created_tags),
        "created-tagged",
    )
    .await;
    let reported = stored_tags(&emulator, ACCOUNT, &tagged, "a job created tagged").await;
    assert_eq!(
        reported,
        created_tags
            .iter()
            .map(|(key, value)| tag(key, value))
            .collect::<Vec<_>>(),
        "case {:?}: expected the set the create recorded, got {reported:?}",
        "a job created tagged"
    );

    // Then each put replaces the whole set, however it overlaps the set
    // already stored: the last write wins and no earlier key survives it.
    let replacements: [TagSetRow; 5] = [
        (
            "a set disjoint from the one stored",
            vec![tag("team", "search"), tag("tier", "gold")],
        ),
        (
            "a set overlapping the one stored in one key",
            vec![tag("team", "indexing"), tag("owner", "platform")],
        ),
        ("a subset of the one stored", vec![tag("team", "indexing")]),
        ("a larger set", numbered_tags(7)),
        (
            "the empty set, which deletes the existing tags",
            Vec::new(),
        ),
    ];
    for (case, tags) in replacements {
        let put = put_job_tagging(&emulator, ACCOUNT, &tagged, &tags).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected PutJobTagging to answer 200, got {} with body {}",
            put.status, put.body
        );
        let reported = stored_tags(&emulator, ACCOUNT, &tagged, case).await;
        assert_eq!(
            reported, tags,
            "case {case:?}: expected the last put to win, got {reported:?}"
        );
    }

    // And the job nobody tagged still carries nothing: one job's tag set is
    // not another's.
    let reported = stored_tags(&emulator, ACCOUNT, &untagged, "the job nobody tagged").await;
    assert!(
        reported.is_empty(),
        "case {:?}: expected the untagged job to keep the empty set, got {reported:?}",
        "the job nobody tagged"
    );
}

#[tokio::test]
async fn a_delete_empties_the_whole_set_and_leaves_the_job_otherwise_alone() {
    let emulator = Emulator::start();

    for (row, (case, tags)) in tag_set_rows().into_iter().enumerate() {
        let shape = JobCase::new(
            "lambda invoke from a generated manifest",
            ACTION_LAMBDA_INVOKE,
            GENERATED_MANIFEST,
        );
        let job_id = create_prepared_job(&emulator, ACCOUNT, &shape, &format!("delete-{row}")).await;
        let put = put_job_tagging(&emulator, ACCOUNT, &job_id, &tags).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected PutJobTagging to answer 200, got {} with body {}",
            put.status, put.body
        );

        let before = describe_job(&emulator, ACCOUNT, &job_id).await;

        // The whole set goes, whatever it held — including a set that was
        // already empty, which is the state the caller asked for.
        for pass in ["the first delete", "a repeated delete"] {
            let deleted = delete_job_tagging(&emulator, ACCOUNT, &job_id).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?} / {pass}: expected DeleteJobTagging to answer 200, got {} with \
                 body {}",
                deleted.status, deleted.body
            );
            let reported = stored_tags(&emulator, ACCOUNT, &job_id, case).await;
            assert!(
                reported.is_empty(),
                "case {case:?} / {pass}: expected the empty set after a delete, got {reported:?}"
            );
        }

        let after = describe_job(&emulator, ACCOUNT, &job_id).await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: expected the delete to change nothing else about the job, got {} \
             from {}",
            after.body, before.body
        );

        // The sub-resource survives its own deletion: a later put stores a new
        // set on the same job.
        let again = vec![tag("after", "delete")];
        let put = put_job_tagging(&emulator, ACCOUNT, &job_id, &again).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected a put after a delete to answer 200, got {} with body {}",
            put.status, put.body
        );
        let reported = stored_tags(&emulator, ACCOUNT, &job_id, case).await;
        assert_eq!(
            reported, again,
            "case {case:?}: expected the put after the delete to be stored, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_tag_set_is_reached_only_through_the_account_and_job_id_that_own_it() {
    let emulator = Emulator::start();

    let shape = JobCase::new("copy from a CSV manifest", ACTION_PUT_OBJECT_COPY, MANIFEST_CSV);
    let mine = create_prepared_job(&emulator, ACCOUNT, &shape, "scope-mine").await;
    let second = create_prepared_job(&emulator, ACCOUNT, &shape, "scope-second").await;

    let mine_tags = vec![tag("owner", "mine")];
    let second_tags = vec![tag("owner", "second"), tag("stage", "dev")];
    for (job_id, tags) in [(&mine, &mine_tags), (&second, &second_tags)] {
        let put = put_job_tagging(&emulator, ACCOUNT, job_id, tags).await;
        assert_eq!(
            put.status, 200,
            "case {job_id:?}: expected PutJobTagging to answer 200, got {} with body {}",
            put.status, put.body
        );
    }

    // Each job reads its own set, never the other's.
    for (case, job_id, expected) in [
        ("the first job", &mine, &mine_tags),
        ("the second job", &second, &second_tags),
    ] {
        let reported = stored_tags(&emulator, ACCOUNT, job_id, case).await;
        assert_eq!(
            reported, *expected,
            "case {case:?}: expected each job to keep its own set, got {reported:?}"
        );
    }

    // A well-formed `JobId` (5..36 of ^[a-zA-Z0-9\-\_]+$) the named account
    // owns no job for is refused by all three operations.
    let unknown_ids = [
        (
            "a well-formed id no job has",
            "00000000-0000-0000-0000-000000".to_string(),
        ),
        ("the shortest id the model admits", "aaaaa".to_string()),
        (
            "an id with the underscores the model admits",
            "job_id_000".to_string(),
        ),
    ];
    for (case, unknown) in unknown_ids {
        let refusals = [
            ("GetJobTagging", get_job_tagging(&emulator, ACCOUNT, &unknown).await),
            (
                "PutJobTagging",
                put_job_tagging(&emulator, ACCOUNT, &unknown, &[tag("owner", "nobody")]).await,
            ),
            (
                "DeleteJobTagging",
                delete_job_tagging(&emulator, ACCOUNT, &unknown).await,
            ),
        ];
        for (operation, refused) in refusals {
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected {operation} of {unknown:?} to be refused 400, got {} \
                 with body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.code(),
                "InvalidRequest",
                "case {case:?}: expected {operation} of {unknown:?} to answer InvalidRequest, \
                 got body {}",
                refused.body
            );
        }
    }

    // The same three operations addressed in another account are refused ahead
    // of the job lookup — for a job id this account DOES own as much as for one
    // nothing owns — so a tag set is never reached across the account boundary.
    for (case, job_id) in [
        ("a job id this account owns", mine.clone()),
        ("an id no job has", "00000000-0000-0000-0000-000000".to_string()),
    ] {
        for (operation, refused) in [
            (
                "GetJobTagging",
                get_job_tagging(&emulator, OTHER_ACCOUNT, &job_id).await,
            ),
            (
                "PutJobTagging",
                put_job_tagging(&emulator, OTHER_ACCOUNT, &job_id, &[tag("owner", "nobody")]).await,
            ),
            (
                "DeleteJobTagging",
                delete_job_tagging(&emulator, OTHER_ACCOUNT, &job_id).await,
            ),
        ] {
            assert_foreign_account_refused(
                &format!("{operation} for another account, {case}"),
                OTHER_ACCOUNT,
                &refused,
            );
        }
    }

    // And nothing those refusals named was written: both jobs keep their sets.
    for (case, job_id, expected) in [
        ("the first job", &mine, &mine_tags),
        ("the second job", &second, &second_tags),
    ] {
        let reported = stored_tags(&emulator, ACCOUNT, job_id, case).await;
        assert_eq!(
            reported, *expected,
            "case {case:?}: expected a refused request to store nothing, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_set_past_the_published_bounds_is_refused_and_leaves_the_stored_set_alone() {
    let emulator = Emulator::start();

    let shape = JobCase::new(
        "tagging from an inventory manifest",
        ACTION_PUT_OBJECT_TAGGING,
        MANIFEST_INVENTORY,
    );
    let job_id = create_prepared_job(&emulator, ACCOUNT, &shape, "bounds").await;

    // The bound itself is admitted: AWS documents a maximum of 50 tags per job.
    let at_bound = numbered_tags(MAX_JOB_TAGS);
    let put = put_job_tagging(&emulator, ACCOUNT, &job_id, &at_bound).await;
    assert_eq!(
        put.status, 200,
        "expected a set of {MAX_JOB_TAGS} tags to be stored, got {} with body {}",
        put.status, put.body
    );

    // One past the documented maximum, and a key named twice, are sets the
    // service does not store: each is refused with its own code.
    let mut repeated = numbered_tags(3);
    repeated.push(tag("key-01", "a second value for one key"));
    let refusals: [RefusalRow; 4] = [
        (
            "one tag past the maximum a job admits",
            numbered_tags(MAX_JOB_TAGS + 1),
            "TooManyTagsException",
        ),
        (
            "far past the maximum a job admits",
            numbered_tags(MAX_JOB_TAGS * 2),
            "TooManyTagsException",
        ),
        (
            "the same key twice",
            vec![tag("owner", "first"), tag("owner", "second")],
            "InvalidRequest",
        ),
        ("a key repeated later in the set", repeated, "InvalidRequest"),
    ];
    for (case, tags, code) in refusals {
        let refused = put_job_tagging(&emulator, ACCOUNT, &job_id, &tags).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected a refusal 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            code,
            "case {case:?}: expected {code}, got body {}",
            refused.body
        );
    }

    // One past each published length bound is refused by the boundary
    // validator, before the handler runs, so only the status is this suite's
    // to pin.
    for (case, tags) in [
        (
            "a key one character past its maximum",
            vec![tag(&"k".repeat(KEY_MAX_LENGTH + 1), "value")],
        ),
        (
            "a key one character below its minimum",
            vec![tag("", "value")],
        ),
        (
            "a value one character past its maximum",
            vec![tag("owner", &"v".repeat(VALUE_MAX_LENGTH + 1))],
        ),
    ] {
        let refused = put_job_tagging(&emulator, ACCOUNT, &job_id, &tags).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected a tag outside the modelled lengths to be refused 400, got \
             {} with body {}",
            refused.status, refused.body
        );
    }

    // Every refusal above left the stored set exactly as the admitted put left
    // it.
    let reported = stored_tags(&emulator, ACCOUNT, &job_id, "after the refusals").await;
    assert_eq!(
        reported, at_bound,
        "expected every refused put to leave the stored set alone, got {reported:?}"
    );
}
