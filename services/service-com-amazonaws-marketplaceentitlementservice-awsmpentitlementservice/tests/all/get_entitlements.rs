// SPDX-License-Identifier: CC0-1.0

//! `GetEntitlements` end to end over the router: seed the store, then read it back
//! through HTTP and assert the rules the API follows.
//!
//! Every case drives the real request walk — `MultiServiceRouter::dispatch` with an
//! awsJson1_1 `POST /` — so deserialization, modeled-constraint validation, the handler
//! and serialization are all exercised, not just the handler body.

use std::collections::BTreeSet;
use std::sync::Arc;

use service_com_amazonaws_marketplaceentitlementservice_awsmpentitlementservice::state::{
    seed_entitlements, service_state, StoredEntitlement, StoredEntitlementValue,
};
use service_com_amazonaws_marketplaceentitlementservice_awsmpentitlementservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The service's sigv4 signing name, which is what the router's `can_handle` matches the
/// request's resolved route key against (see `OPERATIONS.md`).
const SIGNING_NAME: &str = "aws-marketplace";

/// Product code carrying the filter/value fixtures.
const PROD_MAIN: &str = "prod-abc";
/// A second product code, to prove selection never leaks across products.
const PROD_OTHER: &str = "prod-xyz";
/// Product code carrying the pagination fixtures.
const PROD_PAGED: &str = "prod-paged";

/// How many entitlements `PROD_PAGED` is seeded with: more than the 25 the modeled
/// `PageSizeInteger` range allows as a maximum, so the unpaginated call has to page.
const PAGED_COUNT: usize = 30;

/// An engine plus a router serving only this service over it.
fn build_router() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// The fixtures for `PROD_MAIN`. Each has a unique `Dimension`, which the assertions use
/// as its identity; `Bare` deliberately carries nothing but a product code and dimension,
/// so a filter on an unset member has something to not match.
fn main_fixtures() -> Vec<StoredEntitlement> {
    vec![
        StoredEntitlement::new(PROD_MAIN)
            .with_dimension("Users")
            .with_customer_identifier("cust-1")
            .with_customer_aws_account_id("111122223333")
            .with_license_arn("arn:aws:license-manager::111122223333:license/l-a")
            .with_value(StoredEntitlementValue::integer(10))
            .with_expiration_date(aws_smithy_types::DateTime::from_secs(1_893_456_000)),
        StoredEntitlement::new(PROD_MAIN)
            .with_dimension("Data")
            .with_customer_identifier("cust-1")
            .with_customer_aws_account_id("111122223333")
            .with_license_arn("arn:aws:license-manager::111122223333:license/l-b")
            .with_value(StoredEntitlementValue::double(12.5)),
        StoredEntitlement::new(PROD_MAIN)
            .with_dimension("Support")
            .with_customer_identifier("cust-2")
            .with_customer_aws_account_id("222233334444")
            .with_license_arn("arn:aws:license-manager::222233334444:license/l-c")
            .with_value(StoredEntitlementValue::boolean(true)),
        StoredEntitlement::new(PROD_MAIN)
            .with_dimension("Tier")
            .with_customer_identifier("cust-3")
            .with_customer_aws_account_id("333344445555")
            .with_license_arn("arn:aws:license-manager::333344445555:license/l-d")
            .with_value(StoredEntitlementValue::string("GOLD")),
        StoredEntitlement::new(PROD_MAIN).with_dimension("Bare"),
    ]
}

/// Seed `PROD_MAIN`, `PROD_OTHER` and `PROD_PAGED` into `engine`'s store.
fn seed_all(engine: &engine::InMemoryEngine) {
    let mut records = main_fixtures();
    records.push(
        StoredEntitlement::new(PROD_OTHER)
            .with_dimension("Users")
            .with_customer_identifier("cust-1")
            .with_value(StoredEntitlementValue::integer(1)),
    );
    records.extend((0..PAGED_COUNT).map(|index| {
        StoredEntitlement::new(PROD_PAGED)
            .with_dimension(format!("p-{index:02}"))
            .with_customer_identifier("cust-1")
            .with_value(StoredEntitlementValue::integer(
                i32::try_from(index).expect("fixture index fits in i32"),
            ))
    }));
    let seeded = seed_entitlements(engine, records).expect("the shared engine holds the store");
    assert_eq!(
        seeded,
        main_fixtures().len() + 1 + PAGED_COUNT,
        "the seeding seam must append every record it was given"
    );
}

/// One `GetEntitlements` call. Returns the HTTP status and the parsed JSON body.
async fn get_entitlements(
    router: &MultiServiceRouter,
    body: &serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "AWSMPEntitlementService.GetEntitlements")
        // The routing key is read from the sigv4 credential scope's service field.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the fixture request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice::<serde_json::Value>(response.body())
        .unwrap_or(serde_json::Value::Null);
    (status, parsed)
}

/// The `Dimension` of each entitlement in a successful response, in wire order.
fn dimensions(body: &serde_json::Value) -> Vec<String> {
    body["Entitlements"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|entitlement| {
                    entitlement["Dimension"]
                        .as_str()
                        .unwrap_or("<missing>")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A successful call's entitlement dimensions, asserting HTTP 200 first.
async fn ok_dimensions(
    router: &MultiServiceRouter,
    case: &str,
    body: &serde_json::Value,
) -> Vec<String> {
    let (status, parsed) = get_entitlements(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {parsed:?}"
    );
    dimensions(&parsed)
}

#[tokio::test]
async fn product_code_selects_only_that_products_stored_entitlements() {
    let (engine, router) = build_router();
    seed_all(&engine);

    // Rows differ in how many entitlements the product has: five, one, thirty, none.
    let longest_product_code = "c".repeat(255);
    let cases: Vec<(&str, usize)> = vec![
        (PROD_MAIN, main_fixtures().len()),
        (PROD_OTHER, 1),
        (PROD_PAGED, 25), // one default-sized page; the rest follow the NextToken
        ("does-not-exist", 0),
        ("p", 0),                      // ProductCode length lower bound: 1
        (&longest_product_code, 0),    // ProductCode length upper bound: 255
    ];

    for (product_code, expected_len) in cases {
        let case = format!("ProductCode={:?}", &product_code[..product_code.len().min(12)]);
        let (status, parsed) =
            get_entitlements(&router, &serde_json::json!({"ProductCode": product_code})).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with body {parsed:?}"
        );
        let returned = dimensions(&parsed);
        assert_eq!(
            returned.len(),
            expected_len,
            "case {case}: expected {expected_len} entitlements, got {returned:?}"
        );
        for entitlement in parsed["Entitlements"]
            .as_array()
            .expect("Entitlements is always a list on a 200")
        {
            assert_eq!(
                entitlement["ProductCode"].as_str(),
                Some(product_code),
                "case {case}: an entitlement for another product leaked in: {entitlement:?}"
            );
        }
    }
}

#[tokio::test]
async fn product_code_outside_the_modeled_length_bounds_is_refused() {
    let (engine, router) = build_router();
    seed_all(&engine);

    // `ProductCode` has `@length(min: 1, max: 255)`; one past each bound must be refused.
    for (case, product_code) in [("below min", String::new()), ("above max", "c".repeat(256))] {
        let (status, parsed) =
            get_entitlements(&router, &serde_json::json!({"ProductCode": product_code})).await;
        assert_eq!(
            status, 400,
            "case {case:?} (len {}): expected HTTP 400, got {status} with body {parsed:?}",
            product_code.len()
        );
    }
}

#[tokio::test]
async fn every_stored_member_is_returned_as_stored() {
    let (engine, router) = build_router();
    seed_all(&engine);

    let (status, parsed) =
        get_entitlements(&router, &serde_json::json!({"ProductCode": PROD_MAIN})).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {parsed:?}");
    let returned = parsed["Entitlements"]
        .as_array()
        .expect("Entitlements is always a list on a 200")
        .clone();
    assert_eq!(
        returned.len(),
        main_fixtures().len(),
        "expected every seeded entitlement, got {returned:?}"
    );

    for (stored, wire) in main_fixtures().iter().zip(returned.iter()) {
        let case = stored.dimension.clone().unwrap_or_default();
        assert_eq!(
            wire["ProductCode"].as_str(),
            Some(stored.product_code.as_str()),
            "case {case:?}: expected the stored ProductCode, got {wire:?}"
        );
        assert_eq!(
            wire["Dimension"].as_str(),
            stored.dimension.as_deref(),
            "case {case:?}: expected the stored Dimension, got {wire:?}"
        );
        assert_eq!(
            wire["CustomerIdentifier"].as_str(),
            stored.customer_identifier.as_deref(),
            "case {case:?}: expected the stored CustomerIdentifier, got {wire:?}"
        );
        assert_eq!(
            wire["CustomerAWSAccountId"].as_str(),
            stored.customer_aws_account_id.as_deref(),
            "case {case:?}: expected the stored CustomerAWSAccountId, got {wire:?}"
        );
        assert_eq!(
            wire["LicenseArn"].as_str(),
            stored.license_arn.as_deref(),
            "case {case:?}: expected the stored LicenseArn, got {wire:?}"
        );
        match stored.expiration_date {
            None => assert!(
                wire.get("ExpirationDate").is_none_or(serde_json::Value::is_null),
                "case {case:?}: no ExpirationDate is stored, yet one was returned: {wire:?}"
            ),
            Some(expiration) => assert_eq!(
                wire["ExpirationDate"].as_f64(),
                Some(expiration.as_secs_f64()),
                "case {case:?}: expected the stored ExpirationDate, got {wire:?}"
            ),
        }

        // `EntitlementValue` populates exactly the one member matching the entitlement's
        // type; the other three stay unset.
        match &stored.value {
            None => assert!(
                wire.get("Value").is_none_or(serde_json::Value::is_null),
                "case {case:?}: no Value is stored, yet one was returned: {wire:?}"
            ),
            Some(value) => {
                assert_eq!(
                    value.populated_count(),
                    1,
                    "case {case:?}: the fixture itself must populate exactly one member of \
                     EntitlementValue, got {value:?}"
                );
                let wire_value = &wire["Value"];
                let populated: BTreeSet<&str> = ["IntegerValue", "DoubleValue", "BooleanValue", "StringValue"]
                    .into_iter()
                    .filter(|member| {
                        wire_value
                            .get(*member)
                            .is_some_and(|found| !found.is_null())
                    })
                    .collect();
                assert_eq!(
                    populated.len(),
                    1,
                    "case {case:?}: expected exactly one populated EntitlementValue member, \
                     got {populated:?} in {wire_value:?}"
                );
                assert_eq!(
                    wire_value["IntegerValue"].as_i64(),
                    value.integer_value.map(i64::from),
                    "case {case:?}: expected the stored IntegerValue, got {wire_value:?}"
                );
                assert_eq!(
                    wire_value["DoubleValue"].as_f64(),
                    value.double_value,
                    "case {case:?}: expected the stored DoubleValue, got {wire_value:?}"
                );
                assert_eq!(
                    wire_value["BooleanValue"].as_bool(),
                    value.boolean_value,
                    "case {case:?}: expected the stored BooleanValue, got {wire_value:?}"
                );
                assert_eq!(
                    wire_value["StringValue"].as_str(),
                    value.string_value.as_deref(),
                    "case {case:?}: expected the stored StringValue, got {wire_value:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn filter_values_union_within_a_key_and_keys_intersect() {
    let (engine, router) = build_router();
    seed_all(&engine);

    // The four keys are the values of the modeled `GetEntitlementFilterName` enum
    // (CUSTOMER_IDENTIFIER, DIMENSION, CUSTOMER_AWS_ACCOUNT_ID, LICENSE_ARN); every one
    // appears below, alone and intersected with another.
    let cases: Vec<(&str, serde_json::Value, Vec<&str>)> = vec![
        (
            "no filter returns every entitlement of the product",
            serde_json::json!({}),
            vec!["Users", "Data", "Support", "Tier", "Bare"],
        ),
        (
            "one DIMENSION value",
            serde_json::json!({"DIMENSION": ["Users"]}),
            vec!["Users"],
        ),
        (
            "two DIMENSION values are unioned",
            serde_json::json!({"DIMENSION": ["Users", "Support"]}),
            vec!["Users", "Support"],
        ),
        (
            "three DIMENSION values are unioned, unknown members ignored",
            serde_json::json!({"DIMENSION": ["Tier", "no-such-dimension", "Bare"]}),
            vec!["Tier", "Bare"],
        ),
        (
            "one CUSTOMER_IDENTIFIER matching two entitlements",
            serde_json::json!({"CUSTOMER_IDENTIFIER": ["cust-1"]}),
            vec!["Users", "Data"],
        ),
        (
            "CUSTOMER_IDENTIFIER values are unioned",
            serde_json::json!({"CUSTOMER_IDENTIFIER": ["cust-2", "cust-3"]}),
            vec!["Support", "Tier"],
        ),
        (
            "CUSTOMER_IDENTIFIER intersected with DIMENSION",
            serde_json::json!({"CUSTOMER_IDENTIFIER": ["cust-1"], "DIMENSION": ["Data"]}),
            vec!["Data"],
        ),
        (
            "an intersection with no overlap is empty",
            serde_json::json!({"CUSTOMER_IDENTIFIER": ["cust-1"], "DIMENSION": ["Support"]}),
            vec![],
        ),
        (
            "one CUSTOMER_AWS_ACCOUNT_ID",
            serde_json::json!({"CUSTOMER_AWS_ACCOUNT_ID": ["222233334444"]}),
            vec!["Support"],
        ),
        (
            "CUSTOMER_AWS_ACCOUNT_ID values are unioned",
            serde_json::json!({"CUSTOMER_AWS_ACCOUNT_ID": ["111122223333", "333344445555"]}),
            vec!["Users", "Data", "Tier"],
        ),
        (
            "one LICENSE_ARN",
            serde_json::json!({"LICENSE_ARN": ["arn:aws:license-manager::222233334444:license/l-c"]}),
            vec!["Support"],
        ),
        (
            "LICENSE_ARN intersected with DIMENSION",
            serde_json::json!({
                "LICENSE_ARN": ["arn:aws:license-manager::111122223333:license/l-a"],
                "DIMENSION": ["Users"]
            }),
            vec!["Users"],
        ),
        (
            "LICENSE_ARN intersected with a disjoint CUSTOMER_AWS_ACCOUNT_ID",
            serde_json::json!({
                "LICENSE_ARN": ["arn:aws:license-manager::111122223333:license/l-a"],
                "CUSTOMER_AWS_ACCOUNT_ID": ["222233334444"]
            }),
            vec![],
        ),
        (
            "a key whose member is unset on an entitlement never matches it",
            serde_json::json!({"CUSTOMER_IDENTIFIER": ["cust-1", "cust-2", "cust-3"]}),
            vec!["Users", "Data", "Support", "Tier"],
        ),
        (
            "no entitlement matches the value",
            serde_json::json!({"DIMENSION": ["no-such-dimension"]}),
            vec![],
        ),
    ];

    for (case, filter, expected) in cases {
        let mut body = serde_json::json!({"ProductCode": PROD_MAIN});
        if filter.as_object().is_some_and(|map| !map.is_empty()) {
            body["Filter"] = filter;
        }
        let returned = ok_dimensions(&router, case, &body).await;
        assert_eq!(
            returned, expected,
            "case {case:?}: expected dimensions {expected:?}, got {returned:?}"
        );
    }
}

#[tokio::test]
async fn mutually_exclusive_customer_filter_keys_are_refused() {
    let (engine, router) = build_router();
    seed_all(&engine);

    // CustomerIdentifier and CustomerAWSAccountId name the same buyer two ways, so the
    // model's docs declare them mutually exclusive in one request. Each row pairs them
    // with a different third key / value count, so the refusal is not keyed to one shape.
    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "both keys, one value each",
            serde_json::json!({
                "CUSTOMER_IDENTIFIER": ["cust-1"],
                "CUSTOMER_AWS_ACCOUNT_ID": ["111122223333"]
            }),
        ),
        (
            "both keys, several values each",
            serde_json::json!({
                "CUSTOMER_IDENTIFIER": ["cust-1", "cust-2"],
                "CUSTOMER_AWS_ACCOUNT_ID": ["111122223333", "222233334444"]
            }),
        ),
        (
            "both keys alongside DIMENSION",
            serde_json::json!({
                "CUSTOMER_IDENTIFIER": ["cust-1"],
                "CUSTOMER_AWS_ACCOUNT_ID": ["111122223333"],
                "DIMENSION": ["Users"]
            }),
        ),
        (
            "both keys, naming values that match nothing",
            serde_json::json!({
                "CUSTOMER_IDENTIFIER": ["nobody"],
                "CUSTOMER_AWS_ACCOUNT_ID": ["000000000000"]
            }),
        ),
    ];

    for (case, filter) in cases {
        let (status, parsed) = get_entitlements(
            &router,
            &serde_json::json!({"ProductCode": PROD_MAIN, "Filter": filter}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {parsed:?}"
        );
        assert!(
            parsed.to_string().contains("InvalidParameterException"),
            "case {case:?}: expected the InvalidParameterException error code, got {parsed:?}"
        );
    }

    // Either key on its own is fine — the refusal is about the combination.
    for (case, filter) in [
        (
            "CUSTOMER_IDENTIFIER alone",
            serde_json::json!({"CUSTOMER_IDENTIFIER": ["cust-1"]}),
        ),
        (
            "CUSTOMER_AWS_ACCOUNT_ID alone",
            serde_json::json!({"CUSTOMER_AWS_ACCOUNT_ID": ["111122223333"]}),
        ),
    ] {
        let returned = ok_dimensions(
            &router,
            case,
            &serde_json::json!({"ProductCode": PROD_MAIN, "Filter": filter}),
        )
        .await;
        assert_eq!(
            returned,
            vec!["Users".to_string(), "Data".to_string()],
            "case {case:?}: expected the cust-1 entitlements, got {returned:?}"
        );
    }
}

#[tokio::test]
async fn pages_partition_the_unpaginated_result_exactly() {
    let (engine, router) = build_router();
    seed_all(&engine);

    let expected: Vec<String> = (0..PAGED_COUNT).map(|index| format!("p-{index:02}")).collect();

    // `MaxResults` targets `PageSizeInteger`, `@range(min: 1, max: 25)`: both published
    // bounds are here, plus sizes in between and the absent case (which must behave as
    // the maximum, since that is the widest page the API can be asked for).
    let cases: Vec<(&str, Option<i64>)> = vec![
        ("MaxResults absent", None),
        ("MaxResults at the range minimum", Some(1)),
        ("MaxResults 2", Some(2)),
        ("MaxResults 7", Some(7)),
        ("MaxResults 24", Some(24)),
        ("MaxResults at the range maximum", Some(25)),
    ];

    for (case, max_results) in cases {
        let mut collected: Vec<String> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;

        loop {
            let mut body = serde_json::json!({"ProductCode": PROD_PAGED});
            if let Some(max_results) = max_results {
                body["MaxResults"] = serde_json::json!(max_results);
            }
            if let Some(token) = &next_token {
                body["NextToken"] = serde_json::json!(token);
            }

            let (status, parsed) = get_entitlements(&router, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?} page {pages}: expected HTTP 200, got {status} with body {parsed:?}"
            );
            let page = dimensions(&parsed);
            let page_cap = max_results.unwrap_or(25);
            assert!(
                i64::try_from(page.len()).expect("a page length fits in i64") <= page_cap,
                "case {case:?} page {pages}: expected at most {page_cap} entitlements, \
                 got {} ({page:?})",
                page.len()
            );
            for dimension in &page {
                assert!(
                    seen.insert(dimension.clone()),
                    "case {case:?} page {pages}: {dimension:?} was already returned on an \
                     earlier page; pages must not overlap"
                );
            }
            collected.extend(page);
            pages += 1;

            next_token = parsed["NextToken"].as_str().map(str::to_string);
            if let Some(token) = &next_token {
                assert!(
                    !token.is_empty() && !token.chars().any(char::is_whitespace),
                    "case {case:?} page {pages}: NextToken must match ^\\S+$, got {token:?}"
                );
            } else {
                break;
            }
            assert!(
                pages <= PAGED_COUNT + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            collected, expected,
            "case {case:?}: the pages concatenated must equal the whole result in order, \
             got {collected:?}"
        );
        let expected_pages = max_results
            .unwrap_or(25)
            .try_into()
            .map(|size: usize| PAGED_COUNT.div_ceil(size))
            .expect("the page size is positive");
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

#[tokio::test]
async fn max_results_outside_the_modeled_range_is_refused() {
    let (engine, router) = build_router();
    seed_all(&engine);

    // `PageSizeInteger` is `@range(min: 1, max: 25)`; one past each bound is refused.
    for (case, max_results) in [("below min", 0), ("above max", 26), ("negative", -1)] {
        let (status, parsed) = get_entitlements(
            &router,
            &serde_json::json!({"ProductCode": PROD_PAGED, "MaxResults": max_results}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} (MaxResults={max_results}): expected HTTP 400, got {status} with \
             body {parsed:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_that_is_not_a_cursor_of_this_query_is_refused() {
    let (engine, router) = build_router();
    seed_all(&engine);

    // A real token, to show the refusals below are about the token and not the request.
    let (status, first) = get_entitlements(
        &router,
        &serde_json::json!({"ProductCode": PROD_PAGED, "MaxResults": 2}),
    )
    .await;
    assert_eq!(status, 200, "the first page must succeed, got {first:?}");
    let issued = first["NextToken"]
        .as_str()
        .expect("a 30-record product paged at 2 has a second page")
        .to_string();

    // Rows differ along the shape of the forgery: free text, a plausible structure, a
    // valid token with a tampered offset or digest, and a valid token for another query.
    let mut cases: Vec<(&str, String, &str)> = vec![
        ("free text", "not-a-token".to_string(), PROD_PAGED),
        ("plausible structure", "30.deadbeef".to_string(), PROD_PAGED),
        ("hex offset, invented digest", format!("{}.{}", hex::encode("2"), "0".repeat(32)), PROD_PAGED),
        ("digest only", format!(".{}", "0".repeat(32)), PROD_PAGED),
        ("offset only", hex::encode("2"), PROD_PAGED),
        ("a token issued for another product code", issued.clone(), PROD_MAIN),
    ];
    // The digest half tampered with, one hex character at a time.
    let mut tampered = issued.clone();
    let last = tampered.pop().expect("the issued token is non-empty");
    tampered.push(if last == 'a' { 'b' } else { 'a' });
    cases.push(("tampered digest", tampered, PROD_PAGED));

    for (case, token, product_code) in cases {
        let (status, parsed) = get_entitlements(
            &router,
            &serde_json::json!({"ProductCode": product_code, "NextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} (token {token:?}): expected HTTP 400, got {status} with body {parsed:?}"
        );
    }

    // A token issued for one filter does not resume a differently-filtered query.
    let (status, filtered) = get_entitlements(
        &router,
        &serde_json::json!({
            "ProductCode": PROD_PAGED,
            "Filter": {"CUSTOMER_IDENTIFIER": ["cust-1"]},
            "MaxResults": 2
        }),
    )
    .await;
    assert_eq!(status, 200, "the filtered first page must succeed, got {filtered:?}");
    let filtered_token = filtered["NextToken"]
        .as_str()
        .expect("the filtered query has a second page")
        .to_string();
    let (status, parsed) = get_entitlements(
        &router,
        &serde_json::json!({"ProductCode": PROD_PAGED, "NextToken": filtered_token}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a token issued for another filter\": expected HTTP 400, got {status} with \
         body {parsed:?}"
    );

    // And the token really is usable for the query it was issued for.
    let (status, second) = get_entitlements(
        &router,
        &serde_json::json!({"ProductCode": PROD_PAGED, "MaxResults": 2, "NextToken": issued}),
    )
    .await;
    assert_eq!(status, 200, "the issued token must resume the query, got {second:?}");
    assert_eq!(
        dimensions(&second),
        vec!["p-02".to_string(), "p-03".to_string()],
        "the issued token must resume exactly where page one ended"
    );
}

#[tokio::test]
async fn reading_entitlements_never_changes_what_a_later_caller_sees() {
    let (engine, router) = build_router();
    seed_all(&engine);

    let snapshot = {
        let state = service_state(&*engine).expect("the shared engine holds the store");
        format!("{:?}", state.lock().entitlements)
    };

    // A spread of reads: plain, filtered, paginated, a miss, and a refusal.
    let bodies = vec![
        serde_json::json!({"ProductCode": PROD_MAIN}),
        serde_json::json!({"ProductCode": PROD_MAIN, "Filter": {"DIMENSION": ["Users"]}}),
        serde_json::json!({"ProductCode": PROD_PAGED, "MaxResults": 1}),
        serde_json::json!({"ProductCode": "does-not-exist"}),
        serde_json::json!({"ProductCode": PROD_MAIN, "NextToken": "not-a-token"}),
    ];

    let mut first_answers: Vec<serde_json::Value> = Vec::new();
    for body in &bodies {
        let (_, parsed) = get_entitlements(&router, body).await;
        first_answers.push(parsed);
    }
    for (body, first) in bodies.iter().zip(first_answers.iter()) {
        let (_, again) = get_entitlements(&router, body).await;
        assert_eq!(
            &again, first,
            "case {body:?}: a repeated read answered differently, so a read mutated state"
        );
    }

    let after = {
        let state = service_state(&*engine).expect("the shared engine holds the store");
        format!("{:?}", state.lock().entitlements)
    };
    assert_eq!(
        after, snapshot,
        "GetEntitlements is a read: the store must be byte-identical after every call"
    );
}
