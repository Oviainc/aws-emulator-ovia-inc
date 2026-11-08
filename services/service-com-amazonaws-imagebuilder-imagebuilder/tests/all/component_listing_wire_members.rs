// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a component listing says beside its entries' modelled
//! members: the `billable` flag each entry carries, the FORM of the `nextToken` a truncated
//! page hands back, and how a listing refuses a filter it does not filter on.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * A component version a read reports carries `billable`, the flag that says whether it
//!   incurs Amazon Web Services Marketplace charges, and it is read off the stored
//!   component: only a component the Marketplace publishes is metered, so every component a
//!   caller created in this account reports `false` — never an omitted member. The rows
//!   below therefore differ along everything the listing generalises over (how many build
//!   versions a version holds, the platform, the type, the filter that selected it, whether
//!   the entry stands for a version or a name, the page size and the Region), and the
//!   Marketplace-owned case, which no request of this API can store, is pinned by the unit
//!   test beside `ComponentRecord::billable` in `src/state.rs`.
//! * A `nextToken` is an OPAQUE handle: it is drawn from base64's alphabet, it exposes
//!   neither the operation, the Region's endpoint nor the query that selected the page — in
//!   the token or in anything decoded out of it — and it resumes exactly the listing that
//!   issued it; every other listing refuses it as a value its `nextToken` parameter does
//!   not admit. The rows span the paginated listings this service serves rather than the one
//!   the report named.
//! * A filter naming a member a listing does not filter on is a value its `filters`
//!   parameter does not admit, so it is refused as that parameter's value — Image Builder's
//!   `InvalidParameterValueException` sentence — whose requirement states the filters THAT
//!   operation allows and echoes neither the name the request carried nor any value. The
//!   rows span the listings of this service that take filters, each with its own allowed
//!   set.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// A second Region, so the rules are shown to hold at another Region's endpoint too.
const OTHER_REGION: &str = "us-east-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A build component document: one `build` phase, so the derived type is `BUILD`.
const BUILD_DOCUMENT: &str = "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hello\n";

/// A test component document: a single phase named `test`, so the derived type is `TEST`.
const TEST_DOCUMENT: &str = "name: CheckWorld\nschemaVersion: 1.0\nphases:\n  - name: test\n    steps:\n      - name: Check\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo ok\n";

/// The characters a base64 value is spelled with (RFC 4648 §4), which is the alphabet an
/// opaque AWS continuation token is drawn from.
const BASE64_ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and
/// the handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// A `CreateComponent` body: the required members, `clientToken`, and whatever else `extra`
/// names.
fn component_body(name: &str, version: &str, client_token: &str, extra: Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!(version));
    body.insert("platform".to_string(), json!("Linux"));
    body.insert("clientToken".to_string(), json!(client_token));
    body.insert("data".to_string(), json!(BUILD_DOCUMENT));
    if let Value::Object(extra) = extra {
        for (member, value) in extra {
            body.insert(member, value);
        }
    }
    Value::Object(body)
}

/// `ListComponents` with `body`, in `region`.
async fn list_components_in(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(router, region, "POST", "/ListComponents", Some(body)).await
}

/// `ListComponents` with `body`, in [`REGION`].
async fn list_components(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    list_components_in(router, REGION, body).await
}

/// The entries of one `ListComponents` page.
fn entries(case: &str, body: &Value) -> Vec<Value> {
    body.get("componentVersionList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a componentVersionList in {body}"))
        .clone()
}

/// The error code a refusal carries, from `__type` (restJson1 renders the shape there,
/// optionally namespace-qualified).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string()
}

/// The components the cases below list: three versions of one name — the first of them
/// stored twice, so one version holds two build versions — a second name of another
/// platform, type and supported OS version, and one component of the other Region.
async fn stored_components(router: &MultiServiceRouter) {
    let created: Vec<(&str, &str, &str, &str, Value)> = vec![
        (
            "alpha-component",
            "1.0.0",
            "btok-alpha-100-build-1",
            REGION,
            json!({"description": "the first build"}),
        ),
        (
            "alpha-component",
            "1.0.0",
            "btok-alpha-100-build-2",
            REGION,
            json!({"description": "the second build"}),
        ),
        (
            "alpha-component",
            "1.1.0",
            "btok-alpha-110",
            REGION,
            json!({"description": "the minor line"}),
        ),
        (
            "alpha-component",
            "2.0.0",
            "btok-alpha-200",
            REGION,
            json!({
                "description": "the next major",
                "supportedOsVersions": ["Amazon Linux 2"],
            }),
        ),
        (
            "Beta Component",
            "1.0.0",
            "btok-beta-100",
            REGION,
            json!({
                "description": "checks the image",
                "platform": "Windows",
                "data": TEST_DOCUMENT,
                "supportedOsVersions": ["Microsoft Windows Server 2019"],
            }),
        ),
        (
            "gamma-component",
            "1.0.0",
            "btok-gamma-100",
            OTHER_REGION,
            json!({"description": "another Region's own"}),
        ),
    ];
    for (name, version, token, region, extra) in created {
        let body = component_body(name, version, token, extra);
        let (status, created) =
            call_in(router, region, "PUT", "/CreateComponent", Some(&body)).await;
        assert_eq!(
            status, 200,
            "case creating {name} {version} in {region}: expected HTTP 200, got {status} with \
             {created}"
        );
    }
}

/// RULE: every entry of a component listing reports `billable` — the boolean that says
/// whether the version incurs Marketplace charges — and reports `false` for a component
/// this account owns, whatever selected the entry and whatever the entry stands for.
#[tokio::test]
async fn every_listing_entry_reports_billable_for_the_component_it_stands_for() {
    let router = emulator();
    stored_components(&router).await;

    // (case, the Region the listing is served in, the request body, how many entries it
    // must report) — rows differing along what the listing generalises over: the page size,
    // whether an entry stands for a version or a name, which member a filter names, how
    // many entries it selects, and the Region.
    let cases: Vec<(&str, &str, Value, usize)> = vec![
        ("the whole listing", REGION, json!({}), 4),
        ("one entry per page", REGION, json!({"maxResults": 1}), 1),
        ("two entries per page", REGION, json!({"maxResults": 2}), 2),
        (
            "a page larger than the listing",
            REGION,
            json!({"maxResults": 25}),
            4,
        ),
        ("the listing by name", REGION, json!({"byName": true}), 2),
        (
            "one name per page",
            REGION,
            json!({"byName": true, "maxResults": 1}),
            1,
        ),
        (
            "the name filter",
            REGION,
            json!({"filters": [{"name": "name", "values": ["alpha-component"]}]}),
            3,
        ),
        (
            "the platform filter, selecting the other platform",
            REGION,
            json!({"filters": [{"name": "platform", "values": ["Windows"]}]}),
            1,
        ),
        (
            "the type filter, selecting the test component",
            REGION,
            json!({"filters": [{"name": "type", "values": ["TEST"]}]}),
            1,
        ),
        (
            "the version filter with a wildcard node",
            REGION,
            json!({"filters": [{"name": "version", "values": ["1.0.x"]}]}),
            2,
        ),
        (
            "an explicit owner of Self",
            REGION,
            json!({"owner": "Self"}),
            4,
        ),
        (
            "the other Region's own listing",
            OTHER_REGION,
            json!({}),
            1,
        ),
    ];

    for (case, region, body, expected_entries) in cases {
        let (status, listed) = list_components_in(&router, region, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {listed}"
        );
        let page = entries(case, &listed);
        assert_eq!(
            page.len(),
            expected_entries,
            "case {case:?}: expected {expected_entries} entries, got {page:?}"
        );
        for entry in &page {
            let billable = entry.get("billable");
            assert_eq!(
                billable,
                Some(&json!(false)),
                "case {case:?}: expected billable false on every entry, got {billable:?} in \
                 {entry}"
            );
            assert!(
                billable.is_some_and(Value::is_boolean),
                "case {case:?}: expected billable to be a JSON boolean, got {billable:?}"
            );
            // The value follows the stored component: this one is owned by the calling
            // account, which is what makes it unmetered.
            let owner = entry.get("owner");
            assert_eq!(
                owner,
                Some(&json!(ACCOUNT)),
                "case {case:?}: expected the entry to be owned by {ACCOUNT}, got {owner:?}"
            );
        }
    }

    // Paging the listing reports the member on every page, not just the first.
    let case = "every page of the walked listing";
    let mut token: Option<String> = None;
    let mut seen = 0;
    loop {
        let mut body = Map::new();
        body.insert("maxResults".to_string(), json!(1));
        if let Some(token) = &token {
            body.insert("nextToken".to_string(), json!(token));
        }
        let (status, page) = list_components(&router, &Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {page}"
        );
        for entry in entries(case, &page) {
            seen += 1;
            let billable = entry.get("billable");
            assert_eq!(
                billable,
                Some(&json!(false)),
                "case {case:?}: entry {seen} expected billable false, got {billable:?}"
            );
        }
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, 4,
        "case {case:?}: expected the walk to report 4 entries, got {seen}"
    );
}

/// One paginated listing of this service, as a case: the operation, the body that asks for
/// one entry per page, and the member its entries arrive under.
struct Paginated {
    case: &'static str,
    uri: &'static str,
    body: Value,
    items: &'static str,
}

/// The `nextToken` of one page, if any.
fn page_token(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The item keys (`arn`) one page reports under `items`.
fn item_arns(case: &str, body: &Value, items: &str) -> Vec<String> {
    body.get(items)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {items} in {body}"))
        .iter()
        .map(|entry| {
            entry
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// RULE: the `nextToken` a truncated page hands back is an opaque base64 handle that
/// exposes nothing of the listing it continues, and it resumes that listing alone.
#[tokio::test]
async fn a_truncated_page_hands_back_an_opaque_token_that_resumes_its_own_listing() {
    let router = emulator();
    stored_components(&router).await;

    let listings = vec![
        Paginated {
            case: "ListComponents, one entry per page",
            uri: "/ListComponents",
            body: json!({"maxResults": 1}),
            items: "componentVersionList",
        },
        Paginated {
            case: "ListComponents by name, one name per page",
            uri: "/ListComponents",
            body: json!({"maxResults": 1, "byName": true}),
            items: "componentVersionList",
        },
        Paginated {
            case: "ListComponents under a name filter",
            uri: "/ListComponents",
            body: json!({
                "maxResults": 1,
                "owner": "Self",
                "filters": [{"name": "name", "values": ["alpha-component"]}],
            }),
            items: "componentVersionList",
        },
        Paginated {
            case: "ListComponentBuildVersions, one build version per page",
            uri: "/ListComponentBuildVersions",
            body: json!({"maxResults": 1}),
            items: "componentSummaryList",
        },
    ];

    for listing in listings {
        let case = listing.case;
        let (status, first) =
            call_in(&router, REGION, "POST", listing.uri, Some(&listing.body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {first}"
        );
        let token = page_token(&first)
            .unwrap_or_else(|| panic!("case {case:?}: expected a nextToken in {first}"));

        // Base64's alphabet, and nothing else: no separator, no hex-only spelling.
        assert!(
            !token.is_empty() && token.chars().all(|c| BASE64_ALPHABET.contains(c)),
            "case {case:?}: expected a base64 token, got {token:?}"
        );

        // Nothing of the LISTING the token continues is readable out of it — neither the
        // operation, nor the Region's endpoint, nor the query that selected the page —
        // neither in the token nor in the bytes it decodes to. What it does carry is the
        // position: the key of the last entry of the page it follows, which the caller was
        // just handed.
        let decoded = ::aws_smithy_types::base64::decode(&token)
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
            .unwrap_or_else(|error| {
                panic!("case {case:?}: expected a base64 token, got {token:?} ({error})")
            });
        let must_not_appear: Vec<String> = vec![
            listing.uri.trim_start_matches('/').to_string(),
            "owner=".to_string(),
            "filters=".to_string(),
            "byName".to_string(),
        ];
        for secret in &must_not_appear {
            assert!(
                !token.contains(secret.as_str()) && !decoded.contains(secret.as_str()),
                "case {case:?}: expected {secret:?} to be unreadable, found it in {token:?} \
                 (which decodes to {decoded:?})"
            );
        }
        // And the token is not the plaintext spelling a page cursor must never travel in:
        // no digest-and-payload split, and nothing hex-encoded.
        assert!(
            !token.contains('.')
                && !token
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == '=' || c == '+' || c == '/'),
            "case {case:?}: expected an opaque token rather than a rendered cursor, got \
             {token:?}"
        );

        // It is still a cursor: handed back unchanged to the same listing, it continues it,
        // and the walk reports every item exactly once.
        let mut walked = item_arns(case, &first, listing.items);
        let mut token = Some(token);
        while let Some(carried) = token {
            let mut body = listing.body.as_object().cloned().unwrap_or_default();
            body.insert("nextToken".to_string(), json!(carried));
            let (status, page) =
                call_in(&router, REGION, "POST", listing.uri, Some(&Value::Object(body))).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for the next page, got {status} with {page}"
            );
            walked.extend(item_arns(case, &page, listing.items));
            token = page_token(&page);
        }
        let mut unique = walked.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            walked.len(),
            "case {case:?}: expected each item once, got {walked:?}"
        );
        assert!(
            walked.len() > 1,
            "case {case:?}: expected the walk to pass more than one page, got {walked:?}"
        );
    }
}

/// RULE: a token names a position in ONE listing, so every other listing refuses it as a
/// value its `nextToken` parameter does not admit — this service's parameter-value sentence
/// about that member — rather than resuming somewhere the caller never asked about.
#[tokio::test]
async fn a_token_is_refused_by_every_listing_but_the_one_that_issued_it() {
    let router = emulator();
    stored_components(&router).await;

    let (_, first) = list_components(&router, &json!({"maxResults": 1})).await;
    let issued = page_token(&first)
        .unwrap_or_else(|| panic!("case the first page: expected a nextToken in {first}"));
    let (_, build_versions) = call_in(
        &router,
        REGION,
        "POST",
        "/ListComponentBuildVersions",
        Some(&json!({"maxResults": 1})),
    )
    .await;
    let from_build_versions = page_token(&build_versions).unwrap_or_else(|| {
        panic!("case the build-version page: expected a nextToken in {build_versions}")
    });

    // (case, the operation the token is spent on, the Region it is spent in, the body)
    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            "a value no listing minted",
            "/ListComponents",
            REGION,
            json!({"nextToken": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", "maxResults": 1}),
        ),
        (
            "the empty-looking token of the old plaintext spelling",
            "/ListComponents",
            REGION,
            json!({"nextToken": "00000000000000000000000000000000.6162", "maxResults": 1}),
        ),
        (
            "a token issued for the listing by version, spent on the listing by name",
            "/ListComponents",
            REGION,
            json!({"nextToken": issued.clone(), "maxResults": 1, "byName": true}),
        ),
        (
            "a token issued for the unfiltered listing, spent on a filtered one",
            "/ListComponents",
            REGION,
            json!({
                "nextToken": issued.clone(),
                "maxResults": 1,
                "filters": [{"name": "platform", "values": ["Linux"]}],
            }),
        ),
        (
            "a token issued in one Region, spent in another",
            "/ListComponents",
            OTHER_REGION,
            json!({"nextToken": issued.clone(), "maxResults": 1}),
        ),
        (
            "a token issued by the component listing, spent on the build-version listing",
            "/ListComponentBuildVersions",
            REGION,
            json!({"nextToken": issued.clone(), "maxResults": 1}),
        ),
        (
            "a token issued by the build-version listing, spent on the component listing",
            "/ListComponents",
            REGION,
            json!({"nextToken": from_build_versions, "maxResults": 1}),
        ),
        (
            "an issued token with its last character changed",
            "/ListComponents",
            REGION,
            json!({
                "nextToken": format!(
                    "{}{}",
                    &issued[..issued.len() - 1],
                    if issued.ends_with('A') { "B" } else { "A" },
                ),
                "maxResults": 1,
            }),
        ),
    ];

    for (case, uri, region, body) in cases {
        let (status, refused) = call_in(&router, region, "POST", uri, Some(&body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        let code = error_code(case, &refused);
        assert_eq!(
            code, "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {code:?} in {refused}"
        );
        let message = refused
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message,
            "The value supplied for parameter 'nextToken' is not valid. The provided token \
             does not meet the required format.",
            "case {case:?}: got {message:?}"
        );
        // The refused token is never echoed back: it is the value, and the requirement is
        // what the parameter admits.
        if let Some(carried) = body.get("nextToken").and_then(Value::as_str) {
            assert!(
                !message.contains(carried),
                "case {case:?}: expected the refused token not to be echoed, got {message:?}"
            );
        }
    }
}

/// RULE: a filter naming a member a listing does not filter on is a value that listing's
/// `filters` parameter does not admit, so it is refused as that parameter's value, stating
/// the filters THAT operation allows.
#[tokio::test]
async fn an_unsupported_filter_is_refused_as_the_filters_parameters_value() {
    let router = emulator();
    stored_components(&router).await;

    // (case, the operation, the filters it allows, in the order its refusal states them).
    // Each set is the operation's own `filters` member documentation — `ListComponents`:
    // "description, name, platform, productCodes, status, supportedOsVersion, type,
    // version"; `ListImageRecipes`: "name, parentImage, platform"; `ListImageBuildVersions`:
    // "name, osVersion, platform, type, version" — and `ListComponents` states them in the
    // order AWS's own recorded refusal of that operation lists them in.
    let listings: Vec<(&str, &str, &str)> = vec![
        (
            "the component listing",
            "/ListComponents",
            "name, version, description, platform, type, supportedOsVersion, productCodes, \
             status",
        ),
        (
            "the image-recipe listing",
            "/ListImageRecipes",
            "name, parentImage, platform",
        ),
        (
            "the image-build-version listing",
            "/ListImageBuildVersions",
            "name, osVersion, platform, type, version",
        ),
    ];

    // The filter names the rows send: one that belongs to another listing of this service,
    // one that is a member of the resource but not a filter, one spelled in the wrong case,
    // and one nothing at all names.
    let unsupported = ["owner", "arn", "Name", "nme"];

    for (case, uri, allowed) in listings {
        for name in unsupported {
            let case = format!("{case}, the filter {name:?}");
            let (status, refused) = call_in(
                &router,
                REGION,
                "POST",
                uri,
                Some(&json!({"filters": [{"name": name, "values": ["anything"]}]})),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {refused}"
            );
            let code = error_code(&case, &refused);
            assert_eq!(
                code, "InvalidParameterValueException",
                "case {case:?}: expected InvalidParameterValueException, got {code:?}"
            );
            let operation = uri.trim_start_matches('/');
            let expected = format!(
                "The value supplied for parameter 'filters' is not valid. Allowed filters for \
                 {operation} are: {allowed}."
            );
            let message = refused
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            // The refused value is withheld: the requirement is what the parameter admits.
            assert!(
                !message.contains(&format!("'{name}'")),
                "case {case:?}: expected the refused filter name not to be echoed, got \
                 {message:?}"
            );
        }
    }

    // A page size below the modelled minimum is the same sentence about its own parameter,
    // in this service's own wording for a `@range` minimum.
    let (status, refused) = list_components(&router, &json!({"maxResults": 0})).await;
    let case = "a page size below the modelled minimum";
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused}"
    );
    let message = refused
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        message,
        "The value supplied for parameter 'maxResults' is not valid. maxResults must be \
         bigger than or equal to 1",
        "case {case:?}: got {message:?}"
    );
}
