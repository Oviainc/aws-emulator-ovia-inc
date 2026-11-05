// SPDX-License-Identifier: CC0-1.0

//! A configuration profile's `Type` is fixed when the profile is created, and
//! `UpdateConfigurationProfile` refuses a request that asks it to become a different one.
//!
//! The rule under test: a request whose body names a `Type` the addressed profile is not
//! is refused WHOLE — the modelled `BadRequestException` (HTTP 400) with AppConfig's
//! Type-specific sentence — and NONE of the members that request carried is applied, so a
//! legitimate `Description` submitted beside the smuggled kind does not land either. A
//! body naming the kind the profile already is asks for no change and is served as a
//! request that carried no such key.
//!
//! `UpdateConfigurationProfileRequest` declares no `Type` member
//! (`smithy_query(query="operation",operation="UpdateConfigurationProfile")`), so an SDK
//! cannot send one and only a hand-built request reaches this rule; the rows below are
//! raw JSON bodies for exactly that reason.
//!
//! The table spans what the rule generalises over: which kind the profile was created as
//! (each value the model's `ConfigurationProfile$Type` documentation lists, plus a profile
//! created with no `Type` at all, which AppConfig stores as the freeform kind), whether
//! the body names the same kind or another one, how the key arrives (a string, a
//! differently cased string, a kind no AppConfig profile has, a non-string JSON value,
//! `null`), and whether the same request also carries a member the operation does model.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::{
    FEATURE_FLAGS_PROFILE_TYPE, FREEFORM_PROFILE_TYPE,
};

use crate::appconfig_workflow::{
    create_application, create_profile_from, json_call, router, Answer,
};

/// The sentence AppConfig refuses such a request with, as the recorded refusal words it.
const IMMUTABLE_TYPE_MESSAGE: &str =
    "Changing the Type of this configuration profile is not supported.";

/// The `Description` every profile below is created with, so a refused update is visible
/// as the description NOT having moved.
const CREATED_DESCRIPTION: &str = "the profile as created";

/// The `Description` every patch below asks for: a member the operation does model, so an
/// accepted request applies it and a refused one must not.
const PATCHED_DESCRIPTION: &str = "the description the patch asked for";

/// `PATCH` a profile with a raw body, which is the only way a `Type` key reaches the
/// service: no SDK models the member.
async fn update_profile(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    patch: &Value,
) -> Answer {
    json_call(
        router,
        "PATCH",
        &format!("/applications/{application}/configurationprofiles/{profile}"),
        patch,
    )
    .await
}

/// `GET` a profile, the implemented reader every row reads its update back through.
async fn get_profile(router: &MultiServiceRouter, application: &str, profile: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/configurationprofiles/{profile}"),
        &Value::Null,
    )
    .await
}

/// The profile one row starts from: a hosted profile created as `created_type` (or with no
/// `Type` at all when that is `None`), carrying [`CREATED_DESCRIPTION`]. Answers its id.
async fn create_profile_of(
    router: &MultiServiceRouter,
    application: &str,
    case: &str,
    created_type: Option<&str>,
) -> String {
    let mut body = json!({
        "Name": format!("profile-{}", case.replace(' ', "-")),
        "LocationUri": "hosted",
        "Description": CREATED_DESCRIPTION,
    });
    if let Some(created_type) = created_type {
        body["Type"] = Value::from(created_type);
    }
    let created = create_profile_from(router, application, case, &body).await;
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: the created profile must carry an Id"))
}

/// One row: what a failing assertion calls it, the `Type` the profile was created with,
/// the value the patch carries under `Type`, and whether that asks for a different kind —
/// which is what decides the refusal.
struct Row {
    case: &'static str,
    created_type: Option<&'static str>,
    submitted_type: Value,
    refused: bool,
}

/// Every patch names the profile's kind, and the ones naming another kind are refused
/// whole: nothing the request carried is applied, and the profile keeps the kind and the
/// description it had. The accepted ones apply the member the operation models and leave
/// the kind where it was.
#[tokio::test]
async fn an_update_naming_another_type_is_refused_and_applies_nothing() {
    let router = router();
    let application = create_application(&router, "immutable-type").await;

    // The two kinds are the values the model's `ConfigurationProfile$Type` documentation
    // lists ("AWS.AppConfig.FeatureFlags", "AWS.Freeform"); a profile created with no
    // `Type` is stored as the freeform kind, so naming that kind asks for no change.
    let rows: Vec<Row> = vec![
        Row {
            case: "freeform asked to become feature flags",
            created_type: Some(FREEFORM_PROFILE_TYPE),
            submitted_type: Value::from(FEATURE_FLAGS_PROFILE_TYPE),
            refused: true,
        },
        Row {
            case: "feature flags asked to become freeform",
            created_type: Some(FEATURE_FLAGS_PROFILE_TYPE),
            submitted_type: Value::from(FREEFORM_PROFILE_TYPE),
            refused: true,
        },
        Row {
            case: "created with no type, asked to become feature flags",
            created_type: None,
            submitted_type: Value::from(FEATURE_FLAGS_PROFILE_TYPE),
            refused: true,
        },
        Row {
            case: "a kind no AppConfig profile is",
            created_type: Some(FREEFORM_PROFILE_TYPE),
            submitted_type: Value::from("Custom.Configuration"),
            refused: true,
        },
        Row {
            case: "the stored kind in another case",
            created_type: Some(FREEFORM_PROFILE_TYPE),
            submitted_type: Value::from(FREEFORM_PROFILE_TYPE.to_ascii_lowercase()),
            refused: true,
        },
        Row {
            case: "the kind as a number",
            created_type: Some(FREEFORM_PROFILE_TYPE),
            submitted_type: json!(7),
            refused: true,
        },
        Row {
            case: "the kind as an object",
            created_type: Some(FEATURE_FLAGS_PROFILE_TYPE),
            submitted_type: json!({ "Type": FEATURE_FLAGS_PROFILE_TYPE }),
            refused: true,
        },
        Row {
            case: "the empty kind",
            created_type: Some(FREEFORM_PROFILE_TYPE),
            submitted_type: Value::from(""),
            refused: true,
        },
        Row {
            case: "freeform restating freeform",
            created_type: Some(FREEFORM_PROFILE_TYPE),
            submitted_type: Value::from(FREEFORM_PROFILE_TYPE),
            refused: false,
        },
        Row {
            case: "feature flags restating feature flags",
            created_type: Some(FEATURE_FLAGS_PROFILE_TYPE),
            submitted_type: Value::from(FEATURE_FLAGS_PROFILE_TYPE),
            refused: false,
        },
        Row {
            case: "created with no type, restating the freeform kind it was stored as",
            created_type: None,
            submitted_type: Value::from(FREEFORM_PROFILE_TYPE),
            refused: false,
        },
        Row {
            case: "the kind as null, which is no value at all",
            created_type: Some(FREEFORM_PROFILE_TYPE),
            submitted_type: Value::Null,
            refused: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let stored_type = row.created_type.unwrap_or(FREEFORM_PROFILE_TYPE);
        let profile = create_profile_of(&router, &application, case, row.created_type).await;

        // The patch asks for the kind AND for a member the operation does model, so a
        // partial write would show up as the description having moved.
        let patch = json!({
            "Description": PATCHED_DESCRIPTION,
            "Type": row.submitted_type,
        });
        let answer = update_profile(&router, &application, &profile, &patch).await;

        if row.refused {
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: a request naming another Type must be refused with the \
                 modelled BadRequestException's HTTP 400, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert!(
                answer.error_code().contains("BadRequestException"),
                "case {case:?}: expected the modelled BadRequestException, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.string("Message").as_deref(),
                Some(IMMUTABLE_TYPE_MESSAGE),
                "case {case:?}: expected AppConfig's Type-specific sentence, got {:?}",
                answer.string("Message")
            );
        } else {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: a request naming the kind the profile already is asks for \
                 no change and must be served, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.string("Description").as_deref(),
                Some(PATCHED_DESCRIPTION),
                "case {case:?}: the member the operation models must be applied, got {:?}",
                answer.string("Description")
            );
            assert_eq!(
                answer.string("Type").as_deref(),
                Some(stored_type),
                "case {case:?}: the profile keeps the kind it was created as, got {:?}",
                answer.string("Type")
            );
        }

        // The stored record decides it: a refused request applied NOTHING, an accepted one
        // applied only the member it could.
        let expected_description = if row.refused {
            CREATED_DESCRIPTION
        } else {
            PATCHED_DESCRIPTION
        };
        let fetched = get_profile(&router, &application, &profile).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetConfigurationProfile must answer 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            fetched.string("Description").as_deref(),
            Some(expected_description),
            "case {case:?}: expected the stored description {expected_description:?}, got {:?}",
            fetched.string("Description")
        );
        assert_eq!(
            fetched.string("Type").as_deref(),
            Some(stored_type),
            "case {case:?}: no update can move the profile's kind, got {:?}",
            fetched.string("Type")
        );
    }
}

/// The refusal is about the profile the request addresses, so a request that addresses no
/// profile is still the miss the operation models — a `Type` in the body does not turn a
/// 404 into a 400 — and a body that names no `Type` at all is untouched by the rule.
#[tokio::test]
async fn naming_a_type_does_not_change_how_other_requests_are_answered() {
    let router = router();
    let application = create_application(&router, "immutable-type-others").await;
    let profile = create_profile_of(
        &router,
        &application,
        "the profile that exists",
        Some(FREEFORM_PROFILE_TYPE),
    )
    .await;

    // Rows differ along WHAT the request addresses and whether it names a kind at all.
    let rows: [(&str, &str, &str, Value, u16); 5] = [
        (
            "an application nobody created, naming another kind",
            "zzzzzzz",
            "zzzzzzz",
            json!({ "Type": FEATURE_FLAGS_PROFILE_TYPE }),
            404,
        ),
        (
            "a profile the application does not hold, naming another kind",
            &application,
            "zzzzzzz",
            json!({ "Type": FEATURE_FLAGS_PROFILE_TYPE }),
            404,
        ),
        (
            "a profile the application does not hold, naming no kind",
            &application,
            "zzzzzzz",
            json!({ "Description": PATCHED_DESCRIPTION }),
            404,
        ),
        (
            "the profile that exists, naming no kind",
            &application,
            &profile,
            json!({ "Description": PATCHED_DESCRIPTION }),
            200,
        ),
        (
            "the profile that exists, with an empty body",
            &application,
            &profile,
            json!({}),
            200,
        ),
    ];

    for (case, application_id, profile_id, patch, expected_status) in rows {
        let answer = update_profile(&router, application_id, profile_id, &patch).await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if expected_status == 404 {
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {case:?}: expected the modelled ResourceNotFoundException, got {:?}",
                answer.error_code()
            );
        }
    }

    // The profile the serving rows addressed kept its kind and took the description they
    // carried.
    let fetched = get_profile(&router, &application, &profile).await;
    assert_eq!(
        fetched.string("Type").as_deref(),
        Some(FREEFORM_PROFILE_TYPE),
        "the profile keeps the kind it was created as, got {:?}",
        fetched.string("Type")
    );
    assert_eq!(
        fetched.string("Description").as_deref(),
        Some(PATCHED_DESCRIPTION),
        "a served update must persist the member it carried, got {:?}",
        fetched.string("Description")
    );
}
