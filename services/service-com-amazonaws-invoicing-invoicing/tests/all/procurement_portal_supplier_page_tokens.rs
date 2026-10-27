// SPDX-License-Identifier: CC0-1.0

//! The `NextToken` `ListProcurementPortalSuppliers` hands back, driven end to end over
//! this service's wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson1_0 body, so the
//! tokens these tests hand back in are exactly the ones an SDK paginator would.
//!
//! The rule under test: a pagination token this operation mints is one it accepts back —
//! it satisfies the model's own constraints on that member on BOTH sides of the
//! operation (`BasicStringWithoutSpace`, `@length(min: 0, max: 1024)`, `@pattern ^\S+$`
//! for the output `NextToken` and for the input one:
//! `smithy_query(query="operation", operation="ListProcurementPortalSuppliers")`) for
//! every preference the walk can report, so every walk can be completed to its last page.
//!
//! The table spans the lengths the walk's order is built from, which is where
//! caller-supplied text enters it: a preference's `SupplierIdentifier`
//! (`smithy_query(query="operation", operation="CreateProcurementPortalPreference")`) and
//! a `SellerOfRecords` entry (`smithy_query(query="shape", shape="SellerOfRecords")`)
//! both target `BasicStringWithoutSpace`, `@length(min: 0, max: 1024)` — asserted at one
//! character, in between, at that published maximum for the identifier, at it for the
//! identifier and the seller-of-record ids together, and one past it (refused by the
//! write, so no such preference is ever stored), each walked at several page sizes.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The portal these preferences are configured against, and the `PortalIdentifier` a
/// caller names it by (the portal name lowercased with its underscores hyphenated, the
/// identifier `ListProcurementPortals` reports). The longest value of
/// `ProcurementPortalName` (`smithy_query(query="shape",
/// shape="ProcurementPortalName")`), so it is the portal whose tokens carry the longest
/// namespace.
const PORTAL_NAME: &str = "SAP_BUSINESS_NETWORK";
const PORTAL_IDENTIFIER: &str = "sap-business-network";

/// `NextToken`'s published maximum length, the same on the input and the output member
/// (`smithy_query(query="operation", operation="ListProcurementPortalSuppliers")`).
const MAX_NEXT_TOKEN_LEN: usize = 1024;

/// `BasicStringWithoutSpace`'s published maximum length, which both a preference's
/// `SupplierIdentifier` and a `SellerOfRecords` entry target.
const MAX_IDENTIFIER_LEN: usize = 1024;

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson1_0 request, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Invoicing.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case}: expected HTTP 200 from {operation}, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `BasicStringWithoutSpace` of exactly `length` characters, opening with `tag` so two
/// values of the same length are distinct: non-space throughout, as `@pattern ^\S+$`
/// requires.
fn identifier(tag: char, length: usize) -> String {
    let mut value = String::with_capacity(length);
    value.push(tag);
    while value.chars().count() < length {
        value.push('x');
    }
    value.truncate(length.max(1));
    value
}

/// One supplier as this suite compares it: its identifier, its environment and the
/// seller of record it applies to (empty when it applies to all of them).
type Supplier = (String, String, String);

/// `value` as a `Supplier`.
fn supplier(case: &str, value: &Value) -> Supplier {
    let field = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let identifier = field("SupplierIdentifier");
    assert!(
        !identifier.is_empty(),
        "case {case}: expected a SupplierIdentifier in {value}"
    );
    (identifier, field("Environment"), field("SellerOfRecord"))
}

/// One row of the table: how long the caller-supplied text the walk's order is built
/// from is, and how much of it there is.
#[derive(Debug, Clone, Copy)]
struct TokenCase {
    case: &'static str,
    /// The length of the preference's `SupplierIdentifier` (and of its test
    /// environment's, when it has one).
    identifier_len: usize,
    /// How many seller-of-record ids the preference selects; none means it applies to
    /// every seller of record and so configures its identity once per environment.
    sellers: usize,
    /// The length of each selected seller-of-record id.
    seller_len: usize,
    /// Whether the preference also configures a test environment, which contributes a
    /// second supplier identity.
    test_env: bool,
}

/// The seller-of-record ids a row selects, each of its own length and distinct.
fn seller_ids(case: &TokenCase) -> Vec<String> {
    (0..case.sellers)
        .map(|index| {
            identifier(
                char::from(b'a' + u8::try_from(index).unwrap_or(0)),
                case.seller_len,
            )
        })
        .collect()
}

/// The `CreateProcurementPortalPreference` request a row stores.
fn create_request(case: &TokenCase) -> Value {
    let mut request = json!({
        "ProcurementPortalName": PORTAL_NAME,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": "page-token-buyer",
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": identifier('p', case.identifier_len),
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": false,
        // `Contacts` is modelled `@length(min: 1, max: 1)`.
        "Contacts": [{"Name": "Portal Administrator", "Email": "page-token@example.com"}],
    });
    if case.sellers > 0 {
        request["Selector"] = json!({"SellerOfRecords": seller_ids(case)});
    }
    if case.test_env {
        request["TestEnvPreference"] = json!({
            "BuyerDomain": "NetworkID",
            "BuyerIdentifier": "page-token-buyer-test",
            "SupplierDomain": "NetworkID",
            "SupplierIdentifier": identifier('t', case.identifier_len),
        });
    }
    request
}

/// The suppliers a row's preference configures, in the order a reader walks them (by
/// identifier, then environment, then seller of record): the production identity, the
/// test one when it configures a test environment, each once per selected seller-of-record
/// id and once with no seller of record when it selects none.
fn expected_suppliers(case: &TokenCase) -> Vec<Supplier> {
    let mut environments = vec![("PROD", identifier('p', case.identifier_len))];
    if case.test_env {
        environments.push(("TEST", identifier('t', case.identifier_len)));
    }
    let sellers = if case.sellers > 0 {
        seller_ids(case)
    } else {
        vec![String::new()]
    };
    environments
        .into_iter()
        .flat_map(|(environment, supplier_identifier)| {
            sellers.iter().map(move |seller| {
                (
                    supplier_identifier.clone(),
                    environment.to_string(),
                    seller.clone(),
                )
            })
        })
        .collect::<BTreeSet<Supplier>>()
        .into_iter()
        .collect()
}

/// Walk the whole listing `max_results` at a time, asserting of every `NextToken` the
/// walk is handed that it satisfies the model's constraints on that member, and handing
/// each one straight back as a paginator would. Answers the suppliers reported, in order,
/// and the tokens the walk was handed.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    max_results: i64,
) -> (Vec<Supplier>, Vec<String>) {
    let mut reported = Vec::new();
    let mut tokens: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({
            "PortalIdentifier": PORTAL_IDENTIFIER,
            "MaxResults": max_results,
        });
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let page = ok(router, case, "ListProcurementPortalSuppliers", &request).await;
        let items = page
            .get("ProcurementPortalSuppliers")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected ProcurementPortalSuppliers in {page}"))
            .clone();
        reported.extend(items.iter().map(|item| supplier(case, item)));
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(token) = next_token.clone() else {
            break;
        };
        assert!(
            token.len() <= MAX_NEXT_TOKEN_LEN,
            "case {case}: expected a NextToken within the modelled {MAX_NEXT_TOKEN_LEN} \
             characters, got {} characters",
            token.len()
        );
        assert!(
            !token.is_empty() && !token.chars().any(char::is_whitespace),
            "case {case}: expected a NextToken matching the modelled ^\\S+$, got {token:?}"
        );
        tokens.push(token);
        assert!(
            tokens.len() <= 256,
            "case {case}: the walk did not terminate within 256 pages"
        );
    }
    (reported, tokens)
}

/// The rows the rule is asserted over: one character of identifier, a short identifier
/// with several seller-of-record ids, an identifier long enough that naming a position by
/// the walk's own key would carry a token past `NextToken`'s maximum, the identifier at
/// its published maximum, and the identifier and the seller-of-record ids at that maximum
/// together.
fn token_cases() -> Vec<TokenCase> {
    vec![
        TokenCase {
            case: "one character of identifier, selecting every seller of record",
            identifier_len: 1,
            sellers: 0,
            seller_len: 0,
            test_env: true,
        },
        TokenCase {
            case: "a short identifier with three seller-of-record ids",
            identifier_len: 16,
            sellers: 3,
            seller_len: 8,
            test_env: false,
        },
        TokenCase {
            case: "an identifier past half the token's maximum",
            identifier_len: 462,
            sellers: 2,
            seller_len: 8,
            test_env: true,
        },
        TokenCase {
            case: "the identifier at its published maximum",
            identifier_len: MAX_IDENTIFIER_LEN,
            sellers: 2,
            seller_len: 8,
            test_env: false,
        },
        TokenCase {
            case: "the identifier and the seller-of-record ids both at their published maximum",
            identifier_len: MAX_IDENTIFIER_LEN,
            sellers: 3,
            seller_len: MAX_IDENTIFIER_LEN,
            test_env: true,
        },
    ]
}

#[tokio::test]
async fn every_minted_token_is_one_the_walk_accepts_back() {
    for token_case in token_cases() {
        let expected = expected_suppliers(&token_case);
        // Every page size that splits these listings, including the smallest the model
        // admits (`MaxResults` is `@range(min: 1, max: 100)`), so a token is minted at
        // every position of the walk.
        for max_results in [1, 2, 3] {
            let case = format!("{} at MaxResults {max_results}", token_case.case);
            // One emulator per row, so the portal reports exactly this row's suppliers.
            let router = emulator();
            ok(
                &router,
                &case,
                "CreateProcurementPortalPreference",
                &create_request(&token_case),
            )
            .await;

            let (reported, tokens) = walk(&router, &case, max_results).await;
            assert_eq!(
                reported, expected,
                "case {case}: expected the pages to concatenate to {expected:?}, got {reported:?}"
            );
            let page_size = usize::try_from(max_results).unwrap_or(1);
            let owed_tokens = expected.len().div_ceil(page_size).max(1) - 1;
            assert_eq!(
                tokens.len(),
                owed_tokens,
                "case {case}: expected {owed_tokens} tokens for {} suppliers, got {:?}",
                expected.len(),
                tokens.len()
            );
            // Every token is distinct: each names the position its own page resumes at.
            let distinct: BTreeSet<&String> = tokens.iter().collect();
            assert_eq!(
                distinct.len(),
                tokens.len(),
                "case {case}: expected {} distinct tokens, got {} ",
                tokens.len(),
                distinct.len()
            );
        }
    }
}

#[tokio::test]
async fn refuses_a_write_past_the_published_identifier_length() {
    let router = emulator();
    // One past `BasicStringWithoutSpace`'s published maximum, on each member the walk's
    // order is built from: refused by the write, so no preference the listing reports can
    // carry more than the maximum the rule above is asserted at.
    for (case, member, value) in [
        (
            "a supplier identifier one past the maximum",
            "SupplierIdentifier",
            json!(identifier('p', MAX_IDENTIFIER_LEN + 1)),
        ),
        (
            "a seller-of-record id one past the maximum",
            "Selector",
            json!({"SellerOfRecords": [identifier('a', MAX_IDENTIFIER_LEN + 1)]}),
        ),
    ] {
        let mut request = create_request(&TokenCase {
            case,
            identifier_len: 16,
            sellers: 0,
            seller_len: 0,
            test_env: false,
        });
        request[member] = value;
        let (status, body) = call(&router, "CreateProcurementPortalPreference", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case}: expected a ValidationException, got {body}"
        );
    }
}

#[tokio::test]
async fn refuses_a_token_whose_position_this_walk_never_minted() {
    let router = emulator();
    ok(
        &router,
        "a walk to mint against",
        "CreateProcurementPortalPreference",
        &create_request(&TokenCase {
            case: "a walk to mint against",
            identifier_len: 32,
            sellers: 3,
            seller_len: 8,
            test_env: false,
        }),
    )
    .await;

    // The token is opaque hex over this walk's own namespace and the position it resumes
    // at: a position that is not one this walk names is no more a token of its than hex
    // of another walk's namespace is.
    let forged = |position: &str| {
        hex::encode(format!(
            "procurement-portal-suppliers:{PORTAL_NAME}:{position}"
        ))
    };
    for (case, next_token) in [
        ("a position that is not a number", forged("not-a-position")),
        ("a negative position", forged("-1")),
        (
            "a position no listing could hold",
            forged("999999999999999999999999999999"),
        ),
        ("an empty position", forged("")),
        ("a token this service never minted", "not-a-token".to_string()),
    ] {
        let (status, body) = call(
            &router,
            "ListProcurementPortalSuppliers",
            &json!({"PortalIdentifier": PORTAL_IDENTIFIER, "NextToken": next_token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            body.get("Reason").or_else(|| body.get("reason")),
            Some(&json!("invalidNextToken")),
            "case {case}: expected the modelled invalidNextToken reason, got {body}"
        );
    }
}

#[tokio::test]
async fn resumes_a_walk_whose_listing_shrank_between_pages() {
    let router = emulator();
    let token_case = TokenCase {
        case: "a listing that shrinks mid-walk",
        identifier_len: MAX_IDENTIFIER_LEN,
        sellers: 2,
        seller_len: MAX_IDENTIFIER_LEN,
        test_env: false,
    };
    let created = ok(
        &router,
        token_case.case,
        "CreateProcurementPortalPreference",
        &create_request(&token_case),
    )
    .await;
    let arn = created
        .get("ProcurementPortalPreferenceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a ProcurementPortalPreferenceArn in {created}"))
        .to_string();

    let first = ok(
        &router,
        token_case.case,
        "ListProcurementPortalSuppliers",
        &json!({"PortalIdentifier": PORTAL_IDENTIFIER, "MaxResults": 1}),
    )
    .await;
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a NextToken in {first}"))
        .to_string();

    // The preference goes away before the walk resumes, so the position the token names
    // is past the end of what the portal now configures: the remainder of the walk is
    // empty and the walk ends, rather than the token this service minted being refused.
    ok(
        &router,
        token_case.case,
        "DeleteProcurementPortalPreference",
        &json!({"ProcurementPortalPreferenceArn": arn}),
    )
    .await;
    let resumed = ok(
        &router,
        token_case.case,
        "ListProcurementPortalSuppliers",
        &json!({"PortalIdentifier": PORTAL_IDENTIFIER, "MaxResults": 1, "NextToken": token}),
    )
    .await;
    assert_eq!(
        resumed.get("ProcurementPortalSuppliers"),
        Some(&json!([])),
        "case {}: expected an empty remainder, got {resumed}",
        token_case.case
    );
    assert_eq!(
        resumed.get("NextToken"),
        None,
        "case {}: expected no further NextToken, got {resumed}",
        token_case.case
    );
}
