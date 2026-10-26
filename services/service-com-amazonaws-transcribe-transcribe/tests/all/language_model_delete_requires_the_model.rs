// SPDX-License-Identifier: CC0-1.0

//! THE RULE THIS SUITE IS ABOUT: `DeleteLanguageModel` removes the custom language
//! model the Region serving the call holds under the EXACT name in the request. A name
//! that Region holds no MODEL under — however it comes to hold none — is refused with
//! this service's absent-resource client error ("Your request didn't pass one or more
//! validation tests. This can occur when the entity you're trying to delete doesn't
//! exist …", `BadRequestException`), and the call removes nothing: the delete is not
//! idempotent and never answers an empty success for a model that is not there.
//!
//! The table spans the ways a name can be one this Region holds no model under, which
//! is what the rule generalises over, rather than the spelling of one absent name
//! (`crate::delete_requires_the_entity` spans the name forms the member's
//! `@length(1..200)` / `@pattern(^[0-9a-zA-Z._-]+$)` constraint allows, across every
//! family's delete): never created at all, differing from a held model only in letter
//! case ("custom language model names are case sensitive",
//! `smithy_query(query="operation", operation="DeleteLanguageModel")`), held by a
//! resource of ANOTHER family under that name, and held under that name by another
//! Region only.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`), this family's request builder
//! (`crate::language_models::language_model_request`) and its listing reader
//! (`crate::language_model_quota::stored_names`) rather than standing up a second
//! emulator or restating either.

use serde_json::json;

use crate::language_model_quota::stored_names;
use crate::language_models::{language_model_request, PAST_THE_TRANSITION};
use crate::transcription_jobs::{Reply, Transcribe};

/// The status this service puts on every client error
/// (`crate::scaffolded::wire_status`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The error shape this service's absent-resource refusals carry.
const ABSENT_RESOURCE_SHAPE: &str = "BadRequestException";

/// The word this service's own messages name this family by
/// (`crate::state::KIND_LANGUAGE_MODEL`).
const MODEL_KIND: &str = "language model";

/// How this service words "there is no such custom language model"
/// (`crate::state::not_found_message` for this family's kind): the service's own
/// sentence, naming the KIND of resource and no value from the request.
const ABSENT_MODEL_MESSAGE: &str = "The requested language model couldn't be found. \
     Check the language model name and try your request again.";

/// The Region the calls under test are signed for.
const SERVING_REGION: &str = "us-east-1";

/// A second Region, so a row can hold a model somewhere OTHER than where the delete is
/// served.
const OTHER_REGION: &str = "us-west-2";

/// A model the serving Region holds, spelled in mixed case so the case-sensitivity rows
/// have a held name to differ from.
const MIXED_CASE_MODEL: &str = "MixedCaseModel";

/// A model the serving Region holds, plainly spelled.
const HELD_MODEL: &str = "held.model";

/// A name the serving Region holds a custom VOCABULARY under and no model.
const VOCABULARY_ONLY: &str = "vocabulary-only-name";

/// A name the serving Region holds a custom VOCABULARY FILTER under and no model.
const FILTER_ONLY: &str = "filter-only-name";

/// A name only [`OTHER_REGION`] holds a model under.
const OTHER_REGION_MODEL: &str = "another-regions-model";

/// Create the custom language model `name` in `region`, asserting the create is served.
async fn create_model(transcribe: &Transcribe, case: &str, region: &str, name: &str) {
    transcribe
        .call_in_region(
            "CreateLanguageModel",
            language_model_request(name, "en-US", "WideBand"),
            Some(region),
        )
        .await
        .ok_status(case, 200);
}

/// Assert `reply` is this service's absent-resource refusal for a custom language
/// model: the client-error status, the modelled shape in the body's `__type`, the
/// service's own sentence verbatim, nothing echoed from the request, and no member of a
/// success.
///
/// The status, the modelled shape, the kind the message names and the no-echo check are
/// the cross-family delete suite's own assertion of the same rule
/// (`crate::delete_requires_the_entity::absent_resource_refusal`), reused rather than
/// spelled a second time so the two cannot drift; what is local is the EXACT sentence
/// that helper leaves open and the no-success-member check.
fn absent_model_refusal(reply: &Reply, case: &str, asked_about: &str) {
    crate::delete_requires_the_entity::absent_resource_refusal(
        reply,
        case,
        MODEL_KIND,
        asked_about,
    );
    let message = reply.error_message(case);
    assert_eq!(
        message, ABSENT_MODEL_MESSAGE,
        "case {case:?}: expected this service's absent-model sentence, got {message:?}"
    );
    assert!(
        reply.body.get("ModelName").is_none(),
        "case {case:?}: a refusal carries no member of a success, got {:?}",
        reply.body
    );
}

/// The models, vocabulary and vocabulary filter every row of the refusal table is
/// probed against: two models in the serving Region, one model in another Region, and a
/// vocabulary and a filter whose names no model is held under.
async fn with_the_fixture() -> Transcribe {
    let transcribe = Transcribe::new();

    for name in [HELD_MODEL, MIXED_CASE_MODEL] {
        create_model(
            &transcribe,
            &format!("create the held model {name:?}"),
            SERVING_REGION,
            name,
        )
        .await;
    }
    create_model(
        &transcribe,
        "create the other Region's model",
        OTHER_REGION,
        OTHER_REGION_MODEL,
    )
    .await;
    transcribe
        .call_in_region(
            "CreateVocabulary",
            json!({
                "VocabularyName": VOCABULARY_ONLY,
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark", "Transcribe"]
            }),
            Some(SERVING_REGION),
        )
        .await
        .ok_status("create the vocabulary whose name no model is held under", 200);
    transcribe
        .call_in_region(
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": FILTER_ONLY,
                "LanguageCode": "en-US",
                "Words": ["crud", "bogus"]
            }),
            Some(SERVING_REGION),
        )
        .await
        .ok_status("create the filter whose name no model is held under", 200);

    transcribe
}

/// RULE: a `DeleteLanguageModel` naming something the serving Region holds no custom
/// language model under is refused — for every way the name can be one it holds no
/// model under — and removes nothing, so both held models, the other Region's model and
/// the same-named resources of the other two families are all still there afterwards.
#[tokio::test]
async fn deleting_a_name_this_region_holds_no_model_under_is_refused() {
    let transcribe = with_the_fixture().await;

    // The rows differ along what "this Region holds no model under that name" can mean:
    // nothing was ever created under it; it differs from a held model only in letter
    // case, in either direction and in a single letter; it names a resource of another
    // family; it is held by another Region only.
    let rows: Vec<(&str, String)> = vec![
        (
            "a name nothing was ever created under",
            "never-created-at-all".to_string(),
        ),
        (
            "a held model's name, lowercased",
            MIXED_CASE_MODEL.to_lowercase(),
        ),
        (
            "a held model's name, uppercased",
            MIXED_CASE_MODEL.to_uppercase(),
        ),
        (
            "a held model's name differing in one letter's case",
            "MixedCaseModeL".to_string(),
        ),
        (
            "a name held by a custom vocabulary and by no model",
            VOCABULARY_ONLY.to_string(),
        ),
        (
            "a name held by a custom vocabulary filter and by no model",
            FILTER_ONLY.to_string(),
        ),
        (
            "a name only another Region holds a model under",
            OTHER_REGION_MODEL.to_string(),
        ),
    ];

    let mut expected = vec![HELD_MODEL.to_string(), MIXED_CASE_MODEL.to_string()];
    expected.sort();

    for (case, name) in &rows {
        let reply = transcribe
            .call_in_region(
                "DeleteLanguageModel",
                json!({"ModelName": name}),
                Some(SERVING_REGION),
            )
            .await;
        absent_model_refusal(&reply, case, name);

        // Nothing was removed and nothing was minted: the serving Region still holds
        // exactly the two models it was given, each readable, and the refused name is
        // still not one of them.
        assert_eq!(
            stored_names(&transcribe, Some(SERVING_REGION), case).await,
            expected,
            "case {case:?}: a refused delete must leave the Region's models as they were"
        );
        for held in [HELD_MODEL, MIXED_CASE_MODEL] {
            transcribe
                .call_in_region(
                    "DescribeLanguageModel",
                    json!({"ModelName": held}),
                    Some(SERVING_REGION),
                )
                .await
                .ok(&format!("{case} / {held} is still readable"));
        }
        transcribe
            .call_in_region(
                "DescribeLanguageModel",
                json!({"ModelName": OTHER_REGION_MODEL}),
                Some(OTHER_REGION),
            )
            .await
            .ok(&format!("{case} / the other Region's model is untouched"));
        transcribe
            .call_in_region(
                "GetVocabulary",
                json!({"VocabularyName": VOCABULARY_ONLY}),
                Some(SERVING_REGION),
            )
            .await
            .ok(&format!("{case} / the same-named vocabulary is untouched"));
        transcribe
            .call_in_region(
                "GetVocabularyFilter",
                json!({"VocabularyFilterName": FILTER_ONLY}),
                Some(SERVING_REGION),
            )
            .await
            .ok(&format!("{case} / the same-named filter is untouched"));
    }
}

/// RULE, the other half: the EXACT name in the serving Region removes exactly that
/// model, once. Every neighbour — a model whose name differs only in case, an unrelated
/// model, the same name in another Region — survives it, and the second delete of the
/// same name is the absent-resource refusal.
#[tokio::test]
async fn the_exact_name_removes_exactly_that_model_once() {
    let transcribe = Transcribe::new();

    // Two names differing only in case are two models: the create of the second is a
    // success, not a name conflict.
    let case = "two models whose names differ only in case";
    for name in ["alpha.one", "Alpha.One", "beta-two"] {
        create_model(
            &transcribe,
            &format!("{case} / create {name:?}"),
            SERVING_REGION,
            name,
        )
        .await;
    }
    create_model(
        &transcribe,
        &format!("{case} / create the other Region's model"),
        OTHER_REGION,
        "alpha.one",
    )
    .await;

    // A model the service has not finished with is refused for its STATE, not its
    // absence (`crate::language_models` pins that refusal), so the delete under test is
    // made once every row's clock-owed transition has fallen due.
    std::thread::sleep(PAST_THE_TRANSITION);

    let deleted = transcribe
        .call_in_region(
            "DeleteLanguageModel",
            json!({"ModelName": "alpha.one"}),
            Some(SERVING_REGION),
        )
        .await;
    deleted.ok_status(case, 200);
    crate::vocabulary_delete_and_tag_sets::empty_body(&deleted, case);

    // Exactly that one model is gone from the serving Region.
    transcribe
        .call_in_region(
            "DescribeLanguageModel",
            json!({"ModelName": "alpha.one"}),
            Some(SERVING_REGION),
        )
        .await
        .refused(case, CLIENT_ERROR_STATUS, ABSENT_RESOURCE_SHAPE);
    assert_eq!(
        stored_names(&transcribe, Some(SERVING_REGION), case).await,
        vec!["Alpha.One".to_string(), "beta-two".to_string()],
        "case {case:?}: only the exactly named model may be removed"
    );
    for (region, name) in [
        (SERVING_REGION, "Alpha.One"),
        (SERVING_REGION, "beta-two"),
        (OTHER_REGION, "alpha.one"),
    ] {
        transcribe
            .call_in_region(
                "DescribeLanguageModel",
                json!({"ModelName": name}),
                Some(region),
            )
            .await
            .ok(&format!("{case} / {name:?} in {region} survives"));
    }

    // And the delete does not repeat: the name it removed is now one this Region holds
    // no model under. (That the freed name can be CREATED again is pinned by
    // `crate::language_models::a_language_model_name_is_claimed_once_and_a_delete_frees_it`.)
    let again = format!("{case} / delete #2");
    let reply = transcribe
        .call_in_region(
            "DeleteLanguageModel",
            json!({"ModelName": "alpha.one"}),
            Some(SERVING_REGION),
        )
        .await;
    absent_model_refusal(&reply, &again, "alpha.one");
}
