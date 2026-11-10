// SPDX-License-Identifier: CC0-1.0

//! Managed login branding over the wire: `CreateManagedLoginBranding` stores one
//! branding style for an app client of the pool it names and every later
//! `DescribeManagedLoginBranding` renders the STORED style, driven through the
//! router exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a created branding style is a resource of the named app
//! client, minted a `ManagedLoginBrandingId` that identifies it for later reads;
//! it renders the pool it belongs to, the `UseCognitoProvidedValues` flag and
//! the submitted settings document and image assets unchanged, both timestamps,
//! and nothing else; a style is linked to one app client, so a second style for
//! that client is refused and leaves the stored one untouched while another app
//! client is a different resource; a request that asks for the Cognito-provided
//! defaults and supplies customizations as well is refused; and an absent pool,
//! app client or style id is a 400. The reader's MERGED view
//! (`ReturnMergedResources` true) answers the same style with the options it
//! leaves at the service's defaults filled in, which the sibling
//! `managed_login_branding_updates` suite covers in full. Each table row names
//! the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// Every category `AssetCategoryType` admits, in the order the enum lists them
/// (`smithy_query(query="shape",shape="AssetCategoryType")`) — fifteen values.
pub(crate) const CATEGORIES: [&str; 15] = [
    "FAVICON_ICO",
    "FAVICON_SVG",
    "EMAIL_GRAPHIC",
    "SMS_GRAPHIC",
    "AUTH_APP_GRAPHIC",
    "PASSWORD_GRAPHIC",
    "PASSKEY_GRAPHIC",
    "PAGE_HEADER_LOGO",
    "PAGE_HEADER_BACKGROUND",
    "PAGE_FOOTER_LOGO",
    "PAGE_FOOTER_BACKGROUND",
    "PAGE_BACKGROUND",
    "FORM_BACKGROUND",
    "FORM_LOGO",
    "IDP_BUTTON_ICON",
];

/// Every display mode `ColorSchemeModeType` admits
/// (`smithy_query(query="shape",shape="ColorSchemeModeType")`).
pub(crate) const COLOR_MODES: [&str; 3] = ["LIGHT", "DARK", "DYNAMIC"];

/// Every file type `AssetExtensionType` admits
/// (`smithy_query(query="shape",shape="AssetExtensionType")`).
pub(crate) const EXTENSIONS: [&str; 5] = ["ICO", "JPEG", "PNG", "SVG", "WEBP"];

/// The modelled length maximum of `AssetListType`
/// (`smithy_query(query="operation",operation="CreateManagedLoginBranding")`).
const MAX_ASSETS: usize = 40;

/// One emulator with just this service mounted.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
pub(crate) fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` member of an error body.
pub(crate) fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`.
pub(crate) async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client in `user_pool_id` and return its minted `ClientId`.
pub(crate) async fn client(router: &MultiServiceRouter, user_pool_id: &str, name: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientName": name }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Whether `candidate` is a canonical v4 UUID, the form
/// `ManagedLoginBrandingIdType`'s pattern demands
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[4][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$`).
fn is_v4_uuid(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    let lengths: Vec<usize> = groups.iter().map(|group| group.len()).collect();
    lengths == vec![8, 4, 4, 4, 12]
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
        && groups[2].starts_with('4')
        && groups[3]
            .chars()
            .next()
            .is_some_and(|c| matches!(c.to_ascii_lowercase(), '8' | '9' | 'a' | 'b'))
}

/// One image asset as the wire carries it: `Bytes` is a blob, so it travels
/// base64-encoded, and `ResourceId` is optional.
pub(crate) fn asset(
    category: &str,
    color_mode: &str,
    extension: &str,
    bytes: &[u8],
    resource_id: Option<&str>,
) -> Value {
    let mut entry = Map::new();
    entry.insert("Category".to_string(), json!(category));
    entry.insert("ColorMode".to_string(), json!(color_mode));
    entry.insert("Extension".to_string(), json!(extension));
    entry.insert(
        "Bytes".to_string(),
        json!(::aws_smithy_types::base64::encode(bytes)),
    );
    if let Some(resource_id) = resource_id {
        entry.insert("ResourceId".to_string(), json!(resource_id));
    }
    Value::Object(entry)
}

/// The bytes a rendered asset carries, decoded from its base64 `Bytes` member;
/// an asset that reports no `Bytes` carries none.
pub(crate) fn asset_bytes(rendered: &Value) -> Vec<u8> {
    match rendered.get("Bytes").and_then(Value::as_str) {
        None => Vec::new(),
        Some(encoded) => ::aws_smithy_types::base64::decode(encoded)
            .unwrap_or_else(|error| panic!("Bytes must be base64 ({error}): {encoded:?}")),
    }
}

/// A settings document nested `depth` levels deep, so the size measure the
/// request-size limit rests on is walked over a deep document rather than only
/// a shallow one.
fn nested_settings(depth: usize) -> Value {
    let mut document = json!({ "leaf": "value" });
    for level in 0..depth {
        let mut level_map = Map::new();
        level_map.insert(format!("level{level}"), document);
        document = Value::Object(level_map);
    }
    document
}

/// One row of the `CreateManagedLoginBranding` table: the flag, the settings
/// document and the image assets the request sends.
struct BrandingCase {
    case: &'static str,
    use_cognito_provided_values: bool,
    settings: Option<Value>,
    assets: Vec<Value>,
}

impl BrandingCase {
    /// This case's `CreateManagedLoginBranding` request for `user_pool_id` and
    /// `client_id`. A member the case does not set is left out of the request
    /// entirely, so the table also spans "omitted" against "supplied".
    fn request(&self, user_pool_id: &str, client_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("ClientId".to_string(), json!(client_id));
        if self.use_cognito_provided_values {
            body.insert("UseCognitoProvidedValues".to_string(), json!(true));
        }
        if let Some(settings) = &self.settings {
            body.insert("Settings".to_string(), settings.clone());
        }
        if !self.assets.is_empty() {
            body.insert("Assets".to_string(), Value::Array(self.assets.clone()));
        }
        Value::Object(body)
    }
}

/// The table. The rows differ along what the rule generalises over: whether the
/// style takes the Cognito-provided defaults or carries customizations, the
/// shape and value types of the `Settings` document, and the count (0, 1, 3 and
/// the modelled maximum 40), category, color mode, file type, blob size (the
/// modelled `AssetBytesType` minimum 0 upwards) and resource id of the assets.
fn branding_cases() -> Vec<BrandingCase> {
    vec![
        BrandingCase {
            case: "the Cognito-provided default style, with no customizations at all",
            use_cognito_provided_values: true,
            settings: None,
            assets: Vec::new(),
        },
        BrandingCase {
            case: "a style that customizes nothing and takes no defaults either",
            use_cognito_provided_values: false,
            settings: None,
            assets: Vec::new(),
        },
        BrandingCase {
            case: "a nested settings document of strings and numbers",
            use_cognito_provided_values: false,
            settings: Some(json!({
                "components": {
                    "primaryButton": { "lightMode": { "defaults": { "backgroundColor": "0972d3ff" } } },
                    "pageBackground": { "image": { "enabled": true } }
                },
                "componentClasses": { "buttons": { "borderRadius": 8 } }
            })),
            assets: Vec::new(),
        },
        BrandingCase {
            case: "a settings document carrying every JSON value type a Document admits",
            use_cognito_provided_values: false,
            settings: Some(json!({
                "string": "açaí / 日本語 / Ω",
                "integer": 42,
                "fraction": 1.5,
                "negative": -7,
                "true": true,
                "false": false,
                "null": null,
                "array": [1, "two", false, null, { "nested": [] }],
                "emptyObject": {},
                "emptyArray": []
            })),
            assets: Vec::new(),
        },
        BrandingCase {
            case: "a settings document nested sixty-four levels deep",
            use_cognito_provided_values: false,
            settings: Some(nested_settings(64)),
            assets: Vec::new(),
        },
        BrandingCase {
            case: "one asset, the first category in LIGHT mode, with a resource id",
            use_cognito_provided_values: false,
            settings: None,
            assets: vec![asset(
                CATEGORIES[0],
                COLOR_MODES[0],
                EXTENSIONS[0],
                b"\x00\x01\x02\xff favicon bytes",
                Some("favicon-light"),
            )],
        },
        BrandingCase {
            case: "three assets spanning the color modes, file types and a resource id with spaces and dashes",
            use_cognito_provided_values: false,
            settings: Some(json!({ "components": { "form": { "logo": { "enabled": true } } } })),
            assets: vec![
                asset(
                    CATEGORIES[13],
                    COLOR_MODES[0],
                    EXTENSIONS[2],
                    b"light logo",
                    Some("Form Logo - light"),
                ),
                asset(
                    CATEGORIES[13],
                    COLOR_MODES[1],
                    EXTENSIONS[4],
                    b"dark logo",
                    None,
                ),
                asset(
                    CATEGORIES[11],
                    COLOR_MODES[2],
                    EXTENSIONS[3],
                    "<svg>背景</svg>".as_bytes(),
                    Some("page-background"),
                ),
            ],
        },
        BrandingCase {
            case: "an asset whose blob is empty, the modelled AssetBytesType minimum 0",
            use_cognito_provided_values: false,
            settings: None,
            assets: vec![asset(
                CATEGORIES[1],
                COLOR_MODES[2],
                EXTENSIONS[3],
                b"",
                None,
            )],
        },
        BrandingCase {
            case: "the asset list at its modelled maximum 40",
            use_cognito_provided_values: false,
            settings: None,
            assets: CATEGORIES
                .iter()
                .flat_map(|category| {
                    COLOR_MODES
                        .iter()
                        .map(move |color_mode| (*category, *color_mode))
                })
                .take(MAX_ASSETS)
                .enumerate()
                .map(|(index, (category, color_mode))| {
                    asset(
                        category,
                        color_mode,
                        EXTENSIONS[index % EXTENSIONS.len()],
                        format!("asset-{index}").as_bytes(),
                        Some(&format!("asset-{index}")),
                    )
                })
                .collect(),
        },
    ]
}

#[tokio::test]
async fn created_branding_styles_round_trip_through_stored_state() {
    let router = router();
    let user_pool_id = pool(&router, "branding-pool").await;

    let mut minted_ids: BTreeSet<String> = BTreeSet::new();
    for (index, case) in branding_cases().into_iter().enumerate() {
        let label = case.case;
        // A style is linked to one app client, so each row gets its own app
        // client of the same pool.
        let client_id = client(&router, &user_pool_id, &format!("branding-client-{index}")).await;
        let created = ok(
            &router,
            "CreateManagedLoginBranding",
            &case.request(&user_pool_id, &client_id),
        )
        .await;
        let branding = &created["ManagedLoginBranding"];

        let branding_id = branding["ManagedLoginBrandingId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_v4_uuid(&branding_id),
            "case {label:?}: the minted ManagedLoginBrandingId must be a v4 UUID, got {branding_id:?}"
        );
        assert!(
            minted_ids.insert(branding_id.clone()),
            "case {label:?}: each created style must get its own id, got {branding_id:?} twice"
        );
        assert_eq!(
            branding["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the style must name the pool it was created in, got {branding}"
        );
        assert_eq!(
            branding["UseCognitoProvidedValues"]
                .as_bool()
                .unwrap_or(false),
            case.use_cognito_provided_values,
            "case {label:?}: the stored flag must be the created one, got {branding}"
        );
        match &case.settings {
            Some(settings) => assert_eq!(
                &branding["Settings"], settings,
                "case {label:?}: the stored settings must be the submitted document, got {branding}"
            ),
            None => assert!(
                branding["Settings"].is_null(),
                "case {label:?}: a style that customized no settings reports none, got {branding}"
            ),
        }
        if case.assets.is_empty() {
            let rendered = branding["Assets"]
                .as_array()
                .unwrap_or_else(|| panic!("case {label:?}: Assets must be a list, got {branding}"));
            assert!(
                rendered.is_empty(),
                "case {label:?}: a style that customized no assets holds an empty list of them, \
                 got {branding}"
            );
        } else {
            let rendered = branding["Assets"]
                .as_array()
                .unwrap_or_else(|| panic!("case {label:?}: Assets must be a list, got {branding}"));
            assert_eq!(
                rendered.len(),
                case.assets.len(),
                "case {label:?}: every submitted asset must be stored, got {rendered:?}"
            );
            for (position, (submitted, stored)) in case.assets.iter().zip(rendered).enumerate() {
                for member in ["Category", "ColorMode", "Extension", "ResourceId"] {
                    assert_eq!(
                        stored.get(member), submitted.get(member),
                        "case {label:?}: asset {position} must keep its {member}, got {stored:?}"
                    );
                }
                assert_eq!(
                    asset_bytes(stored),
                    asset_bytes(submitted),
                    "case {label:?}: asset {position} must keep its bytes, got {stored:?}"
                );
            }
        }
        assert!(
            branding["CreationDate"].is_number() && branding["LastModifiedDate"].is_number(),
            "case {label:?}: a created style carries both timestamps, got {branding}"
        );
        assert_eq!(
            branding["CreationDate"], branding["LastModifiedDate"],
            "case {label:?}: a style nothing has modified was last modified when it was created, got {branding}"
        );

        // The round trip: the customized view renders the STORED style, member
        // for member, whether `ReturnMergedResources` is omitted or sent false.
        for body in [
            json!({ "UserPoolId": user_pool_id, "ManagedLoginBrandingId": branding_id }),
            json!({
                "UserPoolId": user_pool_id,
                "ManagedLoginBrandingId": branding_id,
                "ReturnMergedResources": false,
            }),
        ] {
            let read = ok(&router, "DescribeManagedLoginBranding", &body).await;
            assert_eq!(
                read["ManagedLoginBranding"], created["ManagedLoginBranding"],
                "case {label:?}: DescribeManagedLoginBranding must render the stored style for {body}, got {read}"
            );
        }

        // The MERGED view answers the same style with every branding option and
        // every image file it leaves at the service's defaults filled in, so it
        // carries `Settings` even for a style that customized none, and the two
        // views differ in `Settings` and `Assets` alone.
        let merged = ok(
            &router,
            "DescribeManagedLoginBranding",
            &json!({
                "UserPoolId": user_pool_id,
                "ManagedLoginBrandingId": branding_id,
                "ReturnMergedResources": true,
            }),
        )
        .await;
        let merged_branding = &merged["ManagedLoginBranding"];
        assert!(
            merged_branding["Settings"]
                .as_object()
                .is_some_and(|settings| !settings.is_empty()),
            "case {label:?}: the merged view carries the service's default branding options, got {merged}"
        );
        for member in [
            "ManagedLoginBrandingId",
            "UserPoolId",
            "UseCognitoProvidedValues",
            "CreationDate",
            "LastModifiedDate",
        ] {
            assert_eq!(
                merged_branding.get(member),
                branding.get(member),
                "case {label:?}: the merged view differs from the customized one in Settings and Assets alone, got {member} as {merged_branding:?}"
            );
        }

        // And reading it again changes nothing a later caller sees.
        let again = ok(
            &router,
            "DescribeManagedLoginBranding",
            &json!({ "UserPoolId": user_pool_id, "ManagedLoginBrandingId": branding_id }),
        )
        .await;
        assert_eq!(
            again["ManagedLoginBranding"], created["ManagedLoginBranding"],
            "case {label:?}: a read must not change what the next read sees, got {again}"
        );
    }
}

#[tokio::test]
async fn a_second_style_for_one_app_client_is_refused_and_the_stored_style_survives() {
    let router = router();
    let user_pool_id = pool(&router, "duplicate-branding-pool").await;

    for (index, case) in branding_cases().into_iter().enumerate() {
        let label = case.case;
        let client_id = client(&router, &user_pool_id, &format!("duplicate-{index}")).await;
        let created = ok(
            &router,
            "CreateManagedLoginBranding",
            &case.request(&user_pool_id, &client_id),
        )
        .await;
        let branding_id = created["ManagedLoginBranding"]["ManagedLoginBrandingId"]
            .as_str()
            .expect("a minted ManagedLoginBrandingId")
            .to_string();

        // A second style for the same app client, with DIFFERENT settings:
        // refused, and the stored one is left exactly as it was.
        let (status, payload) = call(
            &router,
            "CreateManagedLoginBranding",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "Settings": { "components": { "secondaryButton": { "lightMode": {} } } },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ManagedLoginBrandingExistsException"),
            "case {label:?}: a second style for one app client is a 400 ManagedLoginBrandingExistsException, got {payload}"
        );

        let read = ok(
            &router,
            "DescribeManagedLoginBranding",
            &json!({ "UserPoolId": user_pool_id, "ManagedLoginBrandingId": branding_id }),
        )
        .await;
        assert_eq!(
            read["ManagedLoginBranding"], created["ManagedLoginBranding"],
            "case {label:?}: a refused create must not have touched the stored style, got {read}"
        );

        // The same request on ANOTHER app client of the same pool is a
        // different resource: a style is linked to an app client, not to a pool.
        let other_client = client(&router, &user_pool_id, &format!("duplicate-other-{index}")).await;
        let other = ok(
            &router,
            "CreateManagedLoginBranding",
            &case.request(&user_pool_id, &other_client),
        )
        .await;
        assert_ne!(
            other["ManagedLoginBranding"]["ManagedLoginBrandingId"],
            created["ManagedLoginBranding"]["ManagedLoginBrandingId"],
            "case {label:?}: the same style on another app client is a different resource, got {other}"
        );
    }
}

#[tokio::test]
async fn the_cognito_provided_default_style_cannot_carry_customizations() {
    let router = router();
    let user_pool_id = pool(&router, "defaults-branding-pool").await;

    let one_asset = asset(
        CATEGORIES[7],
        COLOR_MODES[1],
        EXTENSIONS[2],
        b"header logo",
        None,
    );
    // Each row is the customization the request supplies ALONGSIDE
    // `UseCognitoProvidedValues: true`, which the doc says must be omitted.
    for (label, customization) in [
        (
            "the Cognito-provided defaults with a Settings document",
            json!({ "Settings": { "components": { "primaryButton": {} } } }),
        ),
        (
            "the Cognito-provided defaults with an Assets list",
            json!({ "Assets": [one_asset.clone()] }),
        ),
        (
            "the Cognito-provided defaults with both Settings and Assets",
            json!({
                "Settings": { "componentClasses": { "buttons": { "borderRadius": 0 } } },
                "Assets": [one_asset.clone()],
            }),
        ),
    ] {
        // The app client is created per case so the refusal is about the
        // conflicting parameters, not about a missing app client.
        let client_id = client(&router, &user_pool_id, label).await;
        let mut body = customization
            .as_object()
            .cloned()
            .expect("the case's customization is an object");
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("ClientId".to_string(), json!(client_id));
        body.insert("UseCognitoProvidedValues".to_string(), json!(true));
        let (status, payload) =
            call(&router, "CreateManagedLoginBranding", &Value::Object(body)).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {label:?}: the Cognito-provided defaults with customizations is a 400 InvalidParameterException, got {payload}"
        );

        // The refused create stored nothing, so the app client still has no
        // style and the defaults-only request succeeds.
        let created = ok(
            &router,
            "CreateManagedLoginBranding",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "UseCognitoProvidedValues": true,
            }),
        )
        .await;
        assert_eq!(
            created["ManagedLoginBranding"]["UseCognitoProvidedValues"],
            json!(true),
            "case {label:?}: a refused create stores no style, so this one is the client's first, got {created}"
        );
    }
}

#[tokio::test]
async fn an_absent_pool_client_or_branding_id_is_a_400() {
    let router = router();
    let user_pool_id = pool(&router, "notfound-branding-pool").await;
    let client_id = client(&router, &user_pool_id, "notfound-branding-client").await;
    let other_pool = pool(&router, "other-branding-pool").await;
    let created = ok(
        &router,
        "CreateManagedLoginBranding",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "UseCognitoProvidedValues": true,
        }),
    )
    .await;
    let branding_id = created["ManagedLoginBranding"]["ManagedLoginBrandingId"]
        .as_str()
        .expect("a minted ManagedLoginBrandingId")
        .to_string();
    let absent_branding_id = "00000000-0000-4000-8000-000000000000";

    for (label, operation, body) in [
        (
            "CreateManagedLoginBranding on an absent pool",
            "CreateManagedLoginBranding",
            json!({ "UserPoolId": "us-east-1_absent", "ClientId": client_id, "UseCognitoProvidedValues": true }),
        ),
        (
            "CreateManagedLoginBranding for an app client the pool does not hold",
            "CreateManagedLoginBranding",
            json!({ "UserPoolId": user_pool_id, "ClientId": "absentclientid", "UseCognitoProvidedValues": true }),
        ),
        (
            "CreateManagedLoginBranding for an app client of ANOTHER pool",
            "CreateManagedLoginBranding",
            json!({ "UserPoolId": other_pool, "ClientId": client_id, "UseCognitoProvidedValues": true }),
        ),
        (
            "DescribeManagedLoginBranding on an absent pool",
            "DescribeManagedLoginBranding",
            json!({ "UserPoolId": "us-east-1_absent", "ManagedLoginBrandingId": branding_id }),
        ),
        (
            "DescribeManagedLoginBranding for an id the pool does not hold",
            "DescribeManagedLoginBranding",
            json!({ "UserPoolId": user_pool_id, "ManagedLoginBrandingId": absent_branding_id }),
        ),
        (
            "DescribeManagedLoginBranding for a style stored in another pool",
            "DescribeManagedLoginBranding",
            json!({ "UserPoolId": other_pool, "ManagedLoginBrandingId": branding_id }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: must be a 400 ResourceNotFoundException, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_deleted_app_client_takes_its_branding_style_with_it() {
    let router = router();
    let user_pool_id = pool(&router, "deleted-client-branding-pool").await;
    let client_id = client(&router, &user_pool_id, "deleted-client").await;
    let created = ok(
        &router,
        "CreateManagedLoginBranding",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "Settings": { "components": { "pageHeader": {} } },
        }),
    )
    .await;
    let branding_id = created["ManagedLoginBranding"]["ManagedLoginBrandingId"]
        .as_str()
        .expect("a minted ManagedLoginBrandingId")
        .to_string();

    ok(
        &router,
        "DeleteUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await;

    let (status, payload) = call(
        &router,
        "DescribeManagedLoginBranding",
        &json!({ "UserPoolId": user_pool_id, "ManagedLoginBrandingId": branding_id }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "a style linked to a deleted app client is gone with it, got {payload}"
    );
}

/// The documented request-size limit of a branding style: "This operation has a
/// 2-megabyte request-size limit and include the CSS settings and image assets
/// for your app client"
/// (`smithy_query(query="operation",operation="CreateManagedLoginBranding")`).
const MAX_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;

/// The modelled length maximum of one asset's blob, `AssetBytesType`
/// (`smithy_query(query="shape",shape="AssetBytesType")`).
const MAX_ASSET_BYTES: usize = 1_000_000;

#[tokio::test]
async fn a_style_past_the_documented_request_size_limit_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "branding-size-pool").await;

    // Three assets at the modelled blob maximum are 3,000,000 bytes, past the
    // 2-megabyte limit on the settings and the images together.
    let oversized_assets: Vec<Value> = (0..3)
        .map(|index| {
            asset(
                CATEGORIES[index],
                COLOR_MODES[index],
                EXTENSIONS[index],
                &vec![b'i'; MAX_ASSET_BYTES],
                None,
            )
        })
        .collect();

    for (label, settings, assets) in [
        (
            "a settings document one byte past the limit",
            Some(json!({ "css": "c".repeat(MAX_PAYLOAD_BYTES) })),
            Vec::new(),
        ),
        (
            "image files past the limit together",
            None,
            oversized_assets,
        ),
        (
            "settings and one image file that only exceed it together",
            Some(json!({ "css": "c".repeat(MAX_PAYLOAD_BYTES - MAX_ASSET_BYTES + 1) })),
            vec![asset(
                CATEGORIES[0],
                COLOR_MODES[0],
                EXTENSIONS[2],
                &vec![b'i'; MAX_ASSET_BYTES],
                None,
            )],
        ),
    ] {
        let client_id = client(&router, &user_pool_id, label).await;
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("ClientId".to_string(), json!(client_id));
        if let Some(settings) = settings {
            body.insert("Settings".to_string(), settings);
        }
        if !assets.is_empty() {
            body.insert("Assets".to_string(), Value::Array(assets));
        }
        let (status, payload) =
            call(&router, "CreateManagedLoginBranding", &Value::Object(body)).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "LimitExceededException"),
            "case {label:?}: a style past the 2-megabyte limit is a 400 LimitExceededException, got {payload}"
        );

        // The refused create stored nothing, so a style inside the limit still
        // goes through for that app client.
        let created = ok(
            &router,
            "CreateManagedLoginBranding",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "Settings": { "css": "c".repeat(1024) },
            }),
        )
        .await;
        assert_eq!(
            created["ManagedLoginBranding"]["Settings"]["css"],
            json!("c".repeat(1024)),
            "case {label:?}: a style inside the limit is stored, got {created}"
        );
    }
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused_and_stores_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "branding-constraint-pool").await;
    let client_id = client(&router, &user_pool_id, "branding-constraint-client").await;

    let forty_one: Vec<Value> = (0..=MAX_ASSETS)
        .map(|index| {
            asset(
                CATEGORIES[index % CATEGORIES.len()],
                COLOR_MODES[index % COLOR_MODES.len()],
                EXTENSIONS[index % EXTENSIONS.len()],
                format!("asset-{index}").as_bytes(),
                None,
            )
        })
        .collect();

    for (label, operation, body) in [
        (
            "forty-one assets, one past the modelled list maximum 40",
            "CreateManagedLoginBranding",
            json!({ "UserPoolId": user_pool_id, "ClientId": client_id, "Assets": forty_one }),
        ),
        (
            "an asset category outside the enum",
            "CreateManagedLoginBranding",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "Assets": [asset("PAGE_SIDEBAR", COLOR_MODES[0], EXTENSIONS[2], b"bytes", None)],
            }),
        ),
        (
            "an asset color mode outside the enum",
            "CreateManagedLoginBranding",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "Assets": [asset(CATEGORIES[0], "SEPIA", EXTENSIONS[2], b"bytes", None)],
            }),
        ),
        (
            "an asset file type outside the enum",
            "CreateManagedLoginBranding",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "Assets": [asset(CATEGORIES[0], COLOR_MODES[0], "GIF", b"bytes", None)],
            }),
        ),
        (
            "an asset resource id past the modelled length maximum 40",
            "CreateManagedLoginBranding",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "Assets": [asset(
                    CATEGORIES[0],
                    COLOR_MODES[0],
                    EXTENSIONS[2],
                    b"bytes",
                    Some(&"r".repeat(41)),
                )],
            }),
        ),
        (
            "a user pool id the pattern excludes",
            "CreateManagedLoginBranding",
            json!({ "UserPoolId": "no-underscore", "ClientId": client_id, "UseCognitoProvidedValues": true }),
        ),
        (
            "a branding id that is not a v4 UUID",
            "DescribeManagedLoginBranding",
            json!({ "UserPoolId": user_pool_id, "ManagedLoginBrandingId": "not-a-uuid" }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }

    // None of the refused requests stored anything: the app client still has no
    // style, so a well-formed create for it succeeds.
    let created = ok(
        &router,
        "CreateManagedLoginBranding",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "Settings": { "components": { "form": {} } },
        }),
    )
    .await;
    assert_eq!(
        created["ManagedLoginBranding"]["Settings"],
        json!({ "components": { "form": {} } }),
        "a refused create stores no style, so this one is the client's first, got {created}"
    );
}
