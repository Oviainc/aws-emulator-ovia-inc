// SPDX-License-Identifier: CC0-1.0

//! Reading, modifying and deleting a managed login branding style over the
//! wire, driven through the router exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`). The sibling
//! `managed_login_branding` suite covers creating one.
//!
//! The rules under test:
//!
//! * MERGED READ: `ReturnMergedResources` true "returns values for branding
//!   options that are unchanged from Amazon Cognito defaults", while false or
//!   omitted "returns only values that you customized in your branding style"
//!   (both readers' `ReturnMergedResources`). So the merged answer is the
//!   service's default style with the style's OWN options laid over it, option
//!   by option, and it differs from the customized answer in `Settings` alone.
//!   Each test computes that overlay itself from the default style the emulator
//!   reports for a style that customizes nothing, so it asserts the merge rule
//!   rather than any particular default value.
//! * UPDATE: "Amazon Cognito doesn't require that you pass all parameters in one
//!   request and preserves existing style settings that you don't specify", and
//!   a style larger than one request is submitted as "multiple requests, each
//!   with a size smaller than the limit" (`UpdateManagedLoginBranding`). So an
//!   update lays the options it names over the stored style, keeps every option
//!   it does not name, and `UseCognitoProvidedValues` true "reverts to default
//!   style options that are managed by Amazon Cognito" — which leaves the style
//!   with no customization of its own. Every change is observable through a
//!   later read.
//! * SERIALIZED MODIFICATION: `ConcurrentModificationException` "is thrown if
//!   two or more modifications are happening concurrently", so modifications of
//!   ONE style are serialized: overlapping callers never lose one another's
//!   writes, and a caller refused for the overlap is refused with that error and
//!   nothing else.
//! * DELETE: "When you delete a style, you delete the branding association for
//!   an app client", so a deleted style is gone from both readers and its app
//!   client can be given a new one.
//!
//! Each table row names the case it asserts and every assertion reports the
//! value it actually saw.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::managed_login_branding::{
    asset, asset_bytes, call, client, error_code, error_message, ok, pool, router, CATEGORIES,
    COLOR_MODES, EXTENSIONS,
};

/// The one sentence a style this service holds none of is refused with.
const BRANDING_NOT_FOUND: &str = "ManagedLoginBranding does not exist.";

/// The one sentence an overlapping modification of one style is refused with.
const CONCURRENT_MODIFICATION: &str =
    "Only one request to modify a ManagedLoginBranding can be processed at a time";

/// How many callers overlap, per row of the concurrency tables: the smallest
/// race there is, an odd count, and two larger ones (the issue's ten included).
const RACER_COUNTS: [usize; 4] = [2, 3, 5, 10];

/// A fresh pool with one app client, and a style created for that client from
/// `create`: answers the app client id, the minted style id and the created
/// style.
async fn style(
    router: &MultiServiceRouter,
    name: &str,
    create: &Value,
) -> (String, String, String, Value) {
    let user_pool_id = pool(router, &format!("{name}-pool")).await;
    let client_id = client(router, &user_pool_id, &format!("{name}-client")).await;
    let mut body = create
        .as_object()
        .cloned()
        .expect("the create body is an object");
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientId".to_string(), json!(client_id));
    let created = ok(router, "CreateManagedLoginBranding", &Value::Object(body)).await;
    let branding_id = created["ManagedLoginBranding"]["ManagedLoginBrandingId"]
        .as_str()
        .expect("a minted ManagedLoginBrandingId")
        .to_string();
    (
        user_pool_id,
        client_id,
        branding_id,
        created["ManagedLoginBranding"].clone(),
    )
}

/// Read a style back by its id, in the customized view (`merged` false) or the
/// merged one.
async fn describe(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    branding_id: &str,
    merged: bool,
) -> Value {
    let read = ok(
        router,
        "DescribeManagedLoginBranding",
        &json!({
            "UserPoolId": user_pool_id,
            "ManagedLoginBrandingId": branding_id,
            "ReturnMergedResources": merged,
        }),
    )
    .await;
    read["ManagedLoginBranding"].clone()
}

/// Read a style back by the app client it is assigned to.
async fn describe_by_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    merged: bool,
) -> Value {
    let read = ok(
        router,
        "DescribeManagedLoginBrandingByClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "ReturnMergedResources": merged,
        }),
    )
    .await;
    read["ManagedLoginBranding"].clone()
}

/// `patch` laid over `base`, member by member: the rule the merged read and the
/// update both follow, computed HERE so a test's expectation does not come from
/// the implementation it checks. Two objects are walked into; anything else the
/// patch names replaces what `base` held.
fn overlay(base: &Value, patch: &Value) -> Value {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            let mut merged = base.clone();
            for (name, value) in patch {
                let next = match base.get(name) {
                    Some(held) => overlay(held, value),
                    None => value.clone(),
                };
                merged.insert(name.clone(), next);
            }
            Value::Object(merged)
        }
        (_, patch) => patch.clone(),
    }
}

/// How deeply `value` nests, counting the value itself as one level.
fn depth(value: &Value) -> usize {
    match value {
        Value::Object(members) => 1 + members.values().map(depth).max().unwrap_or(0),
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        _ => 1,
    }
}

/// The default style this service reports for a style that customizes nothing:
/// the baseline every merged read is the overlay of. Read through the API, so a
/// test states the RULE and not the document.
async fn default_style_settings(router: &MultiServiceRouter) -> Value {
    let (user_pool_id, _, branding_id, _) = style(
        router,
        "merged-baseline",
        &json!({ "UseCognitoProvidedValues": true }),
    )
    .await;
    let merged = describe(router, &user_pool_id, &branding_id, true).await;
    merged["Settings"].clone()
}

/// The image files this service provides for the roles a style supplies none of
/// its own for: the baseline every merged read of the files is the overlay of.
/// Read through the API, so a test states the RULE and not the file set.
async fn provided_style_assets(router: &MultiServiceRouter) -> Value {
    let (user_pool_id, _, branding_id, _) = style(
        router,
        "provided-assets-baseline",
        &json!({ "UseCognitoProvidedValues": true }),
    )
    .await;
    let provided = describe(router, &user_pool_id, &branding_id, true).await;
    provided["Assets"].clone()
}

/// A readable key for one asset slot: its category and its display mode, which
/// is what a style holds one image file per.
fn slot(rendered: &Value) -> String {
    format!(
        "{}/{}",
        rendered["Category"].as_str().unwrap_or("<none>"),
        rendered["ColorMode"].as_str().unwrap_or("<none>")
    )
}

/// Every asset of a rendered style, keyed by its slot, so two lists can be
/// compared slot by slot.
fn assets_by_slot(rendered: &Value) -> BTreeMap<String, Value> {
    rendered["Assets"]
        .as_array()
        .map(|assets| {
            assets
                .iter()
                .map(|asset| (slot(asset), asset.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the update table: the style the row creates, the members its
/// `UpdateManagedLoginBranding` carries, and what the update must leave behind.
struct UpdateCase {
    case: &'static str,
    create: Value,
    update: Value,
    /// The settings the style must hold afterwards, or `None` for a style that
    /// must hold none at all.
    expected_settings: Option<Value>,
    /// The assets the style must hold afterwards, slot by slot.
    expected_assets: Vec<Value>,
    expected_use_cognito_provided_values: bool,
}

/// The table. The rows differ along what the rule generalises over: whether the
/// update names `Settings`, `Assets`, both or neither; whether the named
/// options are new, overlap the stored ones at a leaf, or nest deeper than them;
/// the JSON value types the document carries; whether an image file fills a new
/// slot or replaces a stored one; and whether the style is reverted to the
/// service's defaults.
fn update_cases() -> Vec<UpdateCase> {
    let light_logo = asset(
        CATEGORIES[13],
        COLOR_MODES[0],
        EXTENSIONS[2],
        b"light logo",
        Some("form-logo-light"),
    );
    let dark_logo = asset(
        CATEGORIES[13],
        COLOR_MODES[1],
        EXTENSIONS[4],
        b"dark logo",
        None,
    );
    let replaced_light_logo = asset(
        CATEGORIES[13],
        COLOR_MODES[0],
        EXTENSIONS[3],
        b"<svg>replacement</svg>",
        Some("form-logo-light-v2"),
    );
    vec![
        UpdateCase {
            case: "an update that names nothing preserves the whole style",
            create: json!({ "Settings": { "components": { "form": { "borderRadius": 4 } } } }),
            update: json!({}),
            expected_settings: Some(json!({ "components": { "form": { "borderRadius": 4 } } })),
            expected_assets: Vec::new(),
            expected_use_cognito_provided_values: false,
        },
        UpdateCase {
            case: "an update that adds a settings group the style did not carry",
            create: json!({ "Settings": { "components": { "form": { "borderRadius": 4 } } } }),
            update: json!({ "Settings": { "componentClasses": { "buttons": { "borderRadius": 0 } } } }),
            expected_settings: Some(json!({
                "components": { "form": { "borderRadius": 4 } },
                "componentClasses": { "buttons": { "borderRadius": 0 } },
            })),
            expected_assets: Vec::new(),
            expected_use_cognito_provided_values: false,
        },
        UpdateCase {
            case: "an update that overwrites one leaf and keeps its siblings",
            create: json!({
                "Settings": {
                    "components": {
                        "form": { "borderRadius": 4, "borderWidth": 1 },
                        "pageBackground": { "image": { "enabled": true } },
                    }
                }
            }),
            update: json!({ "Settings": { "components": { "form": { "borderWidth": 3 } } } }),
            expected_settings: Some(json!({
                "components": {
                    "form": { "borderRadius": 4, "borderWidth": 3 },
                    "pageBackground": { "image": { "enabled": true } },
                }
            })),
            expected_assets: Vec::new(),
            expected_use_cognito_provided_values: false,
        },
        UpdateCase {
            case: "an update whose document nests deeper than the stored one",
            create: json!({ "Settings": { "components": { "form": {} } } }),
            update: json!({
                "Settings": {
                    "components": { "form": { "logo": { "location": "START", "position": "TOP" } } }
                }
            }),
            expected_settings: Some(json!({
                "components": { "form": { "logo": { "location": "START", "position": "TOP" } } }
            })),
            expected_assets: Vec::new(),
            expected_use_cognito_provided_values: false,
        },
        UpdateCase {
            case: "an update carrying every JSON value type a Document admits",
            create: json!({ "Settings": { "keep": "me", "replace": "old" } }),
            update: json!({
                "Settings": {
                    "replace": "new",
                    "string": "açaí / 日本語 / Ω",
                    "integer": 42,
                    "fraction": 1.5,
                    "negative": -7,
                    "true": true,
                    "false": false,
                    "null": null,
                    "array": [1, "two", false, null, { "nested": [] }],
                    "emptyObject": {},
                    "emptyArray": [],
                }
            }),
            expected_settings: Some(json!({
                "keep": "me",
                "replace": "new",
                "string": "açaí / 日本語 / Ω",
                "integer": 42,
                "fraction": 1.5,
                "negative": -7,
                "true": true,
                "false": false,
                "null": null,
                "array": [1, "two", false, null, { "nested": [] }],
                "emptyObject": {},
                "emptyArray": [],
            })),
            expected_assets: Vec::new(),
            expected_use_cognito_provided_values: false,
        },
        UpdateCase {
            case: "a first settings document for a style that carried none",
            create: json!({}),
            update: json!({ "Settings": { "categories": { "global": { "colorSchemeMode": "DARK" } } } }),
            expected_settings: Some(
                json!({ "categories": { "global": { "colorSchemeMode": "DARK" } } }),
            ),
            expected_assets: Vec::new(),
            expected_use_cognito_provided_values: false,
        },
        UpdateCase {
            case: "an update that adds an image file for a slot the style did not fill",
            create: json!({ "Assets": [light_logo.clone()] }),
            update: json!({ "Assets": [dark_logo.clone()] }),
            expected_settings: None,
            expected_assets: vec![light_logo.clone(), dark_logo.clone()],
            expected_use_cognito_provided_values: false,
        },
        UpdateCase {
            case: "an update that replaces the image file of a slot the style already filled",
            create: json!({ "Assets": [light_logo.clone(), dark_logo.clone()] }),
            update: json!({ "Assets": [replaced_light_logo.clone()] }),
            expected_settings: None,
            expected_assets: vec![replaced_light_logo.clone(), dark_logo.clone()],
            expected_use_cognito_provided_values: false,
        },
        UpdateCase {
            case: "an update naming settings and image files together",
            create: json!({
                "Settings": { "components": { "favicon": { "enabledTypes": ["ICO"] } } },
                "Assets": [light_logo.clone()],
            }),
            update: json!({
                "Settings": { "components": { "favicon": { "enabledTypes": ["ICO", "SVG"] } } },
                "Assets": [dark_logo.clone()],
            }),
            expected_settings: Some(
                json!({ "components": { "favicon": { "enabledTypes": ["ICO", "SVG"] } } }),
            ),
            expected_assets: vec![light_logo.clone(), dark_logo.clone()],
            expected_use_cognito_provided_values: false,
        },
        UpdateCase {
            case: "an update that reverts a customized style to the service's defaults",
            create: json!({
                "Settings": { "components": { "form": { "borderRadius": 4 } } },
                "Assets": [light_logo.clone()],
            }),
            update: json!({ "UseCognitoProvidedValues": true }),
            // The reverted style keeps no customization of its own, so the
            // service's own options are what its MERGED view reports.
            expected_settings: None,
            expected_assets: Vec::new(),
            expected_use_cognito_provided_values: true,
        },
        UpdateCase {
            case: "an update that opts a default style out of the service's defaults",
            create: json!({ "UseCognitoProvidedValues": true }),
            update: json!({
                "UseCognitoProvidedValues": false,
                "Settings": { "categories": { "form": { "displayGraphics": false } } },
            }),
            expected_settings: Some(
                json!({ "categories": { "form": { "displayGraphics": false } } }),
            ),
            expected_assets: Vec::new(),
            expected_use_cognito_provided_values: false,
        },
    ]
}

/// Assert `rendered` is the style the row expects, naming where it came from.
fn assert_style(case: &str, where_from: &str, rendered: &Value, expected: &UpdateCase) {
    match &expected.expected_settings {
        Some(settings) => assert_eq!(
            &rendered["Settings"], settings,
            "case {case:?}: {where_from} must carry the updated settings, got {rendered}"
        ),
        None => assert!(
            rendered["Settings"].is_null(),
            "case {case:?}: {where_from} must carry no settings at all, got {rendered}"
        ),
    }
    assert_eq!(
        rendered["UseCognitoProvidedValues"]
            .as_bool()
            .unwrap_or(false),
        expected.expected_use_cognito_provided_values,
        "case {case:?}: {where_from} must report the updated flag, got {rendered}"
    );
    let rendered_assets = assets_by_slot(rendered);
    let expected_assets: BTreeMap<String, Value> = expected
        .expected_assets
        .iter()
        .map(|asset| (slot(asset), asset.clone()))
        .collect();
    assert_eq!(
        rendered_assets.keys().collect::<Vec<_>>(),
        expected_assets.keys().collect::<Vec<_>>(),
        "case {case:?}: {where_from} must hold exactly the expected image slots, got {rendered}"
    );
    for (slot, expected_asset) in &expected_assets {
        let stored = &rendered_assets[slot];
        for member in ["Category", "ColorMode", "Extension", "ResourceId"] {
            assert_eq!(
                stored.get(member),
                expected_asset.get(member),
                "case {case:?}: {where_from} slot {slot} must carry the expected {member}, got {stored}"
            );
        }
        assert_eq!(
            asset_bytes(stored),
            asset_bytes(expected_asset),
            "case {case:?}: {where_from} slot {slot} must carry the expected bytes, got {stored}"
        );
    }
}

#[tokio::test]
async fn an_update_lays_its_options_over_the_stored_style_and_preserves_the_rest() {
    let router = router();

    for (index, case) in update_cases().into_iter().enumerate() {
        let label = case.case;
        let (user_pool_id, client_id, branding_id, created) =
            style(&router, &format!("update-{index}"), &case.create).await;

        let mut body = case
            .update
            .as_object()
            .cloned()
            .expect("the update body is an object");
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert(
            "ManagedLoginBrandingId".to_string(),
            json!(branding_id.clone()),
        );
        let updated = ok(
            &router,
            "UpdateManagedLoginBranding",
            &Value::Object(body.clone()),
        )
        .await;
        let updated = updated["ManagedLoginBranding"].clone();

        assert_eq!(
            updated["ManagedLoginBrandingId"],
            json!(branding_id),
            "case {label:?}: the update answers the SAME style it modified, got {updated}"
        );
        assert_eq!(
            updated["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the updated style still names its pool, got {updated}"
        );
        assert_eq!(
            updated["CreationDate"], created["CreationDate"],
            "case {label:?}: an update does not move the creation date, got {updated}"
        );
        let created_at = created["CreationDate"].as_f64().unwrap_or_default();
        let modified_at = updated["LastModifiedDate"].as_f64().unwrap_or_default();
        assert!(
            modified_at >= created_at,
            "case {label:?}: the modification date must be at or after the creation date, got {updated}"
        );
        assert_style(label, "the update's answer", &updated, &case);

        // The round trip: both readers render the STORED style, so they answer
        // what the update left behind.
        let read = describe(&router, &user_pool_id, &branding_id, false).await;
        assert_eq!(
            read, updated,
            "case {label:?}: DescribeManagedLoginBranding must render what the update stored, got {read}"
        );
        let by_client = describe_by_client(&router, &user_pool_id, &client_id, false).await;
        assert_eq!(
            by_client, updated,
            "case {label:?}: the reader by app client must answer the same style, got {by_client}"
        );

        // And the same update again is the same answer: laying the same options
        // over the style they already left changes nothing.
        let again = ok(&router, "UpdateManagedLoginBranding", &Value::Object(body)).await;
        assert_style(
            label,
            "a repeated update's answer",
            &again["ManagedLoginBranding"],
            &case,
        );
    }
}

#[tokio::test]
async fn the_merged_view_is_the_default_style_under_the_style_s_own_options() {
    let router = router();
    let defaults = default_style_settings(&router).await;
    let provided_assets = provided_style_assets(&router).await;

    assert!(
        defaults.as_object().is_some_and(|members| members.len() > 1),
        "the default style must report the branding options a style can leave alone, got {defaults}"
    );
    assert!(
        depth(&defaults) >= 3,
        "the default style is a nested settings document, got depth {} in {defaults}",
        depth(&defaults)
    );

    // Each row is the style's OWN options; the expectation is the overlay this
    // test computes over the default style, never a document written out here.
    for (index, (label, customization)) in [
        (
            "a style that customizes nothing",
            Value::Null,
        ),
        (
            "a style that overrides one documented option",
            json!({ "categories": { "global": { "colorSchemeMode": "DARK" } } }),
        ),
        (
            "a style that overrides a whole group",
            json!({ "componentClasses": { "buttons": { "borderRadius": 0 } } }),
        ),
        (
            "a style that adds options the default style does not carry",
            json!({ "vendorExtension": { "nested": [1, 2, 3] }, "scalar": 7 }),
        ),
        (
            "a style that replaces a default object with a scalar",
            json!({ "components": "none" }),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let create = match &customization {
            Value::Null => json!({ "UseCognitoProvidedValues": true }),
            settings => json!({ "Settings": settings }),
        };
        let (user_pool_id, client_id, branding_id, _) =
            style(&router, &format!("merged-{index}"), &create).await;

        let customized = describe(&router, &user_pool_id, &branding_id, false).await;
        let merged = describe(&router, &user_pool_id, &branding_id, true).await;

        let expected = match &customization {
            Value::Null => defaults.clone(),
            settings => overlay(&defaults, settings),
        };
        assert_eq!(
            merged["Settings"], expected,
            "case {label:?}: the merged view is the default style with the style's own options laid over it, got {merged}"
        );
        // None of these rows supplies an image file of its own, so the merged
        // view reports the service's own file for every role.
        assert_eq!(
            merged["Assets"], provided_assets,
            "case {label:?}: the merged view reports the service's image files for the roles the style left alone, got {merged}"
        );
        for member in [
            "ManagedLoginBrandingId",
            "UserPoolId",
            "UseCognitoProvidedValues",
            "CreationDate",
            "LastModifiedDate",
        ] {
            assert_eq!(
                merged.get(member),
                customized.get(member),
                "case {label:?}: the two views differ in Settings and Assets alone, got {member} as {merged}"
            );
        }
        // The reader by app client serves the same two views of the same style.
        assert_eq!(
            describe_by_client(&router, &user_pool_id, &client_id, true).await,
            merged,
            "case {label:?}: the merged view by app client must be the merged view by id, got {merged}"
        );

        // A merged read changes nothing a later caller sees.
        assert_eq!(
            describe(&router, &user_pool_id, &branding_id, false).await,
            customized,
            "case {label:?}: a merged read must not change the stored style, got {customized}"
        );
    }
}

#[tokio::test]
async fn an_update_names_the_merged_options_a_style_keeps() {
    let router = router();
    let defaults = default_style_settings(&router).await;

    // The lifecycle a client follows: read the merged style, change one option
    // in it, and submit it as the style's own. What it reads back is what it
    // sent, and the merged view of that style is unchanged apart from the
    // option it changed.
    let (user_pool_id, _, branding_id, _) = style(
        &router,
        "merged-round-trip",
        &json!({ "UseCognitoProvidedValues": true }),
    )
    .await;
    let mut submitted = defaults
        .as_object()
        .cloned()
        .expect("the default style is an object");
    submitted.insert("vendorExtension".to_string(), json!({ "enabled": true }));
    let submitted = Value::Object(submitted);

    let updated = ok(
        &router,
        "UpdateManagedLoginBranding",
        &json!({
            "UserPoolId": user_pool_id,
            "ManagedLoginBrandingId": branding_id,
            "UseCognitoProvidedValues": false,
            "Settings": submitted,
        }),
    )
    .await;
    assert_eq!(
        updated["ManagedLoginBranding"]["Settings"], submitted,
        "the style opted out of the defaults carries exactly the submitted document, got {updated}"
    );
    assert_eq!(
        describe(&router, &user_pool_id, &branding_id, false).await["Settings"],
        submitted,
        "the customized view of the opted-out style is the submitted document"
    );
    assert_eq!(
        describe(&router, &user_pool_id, &branding_id, true).await["Settings"],
        overlay(&defaults, &submitted),
        "the merged view of the opted-out style is the defaults under the submitted document"
    );
}

#[tokio::test]
async fn an_update_outside_the_rules_is_refused_and_changes_nothing() {
    let router = router();
    let (user_pool_id, _, branding_id, created) = style(
        &router,
        "refused-update",
        &json!({ "Settings": { "components": { "form": { "borderRadius": 4 } } } }),
    )
    .await;
    let other_pool = pool(&router, "refused-update-other-pool").await;
    let absent_branding_id = "00000000-0000-4000-8000-000000000000";
    let one_asset = asset(
        CATEGORIES[7],
        COLOR_MODES[1],
        EXTENSIONS[2],
        b"header logo",
        None,
    );

    for (label, status, code, message, body) in [
        (
            "an update naming a pool that does not exist",
            400,
            "ResourceNotFoundException",
            "User pool us-east-1_absent does not exist.".to_string(),
            json!({
                "UserPoolId": "us-east-1_absent",
                "ManagedLoginBrandingId": branding_id,
                "Settings": { "components": {} },
            }),
        ),
        (
            "an update naming a style id the pool does not hold",
            400,
            "ResourceNotFoundException",
            BRANDING_NOT_FOUND.to_string(),
            json!({
                "UserPoolId": user_pool_id,
                "ManagedLoginBrandingId": absent_branding_id,
                "Settings": { "components": {} },
            }),
        ),
        (
            "an update naming a style stored in another pool",
            400,
            "ResourceNotFoundException",
            BRANDING_NOT_FOUND.to_string(),
            json!({
                "UserPoolId": other_pool,
                "ManagedLoginBrandingId": branding_id,
                "Settings": { "components": {} },
            }),
        ),
        (
            "an update that names no user pool",
            400,
            "InvalidParameterException",
            "Missing required parameter UserPoolId".to_string(),
            json!({
                "ManagedLoginBrandingId": branding_id,
                "Settings": { "components": {} },
            }),
        ),
        (
            "an update that names no style",
            400,
            "InvalidParameterException",
            "Missing required parameter ManagedLoginBrandingId".to_string(),
            json!({
                "UserPoolId": user_pool_id,
                "Settings": { "components": {} },
            }),
        ),
        (
            "a revert to the service's defaults that supplies settings as well",
            400,
            "InvalidParameterException",
            "Settings and Assets must be omitted when UseCognitoProvidedValues is true."
                .to_string(),
            json!({
                "UserPoolId": user_pool_id,
                "ManagedLoginBrandingId": branding_id,
                "UseCognitoProvidedValues": true,
                "Settings": { "components": {} },
            }),
        ),
        (
            "a revert to the service's defaults that supplies an image file as well",
            400,
            "InvalidParameterException",
            "Settings and Assets must be omitted when UseCognitoProvidedValues is true."
                .to_string(),
            json!({
                "UserPoolId": user_pool_id,
                "ManagedLoginBrandingId": branding_id,
                "UseCognitoProvidedValues": true,
                "Assets": [one_asset.clone()],
            }),
        ),
    ] {
        let (answered, payload) = call(&router, "UpdateManagedLoginBranding", &body).await;
        assert_eq!(
            (answered, error_code(&payload), error_message(&payload)),
            (status, code, message.as_str()),
            "case {label:?}: expected {status} {code} {message:?}, got {answered} with {payload}"
        );

        // A refused update left the stored style exactly as it was.
        let read = describe(&router, &user_pool_id, &branding_id, false).await;
        assert_eq!(
            read, created,
            "case {label:?}: a refused update must not have touched the stored style, got {read}"
        );
    }
}

#[tokio::test]
async fn deleting_a_style_releases_its_app_client() {
    let router = router();
    let (user_pool_id, client_id, branding_id, _) = style(
        &router,
        "delete-branding",
        &json!({
            "Settings": { "components": { "form": { "borderRadius": 4 } } },
            "Assets": [asset(
                CATEGORIES[13],
                COLOR_MODES[0],
                EXTENSIONS[2],
                b"logo",
                None,
            )],
        }),
    )
    .await;

    let deleted = ok(
        &router,
        "DeleteManagedLoginBranding",
        &json!({ "UserPoolId": user_pool_id, "ManagedLoginBrandingId": branding_id }),
    )
    .await;
    assert!(
        deleted.is_null() || deleted == json!({}),
        "DeleteManagedLoginBranding answers no payload, got {deleted}"
    );

    // Every operation that reaches the style by its id now answers the one
    // not-found sentence, in both views.
    for (label, operation, body) in [
        (
            "the customized read of a deleted style",
            "DescribeManagedLoginBranding",
            json!({ "UserPoolId": user_pool_id, "ManagedLoginBrandingId": branding_id }),
        ),
        (
            "the merged read of a deleted style",
            "DescribeManagedLoginBranding",
            json!({
                "UserPoolId": user_pool_id,
                "ManagedLoginBrandingId": branding_id,
                "ReturnMergedResources": true,
            }),
        ),
        (
            "a second delete of the same style",
            "DeleteManagedLoginBranding",
            json!({ "UserPoolId": user_pool_id, "ManagedLoginBrandingId": branding_id }),
        ),
        (
            "an update of a deleted style",
            "UpdateManagedLoginBranding",
            json!({
                "UserPoolId": user_pool_id,
                "ManagedLoginBrandingId": branding_id,
                "Settings": { "components": {} },
            }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "ResourceNotFoundException", BRANDING_NOT_FOUND),
            "case {label:?}: expected the 400 not-found refusal, got {status} with {payload}"
        );
    }

    // The reader by app client reports the app client it found no style for.
    let (status, payload) = call(
        &router,
        "DescribeManagedLoginBrandingByClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await;
    let by_client_message = format!("ManagedLoginBranding for client {client_id} does not exist.");
    assert_eq!(
        (status, error_code(&payload), error_message(&payload)),
        (
            400,
            "ResourceNotFoundException",
            by_client_message.as_str()
        ),
        "the app client of a deleted style has none, got {status} with {payload}"
    );

    // "When you delete a style, you delete the branding association for an app
    // client": the app client can be given a new style, which is a new resource.
    let recreated = ok(
        &router,
        "CreateManagedLoginBranding",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "UseCognitoProvidedValues": true,
        }),
    )
    .await;
    assert_ne!(
        recreated["ManagedLoginBranding"]["ManagedLoginBrandingId"],
        json!(branding_id),
        "the app client's new style is a new resource, got {recreated}"
    );
}

#[tokio::test]
async fn a_deeply_nested_update_merges_into_a_deeply_nested_stored_style() {
    let router = router();
    // Both documents nest as deeply as a caller can send one, so the merge of a
    // stored style and an update is walked over a depth the CALLER chose on both
    // sides, not only on one.
    let depth = 100;
    let stored = nested_settings(depth, json!({ "stored": true }));
    let (user_pool_id, client_id, branding_id, _) = style(
        &router,
        "deep-update",
        &json!({ "Settings": stored.clone() }),
    )
    .await;

    let patch = nested_settings(depth, json!({ "added": 1 }));
    let updated = ok(
        &router,
        "UpdateManagedLoginBranding",
        &json!({
            "UserPoolId": user_pool_id,
            "ManagedLoginBrandingId": branding_id,
            "Settings": patch.clone(),
        }),
    )
    .await;

    // The expectation is this test's own overlay of the two documents, so the
    // assertion is that the merge reached the bottom of both.
    let expected = overlay(&stored, &patch);
    assert_eq!(
        updated["ManagedLoginBranding"]["Settings"], expected,
        "the deep update must be laid over the deep stored document all the way down, got {updated}"
    );
    assert_eq!(
        describe(&router, &user_pool_id, &branding_id, false).await["Settings"],
        expected,
        "the stored style must be what the deep update left behind"
    );
    // The merged view of the same style walks the stored document too, and the
    // reader by app client answers the same style.
    let merged = describe(&router, &user_pool_id, &branding_id, true).await;
    assert_eq!(
        merged["Settings"][format!("level{}", depth - 1)],
        expected[format!("level{}", depth - 1)],
        "the merged view keeps the style's own deep options, got {merged}"
    );
    assert_eq!(
        describe_by_client(&router, &user_pool_id, &client_id, false).await["Settings"],
        expected,
        "the reader by app client answers the same deep style"
    );
}

/// A settings document nested `depth` levels deep with `leaf` at the bottom,
/// each level named for its distance from the top so two such documents share
/// every level's name.
fn nested_settings(depth: usize, leaf: Value) -> Value {
    let mut document = leaf;
    for level in 0..depth {
        let mut members = Map::new();
        members.insert(format!("level{level}"), document);
        document = Value::Object(members);
    }
    document
}

/// One emulator shared by every racer, so the overlapping calls reach one store.
fn shared_router() -> Arc<MultiServiceRouter> {
    Arc::new(router())
}

/// Dispatch every body in `bodies` to `operation` AT ONCE and hand back what
/// each one answered, in the order the bodies were given.
///
/// Each call runs in its own spawned task against the ONE shared emulator, so on
/// the multi-thread runtime these tests declare the calls really do overlap.
async fn race(
    router: &Arc<MultiServiceRouter>,
    bodies: Vec<(&'static str, Value)>,
) -> Vec<(usize, u16, Value)> {
    let mut tasks = Vec::new();
    for (index, (operation, body)) in bodies.into_iter().enumerate() {
        let router = Arc::clone(router);
        tasks.push(tokio::spawn(async move {
            let (status, payload) = call(&router, operation, &body).await;
            (index, status, payload)
        }));
    }
    let mut answers = Vec::new();
    for task in tasks {
        answers.push(task.await.expect("a racing task must not panic"));
    }
    answers.sort_by_key(|(index, _, _)| *index);
    answers
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_updates_of_one_style_lose_no_write_and_every_loser_is_refused() {
    for count in RACER_COUNTS {
        let case = format!("{count} overlapping updates of one style");
        let router = shared_router();
        let (user_pool_id, _, branding_id, _) = style(
            &router,
            &format!("race-{count}"),
            &json!({ "Settings": { "shared": { "stored": true } } }),
        )
        .await;

        // Each racer customizes its OWN option of the same style, so a write
        // another racer dropped is visible as a missing option.
        let bodies: Vec<(&'static str, Value)> = (0..count)
            .map(|racer| {
                let mut shared = Map::new();
                shared.insert(format!("racer{racer}"), json!(racer));
                (
                    "UpdateManagedLoginBranding",
                    json!({
                        "UserPoolId": user_pool_id,
                        "ManagedLoginBrandingId": branding_id,
                        "Settings": { "shared": Value::Object(shared) },
                    }),
                )
            })
            .collect();
        let answers = race(&router, bodies).await;

        let mut winners: Vec<usize> = Vec::new();
        for (racer, status, payload) in &answers {
            if *status == 200 {
                winners.push(*racer);
                continue;
            }
            assert_eq!(
                (
                    *status,
                    error_code(payload),
                    error_message(payload)
                ),
                (400, "ConcurrentModificationException", CONCURRENT_MODIFICATION),
                "case {case}: racer {racer} was refused, so it must be the modelled concurrent-modification refusal, got {status} with {payload}"
            );
        }
        assert!(
            !winners.is_empty(),
            "case {case}: at least one overlapping update must be applied, got {answers:?}"
        );

        // Every applied update is in the stored style, and nothing else is: an
        // accepted write is never lost to another racer's read-modify-write.
        let settled = describe(&router, &user_pool_id, &branding_id, false).await;
        let mut expected = Map::new();
        expected.insert("stored".to_string(), json!(true));
        for racer in &winners {
            expected.insert(format!("racer{racer}"), json!(racer));
        }
        assert_eq!(
            settled["Settings"],
            json!({ "shared": Value::Object(expected) }),
            "case {case}: the settled style carries exactly the applied updates {winners:?}, got {settled}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_overlapping_update_and_delete_leave_one_consistent_style() {
    for count in RACER_COUNTS {
        let case = format!("{count} overlapping modifications, one of them a delete");
        let router = shared_router();
        let (user_pool_id, client_id, branding_id, _) = style(
            &router,
            &format!("race-delete-{count}"),
            &json!({ "Settings": { "stored": true } }),
        )
        .await;

        // One delete racing count-1 updates of the same style.
        let mut bodies: Vec<(&'static str, Value)> = (0..count - 1)
            .map(|racer| {
                let mut settings = Map::new();
                settings.insert(format!("racer{racer}"), json!(racer));
                (
                    "UpdateManagedLoginBranding",
                    json!({
                        "UserPoolId": user_pool_id,
                        "ManagedLoginBrandingId": branding_id,
                        "Settings": Value::Object(settings),
                    }),
                )
            })
            .collect();
        bodies.push((
            "DeleteManagedLoginBranding",
            json!({ "UserPoolId": user_pool_id, "ManagedLoginBrandingId": branding_id }),
        ));
        let deleter = bodies.len() - 1;
        let answers = race(&router, bodies).await;

        let mut delete_applied = false;
        for (racer, status, payload) in &answers {
            if *status == 200 {
                delete_applied |= *racer == deleter;
                continue;
            }
            // A loser is refused either for the overlap or because the delete
            // got there first and left no style to modify.
            let refusal = (*status, error_code(payload), error_message(payload));
            assert!(
                refusal
                    == (
                        400,
                        "ConcurrentModificationException",
                        CONCURRENT_MODIFICATION
                    )
                    || refusal == (400, "ResourceNotFoundException", BRANDING_NOT_FOUND),
                "case {case}: racer {racer} must be refused for the overlap or for the style being gone, got {status} with {payload}"
            );
        }

        // An independent read agrees with whichever side won: the style is gone
        // exactly when the delete was applied.
        let (status, payload) = call(
            &router,
            "DescribeManagedLoginBranding",
            &json!({ "UserPoolId": user_pool_id, "ManagedLoginBrandingId": branding_id }),
        )
        .await;
        if delete_applied {
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (400, "ResourceNotFoundException", BRANDING_NOT_FOUND),
                "case {case}: the delete was applied, so the style is gone, got {status} with {payload}"
            );
            let (by_client_status, by_client) = call(
                &router,
                "DescribeManagedLoginBrandingByClient",
                &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
            )
            .await;
            assert_eq!(
                (by_client_status, error_code(&by_client)),
                (400, "ResourceNotFoundException"),
                "case {case}: the app client of the deleted style has none, got {by_client_status} with {by_client}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case}: the delete was refused, so the style survives, got {status} with {payload}"
            );
            assert_eq!(
                payload["ManagedLoginBranding"]["ManagedLoginBrandingId"],
                json!(branding_id),
                "case {case}: the surviving style is the one the race ran against, got {payload}"
            );
        }
    }
}
