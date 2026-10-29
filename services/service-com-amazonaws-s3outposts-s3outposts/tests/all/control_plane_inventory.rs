// SPDX-License-Identifier: CC0-1.0

//! The S3 on Outposts inventory is the AWS Outposts control plane's own, gated on the
//! S3 capacity ordered for each Outpost — driven end to end over HTTP across BOTH
//! emulated services.
//!
//! Every case here goes through the real front door (`MultiServiceRouter::dispatch`),
//! with the `outposts` calls and the `s3-outposts` calls addressed exactly as a signed
//! SDK call arrives, so what is asserted is the emulator's HTTP behaviour and nothing
//! reaches a handler directly.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. An Outpost the AWS Outposts control plane holds is reported by
//!    `ListOutpostsWithS3` exactly when it is active, not being decommissioned, and
//!    the orders placed on it bought S3 storage capacity — and its `CapacityInBytes`
//!    is that ordered capacity: the S3 capacity the catalog publishes for each line
//!    item's configuration times the quantity ordered, summed over the orders on it.
//! 2. An Outpost with no ordered S3 capacity is in no listing and resolves nowhere:
//!    every operation that names it answers the modelled 404.
//! 3. The endpoint lifecycle runs on an Outpost provisioned purely through AWS API
//!    calls, and each resource stays inside the Region its Outpost is installed in,
//!    with every ARN in that Region's own partition.
//! 4. An Outpost of another account, shared through Resource Access Manager, is
//!    reported beside the control plane's own, each under its owner's account.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_outposts_outpostsolafservice::state as outposts_state;
use service_com_amazonaws_s3outposts_s3outposts::state::{
    OutpostRecord, OUTPOST_NOT_FOUND_MESSAGE,
};
use service_com_amazonaws_s3outposts_s3outposts::ComAmazonawsS3outpostsS3outpostsState;

/// The Region most cases work in; the Region cases vary it.
pub(crate) const REGION: &str = "us-east-1";

/// The `PaymentOption` every order here is placed on, from the modelled value set
/// `PAYMENT_OPTION_VALUES`.
const PAYMENT_OPTION: &str = "ALL_UPFRONT";

/// A `SkuCode` of the modelled form (`^OR-[A-Z0-9]{7}$`) that this emulator's catalog
/// publishes no configuration under — checked against the published catalog by
/// [`Catalog::discover`] rather than assumed.
const UNPUBLISHED_SKU: &str = "OR-0000000";

/// An account that is NOT the one this emulator serves: the owner of an Outpost shared
/// with the caller through Resource Access Manager. A synthetic, non-routable
/// placeholder in the same all-zero family as `engine::emulator_account_id`.
pub(crate) const SHARING_ACCOUNT_ID: &str = "000000000001";

/// One emulator hosting BOTH services over ONE shared engine — the arrangement the
/// router builds, and the only one in which `s3-outposts` can report the Outposts
/// `outposts` owns.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The one engine both services were built over. Reached only to stand up a state
    /// no sequence of AWS API calls can reach (an Outpost shared by another account, a
    /// decommission of an Outpost that still has an order on it); every assertion
    /// about behaviour goes over the wire.
    engine: Arc<engine::InMemoryEngine>,
}

/// One answer: the HTTP status and the parsed JSON body.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Reply {
    /// The wire error code (`__type`), short form, or the empty string on a success.
    pub(crate) fn error_code(&self) -> String {
        self.body["__type"]
            .as_str()
            .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
            .unwrap_or_default()
    }

    /// The `Message` an error payload carries, or the generic `message` the boundary
    /// validator renders.
    pub(crate) fn message(&self) -> &str {
        self.body["Message"]
            .as_str()
            .or_else(|| self.body["message"].as_str())
            .unwrap_or_default()
    }
}

/// A SigV4 credential scope naming `signing_name`, signed for `region`: what routes a
/// request to one of the two services, and the Region it is served in.
fn authorization(signing_name: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_s3outposts_s3outposts::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_outposts_outpostsolafservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router, engine }
    }

    /// Dispatch one request to the service `signing_name` names, addressed to `region`.
    pub(crate) async fn call(
        &self,
        signing_name: &str,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        let mut builder = ::http::Request::builder()
            .method(method)
            .uri(format!("https://{signing_name}.{region}.amazonaws.com{path}"))
            .header("host", format!("{signing_name}.{region}.amazonaws.com"))
            .header("authorization", authorization(signing_name, region));
        if !payload.is_empty() {
            builder = builder.header("content-type", "application/json");
        }
        let request = builder
            .body(Bytes::from(payload))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{method} {path} answered a body that is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Reply { status, body }
    }

    /// One `outposts` call in `region`.
    pub(crate) async fn outposts(&self, region: &str, method: &str, path: &str, body: Option<Value>) -> Reply {
        self.call("outposts", region, method, path, body).await
    }

    /// One `outposts` call that must be served, returning its body.
    pub(crate) async fn outposts_ok(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Value {
        let reply = self.outposts(region, method, path, body).await;
        assert_eq!(
            reply.status, 200,
            "outposts {method} {path} in {region}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body
    }

    /// One `s3-outposts` call in `region`.
    pub(crate) async fn s3(&self, region: &str, method: &str, path: &str, body: Option<Value>) -> Reply {
        self.call("s3-outposts", region, method, path, body).await
    }

    /// This service's own store in the shared engine. See [`Emulator::engine`].
    pub(crate) fn s3_store(&self) -> ::engine::ServiceState<ComAmazonawsS3outpostsS3outpostsState> {
        service_com_amazonaws_s3outposts_s3outposts::state::service_state(&*self.engine)
            .expect("the S3 on Outposts store is reachable through the shared engine")
    }

    /// The AWS Outposts store in the shared engine. See [`Emulator::engine`].
    pub(crate) fn outposts_store(
        &self,
    ) -> ::engine::ServiceState<
        service_com_amazonaws_outposts_outpostsolafservice::ComAmazonawsOutpostsOutpostsolafserviceState,
    > {
        outposts_state::service_state(&*self.engine)
            .expect("the AWS Outposts store is reachable through the shared engine")
    }

    /// One site and one Outpost on it in `region`, through `outposts:CreateSite` and
    /// `outposts:CreateOutpost`, as that Outpost's `OutpostId`.
    pub(crate) async fn an_outpost_in(&self, region: &str, name: &str) -> String {
        let site = self
            .outposts_ok(
                region,
                "POST",
                "/sites",
                Some(json!({ "Name": format!("{name}-site") })),
            )
            .await;
        let site_id = string_at(&site["Site"], "SiteId");
        let outpost = self
            .outposts_ok(
                region,
                "POST",
                "/outposts",
                Some(json!({
                    "Name": name,
                    "SiteId": site_id,
                    "SupportedHardwareType": "RACK",
                })),
            )
            .await;
        string_at(&outpost["Outpost"], "OutpostId")
    }

    /// `outposts:CreateOrder` placing `line_items` (SKU and quantity) against
    /// `outpost_id`, as the `OrderId` it minted.
    pub(crate) async fn an_order_in(
        &self,
        region: &str,
        outpost_id: &str,
        line_items: &[(String, i64)],
    ) -> String {
        let items: Vec<Value> = line_items
            .iter()
            .map(|(sku, quantity)| json!({ "CatalogItemId": sku, "Quantity": quantity }))
            .collect();
        let order = self
            .outposts_ok(
                region,
                "POST",
                "/orders",
                Some(json!({
                    "OutpostIdentifier": outpost_id,
                    "PaymentOption": PAYMENT_OPTION,
                    "LineItems": items,
                })),
            )
            .await;
        string_at(&order["Order"], "OrderId")
    }

    /// `outposts:CancelOrder` on `order_id`.
    pub(crate) async fn cancel_order_in(&self, region: &str, order_id: &str) {
        self.outposts_ok(
            region,
            "POST",
            &format!("/orders/{order_id}/cancel"),
            Some(json!({})),
        )
        .await;
    }

    /// `s3-outposts:ListOutpostsWithS3` in `region`, as the `Outposts` it reported.
    pub(crate) async fn inventory_in(&self, region: &str, query: &str) -> Vec<Value> {
        let reply = self
            .s3(
                region,
                "GET",
                &format!("/S3Outposts/ListOutpostsWithS3{query}"),
                None,
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "ListOutpostsWithS3 in {region}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body["Outposts"]
            .as_array()
            .unwrap_or_else(|| panic!("ListOutpostsWithS3 must carry Outposts: {}", reply.body))
            .clone()
    }

    /// The whole inventory of `region`, one page of the published ceiling.
    pub(crate) async fn inventory(&self, region: &str) -> Vec<Value> {
        self.inventory_in(region, "?maxResults=100").await
    }

    /// `s3-outposts:CreateEndpoint` on `outpost_id`, in `region`.
    pub(crate) async fn create_endpoint_in(
        &self,
        region: &str,
        outpost_id: &str,
        nth: u8,
    ) -> Reply {
        self.s3(
            region,
            "POST",
            "/S3Outposts/CreateEndpoint",
            Some(json!({
                "OutpostId": outpost_id,
                "SubnetId": format!("subnet-0a1b2c3d4e5f6071{nth}"),
                "SecurityGroupId": format!("sg-0a1b2c3d4e5f6071{nth}"),
            })),
        )
        .await
    }

    /// `s3-outposts:ListSharedEndpoints` scoped to `outpost_id`, in `region`.
    pub(crate) async fn shared_endpoints_in(&self, region: &str, outpost_id: &str) -> Reply {
        self.s3(
            region,
            "GET",
            &format!("/S3Outposts/ListSharedEndpoints?outpostId={outpost_id}&maxResults=100"),
            None,
        )
        .await
    }

    /// `s3-outposts:ListEndpoints` in `region`.
    pub(crate) async fn endpoints_in(&self, region: &str) -> Reply {
        self.s3(
            region,
            "GET",
            "/S3Outposts/ListEndpoints?maxResults=100",
            None,
        )
        .await
    }

    /// `s3-outposts:DeleteEndpoint` on the `(Outpost, endpoint)` pair, in `region`.
    pub(crate) async fn delete_endpoint_in(
        &self,
        region: &str,
        outpost_id: &str,
        endpoint_id: &str,
    ) -> Reply {
        self.s3(
            region,
            "DELETE",
            &format!(
                "/S3Outposts/DeleteEndpoint?endpointId={endpoint_id}&outpostId={outpost_id}"
            ),
            None,
        )
        .await
    }
}

/// Bring every install the orders placed in the co-hosted AWS Outposts control plane
/// owe due NOW.
///
/// An Outpost of that control plane holds no hardware, and reports the non-terminal
/// lifecycle status, until an order placed on it has delivered hardware and that
/// hardware has been installed — the steps of which fall due over spans far longer than
/// a case may sleep for. So every order is placed over the wire, exactly as a caller
/// places one, and only the instants its steps fall due at are moved, through the same
/// store seam this suite reaches that control plane through. The next call materialises
/// what the clock then owes. An order nothing is owed on — a cancelled one — is left
/// exactly as it is.
pub(crate) fn deliver_orders(emulator: &Emulator) {
    let store = emulator.outposts_store();
    let mut store = store.lock();
    let (due, at) = (std::time::Instant::now(), std::time::SystemTime::now());
    for record in store.orders.values_mut() {
        for transition in record.pending.iter_mut() {
            transition.due = due;
            transition.at = at;
        }
    }
}

/// Which catalog configuration a line item of a case names.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Item {
    /// The nth configuration that supports S3 storage, in the order the `outposts`
    /// catalog publishes them.
    S3(usize),
    /// A published configuration that supports no S3 storage.
    EbsOnly,
    /// A well-formed SKU the catalog publishes nothing under.
    Unpublished,
}

/// The line items of ONE order a case places: which configuration, and how many units
/// of it.
type OrderItems = Vec<(Item, i64)>;

/// Every order a case places on one Outpost.
type Orders = Vec<OrderItems>;

/// One row of the table that asserts what an Outpost with no ordered S3 capacity does:
/// the case, the orders placed on it, and whether each is cancelled again.
type UnprovisionedRow = (&'static str, Orders, bool);

/// The orderable configurations, as a client discovers them: through
/// `outposts:ListCatalogItems`, filtered by the storage option each supports.
pub(crate) struct Catalog {
    /// The SKUs of the configurations that support S3 storage, in catalog order.
    s3: Vec<String>,
    /// The SKU of one published configuration that supports no S3 storage.
    ebs_only: String,
    /// The S3 capacity, in bytes, this emulator's AWS Outposts catalog publishes for
    /// each SKU. Reference data read from the same catalog `ListCatalogItems`
    /// publishes, so no capacity figure is written down in this suite.
    published: BTreeMap<String, i64>,
}

impl Catalog {
    /// Discover the catalog over the wire.
    pub(crate) async fn discover(emulator: &Emulator) -> Self {
        let s3 = catalog_skus(emulator, "S3").await;
        assert!(
            s3.len() >= 2,
            "the catalog must publish at least two S3-capable configurations, got {s3:?}"
        );
        let ebs_only = catalog_skus(emulator, "EBS")
            .await
            .into_iter()
            .find(|sku| !s3.contains(sku))
            .expect("the catalog publishes a configuration that supports no S3 storage");
        let published: BTreeMap<String, i64> = outposts_state::catalog_item_catalog()
            .into_iter()
            .filter_map(|record| {
                Some((record.item.catalog_item_id.clone()?, record.s3_capacity_bytes))
            })
            .collect();
        assert!(
            !published.contains_key(UNPUBLISHED_SKU),
            "{UNPUBLISHED_SKU} must name no published configuration, got {:?}",
            published.get(UNPUBLISHED_SKU)
        );
        for sku in &s3 {
            assert!(
                published.get(sku).copied().unwrap_or_default() > 0,
                "case {sku:?}: a configuration that supports S3 storage carries S3 capacity, \
                 got {:?}",
                published.get(sku)
            );
        }
        assert_eq!(
            published.get(&ebs_only).copied(),
            Some(0),
            "case {ebs_only:?}: a configuration supporting no S3 storage carries no S3 \
             capacity, got {:?}",
            published.get(&ebs_only)
        );
        Self {
            s3,
            ebs_only,
            published,
        }
    }

    /// The SKU `item` names.
    fn sku(&self, item: Item) -> String {
        match item {
            Item::S3(index) => self.s3[index % self.s3.len()].clone(),
            Item::EbsOnly => self.ebs_only.clone(),
            Item::Unpublished => UNPUBLISHED_SKU.to_string(),
        }
    }

    /// The S3 capacity, in bytes, the catalog publishes for one unit of `item`.
    fn capacity(&self, item: Item) -> i64 {
        self.published
            .get(&self.sku(item))
            .copied()
            .unwrap_or_default()
    }

    /// One order's line items, as `(SKU, quantity)` pairs.
    pub(crate) fn line_items(&self, items: &[(Item, i64)]) -> Vec<(String, i64)> {        items
            .iter()
            .map(|(item, quantity)| (self.sku(*item), *quantity))
            .collect()
    }

    /// The S3 capacity those orders provision: the published capacity of each line
    /// item's configuration times the quantity ordered, summed.
    fn provisioned(&self, orders: &[OrderItems]) -> i64 {
        orders
            .iter()
            .flatten()
            .map(|(item, quantity)| self.capacity(*item) * quantity)
            .sum()
    }
}

/// The SKUs `ListCatalogItems` reports for one storage option.
async fn catalog_skus(emulator: &Emulator, storage: &str) -> Vec<String> {
    let body = emulator
        .outposts_ok(
            REGION,
            "GET",
            &format!("/catalog/items?SupportedStorageFilter={storage}&MaxResults=1000"),
            None,
        )
        .await;
    body["CatalogItems"]
        .as_array()
        .unwrap_or_else(|| panic!("ListCatalogItems must carry CatalogItems: {body}"))
        .iter()
        .map(|item| string_at(item, "CatalogItemId"))
        .collect()
}

/// The string member `name` of `value`, or a panic naming what was there instead.
pub(crate) fn string_at(value: &Value, name: &str) -> String {
    value[name]
        .as_str()
        .unwrap_or_else(|| panic!("{name} must be a string, got {value}"))
        .to_string()
}

/// The `OutpostId`s an inventory reports, in the order it reported them.
pub(crate) fn reported_ids(outposts: &[Value]) -> Vec<String> {
    outposts
        .iter()
        .map(|outpost| string_at(outpost, "OutpostId"))
        .collect()
}

/// The one Outpost an inventory of exactly one record reports.
fn only_outpost(case: &str, outposts: &[Value]) -> Value {
    assert_eq!(
        outposts.len(),
        1,
        "case {case:?}: expected exactly one Outpost in the inventory, got {outposts:?}"
    );
    outposts[0].clone()
}

/// Rule 1: an Outpost ordered S3 storage capacity through the `outposts` API is
/// reported by `ListOutpostsWithS3`, with the capacity those orders bought.
///
/// The table varies what the rule generalises over: which configuration is ordered, how
/// many units of it, how many line items one order carries, how many orders the Outpost
/// carries, and line items that provision no S3 capacity at all beside ones that do.
#[tokio::test]
async fn an_outpost_is_reported_with_the_s3_capacity_its_orders_provisioned() {
    let catalog = Catalog::discover(&Emulator::new()).await;
    let account = engine::emulator_account_id().to_string();
    let every_s3_configuration: OrderItems =
        (0..catalog.s3.len()).map(|index| (Item::S3(index), 1)).collect();

    let rows: Vec<(&str, Orders)> = vec![
        (
            "one unit of the first S3-capable configuration",
            vec![vec![(Item::S3(0), 1)]],
        ),
        (
            "four units of one S3-capable configuration",
            vec![vec![(Item::S3(1), 4)]],
        ),
        (
            "one unit of every S3-capable configuration, in one order",
            vec![every_s3_configuration],
        ),
        (
            "two separate orders on the same Outpost",
            vec![vec![(Item::S3(0), 2)], vec![(Item::S3(1), 3)]],
        ),
        (
            "an S3-capable line item beside an EBS-only one",
            vec![vec![(Item::S3(1), 1), (Item::EbsOnly, 5)]],
        ),
        (
            "an S3-capable line item beside one naming an unpublished SKU",
            vec![vec![(Item::S3(0), 1), (Item::Unpublished, 2)]],
        ),
    ];

    for (case, orders) in &rows {
        let emulator = Emulator::new();
        let outpost_id = emulator.an_outpost_in(REGION, "ordered").await;
        for order in orders {
            emulator
                .an_order_in(REGION, &outpost_id, &catalog.line_items(order))
                .await;
        }
        deliver_orders(&emulator);

        let reported = only_outpost(case, &emulator.inventory(REGION).await);
        let expected = catalog.provisioned(orders);
        assert!(
            expected > 0,
            "case {case:?}: the table must order some S3 capacity, got {expected}"
        );
        assert_eq!(
            string_at(&reported, "OutpostId"),
            outpost_id,
            "case {case:?}: the inventory reports the Outpost the control plane holds, got \
             {reported}"
        );
        assert_eq!(
            string_at(&reported, "OwnerId"),
            account,
            "case {case:?}: the Outpost is owned by the account that created it, got {reported}"
        );
        assert_eq!(
            string_at(&reported, "OutpostArn"),
            format!("arn:aws:outposts:{REGION}:{account}:outpost/{outpost_id}"),
            "case {case:?}: OutpostArn is in the AWS Outposts ARN namespace, got {reported}"
        );
        assert_eq!(
            string_at(&reported, "S3OutpostArn"),
            format!("arn:aws:s3-outposts:{REGION}:{account}:outpost/{outpost_id}/s3"),
            "case {case:?}: S3OutpostArn names the S3 capacity on that Outpost, got {reported}"
        );
        assert_eq!(
            reported["CapacityInBytes"].as_i64(),
            Some(expected),
            "case {case:?}: CapacityInBytes is the capacity the orders provisioned, got \
             {reported}"
        );

        // And the endpoint lifecycle is reachable on it: the Outpost the inventory
        // reports is one `CreateEndpoint` resolves.
        let created = emulator.create_endpoint_in(REGION, &outpost_id, 0).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateEndpoint on a provisioned Outpost is served, got {} {}",
            created.status, created.body
        );
    }
}

/// Rule 1, the other direction, and rule 2: an Outpost the orders on it bought no S3
/// capacity for is in no inventory, and every reference to it answers the modelled 404.
#[tokio::test]
async fn an_outpost_without_ordered_s3_capacity_is_reported_and_resolved_nowhere() {
    let catalog = Catalog::discover(&Emulator::new()).await;

    // Each row is the orders placed on the Outpost; none of them buys S3 capacity.
    let rows: Vec<UnprovisionedRow> = vec![
        ("no order at all", vec![], false),
        (
            "one order of an EBS-only configuration",
            vec![vec![(Item::EbsOnly, 2)]],
            false,
        ),
        (
            "one order naming a SKU the catalog publishes nothing under",
            vec![vec![(Item::Unpublished, 3)]],
            false,
        ),
        (
            "the only S3-capable order cancelled again",
            vec![vec![(Item::S3(0), 1)]],
            true,
        ),
    ];

    for (case, orders, cancelled) in &rows {
        let emulator = Emulator::new();
        let outpost_id = emulator.an_outpost_in(REGION, "unprovisioned").await;
        for order in orders {
            let order_id = emulator
                .an_order_in(REGION, &outpost_id, &catalog.line_items(order))
                .await;
            if *cancelled {
                emulator.cancel_order_in(REGION, &order_id).await;
            }
        }
        deliver_orders(&emulator);

        assert!(
            emulator.inventory(REGION).await.is_empty(),
            "case {case:?}: an Outpost with no ordered S3 capacity is in no inventory, got {:?}",
            emulator.inventory(REGION).await
        );

        // Every door into this service's endpoint lifecycle resolves the Outpost first,
        // so each one answers the service's single not-found text.
        let refusals = vec![
            (
                "CreateEndpoint",
                emulator.create_endpoint_in(REGION, &outpost_id, 1).await,
            ),
            (
                "ListSharedEndpoints",
                emulator.shared_endpoints_in(REGION, &outpost_id).await,
            ),
            (
                "DeleteEndpoint",
                emulator
                    .delete_endpoint_in(REGION, &outpost_id, "0000000000000000001")
                    .await,
            ),
        ];
        for (operation, reply) in refusals {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: {operation} on an Outpost with no S3 capacity is the modelled \
                 404, got {} {} {}",
                reply.status,
                reply.error_code(),
                reply.body
            );
            assert_eq!(
                reply.message(),
                OUTPOST_NOT_FOUND_MESSAGE,
                "case {case:?}: {operation} carries this service's not-found text, got {}",
                reply.body
            );
        }
    }
}

/// Rule 1: an Outpost still provisioning, or already being decommissioned, is not one
/// `ListOutpostsWithS3` reports — "outposts that are Active (not pending any
/// provisioning nor decommissioned)".
#[tokio::test]
async fn only_an_active_outpost_that_is_not_being_decommissioned_is_reported() {
    let catalog = Catalog::discover(&Emulator::new()).await;
    let emulator = Emulator::new();
    let outpost_id = emulator.an_outpost_in(REGION, "provisioning").await;
    emulator
        .an_order_in(
            REGION,
            &outpost_id,
            &catalog.line_items(&[(Item::S3(0), 1)]),
        )
        .await;

    // Nothing has been installed on the Outpost yet, so it has no S3 capacity to
    // report and nothing resolves it.
    assert!(
        emulator.inventory(REGION).await.is_empty(),
        "an Outpost that is still provisioning is in no inventory, got {:?}",
        emulator.inventory(REGION).await
    );
    let early = emulator.create_endpoint_in(REGION, &outpost_id, 2).await;
    assert_eq!(
        early.status, 404,
        "CreateEndpoint on an Outpost that is still provisioning is refused, got {} {}",
        early.status, early.body
    );

    deliver_orders(&emulator);
    assert_eq!(
        reported_ids(&emulator.inventory(REGION).await),
        vec![outpost_id.clone()],
        "once the Outpost is active its ordered S3 capacity is reported"
    );

    // A decommission of an Outpost that still carries an order is blocked by that
    // order, so the request is recorded through the control plane's own store — the one
    // state this case cannot reach by calling the API.
    {
        let store = emulator.outposts_store();
        let mut store = store.lock();
        let record = store
            .outpost_mut(REGION, &outpost_id)
            .expect("the control plane holds the Outpost it created");
        record.decommission_requested_at = Some(std::time::SystemTime::now());
    }
    assert!(
        emulator.inventory(REGION).await.is_empty(),
        "an Outpost being decommissioned is in no inventory, got {:?}",
        emulator.inventory(REGION).await
    );
}

/// Rule 3: the whole endpoint lifecycle runs on an Outpost provisioned purely through
/// AWS API calls, and a created endpoint reads back through this account's own listing
/// before a delete makes it observably gone.
#[tokio::test]
async fn the_endpoint_lifecycle_runs_on_an_outpost_the_control_plane_provisioned() {
    let catalog = Catalog::discover(&Emulator::new()).await;
    let emulator = Emulator::new();
    let outpost_id = emulator.an_outpost_in(REGION, "endpoints").await;
    emulator
        .an_order_in(
            REGION,
            &outpost_id,
            &catalog.line_items(&[(Item::S3(1), 2)]),
        )
        .await;
    deliver_orders(&emulator);

    let created = emulator.create_endpoint_in(REGION, &outpost_id, 3).await;
    assert_eq!(
        created.status, 200,
        "CreateEndpoint on the provisioned Outpost is served, got {} {}",
        created.status, created.body
    );
    let arn = string_at(&created.body, "EndpointArn");
    let account = engine::emulator_account_id().to_string();
    let endpoint_id = arn
        .rsplit('/')
        .next()
        .expect("an EndpointArn ends in the endpoint id")
        .to_string();
    assert_eq!(
        arn,
        format!(
            "arn:aws:s3-outposts:{REGION}:{account}:outpost/{outpost_id}/endpoint/{endpoint_id}"
        ),
        "the endpoint's ARN names the Outpost it is on, in that Outpost's own Region"
    );

    // Read back through this account's own listing: the Outpost was provisioned through
    // this account's control plane, so it is one the account OWNS, and the Outpost-scoped
    // listing — which serves the Outposts another account shared with this caller — does
    // not report endpoints for it (`shared_endpoint_listing.rs`).
    let reply = emulator.endpoints_in(REGION).await;
    assert_eq!(
        reply.status, 200,
        "case \"ListEndpoints\": the listing is served, got {} {}",
        reply.status, reply.body
    );
    let arns: Vec<String> = reply.body["Endpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"ListEndpoints\": must carry Endpoints: {}", reply.body))
        .iter()
        .map(|endpoint| string_at(endpoint, "EndpointArn"))
        .collect();
    assert_eq!(
        arns,
        vec![arn.clone()],
        "case \"ListEndpoints\": the created endpoint is the one reported"
    );
    let refused = emulator.shared_endpoints_in(REGION, &outpost_id).await;
    assert_eq!(
        refused.status, 404,
        "case \"ListSharedEndpoints\": an Outpost this account owns is not one it serves, \
         got {} {}",
        refused.status, refused.body
    );

    let deleted = emulator
        .delete_endpoint_in(REGION, &outpost_id, &endpoint_id)
        .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteEndpoint on the endpoint just created is served, got {} {}",
        deleted.status, deleted.body
    );
    // A delete is not finished when it answers: the endpoint carries the transient
    // status until the clock owes its removal, which the wait below lets fall due.
    std::thread::sleep(
        service_com_amazonaws_s3outposts_s3outposts::state::ENDPOINT_TRANSITION
            + Duration::from_millis(400),
    );
    let after = emulator.endpoints_in(REGION).await;
    assert_eq!(
        after.body["Endpoints"].as_array().map(Vec::len),
        Some(0),
        "a deleted endpoint is observably gone, got {}",
        after.body
    );
}

/// Rule 3: an Outpost is reported only through the Region it was created in, and every
/// ARN naming it is in that Region's own partition.
#[tokio::test]
async fn each_region_reports_only_the_outposts_provisioned_in_it() {
    let catalog = Catalog::discover(&Emulator::new()).await;
    let emulator = Emulator::new();
    let account = engine::emulator_account_id().to_string();

    // One Region per partition prefix the ARN members publish, so the partition is
    // exercised as a function of the Region rather than a constant.
    let regions = [
        ("us-east-1", "aws"),
        ("eu-west-1", "aws"),
        ("us-gov-west-1", "aws-us-gov"),
        ("cn-north-1", "aws-cn"),
    ];
    let mut placed: Vec<(&str, &str, String)> = Vec::new();
    for (region, partition) in regions {
        let outpost_id = emulator.an_outpost_in(region, "regional").await;
        emulator
            .an_order_in(region, &outpost_id, &catalog.line_items(&[(Item::S3(0), 1)]))
            .await;
        placed.push((region, partition, outpost_id));
    }
    deliver_orders(&emulator);

    for (region, partition, outpost_id) in &placed {
        let case = format!("{region} ({partition})");
        let reported = only_outpost(&case, &emulator.inventory(region).await);
        assert_eq!(
            string_at(&reported, "OutpostId"),
            *outpost_id,
            "case {case:?}: a Region reports its own Outpost and no other, got {reported}"
        );
        assert_eq!(
            string_at(&reported, "OutpostArn"),
            format!("arn:{partition}:outposts:{region}:{account}:outpost/{outpost_id}"),
            "case {case:?}: OutpostArn is in the partition its Region belongs to, got {reported}"
        );
        assert_eq!(
            string_at(&reported, "S3OutpostArn"),
            format!("arn:{partition}:s3-outposts:{region}:{account}:outpost/{outpost_id}/s3"),
            "case {case:?}: S3OutpostArn is in the same partition, got {reported}"
        );
    }

    // A Region nothing was provisioned in reports nothing at all.
    assert!(
        emulator.inventory("ap-south-1").await.is_empty(),
        "a Region with no provisioned Outpost reports an empty inventory, got {:?}",
        emulator.inventory("ap-south-1").await
    );
}

/// Rule 4: an Outpost another account shares with this one is reported beside the
/// control plane's own, each under its owner's account — and the listing pages over the
/// two together, for any page size.
#[tokio::test]
async fn a_shared_outpost_is_reported_beside_the_accounts_own() {
    let catalog = Catalog::discover(&Emulator::new()).await;
    let emulator = Emulator::new();
    let account = engine::emulator_account_id().to_string();

    // Two Outposts of this account, provisioned through the API.
    let mut own: Vec<String> = Vec::new();
    for (index, name) in ["own-a", "own-b"].into_iter().enumerate() {
        let outpost_id = emulator.an_outpost_in(REGION, name).await;
        emulator
            .an_order_in(
                REGION,
                &outpost_id,
                &catalog.line_items(&[(Item::S3(index), 1)]),
            )
            .await;
        own.push(outpost_id);
    }
    deliver_orders(&emulator);

    // And one of ANOTHER account, shared through Resource Access Manager. No AWS API
    // of this emulator shares an Outpost, so it is declared through the store seam the
    // operator's own setting reaches.
    let shared_id = "op-2c3d4e5f607182930";
    let shared_capacity = 24_000_000_000_000_i64;
    {
        let store = emulator.s3_store();
        let mut store = store.lock();
        store.provision_outpost(
            OutpostRecord::provisioned(shared_id, SHARING_ACCOUNT_ID, shared_capacity, REGION)
                .expect("the shared Outpost is a provisionable record"),
        );
    }

    let reported = emulator.inventory(REGION).await;
    let owners: BTreeMap<String, String> = reported
        .iter()
        .map(|outpost| {
            (
                string_at(outpost, "OutpostId"),
                string_at(outpost, "OwnerId"),
            )
        })
        .collect();
    for outpost_id in &own {
        assert_eq!(
            owners.get(outpost_id).map(String::as_str),
            Some(account.as_str()),
            "the Outposts this account created are reported under its own account, got {owners:?}"
        );
    }
    assert_eq!(
        owners.get(shared_id).map(String::as_str),
        Some(SHARING_ACCOUNT_ID),
        "a shared Outpost keeps the owner's account, got {owners:?}"
    );
    assert_eq!(
        reported
            .iter()
            .find(|outpost| string_at(outpost, "OutpostId") == shared_id)
            .and_then(|outpost| outpost["CapacityInBytes"].as_i64()),
        Some(shared_capacity),
        "a shared Outpost keeps the capacity it was shared with, got {reported:?}"
    );

    // The merged inventory is ONE listing: every page size walks the same set, in one
    // stable order, with a cursor that resumes where the page stopped.
    let whole = reported_ids(&reported);
    let mut sorted = whole.clone();
    sorted.sort();
    assert_eq!(
        whole, sorted,
        "the inventory is reported in one stable OutpostId order, got {whole:?}"
    );
    for page_size in [1_usize, 2, 3, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let query = match &cursor {
                None => format!("?maxResults={page_size}"),
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
            };
            let reply = emulator
                .s3(
                    REGION,
                    "GET",
                    &format!("/S3Outposts/ListOutpostsWithS3{query}"),
                    None,
                )
                .await;
            assert_eq!(
                reply.status, 200,
                "case page size {page_size}: the page is served, got {} {}",
                reply.status, reply.body
            );
            let page = reply.body["Outposts"]
                .as_array()
                .unwrap_or_else(|| panic!("a page must carry Outposts: {}", reply.body))
                .clone();
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page holds at most that many, got {}",
                page.len()
            );
            walked.extend(reported_ids(&page));
            cursor = reply.body["NextToken"].as_str().map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case page size {page_size}: paging walks the whole merged inventory exactly once"
        );
        assert_eq!(
            walked.iter().collect::<BTreeSet<_>>().len(),
            walked.len(),
            "case page size {page_size}: no Outpost is served twice, got {walked:?}"
        );
    }
}
