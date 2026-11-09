// SPDX-License-Identifier: CC0-1.0

//! Reserved nodes: the published offerings and their pricing, the terms and
//! offering types, the states a purchased node moves through and the
//! reserved-node exchange surface.

use super::*;

/// The currency every price in the reserved-node catalog is quoted in.
pub const RESERVED_NODE_CURRENCY_CODE: &str = "USD";

/// The `RecurringChargeFrequency` a reserved node offering's recurring charge is
/// applied at: once per hour of the term.
pub const RECURRING_CHARGE_FREQUENCY_HOURLY: &str = "Hourly";

/// Every term a reserved node offering is published for, as
/// `(duration in seconds, share of the on-demand rate the term is charged at)`,
/// shortest term first.
///
/// `Duration` is "The duration, in seconds, for which the offering will reserve
/// the node" (`smithy_query(query="shape", shape="ReservedNodeOffering")`), and
/// a term is one dimension of a reservation's configuration — "(term, payment
/// type, or number of nodes)" (`smithy_query(query="doc",
/// shape="AcceptReservedNodeExchange")`). The two terms are a one-year and a
/// three-year commitment of 365-day years, and the longer commitment is charged
/// at the lower share; both the terms and the shares are this emulator's own
/// published catalog, the model publishing no list of either.
pub const RESERVED_NODE_TERMS: [(i32, f64); 2] = [(31_536_000, 0.70), (94_608_000, 0.45)];

/// The payment option that charges nothing upfront: the whole term is charged
/// for every hour of it as the offering's recurring charge.
pub const NO_UPFRONT_OFFERING_TYPE: &str = "No Upfront";

/// The payment option that charges part of the term upfront and the rest for
/// every hour of it as the offering's recurring charge.
pub const PARTIAL_UPFRONT_OFFERING_TYPE: &str = "Partial Upfront";

/// The payment option that charges the whole term upfront, leaving a recurring
/// charge of nothing.
pub const ALL_UPFRONT_OFFERING_TYPE: &str = "All Upfront";

/// Every offering type a reserved node offering is published under, as
/// `(name, share of the term's cost charged upfront)`, in catalog order.
///
/// `OfferingType` is "The anticipated utilization of the reserved node, as
/// defined in the reserved node offering" (`smithy_query(query="shape",
/// shape="ReservedNodeOffering")`), and what the service publishes one offering
/// of each of is the PAYMENT OPTION that utilization is paid under: the whole
/// term upfront, part of it, or none of it. The utilization classes reserved
/// nodes were once published under are retired, so the catalog carries none of
/// them. The shares are this emulator's own published catalog; the model
/// publishes no value list for the member.
pub const RESERVED_NODE_OFFERING_TYPES: [(&str, f64); 3] = [
    (NO_UPFRONT_OFFERING_TYPE, 0.0),
    (PARTIAL_UPFRONT_OFFERING_TYPE, 0.5),
    (ALL_UPFRONT_OFFERING_TYPE, 1.0),
];

/// The modelled `ReservedNodeOfferingType` of an offering an existing
/// reservation can be exchanged onto (a value of
/// `RESERVED_NODE_OFFERING_TYPE_VALUES`).
pub const RESERVED_NODE_OFFERING_TYPE_UPGRADABLE: &str = "Upgradable";

/// The modelled `ReservedNodeOfferingType` of an offering that is only ever
/// purchased outright (a value of `RESERVED_NODE_OFFERING_TYPE_VALUES`).
pub const RESERVED_NODE_OFFERING_TYPE_REGULAR: &str = "Regular";

/// The node-type prefix of an offering an existing reservation can be exchanged
/// onto.
///
/// Source: `smithy_query(query="doc", shape="GetReservedNodeExchangeOfferings")`
/// — "Returns an array of DC2 ReservedNodeOfferings that matches the payment
/// type, term, and usage price of the given DC1 reserved node": the offerings an
/// exchange can target are the DC2 ones, so those are the node types the
/// EXCHANGE-TARGET catalog ([`published_exchange_reserved_node_offerings`])
/// publishes an offering of, each carrying
/// [`RESERVED_NODE_OFFERING_TYPE_UPGRADABLE`]. The DC2 family is named in one
/// place ([`PREVIOUS_GENERATION_NODE_TYPE_PREFIX`]), so the family this catalog
/// targets and the family the orderable catalog no longer offers a NEW cluster
/// cannot drift apart.
pub const EXCHANGE_TARGET_NODE_TYPE_PREFIX: &str = PREVIOUS_GENERATION_NODE_TYPE_PREFIX;

/// The hourly on-demand rate, in [`RESERVED_NODE_CURRENCY_CODE`], this emulator
/// publishes for one node of each offered node type — the rate every price in
/// that node type's offerings is derived from.
///
/// Listed in the order of [`ORDERABLE_NODE_TYPES`], and covering every node type
/// in it, so the catalog publishes an offering for every node type a cluster can
/// be created with. The rates rise with the size of the node and are this
/// emulator's own published catalog: the model publishes no price for any node
/// type.
pub(super) const NODE_TYPE_HOURLY_RATE: [(&str, f64); 10] = [
    ("dc2.large", 0.25),
    ("dc2.8xlarge", 4.80),
    ("rg.large", 0.36),
    ("rg.xlarge", 0.72),
    ("rg.4xlarge", 2.88),
    ("rg.12xlarge", 8.64),
    ("ra3.large", 0.52),
    ("ra3.xlplus", 1.09),
    ("ra3.4xlarge", 3.26),
    ("ra3.16xlarge", 13.04),
];

/// The hourly on-demand rate [`NODE_TYPE_HOURLY_RATE`] publishes for this node
/// type, if the catalog offers it.
pub fn node_type_hourly_rate(node_type: &str) -> Option<f64> {
    NODE_TYPE_HOURLY_RATE
        .into_iter()
        .find(|(offered, _)| *offered == node_type)
        .map(|(_, rate)| rate)
}

/// `value` rounded to `places` decimal places, so a published price carries only
/// the digits a currency amount has.
pub(super) fn round_to(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// The offering identifier of the one offering for this node type, term,
/// payment option and `ReservedNodeOfferingType`: a lowercase-hex 8-4-4-4-12
/// value.
///
/// Derived from the offering's OWN four dimensions, so the same offering always
/// carries the same id — the id a caller passes to `PurchaseReservedNodeOffering`
/// ("you then use the unique offering ID in you call to
/// PurchaseReservedNodeOffering", `smithy_query(query="doc",
/// operation="DescribeReservedNodeOfferings")`) names the same offering on every
/// read and is not a per-run value. The `ReservedNodeOfferingType` is one of the
/// four, so the exchange-target offering of a node type, term and payment option
/// is a DIFFERENT offering from the regular one a purchase buys, exactly as the
/// two catalogs publish them.
pub fn reserved_node_offering_id(
    node_type: &str,
    duration: i32,
    offering_type: &str,
    reserved_node_offering_type: &str,
) -> String {
    content_uuid(&format!(
        "reservednodeoffering/{node_type}/{duration}/{offering_type}/{reserved_node_offering_type}"
    ))
}

/// One published reserved node offering: what it reserves, for how long, under
/// which payment option, and how the term's cost is charged.
///
/// The whole term costs the node type's hourly rate ([`NODE_TYPE_HOURLY_RATE`])
/// at the term's share ([`RESERVED_NODE_TERMS`]) for every hour of the term, and
/// the payment option splits that cost in two: the share it charges upfront
/// becomes the `FixedPrice`, and whatever is left is charged for every hour of
/// the term — "regardless of whether you are creating any clusters using the node
/// offering" (`smithy_query(query="doc", shape="ReservedNodeOffering",
/// member="RecurringCharges")`) — as the offering's ONE recurring charge, at
/// [`RECURRING_CHARGE_FREQUENCY_HOURLY`]. Nothing is charged per hour the
/// cluster using the offering RUNS, so the `UsagePrice` of every published
/// offering is zero.
///
/// Every offering therefore carries a `RecurringCharges` STRUCTURE, whose amount
/// is zero for the option that charges the whole term upfront: a reader of
/// `RecurringCharges.RecurringCharge.RecurringChargeAmount` finds a charge on
/// every entry of the catalog rather than an empty member on some of them.
pub(super) fn reserved_node_offering(
    node_type: &str,
    duration: i32,
    offering_type: &str,
    reserved_node_offering_type: &str,
) -> ReservedNodeOffering {
    let hourly_rate = node_type_hourly_rate(node_type).unwrap_or_default();
    let term_share = RESERVED_NODE_TERMS
        .into_iter()
        .find(|(offered, _)| *offered == duration)
        .map(|(_, share)| share)
        .unwrap_or_default();
    let upfront_share = RESERVED_NODE_OFFERING_TYPES
        .into_iter()
        .find(|(offered, _)| *offered == offering_type)
        .map(|(_, share)| share)
        .unwrap_or_default();

    let hours = f64::from(duration) / 3600.0;
    let reserved_hourly_rate = hourly_rate * term_share;
    let fixed_price = round_to(reserved_hourly_rate * hours * upfront_share, 2);
    let hourly_remainder = round_to(reserved_hourly_rate * (1.0 - upfront_share), 3);

    ReservedNodeOffering {
        reserved_node_offering_id: Some(reserved_node_offering_id(
            node_type,
            duration,
            offering_type,
            reserved_node_offering_type,
        )),
        node_type: Some(node_type.to_string()),
        duration: Some(duration),
        fixed_price: Some(fixed_price),
        usage_price: Some(0.0),
        currency_code: Some(RESERVED_NODE_CURRENCY_CODE.to_string()),
        offering_type: Some(offering_type.to_string()),
        recurring_charges: Some(vec![RecurringCharge {
            recurring_charge_amount: Some(hourly_remainder),
            recurring_charge_frequency: Some(RECURRING_CHARGE_FREQUENCY_HOURLY.to_string()),
        }]),
        reserved_node_offering_type: Some(reserved_node_offering_type.to_string()),
    }
}

/// Every reserved node offering this service publishes, in the order
/// `DescribeReservedNodeOfferings` reports them: node type (as
/// [`ORDERABLE_NODE_TYPES`] offers them), then term (shortest first), then
/// offering type.
///
/// The catalog is the full product of those three dimensions — "a list of the
/// available reserved node offerings … including the node type, the fixed and
/// recurring costs of reserving the node and duration the node will be reserved
/// for you" (`smithy_query(query="doc",
/// operation="DescribeReservedNodeOfferings")`) — so a node type a cluster can be
/// created with can be reserved for either term under any payment option. Every
/// entry is a `Regular` offering: it is bought outright by
/// `PurchaseReservedNodeOffering`, while the offerings an existing reservation
/// is exchanged ONTO are a catalog of their own
/// ([`published_exchange_reserved_node_offerings`]). Seeded into
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::reserved_node_offerings`]
/// when the store is created, exactly like the engine-version and
/// maintenance-track catalogs: an offering exists before anybody reserves
/// anything, and no operation adds to or removes from the list.
pub fn published_reserved_node_offerings() -> Vec<ReservedNodeOffering> {
    offerings_of(
        ORDERABLE_NODE_TYPES.iter().copied(),
        RESERVED_NODE_OFFERING_TYPE_REGULAR,
    )
}

/// Every reserved node offering an existing reservation can be exchanged ONTO,
/// in the order `GetReservedNodeExchangeOfferings` reports them.
///
/// Source: `smithy_query(query="doc", operation="GetReservedNodeExchangeOfferings")`
/// — "Returns an array of DC2 ReservedNodeOfferings that matches the payment
/// type, term, and usage price of the given DC1 reserved node": an
/// exchange-target offering is published for each node type of the DC2 family
/// ([`EXCHANGE_TARGET_NODE_TYPE_PREFIX`]) under the same terms and payment
/// options as the purchase catalog, and each carries the modelled
/// [`RESERVED_NODE_OFFERING_TYPE_UPGRADABLE`] — which is what makes it a target
/// an exchange may move onto rather than a product a purchase buys.
pub fn published_exchange_reserved_node_offerings() -> Vec<ReservedNodeOffering> {
    offerings_of(
        ORDERABLE_NODE_TYPES
            .iter()
            .copied()
            .filter(|node_type| node_type.starts_with(EXCHANGE_TARGET_NODE_TYPE_PREFIX)),
        RESERVED_NODE_OFFERING_TYPE_UPGRADABLE,
    )
}

/// One catalog of offerings: every `node_types` entry the catalog prices, for
/// every term and under every payment option, published as
/// `reserved_node_offering_type`.
///
/// The ONE rule both published catalogs are built by, so the purchase catalog
/// and the exchange-target catalog cannot disagree about the terms, the payment
/// options or the prices of a node type.
fn offerings_of<I>(node_types: I, reserved_node_offering_type: &str) -> Vec<ReservedNodeOffering>
where
    I: IntoIterator<Item = &'static str>,
{
    let mut offerings = Vec::new();
    for node_type in node_types {
        if node_type_hourly_rate(node_type).is_none() {
            continue;
        }
        for (duration, _) in RESERVED_NODE_TERMS {
            for (offering_type, _) in RESERVED_NODE_OFFERING_TYPES {
                offerings.push(reserved_node_offering(
                    node_type,
                    duration,
                    offering_type,
                    reserved_node_offering_type,
                ));
            }
        }
    }
    offerings
}

/// The `State` a reservation a purchase just recorded reports: the sale is
/// approved, the payment is not confirmed yet.
///
/// Source: `smithy_query(query="doc", shape="ReservedNode", member="State")` —
/// "pending-payment-This reserved node has recently been purchased, and the sale
/// has been approved, but payment has not yet been confirmed."
pub const RESERVED_NODE_STATE_PENDING_PAYMENT: &str = "pending-payment";

/// The `State` a reservation reports once its payment is confirmed, from the
/// same documented value list as [`RESERVED_NODE_STATE_PENDING_PAYMENT`]:
/// "active-This reserved node is owned by the caller and is available for use."
pub const RESERVED_NODE_STATE_ACTIVE: &str = "active";

/// How long after the purchase that starts it the payment confirmation falls
/// due: a reservation reported [`RESERVED_NODE_STATE_PENDING_PAYMENT`] becomes
/// [`RESERVED_NODE_STATE_ACTIVE`] this long after the
/// `PurchaseReservedNodeOffering` call that recorded it.
///
/// The span is the emulator's own choice — Amazon Redshift documents no
/// confirmation duration — and is deliberately non-zero, for the same reason as
/// [`PROVISIONING_SPAN`]: a caller that reads the reservation straight after
/// buying it still observes the transient state the purchase reported. The due
/// instant is stored and nothing ever sleeps for it.
pub const RESERVED_NODE_PAYMENT_SPAN: Duration = Duration::from_millis(500);

/// How many nodes a purchase reserves when the request names no `NodeCount`.
///
/// Source: `smithy_query(query="doc",
/// shape="PurchaseReservedNodeOfferingMessage", member="NodeCount")` — "The
/// number of reserved nodes that you want to purchase. Default: 1".
pub const DEFAULT_RESERVED_NODE_COUNT: i32 = 1;

/// How many reserved compute nodes the store holds before
/// `PurchaseReservedNodeOffering` refuses with the modelled
/// `ReservedNodeQuotaExceededFault`.
///
/// The same reasoning as [`CLUSTER_QUOTA`]: Amazon Redshift publishes an
/// adjustable compute-node quota per account per Region, and this emulator
/// models neither Service Quotas nor an account's adjustments — so this is the
/// emulator's OWN bound on
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::reserved_nodes`]
/// rather than a claim about any account's limit. It bounds NODES rather than
/// reservations, because the fault it raises is about the "compute node quota"
/// and one reservation may hold many nodes.
pub const RESERVED_NODE_QUOTA: i32 = 10_000;

/// A fresh `ReservedNodeId`.
///
/// "The unique identifier for the reservation" (`smithy_query(query="shape",
/// shape="ReservedNode")`) is minted by the service and carries nothing from the
/// request — two purchases of the SAME offering are two reservations — so it is
/// drawn per call, exactly like [`new_usage_limit_id`], and the minted id is
/// STORED so every later read of that reservation reports the same one.
pub fn new_reserved_node_id() -> String {
    crate::service::query_error_envelope::new_request_id()
}

/// The reservation a purchase of `offering` for `node_count` nodes records.
///
/// ONE rule for what a reservation reports: it carries its offering's own
/// dimensions and per-node charges verbatim — node type, term, currency,
/// offering type, utilization class and prices are the offering's, which is what
/// makes a reservation and the offering it was bought from agree — and adds what
/// belongs to the reservation alone: the id this purchase minted for it, when its
/// term started, how many nodes it holds, and the state of its payment.
pub fn reserved_node_of_offering(
    reserved_node_id: String,
    offering: &ReservedNodeOffering,
    node_count: i32,
    start_time: ::aws_smithy_types::DateTime,
) -> ReservedNode {
    ReservedNode {
        reserved_node_id: Some(reserved_node_id),
        reserved_node_offering_id: offering.reserved_node_offering_id.clone(),
        node_type: offering.node_type.clone(),
        start_time: Some(start_time),
        duration: offering.duration,
        fixed_price: offering.fixed_price,
        usage_price: offering.usage_price,
        currency_code: offering.currency_code.clone(),
        node_count: Some(node_count),
        state: Some(RESERVED_NODE_STATE_PENDING_PAYMENT.to_string()),
        offering_type: offering.offering_type.clone(),
        recurring_charges: offering.recurring_charges.clone(),
        reserved_node_offering_type: offering.reserved_node_offering_type.clone(),
    }
}

/// What an operation answers when the value a request supplied on an
/// identifier-bearing member names nothing: the MEMBER the value arrived on,
/// that value, and the fixed tail ` not found.`.
///
/// The whole sentence is fixed apart from the value, and the member name is the
/// prefix — so the refusal says which parameter of the request it is about
/// rather than describing the resource in prose.
///
/// Which operations word their refusal this way is an attribute of the
/// OPERATION, exactly as it is for `ClusterNotFoundFault` (see
/// [`ClusterNotFoundWording`]): recorded refusals of
/// `DescribeReservedNodeOfferings`, `PurchaseReservedNodeOffering` and
/// `DescribeReservedNodes` show this sentence, while the reserved-node EXCHANGE
/// operations are recorded answering the resource-named sentence of
/// [`reserved_node_not_found_message`] instead. One helper per sentence, so no
/// operation can quietly inherit another one's.
pub fn member_value_not_found_message(member: &str, value: &str) -> String {
    format!("{member} {value} not found.")
}

/// What a reserved-node EXCHANGE operation naming a `ReservedNodeId` answers
/// when the account holds no reservation under it: the resource named in prose,
/// the identifier the request asked for, then ` not found.`.
///
/// Shared by every exchange operation that looks a reservation up, so a client
/// sees one message for one condition across that family. It is NOT the sentence
/// `DescribeReservedNodes` answers about the same member — see
/// [`member_value_not_found_message`] for why the two differ.
pub fn reserved_node_not_found_message(reserved_node_id: &str) -> String {
    format!("Reserved Node {reserved_node_id} not found.")
}

/// The input member a reservation is named through.
pub const MEMBER_RESERVED_NODE_ID: &str = "ReservedNodeId";

/// The input member a published offering is named through by the operations that
/// read or buy one (`DescribeReservedNodeOfferings`,
/// `PurchaseReservedNodeOffering`).
pub const MEMBER_RESERVED_NODE_OFFERING_ID: &str = "ReservedNodeOfferingId";

/// The input member an exchange request is named through.
pub const MEMBER_RESERVED_NODE_EXCHANGE_REQUEST_ID: &str = "ReservedNodeExchangeRequestId";

/// Whether `value` has the FORM of an identifier this service MINTS: five
/// lowercase-hex groups of 8-4-4-4-12 characters.
///
/// Every identifier this service issues for a reservation, for an offering and
/// for an exchange request has that form (see [`new_reserved_node_id`],
/// [`reserved_node_offering_id`] and [`new_reserved_node_exchange_request_id`]),
/// so a value of any other form names nothing this account could ever hold —
/// which is what lets the exchange operations rule it out as a bad parameter
/// value before any lookup runs.
///
/// A shape test over the form this crate already MINTS with its existing digest,
/// not a parser: it decodes nothing, keeps nothing and allocates nothing, which
/// is also why it needs no crate of its own (the generator that mints these ids
/// records why this crate takes no `uuid` dependency).
pub fn is_minted_identifier(value: &str) -> bool {
    /// The character count of each group, in order.
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

    let mut groups = value.split('-');
    for width in GROUPS {
        let Some(group) = groups.next() else {
            return false;
        };
        let hex = group
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if group.len() != width || !hex {
            return false;
        }
    }
    groups.next().is_none()
}

/// What an exchange operation answers an identifier whose FORM it ruled out
/// before any lookup: the member it was given on and the value that member
/// cannot take.
pub fn malformed_identifier_message(member: &str, value: &str) -> String {
    format!("Invalid value for {member}: '{value}'. {member} must be a universally unique identifier.")
}

/// The `State` a reservation reports while it is being exchanged for another.
///
/// Source: `smithy_query(query="doc", shape="ReservedNode", member="State")` —
/// "exchanging-The owner is exchanging the reserved node for another reserved
/// node", from the same documented value list as
/// [`RESERVED_NODE_STATE_PENDING_PAYMENT`].
pub const RESERVED_NODE_STATE_EXCHANGING: &str = "exchanging";

/// The `State` the reservation an exchange moved OFF reports once that exchange
/// is complete, from the same documented value list: "retired-The reserved node
/// is no longer available."
pub const RESERVED_NODE_STATE_RETIRED: &str = "retired";

/// The `Status` an exchange request reports from the call that asked for it.
///
/// Source: `smithy_query(query="doc", shape="ReservedNodeExchangeStatus",
/// member="Status")` — "The status of the reserved-node exchange request.
/// Statuses include in-progress and requested", a value of
/// `ReservedNodeExchangeStatusType`.
pub const RESERVED_NODE_EXCHANGE_STATUS_REQUESTED: &str = "REQUESTED";

/// The `Status` an exchange request reports once the exchange is complete, from
/// the same modelled value list as
/// [`RESERVED_NODE_EXCHANGE_STATUS_REQUESTED`].
pub const RESERVED_NODE_EXCHANGE_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// How long after the `AcceptReservedNodeExchange` call that asks for it an
/// exchange falls due: the request leaves
/// [`RESERVED_NODE_EXCHANGE_STATUS_REQUESTED`], the reservation it moved off
/// leaves [`RESERVED_NODE_STATE_EXCHANGING`] for
/// [`RESERVED_NODE_STATE_RETIRED`], and the reservation it minted leaves
/// [`RESERVED_NODE_STATE_PENDING_PAYMENT`] — one instant for all three, so no
/// caller can see the exchange half applied.
///
/// The span is the emulator's own choice — Amazon Redshift documents no exchange
/// duration — and is deliberately non-zero, for the same reason as
/// [`RESERVED_NODE_PAYMENT_SPAN`]: a caller that polls the exchange straight
/// after asking for it still observes the transient status the accept reported.
/// The due instant is stored and nothing ever sleeps for it.
pub const RESERVED_NODE_EXCHANGE_SPAN: Duration = Duration::from_millis(500);

/// The node-type prefix of a cluster or snapshot whose reserved nodes
/// reserved-node exchange covers: Amazon Redshift exchanges DS2 reservations
/// only, which is what [`ONLY_DS2_EXCHANGEABLE`] says to every other node type.
pub const EXCHANGEABLE_CLUSTER_NODE_TYPE_PREFIX: &str = "ds2.";

/// What a cluster-scoped exchange request about a resolved resource of any other
/// node type is refused with: the whole sentence is fixed, naming the one node
/// family the operation covers rather than the resource it was asked about.
pub const ONLY_DS2_EXCHANGEABLE: &str = "You can only exchange DS2 reserved nodes.";

/// The `ActionType` of a reserved-node exchange initiated from a cluster RESIZE,
/// so the cluster being resized is what the configuration options are reported
/// for.
///
/// Source: `smithy_query(query="shape",
/// shape="ReservedNodeExchangeActionType")` — one of the two values the enum
/// declares.
pub const EXCHANGE_ACTION_RESIZE_CLUSTER: &str = "resize-cluster";

/// The `ActionType` of a reserved-node exchange initiated from a snapshot
/// RESTORE, the other value of the same enum, so the snapshot being restored
/// from is what the configuration options are reported for.
pub const EXCHANGE_ACTION_RESTORE_CLUSTER: &str = "restore-cluster";

/// A fresh `ReservedNodeExchangeRequestId`.
///
/// "The identifier of the reserved-node exchange request"
/// (`smithy_query(query="shape", shape="ReservedNodeExchangeStatus")`) is minted
/// by the service and carries nothing from the request, exactly like
/// [`new_reserved_node_id`], and the minted id is STORED so every later read of
/// that exchange reports the same one.
pub fn new_reserved_node_exchange_request_id() -> String {
    crate::service::query_error_envelope::new_request_id()
}

/// What an operation naming a `ReservedNodeExchangeRequestId` answers when this
/// account asked for no such exchange, worded like the sibling refusal for a
/// reservation id nobody holds ([`reserved_node_not_found_message`]).
pub fn reserved_node_exchange_not_found_message(request_id: &str) -> String {
    format!("Reserved Node Exchange Request {request_id} not found.")
}

/// What the exchange-status read answers a request that names NEITHER of the two
/// optional filters that say which exchanges to report.
///
/// Wholly fixed: nothing of the request appears in it. It states the rule the
/// operation enforces — either member alone will do, and so will both together,
/// because each one NARROWS the report — so a caller is not told to pick exactly
/// one when the service accepts both.
pub const MISSING_EXCHANGE_STATUS_FILTER: &str = "You must supply one or both of the optional \
     ReservedNodeId and ReservedNodeExchangeRequestId parameters.";

/// What a reserved-node exchange operation answers a `ClusterIdentifier` that
/// names no cluster this account holds: this family's own wording from the
/// per-operation catalog, [`ClusterNotFoundWording`].
pub fn exchange_cluster_not_found_message(cluster_identifier: &str) -> String {
    cluster_not_found_message(
        ClusterNotFoundWording::TheClusterWasNotFound,
        cluster_identifier,
    )
}

/// What a reserved-node exchange operation answers a `SnapshotIdentifier` that
/// names no snapshot this account holds, worded like its sibling
/// [`exchange_cluster_not_found_message`].
pub fn exchange_snapshot_not_found_message(snapshot_identifier: &str) -> String {
    format!("The snapshot {snapshot_identifier} was not found.")
}

/// Why a reservation cannot take part in an exchange, classified so each
/// operation answers it with the fault its OWN model declares for that class.
///
/// One classification for every exchange operation that dereferences a
/// `ReservedNodeId`, so no two of them can refuse the same reservation
/// differently.
#[derive(Debug)]
pub enum ExchangeRefusal {
    /// An identifier of a form this service never mints, ruled out before any
    /// lookup. The model declares no client-input fault for it, so it is
    /// answered with the `InvalidParameterValue` code Amazon Redshift itself
    /// uses.
    InvalidParameterValue(String),
    /// A well-formed identifier naming no reservation this account holds —
    /// `ReservedNodeNotFoundFault`.
    ReservedNodeNotFound(String),
    /// A reservation whose payment is not confirmed yet, so it is not available
    /// for use — `InvalidReservedNodeStateFault`.
    InvalidReservedNodeState(String),
    /// A reservation an exchange already moved off —
    /// `ReservedNodeAlreadyMigratedFault`.
    ReservedNodeAlreadyMigrated(String),
    /// The exchange would take the account past the compute-node bound this
    /// store holds ([`RESERVED_NODE_QUOTA`]) — `ReservedNodeQuotaExceededFault`.
    /// Only the path that MINTS a reservation can raise it.
    QuotaExceeded(String),
}

/// How close two published prices must be to be the same amount: a tenth of the
/// smallest digit a price carries ([`reserved_node_offering`] rounds every price
/// to three decimal places), so two prices computed from the same rate match and
/// two rounded to different amounts do not.
pub(super) const PRICE_MATCH_TOLERANCE: f64 = 0.000_1;

/// Whether two published prices are the same amount.
pub(super) fn prices_match(left: Option<f64>, right: Option<f64>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => (left - right).abs() < PRICE_MATCH_TOLERANCE,
        (None, None) => true,
        _ => false,
    }
}
