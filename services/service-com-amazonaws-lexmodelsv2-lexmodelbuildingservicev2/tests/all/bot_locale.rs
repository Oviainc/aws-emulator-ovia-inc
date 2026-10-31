// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Lex Models V2 bot locale slice: `CreateBotLocale` and
//! `DescribeBotLocale`.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator
//! serves on its socket, so the restJson1 routing, the modelled constraints and the
//! error envelopes are exercised exactly as a client sees them. The harness itself is
//! the bot lifecycle suite's, reused rather than copied.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a create stores the locale under the bot, bot version and locale id the request
//!   names, so a read of that triple reports back what was stored;
//! * the locale name reported is the one the supported-locales table carries for the
//!   id, for every supported locale;
//! * a locale that was saved and not built settles from `Creating` to `NotBuilt`;
//! * a locale id the service does not support, or a value outside a modelled enum or a
//!   published bound, is refused and stores nothing;
//! * a locale id already defined on that bot version is refused as a conflict, and the
//!   stored locale is left alone;
//! * a locale belongs to one version of one bot: no other triple reads it, and deleting
//!   the bot takes its locales with it.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;

use crate::bot_lifecycle::{
    call, create_body, create_bot, emulator, error_code, member, seconds, text,
};

/// The members `CreateBotLocale` accepts that `DescribeBotLocale` reports back
/// unchanged. `nluIntentConfidenceThreshold` is compared numerically instead, so that a
/// double rendered as `1` and one rendered as `1.0` still compare equal, and
/// `speechDetectionSensitivity` through [`expected_sensitivity`], because a create that
/// names no level is answered with the documented standard one.
const ECHOED_BY_DESCRIBE: [&str; 8] = [
    "localeId",
    "description",
    "voiceSettings",
    "unifiedSpeechSettings",
    "audioFillerSettings",
    "speechRecognitionSettings",
    "generativeAISettings",
    "speakerDiarizationSettings",
];

/// The sensitivity level a response must report: the one the request named, or the
/// documented standard level when the request named none.
fn expected_sensitivity(request: &Value) -> Value {
    match member(request, "speechDetectionSensitivity") {
        Value::Null => Value::String(state::DEFAULT_SPEECH_DETECTION_SENSITIVITY.to_string()),
        named => named,
    }
}

/// A foundation-model ARN satisfying the modelled `BedrockModelArn` pattern
/// (`smithy_query(query="shape",shape="SpeechFoundationModel")`).
const FOUNDATION_MODEL_ARN: &str =
    "arn:aws:bedrock:us-east-1::foundation-model/amazon.nova-sonic-v1:0";

/// `CreateBotLocale`'s URI: `PUT /bots/{botId}/botversions/{botVersion}/botlocales`.
pub(crate) fn locales_uri(bot_id: &str, bot_version: &str) -> String {
    format!("/bots/{bot_id}/botversions/{bot_version}/botlocales")
}

/// `DescribeBotLocale`'s URI:
/// `GET /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}`.
pub(crate) fn locale_uri(bot_id: &str, bot_version: &str, locale_id: &str) -> String {
    format!("/bots/{bot_id}/botversions/{bot_version}/botlocales/{locale_id}")
}

/// A `CreateBotLocale` request body carrying the required members plus `extra`.
pub(crate) fn locale_body(locale_id: &str, threshold: f64, extra: Value) -> Value {
    let mut body = json!({
        "localeId": locale_id,
        "nluIntentConfidenceThreshold": threshold,
    });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (key, value) in extra.as_object().expect("extras are a JSON object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

/// A numeric response member, compared as a double whatever the wire rendering.
fn number(body: &Value, name: &str) -> f64 {
    member(body, name).as_f64().unwrap_or(f64::NAN)
}

/// Create a bot to hang locales off, and return its minted id.
pub(crate) async fn host_bot(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> String {
    create_bot(router, &create_body(name, 300, json!({}))).await
}

/// The table every round-trip assertion runs over: rows differ in the locale id, in the
/// confidence threshold (including both published bounds), in which optional members are
/// set and in the sensitivity level chosen.
fn locale_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required members",
            locale_body("en_US", 0.4, json!({})),
        ),
        (
            "the confidence threshold at the published minimum",
            locale_body("de_DE", 0.0, json!({})),
        ),
        (
            "the confidence threshold at the published maximum",
            locale_body("ja_JP", 1.0, json!({})),
        ),
        (
            "a locale id whose region subtag is numeric",
            locale_body(
                "es_419",
                0.75,
                json!({ "description": "a locale with a numeric region subtag" }),
            ),
        ),
        (
            "every optional member set",
            locale_body(
                "fr_CA",
                0.5,
                json!({
                    "description": "a locale with every optional member set",
                    "voiceSettings": { "voiceId": "Gabrielle", "engine": "neural" },
                    "unifiedSpeechSettings": {
                        "speechFoundationModel": {
                            "modelArn": FOUNDATION_MODEL_ARN,
                            "voiceId": "Gabrielle",
                        },
                    },
                    "audioFillerSettings": {
                        "enabled": true,
                        "startDelayInMilliseconds": 500,
                        "minimumPlayDurationInMilliseconds": 1_000,
                        "responseDeliveryDelayInMilliseconds": 200,
                    },
                    "speechRecognitionSettings": { "speechModelPreference": "Standard" },
                    "generativeAISettings": {
                        "runtimeSettings": {
                            "slotResolutionImprovement": { "enabled": true },
                        },
                    },
                    "speechDetectionSensitivity": "HighNoiseTolerance",
                    "speakerDiarizationSettings": { "enabled": true },
                }),
            ),
        ),
        (
            "a description at the published maximum length",
            locale_body(
                "pt_BR",
                0.2,
                json!({ "description": "d".repeat(2_000) }),
            ),
        ),
        (
            "the default sensitivity level",
            locale_body(
                "ko_KR",
                0.9,
                json!({ "speechDetectionSensitivity": "Default" }),
            ),
        ),
        (
            "the maximum sensitivity level",
            locale_body(
                "zh_CN",
                0.65,
                json!({ "speechDetectionSensitivity": "MaximumNoiseTolerance" }),
            ),
        ),
    ]
}

#[tokio::test]
async fn a_created_bot_locale_is_stored_and_read_back_through_its_own_triple() {
    let router = emulator();
    let bot_id = host_bot(&router, "LocaleRoundTripBot").await;

    let mut created_locales: Vec<(String, f64)> = Vec::new();

    for (case, request) in locale_cases() {
        let (status, created) = call(
            &router,
            "PUT",
            &locales_uri(&bot_id, "DRAFT"),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected CreateBotLocale to answer 202, got {status} {created:?}"
        );
        let locale_id = text(&request, "localeId");
        assert_eq!(
            text(&created, "localeId"),
            locale_id,
            "case {case:?}: CreateBotLocale must report the localeId it stored, got {created:?}"
        );
        assert_eq!(
            text(&created, "botId"),
            bot_id,
            "case {case:?}: CreateBotLocale must report the bot it was called on, got {created:?}"
        );
        assert_eq!(
            text(&created, "botVersion"),
            "DRAFT",
            "case {case:?}: CreateBotLocale must report the bot version, got {created:?}"
        );
        assert_eq!(
            text(&created, "botLocaleStatus"),
            "Creating",
            "case {case:?}: a locale AWS reports as Creating must not start settled: {created:?}"
        );
        let expected_name = state::locale_name(&locale_id)
            .unwrap_or_else(|| panic!("case {case:?}: {locale_id} is a supported locale"));
        assert_eq!(
            text(&created, "localeName"),
            expected_name,
            "case {case:?}: expected localeName {expected_name:?}, got {created:?}"
        );
        let created_at = seconds(&created, "creationDateTime");
        assert!(
            created_at > 0.0,
            "case {case:?}: creationDateTime must be recorded, got {created:?}"
        );

        // READ BACK — the stored record, member for member.
        let (status, described) =
            call(&router, "GET", &locale_uri(&bot_id, "DRAFT", &locale_id), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBotLocale to answer 200, got {status} {described:?}"
        );
        for name in ECHOED_BY_DESCRIBE {
            assert_eq!(
                member(&described, name),
                member(&request, name),
                "case {case:?}: DescribeBotLocale must report the stored {name}, got {described:?}"
            );
        }
        // A create that named no sensitivity level stored the documented standard one,
        // and both the create and the read report that stored level.
        for (reporter, reported) in [("CreateBotLocale", &created), ("DescribeBotLocale", &described)]
        {
            assert_eq!(
                member(reported, "speechDetectionSensitivity"),
                expected_sensitivity(&request),
                "case {case:?}: {reporter} must report the stored sensitivity level, got \
                 {reported:?}"
            );
        }
        let threshold = number(&request, "nluIntentConfidenceThreshold");
        assert!(
            (number(&described, "nluIntentConfidenceThreshold") - threshold).abs() < 1e-9,
            "case {case:?}: expected nluIntentConfidenceThreshold {threshold}, got {described:?}"
        );
        assert_eq!(
            text(&described, "localeName"),
            expected_name,
            "case {case:?}: DescribeBotLocale must report the stored localeName, got {described:?}"
        );
        assert_eq!(
            text(&described, "botId"),
            bot_id,
            "case {case:?}: DescribeBotLocale must report the bot it was asked about"
        );
        assert!(
            (seconds(&described, "creationDateTime") - created_at).abs() < 1e-3,
            "case {case:?}: creationDateTime must be the stored one, got {described:?}"
        );
        assert!(
            seconds(&described, "lastUpdatedDateTime") >= created_at,
            "case {case:?}: lastUpdatedDateTime must not precede creationDateTime, got {described:?}"
        );
        assert_eq!(
            member(&described, "lastBuildSubmittedDateTime"),
            Value::Null,
            "case {case:?}: a locale never submitted for building has no build timestamp"
        );

        created_locales.push((locale_id, threshold));
    }

    // Each locale is its own record: creating later ones changed none of the earlier.
    for (locale_id, threshold) in &created_locales {
        let (status, described) =
            call(&router, "GET", &locale_uri(&bot_id, "DRAFT", locale_id), None).await;
        assert_eq!(
            status, 200,
            "locale {locale_id}: every locale of the bot stays readable, got {status} {described:?}"
        );
        assert!(
            (number(&described, "nluIntentConfidenceThreshold") - threshold).abs() < 1e-9,
            "locale {locale_id}: expected the threshold {threshold} it was created with, got {described:?}"
        );
    }
}

#[tokio::test]
async fn every_supported_locale_is_accepted_under_its_published_name() {
    let router = emulator();
    let bot_id = host_bot(&router, "AllLocalesBot").await;

    for (locale_id, locale_name) in state::SUPPORTED_LOCALES {
        let (status, created) = call(
            &router,
            "PUT",
            &locales_uri(&bot_id, "DRAFT"),
            Some(locale_body(locale_id, 0.4, json!({}))),
        )
        .await;
        assert_eq!(
            status, 202,
            "locale {locale_id}: expected 202, got {status} {created:?}"
        );
        assert_eq!(
            text(&created, "localeName"),
            locale_name,
            "locale {locale_id}: expected localeName {locale_name:?}, got {created:?}"
        );

        let (status, described) =
            call(&router, "GET", &locale_uri(&bot_id, "DRAFT", locale_id), None).await;
        assert_eq!(
            status, 200,
            "locale {locale_id}: expected DescribeBotLocale to answer 200, got {status} {described:?}"
        );
        assert_eq!(
            text(&described, "localeName"),
            locale_name,
            "locale {locale_id}: DescribeBotLocale must report the same name, got {described:?}"
        );
    }

    // The table carries no duplicate id and no duplicate name, so one locale id names
    // exactly one locale.
    let ids: BTreeSet<&str> = state::SUPPORTED_LOCALES.iter().map(|(id, _)| *id).collect();
    assert_eq!(
        ids.len(),
        state::SUPPORTED_LOCALES.len(),
        "the supported-locale table must not repeat a locale id"
    );
}

#[tokio::test]
async fn a_saved_locale_settles_from_creating_to_not_built() {
    let router = emulator();
    let bot_id = host_bot(&router, "SettlingLocaleBot").await;

    let (_, created) = call(
        &router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(locale_body("en_GB", 0.4, json!({}))),
    )
    .await;
    assert_eq!(
        text(&created, "botLocaleStatus"),
        "Creating",
        "the create itself reports the transient status, got {created:?}"
    );

    // Straight after the create the caller still observes the transient status.
    let (_, described) = call(&router, "GET", &locale_uri(&bot_id, "DRAFT", "en_GB"), None).await;
    assert_eq!(
        text(&described, "botLocaleStatus"),
        "Creating",
        "a locale read back immediately must still be Creating, got {described:?}"
    );

    // Past the due instant the transition the clock owed is materialised: a locale that
    // was saved and never built is NotBuilt.
    tokio::time::sleep(state::CREATION_SETTLES_AFTER + Duration::from_millis(150)).await;
    let (_, described) = call(&router, "GET", &locale_uri(&bot_id, "DRAFT", "en_GB"), None).await;
    assert_eq!(
        text(&described, "botLocaleStatus"),
        "NotBuilt",
        "a saved but unbuilt locale past its due instant must be NotBuilt, got {described:?}"
    );
}

#[tokio::test]
async fn requests_outside_the_accepted_values_are_refused_and_store_nothing() {
    let router = emulator();
    let bot_id = host_bot(&router, "RefusingLocaleBot").await;

    // Each row is refused as a 400 `ValidationException` rather than stored: a locale
    // the service does not support, a value outside a modelled enum, and a confidence
    // threshold past each published bound.
    let cases: Vec<(&str, Value)> = vec![
        ("a locale id no supported locale carries", locale_body("xx_XX", 0.4, json!({}))),
        ("a locale id in BCP 47 rather than Lex form", locale_body("en-US", 0.4, json!({}))),
        ("a locale id differing only in case", locale_body("EN_US", 0.4, json!({}))),
        (
            "a confidence threshold below the published minimum",
            locale_body("en_US", -0.1, json!({})),
        ),
        (
            "a confidence threshold above the published maximum",
            locale_body("en_US", 1.1, json!({})),
        ),
        (
            "a sensitivity level outside the modelled enum",
            locale_body("en_US", 0.4, json!({ "speechDetectionSensitivity": "Highest" })),
        ),
    ];

    for (case, request) in &cases {
        let (status, response) = call(
            &router,
            "PUT",
            &locales_uri(&bot_id, "DRAFT"),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
    }

    // None of the refusals stored a locale, under the id it named or under any other.
    for (case, request) in &cases {
        let locale_id = text(request, "localeId");
        let (status, response) =
            call(&router, "GET", &locale_uri(&bot_id, "DRAFT", &locale_id), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must store nothing, got {status} {response:?}"
        );
    }
}

#[tokio::test]
async fn a_locale_is_created_once_per_bot_version_and_only_on_a_bot_that_exists() {
    let router = emulator();
    let bot_id = host_bot(&router, "ConflictingLocaleBot").await;
    let other_bot_id = host_bot(&router, "SecondLocaleBot").await;

    let first = locale_body("en_US", 0.4, json!({ "description": "the stored locale" }));
    let (status, _) = call(
        &router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(first.clone()),
    )
    .await;
    assert_eq!(status, 202, "the first create of a locale must succeed");

    // A second create of the same locale on the same bot version conflicts.
    let second = locale_body("en_US", 0.9, json!({ "description": "the losing locale" }));
    let (status, response) = call(
        &router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(second.clone()),
    )
    .await;
    assert_eq!(
        status, 409,
        "a duplicate localeId must be refused, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ConflictException",
        "expected ConflictException for a duplicate localeId, got {response:?}"
    );

    // And the refusal left the stored locale as it was.
    let (_, described) = call(&router, "GET", &locale_uri(&bot_id, "DRAFT", "en_US"), None).await;
    assert_eq!(
        text(&described, "description"),
        "the stored locale",
        "the refused create must not have overwritten the stored locale, got {described:?}"
    );

    // The same locale id on ANOTHER bot is a different locale, so it is accepted.
    let (status, created) = call(
        &router,
        "PUT",
        &locales_uri(&other_bot_id, "DRAFT"),
        Some(second),
    )
    .await;
    assert_eq!(
        status, 202,
        "the same localeId on another bot must be accepted, got {status} {created:?}"
    );
    assert_eq!(
        text(&created, "botId"),
        other_bot_id,
        "the second locale belongs to the bot it was created on, got {created:?}"
    );

    // A bot that does not exist cannot take a locale.
    let (status, response) = call(
        &router,
        "PUT",
        &locales_uri("ABCDEFGHIJ", "DRAFT"),
        Some(first),
    )
    .await;
    assert_eq!(
        status, 412,
        "a locale for an unknown bot must be refused, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "PreconditionFailedException",
        "expected PreconditionFailedException for an unknown bot, got {response:?}"
    );
}

#[tokio::test]
async fn a_locale_is_reachable_only_under_its_own_bot_version_and_id() {
    let router = emulator();
    let bot_id = host_bot(&router, "ScopedLocaleBot").await;
    let other_bot_id = host_bot(&router, "OtherScopedLocaleBot").await;

    let (status, _) = call(
        &router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(locale_body("it_IT", 0.4, json!({}))),
    )
    .await;
    assert_eq!(status, 202, "the create must succeed");

    // Every triple but its own misses: another bot, a numbered version, another locale.
    let misses: [(&str, String); 3] = [
        ("another bot", locale_uri(&other_bot_id, "DRAFT", "it_IT")),
        ("a numbered bot version", locale_uri(&bot_id, "1", "it_IT")),
        ("a locale never created", locale_uri(&bot_id, "DRAFT", "nl_NL")),
    ];
    for (case, uri) in misses {
        let (status, response) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected DescribeBotLocale to answer 404, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response:?}"
        );
    }

    // A locale is defined inside its bot, so deleting the bot takes the locale too.
    let (status, _) = call(&router, "DELETE", &format!("/bots/{bot_id}"), None).await;
    assert_eq!(status, 202, "deleting the host bot must succeed");
    let (status, response) =
        call(&router, "GET", &locale_uri(&bot_id, "DRAFT", "it_IT"), None).await;
    assert_eq!(
        status, 404,
        "a deleted bot's locale must be gone, got {status} {response:?}"
    );
}
