// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `uxc` account-UX-setting surface:
//! `UpdateAccountCustomizations`, `GetAccountCustomizations` and `ListServices`.
//!
//! Every case drives the real wire path — a `restJson1` request through the
//! `MultiServiceRouter` into the service's own handlers — so what is asserted is
//! the emulator's HTTP behaviour, not a handler called directly.

use serde_json::{json, Value};
use service_com_amazonaws_uxc_awsaccountuxsetting::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A SigV4 `Authorization` header whose credential scope names this service, which
/// is the routing key `MultiServiceRouter` dispatches on. The signature itself is
/// never checked here: request authentication is off below.
const SIGV4_UXC: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/uxc/aws4_request, SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// A router mounting one freshly-stated copy of the service.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Explicit, so the suite does not depend on the process-wide auth switch.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Percent-encode a query-parameter value: a page token is base64, whose `+`, `/`
/// and `=` would otherwise be read as query syntax (or as a space).
fn encode_query_value(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(b).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Send one request and return `(status, error-type, body)`. `error-type` is the
/// `x-amzn-errortype` header restJson1 resolves an error shape from.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let raw = body
        .map(|b| serde_json::to_vec(&b).expect("serialize the request body"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "uxc.us-east-1.amazonaws.com")
        .header("authorization", SIGV4_UXC)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(raw))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, error_type, value)
}

async fn get_customizations(router: &MultiServiceRouter) -> (u16, String, Value) {
    call(router, "GET", "/v1/account-customizations", None).await
}

async fn update_customizations(
    router: &MultiServiceRouter,
    body: Value,
) -> (u16, String, Value) {
    call(router, "PATCH", "/v1/account-customizations", Some(body)).await
}

/// The `ListServices` request URI for one page request.
fn list_services_uri(max_results: Option<i64>, next_token: Option<&str>) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(max) = max_results {
        query.push(format!("maxResults={max}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={}", encode_query_value(token)));
    }
    if query.is_empty() {
        "/v1/services".to_string()
    } else {
        format!("/v1/services?{}", query.join("&"))
    }
}

/// One page of `ListServices`: `(services, nextToken)`.
async fn list_page(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let uri = list_services_uri(max_results, next_token);

    let (status, _, body) = call(router, "GET", &uri, None).await;
    assert_eq!(status, 200, "ListServices {uri}: expected 200, got {status} with body {body}");
    let services = body
        .get("services")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListServices {uri}: no `services` array in {body}"))
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("ListServices {uri}: non-string service in {body}"))
                .to_string()
        })
        .collect();
    let next = body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (services, next)
}

/// The whole catalog, as the single page an unsized request returns.
async fn whole_catalog(router: &MultiServiceRouter) -> Vec<String> {
    let (services, next) = list_page(router, None, None).await;
    assert!(
        next.is_none(),
        "an unsized page holding the whole catalog must not carry a nextToken, got {next:?}"
    );
    services
}

/// A service identifier legal under the model's `Service` member pattern
/// `^[a-z0-9]+(-[a-z0-9]+)*$` (`smithy_query(query="shape", shape="ServiceList")`).
fn generated_service_id(n: usize) -> String {
    format!("svc-{n}")
}

/// A Region code legal under the model's `Region` member pattern
/// `^[a-z]{2}(-[a-z]{1,10}){1,2}-[1-9]$` (`smithy_query(query="shape", shape="RegionsList")`).
fn generated_region(n: usize) -> String {
    let first = char::from(b'a' + (n / 26) as u8);
    let second = char::from(b'a' + (n % 26) as u8);
    format!("eu-{first}{second}-1")
}

fn strings(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|v| json!(v)).collect())
}

// ---------------------------------------------------------------------------
// GetAccountCustomizations
// ---------------------------------------------------------------------------

/// An account that has never been updated reports the documented defaults:
/// `accountColor` `none`, and no list at all (null = "all are visible").
#[tokio::test]
async fn get_before_any_update_returns_the_documented_defaults() {
    let router = build_router();
    let (status, _, body) = get_customizations(&router).await;

    assert_eq!(status, 200, "fresh account: expected 200, got {status} ({body})");
    assert_eq!(
        body.get("accountColor").and_then(Value::as_str),
        Some("none"),
        "fresh account: expected accountColor none, got {body}"
    );
    assert!(
        body.get("visibleServices").is_none() && body.get("visibleRegions").is_none(),
        "fresh account: an unconfigured list must be absent, got {body}"
    );
}

// ---------------------------------------------------------------------------
// UpdateAccountCustomizations -> GetAccountCustomizations
// ---------------------------------------------------------------------------

/// The rule, over a table spanning the class: a settings value accepted by
/// `UpdateAccountCustomizations` is persisted verbatim for the calling account,
/// returned by that call, and returned again by `GetAccountCustomizations` — and
/// re-applying the same request changes neither (`@idempotent`).
///
/// Rows differ along colour value, list size (0, 1, 3, 10), element form and
/// ordering, which is what the rule generalises over.
#[tokio::test]
async fn update_persists_and_get_reads_back_every_shape_of_setting() {
    struct Case {
        name: &'static str,
        color: &'static str,
        services: Vec<&'static str>,
        regions: Vec<&'static str>,
    }

    let cases = vec![
        Case {
            name: "two services and one Region",
            color: "teal",
            services: vec!["s3", "lambda"],
            regions: vec!["us-east-1"],
        },
        Case {
            name: "explicitly empty lists with the reset colour",
            color: "none",
            services: vec![],
            regions: vec![],
        },
        Case {
            name: "catalog identifiers with a hyphen, several Region forms",
            color: "darkBlue",
            services: vec!["cognito-idp", "access-analyzer", "xray"],
            regions: vec!["eu-west-3", "ap-southeast-2", "us-gov-west-1"],
        },
        Case {
            name: "ten services in non-alphabetical order",
            color: "lightBlue",
            services: vec![
                "sqs",
                "ec2",
                "iam",
                "dynamodb",
                "kms",
                "athena",
                "glue",
                "ecr",
                "batch",
                "states",
            ],
            regions: vec!["ca-central-1"],
        },
        Case {
            name: "single service, single-letter-rich colour",
            color: "yellow",
            services: vec!["secretsmanager"],
            regions: vec!["ap-northeast-1"],
        },
    ];

    for case in cases {
        // A fresh router per row: each row is a complete setting, not a merge.
        let router = build_router();
        let request = json!({
            "accountColor": case.color,
            "visibleServices": strings(&case.services),
            "visibleRegions": strings(&case.regions),
        });
        let expected = json!({
            "accountColor": case.color,
            "visibleServices": strings(&case.services),
            "visibleRegions": strings(&case.regions),
        });

        let (status, _, updated) = update_customizations(&router, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: Update expected 200, got {status} ({updated})",
            case.name
        );
        assert_eq!(
            updated, expected,
            "case {:?}: Update must answer the stored record, got {updated:?}",
            case.name
        );

        let (status, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            status, 200,
            "case {:?}: Get expected 200, got {status} ({fetched})",
            case.name
        );
        assert_eq!(
            fetched, expected,
            "case {:?}: Get must answer what Update stored, got {fetched:?}",
            case.name
        );

        // @idempotent: the same request again leaves identical state and body.
        let (status, _, repeated) = update_customizations(&router, request).await;
        assert_eq!(
            status, 200,
            "case {:?}: repeated Update expected 200, got {status} ({repeated})",
            case.name
        );
        assert_eq!(
            repeated, expected,
            "case {:?}: a repeated Update must answer identically, got {repeated:?}",
            case.name
        );
        let (_, _, after) = get_customizations(&router).await;
        assert_eq!(
            after, expected,
            "case {:?}: a repeated Update must leave state identical, got {after:?}",
            case.name
        );
    }
}

/// "Only the settings that you include in the request body are modified. Omitted
/// settings remain unchanged." Each member is set by its own request, and every
/// earlier setting survives.
#[tokio::test]
async fn settings_omitted_from_a_request_remain_unchanged() {
    let router = build_router();

    let (status, _, body) = update_customizations(&router, json!({"accountColor": "purple"})).await;
    assert_eq!(status, 200, "colour-only update: expected 200, got {status} ({body})");
    assert_eq!(
        body,
        json!({"accountColor": "purple"}),
        "colour-only update: the lists must stay unconfigured, got {body:?}"
    );

    let (status, _, body) =
        update_customizations(&router, json!({"visibleServices": ["cloudtrail", "config"]})).await;
    assert_eq!(status, 200, "services-only update: expected 200, got {status} ({body})");
    assert_eq!(
        body,
        json!({"accountColor": "purple", "visibleServices": ["cloudtrail", "config"]}),
        "services-only update: the stored colour must survive, got {body:?}"
    );

    let (status, _, body) =
        update_customizations(&router, json!({"visibleRegions": ["sa-east-1"]})).await;
    assert_eq!(status, 200, "regions-only update: expected 200, got {status} ({body})");

    let (_, _, fetched) = get_customizations(&router).await;
    assert_eq!(
        fetched,
        json!({
            "accountColor": "purple",
            "visibleServices": ["cloudtrail", "config"],
            "visibleRegions": ["sa-east-1"],
        }),
        "three partial updates must accumulate into one record, got {fetched:?}"
    );

    // An empty body changes nothing at all.
    let (status, _, body) = update_customizations(&router, json!({})).await;
    assert_eq!(status, 200, "empty update: expected 200, got {status} ({body})");
    assert_eq!(
        body, fetched,
        "an update naming no setting must leave the record alone, got {body:?}"
    );
}

/// A colour outside the modelled `AccountColor` value set is refused with HTTP 400
/// `ValidationException`, and nothing is stored.
#[tokio::test]
async fn an_unmodelled_account_color_is_refused() {
    // Cases: unknown word, wrong case for a modelled value, empty string, and a
    // value of the wrong JSON type.
    let cases: Vec<(&str, Value)> = vec![
        ("unknown colour", json!("chartreuse")),
        ("wrong case", json!("Teal")),
        ("empty string", json!("")),
        ("trailing space", json!("teal ")),
    ];

    for (name, color) in cases {
        let router = build_router();
        let (status, error_type, body) =
            update_customizations(&router, json!({"accountColor": color})).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} ({body})"
        );
        assert_eq!(
            error_type, "ValidationException",
            "case {name:?}: expected a ValidationException, got {error_type:?} ({body})"
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched.get("accountColor").and_then(Value::as_str),
            Some("none"),
            "case {name:?}: a refused update must store nothing, got {fetched:?}"
        );
    }
}

/// The modelled list bounds (`visibleServices` max 500, `visibleRegions` max 100):
/// a list AT the bound is accepted and read back; one element past it is refused
/// with HTTP 400 and stores nothing.
#[tokio::test]
async fn lists_are_accepted_at_their_modelled_bound_and_refused_past_it() {
    // (member, bound) from `smithy_query(query="shape", shape="ServiceList,RegionsList")`.
    let services_at_bound: Vec<String> = (0..500).map(generated_service_id).collect();
    let regions_at_bound: Vec<String> = (0..100).map(generated_region).collect();

    let router = build_router();
    let (status, _, body) = update_customizations(
        &router,
        json!({
            "visibleServices": services_at_bound,
            "visibleRegions": regions_at_bound,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "lists exactly at their bound must be accepted, got {status} ({body})"
    );
    let (_, _, fetched) = get_customizations(&router).await;
    assert_eq!(
        fetched.get("visibleServices"),
        Some(&json!(services_at_bound)),
        "500 services must round-trip verbatim"
    );
    assert_eq!(
        fetched.get("visibleRegions"),
        Some(&json!(regions_at_bound)),
        "100 Regions must round-trip verbatim"
    );

    let over_services: Vec<String> = (0..501).map(generated_service_id).collect();
    let over_regions: Vec<String> = (0..101).map(generated_region).collect();
    for (name, request) in [
        ("501 services", json!({"visibleServices": over_services})),
        ("101 Regions", json!({"visibleRegions": over_regions})),
    ] {
        let router = build_router();
        let (status, error_type, body) = update_customizations(&router, request).await;
        assert_eq!(
            status, 400,
            "case {name:?}: one past the bound must be refused, got {status} ({body})"
        );
        assert_eq!(
            error_type, "ValidationException",
            "case {name:?}: expected a ValidationException, got {error_type:?} ({body})"
        );
        let (_, _, fetched) = get_customizations(&router).await;
        assert!(
            fetched.get("visibleServices").is_none() && fetched.get("visibleRegions").is_none(),
            "case {name:?}: a refused update must store nothing, got {fetched:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// ListServices
// ---------------------------------------------------------------------------

/// The rule, over every page size: paging from the first page to the one without a
/// `nextToken` enumerates the catalog exactly once, in the same stable order the
/// unpaged listing reports, with no page longer than `maxResults`.
#[tokio::test]
async fn paging_enumerates_the_catalog_exactly_once_for_every_page_size() {
    let router = build_router();
    let catalog = whole_catalog(&router).await;
    assert!(
        catalog.len() > 2,
        "the seeded catalog must hold enough entries to page over, got {catalog:?}"
    );

    // Page sizes spanning the class: the minimum, small sizes that do and do not
    // divide the catalog, the sizes either side of its length, and the modelled
    // maximum — plus the unsized request.
    let len = catalog.len() as i64;
    let sizes: Vec<Option<i64>> = vec![
        None,
        Some(1),
        Some(2),
        Some(7),
        Some(len - 1),
        Some(len),
        Some(len + 1),
        Some(1000),
    ];

    for size in sizes {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let (page, next) = list_page(&router, size, token.as_deref()).await;
            pages += 1;
            if let Some(max) = size {
                assert!(
                    page.len() as i64 <= max,
                    "case maxResults={size:?}: page {pages} holds {} items, more than the requested {max}",
                    page.len()
                );
            }
            seen.extend(page);
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                pages <= catalog.len() + 1,
                "case maxResults={size:?}: paging did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            seen, catalog,
            "case maxResults={size:?}: paging must yield the catalog exactly once in order, got {seen:?}"
        );
        if let Some(max) = size {
            let expected_pages = (catalog.len() as i64 + max - 1) / max;
            assert_eq!(
                pages as i64,
                expected_pages.max(1),
                "case maxResults={size:?}: expected {expected_pages} pages, took {pages}"
            );
        }
    }

    let mut sorted = catalog.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted, catalog,
        "the catalog must be stably ordered and duplicate-free, got {catalog:?}"
    );
}

/// Every catalog entry is usable as a `visibleServices` value — which is what
/// `ListServices` exists for — including entries the issue never cited.
#[tokio::test]
async fn every_catalog_entry_is_accepted_as_a_visible_service() {
    let router = build_router();
    let catalog = whole_catalog(&router).await;

    let (status, _, body) =
        update_customizations(&router, json!({"visibleServices": catalog})).await;
    assert_eq!(
        status, 200,
        "the whole catalog must be accepted as visibleServices, got {status} ({body})"
    );
    let (_, _, fetched) = get_customizations(&router).await;
    assert_eq!(
        fetched.get("visibleServices"),
        Some(&json!(catalog)),
        "the stored visibleServices must be the catalog verbatim, got {fetched:?}"
    );

    // And one entry at a time, to pin each identifier on its own.
    for service in &catalog {
        let router = build_router();
        let (status, _, body) =
            update_customizations(&router, json!({"visibleServices": [service]})).await;
        assert_eq!(
            status, 200,
            "case {service:?}: a catalog identifier must be accepted, got {status} ({body})"
        );
    }
}

/// A page request outside the model's rules is refused with HTTP 400
/// `ValidationException`: a `maxResults` outside `1..1000`, and a `nextToken` this
/// service did not issue (malformed, or well-formed but not one of ours).
#[tokio::test]
async fn bad_page_inputs_are_refused() {
    let router = build_router();
    let (_, token) = list_page(&router, Some(1), None).await;
    let issued = token.expect("a first page of one item must carry a nextToken");
    let mutated: String = {
        // Flip the first character to another base64 letter: still inside the
        // modelled pattern and length, but no longer a token this service minted.
        let mut chars: Vec<char> = issued.chars().collect();
        chars[0] = if chars[0] == 'a' { 'b' } else { 'a' };
        chars.into_iter().collect()
    };

    let cases: Vec<(&str, Option<i64>, Option<String>)> = vec![
        ("maxResults below the range", Some(0), None),
        ("maxResults above the range", Some(1001), None),
        ("negative maxResults", Some(-1), None),
        ("nextToken shorter than the modelled minimum", None, Some("abc".to_string())),
        ("nextToken outside the modelled pattern", None, Some("!".repeat(40))),
        ("well-formed token this service never issued", None, Some("a".repeat(40))),
        ("a tampered issued token", None, Some(mutated)),
    ];

    for (name, max_results, next_token) in cases {
        let uri = list_services_uri(max_results, next_token.as_deref());
        let (status, error_type, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} ({body})"
        );
        assert_eq!(
            error_type, "ValidationException",
            "case {name:?}: expected a ValidationException, got {error_type:?} ({body})"
        );
    }
}

/// The pagination framework's refusal message for a continuation token that does
/// not decode as a cursor: a fixed template whose only variable part is the
/// operation name.
fn pagination_token_message(operation: &str) -> String {
    format!(
        "Pagination token exception in operation '{operation}': invalid payload wrapping schema 0"
    )
}

/// The modelled `NextToken` constraints — `@length(32, 512)` and
/// `@pattern ^[-A-Za-z0-9+/_]+=*$`
/// (`smithy_query(query="operation", operation="ListServices")`) — asserted on
/// every row of the table below, so a case cannot accidentally exercise the
/// boundary validator instead of the cursor decode.
fn assert_well_formed_token(name: &str, token: &str) {
    assert!(
        (32..=512).contains(&token.len()),
        "case {name:?}: a row must satisfy @length(32, 512), got {} chars",
        token.len()
    );
    let (body, padding) = match token.find('=') {
        Some(at) => token.split_at(at),
        None => (token, ""),
    };
    let charset_ok = body
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'+' | b'/' | b'_'));
    assert!(
        !body.is_empty() && charset_ok && padding.bytes().all(|b| b == b'='),
        "case {name:?}: a row must satisfy @pattern ^[-A-Za-z0-9+/_]+=*$, got {token:?}"
    );
}

/// The rule, over a table spanning the class: a `nextToken` that SATISFIES the
/// modelled `NextToken` constraints but does not decode as a cursor this service
/// issued for the listing is refused with HTTP 400 `ValidationException` carrying
/// the pagination-token exception template for this operation — and no `fieldList`,
/// because no input member's own constraint was violated.
///
/// Rows differ along length (the modelled minimum, the maximum and in between),
/// charset (one repeated letter, the full alphabet including `+`, `/`, `-`, `_` and
/// `=` padding), payload (undecodable base64, base64 of a non-cursor plaintext) and
/// provenance (invented, versus an issued token tampered with or truncated).
#[tokio::test]
async fn a_well_formed_non_cursor_next_token_is_refused_with_the_pagination_token_message() {
    let router = build_router();
    let (_, token) = list_page(&router, Some(1), None).await;
    let issued = token.expect("a first page of one item must carry a nextToken");

    let tampered: String = {
        // One character of an issued token changed, inside the same charset: it
        // still clears the member's constraints, but no longer decodes.
        let mut chars: Vec<char> = issued.chars().collect();
        let middle = chars.len() / 2;
        chars[middle] = if chars[middle] == 'Z' { 'Y' } else { 'Z' };
        chars.into_iter().collect()
    };
    let truncated: String = issued.chars().take(32).collect();
    assert!(
        issued.len() > 32,
        "an issued token must be longer than the modelled minimum to truncate, got {issued:?}"
    );

    let cases: Vec<(&str, String)> = vec![
        // The recorded instance: 32 characters, charset-valid.
        ("one letter at the modelled minimum length", "A".repeat(32)),
        ("one letter at the modelled maximum length", "A".repeat(512)),
        (
            "the whole modelled charset, no padding",
            "aZ09+/-_".repeat(5),
        ),
        (
            "charset-valid run with trailing padding",
            format!("{}==", "QUJD".repeat(10)),
        ),
        (
            "base64 of a plaintext that is not a cursor",
            aws_smithy_types::base64::encode("definitely-not-a-cursor-token-payload"),
        ),
        (
            "base64 of a cursor-shaped plaintext with the wrong prefix",
            aws_smithy_types::base64::encode("not/a/catalog/offset=000000000042"),
        ),
        ("an issued token with one character changed", tampered),
        ("an issued token truncated to the minimum length", truncated),
    ];

    let expected = pagination_token_message("ListServices");
    for (name, token) in cases {
        assert_well_formed_token(name, &token);

        // Both with and without a page size: the refusal does not depend on it.
        for max_results in [None, Some(1), Some(1000)] {
            let uri = list_services_uri(max_results, Some(&token));
            let (status, error_type, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 400,
                "case {name:?} (maxResults={max_results:?}): expected HTTP 400, got {status} ({body})"
            );
            assert_eq!(
                error_type, "ValidationException",
                "case {name:?} (maxResults={max_results:?}): expected a ValidationException, got {error_type:?} ({body})"
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(expected.as_str()),
                "case {name:?} (maxResults={max_results:?}): expected the pagination-token exception message, got {body:?}"
            );
            assert!(
                body.get("fieldList").is_none(),
                "case {name:?} (maxResults={max_results:?}): a cursor-decode failure names no member, got {body:?}"
            );
        }
    }
}

/// The other side of the same rule: a request that violates an input member's own
/// constraint keeps the member-constraint refusal — HTTP 400 `ValidationException`
/// naming the member — so a caller can still tell a shape violation from a stale
/// cursor by the message.
#[tokio::test]
async fn a_member_constraint_violation_is_not_reported_as_a_pagination_token_exception() {
    let router = build_router();
    let pagination_message = pagination_token_message("ListServices");

    // (case, maxResults, nextToken, the member whose constraint is violated)
    let cases: Vec<(&str, Option<i64>, Option<String>, &str)> = vec![
        ("nextToken one character below @length min", None, Some("A".repeat(31)), "nextToken"),
        ("nextToken far below @length min", None, Some("abc".to_string()), "nextToken"),
        ("nextToken one character above @length max", None, Some("A".repeat(513)), "nextToken"),
        ("nextToken outside @pattern", None, Some("!".repeat(40)), "nextToken"),
        ("nextToken with padding in the middle", None, Some(format!("{}{}", "=".repeat(4), "A".repeat(32))), "nextToken"),
        ("maxResults one below @range min", Some(0), None, "maxResults"),
        ("maxResults one above @range max", Some(1001), None, "maxResults"),
        ("negative maxResults", Some(-1), None, "maxResults"),
    ];

    for (name, max_results, next_token, member) in cases {
        let uri = list_services_uri(max_results, next_token.as_deref());
        let (status, error_type, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} ({body})"
        );
        assert_eq!(
            error_type, "ValidationException",
            "case {name:?}: expected a ValidationException, got {error_type:?} ({body})"
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name:?}: no message in {body}"));
        assert_ne!(
            message, pagination_message,
            "case {name:?}: a member-constraint violation must not borrow the pagination-token message, got {message:?}"
        );
        assert!(
            message.contains(member),
            "case {name:?}: expected the refusal to name {member:?}, got {message:?}"
        );
    }
}

/// The positive control for the same decode path: a `nextToken` the service DID
/// issue still resumes the listing, at every page size, and the resumed page is a
/// distinct, contiguous slice of the catalog.
#[tokio::test]
async fn an_issued_next_token_resumes_the_listing() {
    let router = build_router();
    let catalog = whole_catalog(&router).await;

    for size in [1_i64, 2, 3, 7] {
        let (first, token) = list_page(&router, Some(size), None).await;
        let token = token.unwrap_or_else(|| {
            panic!("case maxResults={size}: a first page must carry a nextToken")
        });
        let (second, _) = list_page(&router, Some(size), Some(&token)).await;

        assert_eq!(
            first,
            catalog[..size as usize].to_vec(),
            "case maxResults={size}: page one must be the catalog's first slice, got {first:?}"
        );
        assert_ne!(
            second.first(),
            first.first(),
            "case maxResults={size}: the resumed page must start past page one, got {second:?}"
        );
        let end = (2 * size as usize).min(catalog.len());
        assert_eq!(
            second,
            catalog[size as usize..end].to_vec(),
            "case maxResults={size}: the resumed page must be the next slice, got {second:?}"
        );
    }
}

/// Reads must not change what a later caller sees: a full paging walk and repeated
/// `GetAccountCustomizations` calls leave the catalog and the customization record
/// exactly as they were.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let router = build_router();
    let before_catalog = whole_catalog(&router).await;
    let (_, _, before_record) = get_customizations(&router).await;

    // Walk every page, twice, and read the record in between.
    for _ in 0..2 {
        let mut token: Option<String> = None;
        loop {
            let (_, next) = list_page(&router, Some(1), token.as_deref()).await;
            let (_, _, record) = get_customizations(&router).await;
            assert_eq!(
                record, before_record,
                "a read must not change the customization record, got {record:?}"
            );
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
    }

    assert_eq!(
        whole_catalog(&router).await,
        before_catalog,
        "a read must not change the catalog it reports"
    );
}
