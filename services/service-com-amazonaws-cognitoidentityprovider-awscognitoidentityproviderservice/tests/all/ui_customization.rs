// SPDX-License-Identifier: CC0-1.0

//! Classic hosted UI branding over the wire: `SetUICustomization` stores the
//! branding of one level of a user pool, `GetUICustomization` renders the STORED
//! branding, and the logo image it reports is FETCHED back from the URL it
//! reports — all driven through the router exactly as an SDK and a browser drive
//! them (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`,
//! and an anonymous `GET` of the reported `ImageUrl`).
//!
//! The rule under test: a classic hosted UI branding configuration is a resource
//! of ONE level of a user pool — the app client a request names, or the
//! pool-level default a request with no app client names — and a
//! `SetUICustomization` stores the style sheet and the logo image at that level,
//! keeping the stored value of every member the request leaves out; the set and
//! every later read of that level render the STORED configuration, with the
//! stored logo located on the endpoint the request arrived at and served there
//! byte for byte; a read of an app client with no branding of its own falls back
//! to the pool-level default, a read of a pool with no branding at either level
//! answers an empty object, and a level named for a pool or an app client that
//! does not exist is a 404.
//!
//! Each table row names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The region the credential scope above signs for.
const REGION: &str = "us-east-1";

/// The endpoint every request of this suite arrives at, which is the endpoint a
/// locator this service hands out has to name.
const ENDPOINT: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The `ClientId` that names a pool's POOL-LEVEL branding: "Set the default
/// configuration for all clients with a ClientId of ALL"
/// (`smithy_query(query="doc",shape="SetUICustomization")`).
const ALL_CLIENTS: &str = "ALL";

/// The modelled length maximum of `CSSType`, the type of
/// `SetUICustomization.CSS` (`smithy_query(query="shape",shape="CSSType")`); its
/// minimum is 0.
const MAX_CSS: usize = 131_072;

/// The modelled length maximum of `ImageFileType`, the type of
/// `SetUICustomization.ImageFile`
/// (`smithy_query(query="shape",shape="ImageFileType")`); its minimum is 0.
const MAX_IMAGE_FILE: usize = 131_072;

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT)
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
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// An ANONYMOUS `GET` of `url` on `host`, as a browser rendering the hosted UI
/// fetches a logo: no credential and no `X-Amz-Target`, only the path. Answers
/// the status, the body and the `Content-Type`.
async fn fetch(
    router: &MultiServiceRouter,
    host: &str,
    url: &str,
) -> (u16, Vec<u8>, Option<String>) {
    let path = url
        .strip_prefix(&format!("https://{host}"))
        .unwrap_or_else(|| panic!("{url} must be a locator on {host}"));
    let request = http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", host)
        .body(Bytes::new())
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let media_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    (
        response.status().as_u16(),
        response.body().to_vec(),
        media_type,
    )
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client in `user_pool_id` and return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str, name: &str) -> String {
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

/// A `SetUICustomization` request body: a member the caller leaves `None` is left
/// out of the request entirely, which is what asks for the stored value to stay.
/// `ImageFile` is a blob, so it travels base64-encoded.
fn set_request(
    user_pool_id: &str,
    client_id: Option<&str>,
    css: Option<&str>,
    image_file: Option<&[u8]>,
) -> Value {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    if let Some(client_id) = client_id {
        body.insert("ClientId".to_string(), json!(client_id));
    }
    if let Some(css) = css {
        body.insert("CSS".to_string(), json!(css));
    }
    if let Some(image_file) = image_file {
        body.insert(
            "ImageFile".to_string(),
            json!(::aws_smithy_types::base64::encode(image_file)),
        );
    }
    Value::Object(body)
}

/// A `GetUICustomization` request body.
fn get_request(user_pool_id: &str, client_id: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    if let Some(client_id) = client_id {
        body.insert("ClientId".to_string(), json!(client_id));
    }
    Value::Object(body)
}

/// Where the logo stored for `client_id` of `user_pool_id` is located on `host`.
fn logo_url(host: &str, user_pool_id: &str, client_id: &str) -> String {
    format!("https://{host}/{user_pool_id}/{client_id}/assets/login_logo.png")
}

/// One row of the `SetUICustomization` table: the style sheet and the logo image
/// the request carries.
struct CustomizationCase {
    case: &'static str,
    css: Option<String>,
    image_file: Option<Vec<u8>>,
    /// The media type the stored image announces in its own leading bytes.
    media_type: Option<&'static str>,
}

impl CustomizationCase {
    /// The bytes this row stores as the logo: a blob of no bytes carries no
    /// image, so it stores none.
    fn stored_logo(&self) -> Option<&[u8]> {
        self.image_file
            .as_deref()
            .filter(|bytes| !bytes.is_empty())
    }
}

/// The table. The rows differ along what the rule generalises over: which of the
/// two optional members the request carries, and the character set, the file
/// format and the size of each — the empty value and the modelled length maximum
/// 131072 of `CSSType` and `ImageFileType` included, and every byte value for
/// the blob.
fn customization_cases() -> Vec<CustomizationCase> {
    vec![
        CustomizationCase {
            case: "an ASCII style sheet and no image",
            css: Some(".label-customizable { color: #1166ff; }".to_string()),
            image_file: None,
            media_type: None,
        },
        CustomizationCase {
            case: "a style sheet of non-ASCII text: CJK, an emoji, a combining mark and RTL",
            css: Some(".banner::after { content: \"ロゴ 🎨 e\u{301} مرحبا\"; }".to_string()),
            image_file: None,
            media_type: None,
        },
        CustomizationCase {
            case: "a style sheet at the modelled length minimum 0",
            css: Some(String::new()),
            image_file: None,
            media_type: None,
        },
        CustomizationCase {
            case: "a style sheet at the modelled length maximum 131072",
            css: Some("/*".to_string() + &"x".repeat(MAX_CSS - 4) + "*/"),
            image_file: None,
            media_type: None,
        },
        CustomizationCase {
            case: "an image of every byte value and no style sheet",
            css: None,
            image_file: Some((0u8..=255).collect()),
            media_type: Some("application/octet-stream"),
        },
        CustomizationCase {
            case: "a style sheet and a PNG image together",
            css: Some("#logo { width: 120px; }".to_string()),
            image_file: Some(b"\x89PNG\r\n\x1a\n logo bytes".to_vec()),
            media_type: Some("image/png"),
        },
        CustomizationCase {
            case: "a JPEG image",
            css: None,
            image_file: Some(b"\xff\xd8\xff\xe0 jpeg logo".to_vec()),
            media_type: Some("image/jpeg"),
        },
        CustomizationCase {
            case: "a GIF image",
            css: Some(".a {}".to_string()),
            image_file: Some(b"GIF89a gif logo".to_vec()),
            media_type: Some("image/gif"),
        },
        CustomizationCase {
            case: "a WebP image",
            css: None,
            image_file: Some(b"RIFF\x10\x00\x00\x00WEBPVP8 ".to_vec()),
            media_type: Some("image/webp"),
        },
        CustomizationCase {
            case: "an image at the modelled length minimum 0, which carries no image",
            css: Some(".form { border: 0; }".to_string()),
            image_file: Some(Vec::new()),
            media_type: None,
        },
        CustomizationCase {
            case: "an image at the modelled length maximum 131072",
            css: None,
            image_file: Some(
                b"\x89PNG\r\n\x1a\n"
                    .iter()
                    .copied()
                    .chain((8..MAX_IMAGE_FILE).map(|index| (index % 251) as u8))
                    .collect(),
            ),
            media_type: Some("image/png"),
        },
        CustomizationCase {
            case: "neither a style sheet nor an image",
            css: None,
            image_file: None,
            media_type: None,
        },
    ]
}

/// Assert `rendered` is the branding `case` asked for at `client_id` of
/// `user_pool_id`.
fn assert_rendered(
    rendered: &Value,
    case: &CustomizationCase,
    user_pool_id: &str,
    client_id: &str,
) {
    let label = case.case;
    assert_eq!(
        rendered["UserPoolId"],
        json!(user_pool_id),
        "case {label:?}: the branding must name the pool it is stored in, got {rendered}"
    );
    assert_eq!(
        rendered["ClientId"],
        json!(client_id),
        "case {label:?}: the branding must name the level it is stored at, got {rendered}"
    );
    match &case.css {
        Some(css) => assert_eq!(
            rendered["CSS"],
            json!(css),
            "case {label:?}: the branding must keep the style sheet it was given, got {rendered}"
        ),
        None => assert!(
            rendered.get("CSS").is_none(),
            "case {label:?}: a branding given no style sheet reports none, got {rendered}"
        ),
    }
    let version = rendered["CSSVersion"].as_str().unwrap_or_else(|| {
        panic!("case {label:?}: stored branding reports a CSS version, got {rendered}")
    });
    assert!(
        version.len() == 14 && version.chars().all(|character| character.is_ascii_digit()),
        "case {label:?}: expected a 14-digit version stamp, got {version:?}"
    );
    let expected_logo = case
        .stored_logo()
        .map(|_| logo_url(ENDPOINT, user_pool_id, client_id));
    assert_eq!(
        rendered.get("ImageUrl").and_then(Value::as_str),
        expected_logo.as_deref(),
        "case {label:?}: expected the logo located at {expected_logo:?}, got {rendered}"
    );
    for stamp in ["CreationDate", "LastModifiedDate"] {
        assert!(
            rendered[stamp].is_number(),
            "case {label:?}: stored branding reports {stamp}, got {rendered}"
        );
    }
}

#[tokio::test]
async fn set_branding_round_trips_through_stored_state() {
    let router = router();

    for case in customization_cases() {
        let label = case.case;
        // A pool per row, so each row reads back only what it stored.
        let user_pool_id = pool(&router, "ui-pool").await;

        let stored = ok(
            &router,
            "SetUICustomization",
            &set_request(
                &user_pool_id,
                None,
                case.css.as_deref(),
                case.image_file.as_deref(),
            ),
        )
        .await;
        let stored = &stored["UICustomization"];
        assert_rendered(stored, &case, &user_pool_id, ALL_CLIENTS);

        // The read renders the SAME stored record, member for member —
        // including the timestamps, since a read writes nothing.
        let read = ok(
            &router,
            "GetUICustomization",
            &get_request(&user_pool_id, None),
        )
        .await;
        assert_eq!(
            read["UICustomization"], *stored,
            "case {label:?}: the read must render what the set stored, got {read}"
        );
        // A second read answers the same again.
        let reread = ok(
            &router,
            "GetUICustomization",
            &get_request(&user_pool_id, None),
        )
        .await;
        assert_eq!(
            reread["UICustomization"], *stored,
            "case {label:?}: a repeated read changes nothing, got {reread}"
        );

        // The logo the branding locates serves the stored bytes, byte for byte.
        if let Some(submitted) = case.stored_logo() {
            let url = stored["ImageUrl"].as_str().expect("a located logo");
            let (status, served, media_type) = fetch(&router, ENDPOINT, url).await;
            assert_eq!(
                status, 200,
                "case {label:?}: {url} must serve the stored logo, got {status}"
            );
            assert!(
                served == submitted,
                "case {label:?}: expected the {} stored bytes back, got {} bytes",
                submitted.len(),
                served.len()
            );
            assert_eq!(
                media_type.as_deref(),
                case.media_type,
                "case {label:?}: expected the media type the image announces, got {media_type:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_located_logo_is_fetchable_on_the_endpoint_the_request_arrived_at() {
    let router = router();
    // A caller that reaches the emulator by its own host gets a locator on THAT
    // host, which is the locator it can fetch: an SDK pointed at the emulator
    // never resolves the public service hostname.
    for host in ["cognito-idp.us-east-1.amazonaws.com", "127.0.0.1:9297"] {
        let created = {
            let request = http::Request::builder()
                .method("POST")
                .uri("/")
                .header("host", host)
                .header("content-type", "application/x-amz-json-1.1")
                .header(
                    "x-amz-target",
                    "AWSCognitoIdentityProviderService.CreateUserPool",
                )
                .header("authorization", AUTHORIZATION)
                .body(Bytes::from(json!({ "PoolName": "ui-host-pool" }).to_string()))
                .expect("the request is well formed");
            let response = router.dispatch(request).await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {host:?}: CreateUserPool must succeed, got {:?}",
                String::from_utf8_lossy(response.body())
            );
            serde_json::from_slice::<Value>(response.body()).expect("a JSON body")
        };
        let user_pool_id = created["UserPool"]["Id"].as_str().expect("a pool id");
        let image = format!("logo bytes for {host}").into_bytes();

        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", host)
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                "AWSCognitoIdentityProviderService.SetUICustomization",
            )
            .header("authorization", AUTHORIZATION)
            .body(Bytes::from(
                set_request(user_pool_id, None, None, Some(&image)).to_string(),
            ))
            .expect("the request is well formed");
        let response = router.dispatch(request).await;
        let payload: Value = serde_json::from_slice(response.body()).expect("a JSON body");
        let url = payload["UICustomization"]["ImageUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {host:?}: a located logo, got {payload}"));
        assert_eq!(
            url,
            logo_url(host, user_pool_id, ALL_CLIENTS),
            "case {host:?}: the logo is located on the endpoint the request arrived at"
        );

        let (status, served, _) = fetch(&router, host, url).await;
        assert_eq!(
            status, 200,
            "case {host:?}: {url} must serve the stored logo, got {status}"
        );
        assert!(
            served == image,
            "case {host:?}: expected the stored bytes back, got {} bytes",
            served.len()
        );
    }
}

#[tokio::test]
async fn a_member_the_request_omits_keeps_its_stored_value() {
    let router = router();
    let user_pool_id = pool(&router, "ui-preserve-pool").await;
    let logo = logo_url(ENDPOINT, &user_pool_id, ALL_CLIENTS);
    let first_image = b"\x89PNG\r\n\x1a\n first logo".to_vec();
    let second_image = vec![0u8, 255, 128, 7, 7, 0];

    // Each step: what the request carries, and what the level holds afterwards.
    struct Step {
        case: &'static str,
        css: Option<&'static str>,
        image_file: Option<Vec<u8>>,
        stored_image: Option<Vec<u8>>,
        stored_css: &'static str,
    }

    let steps = vec![
        Step {
            case: "both members supplied",
            css: Some(".a { color: red; }"),
            image_file: Some(first_image.clone()),
            stored_image: Some(first_image.clone()),
            stored_css: ".a { color: red; }",
        },
        Step {
            case: "a style sheet alone, which keeps the stored image",
            css: Some(".b { color: blue; }"),
            image_file: None,
            stored_image: Some(first_image.clone()),
            stored_css: ".b { color: blue; }",
        },
        Step {
            case: "an image alone, which keeps the stored style sheet",
            css: None,
            image_file: Some(second_image.clone()),
            stored_image: Some(second_image.clone()),
            stored_css: ".b { color: blue; }",
        },
        Step {
            case: "an image of no bytes, which carries no image at all",
            css: None,
            image_file: Some(Vec::new()),
            stored_image: None,
            stored_css: ".b { color: blue; }",
        },
    ];

    for step in steps {
        let case = step.case;
        let answered = ok(
            &router,
            "SetUICustomization",
            &set_request(
                &user_pool_id,
                None,
                step.css,
                step.image_file.as_deref(),
            ),
        )
        .await;
        let answered = &answered["UICustomization"];
        assert_eq!(
            answered["CSS"],
            json!(step.stored_css),
            "case {case:?}: expected the style sheet in force, got {answered}"
        );
        match &step.stored_image {
            Some(expected) => {
                assert_eq!(
                    answered["ImageUrl"],
                    json!(logo),
                    "case {case:?}: expected the logo in force to be located, got {answered}"
                );
                let (status, served, _) = fetch(&router, ENDPOINT, &logo).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the located logo must be served, got {status}"
                );
                assert!(
                    served == *expected,
                    "case {case:?}: expected the stored bytes back, got {served:?}"
                );
            }
            None => {
                assert!(
                    answered.get("ImageUrl").is_none(),
                    "case {case:?}: a level with no image locates none, got {answered}"
                );
                let (status, _, _) = fetch(&router, ENDPOINT, &logo).await;
                assert_ne!(
                    status, 200,
                    "case {case:?}: a level with no image serves none"
                );
            }
        }

        // Every answer above is the stored record, so the read agrees with it.
        let read = ok(
            &router,
            "GetUICustomization",
            &get_request(&user_pool_id, None),
        )
        .await;
        assert_eq!(
            read["UICustomization"], *answered,
            "case {case:?}: the read must render what the set stored, got {read}"
        );
    }
}

#[tokio::test]
async fn client_level_branding_overrides_the_pool_level_default() {
    let router = router();
    let user_pool_id = pool(&router, "ui-levels-pool").await;
    let customized = client(&router, &user_pool_id, "customized").await;
    let plain = client(&router, &user_pool_id, "plain").await;
    let pool_logo = b"pool logo bytes".to_vec();
    let client_logo = b"client logo bytes".to_vec();

    ok(
        &router,
        "SetUICustomization",
        &set_request(
            &user_pool_id,
            None,
            Some(".pool { color: green; }"),
            Some(&pool_logo),
        ),
    )
    .await;

    // Every level of the pool is served the default while no app client has
    // branding of its own, whether the read names an app client, names ALL, or
    // names no app client at all.
    for (case, asked_for) in [
        ("no app client", None),
        ("the ALL default itself", Some(ALL_CLIENTS)),
        ("an app client with no branding of its own", Some(&*plain)),
        (
            "an app client that is about to get its own",
            Some(&*customized),
        ),
    ] {
        let read = ok(
            &router,
            "GetUICustomization",
            &get_request(&user_pool_id, asked_for),
        )
        .await;
        let rendered = &read["UICustomization"];
        assert_eq!(
            rendered["ClientId"],
            json!(ALL_CLIENTS),
            "case {case:?}: the pool-level default answers, got {rendered}"
        );
        assert_eq!(
            rendered["CSS"],
            json!(".pool { color: green; }"),
            "case {case:?}: the pool-level style sheet answers, got {rendered}"
        );
        assert_eq!(
            rendered["ImageUrl"],
            json!(logo_url(ENDPOINT, &user_pool_id, ALL_CLIENTS)),
            "case {case:?}: the pool-level logo answers, got {rendered}"
        );
    }

    // Client-level branding overrides the default for that app client only.
    ok(
        &router,
        "SetUICustomization",
        &set_request(
            &user_pool_id,
            Some(&customized),
            Some(".client { color: purple; }"),
            Some(&client_logo),
        ),
    )
    .await;
    let read = ok(
        &router,
        "GetUICustomization",
        &get_request(&user_pool_id, Some(&customized)),
    )
    .await;
    let rendered = &read["UICustomization"];
    assert_eq!(
        rendered["ClientId"],
        json!(customized),
        "the customized app client's own branding answers, got {rendered}"
    );
    assert_eq!(
        rendered["CSS"],
        json!(".client { color: purple; }"),
        "the customized app client's own style sheet answers, got {rendered}"
    );
    assert_eq!(
        rendered["ImageUrl"],
        json!(logo_url(ENDPOINT, &user_pool_id, &customized)),
        "the customized app client's logo is located under its own level, got {rendered}"
    );

    // Each level serves its OWN image: a locator names one level.
    for (case, level, expected) in [
        ("the pool-level default", ALL_CLIENTS, &pool_logo),
        ("the customized app client", &customized, &client_logo),
    ] {
        let url = logo_url(ENDPOINT, &user_pool_id, level);
        let (status, served, _) = fetch(&router, ENDPOINT, &url).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {url} must serve that level's logo, got {status}"
        );
        assert!(
            served == *expected,
            "case {case:?}: expected that level's stored bytes, got {served:?}"
        );
    }

    // An app client with no branding of its own stores none, so its own level
    // serves no image even though the default has one.
    let (status, _, _) = fetch(
        &router,
        ENDPOINT,
        &logo_url(ENDPOINT, &user_pool_id, &plain),
    )
    .await;
    assert_ne!(
        status, 200,
        "an app client with no branding of its own serves no logo"
    );

    for (case, asked_for) in [
        ("no app client", None),
        ("the other app client", Some(&*plain)),
    ] {
        let read = ok(
            &router,
            "GetUICustomization",
            &get_request(&user_pool_id, asked_for),
        )
        .await;
        let rendered = &read["UICustomization"];
        assert_eq!(
            rendered["CSS"],
            json!(".pool { color: green; }"),
            "case {case:?}: client-level branding leaves the default alone, got {rendered}"
        );
    }
}

#[tokio::test]
async fn a_pool_with_no_branding_reports_an_empty_object() {
    let router = router();
    let user_pool_id = pool(&router, "ui-empty-pool").await;
    let app_client = client(&router, &user_pool_id, "unbranded").await;

    for (case, asked_for) in [
        ("no app client", None),
        ("the ALL default", Some(ALL_CLIENTS)),
        ("an app client", Some(&*app_client)),
    ] {
        let read = ok(
            &router,
            "GetUICustomization",
            &get_request(&user_pool_id, asked_for),
        )
        .await;
        assert_eq!(
            read["UICustomization"],
            json!({}),
            "case {case:?}: a pool with no branding answers an empty object, got {read}"
        );
    }

    // Nothing is stored, so no level of the pool serves a logo.
    for level in [ALL_CLIENTS, &app_client] {
        let url = logo_url(ENDPOINT, &user_pool_id, level);
        let (status, _, _) = fetch(&router, ENDPOINT, &url).await;
        assert_ne!(status, 200, "{url} must serve nothing while nothing is stored");
    }
}

#[tokio::test]
async fn only_a_stored_logo_path_is_served() {
    let router = router();
    let user_pool_id = pool(&router, "ui-route-pool").await;
    ok(
        &router,
        "SetUICustomization",
        &set_request(&user_pool_id, None, None, Some(b"GIF89a logo")),
    )
    .await;

    // (case, path, whether it serves the stored image)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "the level's own logo",
            format!("/{user_pool_id}/{ALL_CLIENTS}/assets/login_logo.png"),
            true,
        ),
        (
            "the same path with a cache-busting query",
            format!("/{user_pool_id}/{ALL_CLIENTS}/assets/login_logo.png?v=1"),
            true,
        ),
        (
            "a logo of a pool that does not exist",
            format!("/{REGION}_aaaaaaaaa/{ALL_CLIENTS}/assets/login_logo.png"),
            false,
        ),
        (
            "a logo of a level that stores none",
            format!("/{user_pool_id}/otherclient/assets/login_logo.png"),
            false,
        ),
        (
            "another asset under the level",
            format!("/{user_pool_id}/{ALL_CLIENTS}/assets/background.png"),
            false,
        ),
        (
            "the level with no asset path at all",
            format!("/{user_pool_id}/{ALL_CLIENTS}"),
            false,
        ),
    ];

    for (case, path, serves) in rows {
        let request = http::Request::builder()
            .method("GET")
            .uri(&path)
            .header("host", ENDPOINT)
            .body(Bytes::new())
            .expect("the request is well formed");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        if serves {
            assert_eq!(
                status, 200,
                "case {case:?}: {path} must serve the stored logo, got {status}"
            );
            assert!(
                response.body().to_vec() == b"GIF89a logo".to_vec(),
                "case {case:?}: expected the stored bytes, got {:?}",
                response.body()
            );
        } else {
            assert_ne!(
                status, 200,
                "case {case:?}: {path} must serve no logo, got 200"
            );
        }
    }

    // A write is never served off this route: it answers GET only.
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!(
            "/{user_pool_id}/{ALL_CLIENTS}/assets/login_logo.png"
        ))
        .header("host", ENDPOINT)
        .body(Bytes::from_static(b"replacement"))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    assert_ne!(
        response.status().as_u16(),
        200,
        "a PUT of the logo path is not served"
    );
    let (status, served, _) = fetch(
        &router,
        ENDPOINT,
        &logo_url(ENDPOINT, &user_pool_id, ALL_CLIENTS),
    )
    .await;
    assert_eq!(status, 200, "the stored logo is still served, got {status}");
    assert!(
        served == b"GIF89a logo".to_vec(),
        "the stored logo is unchanged, got {served:?}"
    );
}

#[tokio::test]
async fn an_absent_pool_or_app_client_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "ui-refusal-pool").await;
    // Inside `UserPoolIdType`'s pattern and length, so the refusal is the
    // lookup's, not the modelled constraint's.
    let absent_pool = format!("{REGION}_aaaaaaaaa");
    let absent_client = "absentclientid";

    for (case, operation, body, expected) in [
        (
            "a set naming a pool that does not exist",
            "SetUICustomization",
            set_request(&absent_pool, None, Some(".a {}"), None),
            format!("User pool {absent_pool} does not exist."),
        ),
        (
            "a read naming a pool that does not exist",
            "GetUICustomization",
            get_request(&absent_pool, None),
            format!("User pool {absent_pool} does not exist."),
        ),
        (
            "a set naming an app client the pool does not hold",
            "SetUICustomization",
            set_request(&user_pool_id, Some(absent_client), Some(".a {}"), None),
            format!("User pool client {absent_client} does not exist."),
        ),
        (
            "a read naming an app client the pool does not hold",
            "GetUICustomization",
            get_request(&user_pool_id, Some(absent_client)),
            format!("User pool client {absent_client} does not exist."),
        ),
    ] {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an absent resource is a 404, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="SetUICustomization")`): a
    // modelled constraint refuses the request, and never with the scaffold's 501.
    for (case, body) in [
        (
            "a style sheet one character past the modelled maximum 131072",
            set_request(&user_pool_id, None, Some(&"y".repeat(MAX_CSS + 1)), None),
        ),
        (
            "an image one byte past the modelled maximum 131072",
            set_request(
                &user_pool_id,
                None,
                None,
                Some(&vec![1u8; MAX_IMAGE_FILE + 1]),
            ),
        ),
        (
            "an app client id the pattern excludes",
            set_request(&user_pool_id, Some("not a client id"), Some(".a {}"), None),
        ),
        (
            "a user pool id the pattern excludes",
            set_request("no-underscore", None, Some(".a {}"), None),
        ),
    ] {
        let (status, payload) = call(&router, "SetUICustomization", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside a modelled constraint is a 400, got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {case:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }

    // None of the refused requests stored anything.
    let read = ok(
        &router,
        "GetUICustomization",
        &get_request(&user_pool_id, None),
    )
    .await;
    assert_eq!(
        read["UICustomization"],
        json!({}),
        "a refused set stores no branding, got {read}"
    );
}
