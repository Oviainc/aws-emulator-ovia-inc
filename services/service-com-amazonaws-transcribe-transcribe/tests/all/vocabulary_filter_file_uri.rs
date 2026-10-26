// SPDX-License-Identifier: CC0-1.0

//! A custom vocabulary FILTER whose words the request names a FILE for, end to end
//! over the wire: `CreateVocabularyFilter` / `UpdateVocabularyFilter` with
//! `VocabularyFilterFileUri` instead of `Words`, then `GetVocabularyFilter` ->
//! `ListVocabularyFilters` -> `ListTagsForResource` -> `DeleteVocabularyFilter` over
//! the record they stored.
//!
//! THE RULE THESE ROWS ASSERT: `CreateVocabularyFilter` and `UpdateVocabularyFilter`
//! publish two alternatives for where a filter's words come from — the list in `Words`
//! and the file at `VocabularyFilterFileUri` — and a request choosing EITHER is
//! accepted and RECORDED, so the call answers 200 with the record it stored and every
//! later operation of the family finds that record and reports the update in its
//! `LastModifiedTime`. The words are not reported by any operation of this family
//! (`GetVocabularyFilterResponse` and `VocabularyFilterInfo` have no member for them,
//! `smithy_query(query="operation", operation="GetVocabularyFilter")`), so the only
//! difference the source makes is the service-managed `DownloadUri`: a filter this
//! service built from the words a request listed has one, and a filter whose words
//! would have had to be READ from a file does not — this emulator reads no object
//! store and makes no outbound request, so nothing built a file for it.
//!
//! The refusals that remain — neither alternative, or both at once — are asserted
//! here too, on the update as well as the create, so what changed is only the file
//! branch. The vocabulary family's own file form, which also carries a modelled
//! processing state, is `crate::vocabulary_file_uri`'s subject.
//!
//! A filter recorded either way is ONE record, so every operation that names a filter
//! by name resolves a file-backed one exactly as it does one built from `Words`: the
//! last test here drives each `Settings.VocabularyFilterName` member of this service's
//! job families over both sources.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler
//! bodies. The emulator, the call and the answer assertions are the transcription-job
//! suite's harness (`crate::transcription_jobs`), reused rather than stood up a
//! second time.

use std::time::Duration;

use serde_json::{json, Value};

use crate::medical_scribe_settings_references::{FILTER_METHODS, KIND_VOCABULARY_FILTER};
use crate::medical_vocabularies::is_absent;
use crate::transcription_job_request_rules::not_found_message;
use crate::transcription_jobs::{Reply, Transcribe};

/// The region every row of this suite is served in (the rows are unsigned, and this
/// is the region this emulator serves a request carrying no SigV4 credential scope
/// in), so a tagging ARN is predictable.
const REGION: &str = "us-east-1";

/// A valid `DataAccessRoleArn`, which the member's `@pattern` pins to an IAM role ARN
/// (`smithy_query(query="operation", operation="CreateVocabularyFilter")`).
const DATA_ACCESS_ROLE: &str = "arn:aws:iam::000000000000:role/TranscribeAccessRole";

/// Long enough that two calls carry distinct `LastModifiedTime` values at the
/// resolution the awsJson1_1 envelope renders a `DateTime` at, so "the update moved
/// the record's modification instant" can be asserted strictly.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const APART_IN_TIME: Duration = Duration::from_millis(5);

/// A `VocabularyFilterFileUri` of exactly `length` characters, inside the member's
/// `@pattern`.
fn file_uri_of_length(length: usize) -> String {
    let prefix = "s3://DOC-EXAMPLE-BUCKET/";
    assert!(
        length > prefix.len(),
        "a URI of {length} characters cannot carry the prefix {prefix:?}"
    );
    format!("{prefix}{}", "a".repeat(length - prefix.len()))
}

/// The class of `CreateVocabularyFilter` request that names a file as the source of
/// its words, one row per axis the rule generalises over: which URI scheme names the
/// file, how long that URI is, the form of the filter name, which of the model's
/// `LanguageCode` values the filter is in, and whether the request also carries tags,
/// a data-access role and an encryption configuration.
///
/// The schemes are the ones `VocabularyFilterFileUri`'s own `@pattern` admits —
/// `^(s3://|http(s*)://).+$` (`smithy_query(query="operation",
/// operation="CreateVocabularyFilter")`) — so the rows cover each alternative of that
/// catalog, including the two the issue did not cite, and the member's published
/// length ceiling of 2000; the language codes are entries of the model's own
/// `LanguageCode` enum (`smithy_query(query="shape", shape="LanguageCode")`).
fn file_backed_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "an s3 file, en-US",
            json!({
                "VocabularyFilterName": "words-from-s3",
                "LanguageCode": "en-US",
                "VocabularyFilterFileUri": "s3://DOC-EXAMPLE-BUCKET/my-filter-file.txt"
            }),
        ),
        (
            "an https file and a dotted, mixed-case name, en-GB",
            json!({
                "VocabularyFilterName": "Words.From-Https",
                "LanguageCode": "en-GB",
                "VocabularyFilterFileUri": "https://example-bucket.s3.amazonaws.com/words.txt"
            }),
        ),
        (
            "an http file, an underscored name and tags, es-US",
            json!({
                "VocabularyFilterName": "words_from_http",
                "LanguageCode": "es-US",
                "VocabularyFilterFileUri": "http://example-bucket.s3.amazonaws.com/words.txt",
                "Tags": [{"Key": "team", "Value": "speech"}, {"Key": "env", "Value": "test"}]
            }),
        ),
        (
            "an s3 file with a data-access role and an encryption configuration, ja-JP",
            json!({
                "VocabularyFilterName": "words-fully-specified",
                "LanguageCode": "ja-JP",
                "VocabularyFilterFileUri": "s3://DOC-EXAMPLE-BUCKET/ja-filter-file.txt",
                "DataAccessRoleArn": DATA_ACCESS_ROLE,
                "EncryptionConfiguration": {"KMSKey": "alias/aws/transcribe"}
            }),
        ),
        (
            "a file URI at the modelled 2000-character ceiling and a name at its own, de-DE",
            json!({
                "VocabularyFilterName": "z".repeat(200),
                "LanguageCode": "de-DE",
                "VocabularyFilterFileUri": file_uri_of_length(2000)
            }),
        ),
    ]
}

/// The `VocabularyFilterName` member of a request row.
fn name_of(request: &Value) -> String {
    request["VocabularyFilterName"]
        .as_str()
        .expect("every row names a filter")
        .to_string()
}

/// The `VocabularyFilterName`s of a `ListVocabularyFilters` answer, in the order it
/// listed them.
fn listed_names(reply: &Reply, case: &str) -> Vec<String> {
    reply.ok(case)["VocabularyFilters"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected VocabularyFilters, got {:?}",
                reply.body
            )
        })
        .iter()
        .map(|entry| {
            entry["VocabularyFilterName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a name in {entry:?}"))
                .to_string()
        })
        .collect()
}

/// An Amazon Transcribe vocabulary-filter ARN naming `name`, in the form
/// `TranscribeArn`'s `@pattern` pins.
fn vocabulary_filter_arn(name: &str) -> String {
    format!(
        "arn:aws:transcribe:{REGION}:{account}:vocabulary-filter/{name}",
        account = engine::emulator_account_id()
    )
}

/// A create that names a file is ACCEPTED and recorded: it answers 200 with the
/// record it stored, that record reads back through `GetVocabularyFilter` with the
/// same facts and no `DownloadUri` — nothing built a file for a filter whose words
/// were never read — it is listed, it carries the create's own tags, and it deletes.
#[tokio::test]
async fn a_file_backed_vocabulary_filter_is_recorded_and_reads_back() {
    let transcribe = Transcribe::new();
    let rows = file_backed_rows();
    let mut created_names: Vec<(&str, String)> = Vec::new();

    for (case, request) in &rows {
        let name = name_of(request);
        let language = request["LanguageCode"]
            .as_str()
            .expect("every row names a language")
            .to_string();

        let created = transcribe
            .call("CreateVocabularyFilter", request.clone())
            .await;
        // Every success of this awsJson1_1 service is 200: the model's `@http`
        // `code: 201` is inert on an RPC protocol.
        created.ok_status(case, 200);
        assert_eq!(
            (
                created.member(case, "VocabularyFilterName"),
                created.member(case, "LanguageCode"),
            ),
            (name.clone(), language.clone()),
            "case {case:?}: the create must answer the record it stored, got {:?}",
            created.body
        );
        let created_time = created.timestamp(case, "LastModifiedTime");

        // The record reads back through the family's own reader, with the same facts.
        let got = transcribe
            .call(
                "GetVocabularyFilter",
                json!({"VocabularyFilterName": &name}),
            )
            .await;
        got.ok(case);
        assert_eq!(
            (
                got.member(case, "VocabularyFilterName"),
                got.member(case, "LanguageCode"),
                got.timestamp(case, "LastModifiedTime"),
            ),
            (name.clone(), language.clone(), created_time),
            "case {case:?}: the reader must report the record just created, got {:?}",
            got.body
        );
        assert!(
            is_absent(&got, "DownloadUri"),
            "case {case:?}: nothing read this filter's file, so there is no file of its \
             own to download, got {:?}",
            got.body
        );
        if let Some(role) = request.get("DataAccessRoleArn").and_then(Value::as_str) {
            assert_eq!(
                got.member(case, "DataAccessRoleArn"),
                role,
                "case {case:?}: the stored data-access role must read back, got {:?}",
                got.body
            );
        }
        if let Some(configuration) = request.get("EncryptionConfiguration") {
            assert_eq!(
                &got.body["EncryptionConfiguration"], configuration,
                "case {case:?}: the stored encryption configuration must read back, got \
                 {:?}",
                got.body
            );
        }

        // The create's own tags are on the record, reachable through the family's ARN.
        let expected_tags = request.get("Tags").cloned().unwrap_or(json!([]));
        let listed_tags = transcribe
            .call(
                "ListTagsForResource",
                json!({"ResourceArn": vocabulary_filter_arn(&name)}),
            )
            .await;
        listed_tags.ok(case);
        let actual_tags = listed_tags.body.get("Tags").cloned().unwrap_or(json!([]));
        assert_eq!(
            actual_tags, expected_tags,
            "case {case:?}: the create's own tags must be stored on the record, got {:?}",
            listed_tags.body
        );

        created_names.push((case, name));
    }

    // Every one of them is in the store, newest first.
    let listed = transcribe
        .call("ListVocabularyFilters", json!({"MaxResults": 100}))
        .await;
    let mut listed = listed_names(&listed, "every stored filter");
    listed.sort();
    let mut expected: Vec<String> = rows.iter().map(|(_, request)| name_of(request)).collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "every file-backed filter must be in the store, got {listed:?}"
    );

    // A filter has no processing state in the model, so it is deletable straight away
    // and is gone afterwards.
    for (case, name) in &created_names {
        transcribe
            .call(
                "DeleteVocabularyFilter",
                json!({"VocabularyFilterName": name}),
            )
            .await
            .ok_status(case, 200);
        transcribe
            .call(
                "GetVocabularyFilter",
                json!({"VocabularyFilterName": name}),
            )
            .await
            .refused(case, 400, "BadRequestException");
    }
}

/// `UpdateVocabularyFilter` accepts either source too, and which one the LAST update
/// chose is what the stored filter's `DownloadUri` follows from — in both directions,
/// so nothing here is keyed to the shape of one request. Each update moves the
/// record's `LastModifiedTime` forward, which is the only thing this family reports
/// about an update having happened.
#[tokio::test]
async fn an_update_switches_a_filter_between_its_two_word_sources() {
    let transcribe = Transcribe::new();
    let name = "switched-filter";
    let created = transcribe
        .call(
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": name,
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
        )
        .await;
    created.ok("create the filter from inline words");
    let mut previous_time =
        created.timestamp("create the filter from inline words", "LastModifiedTime");

    // Each row is one `UpdateVocabularyFilter` and whether a filter whose words came
    // that way has a file of its own to download. The rows alternate between the two
    // sources, so each direction of the switch is covered, and they vary the URI
    // scheme and the number of inline words.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "inline words replaced by an s3 file",
            json!({
                "VocabularyFilterName": name,
                "VocabularyFilterFileUri": "s3://DOC-EXAMPLE-BUCKET/my-filter-file.txt"
            }),
            false,
        ),
        (
            "an s3 file replaced by several inline words",
            json!({
                "VocabularyFilterName": name,
                "Words": ["replaced", "words", "чепуха"]
            }),
            true,
        ),
        (
            "inline words replaced by an https file",
            json!({
                "VocabularyFilterName": name,
                "VocabularyFilterFileUri": "https://example-bucket.s3.amazonaws.com/words.txt"
            }),
            false,
        ),
        (
            "an https file replaced by an http file, with a data-access role",
            json!({
                "VocabularyFilterName": name,
                "VocabularyFilterFileUri": "http://example-bucket.s3.amazonaws.com/words.txt",
                "DataAccessRoleArn": DATA_ACCESS_ROLE
            }),
            false,
        ),
        (
            "an http file replaced by one inline word",
            json!({
                "VocabularyFilterName": name,
                "Words": ["crud"]
            }),
            true,
        ),
    ];

    for (case, request, has_download_uri) in rows {
        // So the modification instant the update records is distinguishable from the
        // one before it at the resolution the wire carries.
        std::thread::sleep(APART_IN_TIME);

        let updated = transcribe
            .call("UpdateVocabularyFilter", request.clone())
            .await;
        updated.ok(case);
        assert_eq!(
            (
                updated.member(case, "VocabularyFilterName"),
                updated.member(case, "LanguageCode"),
            ),
            (name.to_string(), "en-US".to_string()),
            "case {case:?}: the update must answer the stored record, whose language it \
             does not accept and so cannot change, got {:?}",
            updated.body
        );
        let updated_time = updated.timestamp(case, "LastModifiedTime");
        assert!(
            updated_time > previous_time,
            "case {case:?}: the update changed the record, so its LastModifiedTime must \
             move past {previous_time}, got {updated_time}"
        );

        let got = transcribe
            .call(
                "GetVocabularyFilter",
                json!({"VocabularyFilterName": name}),
            )
            .await;
        got.ok(case);
        assert_eq!(
            got.timestamp(case, "LastModifiedTime"),
            updated_time,
            "case {case:?}: the reader must report the instant the update recorded, got \
             {:?}",
            got.body
        );
        assert_eq!(
            !is_absent(&got, "DownloadUri"),
            has_download_uri,
            "case {case:?}: a filter has a file to download only when this service built \
             it from the words the request listed, got {:?}",
            got.body
        );
        assert!(
            is_absent(&got, "Words"),
            "case {case:?}: no operation of this family reports a filter's words, got \
             {:?}",
            got.body
        );

        previous_time = updated_time;
    }
}

/// The refusals the two operations keep: a request that says NEITHER where its words
/// come from, or BOTH at once, is the modelled `BadRequestException` — and on the
/// update it leaves the stored record exactly as it was.
#[tokio::test]
async fn a_filter_request_naming_neither_or_both_sources_is_still_refused() {
    let transcribe = Transcribe::new();
    let name = "unchanged-filter";
    let created = transcribe
        .call(
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": name,
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
        )
        .await;
    created.ok("create the filter the refused updates must leave alone");
    let before = created.timestamp("create", "LastModifiedTime");

    // Rows differ along the axis the rule generalises over: which of the two content
    // members the request carries (neither, an empty inline list, both), and which
    // scheme the file URI of a both-sources request uses. The third column is the
    // refusal the row earns: the handler's own `BadRequestException` where the request
    // is inside every modelled constraint, and the boundary's `ValidationException`
    // where it breaks one (`Words` is `@length(min: 1)`,
    // `smithy_query(query="operation", operation="UpdateVocabularyFilter")`).
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an update with neither Words nor VocabularyFilterFileUri",
            json!({"VocabularyFilterName": name}),
            "BadRequestException",
        ),
        (
            "an update with an empty Words list, below the modelled list floor of 1",
            json!({"VocabularyFilterName": name, "Words": []}),
            "ValidationException",
        ),
        (
            "an update with both Words and an s3 VocabularyFilterFileUri",
            json!({
                "VocabularyFilterName": name,
                "Words": ["crud"],
                "VocabularyFilterFileUri": "s3://DOC-EXAMPLE-BUCKET/my-filter-file.txt"
            }),
            "BadRequestException",
        ),
        (
            "an update with both Words and an https VocabularyFilterFileUri",
            json!({
                "VocabularyFilterName": name,
                "Words": ["crud"],
                "VocabularyFilterFileUri": "https://example-bucket.s3.amazonaws.com/words.txt"
            }),
            "BadRequestException",
        ),
    ];

    for (case, request, code) in rows {
        std::thread::sleep(APART_IN_TIME);
        transcribe
            .call("UpdateVocabularyFilter", request.clone())
            .await
            .refused(case, 400, code);

        // A refused update changes nothing about the stored record.
        let got = transcribe
            .call(
                "GetVocabularyFilter",
                json!({"VocabularyFilterName": name}),
            )
            .await;
        got.ok(case);
        assert_eq!(
            got.timestamp(case, "LastModifiedTime"),
            before,
            "case {case:?}: a refused update must leave the record's modification \
             instant where it was, got {:?}",
            got.body
        );
        assert!(
            !is_absent(&got, "DownloadUri"),
            "case {case:?}: the filter is still the one built from the words the create \
             listed, got {:?}",
            got.body
        );
    }

    // A file URI one past the member's published 2000-character ceiling is refused at
    // the BOUNDARY, before the handler runs, by the constraint trait the model carries.
    std::thread::sleep(APART_IN_TIME);
    transcribe
        .call(
            "UpdateVocabularyFilter",
            json!({
                "VocabularyFilterName": name,
                "VocabularyFilterFileUri": file_uri_of_length(2001)
            }),
        )
        .await
        .refused(
            "an update whose file URI is one past the modelled ceiling",
            400,
            "ValidationException",
        );
    let got = transcribe
        .call(
            "GetVocabularyFilter",
            json!({"VocabularyFilterName": name}),
        )
        .await;
    got.ok("after the boundary refusal");
    assert_eq!(
        got.timestamp("after the boundary refusal", "LastModifiedTime"),
        before,
        "a request refused at the boundary must leave the record untouched, got {:?}",
        got.body
    );
}

/// Where a filter's words come from, as the `CreateVocabularyFilter` member(s) that
/// name the source: both alternatives the operation publishes, the file one over each
/// URI scheme its `@pattern` admits (`^(s3://|http(s*)://).+$`) and at the member's
/// published 2000-character ceiling.
fn word_sources() -> Vec<(&'static str, Value)> {
    vec![
        ("inline Words", json!({"Words": ["crud", "чепуха"]})),
        (
            "an s3 file",
            json!({"VocabularyFilterFileUri": "s3://DOC-EXAMPLE-BUCKET/my-filter-file.txt"}),
        ),
        (
            "an https file",
            json!({
                "VocabularyFilterFileUri": "https://example-bucket.s3.amazonaws.com/words.txt"
            }),
        ),
        (
            "an http file",
            json!({
                "VocabularyFilterFileUri": "http://example-bucket.s3.amazonaws.com/words.txt"
            }),
        ),
        (
            "a file URI at the modelled 2000-character ceiling",
            json!({"VocabularyFilterFileUri": file_uri_of_length(2000)}),
        ),
    ]
}

/// A `CreateVocabularyFilter` request for `name` whose words come from `source`.
fn create_from(name: &str, source: &Value) -> Value {
    let mut request = json!({"VocabularyFilterName": name, "LanguageCode": "en-US"});
    let members = request
        .as_object_mut()
        .expect("a create request is a JSON object");
    for (member, value) in source
        .as_object()
        .expect("a word source is a JSON object")
        .iter()
    {
        members.insert(member.clone(), value.clone());
    }
    request
}

/// A reader's request: the one member that names the resource to report.
fn by_name(member: &str, name: &str) -> Value {
    let mut request = serde_json::Map::new();
    request.insert(member.to_string(), json!(name));
    Value::Object(request)
}

/// One operation that NAMES a custom vocabulary filter: the start that applies it, the
/// reader that reports the stored job, and where the applied name sits in each.
struct FilterConsumer {
    case: &'static str,
    start: &'static str,
    get: &'static str,
    /// The job-name member the start and the reader both carry.
    name_member: &'static str,
    /// The member of the reader's answer the stored job sits under.
    job_member: &'static str,
    /// A start request for the job `job` applying the filter `filter`, qualified by the
    /// vocabulary filter `method` where the member it uses documents one.
    request: fn(job: &str, filter: &str, method: &str) -> Value,
    /// The filter name the stored job reports.
    stored_reference: fn(&Value) -> Option<&str>,
}

/// Every member of this service through which a job applies a custom vocabulary filter
/// it names: the top-level settings of each job family that has one, and the
/// per-language `LanguageIdSettings` that names resources of the same families
/// (`smithy_query(query="shape", shape="Settings,CallAnalyticsJobSettings,\
/// MedicalScribeSettings,LanguageIdSettings")`).
fn filter_consumers() -> Vec<FilterConsumer> {
    vec![
        FilterConsumer {
            case: "StartTranscriptionJob Settings.VocabularyFilterName",
            start: "StartTranscriptionJob",
            get: "GetTranscriptionJob",
            name_member: "TranscriptionJobName",
            job_member: "TranscriptionJob",
            // "if you include VocabularyFilterName in your request, you must also
            // include VocabularyFilterMethod".
            request: |job, filter, method| {
                json!({
                    "TranscriptionJobName": job,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
                    "Settings": {
                        "VocabularyFilterName": filter,
                        "VocabularyFilterMethod": method
                    }
                })
            },
            stored_reference: |job| job["Settings"]["VocabularyFilterName"].as_str(),
        },
        FilterConsumer {
            case: "StartTranscriptionJob LanguageIdSettings[en-US].VocabularyFilterName",
            start: "StartTranscriptionJob",
            get: "GetTranscriptionJob",
            name_member: "TranscriptionJobName",
            job_member: "TranscriptionJob",
            request: |job, filter, _| {
                json!({
                    "TranscriptionJobName": job,
                    "IdentifyLanguage": true,
                    // "It's recommended that you include LanguageOptions when using
                    // LanguageIdSettings to ensure that the correct language dialect
                    // is identified", and "the language codes that you specify must
                    // match the languages of the associated ... custom vocabulary
                    // filters" — the filters these rows hold are all en-US
                    // (`smithy_query(query="doc", shape="StartTranscriptionJobRequest",
                    // member="LanguageIdSettings")`).
                    "LanguageOptions": ["en-US"],
                    "Media": {"MediaFileUri": "https://example.invalid/media/clip.mp4"},
                    "LanguageIdSettings": {"en-US": {"VocabularyFilterName": filter}}
                })
            },
            stored_reference: |job| {
                job["LanguageIdSettings"]["en-US"]["VocabularyFilterName"].as_str()
            },
        },
        FilterConsumer {
            case: "StartCallAnalyticsJob Settings.VocabularyFilterName",
            start: "StartCallAnalyticsJob",
            get: "GetCallAnalyticsJob",
            name_member: "CallAnalyticsJobName",
            job_member: "CallAnalyticsJob",
            request: |job, filter, method| {
                json!({
                    "CallAnalyticsJobName": job,
                    "Media": {"MediaFileUri": "s3://example-media/call.flac"},
                    "Settings": {
                        "VocabularyFilterName": filter,
                        "VocabularyFilterMethod": method
                    }
                })
            },
            stored_reference: |job| job["Settings"]["VocabularyFilterName"].as_str(),
        },
        FilterConsumer {
            case: "StartMedicalScribeJob Settings.VocabularyFilterName",
            start: "StartMedicalScribeJob",
            get: "GetMedicalScribeJob",
            name_member: "MedicalScribeJobName",
            job_member: "MedicalScribeJob",
            // `OutputBucketName` and `DataAccessRoleArn` are `@required` here, and the
            // settings must choose one of the two speaker-partitioning modes.
            request: |job, filter, method| {
                json!({
                    "MedicalScribeJobName": job,
                    "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                    "OutputBucketName": "example-scribe-output",
                    "DataAccessRoleArn": DATA_ACCESS_ROLE,
                    "Settings": {
                        "ShowSpeakerLabels": true,
                        "MaxSpeakerLabels": 2,
                        "VocabularyFilterName": filter,
                        "VocabularyFilterMethod": method
                    }
                })
            },
            stored_reference: |job| job["Settings"]["VocabularyFilterName"].as_str(),
        },
    ]
}

/// A filter is ONE record whatever its words came from, so every member of this service
/// that applies a filter BY NAME resolves a file-backed filter exactly as it does one
/// built from the words a request listed: the start is served, the stored job reports
/// the name it applied, and a name no create recorded is still the family's own
/// not-found refusal — so what serves these starts is the record, not a missing check.
#[tokio::test]
async fn a_file_backed_filter_is_usable_from_every_member_that_names_a_filter() {
    let transcribe = Transcribe::new();
    let sources = word_sources();

    // One held filter per word source, so a start naming it is decided only by where
    // that filter's words came from.
    for (index, (case, source)) in sources.iter().enumerate() {
        transcribe
            .call(
                "CreateVocabularyFilter",
                create_from(&format!("filter-from-{index}"), source),
            )
            .await
            .ok(case);
    }

    for (consumer_index, consumer) in filter_consumers().iter().enumerate() {
        for (index, (source_case, _)) in sources.iter().enumerate() {
            let case = format!("{} applying a filter from {source_case}", consumer.case);
            let filter = format!("filter-from-{index}");
            let job = format!("job-{consumer_index}-{index}");
            let method = FILTER_METHODS[index % FILTER_METHODS.len()];

            transcribe
                .call(consumer.start, (consumer.request)(&job, &filter, method))
                .await
                .ok_status(&case, 200);

            // The served start stored the reference, so the family's reader reports it.
            let fetched = transcribe
                .call(consumer.get, by_name(consumer.name_member, &job))
                .await;
            let stored = fetched.ok(&case)[consumer.job_member].clone();
            assert_eq!(
                (consumer.stored_reference)(&stored),
                Some(filter.as_str()),
                "case {case:?}: the stored job must report the filter it applied, got \
                 {stored:?}"
            );
        }

        // The same member naming a filter this account does NOT hold is refused with
        // the family's not-found text, which names the kind and not the name asked for.
        let case = format!("{} applying a filter no create recorded", consumer.case);
        let refused = transcribe
            .call(
                consumer.start,
                (consumer.request)(
                    &format!("dangling-{consumer_index}"),
                    "never-created",
                    FILTER_METHODS[0],
                ),
            )
            .await;
        refused.refused(&case, 400, "BadRequestException");
        assert_eq!(
            refused.error_message(&case),
            not_found_message(KIND_VOCABULARY_FILTER),
            "case {case:?}: the refusal must be the vocabulary filter family's own \
             not-found text, got {:?}",
            refused.body
        );
    }
}
