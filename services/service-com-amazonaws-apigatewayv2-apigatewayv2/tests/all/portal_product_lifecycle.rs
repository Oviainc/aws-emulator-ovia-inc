// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `PortalProduct` lifecycle.
//!
//! The rules these cases assert:
//!
//! * **`CreatePortalProduct` persists an account-level portal product and answers 201**
//!   with the `PortalProductId` it minted (ten lowercase alphanumerics, which satisfies
//!   the modelled `__stringMin10Max30PatternAZ09`), the `PortalProductArn` naming the
//!   Region the request addressed, a `LastModified` reading the creation instant, and
//!   the `DisplayName`, `Description` and `Tags` the request carried — nothing invented
//!   for a member the request left out. `GetPortalProduct` reports that same record,
//!   member for member.
//! * **A portal product is addressed by its own minted id**: an id nothing minted is
//!   404 `NotFoundException`, and so is a read that names another account as the
//!   resource owner, since this emulator holds its own account's products only.
//! * **A `DisplayName` that names nothing is refused 400** and persists nothing: the
//!   modelled bound is `len 1..255` (`CreatePortalProduct$DisplayName`), and a name of
//!   nothing but whitespace is refused the same way.
//! * **`GetPortalProduct` is a report, not a write**: reading twice reports the same
//!   record, and a read leaves what a later caller sees unchanged.
//! * **One account holds at most `MAX_PORTAL_PRODUCTS` products**, after which a create
//!   is refused 429 `TooManyRequestsException`.
//!
//! The rows vary what each rule generalises over: the display name's length and script,
//! whether a description is absent, empty, maximal or outside ASCII, the tag count, and
//! the Region the request is signed for. The recorded lifecycle that filed the issue —
//! one create of one product, read back — is ONE row of the first table.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::api_lifecycle::{
    as_str, assert_minted_id, call, call_in_region, emulator, error_code, instant,
};

/// The portal-products collection URI (`CreatePortalProduct`'s modelled
/// `POST /v2/portalproducts`).
const COLLECTION: &str = "/v2/portalproducts";

/// How far the reported `LastModified` may sit from this test's clock reading, in
/// seconds. The service reports the instant at second granularity, so a whole second of
/// truncation plus scheduling slack is allowed; the point of the assertion is that the
/// member reads the creation instant rather than zero or a fixed literal.
const LAST_MODIFIED_SLACK_SECONDS: f64 = 60.0;

/// One accepted-create row: the request, and the members the stored product must report.
struct Row {
    case: &'static str,
    request: Value,
    /// The members `CreatePortalProduct` and `GetPortalProduct` must both report.
    expected: Vec<(&'static str, Value)>,
}

/// A description at the modelled maximum (`len 0..1024`).
fn max_description() -> String {
    "d".repeat(1024)
}

/// A display name at the modelled maximum (`len 1..255`).
fn max_display_name() -> String {
    "n".repeat(255)
}

fn accepted_rows() -> Vec<Row> {
    vec![
        Row {
            case: "every optional member set, with two tags",
            request: json!({
                "displayName": "Payments API",
                "description": "everything a payments integrator needs",
                "tags": { "team": "payments", "tier": "gold" }
            }),
            expected: vec![
                ("displayName", json!("Payments API")),
                ("description", json!("everything a payments integrator needs")),
            ],
        },
        Row {
            case: "only the required member: no description and no tags",
            request: json!({ "displayName": "Bare" }),
            expected: vec![("displayName", json!("Bare"))],
        },
        Row {
            case: "an empty description, which the modelled bound allows (len 0..1024)",
            request: json!({ "displayName": "Empty description", "description": "" }),
            expected: vec![("description", json!(""))],
        },
        Row {
            case: "a description at the modelled maximum length",
            request: json!({ "displayName": "Maximal", "description": max_description() }),
            expected: vec![("description", json!(max_description()))],
        },
        Row {
            case: "a description outside ASCII, which round-trips unchanged",
            request: json!({
                "displayName": "Unicode",
                "description": "決済 API — ünïcödé, emoji 🎉, and a combining é"
            }),
            expected: vec![(
                "description",
                json!("決済 API — ünïcödé, emoji 🎉, and a combining é"),
            )],
        },
        Row {
            case: "a display name outside ASCII",
            request: json!({ "displayName": "Zahlungs-API 決済", "description": "ok" }),
            expected: vec![("displayName", json!("Zahlungs-API 決済"))],
        },
        Row {
            case: "a one-character display name, the modelled minimum",
            request: json!({ "displayName": "x" }),
            expected: vec![("displayName", json!("x"))],
        },
        Row {
            case: "a display name at the modelled maximum length",
            request: json!({ "displayName": max_display_name() }),
            expected: vec![("displayName", json!(max_display_name()))],
        },
        Row {
            case: "one tag only",
            request: json!({ "displayName": "Single tag", "tags": { "owner": "api-team" } }),
            expected: vec![("displayName", json!("Single tag"))],
        },
        Row {
            case: "an empty tag collection",
            request: json!({ "displayName": "No tags", "tags": {} }),
            expected: vec![("displayName", json!("No tags"))],
        },
    ]
}

/// The `tags` member of one document, with an absent member read as an empty
/// collection — the ONE decoder a request and both readers are compared through, so an
/// empty collection and an absent one are not asserted as two different things.
fn tags_of(document: &Value) -> BTreeMap<String, String> {
    document
        .get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn create_portal_product_persists_the_record_and_get_reports_it_back() {
    let emulator = emulator();

    let mut minted: Vec<String> = Vec::new();
    for row in accepted_rows() {
        let (status, created) =
            call(&emulator, "POST", COLLECTION, Some(row.request.clone())).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreatePortalProduct, got {status} {created}",
            row.case
        );

        let product_id = as_str(&created, "portalProductId");
        assert_minted_id(row.case, "PortalProductId", &product_id);
        minted.push(product_id.clone());

        // The ARN names the resource and is at least the modelled 20 characters.
        let arn = as_str(&created, "portalProductArn");
        assert!(
            arn.ends_with(&format!("/portalproducts/{product_id}")) && arn.len() >= 20,
            "case {:?}: expected an ARN of the created product, got {arn:?}",
            row.case
        );

        // `LastModified` reads the creation instant.
        let now = ::std::time::SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or_default();
        let reported = instant(row.case, &created, "lastModified");
        assert!(
            (now - reported).abs() <= LAST_MODIFIED_SLACK_SECONDS,
            "case {:?}: expected LastModified to read the creation instant; got {reported} \
             against a clock reading {now}",
            row.case
        );

        // The same record, read back.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("{COLLECTION}/{product_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetPortalProduct, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "portalProductId"),
            product_id,
            "case {:?}: expected the stored product's own id, got {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "portalProductArn"),
            arn,
            "case {:?}: expected GetPortalProduct to report the stored ARN, got {fetched}",
            row.case
        );
        assert_eq!(
            instant(row.case, &fetched, "lastModified"),
            reported,
            "case {:?}: expected GetPortalProduct to report the stored LastModified, got \
             {fetched}",
            row.case
        );

        for (member, value) in &row.expected {
            assert_eq!(
                created.get(member),
                Some(value),
                "case {:?}: expected CreatePortalProduct to report {member}={value}, got \
                 {created}",
                row.case
            );
            assert_eq!(
                fetched.get(member),
                Some(value),
                "case {:?}: expected GetPortalProduct to report {member}={value}, got \
                 {fetched}",
                row.case
            );
        }

        // The tags the request carried, and no others.
        let expected_tags = tags_of(&row.request);
        assert_eq!(
            tags_of(&created),
            expected_tags,
            "case {:?}: expected CreatePortalProduct to report the request's tags, got \
             {created}",
            row.case
        );
        assert_eq!(
            tags_of(&fetched),
            expected_tags,
            "case {:?}: expected GetPortalProduct to report the request's tags, got {fetched}",
            row.case
        );

        // Nothing is invented for a member the request left out, and a product that owns
        // no pages has no ordering of them.
        if row.request.get("description").is_none() {
            assert!(
                fetched.get("description").is_none_or(Value::is_null),
                "case {:?}: expected GetPortalProduct to report no description, got {fetched}",
                row.case
            );
        }
        assert!(
            fetched.get("displayOrder").is_none_or(Value::is_null),
            "case {:?}: expected a freshly created product to report no displayOrder, got \
             {fetched}",
            row.case
        );
    }

    // Every create minted a distinct id.
    for (index, id) in minted.iter().enumerate() {
        assert_eq!(
            minted.iter().filter(|other| *other == id).count(),
            1,
            "product {index}: expected distinct ids, got {minted:?}"
        );
    }
}

#[tokio::test]
async fn the_product_arn_names_the_region_the_request_addressed() {
    let emulator = emulator();

    // The Region comes from the request's SigV4 credential scope, so a product created
    // through a request signed for another Region carries that Region in its ARN.
    for region in ["eu-west-1", "us-east-1", "ap-southeast-2"] {
        let (status, created) = call_in_region(
            &emulator,
            region,
            "POST",
            COLLECTION,
            Some(json!({ "displayName": format!("product in {region}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "region {region:?}: expected 201 from CreatePortalProduct, got {status} {created}"
        );
        let product_id = as_str(&created, "portalProductId");
        assert_eq!(
            as_str(&created, "portalProductArn"),
            format!(
                "arn:aws:apigateway:{region}:{}:/portalproducts/{product_id}",
                ::engine::emulator_account_id()
            ),
            "region {region:?}: expected the ARN to name that Region and the emulator \
             account, got {created}"
        );

        // The stored ARN does not follow the Region a LATER reader signs for.
        let (status, fetched) = call_in_region(
            &emulator,
            "us-west-2",
            "GET",
            &format!("{COLLECTION}/{product_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region:?}: expected 200 from GetPortalProduct, got {status} {fetched}"
        );
        assert_eq!(
            as_str(&fetched, "portalProductArn"),
            as_str(&created, "portalProductArn"),
            "region {region:?}: expected the stored ARN to keep its creation Region, got \
             {fetched}"
        );
    }
}

#[tokio::test]
async fn a_display_name_that_names_nothing_is_refused_and_persists_nothing() {
    let emulator = emulator();

    // `DisplayName` is `len 1..255`, so an empty or over-long name is refused at the
    // boundary; a name of nothing but whitespace satisfies the length bound and is
    // refused by the handler. Either refusal is a 400 naming the member.
    let rows: Vec<(&str, Value)> = vec![
        ("an empty display name", json!({ "displayName": "" })),
        (
            "a display name one character past the maximum",
            json!({ "displayName": "n".repeat(256) }),
        ),
        (
            "a display name of one space",
            json!({ "displayName": " ", "description": "blank" }),
        ),
        (
            "a display name of tabs and newlines",
            json!({ "displayName": "\t\n  " }),
        ),
        (
            "a description one character past the maximum",
            json!({ "displayName": "Fine", "description": "d".repeat(1025) }),
        ),
    ];

    for (case, request) in rows {
        let (status, body) = call(&emulator, "POST", COLLECTION, Some(request)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreatePortalProduct, got {status} {body}"
        );
        let code = error_code(&body);
        assert!(
            code == "BadRequestException" || code == "ValidationException",
            "case {case:?}: expected the request to be refused as invalid, got {code:?} {body}"
        );
    }

    // Every create above was refused, so nothing was persisted AND no id was minted: the
    // first product this account accepts carries the FIRST id of the store's own id
    // bijection, which is the id a fresh emulator's first product carries too.
    let (status, created) = call(
        &emulator,
        "POST",
        COLLECTION,
        Some(json!({ "displayName": "Accepted" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 for a valid product after the refusals, got {status} {created}"
    );
    let after_refusals = as_str(&created, "portalProductId");

    let fresh = crate::api_lifecycle::emulator();
    let (status, first) = call(
        &fresh,
        "POST",
        COLLECTION,
        Some(json!({ "displayName": "Accepted" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 for a fresh emulator's first product, got {status} {first}"
    );
    assert_eq!(
        after_refusals,
        as_str(&first, "portalProductId"),
        "expected the refused creates to have minted nothing, so the first accepted \
         product carries the id a fresh store's first product does, got \
         {after_refusals:?} and {first}"
    );
}

#[tokio::test]
async fn a_portal_product_is_addressed_by_its_own_minted_id() {
    let emulator = emulator();

    let (status, created) = call(
        &emulator,
        "POST",
        COLLECTION,
        Some(json!({ "displayName": "Addressed", "description": "the only product" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreatePortalProduct, got {status} {created}"
    );
    let product_id = as_str(&created, "portalProductId");

    // An id nothing minted names no product, whatever its shape.
    for (case, id) in [
        ("an id nothing minted", "nosuchprod0".to_string()),
        ("the id's uppercase form", product_id.to_uppercase()),
        ("the id with one character dropped", product_id[1..].to_string()),
        ("a long id nothing minted", "z".repeat(30)),
    ] {
        let (status, body) = call(&emulator, "GET", &format!("{COLLECTION}/{id}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetPortalProduct, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // `ResourceOwnerAccountId` selects WHOSE product to read. This emulator holds its own
    // account's products, so that account finds the product and any other does not.
    let (status, mine) = call(
        &emulator,
        "GET",
        &format!(
            "{COLLECTION}/{product_id}?resourceOwnerAccountId={}",
            ::engine::emulator_account_id()
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 when the resource owner is this account, got {status} {mine}"
    );
    assert_eq!(
        as_str(&mine, "portalProductId"),
        product_id,
        "expected the stored product, got {mine}"
    );

    for other_owner in ["111122223333", "000000000001"] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("{COLLECTION}/{product_id}?resourceOwnerAccountId={other_owner}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "owner {other_owner:?}: expected 404 for a product owned elsewhere, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "owner {other_owner:?}: expected NotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn reading_a_portal_product_changes_nothing_a_later_caller_sees() {
    let emulator = emulator();

    let (status, created) = call(
        &emulator,
        "POST",
        COLLECTION,
        Some(json!({
            "displayName": "Read twice",
            "description": "a report must not rewrite the record",
            "tags": { "k": "v" }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreatePortalProduct, got {status} {created}"
    );
    let product_id = as_str(&created, "portalProductId");
    let uri = format!("{COLLECTION}/{product_id}");

    let (_, first) = call(&emulator, "GET", &uri, None).await;
    // A refused read of another product, and a refused create, between the two reads:
    // neither may move what this product reports.
    let (_, _) = call(&emulator, "GET", &format!("{COLLECTION}/nosuchprod0"), None).await;
    let (_, _) = call(&emulator, "POST", COLLECTION, Some(json!({ "displayName": "" }))).await;
    let (status, second) = call(&emulator, "GET", &uri, None).await;

    assert_eq!(
        status, 200,
        "expected 200 from the second GetPortalProduct, got {status} {second}"
    );
    assert_eq!(
        first, second,
        "expected two reads of one product to report the same record, got {first} then \
         {second}"
    );
}

#[tokio::test]
async fn one_account_holds_at_most_the_portal_products_its_bound_allows() {
    // The bound the service enforces, read from the service itself rather than copied:
    // the create past it is refused with the modelled `TooManyRequestsException` ("a
    // limit has been exceeded").
    const LIMIT: usize =
        service_com_amazonaws_apigatewayv2_apigatewayv2::state::MAX_PORTAL_PRODUCTS;

    let emulator = emulator();
    for index in 0..LIMIT {
        let (status, created) = call(
            &emulator,
            "POST",
            COLLECTION,
            Some(json!({ "displayName": format!("product-{index}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "product {index} of {LIMIT}: expected 201 from CreatePortalProduct, got \
             {status} {created}"
        );
    }

    let (status, body) = call(
        &emulator,
        "POST",
        COLLECTION,
        Some(json!({ "displayName": "one too many" })),
    )
    .await;
    assert_eq!(
        status, 429,
        "expected 429 from the product past the bound of {LIMIT}, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {body}"
    );

    // A fresh store is bounded on its own: the full one does not exhaust it.
    let fresh = crate::api_lifecycle::emulator();
    let (status, created) = call(
        &fresh,
        "POST",
        COLLECTION,
        Some(json!({ "displayName": "room" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 in a fresh store, got {status} {created}"
    );
}
