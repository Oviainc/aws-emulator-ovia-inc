// SPDX-License-Identifier: CC0-1.0

//! ONE rule — walk the orders placed on an Outpost that were not cancelled and, for
//! each of their line items, take the figure the published catalog carries for the
//! configuration it names times the quantity of it ordered — stands behind every report
//! of what an Outpost's orders bought, across BOTH emulated services.
//!
//! Three reports are computed from that one walk, and this suite drives each of them
//! over the real front door (`MultiServiceRouter::dispatch`, through the harness
//! [`crate::control_plane_inventory`] stands up), on the SAME orders:
//!
//! * `outposts:GetOutpostInstanceTypes` — the EC2 configuration the hardware those
//!   orders delivered arrived carrying: the largest size of each instance family the
//!   ordered configurations publish EC2 capacity of.
//! * `outposts:GetRenewalPricing` — the price of a renewal of the subscription those
//!   orders hold, which follows from the vCPUs of EC2 capacity they subscribe.
//! * `s3-outposts:ListOutpostsWithS3` — the Amazon S3 storage capacity, in bytes, those
//!   orders provisioned on the Outpost.
//!
//! The rule under test, stated once and then exercised over a SET of inputs: the three
//! reports answer the same walk over the same orders, each asked for the catalog figure
//! it is about, so an order none of them counts (a cancelled one) is counted by none of
//! them, a line item naming a configuration the catalog publishes nothing for
//! contributes to none of them, and the two capacity totals are each the published
//! figure times the quantity ordered — independently, so hardware that carries vCPUs
//! and no S3 storage is priced and provisions nothing.
//!
//! The rows differ along everything the rule generalises over: which configuration is
//! ordered, how many units of it, how many line items one order carries, how many
//! orders the Outpost holds, whether an order was cancelled, a configuration that
//! carries no S3 capacity, and a well-formed SKU the catalog publishes nothing under.
//! Every figure a row expects is computed from the published catalog, so no capacity,
//! price or instance type is written down here.

use serde_json::Value;

use service_com_amazonaws_outposts_outpostsolafservice::state as outposts_state;
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    CURRENCY_USD, PAYMENT_OPTION_UPFRONT_SHARE, PAYMENT_TERM_MONTHS, PRICING_RESULT_PRICED,
    PRICING_RESULT_UNABLE_TO_PRICE, QUOTE_PRICING_TYPE_SUBSCRIPTION,
    SUBSCRIPTION_PRICE_PER_VCPU_MONTH,
};

use crate::control_plane_inventory::{deliver_orders, string_at, Emulator, REGION};

/// A `SkuCode` of the modelled form (`^OR-[A-Z0-9]{7}$`) that this emulator's catalog
/// publishes no configuration under — checked against the published catalog by
/// [`Catalog::publish`] rather than assumed.
const UNPUBLISHED_SKU: &str = "OR-0000002";

/// How close a reported price has to be to the one the rule computes: the cent it is
/// quoted to, which is also the precision the service rounds to.
const CENT: f64 = 0.01;

/// What the published catalog carries for one configuration an order can be placed for:
/// the two capacity figures the reports are computed from, and the largest size of each
/// instance family its EC2 capacity covers, in the order the catalog publishes them.
#[derive(Debug, Clone)]
struct Configuration {
    /// The `CatalogItemId` a line item names it by.
    sku: String,
    /// The vCPUs of EC2 capacity one unit of it carries.
    vcpus: i64,
    /// The Amazon S3 storage capacity, in bytes, one unit of it carries.
    s3_bytes: i64,
    /// The largest size of each instance family it publishes EC2 capacity of, as the
    /// INSTANCE TYPE it names: a published capacity reports its family and the bare
    /// size separately, so the type is the join of the two
    /// ([`outposts_state::capacity_instance_type`]).
    max_sizes: Vec<String>,
}

/// Which published configuration a line item of a row names.
#[derive(Debug, Clone, Copy)]
enum Item {
    /// The nth configuration that carries S3 storage capacity, in catalog order.
    S3(usize),
    /// A published configuration that carries no S3 storage capacity at all.
    NoS3,
    /// A well-formed SKU the catalog publishes nothing under.
    Unpublished,
}

/// One order a row places on its Outpost: its line items and whether it is cancelled
/// again after being placed.
#[derive(Debug, Clone)]
struct Placed {
    items: Vec<(Item, i64)>,
    cancelled: bool,
}

/// An order of `items`, left live.
fn live(items: &[(Item, i64)]) -> Placed {
    Placed {
        items: items.to_vec(),
        cancelled: false,
    }
}

/// The same order, cancelled again after it was placed.
fn cancelled(items: &[(Item, i64)]) -> Placed {
    Placed {
        items: items.to_vec(),
        cancelled: true,
    }
}

/// The configurations an order can be placed for, as this service's own control plane
/// publishes them.
///
/// Reference data read off the AWS Outposts catalog itself — the same records
/// `outposts:ListCatalogItems` publishes — so what a row expects follows from the
/// published catalog rather than from a figure written down in this suite. Narrowed to
/// the configurations the catalog still OFFERS, because `outposts:CreateOrder` takes an
/// order only for one of those.
struct Catalog {
    /// The configurations that carry S3 storage capacity, in catalog order.
    s3: Vec<Configuration>,
    /// One published configuration that carries no S3 storage capacity.
    no_s3: Configuration,
}

impl Catalog {
    fn publish() -> Self {
        let published: Vec<Configuration> = outposts_state::catalog_item_catalog()
            .into_iter()
            .filter(|record| record.is_orderable())
            .filter_map(|record| {
                Some(Configuration {
                    sku: record.item.catalog_item_id.clone()?,
                    vcpus: i64::from(record.capacity_vcpus),
                    s3_bytes: record.s3_capacity_bytes,
                    max_sizes: record
                        .item
                        .ec2_capacities
                        .iter()
                        .flatten()
                        .filter_map(outposts_state::capacity_instance_type)
                        .collect(),
                })
            })
            .collect();
        assert!(
            !published.iter().any(|config| config.sku == UNPUBLISHED_SKU),
            "the rows naming {UNPUBLISHED_SKU:?} need a SKU the catalog publishes nothing \
             under, got {published:?}"
        );
        let s3: Vec<Configuration> = published
            .iter()
            .filter(|config| config.s3_bytes > 0)
            .cloned()
            .collect();
        assert!(
            s3.len() >= 2,
            "the catalog must publish at least two orderable configurations carrying S3 \
             capacity, got {s3:?}"
        );
        let no_s3 = published
            .iter()
            .find(|config| config.s3_bytes == 0 && config.vcpus > 0)
            .cloned()
            .expect("the catalog publishes an orderable configuration with no S3 capacity");
        for config in s3.iter().chain(std::iter::once(&no_s3)) {
            assert!(
                config.vcpus > 0 && !config.max_sizes.is_empty(),
                "case {:?}: a published configuration carries EC2 capacity, got {config:?}",
                config.sku
            );
        }
        Self { s3, no_s3 }
    }

    /// The configuration `item` names, or `None` for the unpublished SKU — which names
    /// none, so the catalog carries no capacity figure for it.
    fn configuration(&self, item: Item) -> Option<&Configuration> {
        match item {
            Item::S3(index) => Some(&self.s3[index % self.s3.len()]),
            Item::NoS3 => Some(&self.no_s3),
            Item::Unpublished => None,
        }
    }

    /// The `CatalogItemId` a line item naming `item` carries.
    fn sku(&self, item: Item) -> String {
        self.configuration(item)
            .map(|config| config.sku.clone())
            .unwrap_or_else(|| UNPUBLISHED_SKU.to_string())
    }

    /// One order's line items, as the `(SKU, quantity)` pairs `CreateOrder` takes.
    fn line_items(&self, order: &Placed) -> Vec<(String, i64)> {
        order
            .items
            .iter()
            .map(|(item, quantity)| (self.sku(*item), *quantity))
            .collect()
    }

    /// The rule, over the orders a row places: for every order it did not cancel, the
    /// figure `published` carries for each line item's configuration times the quantity
    /// of it ordered, totalled. A line item naming a configuration the catalog
    /// publishes that figure none of carries nothing.
    fn ordered_capacity(&self, orders: &[Placed], published: fn(&Configuration) -> i64) -> i64 {
        orders
            .iter()
            .filter(|order| !order.cancelled)
            .flat_map(|order| order.items.iter())
            .map(|(item, quantity)| {
                self.configuration(*item)
                    .map_or(0, |config| published(config) * quantity)
            })
            .sum()
    }

    /// The same walk read for the EC2 configuration it delivers: the largest size of
    /// each instance family the ordered configurations publish capacity of, each once,
    /// in the order the orders and their line items name them.
    fn ordered_instance_types(&self, orders: &[Placed]) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for order in orders.iter().filter(|order| !order.cancelled) {
            for (item, _) in &order.items {
                for max_size in self
                    .configuration(*item)
                    .map(|config| config.max_sizes.as_slice())
                    .unwrap_or_default()
                {
                    if !names.contains(max_size) {
                        names.push(max_size.clone());
                    }
                }
            }
        }
        names
    }
}

/// How many months a term runs for, from the rule the service prices by.
fn months(payment_term: &str) -> f64 {
    let (_, months) = PAYMENT_TERM_MONTHS
        .iter()
        .find(|(term, _)| *term == payment_term)
        .unwrap_or_else(|| panic!("the rule gives {payment_term} a length in months"));
    f64::from(*months)
}

/// The share of a term's cost a payment option pays up front, from the same rule.
fn upfront_share(payment_option: &str) -> f64 {
    let (_, share) = PAYMENT_OPTION_UPFRONT_SHARE
        .iter()
        .find(|(option, _)| *option == payment_option)
        .unwrap_or_else(|| panic!("the rule gives {payment_option} an upfront share"));
    f64::from(*share)
}

/// A price member of a reported option, as a number — `None` when the wire omits it.
fn price(details: &Value, member: &str) -> Option<f64> {
    details.get(member).and_then(Value::as_f64)
}

/// Require `actual` to be `expected` to the cent.
fn assert_price(actual: Option<f64>, expected: f64, member: &str, case: &str) {
    let actual =
        actual.unwrap_or_else(|| panic!("case {case:?}: expected {member} {expected}, got none"));
    assert!(
        (actual - expected).abs() <= CENT.max(expected.abs() * 1e-6),
        "case {case:?}: expected {member} {expected}, got {actual}"
    );
}

/// `outposts:GetOutpostInstanceTypes` on `outpost_id`, as the instance types it
/// reported, in the order it reported them.
async fn instance_types(emulator: &Emulator, outpost_id: &str, case: &str) -> Vec<String> {
    let body = emulator
        .outposts_ok(
            REGION,
            "GET",
            &format!("/outposts/{outpost_id}/instanceTypes?MaxResults=1000"),
            None,
        )
        .await;
    assert!(
        body.get("NextToken").is_none(),
        "case {case:?}: one page of the published ceiling holds every type, got {body}"
    );
    body["InstanceTypes"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected InstanceTypes, got {body}"))
        .iter()
        .map(|item| string_at(item, "InstanceType"))
        .collect()
}

/// `outposts:GetRenewalPricing` on `outpost_id`, checked against the price the
/// service's own published rule gives `capacity` vCPUs of subscribed hardware.
async fn assert_renewal_pricing(emulator: &Emulator, outpost_id: &str, capacity: i64, case: &str) {
    let body = emulator
        .outposts_ok(
            REGION,
            "GET",
            &format!("/outpost/{outpost_id}/renewal-pricing"),
            None,
        )
        .await;
    let reported = body["PricingOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected PricingOptions, got {body}"))
        .clone();
    if capacity == 0 {
        assert_eq!(
            body["PricingResult"].as_str(),
            Some(PRICING_RESULT_UNABLE_TO_PRICE),
            "case {case:?}: nothing is subscribed, so there is no renewal to price, got {body}"
        );
        assert!(
            reported.is_empty(),
            "case {case:?}: an Outpost with nothing subscribed reports no option, got {body}"
        );
        return;
    }
    assert_eq!(
        body["PricingResult"].as_str(),
        Some(PRICING_RESULT_PRICED),
        "case {case:?}: {capacity} vCPUs are subscribed, so the renewal is priced, got {body}"
    );
    assert!(
        !reported.is_empty(),
        "case {case:?}: a priced renewal reports the options it can be bought under, got {body}"
    );
    let monthly_rate = capacity as f64 * f64::from(SUBSCRIPTION_PRICE_PER_VCPU_MONTH);
    for option in &reported {
        assert_eq!(
            option.get("PricingType").and_then(Value::as_str),
            Some(QUOTE_PRICING_TYPE_SUBSCRIPTION),
            "case {case:?}: expected PricingType {QUOTE_PRICING_TYPE_SUBSCRIPTION:?}, got {option}"
        );
        let details = option
            .get("SubscriptionPricingDetails")
            .unwrap_or_else(|| panic!("case {case:?}: expected the pricing details, got {option}"));
        assert_eq!(
            details.get("Currency").and_then(Value::as_str),
            Some(CURRENCY_USD),
            "case {case:?}: expected Currency {CURRENCY_USD:?}, got {details}"
        );
        let payment_option = string_at(details, "PaymentOption");
        let payment_term = string_at(details, "PaymentTerm");
        let case = &format!("{case} ({payment_option} {payment_term})");
        let months = months(&payment_term);
        let share = upfront_share(&payment_option);
        assert_price(
            price(details, "UpfrontPrice"),
            monthly_rate * months * share,
            "UpfrontPrice",
            case,
        );
        assert_price(
            price(details, "MonthlyRecurringPrice"),
            monthly_rate * (1.0 - share),
            "MonthlyRecurringPrice",
            case,
        );
    }
}

/// `s3-outposts:ListOutpostsWithS3`, checked against the S3 capacity `capacity` says
/// the Outpost's orders provisioned: it is reported with that capacity when its orders
/// bought any, and reported nowhere when they bought none.
async fn assert_provisioned_s3(emulator: &Emulator, outpost_id: &str, capacity: i64, case: &str) {
    let inventory = emulator.inventory(REGION).await;
    let reported = inventory
        .iter()
        .find(|outpost| string_at(outpost, "OutpostId") == outpost_id);
    if capacity == 0 {
        assert!(
            reported.is_none(),
            "case {case:?}: an Outpost whose orders bought no S3 capacity is in no inventory, \
             got {inventory:?}"
        );
        return;
    }
    let reported = reported.unwrap_or_else(|| {
        panic!("case {case:?}: expected {outpost_id} in the inventory, got {inventory:?}")
    });
    assert_eq!(
        reported["CapacityInBytes"].as_i64(),
        Some(capacity),
        "case {case:?}: CapacityInBytes is the capacity the orders provisioned, got {reported}"
    );
}

/// The rule, over every row: the three reports are the one walk over the Outpost's
/// uncancelled orders, each asked for the catalog figure it is about.
#[tokio::test]
async fn the_three_reports_follow_one_walk_over_the_outposts_uncancelled_orders() {
    let catalog = Catalog::publish();
    let every_s3_configuration: Vec<(Item, i64)> = (0..catalog.s3.len())
        .map(|index| (Item::S3(index), 1))
        .collect();

    let rows: Vec<(&str, Vec<Placed>)> = vec![
        ("no order at all", vec![]),
        (
            "one unit of one configuration carrying S3 capacity",
            vec![live(&[(Item::S3(0), 1)])],
        ),
        (
            "four units of another configuration carrying S3 capacity",
            vec![live(&[(Item::S3(1), 4)])],
        ),
        (
            "two line items of one order",
            vec![live(&[(Item::S3(0), 1), (Item::S3(1), 2)])],
        ),
        (
            "one unit of every configuration carrying S3 capacity, in one order",
            vec![live(&every_s3_configuration)],
        ),
        (
            "two orders on the same Outpost",
            vec![live(&[(Item::S3(0), 2)]), live(&[(Item::S3(1), 3)])],
        ),
        (
            "a configuration carrying no S3 capacity beside one that does",
            vec![live(&[(Item::S3(0), 1), (Item::NoS3, 5)])],
        ),
        (
            "only a configuration carrying no S3 capacity",
            vec![live(&[(Item::NoS3, 2)])],
        ),
        (
            "a line item naming a SKU the catalog publishes nothing under",
            vec![live(&[(Item::Unpublished, 2), (Item::S3(1), 1)])],
        ),
        (
            "only a SKU the catalog publishes nothing under",
            vec![live(&[(Item::Unpublished, 3)])],
        ),
        (
            "the only order cancelled again",
            vec![cancelled(&[(Item::S3(0), 2)])],
        ),
        (
            "a cancelled order beside a live one",
            vec![
                cancelled(&[(Item::S3(1), 4)]),
                live(&[(Item::S3(0), 1), (Item::NoS3, 1)]),
            ],
        ),
    ];

    // How many rows each report really has something to answer for, so the table
    // cannot pass by expecting nothing anywhere.
    let (mut priced, mut provisioned, mut configured) = (0, 0, 0);
    for (case, orders) in &rows {
        let emulator = Emulator::new();
        let outpost_id = emulator.an_outpost_in(REGION, "ordered").await;
        for order in orders {
            let order_id = emulator
                .an_order_in(REGION, &outpost_id, &catalog.line_items(order))
                .await;
            if order.cancelled {
                emulator.cancel_order_in(REGION, &order_id).await;
            }
        }
        // The hardware a live order bought is installed on the Outpost once the order's
        // own steps fall due, which is what puts an EC2 configuration on it.
        deliver_orders(&emulator);

        let vcpus = catalog.ordered_capacity(orders, |config| config.vcpus);
        let s3_bytes = catalog.ordered_capacity(orders, |config| config.s3_bytes);
        let types = catalog.ordered_instance_types(orders);
        priced += i32::from(vcpus > 0);
        provisioned += i32::from(s3_bytes > 0);
        configured += i32::from(!types.is_empty());
        assert_renewal_pricing(&emulator, &outpost_id, vcpus, case).await;
        assert_provisioned_s3(&emulator, &outpost_id, s3_bytes, case).await;
        assert_eq!(
            instance_types(&emulator, &outpost_id, case).await,
            types,
            "case {case:?}: expected the largest size of each family the ordered hardware \
             carries capacity of"
        );
    }
    for (report, rows_covered) in [
        ("GetRenewalPricing", priced),
        ("ListOutpostsWithS3", provisioned),
        ("GetOutpostInstanceTypes", configured),
    ] {
        assert!(
            rows_covered >= 2,
            "the table must exercise {report} on more than one row that has something to \
             report, got {rows_covered}"
        );
    }
}
