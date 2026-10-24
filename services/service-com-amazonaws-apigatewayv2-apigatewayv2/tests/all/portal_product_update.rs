// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `UpdatePortalProduct`.
//!
//! The rules these cases assert:
//!
//! * **`UpdatePortalProduct` is a PATCH** (`PATCH /v2/portalproducts/{PortalProductId}`):
//!   a member the request carries replaces the stored one, a member it omits leaves the
//!   stored value exactly as it was, and the members this operation does not take (the
//!   product's id, its ARN and its tags) are carried over untouched. The patched record
//!   is what the response reports AND what a later `GetPortalProduct` reports.
//! * **`LastModified` advances when the patch carries a member to apply**, and a patch
//!   that carries none leaves the record — `LastModified` included — exactly as it was.
//! * **A product is patched only through its own minted id**: an id nothing minted is
//!   404 `NotFoundException` and writes nothing.
//! * **A patch whose values are invalid is refused 400 and persists nothing**: the
//!   modelled bounds are `DisplayName` `len 1..255` and `Description` `len 0..1024`
//!   (`UpdatePortalProduct$DisplayName` / `$Description`), and a display name of nothing
//!   but whitespace is refused the same way `CreatePortalProduct` refuses it.
//!
//! The rows vary what the rules generalise over: WHICH members the patch carries (each
//! one alone, all of them, none), the value's length at and past each published bound,
//! its script, whether the member was set before the patch, and whether the patch
//! repeats the value already stored. The recorded portal-product lifecycle that filed
//! the issue — create, patch, read back — is ONE row of the first table.

use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::api_lifecycle::{as_str, call, emulator, error_code, instant};

/// The portal-products collection URI (`CreatePortalProduct`'s modelled
/// `POST /v2/portalproducts`); one product hangs off it as `{COLLECTION}/{id}`, the URI
/// both `GetPortalProduct` and `UpdatePortalProduct` are bound to.
const COLLECTION: &str = "/v2/portalproducts";

/// The members `UpdatePortalProduct` takes as values of the product, from the modelled
/// input (`PortalProductId` is the httpLabel addressing the product, not a value of it).
const PATCHABLE: [&str; 3] = ["displayName", "description", "displayOrder"];

/// How long to wait so a patch lands in a LATER second than the create. The service
/// reports `LastModified` at whole-second granularity, so only a wait past a second
/// boundary makes the move observable on the wire. A BLOCKING wait on purpose, for the
/// reason given in `deployment_stage_release`: this workspace's `tokio` carries no
/// `time` feature.
const SECOND_BOUNDARY_WAIT: Duration = Duration::from_millis(1_100);

/// How far a reported `LastModified` may sit from this test's clock reading, in seconds.
const LAST_MODIFIED_SLACK_SECONDS: f64 = 60.0;

/// One patch row: the product to create, an optional patch applied BEFORE the one under
/// test (so a member that was already set is covered too), and the patch under test.
struct Row {
    case: &'static str,
    create: Value,
    earlier_patch: Option<Value>,
    patch: Value,
}

/// A `DisplayOrder` document. Its page ARNs are `len 20..2048`
/// (`DisplayOrder$OverviewPageArn`, `__listOf__stringMin20Max2048`), so they are spelled
/// as full ARNs of this emulator's account.
fn display_order(section: &str, pages: usize) -> Value {
    let account = ::engine::emulator_account_id();
    let page_arns: Vec<String> = (0..pages)
        .map(|index| format!("arn:aws:apigateway:eu-west-1:{account}:/portalpages/page{index}"))
        .collect();
    json!({
        "overviewPageArn": format!("arn:aws:apigateway:eu-west-1:{account}:/portalpages/overview"),
        "productPageArns": page_arns,
        "contents": [{
            "sectionName": section,
            "productRestEndpointPageArns": [
                format!("arn:aws:apigateway:eu-west-1:{account}:/portalendpointpages/endpoint0")
            ]
        }]
    })
}

/// A description at the modelled maximum (`len 0..1024`).
fn max_description() -> String {
    "d".repeat(1024)
}

/// A display name at the modelled maximum (`len 1..255`).
fn max_display_name() -> String {
    "n".repeat(255)
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the display name alone, leaving the description and tags as they were",
            create: json!({
                "displayName": "Payments API",
                "description": "everything a payments integrator needs",
                "tags": { "team": "payments", "tier": "gold" }
            }),
            earlier_patch: None,
            patch: json!({ "displayName": "Payments API v2" }),
        },
        Row {
            case: "the description alone",
            create: json!({ "displayName": "Catalog", "description": "the old description" }),
            earlier_patch: None,
            patch: json!({ "description": "the new description" }),
        },
        Row {
            case: "a description onto a product created without one",
            create: json!({ "displayName": "Bare" }),
            earlier_patch: None,
            patch: json!({ "description": "now it has one" }),
        },
        Row {
            case: "the description set to empty, which the modelled bound allows (len 0..1024)",
            create: json!({ "displayName": "Emptied", "description": "something" }),
            earlier_patch: None,
            patch: json!({ "description": "" }),
        },
        Row {
            case: "the description set to the modelled maximum length",
            create: json!({ "displayName": "Maximal", "description": "short" }),
            earlier_patch: None,
            patch: json!({ "description": max_description() }),
        },
        Row {
            case: "the display order alone, onto a product that had none",
            create: json!({ "displayName": "Ordered" }),
            earlier_patch: None,
            patch: json!({ "displayOrder": display_order("Getting started", 2) }),
        },
        Row {
            case: "a display order replacing the one an earlier patch stored",
            create: json!({ "displayName": "Reordered" }),
            earlier_patch: Some(json!({ "displayOrder": display_order("First", 3) })),
            patch: json!({ "displayOrder": display_order("Second", 1) }),
        },
        Row {
            case: "a display order carrying no product pages",
            create: json!({ "displayName": "Empty order" }),
            earlier_patch: None,
            patch: json!({ "displayOrder": display_order("Only a section", 0) }),
        },
        Row {
            case: "every patchable member at once",
            create: json!({ "displayName": "All at once", "tags": { "k": "v" } }),
            earlier_patch: None,
            patch: json!({
                "displayName": "All at once, patched",
                "description": "and a description",
                "displayOrder": display_order("Everything", 1)
            }),
        },
        Row {
            case: "a display name at the modelled maximum length",
            create: json!({ "displayName": "Long name" }),
            earlier_patch: None,
            patch: json!({ "displayName": max_display_name() }),
        },
        Row {
            case: "a one-character display name, the modelled minimum",
            create: json!({ "displayName": "Short name" }),
            earlier_patch: None,
            patch: json!({ "displayName": "x" }),
        },
        Row {
            case: "values outside ASCII, which round-trip unchanged",
            create: json!({ "displayName": "Unicode", "description": "ascii" }),
            earlier_patch: None,
            patch: json!({
                "displayName": "Zahlungs-API 決済",
                "description": "決済 API — ünïcödé, emoji 🎉, and a combining é"
            }),
        },
        Row {
            case: "the value the product already carries, repeated",
            create: json!({ "displayName": "Idempotent", "description": "unchanged" }),
            earlier_patch: None,
            patch: json!({ "displayName": "Idempotent", "description": "unchanged" }),
        },
        Row {
            case: "a second patch over the members an earlier one set",
            create: json!({ "displayName": "Twice", "tags": { "owner": "api-team" } }),
            earlier_patch: Some(json!({
                "description": "set by the first patch",
                "displayOrder": display_order("From the first patch", 2)
            })),
            patch: json!({ "displayName": "Twice, patched" }),
        },
    ]
}

/// One member of a document, with an absent member read as `null` — so "absent" and
/// "reported as null" are not asserted as two different things.
fn member(document: &Value, name: &str) -> Value {
    strip_nulls(document.get(name).unwrap_or(&Value::Null))
}

/// `value` with every null object entry dropped, recursively: the request and the two
/// readers are compared through this ONE normalizer, so a structure member a reader
/// omits and one it reports as null compare equal.
fn strip_nulls(value: &Value) -> Value {
    match value {
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .filter(|(_, entry)| !entry.is_null())
                .map(|(key, entry)| (key.clone(), strip_nulls(entry)))
                .collect::<Map<String, Value>>(),
        ),
        Value::Array(entries) => Value::Array(entries.iter().map(strip_nulls).collect()),
        other => other.clone(),
    }
}

/// The `tags` member of one document, with an absent member read as an empty collection.
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

/// Create one product and return its id.
async fn create_product(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: &Value,
) -> String {
    let (status, created) = call(emulator, "POST", COLLECTION, Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreatePortalProduct, got {status} {created}"
    );
    as_str(&created, "portalProductId")
}

/// Read one product back, asserting the read succeeded.
async fn get_product(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    product_id: &str,
) -> Value {
    let (status, fetched) = call(
        emulator,
        "GET",
        &format!("{COLLECTION}/{product_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetPortalProduct, got {status} {fetched}"
    );
    fetched
}

#[tokio::test]
async fn a_patch_replaces_the_members_it_carries_and_leaves_the_others_as_they_were() {
    let emulator = emulator();

    for row in rows() {
        let product_id = create_product(&emulator, row.case, &row.create).await;
        let uri = format!("{COLLECTION}/{product_id}");

        if let Some(earlier) = &row.earlier_patch {
            let (status, patched) = call(&emulator, "PATCH", &uri, Some(earlier.clone())).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected 200 from the earlier UpdatePortalProduct, got {status} \
                 {patched}",
                row.case
            );
        }

        // The record as it stands before the patch under test.
        let before = get_product(&emulator, row.case, &product_id).await;

        let (status, patched) = call(&emulator, "PATCH", &uri, Some(row.patch.clone())).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from UpdatePortalProduct, got {status} {patched}",
            row.case
        );

        // Every patchable member: the one the request carried, else the stored one.
        for name in PATCHABLE {
            let expected = if row.patch.get(name).is_some() {
                member(&row.patch, name)
            } else {
                member(&before, name)
            };
            assert_eq!(
                member(&patched, name),
                expected,
                "case {:?}: expected UpdatePortalProduct to report {name}={expected}, got \
                 {patched}",
                row.case
            );
        }

        // The members this operation does not take are carried over untouched.
        assert_eq!(
            as_str(&patched, "portalProductId"),
            product_id,
            "case {:?}: expected the patched product's own id, got {patched}",
            row.case
        );
        assert_eq!(
            as_str(&patched, "portalProductArn"),
            as_str(&before, "portalProductArn"),
            "case {:?}: expected the stored ARN to survive the patch, got {patched}",
            row.case
        );
        assert_eq!(
            tags_of(&patched),
            tags_of(&before),
            "case {:?}: expected the stored tags to survive the patch, got {patched}",
            row.case
        );

        // What the response reports is what a later reader reports: the patch was
        // persisted, not echoed.
        let fetched = get_product(&emulator, row.case, &product_id).await;
        assert_eq!(
            strip_nulls(&fetched),
            strip_nulls(&patched),
            "case {:?}: expected GetPortalProduct to report the patched record, got {fetched} \
             against {patched}",
            row.case
        );

        // The patch carried a member, so the record's modification instant reads the
        // patch's own clock rather than a fixed literal.
        let now = ::std::time::SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or_default();
        let reported = instant(row.case, &fetched, "lastModified");
        assert!(
            (now - reported).abs() <= LAST_MODIFIED_SLACK_SECONDS,
            "case {:?}: expected LastModified to read the patch instant; got {reported} \
             against a clock reading {now}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_patch_that_carries_no_member_leaves_the_record_exactly_as_it_was() {
    let emulator = emulator();
    let case = "a patch carrying none of the product's members";

    let product_id = create_product(
        &emulator,
        case,
        &json!({
            "displayName": "Untouched",
            "description": "nothing in the patch applies to this",
            "tags": { "team": "platform" }
        }),
    )
    .await;
    let uri = format!("{COLLECTION}/{product_id}");
    let before = get_product(&emulator, case, &product_id).await;

    // Past a second boundary, so a `LastModified` that moved would be visible at the
    // whole-second granularity the member is reported at.
    sleep(SECOND_BOUNDARY_WAIT);

    let (status, patched) = call(&emulator, "PATCH", &uri, Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdatePortalProduct, got {status} {patched}"
    );
    assert_eq!(
        strip_nulls(&patched),
        strip_nulls(&before),
        "case {case:?}: expected the record to be reported unchanged, got {patched} against \
         {before}"
    );

    let after = get_product(&emulator, case, &product_id).await;
    assert_eq!(
        strip_nulls(&after),
        strip_nulls(&before),
        "case {case:?}: expected the stored record to be unchanged, got {after} against \
         {before}"
    );
}

#[tokio::test]
async fn a_patch_that_carries_a_member_advances_last_modified() {
    let emulator = emulator();

    // One row per patchable member, so none of them is the only one that records a
    // modification.
    let patches = [
        ("the display name", json!({ "displayName": "moved" })),
        ("the description", json!({ "description": "moved" })),
        (
            "the display order",
            json!({ "displayOrder": display_order("Moved", 1) }),
        ),
    ];

    for (case, patch) in patches {
        let product_id = create_product(
            &emulator,
            case,
            &json!({ "displayName": "Clock", "description": "created" }),
        )
        .await;
        let uri = format!("{COLLECTION}/{product_id}");
        let created_at = instant(case, &get_product(&emulator, case, &product_id).await, "lastModified");

        sleep(SECOND_BOUNDARY_WAIT);

        let (status, patched) = call(&emulator, "PATCH", &uri, Some(patch)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdatePortalProduct, got {status} {patched}"
        );
        let patched_at = instant(case, &patched, "lastModified");
        assert!(
            patched_at > created_at,
            "case {case:?}: expected LastModified to advance past the creation instant \
             {created_at}, got {patched_at}"
        );

        let fetched = get_product(&emulator, case, &product_id).await;
        assert_eq!(
            instant(case, &fetched, "lastModified"),
            patched_at,
            "case {case:?}: expected GetPortalProduct to report the patched LastModified, got \
             {fetched}"
        );
    }
}

#[tokio::test]
async fn a_product_is_patched_only_through_its_own_minted_id() {
    let emulator = emulator();
    let case = "a patch addressed at an id nothing minted";

    let product_id = create_product(
        &emulator,
        case,
        &json!({ "displayName": "Addressed", "description": "the only product" }),
    )
    .await;
    let before = get_product(&emulator, case, &product_id).await;

    for (label, id) in [
        ("an id nothing minted", "nosuchprod0".to_string()),
        ("the id's uppercase form", product_id.to_uppercase()),
        (
            "the id with its first character dropped",
            product_id[1..].to_string(),
        ),
        ("a long id nothing minted", "z".repeat(30)),
    ] {
        let (status, body) = call(
            &emulator,
            "PATCH",
            &format!("{COLLECTION}/{id}"),
            Some(json!({ "displayName": "should not land" })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 from UpdatePortalProduct, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {body}"
        );
    }

    // None of those refusals wrote anything: the product that does exist is unchanged.
    let after = get_product(&emulator, case, &product_id).await;
    assert_eq!(
        strip_nulls(&after),
        strip_nulls(&before),
        "case {case:?}: expected the existing product to be untouched by the refused \
         patches, got {after} against {before}"
    );
}

#[tokio::test]
async fn a_patch_whose_values_are_invalid_is_refused_and_persists_nothing() {
    let emulator = emulator();
    let case = "an invalid patch";

    let product_id = create_product(
        &emulator,
        case,
        &json!({ "displayName": "Valid", "description": "a valid record" }),
    )
    .await;
    let uri = format!("{COLLECTION}/{product_id}");
    let before = get_product(&emulator, case, &product_id).await;

    // `DisplayName` is `len 1..255` and `Description` is `len 0..1024`, so a value past
    // either bound is refused at the boundary; a display name of nothing but whitespace
    // satisfies the length bound and is refused by the handler, because the display name
    // is the product's only human-readable name in a published portal. Either refusal is
    // a 400 naming the member.
    let rows: Vec<(&str, Value)> = vec![
        ("an empty display name", json!({ "displayName": "" })),
        (
            "a display name one character past the maximum",
            json!({ "displayName": "n".repeat(256) }),
        ),
        (
            "a display name of one space",
            json!({ "displayName": " " }),
        ),
        (
            "a display name of tabs and newlines",
            json!({ "displayName": "\t\n  " }),
        ),
        (
            "a blank display name beside a valid description",
            json!({ "displayName": "   ", "description": "would otherwise land" }),
        ),
        (
            "a description one character past the maximum",
            json!({ "displayName": "Fine", "description": "d".repeat(1025) }),
        ),
    ];

    for (label, patch) in rows {
        let (status, body) = call(&emulator, "PATCH", &uri, Some(patch)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from UpdatePortalProduct, got {status} {body}"
        );
        let code = error_code(&body);
        assert!(
            code == "BadRequestException" || code == "ValidationException",
            "case {label:?}: expected the patch to be refused as invalid, got {code:?} {body}"
        );

        let after = get_product(&emulator, label, &product_id).await;
        assert_eq!(
            strip_nulls(&after),
            strip_nulls(&before),
            "case {label:?}: expected a refused patch to persist nothing, got {after} against \
             {before}"
        );
    }
}
