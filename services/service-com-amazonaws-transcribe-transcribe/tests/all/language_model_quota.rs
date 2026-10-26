// SPDX-License-Identifier: CC0-1.0

//! The count ceiling `CreateLanguageModel` bounds the custom LANGUAGE model family by,
//! end to end over the wire: `CreateLanguageModel` -> `ListLanguageModels` ->
//! `DescribeLanguageModel` -> `DeleteLanguageModel`.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handler's own refusals — not just the handler body.
//! The emulator and the answer assertions are the transcription-job suite's harness
//! (`crate::transcription_jobs`), and the request shape and the transition wait are the
//! language-model suite's (`crate::language_models`), reused rather than stood up a
//! second time.
//!
//! THE RULE UNDER TEST: a create is served while the Region the request is served in
//! holds fewer custom language models than this family's ceiling, and one past it is
//! refused with the modelled `LimitExceededException` — a refusal that reports the
//! ceiling, echoes no name of the caller's and stores nothing. The count is of the
//! RECORDS one Region holds in this one family: it counts a model the service has
//! finished with just as it counts one still training, it does not count another
//! Region's models or another family's resources, and `DeleteLanguageModel` is what
//! frees a slot again — one slot per delete.
//!
//! The rows differ along what the rule generalises over: the language and base model of
//! each model that fills the ceiling (the whole of both modelled enums), whether its
//! request carries tuning data or tags, the shape of the request refused at the ceiling,
//! the Region a call is signed for, and whether the models held are still training or
//! already finished.

use serde_json::{json, Value};

use service_com_amazonaws_transcribe_transcribe::state::MAX_LANGUAGE_MODELS;

use crate::call_analytics_categories::non_talk_time_rule;
use crate::language_models::{language_model_request, PAST_THE_TRANSITION};
use crate::transcription_jobs::Transcribe;

/// The modelled refusal a create past an allowance carries, and the status this
/// service's awsJson1_1 wire protocol puts on every client error (both pinned by
/// `crate::client_error_wire_status`).
const QUOTA_CODE: &str = "LimitExceededException";
/// See [`QUOTA_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;
/// The modelled refusal this service answers for a resource it holds nothing under
/// ("This can occur when the entity you're trying to delete doesn't exist", the model's
/// own doc for `BadRequestException`).
const ABSENT_CODE: &str = "BadRequestException";

/// The largest page `ListLanguageModels` may be asked for: `MaxResults` is
/// `@range(min: 1, max: 100)`
/// (`smithy_query(query="operation", operation="ListLanguageModels")`).
const MAX_PAGE: usize = 100;

/// Both tests read a whole Region's models back in ONE page, so the ceiling the service
/// enforces has to fit that page. Checked at compile time against the service's own
/// constant rather than restated as a literal.
const _: () = assert!(
    MAX_LANGUAGE_MODELS <= MAX_PAGE,
    "this suite reads the family back in one page, so the ceiling must fit the modelled \
     MaxResults maximum"
);

/// The whole `CLMLanguageCode` enum, from
/// `smithy_query(query="shape", shape="CLMLanguageCode")` — so a fill spans every
/// language a custom language model can be created in, including ones no recorded
/// request used.
const LANGUAGES: &[&str] = &[
    "de-DE", "en-AU", "en-GB", "en-US", "es-US", "hi-IN", "ja-JP",
];

/// The whole `BaseModelName` enum, from
/// `smithy_query(query="shape", shape="BaseModelName")`.
const BASE_MODELS: &[&str] = &["NarrowBand", "WideBand"];

/// The `index`-th create of a fill: a model of a different language and base model each
/// time, and — on some rows — with tuning data or tags, so the records counted against
/// the ceiling are not all of one shape.
fn fill_request(index: usize) -> (String, Value) {
    let name = format!("quota-model-{index:03}");
    let mut request = language_model_request(
        &name,
        LANGUAGES[index % LANGUAGES.len()],
        BASE_MODELS[index % BASE_MODELS.len()],
    );
    if index.is_multiple_of(3) {
        request["InputDataConfig"]["TuningDataS3Uri"] =
            json!("s3://DOC-EXAMPLE-BUCKET/quota-tuning/");
    }
    if index.is_multiple_of(5) {
        request["Tags"] = json!([{"Key": "team", "Value": "speech"}]);
    }
    (name, request)
}

/// The requests this suite offers at a full ceiling: one minimal, one carrying every
/// optional member the create takes, and one whose name is at the modelled 200-character
/// ceiling for `ModelName` — the refusal is about the count held, not about the request.
fn requests_past_the_ceiling() -> Vec<(&'static str, String, Value)> {
    let tuned_name = "quota-model-over-tuned".to_string();
    let mut tuned = language_model_request(&tuned_name, "en-GB", "NarrowBand");
    tuned["InputDataConfig"]["TuningDataS3Uri"] = json!("s3://DOC-EXAMPLE-BUCKET/over-tuning/");
    tuned["Tags"] = json!([{"Key": "env", "Value": "test"}]);
    tuned["EncryptionConfiguration"] =
        json!({"KMSKey": "arn:aws:kms:us-east-1:111122223333:key/abcd-1234"});

    let long_name = "q".repeat(200);

    vec![
        (
            "one past the ceiling, a minimal request",
            "quota-model-over".to_string(),
            language_model_request("quota-model-over", "en-US", "WideBand"),
        ),
        (
            "one past the ceiling, with tuning data, tags and an encryption configuration",
            tuned_name,
            tuned,
        ),
        (
            "one past the ceiling, a name at the modelled 200-character ceiling",
            long_name.clone(),
            language_model_request(&long_name, "ja-JP", "WideBand"),
        ),
    ]
}

/// Create this family's ceiling's worth of models in `region`, asserting every one of
/// them is served and recorded as still training, and answer their names in creation
/// order.
async fn fill_to_the_ceiling(transcribe: &Transcribe, region: Option<&str>) -> Vec<String> {
    let mut names = Vec::new();
    for index in 0..MAX_LANGUAGE_MODELS {
        let (name, request) = fill_request(index);
        let case = format!(
            "model {} of {MAX_LANGUAGE_MODELS} in {}",
            index + 1,
            region.unwrap_or("the default Region")
        );
        let created = transcribe
            .call_in_region("CreateLanguageModel", request, region)
            .await;
        created.ok(&case);
        assert_eq!(
            created.member(&case, "ModelStatus"),
            "IN_PROGRESS",
            "case {case:?}: a model inside the ceiling must be recorded as training, got \
             {:?}",
            created.body
        );
        names.push(name);
    }
    names
}

/// The names `ListLanguageModels` reports in `region`, in one page.
///
/// `pub(crate)`: the delete suite of this same family
/// (`crate::language_model_delete_requires_the_model`) asserts WHICH models a Region
/// holds through the same listing, so it reads it with this rather than keeping a
/// second copy that could drift from it.
pub(crate) async fn stored_names(
    transcribe: &Transcribe,
    region: Option<&str>,
    case: &str,
) -> Vec<String> {
    let listed = transcribe
        .call_in_region(
            "ListLanguageModels",
            json!({"MaxResults": MAX_PAGE}),
            region,
        )
        .await;
    let body = listed.ok(case).clone();
    let mut names: Vec<String> = body["Models"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Models list, got {body:?}"))
        .iter()
        .map(|model| {
            model["ModelName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every listed model names itself, got {model:?}")
                })
                .to_string()
        })
        .collect();
    names.sort();
    names
}

/// A create at a full ceiling is refused with the modelled `LimitExceededException`,
/// whatever the request looks like; the refusal reports the ceiling, hands no name of the
/// caller's back, and stores nothing.
#[tokio::test]
async fn a_create_past_this_familys_ceiling_is_refused_and_stores_nothing() {
    let transcribe = Transcribe::new();
    let mut accepted = fill_to_the_ceiling(&transcribe, None).await;
    accepted.sort();

    for (case, name, request) in requests_past_the_ceiling() {
        let refused = transcribe.call("CreateLanguageModel", request).await;
        refused.refused(case, CLIENT_ERROR_STATUS, QUOTA_CODE);

        let message = refused.error_message(case);
        assert!(
            message.contains(&MAX_LANGUAGE_MODELS.to_string()),
            "case {case:?}: the refusal must report the ceiling it was refused against \
             ({MAX_LANGUAGE_MODELS}), got {message:?}"
        );
        assert!(
            !message.contains(&name),
            "case {case:?}: the refusal must not hand the caller's own name back, got \
             {message:?}"
        );

        // Nothing was recorded for the refused create: the family's own reader holds
        // nothing under that name.
        transcribe
            .call("DescribeLanguageModel", json!({"ModelName": &name}))
            .await
            .refused(case, CLIENT_ERROR_STATUS, ABSENT_CODE);
    }

    assert_eq!(
        stored_names(&transcribe, None, "the family at its ceiling").await,
        accepted,
        "the store must hold exactly the models the ceiling admitted"
    );

    // The count is THIS family's: resources of other families are unaffected by it,
    // even under a name a refused create asked for.
    let elsewhere: Vec<(&str, &str, Value)> = vec![
        (
            "a custom vocabulary while the language models are at their ceiling",
            "CreateVocabulary",
            json!({
                "VocabularyName": "quota-model-over",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
        ),
        (
            "a custom vocabulary filter while the language models are at their ceiling",
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "quota-model-over",
                "LanguageCode": "en-US",
                "Words": ["Aardvark"]
            }),
        ),
        (
            "a Call Analytics category while the language models are at their ceiling",
            "CreateCallAnalyticsCategory",
            json!({
                "CategoryName": "quota-model-over",
                "Rules": [non_talk_time_rule(4_000)]
            }),
        ),
    ];
    for (case, operation, request) in elsewhere {
        transcribe.call(operation, request).await.ok(case);
    }
}

/// The ceiling counts the records of ONE Region in this ONE family, whether or not the
/// service has finished with them, and a delete frees exactly one slot.
#[tokio::test]
async fn the_ceiling_counts_one_regions_records_and_a_delete_frees_one_slot() {
    let transcribe = Transcribe::new();
    let home = "us-west-2";
    // Regions the fill never created anything in, in two forms of Region identifier.
    let elsewhere = ["eu-west-1", "ap-southeast-3"];

    let names = fill_to_the_ceiling(&transcribe, Some(home)).await;
    let (_, over_name, over_request) = requests_past_the_ceiling()
        .into_iter()
        .next()
        .expect("a request past the ceiling");

    let case = format!("one past the ceiling in {home}");
    transcribe
        .call_in_region("CreateLanguageModel", over_request.clone(), Some(home))
        .await
        .refused(&case, CLIENT_ERROR_STATUS, QUOTA_CODE);

    // The very same create is served in a Region holding no model of this family: the
    // ceiling the refusal reports is the serving Region's own.
    for region in elsewhere {
        let case = format!("the first model of {region}");
        let created = transcribe
            .call_in_region("CreateLanguageModel", over_request.clone(), Some(region))
            .await;
        created.ok(&case);
        assert_eq!(
            created.member(&case, "ModelName"),
            over_name,
            "case {case:?}: the answer must report the record just stored, got {:?}",
            created.body
        );
    }

    // Every model of the fill is now past its clock-owed transition, so the service has
    // finished with all of them — and they still count: the ceiling bounds the records
    // held, not the models still training.
    std::thread::sleep(PAST_THE_TRANSITION);
    let case = format!("one past the ceiling in {home}, every model finished");
    transcribe
        .call_in_region("CreateLanguageModel", over_request.clone(), Some(home))
        .await
        .refused(&case, CLIENT_ERROR_STATUS, QUOTA_CODE);

    // One delete frees one slot: the create refused twice is now served, and the next
    // one is refused again.
    let freed = names.first().expect("the fill recorded models").clone();
    transcribe
        .call_in_region(
            "DeleteLanguageModel",
            json!({"ModelName": &freed}),
            Some(home),
        )
        .await
        .ok(&format!("delete {freed} in {home}"));

    let case = format!("the create {freed}'s slot makes room for");
    let created = transcribe
        .call_in_region("CreateLanguageModel", over_request, Some(home))
        .await;
    created.ok(&case);
    assert_eq!(
        created.member(&case, "ModelStatus"),
        "IN_PROGRESS",
        "case {case:?}: the model the freed slot admitted must be recorded as training, \
         got {:?}",
        created.body
    );

    let case = format!("a second create after one delete in {home}");
    transcribe
        .call_in_region(
            "CreateLanguageModel",
            language_model_request("quota-model-second-over", "hi-IN", "NarrowBand"),
            Some(home),
        )
        .await
        .refused(&case, CLIENT_ERROR_STATUS, QUOTA_CODE);

    // The Region holds its ceiling's worth again: the deleted name is gone and the one
    // the freed slot admitted is there.
    let mut expected: Vec<String> = names.into_iter().filter(|name| *name != freed).collect();
    expected.push(over_name);
    expected.sort();
    assert_eq!(
        stored_names(&transcribe, Some(home), &format!("the models {home} holds")).await,
        expected,
        "after one delete and one create the Region must hold its ceiling's worth again"
    );
}
