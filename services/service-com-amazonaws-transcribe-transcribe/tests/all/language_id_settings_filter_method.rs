// SPDX-License-Identifier: CC0-1.0

//! The vocabulary-filter co-requirement, read wherever a start NAMES a filter.
//!
//! THE RULE UNDER TEST: a start that names a custom vocabulary filter must also name
//! the method that says how to apply it, whichever member of the request names the
//! filter — "Note that if you include VocabularyFilterName in your request, you must
//! also include VocabularyFilterMethod" is documented on the per-language
//! `LanguageIdSettings` entries word for word as it is on the top-level settings member
//! (`smithy_query(query="shape",
//! shape="LanguageIdSettings,Settings,CallAnalyticsJobSettings")`). So a start whose
//! filter is qualified by no method anywhere is refused with the modelled
//! `BadRequestException` (HTTP 400) and stores nothing, while a start whose filter is
//! qualified — or that names no filter at all — is accepted and reads the per-language
//! settings back unchanged. `LanguageIdSettings` models no method member of its own, so
//! what qualifies a filter it names is the settings shape's `VocabularyFilterMethod`,
//! the only member of either request that says how a filter is applied.
//!
//! The rows span the class rather than one request: both operations that model the map
//! (`StartTranscriptionJob`, which carries it at the top level, and
//! `StartCallAnalyticsJob`, which carries it inside its settings), one up to the
//! modelled five entries, which entry of a map names the filter, every value of the
//! `VocabularyFilterMethod` enum, and entries that name a resource of another family
//! instead.
//!
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// The custom vocabulary filter every row that names a filter applies, created once per
/// emulator so each row is decided by the co-requirement rule and not by the
/// dangling-reference rule.
///
/// ONE filter serves rows in every language code: a filter whose language differs from
/// the identified one "isn't applied" and "there are no errors or warnings associated
/// with a language mismatch" (model doc for
/// `LanguageIdSettings$VocabularyFilterName`).
const HELD_FILTER: &str = "held-filter";

/// The custom vocabulary the rows that name no filter apply, held for the same reason.
const HELD_VOCABULARY: &str = "held-vocabulary";

/// Every value of `VocabularyFilterMethod` (`smithy_query(query="shape",
/// shape="VocabularyFilterMethod")`): each one on its own completes the pair.
const FILTER_METHODS: &[&str] = &["mask", "remove", "tag"];

/// One row: the per-language map a start asks for (JSON null for a start that asks for
/// none), the method it qualifies its filters with, and — for a row the rule refuses —
/// the language code of the entry whose filter name the refusal must spell.
struct Row {
    case: &'static str,
    language_id_settings: Value,
    method: Option<&'static str>,
    refused_entry: Option<&'static str>,
}

/// A per-language entry naming the held filter.
fn names_filter() -> Value {
    json!({"VocabularyFilterName": HELD_FILTER})
}

/// A per-language entry naming the held custom vocabulary, a resource of another
/// family, so the entry names no filter at all.
fn names_vocabulary() -> Value {
    json!({"VocabularyName": HELD_VOCABULARY})
}

/// The maps this rule decides, refused and accepted rows together.
///
/// A map is modelled `length(min: 1, max: 5)` (`smithy_query(query="operation",
/// operation="StartTranscriptionJob")`), so the rows run from one entry up to five, and
/// the entry that names the filter is first, last and in the middle across them.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "one entry naming a filter, no method anywhere",
            language_id_settings: json!({"en-US": names_filter()}),
            method: None,
            refused_entry: Some("en-US"),
        },
        Row {
            case: "one entry in another language naming a filter, no method anywhere",
            language_id_settings: json!({"es-US": names_filter()}),
            method: None,
            refused_entry: Some("es-US"),
        },
        Row {
            case: "a filter named beside a custom vocabulary in one entry, no method",
            language_id_settings: json!({
                "en-GB": {"VocabularyName": HELD_VOCABULARY, "VocabularyFilterName": HELD_FILTER}
            }),
            method: None,
            refused_entry: Some("en-GB"),
        },
        Row {
            case: "three entries, only the last naming a filter, no method",
            language_id_settings: json!({
                "en-AU": names_vocabulary(),
                "en-GB": names_vocabulary(),
                "es-US": names_filter()
            }),
            method: None,
            // The entries are read in language-code order, so the refusal names the
            // first entry that names a filter whichever order the request listed them.
            refused_entry: Some("es-US"),
        },
        Row {
            case: "five entries, the middle one naming a filter, no method",
            language_id_settings: json!({
                "de-DE": names_vocabulary(),
                "en-AU": names_vocabulary(),
                "en-GB": names_filter(),
                "fr-FR": names_vocabulary(),
                "it-IT": names_vocabulary()
            }),
            method: None,
            refused_entry: Some("en-GB"),
        },
        Row {
            case: "two entries each naming a filter, no method",
            language_id_settings: json!({"en-GB": names_filter(), "en-US": names_filter()}),
            method: None,
            refused_entry: Some("en-GB"),
        },
        Row {
            case: "no per-language map at all, and no method",
            language_id_settings: Value::Null,
            method: None,
            refused_entry: None,
        },
        Row {
            case: "entries naming a custom vocabulary only, no method",
            language_id_settings: json!({
                "en-US": names_vocabulary(),
                "es-US": names_vocabulary()
            }),
            method: None,
            refused_entry: None,
        },
        Row {
            case: "a method with no filter named anywhere",
            language_id_settings: json!({"en-US": names_vocabulary()}),
            method: Some("mask"),
            refused_entry: None,
        },
        Row {
            case: "no per-language map, and a method on its own",
            language_id_settings: Value::Null,
            method: Some("remove"),
            refused_entry: None,
        },
        Row {
            case: "five entries, two naming a filter, qualified by a method",
            language_id_settings: json!({
                "de-DE": names_vocabulary(),
                "en-AU": names_filter(),
                "en-GB": names_vocabulary(),
                "es-US": names_filter(),
                "it-IT": names_vocabulary()
            }),
            method: Some("tag"),
            refused_entry: None,
        },
    ];
    // One accepted row per documented method: the method is what a named filter is
    // co-required with, so each value of the enum completes the pair on its own.
    for method in FILTER_METHODS {
        rows.push(Row {
            case: match *method {
                "mask" => "an entry naming a filter, with the mask method",
                "remove" => "an entry naming a filter, with the remove method",
                _ => "an entry naming a filter, with the tag method",
            },
            language_id_settings: json!({"en-US": names_filter()}),
            method: Some(method),
            refused_entry: None,
        });
    }
    rows
}

/// One job family that models the per-language map: where its request carries the map
/// and the method, and how a started job is read back.
struct Family {
    case: &'static str,
    start: &'static str,
    read: &'static str,
    /// The member a read request names the job in.
    read_name_member: &'static str,
    /// The member a read answer carries the job record in.
    record: &'static str,
    /// The path the CALLER spells the map at, which a refusal names the offending entry
    /// under.
    map_path: &'static str,
    /// A start request for the job `name` asking for `row`.
    request: fn(name: &str, row: &Row) -> Value,
    /// The map a stored job of this family reports.
    stored_map: fn(&Value) -> &Value,
}

/// The two operations that accept a `LanguageIdSettings` map (`smithy_query(query=\
/// "shape", shape="StartTranscriptionJobRequest,CallAnalyticsJobSettings")`): the same
/// rows apply to each, so a rule applied to one and not the other fails here.
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "transcription job",
            start: "StartTranscriptionJob",
            read: "GetTranscriptionJob",
            read_name_member: "TranscriptionJobName",
            record: "TranscriptionJob",
            map_path: "LanguageIdSettings",
            request: |name, row| {
                let mut request = json!({
                    "TranscriptionJobName": name,
                    // "If using automatic language identification in your request and
                    // you want to apply a custom language model, a custom vocabulary, or
                    // a custom vocabulary filter, include LanguageIdSettings" — the map
                    // belongs to an identification request, and exactly one of the three
                    // language specifiers may be given.
                    "IdentifyLanguage": true,
                    "Media": {"MediaFileUri": "s3://example-media/audio.flac"}
                });
                let object = request
                    .as_object_mut()
                    .expect("a start request is a JSON object");
                if !row.language_id_settings.is_null() {
                    object.insert(
                        "LanguageIdSettings".to_string(),
                        row.language_id_settings.clone(),
                    );
                }
                if let Some(method) = row.method {
                    object.insert(
                        "Settings".to_string(),
                        json!({"VocabularyFilterMethod": method}),
                    );
                }
                request
            },
            stored_map: |job| &job["LanguageIdSettings"],
        },
        Family {
            case: "call analytics job",
            start: "StartCallAnalyticsJob",
            read: "GetCallAnalyticsJob",
            read_name_member: "CallAnalyticsJobName",
            record: "CallAnalyticsJob",
            map_path: "Settings.LanguageIdSettings",
            request: |name, row| {
                let mut settings = serde_json::Map::new();
                if !row.language_id_settings.is_null() {
                    settings.insert(
                        "LanguageIdSettings".to_string(),
                        row.language_id_settings.clone(),
                    );
                }
                if let Some(method) = row.method {
                    settings.insert("VocabularyFilterMethod".to_string(), json!(method));
                }
                let mut request = json!({
                    "CallAnalyticsJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/call.flac"}
                });
                if !settings.is_empty() {
                    request
                        .as_object_mut()
                        .expect("a start request is a JSON object")
                        .insert("Settings".to_string(), Value::Object(settings));
                }
                request
            },
            stored_map: |job| &job["Settings"]["LanguageIdSettings"],
        },
    ]
}

/// Read the job `name` of `family` back.
async fn read(transcribe: &Transcribe, family: &Family, name: &str) -> Reply {
    let mut request = serde_json::Map::new();
    request.insert(family.read_name_member.to_string(), json!(name));
    transcribe.call(family.read, Value::Object(request)).await
}

/// An emulator holding the custom resources the rows NAME, each in a state a job may
/// apply it in — a custom vocabulary is applicable only once its build has settled
/// `READY`.
async fn given_the_custom_resources() -> Transcribe {
    let transcribe = Transcribe::new();
    transcribe
        .call(
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": HELD_FILTER,
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
        )
        .await
        .ok("CreateVocabularyFilter");
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": HELD_VOCABULARY,
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark", "Transcribe"]
            }),
        )
        .await
        .ok("CreateVocabulary");
    transcribe.vocabulary_ready(HELD_VOCABULARY, None).await;
    transcribe
}

/// How this service words "the member you included needs one you left out", for a
/// filter named at `member` with no method to apply it with.
fn missing_method_message(member: &str) -> String {
    format!(
        "Settings.VocabularyFilterMethod is required when {member} is specified. \
         Correct your request and try again."
    )
}

#[tokio::test]
async fn a_start_naming_a_vocabulary_filter_with_no_method_is_refused() {
    for family in families() {
        let transcribe = given_the_custom_resources().await;
        for (index, row) in rows()
            .iter()
            .enumerate()
            .filter(|(_, row)| row.refused_entry.is_some())
        {
            let language = row.refused_entry.expect("a refused row names its entry");
            let case = format!("{} / {}", family.case, row.case);
            let name = format!("one-sided-{index}");
            let refused = transcribe
                .call(family.start, (family.request)(&name, row))
                .await;
            refused.refused(&case, 400, "BadRequestException");

            // The refusal names the member path the caller sent, down to the entry of
            // the map that named the filter, and the companion it needs.
            let message = refused.error_message(&case);
            let expected = missing_method_message(&format!(
                "{}[{language}].VocabularyFilterName",
                family.map_path
            ));
            assert_eq!(
                message, expected,
                "case {case:?}: expected the refusal {expected:?}, got {message:?}"
            );

            // A refused start stores nothing, so the name it asked for is unknown.
            read(&transcribe, &family, &name)
                .await
                .refused(&case, 400, "BadRequestException");
        }
    }
}

#[tokio::test]
async fn a_start_whose_named_filters_carry_a_method_is_accepted_and_read_back() {
    for family in families() {
        let transcribe = given_the_custom_resources().await;
        for (index, row) in rows()
            .iter()
            .enumerate()
            .filter(|(_, row)| row.refused_entry.is_none())
        {
            let case = format!("{} / {}", family.case, row.case);
            let name = format!("whole-{index}");
            transcribe
                .call(family.start, (family.request)(&name, row))
                .await
                .ok_status(&case, 200);

            // The served start stored the map, so the family's reader reports it
            // unchanged — and a row that asked for no map has none invented for it.
            let fetched = read(&transcribe, &family, &name).await;
            let job = fetched.ok(&case)[family.record].clone();
            let stored = (family.stored_map)(&job);
            assert_eq!(
                stored, &row.language_id_settings,
                "case {case:?}: the stored job must report the per-language settings it \
                 was started with, expected {:?}, got {stored:?}",
                row.language_id_settings
            );
        }
    }
}
