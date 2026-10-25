// SPDX-License-Identifier: CC0-1.0

//! What custom resources a `StartCallAnalyticsJob` request may NAME — the refusal the
//! family's lifecycle suite (`crate::call_analytics_jobs`) does not span, since every
//! row of that suite names only resources the account holds.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handler's own refusals — not just the handler body.
//! The emulator, the call and this family's read helper are the Call Analytics job
//! suite's (`crate::call_analytics_jobs`), reused rather than stood up a second time.
//!
//! THE RULES UNDER TEST:
//!
//! * a start may apply only a custom vocabulary, custom vocabulary filter or custom
//!   language model THIS ACCOUNT HOLDS in the family the member reads — whether the
//!   name sits in the request's top-level `Settings` or in any per-language entry of
//!   `Settings.LanguageIdSettings`, which name resources of the same three families
//!   (`smithy_query(query="shape", shape="CallAnalyticsJobSettings,LanguageIdSettings")`) —
//!   so a name no create of that family recorded is refused with that family's own
//!   not-found refusal and nothing is stored, while the same start is accepted once a
//!   create of that family records the name;
//! * the families are distinct stores, so a name held in another family never stands
//!   in for one the member reads.
//!
//! Each test asserts its rule over a table spanning the class it generalises over (the
//! member that names the resource, the family the name must be held in, and where in
//! the settings it sits), not over one row. The one member the sibling suite
//! `crate::transcription_job_request_rules` already drives through this operation
//! (`Settings.VocabularyName`) stays in the table as the row the others vary from; the
//! family's job quota is `crate::job_concurrency_quota`'s subject, not this suite's.

use serde_json::{json, Value};

use crate::call_analytics_jobs::get;
use crate::language_models::language_model_request;
use crate::transcription_job_request_rules::not_found_message;
use crate::transcription_jobs::Transcribe;

/// A `StartCallAnalyticsJob` request for `name` carrying `settings`, minimal otherwise.
fn start_request(name: &str, settings: &Value) -> Value {
    json!({
        "CallAnalyticsJobName": name,
        "Media": {"MediaFileUri": "s3://example-media/call.flac"},
        "Settings": settings
    })
}

/// One row of [`a_start_naming_a_custom_resource_this_account_lacks_is_refused`]: the
/// settings a start carries, the create that makes this account hold the named
/// resource in the family that member reads, and the word a refusal names that family
/// by.
struct Reference {
    case: &'static str,
    settings: fn(&str) -> Value,
    create: &'static str,
    create_request: fn(&str) -> Value,
    kind: &'static str,
}

/// A custom vocabulary of `name`, held in the `vocabulary` family.
fn vocabulary_create(name: &str) -> Value {
    json!({"VocabularyName": name, "LanguageCode": "en-US", "Phrases": ["Aardvark"]})
}

/// A custom vocabulary FILTER of `name`, held in the `vocabulary filter` family.
fn vocabulary_filter_create(name: &str) -> Value {
    json!({"VocabularyFilterName": name, "LanguageCode": "en-US", "Words": ["crud"]})
}

/// The settings members of `CallAnalyticsJobSettings` that name a custom resource,
/// each once at the top level and once inside a per-language `LanguageIdSettings`
/// entry, in a different language code per row.
fn reference_rows() -> Vec<Reference> {
    vec![
        Reference {
            case: "Settings.VocabularyName",
            settings: |name| json!({"VocabularyName": name}),
            create: "CreateVocabulary",
            create_request: vocabulary_create,
            kind: "vocabulary",
        },
        Reference {
            // `VocabularyFilterMethod` is co-required with the filter name, so the row
            // carries it: what is under test is the family lookup, not that rule.
            case: "Settings.VocabularyFilterName",
            settings: |name| {
                json!({"VocabularyFilterName": name, "VocabularyFilterMethod": "mask"})
            },
            create: "CreateVocabularyFilter",
            create_request: vocabulary_filter_create,
            kind: "vocabulary filter",
        },
        Reference {
            case: "Settings.LanguageModelName",
            settings: |name| json!({"LanguageModelName": name}),
            create: "CreateLanguageModel",
            create_request: |name| language_model_request(name, "en-US", "WideBand"),
            kind: "language model",
        },
        Reference {
            case: "Settings.LanguageIdSettings[en-US].VocabularyName",
            settings: |name| {
                json!({
                    "LanguageOptions": ["en-US", "es-US"],
                    "LanguageIdSettings": {"en-US": {"VocabularyName": name}}
                })
            },
            create: "CreateVocabulary",
            create_request: vocabulary_create,
            kind: "vocabulary",
        },
        Reference {
            case: "Settings.LanguageIdSettings[es-US].VocabularyFilterName",
            settings: |name| {
                json!({
                    "LanguageOptions": ["en-US", "es-US"],
                    "LanguageIdSettings": {"es-US": {"VocabularyFilterName": name}}
                })
            },
            create: "CreateVocabularyFilter",
            create_request: vocabulary_filter_create,
            kind: "vocabulary filter",
        },
        Reference {
            case: "Settings.LanguageIdSettings[en-GB].LanguageModelName",
            settings: |name| {
                json!({
                    "LanguageOptions": ["en-GB", "en-AU"],
                    "LanguageIdSettings": {"en-GB": {"LanguageModelName": name}}
                })
            },
            create: "CreateLanguageModel",
            create_request: |name| language_model_request(name, "en-GB", "WideBand"),
            kind: "language model",
        },
    ]
}

#[tokio::test]
async fn a_start_naming_a_custom_resource_this_account_lacks_is_refused() {
    for (index, row) in reference_rows().into_iter().enumerate() {
        // A fresh emulator per row, so no other row's create is what makes this row's
        // start succeed.
        let transcribe = Transcribe::new();
        let case = row.case;
        let name = format!("absent-resource-{index}");
        let settings = (row.settings)(&name);

        let dangling = format!("dangling-job-{index}");
        let refused = transcribe
            .call("StartCallAnalyticsJob", start_request(&dangling, &settings))
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message,
            not_found_message(row.kind),
            "case {case:?}: the refusal must name the {:?} family, got {message:?}",
            row.kind
        );

        // A refused start stores no job: the name it asked for is still unknown.
        let missing = get(&transcribe, &dangling).await;
        missing.refused(case, 400, "BadRequestException");

        // Held in the family that member reads, the very same start is accepted and
        // the job reads back carrying the settings that named it.
        transcribe
            .call(row.create, (row.create_request)(&name))
            .await
            .ok(&format!("{case}: {}", row.create));
        let held = format!("held-job-{index}");
        transcribe
            .call("StartCallAnalyticsJob", start_request(&held, &settings))
            .await
            .ok(case);
        let fetched = get(&transcribe, &held).await;
        let stored = fetched.ok(case)["CallAnalyticsJob"]["Settings"].clone();
        assert_eq!(
            stored, settings,
            "case {case:?}: the stored job must report the settings it was started \
             with, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_name_held_only_in_another_family_does_not_satisfy_a_start() {
    let transcribe = Transcribe::new();
    let name = "shared-name";

    // The name is held — but in neither family a `VocabularyName` nor a
    // `LanguageModelName` reads: the stores are distinct, so a resource of one family
    // never stands in for another's.
    transcribe
        .call("CreateVocabularyFilter", vocabulary_filter_create(name))
        .await
        .ok("CreateVocabularyFilter shared-name");
    transcribe
        .call(
            "CreateMedicalVocabulary",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-US",
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/clinical-terms.txt"
            }),
        )
        .await
        .ok("CreateMedicalVocabulary shared-name");

    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a custom vocabulary where a vocabulary FILTER and a MEDICAL vocabulary are held",
            json!({"VocabularyName": name}),
            "vocabulary",
        ),
        (
            "a language MODEL where a vocabulary FILTER and a MEDICAL vocabulary are held",
            json!({"LanguageModelName": name}),
            "language model",
        ),
        (
            "a per-language custom vocabulary where the other families hold the name",
            json!({
                "LanguageOptions": ["en-US", "es-US"],
                "LanguageIdSettings": {"en-US": {"VocabularyName": name}}
            }),
            "vocabulary",
        ),
    ];

    for (index, (case, settings, kind)) in rows.iter().enumerate() {
        let job = format!("wrong-family-{index}");
        let refused = transcribe
            .call("StartCallAnalyticsJob", start_request(&job, settings))
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message,
            not_found_message(kind),
            "case {case:?}: the refusal must name the {kind:?} family, got {message:?}"
        );
        get(&transcribe, &job)
            .await
            .refused(case, 400, "BadRequestException");
    }

    // Recorded in the family the member reads, the same name is accepted.
    transcribe
        .call("CreateVocabulary", vocabulary_create(name))
        .await
        .ok("CreateVocabulary shared-name");
    transcribe
        .call(
            "StartCallAnalyticsJob",
            start_request("right-family", &rows[0].1),
        )
        .await
        .ok(rows[0].0);
}
