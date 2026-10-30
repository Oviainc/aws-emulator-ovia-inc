// SPDX-License-Identifier: CC0-1.0

//! The wire form of this service's developer-portal, product and VPC-link
//! `@timestampFormat("date-time")` members.
//!
//! RULE: a structure member whose target carries `@timestampFormat("date-time")` travels
//! on the restJson1 wire as an RFC 3339 / ISO-8601 date-time STRING — never as the epoch
//! NUMBER the protocol's default timestamp encoding would produce — and the string names
//! exactly the instant the service holds for it, sub-second digits included.
//!
//! Every timestamp member of this service targets `__timestampIso8601`
//! (`smithy_query(query="shape", shape="__timestampIso8601")` reports
//! `"timestampFormat": "date-time"`), so the rule reaches every shape that reports one.
//! This suite covers the portal, portal-product, product-page, product-REST-endpoint-page
//! and VPC-link shapes — `lastModified`, `lastPublished`, `logoLastUploaded` and
//! `createdDate`; the `api`, `stage`, `deployment` and domain-name shapes carry the same
//! members and are owned by their own issues.
//!
//! The rule is checked twice over, at both ends of the mechanism that carries it:
//!
//! * on the WIRE, over a table spanning the five resource families and every path that
//!   reports an instant for them — create, read, update and list — including the round
//!   trip back to the instant the create reported (so a well-formed string naming the
//!   wrong moment fails too);
//! * on the SCHEMA, because the member schema's `timestamp_format()` is the single thing
//!   the JSON codec consults (`vendor/smithy-schema-server/src/json.rs::write_timestamp`),
//!   read against the shipped Smithy model itself rather than against a copied list — so
//!   every member of every hand-written schema is a row, the non-timestamp members acting
//!   as the control rows that must report NO format.
//!
//! The harness (router, request signing, response parsing) is the one `api_lifecycle.rs`
//! already builds; this suite drives it rather than copying it.

use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::ops::types::{
    CreatePortalOutput, CreatePortalProductOutput, CreateProductPageOutput,
    CreateProductRestEndpointPageOutput, CreateVpcLinkOutput, GetPortalOutput,
    GetPortalProductOutput, GetProductPageOutput, GetProductRestEndpointPageOutput,
    GetVpcLinkOutput, PortalProductSummary, PortalSummary, PortalTheme,
    ProductPageSummaryNoBody, ProductRestEndpointPageSummaryNoBody, UpdatePortalOutput,
    UpdatePortalProductOutput, UpdateProductPageOutput, UpdateProductRestEndpointPageOutput,
    UpdateVpcLinkOutput, VpcLink,
};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{to_timestamp, PORTAL_BUILD};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator};
use crate::portal_lifecycle::portal_content;

/// This service's Smithy namespace, the prefix of every shape id below.
const NAMESPACE: &str = "com.amazonaws.apigatewayv2";

/// How far a reported instant may sit from this test's clock reading, in seconds. The
/// point of the assertion is that the member reads the write instant rather than zero or a
/// fixed literal, so the slack is generous.
const INSTANT_SLACK_SECONDS: f64 = 60.0;

/// One document a path answered, and the member paths of it that must carry a date-time.
struct Reported {
    case: String,
    document: Value,
    /// Dotted member paths — `lastModified`, or `portalContent.theme.logoLastUploaded` for
    /// a member of a nested structure.
    members: Vec<&'static str>,
}

/// The value at a dotted member path, or `None` when any step of it is absent.
fn at_path<'doc>(document: &'doc Value, path: &str) -> Option<&'doc Value> {
    let mut cursor = document;
    for step in path.split('.') {
        cursor = cursor.get(step)?;
    }
    Some(cursor)
}

/// Assert `path` of `document` is the RFC 3339 date-time string the rule requires, and
/// answer the instant it names in epoch milliseconds.
///
/// The failure messages name the two ways the rule is broken: a non-string (which is what
/// the protocol default's epoch NUMBER is) and a string no date-time parser accepts.
fn date_time_millis(case: &str, document: &Value, path: &str) -> i64 {
    let value = at_path(document, path)
        .unwrap_or_else(|| panic!("case {case:?}: the response must report {path}, got {document}"));
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {path} targets a shape marked @timestampFormat(\"date-time\"), so it \
             must be an RFC 3339 string and never the protocol-default epoch number, got {value}"
        )
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {path} = {text:?} must parse as an RFC 3339 date-time: {error}")
    });
    // Smithy's `date-time` is RFC 3339 with a `Z` offset, so the two designators are part
    // of the form a generated SDK's parser is handed.
    assert!(
        text.contains('T') && text.ends_with('Z'),
        "case {case:?}: {path} must carry the T separator and the Z offset, got {text:?}"
    );
    // The string is the canonical rendering of the instant it names: re-rendering the
    // parsed value reproduces it, so no digit of the stored instant was dropped on the way
    // out.
    let rerendered = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {path} = {text:?} is unrenderable: {error}"));
    assert_eq!(
        rerendered, text,
        "case {case:?}: {path} must be the canonical RFC 3339 rendering of the instant it names"
    );
    parsed
        .to_millis()
        .unwrap_or_else(|error| panic!("case {case:?}: {path} = {text:?} is unrepresentable: {error}"))
}

/// This test's own clock reading, in epoch seconds.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The `items` collection of a list response.
fn items(case: &str, body: &Value) -> Vec<Value> {
    body.get("items")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: the list must report items, got {body}"))
        .clone()
}

/// The element of `items` whose `id_member` is `id`.
fn listed(case: &str, body: &Value, id_member: &str, id: &str) -> Value {
    items(case, body)
        .into_iter()
        .find(|item| item.get(id_member).and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("case {case:?}: the list must report {id}, got {body}"))
}

/// Long enough for a portal build the service owes the clock to have come due.
fn past_the_build() -> Duration {
    PORTAL_BUILD + Duration::from_millis(400)
}

/// Drive every path of the five resource families that reports an instant, and answer what
/// each one reported.
async fn reported_documents(emulator: &MultiServiceRouter) -> Vec<Reported> {
    let mut rows: Vec<Reported> = Vec::new();

    // ---- The portal product: `lastModified` on create, read, update and list.
    let (status, product) = call(
        emulator,
        "POST",
        "/v2/portalproducts",
        Some(json!({ "displayName": "Timestamped product" })),
    )
    .await;
    assert_eq!(status, 201, "CreatePortalProduct must answer 201, got {status} {product}");
    let product_id = as_str(&product, "portalProductId");
    let product_uri = format!("/v2/portalproducts/{product_id}");

    let (_, fetched_product) = call(emulator, "GET", &product_uri, None).await;
    let (_, updated_product) = call(
        emulator,
        "PATCH",
        &product_uri,
        Some(json!({ "description": "an update moves the instant" })),
    )
    .await;
    let (_, listed_products) = call(emulator, "GET", "/v2/portalproducts", None).await;

    for (case, document) in [
        ("CreatePortalProduct", product.clone()),
        ("GetPortalProduct", fetched_product.clone()),
        ("UpdatePortalProduct", updated_product),
        (
            "ListPortalProducts item",
            listed(
                "ListPortalProducts item",
                &listed_products,
                "portalProductId",
                &product_id,
            ),
        ),
    ] {
        rows.push(Reported {
            case: case.to_string(),
            document,
            members: vec!["lastModified"],
        });
    }

    // ---- The product page: `lastModified` on create, read, update and list.
    let pages_uri = format!("{product_uri}/productpages");
    let (status, page) = call(
        emulator,
        "POST",
        &pages_uri,
        Some(json!({ "displayContent": { "title": "Getting started", "body": "Call it" } })),
    )
    .await;
    assert_eq!(status, 201, "CreateProductPage must answer 201, got {status} {page}");
    let page_id = as_str(&page, "productPageId");
    let page_uri = format!("{pages_uri}/{page_id}");

    let (_, fetched_page) = call(emulator, "GET", &page_uri, None).await;
    let (_, updated_page) = call(
        emulator,
        "PATCH",
        &page_uri,
        Some(json!({ "displayContent": { "title": "Getting started", "body": "Call it twice" } })),
    )
    .await;
    // `ListProductPages` is still a stub (501), so its element shape
    // (`ProductPageSummaryNoBody`) is covered by the schema test alone until that
    // operation's own issue implements it.
    for (case, document) in [
        ("CreateProductPage", page.clone()),
        ("GetProductPage", fetched_page.clone()),
        ("UpdateProductPage", updated_page),
    ] {
        rows.push(Reported {
            case: case.to_string(),
            document,
            members: vec!["lastModified"],
        });
    }

    // ---- The product REST endpoint page: the same member on its own four paths.
    let endpoint_pages_uri = format!("{product_uri}/productrestendpointpages");
    let (status, endpoint_page) = call(
        emulator,
        "POST",
        &endpoint_pages_uri,
        Some(json!({
            "restEndpointIdentifier": {
                "identifierParts": {
                    "restApiId": "a1b2c3d4e5",
                    "stage": "prod",
                    "method": "GET",
                    "path": "/orders"
                }
            }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateProductRestEndpointPage must answer 201, got {status} {endpoint_page}"
    );
    let endpoint_page_id = as_str(&endpoint_page, "productRestEndpointPageId");
    let endpoint_page_uri = format!("{endpoint_pages_uri}/{endpoint_page_id}");

    let (_, fetched_endpoint_page) = call(emulator, "GET", &endpoint_page_uri, None).await;
    let (_, updated_endpoint_page) = call(
        emulator,
        "PATCH",
        &endpoint_page_uri,
        Some(json!({ "tryItState": "ENABLED" })),
    )
    .await;
    // `ListProductRestEndpointPages` is still a stub (501), as `ListProductPages` is.
    for (case, document) in [
        ("CreateProductRestEndpointPage", endpoint_page.clone()),
        ("GetProductRestEndpointPage", fetched_endpoint_page.clone()),
        ("UpdateProductRestEndpointPage", updated_endpoint_page),
    ] {
        rows.push(Reported {
            case: case.to_string(),
            document,
            members: vec!["lastModified"],
        });
    }

    // ---- The portal: `lastModified` and the nested `logoLastUploaded` on create, read,
    // update and list, plus `lastPublished` once a publish has settled.
    let (status, portal) = call(
        emulator,
        "POST",
        "/v2/portals",
        Some(json!({
            "authorization": { "none": {} },
            "endpointConfiguration": { "none": {} },
            "portalContent": portal_content("Timestamped portal"),
            "logoUri": "s3://example-portal-assets/logo.png"
        })),
    )
    .await;
    assert_eq!(status, 201, "CreatePortal must answer 201, got {status} {portal}");
    let portal_id = as_str(&portal, "portalId");
    let portal_uri = format!("/v2/portals/{portal_id}");

    let (_, fetched_portal) = call(emulator, "GET", &portal_uri, None).await;
    let (_, updated_portal) = call(
        emulator,
        "PATCH",
        &portal_uri,
        Some(json!({ "rumAppMonitorName": "timestamped-monitor" })),
    )
    .await;
    let (_, listed_portals) = call(emulator, "GET", "/v2/portals", None).await;

    let portal_members = vec!["lastModified", "portalContent.theme.logoLastUploaded"];
    for (case, document) in [
        ("CreatePortal", portal.clone()),
        ("GetPortal", fetched_portal.clone()),
        ("UpdatePortal", updated_portal),
        (
            "ListPortals item",
            listed("ListPortals item", &listed_portals, "portalId", &portal_id),
        ),
    ] {
        rows.push(Reported {
            case: case.to_string(),
            document,
            members: portal_members.clone(),
        });
    }

    // A publish is server-managed: the portal reports the publication once the build's
    // span has passed, so the instant is read after it comes due.
    let (status, accepted) = call(
        emulator,
        "POST",
        &format!("{portal_uri}/publish"),
        Some(json!({ "description": "the first publication" })),
    )
    .await;
    assert_eq!(status, 202, "PublishPortal must answer 202, got {status} {accepted}");
    sleep(past_the_build());
    let (_, published) = call(emulator, "GET", &portal_uri, None).await;
    rows.push(Reported {
        case: "GetPortal once the publish has settled".to_string(),
        document: published,
        members: vec![
            "lastModified",
            "lastPublished",
            "portalContent.theme.logoLastUploaded",
        ],
    });

    // ---- The VPC link: `createdDate` on create, read, update and list.
    let (status, link) = call(
        emulator,
        "POST",
        "/v2/vpclinks",
        Some(json!({ "name": "timestamped", "subnetIds": ["subnet-0a1b2c3d"] })),
    )
    .await;
    assert_eq!(status, 201, "CreateVpcLink must answer 201, got {status} {link}");
    let link_id = as_str(&link, "vpcLinkId");
    let link_uri = format!("/v2/vpclinks/{link_id}");

    let (_, fetched_link) = call(emulator, "GET", &link_uri, None).await;
    let (_, updated_link) = call(
        emulator,
        "PATCH",
        &link_uri,
        Some(json!({ "name": "timestamped-renamed" })),
    )
    .await;
    // `GetVpcLinks` is still a stub (501), so the `VpcLink` element shape is covered by
    // the schema test alone until that operation's own issue implements it.
    for (case, document) in [
        ("CreateVpcLink", link.clone()),
        ("GetVpcLink", fetched_link.clone()),
        ("UpdateVpcLink", updated_link),
    ] {
        rows.push(Reported {
            case: case.to_string(),
            document,
            members: vec!["createdDate"],
        });
    }

    // The created instant a resource reports is the SAME on the create and on the read
    // that follows it: a date-time string naming another moment fails here.
    for (case, created, fetched, member) in [
        ("portal product", &product, &fetched_product, "lastModified"),
        ("product page", &page, &fetched_page, "lastModified"),
        (
            "product REST endpoint page",
            &endpoint_page,
            &fetched_endpoint_page,
            "lastModified",
        ),
        ("portal", &portal, &fetched_portal, "lastModified"),
        ("VPC link", &link, &fetched_link, "createdDate"),
    ] {
        let on_create = date_time_millis(case, created, member);
        let on_read = date_time_millis(case, fetched, member);
        assert_eq!(
            on_create, on_read,
            "case {case:?}: the read must report the instant the create did for {member}, \
             got {on_create} ms then {on_read} ms"
        );
    }

    rows
}

/// RULE on the wire: every reported instant of the five families is an RFC 3339 date-time
/// string naming the moment the service wrote.
#[tokio::test]
async fn every_date_time_member_is_an_rfc_3339_string_naming_the_stored_instant() {
    let emulator = emulator();
    let rows = reported_documents(&emulator).await;

    assert!(
        rows.len() >= 18,
        "the table must span every path that reports an instant, got {} rows",
        rows.len()
    );

    for row in &rows {
        for member in &row.members {
            let reported = date_time_millis(&row.case, &row.document, member) as f64 / 1_000.0;
            let now = now_seconds();
            assert!(
                (now - reported).abs() <= INSTANT_SLACK_SECONDS,
                "case {:?}: {member} must name the instant the service wrote; got {reported} \
                 against a clock reading {now}",
                row.case
            );
        }
    }
}

/// RULE on the precision the wire form states: the projection every reader renders its
/// instants through keeps the stored instant's sub-second digits, so the RFC 3339 string
/// states a fraction exactly when the instant has one.
///
/// The rows span the fraction's resolution — none, milli, micro — its extremes and a
/// fraction whose trailing digits are zero, at one fixed second so the expectation is the
/// stored value rather than the clock. They are microsecond-granular because the shared
/// `aws_smithy_types` date-time writer states the fraction to microsecond resolution.
#[test]
fn the_projection_keeps_the_stored_instant_sub_second_digits() {
    // The second is arbitrary and fixed; what each row varies is the fraction.
    const SECONDS: u64 = 1_767_225_600;

    for (case, nanos) in [
        ("no fraction at all", 0_u32),
        ("a millisecond fraction", 684_000_000),
        ("a microsecond fraction", 684_875_000),
        ("a fraction whose trailing digits are zero", 500_000_000),
        ("the smallest fraction the writer states", 1_000),
        ("the largest fraction the writer states", 999_999_000),
    ] {
        let stored = UNIX_EPOCH + Duration::new(SECONDS, nanos);
        let rendered = to_timestamp(stored)
            .fmt(Format::DateTime)
            .unwrap_or_else(|error| panic!("case {case:?}: the instant must render: {error}"));
        assert_eq!(
            rendered.contains('.'),
            nanos != 0,
            "case {case:?}: the date-time string must state a fraction exactly when the \
             stored instant has one, got {rendered:?}"
        );
        let parsed = DateTime::from_str(&rendered, Format::DateTime)
            .unwrap_or_else(|error| panic!("case {case:?}: {rendered:?} must parse: {error}"));
        assert_eq!(
            (parsed.secs(), parsed.subsec_nanos()),
            (SECONDS as i64, nanos),
            "case {case:?}: the rendered string must name the stored instant to the \
             microsecond, got {rendered:?}"
        );
    }
}

/// The member schemas of one shape, keyed by the shape's local name in the model.
fn shapes_under_test() -> Vec<(&'static str, &'static Schema)> {
    vec![
        ("CreatePortalProductResponse", &CreatePortalProductOutput::SCHEMA),
        ("GetPortalProductResponse", &GetPortalProductOutput::SCHEMA),
        ("UpdatePortalProductResponse", &UpdatePortalProductOutput::SCHEMA),
        ("PortalProductSummary", &PortalProductSummary::SCHEMA),
        ("CreateProductPageResponse", &CreateProductPageOutput::SCHEMA),
        ("GetProductPageResponse", &GetProductPageOutput::SCHEMA),
        ("UpdateProductPageResponse", &UpdateProductPageOutput::SCHEMA),
        ("ProductPageSummaryNoBody", &ProductPageSummaryNoBody::SCHEMA),
        (
            "CreateProductRestEndpointPageResponse",
            &CreateProductRestEndpointPageOutput::SCHEMA,
        ),
        (
            "GetProductRestEndpointPageResponse",
            &GetProductRestEndpointPageOutput::SCHEMA,
        ),
        (
            "UpdateProductRestEndpointPageResponse",
            &UpdateProductRestEndpointPageOutput::SCHEMA,
        ),
        (
            "ProductRestEndpointPageSummaryNoBody",
            &ProductRestEndpointPageSummaryNoBody::SCHEMA,
        ),
        ("CreatePortalResponse", &CreatePortalOutput::SCHEMA),
        ("GetPortalResponse", &GetPortalOutput::SCHEMA),
        ("UpdatePortalResponse", &UpdatePortalOutput::SCHEMA),
        ("PortalSummary", &PortalSummary::SCHEMA),
        ("PortalTheme", &PortalTheme::SCHEMA),
        ("CreateVpcLinkResponse", &CreateVpcLinkOutput::SCHEMA),
        ("GetVpcLinkResponse", &GetVpcLinkOutput::SCHEMA),
        ("UpdateVpcLinkResponse", &UpdateVpcLinkOutput::SCHEMA),
        ("VpcLink", &VpcLink::SCHEMA),
    ]
}

/// The service's own Smithy model, as shipped in this crate.
fn model() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json"
    );
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("parse {path}: {error}"))
}

/// Whether the model marks `member` of a structure as a `date-time` timestamp: the trait
/// on the member itself, or — the form this model uses — on the shape the member targets.
fn is_date_time(model: &Value, member: &Value) -> bool {
    let on_member = member
        .get("traits")
        .and_then(|traits| traits.get("smithy.api#timestampFormat"))
        .and_then(Value::as_str);
    let target = member.get("target").and_then(Value::as_str).unwrap_or_default();
    let on_target = model
        .get("shapes")
        .and_then(|shapes| shapes.get(target))
        .and_then(|shape| shape.get("traits"))
        .and_then(|traits| traits.get("smithy.api#timestampFormat"))
        .and_then(Value::as_str);
    on_member.or(on_target) == Some("date-time")
}

/// RULE on the schema, read against the model rather than against a copied list: for every
/// shape whose schema this crate writes by hand, each member the model declares is present
/// exactly once, under the model's own wire name, and carries the `date-time` format when —
/// and only when — the model marks it so.
///
/// This is what the JSON codec consults, so it is the mechanism the wire test above
/// observes; the non-timestamp members are the control rows.
#[test]
fn every_hand_written_schema_matches_the_model_member_for_member() {
    let model = model();
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the model must declare shapes");

    let mut date_time_members = 0_usize;
    let mut other_members = 0_usize;

    for (local, schema) in shapes_under_test() {
        let shape_id = format!("{NAMESPACE}#{local}");
        let modelled = shapes
            .get(&shape_id)
            .and_then(|shape| shape.get("members"))
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("shape {shape_id} must declare members"));

        let on_schema = schema.members();
        assert_eq!(
            on_schema.len(),
            modelled.len(),
            "shape {local:?}: the schema must name every modelled member, got {:?} against \
             {:?}",
            on_schema
                .iter()
                .map(|member| member.member_name())
                .collect::<Vec<_>>(),
            modelled.keys().collect::<Vec<_>>()
        );

        for (index, member_schema) in on_schema.iter().enumerate() {
            let name = member_schema
                .member_name()
                .unwrap_or_else(|| panic!("shape {local:?}: every member schema must be named"));
            let modelled_member = modelled.get(name).unwrap_or_else(|| {
                panic!("shape {local:?}: {name:?} is no member of {shape_id} in the model")
            });

            assert_eq!(
                member_schema.member_index(),
                Some(index),
                "shape {local:?}: member {name:?} must report the index it sits at, got {:?}",
                member_schema.member_index()
            );

            // The wire key: `@jsonName` where the model gives one, else the member name.
            let expected_wire_name = modelled_member
                .get("traits")
                .and_then(|traits| traits.get("smithy.api#jsonName"))
                .and_then(Value::as_str)
                .unwrap_or(name);
            let wire_name = member_schema
                .json_name()
                .map_or(name, |json_name| json_name.value());
            assert_eq!(
                wire_name, expected_wire_name,
                "shape {local:?}: member {name:?} must travel under the model's wire name"
            );

            let format = member_schema.timestamp_format().map(|carried| carried.format());
            if is_date_time(&model, modelled_member) {
                date_time_members += 1;
                assert!(
                    matches!(format, Some(TimestampFormat::DateTime)),
                    "shape {local:?}: the model marks member {name:?} \
                     @timestampFormat(\"date-time\"), so its schema must carry that format, \
                     got {format:?}"
                );
            } else {
                other_members += 1;
                assert!(
                    format.is_none(),
                    "shape {local:?}: the model gives member {name:?} no @timestampFormat, \
                     got {format:?}"
                );
            }
        }
    }

    // Both kinds were actually exercised, so neither branch of the rule passed by being
    // unreachable.
    assert!(
        date_time_members >= 24 && other_members >= 100,
        "the table must span both kinds of member, got {date_time_members} date-time and \
         {other_members} other members"
    );
}
