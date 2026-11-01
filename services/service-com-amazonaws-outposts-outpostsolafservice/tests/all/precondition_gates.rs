// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the PRECONDITIONS of this API: the state a resource has to be
//! in before a call that acts on it is accepted at all, driven over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws outposts` call arrives — so what
//! it asserts is the emulator's HTTP behaviour, not a handler called directly. The
//! request harness is [`crate::site_and_outpost_lifecycle`]'s, reused rather than
//! rebuilt.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. An order is placed AT the site its Outpost stands on, against the details that
//!    site records — the address the hardware is operated at and the address it is
//!    shipped to, the two site-details checks this API's `OrderingRequirementType`
//!    enumerates (`OPERATING_ADDRESS_EXISTENCE_CHECK_ERROR`,
//!    `SHIPPING_ADDRESS_EXISTENCE_CHECK_ERROR`). A site missing either records
//!    incomplete details, so no order can be placed on an Outpost of it: the request is
//!    refused 400 `ValidationException` and nothing is minted, while the same request on
//!    a site recording both is served and the order reads back.
//! 2. An operation that acts on an Outpost's INSTALLED HARDWARE needs that hardware in
//!    service, which is the terminal `ACTIVE` lifecycle status: `StartCapacityTask`
//!    (which configures the capacity of that hardware) and `StartOutpostDecommission`
//!    (which takes it out of service) are refused 400 `ValidationException` on an
//!    Outpost in any other status, naming the Outpost and the status it holds, with
//!    nothing created and the Outpost left as it stood — and the caller asking only to
//!    plan the work (`DryRun`, `ValidateOnly`) does not get past the precondition,
//!    because planning the work is checking it.
//! 3. A read of a resource configured PER TYPE answers the type it was asked for or
//!    reports that nothing is configured under it: `GetSiteAddress` for an
//!    `AddressType` the site records no address under is 404 `NotFoundException` naming
//!    the type and the site, while the type it does record reads back — and recording
//!    the missing one makes the identical read answer that address.
//!
//! The rows differ along what each rule generalises over: which of a site's addresses
//! are recorded (neither, one, the other, both), how the order names its contents
//! (line items directly, a quote), which operation the precondition is asked of, the
//! lifecycle status the Outpost holds, whether the caller asked to plan only, each
//! modelled `AddressType`, and the identifier form the request names the resource by
//! (bare id, full ARN).

use serde_json::{json, Value};

use crate::site_and_outpost_lifecycle::{
    address, mark_outpost_active, orderable_site, query_value, segment, text, Outposts,
    OUTPOST_ARN_PREFIX,
};

/// Every value the modelled `AddressType` enum declares.
///
/// Source: `smithy_query(query="shape", shape="AddressType")` against this service's
/// model — `["SHIPPING_ADDRESS", "OPERATING_ADDRESS"]`.
const ADDRESS_TYPES: [&str; 2] = ["OPERATING_ADDRESS", "SHIPPING_ADDRESS"];

/// The `AddressType` naming the address a site is at.
const OPERATING: &str = "OPERATING_ADDRESS";
/// The `AddressType` naming the address a site's hardware ships to.
const SHIPPING: &str = "SHIPPING_ADDRESS";

/// The lifecycle status a registered Outpost holds until the hardware ordered for it
/// has been installed, and the terminal one it holds once that hardware is in service.
const PENDING: &str = "PENDING";
/// See [`PENDING`].
const ACTIVE: &str = "ACTIVE";

/// The sentence an order placed at a site whose details are not complete is refused
/// with: wholly fixed text, naming neither the site nor which detail is missing.
const SITE_DETAILS_INCOMPLETE: &str =
    "The site details are incomplete. Reach out to AWS Support for assistance.";

/// A `CatalogItemId` of the modelled `SkuCode` form (`^OR-[A-Z0-9]{7}$`) that this
/// service's published catalog still offers.
const CATALOG_ITEM: &str = "OR-1A2B3C4";

/// One site recording the addresses the row asks for, as its `SiteId`.
async fn a_site(outposts: &Outposts, name: &str, operating: bool, shipping: bool) -> String {
    let mut request = json!({ "Name": name });
    if let Some(members) = request.as_object_mut() {
        if operating {
            members.insert(
                "OperatingAddress".to_string(),
                address("US", "WA", "Seattle"),
            );
        }
        if shipping {
            members.insert(
                "ShippingAddress".to_string(),
                address("US", "WA", "Kirkland"),
            );
        }
    }
    text(&outposts.create_site(&request).await, "SiteId")
}

/// One Outpost on `site_id`, as its `(OutpostId, OutpostArn)` — the two identifier
/// forms the modelled `OutpostIdentifier` pattern accepts.
async fn an_outpost(outposts: &Outposts, site_id: &str, name: &str) -> (String, String) {
    let outpost = outposts
        .create_outpost(&json!({ "Name": name, "SiteId": site_id }))
        .await;
    (text(&outpost, "OutpostId"), text(&outpost, "OutpostArn"))
}

/// The smallest order this service accepts, naming the Outpost by `identifier`: the two
/// members `CreateOrder` marks `@required`, with one line item of a published
/// configuration.
fn order_body(identifier: &str) -> Value {
    json!({
        "OutpostIdentifier": identifier,
        "PaymentOption": "NO_UPFRONT",
        "LineItems": [{ "CatalogItemId": CATALOG_ITEM, "Quantity": 1 }],
    })
}

/// The `OrderId`s `ListOrders` reports for one Outpost — empty for an Outpost no order
/// was ever placed on.
async fn orders_on(outposts: &Outposts, identifier: &str) -> Vec<String> {
    let listed = outposts
        .ok(
            "GET",
            &format!(
                "/list-orders?OutpostIdentifierFilter={}",
                query_value(identifier)
            ),
            None,
        )
        .await;
    listed["Orders"]
        .as_array()
        .map(|orders| orders.iter().map(|order| text(order, "OrderId")).collect())
        .unwrap_or_default()
}

/// The `CapacityTaskId`s `ListCapacityTasks` reports for one Outpost.
async fn tasks_on(outposts: &Outposts, identifier: &str) -> Vec<String> {
    let listed = outposts
        .ok(
            "GET",
            &format!(
                "/capacity/tasks?OutpostIdentifierFilter={}",
                query_value(identifier)
            ),
            None,
        )
        .await;
    listed["CapacityTasks"]
        .as_array()
        .map(|tasks| tasks.iter().map(|task| text(task, "CapacityTaskId")).collect())
        .unwrap_or_default()
}

/// The `LifeCycleStatus` `GetOutpost` reports for one Outpost.
async fn life_cycle_status(outposts: &Outposts, outpost_id: &str) -> String {
    text(&outposts.get_outpost(outpost_id).await, "LifeCycleStatus")
}

/// The error message a reply carries, under the key the modelled error shapes spell it
/// with.
fn message(body: &Value) -> String {
    text(body, "Message")
}

/// Rule 1: an order is refused at a site whose details are not complete and served at
/// one whose details are, over a table of sites and of ways an order names what it is
/// for.
///
/// The rows differ along which of the two addresses the site records (neither, the
/// operating one only, the shipping one only, both), the identifier form the order
/// names its Outpost by, and whether the order carries line items of its own or is
/// taken from a quote — the two ways `CreateOrder` gets an order's contents.
#[tokio::test]
async fn an_order_is_placed_only_at_a_site_whose_details_are_complete() {
    // (case, operating address recorded, shipping address recorded, order accepted)
    let rows: Vec<(&str, bool, bool, bool)> = vec![
        ("a site created with neither address", false, false, false),
        ("a site with the operating address only", true, false, false),
        ("a site with the shipping address only", false, true, false),
        ("a site recording both addresses", true, true, true),
    ];

    for (index, (case, operating, shipping, accepted)) in rows.iter().enumerate() {
        // A store of its own per row: what an order is refused by is what the store
        // holds, so each row starts from an empty one.
        let outposts = Outposts::new();
        let site_id = a_site(
            &outposts,
            &format!("precondition-site-{index}"),
            *operating,
            *shipping,
        )
        .await;
        let (outpost_id, outpost_arn) =
            an_outpost(&outposts, &site_id, &format!("precondition-{index}")).await;

        for named_by in [outpost_id.as_str(), outpost_arn.as_str()] {
            let before = orders_on(&outposts, &outpost_id).await;
            let reply = outposts
                .call("POST", "/orders", Some(&order_body(named_by)))
                .await;
            if *accepted {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?} named by {named_by}: a complete site takes the order, \
                     got {} {:?}",
                    reply.status, reply.body
                );
                // The round trip: the order the create answered is the order
                // `GetOrder` reads back, and the one the Outpost's list reports.
                let order_id = text(&reply.body["Order"], "OrderId");
                let read = outposts
                    .ok("GET", &format!("/orders/{}", segment(&order_id)), None)
                    .await["Order"]
                    .clone();
                assert_eq!(
                    text(&read, "OrderId"),
                    order_id,
                    "case {case:?} named by {named_by}: GetOrder must read the order back, \
                     got {read:?}"
                );
                let listed = orders_on(&outposts, &outpost_id).await;
                assert!(
                    listed.contains(&order_id),
                    "case {case:?} named by {named_by}: the order must be listable on its \
                     Outpost, got {listed:?}"
                );
                continue;
            }
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?} named by {named_by}: an incomplete site takes no order, \
                 got {} {} {:?}",
                reply.status,
                reply.error_code,
                reply.body
            );
            assert_eq!(
                message(&reply.body),
                SITE_DETAILS_INCOMPLETE,
                "case {case:?} named by {named_by}: expected the incomplete-site sentence, \
                 got {:?}",
                reply.body
            );
            let after = orders_on(&outposts, &outpost_id).await;
            assert_eq!(
                after, before,
                "case {case:?} named by {named_by}: the refused order must mint nothing, \
                 got {after:?}"
            );
        }
    }
}

/// Rule 1 again, on the OTHER way an order names its contents and on the state the
/// refusal tracks: a quote-backed order is refused at an incomplete site too, and
/// recording the missing address makes the identical request succeed.
#[tokio::test]
async fn recording_the_missing_address_makes_the_same_order_succeed() {
    let outposts = Outposts::new();
    let site_id = a_site(&outposts, "restored-site", true, false).await;
    let (outpost_id, _) = an_outpost(&outposts, &site_id, "restored").await;

    let refused = outposts
        .call("POST", "/orders", Some(&order_body(&outpost_id)))
        .await;
    assert_eq!(
        (refused.status, message(&refused.body)),
        (400, SITE_DETAILS_INCOMPLETE.to_string()),
        "case \"before the shipping address is recorded\": the order is refused, got {} {:?}",
        refused.status,
        refused.body
    );

    // The same refusal reaches an order taken from a QUOTE, which is the other way
    // `CreateOrder` gets its contents: the precondition is the site's, not the request's.
    let quote = outposts
        .ok(
            "POST",
            "/quotes",
            Some(&json!({
                "CountryCode": "US",
                "RequestedCapacities": [
                    { "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 1.0 },
                ],
                "OutpostIdentifier": outpost_id.clone(),
            })),
        )
        .await["Quote"]
        .clone();
    let quote_id = text(&quote, "QuoteId");
    let from_quote = json!({
        "OutpostIdentifier": outpost_id.clone(),
        "PaymentOption": "NO_UPFRONT",
        "QuoteIdentifier": quote_id.clone(),
    });
    let refused = outposts.call("POST", "/orders", Some(&from_quote)).await;
    assert_eq!(
        (refused.status, message(&refused.body)),
        (400, SITE_DETAILS_INCOMPLETE.to_string()),
        "case \"an order taken from a quote at an incomplete site\": it is refused, got {} {:?}",
        refused.status,
        refused.body
    );
    assert_eq!(
        orders_on(&outposts, &outpost_id).await,
        Vec::<String>::new(),
        "case \"nothing accepted yet\": the Outpost's orders must still be empty"
    );

    // `UpdateSiteAddress` completes the site's details, and the identical request is
    // then served — the refusal tracks what the site records and nothing else.
    outposts
        .ok(
            "PUT",
            &format!("/sites/{}/address", segment(&site_id)),
            Some(&json!({
                "AddressType": SHIPPING,
                "Address": address("US", "WA", "Kirkland"),
            })),
        )
        .await;
    let placed = outposts
        .ok("POST", "/orders", Some(&order_body(&outpost_id)))
        .await;
    let order_id = text(&placed["Order"], "OrderId");
    assert!(
        orders_on(&outposts, &outpost_id).await.contains(&order_id),
        "case \"after the shipping address is recorded\": the order must be placed and \
         listable, got {placed:?}"
    );
}

/// What one row of rule 2 asks of an Outpost: the operation, the request body, and the
/// refusal wording the status it holds is stated in.
struct Gated {
    case: &'static str,
    /// The route under the Outpost the request goes to, by either identifier form.
    route: fn(&str) -> String,
    body: Value,
}

/// The two routes rule 2 covers, each in the plain form and in the
/// plan-the-work-only form the model declares for it (`StartCapacityTask$DryRun`,
/// `StartOutpostDecommission$ValidateOnly`, both defaulted `false`).
fn gated_calls() -> Vec<Gated> {
    vec![
        Gated {
            case: "StartCapacityTask",
            route: |identifier| format!("/outposts/{}/capacity", segment(identifier)),
            body: json!({ "InstancePools": [{ "InstanceType": "c5.xlarge", "Count": 1 }] }),
        },
        Gated {
            case: "StartCapacityTask, dry run",
            route: |identifier| format!("/outposts/{}/capacity", segment(identifier)),
            body: json!({
                "InstancePools": [{ "InstanceType": "m5.4xlarge", "Count": 2 }],
                "DryRun": true,
            }),
        },
        Gated {
            case: "StartOutpostDecommission",
            route: |identifier| format!("/outposts/{}/decommission", segment(identifier)),
            body: json!({}),
        },
        Gated {
            case: "StartOutpostDecommission, validate only",
            route: |identifier| format!("/outposts/{}/decommission", segment(identifier)),
            body: json!({ "ValidateOnly": true }),
        },
    ]
}

/// The refusal `StartCapacityTask` states for an Outpost that is not `ACTIVE`, as AWS
/// words it: the Outpost id twice and the status it holds.
fn capacity_refusal(outpost_id: &str, status: &str) -> String {
    format!(
        "Failed to create capacity task for {outpost_id}. Outpost should have LifeCycleStatus \
         being {ACTIVE}, but {outpost_id} has LifeCycleStatus {status}"
    )
}

/// The refusal `StartOutpostDecommission` states for the same, as AWS words it.
fn decommission_refusal(outpost_id: &str, status: &str) -> String {
    format!("Outpost {outpost_id} has invalid lifecycleStatus={status}")
}

/// The wording the row's operation refuses with.
fn expected_refusal(case: &str, outpost_id: &str, status: &str) -> String {
    match case.starts_with("StartCapacityTask") {
        true => capacity_refusal(outpost_id, status),
        false => decommission_refusal(outpost_id, status),
    }
}

/// Rule 2: every operation that acts on an Outpost's installed hardware is refused
/// while that hardware is not in service, and served once it is.
///
/// The rows are the two gated operations, each in its plain and plan-only form, crossed
/// with the two identifier forms the request can name the Outpost by — and each row is
/// driven twice: once on an Outpost still in the provisioning status and once on the
/// same Outpost in its terminal status.
#[tokio::test]
async fn an_operation_on_installed_hardware_is_refused_until_the_outpost_is_active() {
    for row in gated_calls() {
        let case = row.case;
        for named_by_arn in [false, true] {
            // A store of its own per row: what the call does depends on what the store
            // holds about this one Outpost.
            let outposts = Outposts::new();
            let (outpost_id, outpost_arn) = outposts.site_and_outpost("gated").await;
            let identifier = match named_by_arn {
                true => outpost_arn.clone(),
                false => outpost_id.clone(),
            };
            let route = (row.route)(&identifier);

            // The Outpost is where a create leaves it: hardware ordered for it has not
            // been installed, so nothing is in service.
            let held = life_cycle_status(&outposts, &outpost_id).await;
            assert_eq!(
                held, PENDING,
                "case {case:?} named by {identifier:?}: a just-created Outpost holds \
                 {PENDING}, got {held:?}"
            );

            let reply = outposts.call("POST", &route, Some(&row.body)).await;
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?} named by {identifier:?}: it must be refused while the \
                 Outpost holds {held}, got {} {} {:?}",
                reply.status,
                reply.error_code,
                reply.body
            );
            assert_eq!(
                message(&reply.body),
                expected_refusal(case, &outpost_id, &held),
                "case {case:?} named by {identifier:?}: the refusal must name the Outpost \
                 and the status that blocks it, got {:?}",
                reply.body
            );
            // Nothing was created and nothing moved: the Outpost is as it stood, and no
            // capacity task was minted for it.
            assert_eq!(
                life_cycle_status(&outposts, &outpost_id).await,
                held,
                "case {case:?} named by {identifier:?}: the refusal must leave the \
                 Outpost's status alone"
            );
            assert_eq!(
                tasks_on(&outposts, &outpost_id).await,
                Vec::<String>::new(),
                "case {case:?} named by {identifier:?}: the refusal must create no \
                 capacity task"
            );

            // The same call on the same Outpost once its hardware is in service: the
            // precondition is met, so the operation answers from its own outcomes.
            mark_outpost_active(&outposts, &outpost_id);
            let served = outposts.call("POST", &route, Some(&row.body)).await;
            assert_eq!(
                served.status, 200,
                "case {case:?} named by {identifier:?}: an {ACTIVE} Outpost takes the \
                 call, got {} {:?}",
                served.status, served.body
            );
        }
    }
}

/// Rule 2's other half: an Outpost of ANOTHER endpoint, and one no store holds, are
/// answered by the identifier rules rather than by the precondition — the gate is about
/// the Outpost's state, not about the value the request carried.
#[tokio::test]
async fn the_precondition_is_judged_after_the_outpost_is_resolved() {
    let outposts = Outposts::new();
    let absent = "op-00000000000000000";
    let cases: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            "an Outpost id nothing holds, capacity",
            format!("/outposts/{absent}/capacity"),
            json!({ "InstancePools": [{ "InstanceType": "c5.xlarge", "Count": 1 }] }),
            404,
            "NotFoundException",
        ),
        (
            "an Outpost id nothing holds, decommission",
            format!("/outposts/{absent}/decommission"),
            json!({}),
            404,
            "NotFoundException",
        ),
        (
            "the ARN form of an Outpost nothing holds, capacity",
            format!(
                "/outposts/{}/capacity",
                segment(&format!("{OUTPOST_ARN_PREFIX}{absent}"))
            ),
            json!({ "InstancePools": [{ "InstanceType": "c5.xlarge", "Count": 1 }] }),
            404,
            "NotFoundException",
        ),
        (
            "a value that is no Outpost identifier, decommission",
            "/outposts/not-an-outpost/decommission".to_string(),
            json!({}),
            400,
            "ValidationException",
        ),
    ];
    for (case, route, body, status, code) in &cases {
        let reply = outposts.call("POST", route, Some(body)).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (*status, *code),
            "case {case:?}: expected {status} {code}, got {} {} {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// Rule 3: a `GetSiteAddress` for a type the site records no address under is refused,
/// the type it does record reads back, and configuring the missing one makes the
/// identical read answer it.
///
/// The rows are every combination of the two modelled `AddressType` values with what
/// the site records (neither, this type, the other type, both), each read by both
/// identifier forms.
#[tokio::test]
async fn a_site_address_is_read_only_for_a_type_the_site_records() {
    let outposts = Outposts::new();
    // (case, operating recorded, shipping recorded)
    let sites: Vec<(&str, bool, bool)> = vec![
        ("neither address", false, false),
        ("the operating address only", true, false),
        ("the shipping address only", false, true),
        ("both addresses", true, true),
    ];

    for (index, (case, operating, shipping)) in sites.iter().enumerate() {
        let site_id = a_site(
            &outposts,
            &format!("address-gate-{index}"),
            *operating,
            *shipping,
        )
        .await;
        let site_arn = text(&outposts.get_site(&site_id).await, "SiteArn");

        for address_type in ADDRESS_TYPES {
            let recorded = match address_type {
                OPERATING => *operating,
                _ => *shipping,
            };
            for named_by in [site_id.as_str(), site_arn.as_str()] {
                let reply = outposts
                    .call(
                        "GET",
                        &format!(
                            "/sites/{}/address?AddressType={}",
                            segment(named_by),
                            query_value(address_type)
                        ),
                        None,
                    )
                    .await;
                if recorded {
                    assert_eq!(
                        reply.status, 200,
                        "case {case:?}, {address_type} by {named_by}: a recorded address \
                         reads back, got {} {:?}",
                        reply.status, reply.body
                    );
                    assert_eq!(
                        (
                            text(&reply.body, "SiteId"),
                            text(&reply.body, "AddressType"),
                            reply.body.get("Address").is_some()
                        ),
                        (site_id.clone(), address_type.to_string(), true),
                        "case {case:?}, {address_type} by {named_by}: the read names the \
                         site, the type and the address, got {:?}",
                        reply.body
                    );
                    continue;
                }
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (404, "NotFoundException"),
                    "case {case:?}, {address_type} by {named_by}: a type the site records \
                     nothing under must be refused, got {} {} {:?}",
                    reply.status,
                    reply.error_code,
                    reply.body
                );
                assert_eq!(
                    message(&reply.body),
                    format!("{address_type} not found for SiteId={site_id}"),
                    "case {case:?}, {address_type} by {named_by}: the refusal must name the \
                     type and the site, got {:?}",
                    reply.body
                );
            }
        }
    }

    // The restore leg, on a site that records only one of the two: configuring the
    // missing type through `UpdateSiteAddress` makes the identical read answer it, and
    // leaves the type that was already there exactly as it was.
    let site_id = a_site(&outposts, "address-gate-restored", true, false).await;
    let shipping = address("IE", "Leinster", "Dublin");
    outposts
        .ok(
            "PUT",
            &format!("/sites/{}/address", segment(&site_id)),
            Some(&json!({ "AddressType": SHIPPING, "Address": shipping })),
        )
        .await;
    for (address_type, expected) in [
        (SHIPPING, shipping.clone()),
        (OPERATING, address("US", "WA", "Seattle")),
    ] {
        let read = outposts
            .ok(
                "GET",
                &format!(
                    "/sites/{}/address?AddressType={}",
                    segment(&site_id),
                    query_value(address_type)
                ),
                None,
            )
            .await;
        assert_eq!(
            read,
            json!({ "SiteId": site_id, "AddressType": address_type, "Address": expected }),
            "case \"restored {address_type}\": the read must answer the recorded address, \
             got {read:?}"
        );
    }
}

/// Rule 1's shared harness, exercised: the orderable site every other suite places its
/// orders at records both addresses, so the gate above never fires for them.
#[tokio::test]
async fn the_shared_orderable_site_records_both_addresses() {
    let outposts = Outposts::new();
    let site_id = text(
        &outposts.create_site(&orderable_site("shared-orderable")).await,
        "SiteId",
    );
    for address_type in ADDRESS_TYPES {
        let reply = outposts
            .call(
                "GET",
                &format!(
                    "/sites/{}/address?AddressType={}",
                    segment(&site_id),
                    query_value(address_type)
                ),
                None,
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {address_type:?}: the shared orderable site records this address, \
             got {} {:?}",
            reply.status, reply.body
        );
    }
}
