// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ARN form of the identifier members of this API, driven
//! over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses).
//!
//! The rule under test, stated once and then exercised over a SET of inputs: an ARN
//! is an ABSOLUTE identity — its partition, its Region and its account id are part of
//! the name — so an identifier supplied in ARN form names a resource of the endpoint
//! being called only when all three are that endpoint's own. One that differs in any
//! of them names no resource the endpoint holds, so every operation naming it answers
//! the modelled `NotFoundException` (404) — in the wording that route refuses an
//! unresolvable identifier of that kind with — and persists nothing; a filter naming
//! one simply matches nothing. The
//! bare id and the endpoint's OWN ARN keep naming the resource.
//!
//! The rows differ along everything the rule generalises over: which ARN segment the
//! value differs in (account, Region, partition, Region + partition, all three), the
//! endpoint the call is addressed to and the partition it belongs to (a commercial
//! Region and a Region of another partition, so the check is not "the partition must
//! be `aws`"), the resource kind (site, Outpost, quote), where the identifier travels
//! (a URI label and a request body), and the operation — every route of the
//! implemented slice that names a site, an Outpost or a quote by identifier, reading,
//! changing, deleting and referencing it.

use serde_json::{json, Value};

use crate::capacity_task_lifecycle::pool;
use crate::quote_lifecycle::request as quote_request;
use crate::site_and_outpost_lifecycle::{address, ids, segment, text, Outposts};

/// A 12-digit account id that is not the one this emulator presents, so an ARN
/// carrying it names another account's resource. Any value of the modelled
/// `\d{12}` account field that is not [`engine::emulator_account_id`] would do.
const OTHER_ACCOUNT: &str = "999999999999";

/// A catalog item this service publishes, which an order's line item names.
const CATALOG_ITEM: &str = "OR-1A2B3C4";

/// One endpoint this suite exercises the rule at, with the identities an ARN is
/// forged against it from.
///
/// Source: AWS's published Regions and the partitions they belong to — `aws` for the
/// commercial Regions and `aws-cn` for `cn-*` — which is the mapping every ARN this
/// service mints follows and which the modelled ARN patterns spell `aws([a-z-]+)?`.
///
/// The same rule holds of the tagging `ResourceArn` member, so
/// [`crate::tagging_arn_identity`] exercises it over these very endpoints and
/// forgeries rather than restating them.
pub(crate) struct Endpoint {
    /// The Region every call of the case is addressed to, and whose inventory holds
    /// the resources it was given.
    pub(crate) region: &'static str,
    /// The partition that Region belongs to: the one its own ARNs carry.
    partition: &'static str,
    /// Another Region of the SAME partition.
    sibling_region: &'static str,
    /// Another partition, and a Region of it.
    other_partition: &'static str,
    other_region: &'static str,
}

/// The endpoints the rule is exercised at: one commercial Region, and one Region of
/// another partition — so no row's expectation can come from a fixed `aws` literal.
pub(crate) const ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        region: "us-east-1",
        partition: "aws",
        sibling_region: "eu-west-1",
        other_partition: "aws-cn",
        other_region: "cn-north-1",
    },
    Endpoint {
        region: "cn-north-1",
        partition: "aws-cn",
        sibling_region: "cn-northwest-1",
        other_partition: "aws",
        other_region: "us-east-1",
    },
];

/// The identity each row writes into the ARN in place of the endpoint's own, as
/// `(case, partition, Region, account)`.
///
/// Every row differs from the endpoint in at least one of the three segments an ARN
/// identifies a resource by, and together they span each segment alone, two of them
/// together, and all three.
pub(crate) fn forgeries(endpoint: &Endpoint) -> Vec<(String, String, String, String)> {
    let account = engine::emulator_account_id().to_string();
    vec![
        (
            "another account".to_string(),
            endpoint.partition.to_string(),
            endpoint.region.to_string(),
            OTHER_ACCOUNT.to_string(),
        ),
        (
            "another Region of the same partition".to_string(),
            endpoint.partition.to_string(),
            endpoint.sibling_region.to_string(),
            account.clone(),
        ),
        (
            "another partition and a Region of it".to_string(),
            endpoint.other_partition.to_string(),
            endpoint.other_region.to_string(),
            account.clone(),
        ),
        (
            "another partition, this endpoint's Region".to_string(),
            endpoint.other_partition.to_string(),
            endpoint.region.to_string(),
            account,
        ),
        (
            "another partition, Region and account".to_string(),
            endpoint.other_partition.to_string(),
            endpoint.other_region.to_string(),
            OTHER_ACCOUNT.to_string(),
        ),
    ]
}

/// One Outposts resource ARN: `arn:<partition>:outposts:<region>:<account>:<kind>/<id>`,
/// the form every ARN branch of an identifier pattern in this model spells.
pub(crate) fn arn(
    partition: &str,
    region: &str,
    account: &str,
    kind: &str,
    resource_id: &str,
) -> String {
    format!("arn:{partition}:outposts:{region}:{account}:{kind}/{resource_id}")
}

/// The resources one endpoint was given, each kind the identifier members name.
struct Created {
    site_id: String,
    outpost_id: String,
    quote_id: String,
    capacity_task_id: String,
    order_id: String,
}

/// One call that names a resource by identifier: the request, plus the input member
/// the identifier arrived in and the value it carried, which is what a not-found body
/// echoes on the routes that word their refusal that way.
struct Call {
    case: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    member: &'static str,
    supplied: String,
    worded: Option<String>,
}

impl Call {
    fn new(
        case: String,
        method: &'static str,
        path: String,
        body: Option<Value>,
        member: &'static str,
        supplied: &str,
    ) -> Self {
        Self {
            case,
            method,
            path,
            body,
            member,
            supplied: supplied.to_string(),
            worded: None,
        }
    }

    /// The wording this route states instead of the echo, for a kind of resource
    /// AWS refuses by a sentence of its own or by the identifier it resolved.
    fn worded(mut self, message: String) -> Self {
        self.worded = Some(message);
        self
    }

    /// The `$.Message` this service's not-found rule states for the call: this
    /// route's own wording, or — on the registry routes that echo — the member the
    /// identifier arrived in, `=`, and the value exactly as supplied.
    fn expected(&self) -> String {
        self.worded
            .clone()
            .unwrap_or_else(|| format!("{}={}", self.member, self.supplied))
    }
}

/// The bare `op-…` id an Outpost identifier carries, whichever of the two modelled
/// forms it arrived in: the resource part of an ARN, or the value itself.
fn bare_outpost_id(identifier: &str) -> &str {
    identifier.rsplit_once('/').map_or(identifier, |(_, id)| id)
}

/// The sentence the capacity routes refuse an Outpost they hold none of with.
fn no_outpost_with(identifier: &str) -> String {
    format!("No Outpost with outpostId={}", bare_outpost_id(identifier))
}

/// The sentence the quote routes refuse a quote they hold none of with.
fn no_quote_with(identifier: &str) -> String {
    format!("Could not find Quote with identifier {identifier}")
}

/// Create, through `region`'s endpoint, one of each resource an identifier member of
/// this API names: a site with both addresses, an Outpost on it, a quote associated
/// with that Outpost, a capacity task on it and an order placed on it.
async fn create_everything(outposts: &Outposts, region: &str) -> Created {
    let site = outposts
        .create_site_in(
            region,
            &json!({
                "Name": format!("{region} site"),
                "OperatingAddress": address("US", "WA", "Seattle"),
                "ShippingAddress": address("US", "WA", "Kirkland"),
            }),
        )
        .await;
    let site_id = text(&site, "SiteId");
    let outpost = outposts
        .create_outpost_in(
            region,
            &json!({ "Name": format!("{region} outpost"), "SiteId": site_id }),
        )
        .await;
    let outpost_id = text(&outpost, "OutpostId");
    // A capacity task is refused on an Outpost whose hardware is not in service
    // ([`crate::precondition_gates`]), so the Outpost this creates one on is put past
    // that gate first — which is what an Outpost holding a capacity task has to be.
    crate::site_and_outpost_lifecycle::mark_outpost_active(outposts, &outpost_id);
    let quote = outposts
        .ok_in(
            region,
            "POST",
            "/quotes",
            Some(&quote_request(
                "US",
                json!([]),
                json!({ "OutpostIdentifier": outpost_id }),
            )),
        )
        .await["Quote"]
        .clone();
    let task = outposts
        .ok_in(
            region,
            "POST",
            &format!("/outposts/{}/capacity", segment(&outpost_id)),
            Some(&json!({ "InstancePools": [pool("m5.4xlarge", 1)] })),
        )
        .await;
    let order = outposts
        .ok_in(
            region,
            "POST",
            "/orders",
            Some(&json!({
                "OutpostIdentifier": outpost_id,
                "PaymentOption": "ALL_UPFRONT",
                "LineItems": [{ "CatalogItemId": CATALOG_ITEM, "Quantity": 1 }],
            })),
        )
        .await["Order"]
        .clone();
    Created {
        site_id,
        outpost_id,
        quote_id: text(&quote, "QuoteId"),
        capacity_task_id: text(&task, "CapacityTaskId"),
        order_id: text(&order, "OrderId"),
    }
}

/// Every route of the implemented slice that names a SITE by `SiteId`, with `site`
/// as that identifier.
fn site_calls(site: &str) -> Vec<Call> {
    let at = |suffix: &str| format!("/sites/{}{suffix}", segment(site));
    vec![
        Call::new(
            format!("GetSite {site}"),
            "GET",
            at(""),
            None,
            "siteId",
            site,
        ),
        Call::new(
            format!("GetSiteAddress {site}"),
            "GET",
            at("/address?AddressType=OPERATING_ADDRESS"),
            None,
            "siteId",
            site,
        ),
        Call::new(
            format!("UpdateSite {site}"),
            "PATCH",
            at(""),
            Some(json!({ "Name": "renamed through a foreign ARN" })),
            "siteId",
            site,
        ),
        Call::new(
            format!("UpdateSiteAddress {site}"),
            "PUT",
            at("/address"),
            Some(json!({
                "AddressType": "SHIPPING_ADDRESS",
                "Address": address("US", "OR", "Portland"),
            })),
            "siteId",
            site,
        ),
        Call::new(
            format!("UpdateSiteRackPhysicalProperties {site}"),
            "PATCH",
            at("/rackPhysicalProperties"),
            Some(json!({ "PowerDrawKva": "POWER_10_KVA" })),
            "siteId",
            site,
        ),
        Call::new(
            format!("DeleteSite {site}"),
            "DELETE",
            at(""),
            None,
            "siteId",
            site,
        ),
        Call::new(
            format!("CreateOutpost on {site}"),
            "POST",
            "/outposts".to_string(),
            Some(json!({ "Name": "outpost on a foreign site", "SiteId": site })),
            "siteId",
            site,
        ),
    ]
}

/// Every route of the implemented slice that names an OUTPOST, with `outpost` as that
/// identifier — the `OutpostId` member on the Outpost's own routes, and the
/// `OutpostIdentifier` member everywhere else.
fn outpost_calls(outpost: &str, capacity_task_id: &str) -> Vec<Call> {
    let at = |suffix: &str| format!("/outposts/{}{suffix}", segment(outpost));
    let task = format!("/capacity/{}", segment(capacity_task_id));
    vec![
        Call::new(
            format!("GetOutpost {outpost}"),
            "GET",
            at(""),
            None,
            "outpostId",
            outpost,
        ),
        Call::new(
            format!("UpdateOutpost {outpost}"),
            "PATCH",
            at(""),
            Some(json!({ "Name": "renamed through a foreign ARN" })),
            "outpostId",
            outpost,
        ),
        Call::new(
            format!("DeleteOutpost {outpost}"),
            "DELETE",
            at(""),
            None,
            "outpostId",
            outpost,
        ),
        Call::new(
            format!("GetPrivateConnectivityConfig {outpost}"),
            "GET",
            at("/privateConnectivity"),
            None,
            "outpostId",
            outpost,
        ),
        Call::new(
            format!("CreatePrivateConnectivityConfig {outpost}"),
            "POST",
            at("/privateConnectivity"),
            Some(json!({ "VpcInformationList": [{ "VpcId": "vpc-0000000000001" }] })),
            "outpostId",
            outpost,
        ),
        Call::new(
            format!("GetOutpostInstanceTypes {outpost}"),
            "GET",
            at("/instanceTypes"),
            None,
            "outpostId",
            outpost,
        ),
        Call::new(
            format!("GetOutpostSupportedInstanceTypes {outpost}"),
            "GET",
            at("/supportedInstanceTypes"),
            None,
            "outpostIdentifier",
            outpost,
        ),
        Call::new(
            format!("ListAssets {outpost}"),
            "GET",
            at("/assets"),
            None,
            "outpostIdentifier",
            outpost,
        ),
        Call::new(
            format!("ListAssetInstances {outpost}"),
            "GET",
            at("/assetInstances"),
            None,
            "outpostIdentifier",
            outpost,
        ),
        Call::new(
            format!("GetCapacityTask {outpost}"),
            "GET",
            at(&task),
            None,
            "outpostIdentifier",
            outpost,
        )
        .worded(no_outpost_with(outpost)),
        Call::new(
            format!("CancelCapacityTask {outpost}"),
            "POST",
            at(&task),
            None,
            "outpostIdentifier",
            outpost,
        )
        .worded(no_outpost_with(outpost)),
        Call::new(
            format!("ListBlockingInstancesForCapacityTask {outpost}"),
            "GET",
            at(&format!("{task}/blockingInstances")),
            None,
            "outpostIdentifier",
            outpost,
        )
        .worded(no_outpost_with(outpost)),
        Call::new(
            format!("StartCapacityTask {outpost}"),
            "POST",
            at("/capacity"),
            Some(json!({ "InstancePools": [pool("m5.4xlarge", 1)] })),
            "outpostIdentifier",
            outpost,
        )
        .worded(no_outpost_with(outpost)),
        Call::new(
            format!("StartOutpostDecommission {outpost}"),
            "POST",
            at("/decommission"),
            Some(json!({ "ValidateOnly": false })),
            "outpostIdentifier",
            outpost,
        ),
        // `GetRenewalPricing`'s route is the singular `/outpost/{…}`.
        Call::new(
            format!("GetRenewalPricing {outpost}"),
            "GET",
            format!("/outpost/{}/renewal-pricing", segment(outpost)),
            None,
            "outpostIdentifier",
            outpost,
        ),
        // The three routes that take an Outpost identifier in the BODY rather than in
        // the URI: the same member, read the same way.
        Call::new(
            format!("CreateQuote for {outpost}"),
            "POST",
            "/quotes".to_string(),
            Some(quote_request(
                "US",
                json!([]),
                json!({ "OutpostIdentifier": outpost }),
            )),
            "outpostIdentifier",
            outpost,
        ),
        Call::new(
            format!("CreateOrder on {outpost}"),
            "POST",
            "/orders".to_string(),
            Some(json!({
                "OutpostIdentifier": outpost,
                "PaymentOption": "ALL_UPFRONT",
                "LineItems": [{ "CatalogItemId": CATALOG_ITEM, "Quantity": 1 }],
            })),
            "outpostIdentifier",
            outpost,
        )
        // This route resolves the Outpost to its ARN first, and names that ARN —
        // which, for an identifier that arrived AS an ARN, is the one supplied.
        .worded(format!("outpostId={outpost}")),
        Call::new(
            format!("CreateRenewal for {outpost}"),
            "POST",
            "/renewals".to_string(),
            Some(json!({
                "OutpostIdentifier": outpost,
                "PaymentOption": "ALL_UPFRONT",
                "PaymentTerm": "ONE_YEAR",
            })),
            "outpostIdentifier",
            outpost,
        ),
    ]
}

/// Every route of the implemented slice that names a QUOTE by `QuoteIdentifier`, with
/// `quote` as that identifier.
fn quote_calls(quote: &str) -> Vec<Call> {
    let at = || format!("/quotes/{}", segment(quote));
    vec![
        Call::new(
            format!("GetQuote {quote}"),
            "GET",
            at(),
            None,
            "quoteIdentifier",
            quote,
        )
        .worded(no_quote_with(quote)),
        Call::new(
            format!("UpdateQuote {quote}"),
            "PATCH",
            at(),
            Some(json!({ "CountryCode": "DE" })),
            "quoteIdentifier",
            quote,
        )
        .worded(no_quote_with(quote)),
        Call::new(
            format!("DeleteQuote {quote}"),
            "DELETE",
            at(),
            None,
            "quoteIdentifier",
            quote,
        )
        .worded(no_quote_with(quote)),
    ]
}

/// The routes that take a quote identifier or an Outpost identifier BESIDE another
/// identifier that does name a resource here: `CreateOrder`'s `QuoteIdentifier`,
/// placed on the endpoint's own Outpost, and the association `UpdateQuote` takes in
/// its body, on the endpoint's own quote. Each has exactly one forged member, so the
/// refusal is about that one.
fn quote_reference_calls(created: &Created, outpost: &str, quote: &str) -> Vec<Call> {
    vec![
        Call::new(
            format!("CreateOrder from quote {quote}"),
            "POST",
            "/orders".to_string(),
            Some(json!({
                "OutpostIdentifier": created.outpost_id,
                "QuoteIdentifier": quote,
                "PaymentOption": "ALL_UPFRONT",
            })),
            "quoteIdentifier",
            quote,
        )
        .worded(no_quote_with(quote)),
        Call::new(
            format!("UpdateQuote associating {outpost}"),
            "PATCH",
            format!("/quotes/{}", segment(&created.quote_id)),
            Some(json!({ "OutpostIdentifier": outpost })),
            "outpostIdentifier",
            outpost,
        ),
    ]
}

/// Every call of the rule, for one endpoint's resources named by `forged` identifiers.
fn calls_naming(created: &Created, forged_site: &str, forged_outpost: &str, forged_quote: &str) -> Vec<Call> {
    let mut calls = site_calls(forged_site);
    calls.extend(outpost_calls(forged_outpost, &created.capacity_task_id));
    calls.extend(quote_calls(forged_quote));
    calls.extend(quote_reference_calls(created, forged_outpost, forged_quote));
    calls
}

/// The rule itself: at each endpoint, every route that names a site, an Outpost or a
/// quote by an ARN of another partition, Region or account answers 404
/// `NotFoundException` in that route's own not-found wording — and the endpoint's own
/// inventory is left exactly as it was.
#[tokio::test]
async fn an_identifier_arn_of_another_endpoint_names_no_resource_here() {
    for endpoint in ENDPOINTS {
        let outposts = Outposts::new();
        let created = create_everything(&outposts, endpoint.region).await;

        // The control: the endpoint's OWN ARN for each kind names the resource, so a
        // refusal below is about the identity the ARN states and not about the ARN
        // form itself.
        let account = engine::emulator_account_id();
        for (kind, resource_id, route) in [
            ("site", &created.site_id, "sites"),
            ("outpost", &created.outpost_id, "outposts"),
            ("quote", &created.quote_id, "quotes"),
        ] {
            let own = arn(
                endpoint.partition,
                endpoint.region,
                account,
                kind,
                resource_id,
            );
            let reply = outposts
                .call_in(
                    endpoint.region,
                    "GET",
                    &format!("/{route}/{}", segment(&own)),
                    None,
                )
                .await;
            assert_eq!(
                reply.status, 200,
                "case {:?} in {}: the endpoint's own ARN names the {kind}, got {} {:?}",
                own, endpoint.region, reply.status, reply.body
            );
        }

        for (case, partition, region, forged_account) in forgeries(endpoint) {
            let forged_site = arn(&partition, &region, &forged_account, "site", &created.site_id);
            let forged_outpost = arn(
                &partition,
                &region,
                &forged_account,
                "outpost",
                &created.outpost_id,
            );
            let forged_quote = arn(
                &partition,
                &region,
                &forged_account,
                "quote",
                &created.quote_id,
            );
            for call in calls_naming(&created, &forged_site, &forged_outpost, &forged_quote) {
                let reply = outposts
                    .call_in(endpoint.region, call.method, &call.path, call.body.as_ref())
                    .await;
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (404, "NotFoundException"),
                    "case {case:?} at {} ({:?}): an ARN that is not this endpoint's own names no \
                     resource here, got {} {} {:?}",
                    endpoint.region,
                    call.case,
                    reply.status,
                    reply.error_code,
                    reply.body
                );
                let message = text(&reply.body, "Message");
                assert_eq!(
                    message,
                    call.expected(),
                    "case {case:?} at {} ({:?}): the body states this route's not-found \
                     wording, expected {:?}, got {message:?}",
                    endpoint.region,
                    call.case,
                    call.expected()
                );
            }
        }

        // Nothing any forged call asked for happened: every resource is still exactly
        // as its own endpoint created it, and nothing new was created.
        let site = outposts
            .ok_in(
                endpoint.region,
                "GET",
                &format!("/sites/{}", segment(&created.site_id)),
                None,
            )
            .await["Site"]
            .clone();
        assert_eq!(
            text(&site, "Name"),
            format!("{} site", endpoint.region),
            "case {}: no forged UpdateSite renamed the site, got {site:?}",
            endpoint.region
        );
        let shipping = outposts
            .ok_in(
                endpoint.region,
                "GET",
                &format!(
                    "/sites/{}/address?AddressType=SHIPPING_ADDRESS",
                    segment(&created.site_id)
                ),
                None,
            )
            .await["Address"]
            .clone();
        assert_eq!(
            text(&shipping, "City"),
            "Kirkland",
            "case {}: no forged UpdateSiteAddress moved the shipping address, got {shipping:?}",
            endpoint.region
        );
        let outpost = outposts
            .get_outpost_in(endpoint.region, &created.outpost_id)
            .await;
        assert_eq!(
            text(&outpost, "Name"),
            format!("{} outpost", endpoint.region),
            "case {}: no forged UpdateOutpost renamed the Outpost, got {outpost:?}",
            endpoint.region
        );
        let connectivity = outposts
            .ok_in(
                endpoint.region,
                "GET",
                &format!(
                    "/outposts/{}/privateConnectivity",
                    segment(&created.outpost_id)
                ),
                None,
            )
            .await;
        assert_eq!(
            connectivity.get("PrivateConnectivityConfig"),
            None,
            "case {}: no forged CreatePrivateConnectivityConfig configured the Outpost, \
             got {connectivity:?}",
            endpoint.region
        );
        assert_eq!(
            ids(
                &outposts.list_outposts_in(endpoint.region, "").await,
                "Outposts"
            ),
            vec![created.outpost_id.clone()],
            "case {}: no forged CreateOutpost placed an Outpost",
            endpoint.region
        );
        assert_eq!(
            ids(&outposts.list_sites_in(endpoint.region, "").await, "Sites"),
            vec![created.site_id.clone()],
            "case {}: the site is still there and no other was created",
            endpoint.region
        );
        for (route, member, id_member, expected) in [
            ("/quotes", "Quotes", "QuoteId", created.quote_id.clone()),
            ("/list-orders", "Orders", "OrderId", created.order_id.clone()),
            (
                "/capacity/tasks",
                "CapacityTasks",
                "CapacityTaskId",
                created.capacity_task_id.clone(),
            ),
        ] {
            let listed = listed_ids(
                &outposts.ok_in(endpoint.region, "GET", route, None).await,
                member,
                id_member,
            );
            assert_eq!(
                listed,
                vec![expected.clone()],
                "case {}: {member} still holds exactly what this endpoint created, got {listed:?}",
                endpoint.region
            );
        }
        // The quote is still associated with the Outpost it was created against: no
        // forged `UpdateQuote` moved or cleared the association.
        let quote = outposts
            .ok_in(
                endpoint.region,
                "GET",
                &format!("/quotes/{}", segment(&created.quote_id)),
                None,
            )
            .await["Quote"]
            .clone();
        assert_eq!(
            text(&quote, "OutpostArn"),
            arn(
                endpoint.partition,
                endpoint.region,
                engine::emulator_account_id(),
                "outpost",
                &created.outpost_id
            ),
            "case {}: the quote still names this endpoint's Outpost, got {quote:?}",
            endpoint.region
        );
        assert_eq!(
            text(&quote, "CountryCode"),
            "US",
            "case {}: no forged UpdateQuote changed the quote, got {quote:?}",
            endpoint.region
        );
    }
}

/// The other half of the rule, for the member that is a FILTER rather than an
/// identifier the request is addressed to: a filter naming an Outpost of another
/// partition, Region or account matches nothing, which is an empty page and not a
/// refusal — while the bare id and this endpoint's own ARN both narrow to the
/// Outpost's own entries.
#[tokio::test]
async fn an_outpost_filter_arn_of_another_endpoint_matches_nothing() {
    for endpoint in ENDPOINTS {
        let outposts = Outposts::new();
        let created = create_everything(&outposts, endpoint.region).await;
        let own = arn(
            endpoint.partition,
            endpoint.region,
            engine::emulator_account_id(),
            "outpost",
            &created.outpost_id,
        );

        // Both forms that DO name this endpoint's Outpost narrow to its own entries.
        for identifier in [&created.outpost_id, &own] {
            for (route, member, id_member, expected) in [
                ("/list-orders", "Orders", "OrderId", &created.order_id),
                (
                    "/capacity/tasks",
                    "CapacityTasks",
                    "CapacityTaskId",
                    &created.capacity_task_id,
                ),
            ] {
                let listed = listed_ids(
                    &outposts
                        .ok_in(
                            endpoint.region,
                            "GET",
                            &format!(
                                "{route}?OutpostIdentifierFilter={}",
                                segment(identifier)
                            ),
                            None,
                        )
                        .await,
                    member,
                    id_member,
                );
                assert_eq!(
                    listed,
                    vec![expected.clone()],
                    "case {identifier:?} at {}: the filter narrows to this Outpost's {member}, \
                     got {listed:?}",
                    endpoint.region
                );
            }
        }

        // Every forged identity matches nothing at all.
        for (case, partition, region, forged_account) in forgeries(endpoint) {
            let forged = arn(
                &partition,
                &region,
                &forged_account,
                "outpost",
                &created.outpost_id,
            );
            for (route, member, id_member) in [
                ("/list-orders", "Orders", "OrderId"),
                ("/capacity/tasks", "CapacityTasks", "CapacityTaskId"),
            ] {
                let reply = outposts
                    .call_in(
                        endpoint.region,
                        "GET",
                        &format!("{route}?OutpostIdentifierFilter={}", segment(&forged)),
                        None,
                    )
                    .await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?} at {} ({route}): a filter is not an identifier the request is \
                     addressed to, so it is answered rather than refused, got {} {:?}",
                    endpoint.region, reply.status, reply.body
                );
                let listed = listed_ids(&reply.body, member, id_member);
                assert!(
                    listed.is_empty(),
                    "case {case:?} at {} ({route}): an ARN of another endpoint names no Outpost \
                     of this inventory, so nothing matches it, got {listed:?}",
                    endpoint.region
                );
            }
        }
    }
}

/// The identifiers a list page reports, read off `id_member` of each entry of
/// `member` — the counterpart of [`ids`] for the lists whose entries are keyed by
/// something other than a site or Outpost id.
fn listed_ids(output: &Value, member: &str, id_member: &str) -> Vec<String> {
    output
        .get(member)
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| text(item, id_member)).collect())
        .unwrap_or_default()
}
