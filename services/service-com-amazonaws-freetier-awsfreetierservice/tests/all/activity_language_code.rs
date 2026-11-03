// SPDX-License-Identifier: CC0-1.0

//! What `languageCode` DOES on the activity readers, driven over the wire
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSFreeTierService.<Operation>`)
//! through the same multi-service router the emulator boots.
//!
//! The rule, from the model: `GetAccountActivityRequest$languageCode` is "the
//! language code used to return translated title and description fields" and
//! `ListAccountActivitiesRequest$languageCode` the same for the title, so the
//! text a reader reports is a function of the STORED row and the requested
//! code — and of nothing else. Three consequences this suite pins, over the
//! model's whole `LanguageCode` value set and the whole catalog rather than one
//! code and one row:
//!
//! 1. The code SELECTS text: a code naming a language the row is translated
//!    into reports that translation, so two codes in different languages do not
//!    report the same title and description.
//! 2. The code selects NOTHING ELSE: every other member of the detail view
//!    (`activityId`, `status`, `instructionsUrl`, `reward`, the timestamps, the
//!    estimate) is byte-identical whatever the code, because those members are
//!    not translated text.
//! 3. A read is a read: asking twice in one language, and asking again after a
//!    request in another language or a refused one, reports the same text.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::ops::types::LANGUAGE_CODE_VALUES;
use service_com_amazonaws_freetier_awsfreetierservice::state::{
    primary_subtag, ACTIVITIES_MAX_RESULTS_CEILING,
};
use service_com_amazonaws_freetier_awsfreetierservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The language a request that names no `languageCode` is answered in: the
/// modelled code this service holds its catalog's base text in
/// (`crate::state::DEFAULT_LANGUAGE_CODE`).
const BASE_LANGUAGE_CODE: &str = "en-US";

/// The largest page `ListAccountActivities` serves
/// (`crate::state::ACTIVITIES_MAX_RESULTS_CEILING`), so one call reports the
/// whole catalog.
const PAGE_SIZE_CEILING: i32 = ACTIVITIES_MAX_RESULTS_CEILING;

/// The members of the detail view that `languageCode` is documented to select,
/// and so the only ones allowed to differ between two codes. Everything else
/// the output models (`GetAccountActivityOutput`: `activityId`, `status`,
/// `instructionsUrl`, `reward`, `estimatedTimeToCompleteInMinutes`,
/// `expiresAt`, `startedAt`, `completedAt`) must not.
const TRANSLATED_MEMBERS: [&str; 2] = ["title", "description"];

/// This service mounted on one shared engine — the same assembly
/// `core/router` performs, minus the other services.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // In-process tests sign with a placeholder key; signature verification is a
    // front-door setting, not part of what these tests are about.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router
}

/// Dispatch one operation and report its HTTP status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://freetier.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSFreeTierService.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEMULATOR/20260101/us-east-1/freetier/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0000",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Dispatch one operation, requiring success.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {payload}"
    );
    payload
}

/// The string at `member`, required to be present.
fn text(payload: &Value, member: &str) -> String {
    payload[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected a string at {member}, got {payload}"))
        .to_string()
}

/// The detail view of `activity_id`; `None` names no language at all.
async fn detail(
    router: &MultiServiceRouter,
    activity_id: &str,
    language_code: Option<&str>,
) -> Value {
    let mut body = json!({"activityId": activity_id});
    if let Some(code) = language_code {
        body["languageCode"] = json!(code);
    }
    ok(router, "GetAccountActivity", &body).await
}

/// Every activity id the catalog holds.
async fn activity_ids(router: &MultiServiceRouter) -> Vec<String> {
    ok(router, "ListAccountActivities", &json!({"maxResults": PAGE_SIZE_CEILING}))
        .await["activities"]
        .as_array()
        .expect("activities is a list")
        .iter()
        .map(|activity| text(activity, "activityId"))
        .collect()
}

/// The title `ListAccountActivities` reports for each activity in
/// `language_code`, keyed by id.
async fn listed_titles(
    router: &MultiServiceRouter,
    language_code: Option<&str>,
) -> BTreeMap<String, String> {
    let mut body = json!({"maxResults": PAGE_SIZE_CEILING});
    if let Some(code) = language_code {
        body["languageCode"] = json!(code);
    }
    ok(router, "ListAccountActivities", &body).await["activities"]
        .as_array()
        .expect("activities is a list")
        .iter()
        .map(|activity| (text(activity, "activityId"), text(activity, "title")))
        .collect()
}

/// The response with its translated members removed: what must not vary with
/// the requested language.
fn untranslated(payload: &Value) -> Value {
    let mut stripped = payload.clone();
    let object = stripped
        .as_object_mut()
        .unwrap_or_else(|| panic!("the detail view is a structure, got {payload}"));
    for member in TRANSLATED_MEMBERS {
        object.remove(member);
    }
    stripped
}

/// `GetAccountActivity$languageCode` selects the language of the reported
/// `title` and `description`, and selects nothing else about the response.
///
/// The table spans the model's WHOLE `LanguageCode` value set
/// (`ops::types::LANGUAGE_CODE_VALUES`, generated from the Smithy enum) plus the
/// row that names no code, over every activity in the catalog.
#[tokio::test]
async fn detail_text_is_reported_in_the_requested_language() {
    let router = router();
    let ids = activity_ids(&router).await;
    assert!(
        !ids.is_empty(),
        "case catalog: the account holds activities to read in each language"
    );
    assert!(
        LANGUAGE_CODE_VALUES.contains(&BASE_LANGUAGE_CODE),
        "case base language: {BASE_LANGUAGE_CODE} must be a modelled LanguageCode, got \
         {LANGUAGE_CODE_VALUES:?}"
    );

    for id in &ids {
        let base = detail(&router, id, Some(BASE_LANGUAGE_CODE)).await;

        // A request that names no code is answered in the service's base
        // language: `languageCode` is optional and the model names no other
        // default.
        let unspecified = detail(&router, id, None).await;
        assert_eq!(
            unspecified, base,
            "case {id} no languageCode: a request that names no code must be answered in the \
             base language, got {unspecified} against {base}"
        );

        // (code, (title, description)) for every modelled code, so the rows can
        // be compared against each other below.
        let mut rendered: BTreeMap<&str, (String, String)> = BTreeMap::new();
        for code in LANGUAGE_CODE_VALUES {
            let view = detail(&router, id, Some(code)).await;

            // The code does not change the identity, status or any other
            // untranslated member of the activity.
            assert_eq!(
                untranslated(&view),
                untranslated(&base),
                "case {id}/{code}: only title and description may vary with the language code, \
                 got {view} against {base}"
            );

            let title = text(&view, "title");
            let description = text(&view, "description");
            for (member, value) in TRANSLATED_MEMBERS.iter().zip([&title, &description]) {
                // `GenericString`: len 0..1024. Required members, so present,
                // and this catalog's text is never empty.
                assert!(
                    !value.is_empty() && value.chars().count() <= 1024,
                    "case {id}/{code}: {member} must be non-empty and at most 1024 characters, \
                     got {value:?}"
                );
            }

            // A read reports stored text: asking the same question twice gets
            // the same answer, after a question in another language too.
            let again = detail(&router, id, Some(code)).await;
            assert_eq!(
                again, view,
                "case {id}/{code}: the reported text must depend only on the row and the code, \
                 got {again} then {view}"
            );

            // The two readers read the SAME localized row, so the summary's
            // title is the detail view's title in the language asked for.
            let listed = listed_titles(&router, Some(code)).await;
            assert_eq!(
                listed.get(id),
                Some(&title),
                "case {id}/{code}: the list must report the detail view's title in the requested \
                 language, got {listed:?}"
            );

            rendered.insert(code, (title, description));
        }

        // Codes naming DIFFERENT languages report different text; codes naming
        // the same language (`en-GB` and `en-US`) agree, because the catalog
        // holds one text per language rather than one per code.
        for (code, rendering) in &rendered {
            for (other, other_rendering) in &rendered {
                if code >= other {
                    continue;
                }
                if primary_subtag(code) == primary_subtag(other) {
                    continue;
                }
                assert_ne!(
                    rendering, other_rendering,
                    "case {id}/{code} vs {other}: two codes naming different languages must not \
                     report the same title and description, got {rendering:?} for both"
                );
            }
        }

        // One rendering per language, so no code collapses onto another
        // language's text: the catalog holds a translation for every modelled
        // language.
        let languages: BTreeSet<&str> = LANGUAGE_CODE_VALUES
            .iter()
            .map(|code| primary_subtag(code))
            .collect();
        let renderings: BTreeSet<&(String, String)> = rendered.values().collect();
        assert!(
            renderings.len() >= languages.len(),
            "case {id} distinct renderings: expected at least {} distinct renderings over the {} \
             modelled codes, got {}",
            languages.len(),
            LANGUAGE_CODE_VALUES.len(),
            renderings.len()
        );
    }
}

/// A `languageCode` the model's enum does not declare names no language the
/// catalog could be read in: the request is refused with the operation's
/// modelled `ValidationException` (HTTP 400), and refusing it changes nothing a
/// later reader sees.
#[tokio::test]
async fn an_unmodelled_language_code_is_refused_and_changes_nothing() {
    let router = router();
    let ids = activity_ids(&router).await;
    let id = ids.first().expect("the account holds activities").clone();
    let before = detail(&router, &id, Some(BASE_LANGUAGE_CODE)).await;
    let listed_before = listed_titles(&router, Some("ja-JP")).await;

    // Rows differ in HOW the code misses the enum: wrong separator, wrong case,
    // the primary subtag alone, an unmodelled region, an unmodelled language,
    // the empty string, and a code with trailing space.
    for code in [
        "en_US",
        "EN-US",
        "en",
        "en-AU",
        "sv-SE",
        "",
        "en-US ",
        "ja-JP;en-US",
    ] {
        assert!(
            !LANGUAGE_CODE_VALUES.contains(&code),
            "case {code:?}: the probe must not be a modelled LanguageCode"
        );
        for (operation, body) in [
            (
                "GetAccountActivity",
                json!({"activityId": id, "languageCode": code}),
            ),
            ("ListAccountActivities", json!({"languageCode": code})),
        ] {
            let (status, payload) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {operation} {code:?}: expected HTTP 400, got {status} with {payload}"
            );
            let error_code = payload["__type"]
                .as_str()
                .unwrap_or_default()
                .rsplit(['#', ':'])
                .next()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                error_code, "ValidationException",
                "case {operation} {code:?}: expected a ValidationException, got {payload}"
            );
        }
    }

    let after = detail(&router, &id, Some(BASE_LANGUAGE_CODE)).await;
    assert_eq!(
        after, before,
        "case after refusals: a refused language code must leave the catalog unchanged, got \
         {after} against {before}"
    );
    let listed_after = listed_titles(&router, Some("ja-JP")).await;
    assert_eq!(
        listed_after, listed_before,
        "case after refusals: the listed text must be unchanged, got {listed_after:?} against \
         {listed_before:?}"
    );
}
