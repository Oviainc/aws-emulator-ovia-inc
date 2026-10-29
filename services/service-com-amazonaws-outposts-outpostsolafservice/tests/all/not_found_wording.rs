// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for HOW this service words a `NotFoundException`, driven over the
//! wire through `MultiServiceRouter::dispatch` exactly as a signed `aws outposts` call
//! arrives.
//!
//! The rule under test, stated once and then exercised over a SET of inputs: the
//! message of a not-found refusal is fixed by the KIND of resource the request named
//! and could not be resolved — and by the path that resolves it — not by the name of
//! the input member the identifier arrived in. Each kind has ONE text, whose only
//! variable part is the identifier taken from the request:
//!
//! * an order: `No such order for order id <orderId>`;
//! * a quote: `Could not find Quote with identifier <quoteIdentifier>`, naming the
//!   identifier as supplied, so the ARN form reads its ARN back;
//! * an installed asset: `No such asset id <assetId> found.`;
//! * the parent Outpost of a CAPACITY route: `No Outpost with outpostId=<outpostId>`,
//!   naming the Outpost's own bare id whichever form the request supplied;
//! * the Outpost an ORDER is placed on, or the one a DELETE names: `outpostId=<the
//!   Outpost ARN the identifier resolves to>` — the ARN supplied, or the one this
//!   endpoint mints for a bare id.
//!
//! So every route that fails to resolve an identifier of one kind answers the same
//! text as every other route that fails to resolve one of that kind, and none of them
//! answers the `<member>=<value>` echo the site and Outpost registry routes word their
//! own refusals by (those are [`crate::error_message_rules`]'s).
//!
//! The rows differ along what the rule generalises over: which operation refuses,
//! which member the identifier arrived in (URI label, query parameter, body member),
//! which of the two identifier forms the modelled patterns accept it in (bare id,
//! full ARN — local and of another partition), and why it resolves to nothing (never
//! minted, deleted, or a resource of another Outpost). Every row's status and wire
//! error code are asserted beside its message, and each test ends on a control: while
//! the resource DOES exist the very same identifier is answered 200, so the wording is
//! read off the store rather than returned for the shape of the request.
//!
//! The member names and identifier patterns the rows use are the modelled ones, from
//! `smithy_query(query="operation",operation="GetOrder,GetQuote,GetCapacityTask,
//! CreateOrder,DeleteOutpost,StartConnection")`. The texts themselves are AWS's own
//! wording for this error class.

use serde_json::{json, Value};

use crate::capacity_task_lifecycle::{
    an_order, capacity_route, install, outpost, pool, task_route, ABSENT_CAPACITY_TASK_ID,
};
use crate::quote_lifecycle::{create_quote, request as quote_request, QUOTE_ARN_PREFIX};
use crate::site_and_outpost_lifecycle::{
    segment, text, Outposts, ABSENT_OUTPOST_ID, OUTPOST_ARN_PREFIX,
};

/// Order ids of the modelled `OrderId` form (`^oo-[a-f0-9]{17}$`) this emulator never
/// mints: well formed, so the boundary passes them through, and naming no order.
const ABSENT_ORDER_IDS: [&str; 2] = ["oo-00000000000000000", "oo-1a2b3c4d5e6f7a8b9"];

/// Quote ids of the modelled `QuoteIdentifier` bare-id form (`oq-[a-f0-9]{17}`) that
/// name no quote.
const ABSENT_QUOTE_IDS: [&str; 2] = ["oq-00000000000000000", "oq-0f1e2d3c4b5a69788"];

/// `AssetId` values of the modelled `StartConnection` form (`^(\w+)$`, len 1..100)
/// that no Outpost of a store holds: the ten-digit form the hardware of an Outpost
/// carries, and a word form the pattern equally admits.
const ABSENT_ASSET_IDS: [&str; 2] = ["1234567890", "asset_nothing_holds"];

/// The same, narrowed to the `AssetIdInput` form (`^\d{10}$`) the capacity and
/// instance-type routes model their `AssetId` as.
const ABSENT_ASSET_ID_DIGITS: [&str; 2] = ["1234567890", "9999999999"];

/// The hardware recorded on the Outpost a row needs an asset of.
const INSTALLED_ASSET: &str = "3000000001";

/// An Outpost ARN of ANOTHER partition, Region and account: a value the modelled
/// `OutpostId` / `OutpostIdentifier` pattern accepts, naming an Outpost of an endpoint
/// this one is not.
const FOREIGN_OUTPOST_ARN: &str =
    "arn:aws-cn:outposts:cn-north-1:123456789012:outpost/op-2222222222222222a";

/// AWS's text for an order no order inventory holds.
fn no_such_order(order_id: &str) -> String {
    format!("No such order for order id {order_id}")
}

/// AWS's text for a quote no quote inventory holds.
fn no_such_quote(identifier: &str) -> String {
    format!("Could not find Quote with identifier {identifier}")
}

/// AWS's text for an `AssetId` no installed hardware carries.
fn no_such_asset(asset_id: &str) -> String {
    format!("No such asset id {asset_id} found.")
}

/// AWS's text for the parent Outpost of a capacity route.
fn no_such_outpost(outpost_id: &str) -> String {
    format!("No Outpost with outpostId={outpost_id}")
}

/// AWS's text for the Outpost an order is placed on, and for the one a delete names:
/// the ARN the supplied identifier resolves to.
fn no_such_outpost_arn(arn: &str) -> String {
    format!("outpostId={arn}")
}

/// One row: the call, and the whole `$.Message` the rule requires for it.
struct Case {
    case: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    expected: String,
}

/// A row stated inline.
fn case(
    case: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    expected: String,
) -> Case {
    Case {
        case,
        method,
        path,
        body,
        expected,
    }
}

/// Drive every row and assert the status, the wire error code and the exact message.
async fn expect_not_found(outposts: &Outposts, cases: &[Case]) {
    for row in cases {
        let reply = outposts.call(row.method, &row.path, row.body.as_ref()).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {:?}: expected 404 NotFoundException, got {} {} {}",
            row.case,
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = text(&reply.body, "Message");
        assert_eq!(
            actual, row.expected,
            "case {:?}: expected $.Message {:?}, got {actual:?}",
            row.case, row.expected
        );
    }
}

/// The capacity routes nested under an Outpost that resolve the parent first: the two
/// reads, the cancel and the start, each as `(name, method, path, body)` for one
/// Outpost identifier.
fn capacity_routes(outpost_identifier: &str, capacity_task_id: &str) -> Vec<Case> {
    let task = task_route(outpost_identifier, capacity_task_id);
    let expected = no_such_outpost(bare_outpost_id(outpost_identifier));
    vec![
        case(
            format!("GetCapacityTask under {outpost_identifier}"),
            "GET",
            task.clone(),
            None,
            expected.clone(),
        ),
        case(
            format!("CancelCapacityTask under {outpost_identifier}"),
            "POST",
            task.clone(),
            None,
            expected.clone(),
        ),
        case(
            format!("ListBlockingInstancesForCapacityTask under {outpost_identifier}"),
            "GET",
            format!("{task}/blockingInstances"),
            None,
            expected.clone(),
        ),
        case(
            format!("StartCapacityTask on {outpost_identifier}"),
            "POST",
            capacity_route(outpost_identifier),
            Some(json!({ "InstancePools": [pool("m5.4xlarge", 1)] })),
            expected,
        ),
    ]
}

/// The bare `op-…` id an Outpost identifier carries, in either modelled form.
fn bare_outpost_id(identifier: &str) -> &str {
    identifier.rsplit_once('/').map_or(identifier, |(_, id)| id)
}

/// `GetOutpostSupportedInstanceTypes` scoped by one query parameter.
fn supported_instance_types(outpost_identifier: &str, query: &str) -> String {
    format!(
        "/outposts/{}/supportedInstanceTypes?{query}",
        segment(outpost_identifier)
    )
}

/// Every route that resolves an `OrderId` states the order wording — whether the id
/// arrives as a URI label, as a query parameter or as a body member, and whether it
/// names no order at all or an order of another Outpost.
#[tokio::test]
async fn every_route_that_resolves_an_order_states_the_order_wording() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "order-wording").await;
    let elsewhere = outpost(&outposts, "order-wording-elsewhere").await;
    let order_of_elsewhere = an_order(&outposts, &elsewhere).await;

    let mut cases: Vec<Case> = Vec::new();
    for absent in ABSENT_ORDER_IDS {
        cases.push(case(
            format!("GetOrder {absent}"),
            "GET",
            format!("/orders/{absent}"),
            None,
            no_such_order(absent),
        ));
        cases.push(case(
            format!("CancelOrder {absent}"),
            "POST",
            format!("/orders/{absent}/cancel"),
            None,
            no_such_order(absent),
        ));
        cases.push(case(
            format!("StartCapacityTask naming order {absent}"),
            "POST",
            capacity_route(&outpost_id),
            Some(json!({
                "InstancePools": [pool("m5.4xlarge", 1)],
                "OrderId": absent,
            })),
            no_such_order(absent),
        ));
        cases.push(case(
            format!("GetOutpostSupportedInstanceTypes scoped to order {absent}"),
            "GET",
            supported_instance_types(&outpost_id, &format!("OrderId={absent}")),
            None,
            no_such_order(absent),
        ));
    }
    // An order that EXISTS, named on a route it is not an order of: the same wording,
    // because the id names no order of the scope the call asked about.
    cases.push(case(
        format!("GetOutpostSupportedInstanceTypes scoped to another Outpost's order {order_of_elsewhere}"),
        "GET",
        supported_instance_types(&outpost_id, &format!("OrderId={order_of_elsewhere}")),
        None,
        no_such_order(&order_of_elsewhere),
    ));

    expect_not_found(&outposts, &cases).await;

    // The control: the order this store DOES hold is answered 200 by the same route
    // that refuses the ids above.
    let order_id = an_order(&outposts, &outpost_id).await;
    let read = outposts.ok("GET", &format!("/orders/{order_id}"), None).await;
    assert_eq!(
        text(&read["Order"], "OrderId"),
        order_id,
        "case \"GetOrder on the stored order\": expected the order back, got {read}"
    );
}

/// Every route that resolves a `QuoteIdentifier` states the quote wording, for both
/// identifier forms the modelled pattern accepts and whichever member it arrives in.
#[tokio::test]
async fn every_route_that_resolves_a_quote_states_the_quote_wording() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "quote-wording").await;

    let mut cases: Vec<Case> = Vec::new();
    for absent in ABSENT_QUOTE_IDS {
        for supplied in [absent.to_string(), format!("{QUOTE_ARN_PREFIX}{absent}")] {
            let quote_route = format!("/quotes/{}", segment(&supplied));
            cases.push(case(
                format!("GetQuote {supplied}"),
                "GET",
                quote_route.clone(),
                None,
                no_such_quote(&supplied),
            ));
            cases.push(case(
                format!("DeleteQuote {supplied}"),
                "DELETE",
                quote_route.clone(),
                None,
                no_such_quote(&supplied),
            ));
            cases.push(case(
                format!("UpdateQuote {supplied}"),
                "PATCH",
                quote_route,
                Some(json!({ "Description": "no such quote" })),
                no_such_quote(&supplied),
            ));
            cases.push(case(
                format!("CreateOrder from quote {supplied}"),
                "POST",
                "/orders".to_string(),
                Some(json!({
                    "OutpostIdentifier": outpost_id,
                    "QuoteIdentifier": supplied,
                    "PaymentOption": "ALL_UPFRONT",
                })),
                no_such_quote(&supplied),
            ));
        }
    }

    expect_not_found(&outposts, &cases).await;

    // The control: a quote this store holds reads back 200 through the route that
    // refuses the identifiers above — and once it is deleted, the refusal is the one
    // the rule states for it.
    let quote = create_quote(&outposts, &quote_request("US", json!([]), json!({}))).await;
    let quote_id = text(&quote, "QuoteId");
    let read = outposts
        .ok("GET", &format!("/quotes/{quote_id}"), None)
        .await;
    assert_eq!(
        text(&read["Quote"], "QuoteId"),
        quote_id,
        "case \"GetQuote on the stored quote\": expected the quote back, got {read}"
    );
    let _ = outposts
        .ok("DELETE", &format!("/quotes/{quote_id}"), None)
        .await;
    expect_not_found(
        &outposts,
        &[case(
            format!("GetQuote on the deleted quote {quote_id}"),
            "GET",
            format!("/quotes/{quote_id}"),
            None,
            no_such_quote(&quote_id),
        )],
    )
    .await;
}

/// Every route that resolves an `AssetId` states the asset wording, for both forms the
/// modelled pattern admits and whether the hardware is unknown everywhere or installed
/// on another Outpost.
#[tokio::test]
async fn every_route_that_resolves_an_asset_states_the_asset_wording() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "asset-wording").await;
    let elsewhere = outpost(&outposts, "asset-wording-elsewhere").await;
    install(&outposts, &outpost_id, &[INSTALLED_ASSET], "asset-wording");
    // A WireGuard public key of the modelled form (44 characters, base64 of 32 bytes).
    let client_public_key = format!("{}=", "A".repeat(43));

    let mut cases: Vec<Case> = Vec::new();
    for absent in ABSENT_ASSET_IDS {
        cases.push(case(
            format!("StartConnection for asset {absent}"),
            "POST",
            "/connections".to_string(),
            Some(json!({
                "AssetId": absent,
                "ClientPublicKey": client_public_key,
                "NetworkInterfaceDeviceIndex": 0,
            })),
            no_such_asset(absent),
        ));
    }
    // The capacity and instance-type routes model their `AssetId` as the ten-digit
    // `AssetIdInput`, so these rows name that form.
    for absent in ABSENT_ASSET_ID_DIGITS {
        cases.push(case(
            format!("StartCapacityTask naming asset {absent}"),
            "POST",
            capacity_route(&outpost_id),
            Some(json!({
                "InstancePools": [pool("m5.4xlarge", 1)],
                "AssetId": absent,
            })),
            no_such_asset(absent),
        ));
        cases.push(case(
            format!("GetOutpostSupportedInstanceTypes scoped to asset {absent}"),
            "GET",
            supported_instance_types(&outpost_id, &format!("AssetId={absent}")),
            None,
            no_such_asset(absent),
        ));
    }
    // Hardware that exists, named on an Outpost it is not installed on: the same
    // wording, because that Outpost's hardware carries no such id.
    cases.push(case(
        format!("StartCapacityTask on {elsewhere} naming {INSTALLED_ASSET}"),
        "POST",
        capacity_route(&elsewhere),
        Some(json!({
            "InstancePools": [pool("m5.4xlarge", 1)],
            "AssetId": INSTALLED_ASSET,
        })),
        no_such_asset(INSTALLED_ASSET),
    ));
    cases.push(case(
        format!("GetOutpostSupportedInstanceTypes on {elsewhere} scoped to {INSTALLED_ASSET}"),
        "GET",
        supported_instance_types(&elsewhere, &format!("AssetId={INSTALLED_ASSET}")),
        None,
        no_such_asset(INSTALLED_ASSET),
    ));

    expect_not_found(&outposts, &cases).await;

    // The control: the hardware the store DOES hold starts a connection, so the
    // refusals above are about what the store holds and not about the call.
    let started = outposts
        .ok(
            "POST",
            "/connections",
            Some(&json!({
                "AssetId": INSTALLED_ASSET,
                "ClientPublicKey": client_public_key,
                "NetworkInterfaceDeviceIndex": 0,
            })),
        )
        .await;
    assert!(
        !text(&started, "ConnectionId").is_empty(),
        "case \"StartConnection for the installed asset\": expected a connection, got {started}"
    );
}

/// Every CAPACITY route states the Outpost sentence for a parent Outpost it holds
/// none of, naming that Outpost's own bare id whichever identifier form the request
/// supplied — including an ARN of another endpoint, which names no Outpost here.
#[tokio::test]
async fn every_capacity_route_states_the_outpost_sentence() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "capacity-wording").await;
    let task_id = text(
        &outposts
            .ok(
                "POST",
                &capacity_route(&outpost_id),
                Some(&json!({ "InstancePools": [pool("m5.4xlarge", 1)] })),
            )
            .await,
        "CapacityTaskId",
    );

    let absent_local_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");
    let mut cases: Vec<Case> = Vec::new();
    for identifier in [
        ABSENT_OUTPOST_ID.to_string(),
        absent_local_arn,
        FOREIGN_OUTPOST_ARN.to_string(),
    ] {
        cases.extend(capacity_routes(&identifier, &task_id));
    }
    // A task id this store holds nothing for is a refusal about the TASK, not about
    // the Outpost, so it keeps the capacity-task identifier echo.
    cases.push(case(
        format!("GetCapacityTask for task {ABSENT_CAPACITY_TASK_ID}"),
        "GET",
        task_route(&outpost_id, ABSENT_CAPACITY_TASK_ID),
        None,
        format!("capacityTaskId={ABSENT_CAPACITY_TASK_ID}"),
    ));

    expect_not_found(&outposts, &cases).await;

    // The control: the Outpost this store holds answers the same route 200.
    let read = outposts
        .ok("GET", &task_route(&outpost_id, &task_id), None)
        .await;
    assert_eq!(
        text(&read, "CapacityTaskId"),
        task_id,
        "case \"GetCapacityTask on the stored task\": expected the task back, got {read}"
    );
}

/// The order and Outpost MANAGEMENT paths name the Outpost by the ARN the supplied
/// identifier resolves to: the ARN a request supplied, or the one this endpoint mints
/// for a bare id.
#[tokio::test]
async fn the_order_and_delete_paths_name_the_resolved_outpost_arn() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts.site_and_outpost("arn-wording").await;
    let absent_local_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");

    let mut cases: Vec<Case> = Vec::new();
    for (supplied, resolved) in [
        // A bare id names an Outpost of the endpoint the request reached, so the ARN
        // is this endpoint's own for that id.
        (
            ABSENT_OUTPOST_ID.to_string(),
            format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}"),
        ),
        // An ARN already states the identity it names, so it is the ARN reported —
        // whether it is this endpoint's own or another's.
        (absent_local_arn.clone(), absent_local_arn),
        (
            FOREIGN_OUTPOST_ARN.to_string(),
            FOREIGN_OUTPOST_ARN.to_string(),
        ),
    ] {
        cases.push(case(
            format!("DeleteOutpost {supplied}"),
            "DELETE",
            format!("/outposts/{}", segment(&supplied)),
            None,
            no_such_outpost_arn(&resolved),
        ));
        cases.push(case(
            format!("CreateOrder on {supplied}"),
            "POST",
            "/orders".to_string(),
            Some(json!({
                "OutpostIdentifier": supplied,
                "PaymentOption": "ALL_UPFRONT",
                "LineItems": [{ "CatalogItemId": "OR-1A2B3C4", "Quantity": 1 }],
            })),
            no_such_outpost_arn(&resolved),
        ));
    }

    expect_not_found(&outposts, &cases).await;

    // The control, and the state half of the rule: the Outpost this store holds takes
    // an order and is deleted 200, and the delete is what turns the identifier into
    // one these paths refuse — by the ARN it resolves to, for either form.
    let order = an_order(&outposts, &outpost_id).await;
    assert!(
        !order.is_empty(),
        "case \"CreateOrder on the stored Outpost\": expected an order, got an empty id"
    );
    // The order outstanding on the Outpost is what a delete conflicts with, so it is
    // cancelled first: this case is about the not-found wording, not about a conflict.
    let _ = outposts
        .ok("POST", &format!("/orders/{order}/cancel"), None)
        .await;
    let _ = outposts
        .ok("DELETE", &format!("/outposts/{outpost_id}"), None)
        .await;
    let resolved = format!("{OUTPOST_ARN_PREFIX}{outpost_id}");
    expect_not_found(
        &outposts,
        &[
            case(
                format!("DeleteOutpost again, by bare id {outpost_id}"),
                "DELETE",
                format!("/outposts/{outpost_id}"),
                None,
                no_such_outpost_arn(&resolved),
            ),
            case(
                format!("DeleteOutpost again, by ARN {resolved}"),
                "DELETE",
                format!("/outposts/{}", segment(&resolved)),
                None,
                no_such_outpost_arn(&resolved),
            ),
        ],
    )
    .await;
}
