// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-outposts-outpostsolafservice service, plus
//! the rules more than one operation in the site + Outpost lifecycle applies: id
//! and ARN minting, scoping a resource to the region it was created in, resolving the
//! `id OR ARN` identifier forms the modelled
//! `@httpLabel` patterns accept, projecting a resource's tag map and an Outpost's
//! two Availability Zone identities, selecting a site's two addresses by the
//! modelled `AddressType` and keeping the `Site` projection of the operating one in
//! step with it, materialising the Outpost lifecycle transition,
//! the quote expiry, the order's progress and the capacity-task progression the
//! clock owes, allocating the
//! tunnel an Outpost-server installation connection is reported as, cutting an
//! ordered list into pages with the modelled `Token` the next page resumes from, the
//! orderable-instance-type catalog this service publishes, and
//! the two rules this service words its refusals by — the `NotFoundException`
//! identifier echo and the value-omitting `ValidationException` report.

use std::collections::{BTreeMap, HashMap, VecDeque};

use sha2::{Digest, Sha256};
use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    Address, AssetInfo, AssetInstance, CapacityTaskFailure, ConnectionDetails,
    DetailedInstanceTypeItem, FormFactorConfig, InstanceTypeCapacity, InstancesToExclude, Order,
    Outpost, PricingOption, PrivateConnectivityConfig, Quote, Site,
};

/// The region every minted ARN falls back to when the request carries no SigV4
/// credential scope to read one from (an unsigned, anonymous call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace of this service, as the model's `awsService` trait declares
/// it (`arnNamespace: outposts`) — the third segment of every ARN it mints.
pub const ARN_NAMESPACE: &str = "outposts";

/// The ARN resource TYPE a site's ARN carries (`…:site/os-…`), as the modelled
/// `SiteArn` pattern spells it.
///
/// Minting an ARN ([`site_arn`]) and reading one back ([`resource_arn_parts`])
/// name the kind through this one constant, so the two can never disagree about
/// what a site's ARN looks like.
pub const SITE_RESOURCE_TYPE: &str = "site";

/// The ARN resource TYPE an Outpost's ARN carries (`…:outpost/op-…`), from the
/// modelled `OutpostArn` pattern. See [`SITE_RESOURCE_TYPE`].
pub const OUTPOST_RESOURCE_TYPE: &str = "outpost";

/// The `LifeCycleStatus` a freshly created Outpost carries.
///
/// AWS answers `CreateOutpost` with an Outpost that is still being provisioned, so
/// the status a create reports — and every read of it reports until the transition
/// below falls due — is the non-terminal one. The model constrains
/// `LifeCycleStatus` without declaring a value set (len 1..20, pattern
/// `^[ A-Za-z]+$`), and both values this service uses satisfy it.
pub const LIFE_CYCLE_STATUS_PENDING: &str = "PENDING";

/// The `LifeCycleStatus` an Outpost settles in: the terminal, ready status it
/// holds once provisioning is done, which no later call moves it out of.
pub const LIFE_CYCLE_STATUS_ACTIVE: &str = "ACTIVE";

/// How long after `CreateOutpost` the Outpost's
/// [`LIFE_CYCLE_STATUS_PENDING`] -> [`LIFE_CYCLE_STATUS_ACTIVE`] transition falls
/// due.
///
/// AWS publishes no duration for provisioning an Outpost, so this span is the
/// emulator's own: short enough that a workflow polling for a ready Outpost
/// finishes at once, and long enough that a caller acting straight after the create
/// still observes the pending status the create reported. It is never zero —
/// a transition the clock owes is owed at an instant, not on the next read.
pub const OUTPOST_PROVISIONING: std::time::Duration = std::time::Duration::from_millis(500);

/// The `PrivateConnectivityStatus` an Outpost's private connectivity carries once
/// it has been configured.
///
/// The modelled `PrivateConnectivityStatus` enum declares exactly two values,
/// `ENABLED` and `DISABLED` (see
/// [`crate::ops::types::shared_types::PRIVATE_CONNECTIVITY_STATUS_VALUES`]), and neither is a
/// provisioning value — so a configuration that exists is already in a terminal
/// status and the clock owes no transition on it.
pub const PRIVATE_CONNECTIVITY_ENABLED: &str = "ENABLED";

/// The `QuoteStatus` a freshly created quote carries.
///
/// `CreateQuote` answers with a quote that exists to be read and ordered from, so
/// the status a create reports is the one a quote is usable in. The modelled
/// `QuoteStatus` enum declares exactly three values — `CREATED`,
/// `ORDER_SUBMITTED` and `EXPIRED` — and a quote reaches the other two only by
/// something happening to it: an order being submitted from it, or its own
/// `ExpirationDate` passing.
pub const QUOTE_STATUS_CREATED: &str = "CREATED";

/// The `QuoteStatus` a quote holds once the `ExpirationDate` it reports has
/// passed. See [`QUOTE_STATUS_CREATED`] and
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::settle_quotes`].
pub const QUOTE_STATUS_EXPIRED: &str = "EXPIRED";

/// The `QuoteStatus` a quote holds once an order has been submitted from it — the
/// model's "`ORDER_SUBMITTED` - An order has been submitted for the quote", which
/// is also what makes the quote's `SubmittedOrderId` present.
///
/// `CreateOrder` is what moves a quote into it, naming the order it minted in the
/// quote's `SubmittedOrderId`. It is also one of the two records this store holds of
/// an Outpost ORDER placed against an Outpost — the other being the order itself
/// ([`OrderRecord`]) — which is what `StartOutpostDecommission` reads to decide
/// whether an order still blocks a decommission.
pub const QUOTE_STATUS_ORDER_SUBMITTED: &str = "ORDER_SUBMITTED";

/// The `OrderStatus` a just-submitted order carries: the model's "`PREPARING` -
/// Order is received and being prepared".
pub const ORDER_STATUS_PREPARING: &str = "PREPARING";

/// The `OrderStatus` an order moves to once Amazon Web Services has started on the
/// hardware: the model's "`IN_PROGRESS` - Order is either being built or shipped. To
/// get more details, see the line item status".
///
/// This is as far as the clock carries an order. The two statuses past it —
/// `DELIVERED` ("Order was delivered to the Outpost site") and `COMPLETED` ("Order
/// is complete") — are reached by hardware physically arriving at the site and being
/// installed, which leaves a record this emulator does not have: an installed asset
/// on the Outpost ([`OutpostRecord::assets`]), reported by `ListAssets` and by the
/// line item's own `AssetInformationList`. Reporting an order delivered or complete
/// with no asset behind it would be a claim about hardware nothing installed, so the
/// emulator stops where it still has a record to show.
pub const ORDER_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `OrderStatus` of a cancelled order — the model's "`CANCELLED` - Order is
/// cancelled". No call in the implemented slice reaches it (`CancelOrder` is still a
/// scaffold stub with an issue of its own); it is named here because it is one of the
/// two statuses in which an order is FINISHED and so no longer outstanding on its
/// Outpost. See [`ORDER_STATUS_SETTLED`].
pub const ORDER_STATUS_CANCELLED: &str = "CANCELLED";

/// The `OrderStatus` of an order that ran to its end — the model's "`COMPLETED` -
/// Order is complete". See [`ORDER_STATUS_IN_PROGRESS`] for why the clock does not
/// carry an order this far, and [`ORDER_STATUS_SETTLED`].
pub const ORDER_STATUS_COMPLETED: &str = "COMPLETED";

/// Every `OrderStatus` in which an order is FINISHED: nothing more will happen to it
/// and it no longer holds anything up. An order in any other status is still
/// outstanding on its Outpost, which is what
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::blocking_resource_types`] reads.
///
/// `ERROR` ("Customer should contact support") is deliberately NOT here: an order in
/// it is not finished but waiting for a human, which the modelled
/// `BlockingResourceType` enum names a value of its own for. Nothing in this service
/// produces that status.
pub const ORDER_STATUS_SETTLED: &[&str] = &[ORDER_STATUS_CANCELLED, ORDER_STATUS_COMPLETED];

/// The `LineItemStatus` a line item of a just-submitted order carries, the item-level
/// counterpart of [`ORDER_STATUS_PREPARING`].
pub const LINE_ITEM_STATUS_PREPARING: &str = "PREPARING";

/// The `LineItemStatus` a line item moves to with its order's
/// [`ORDER_STATUS_IN_PROGRESS`]: that status is "either being built or shipped", and
/// the line item is what says which. The emulator reports the first of the two —
/// `SHIPPED` is a claim about hardware in transit, which it has no record of (see
/// [`ORDER_STATUS_IN_PROGRESS`]).
pub const LINE_ITEM_STATUS_BUILDING: &str = "BUILDING";

/// The `OrderType` of an order placed for an Outpost, from the modelled enum
/// (`OUTPOST | REPLACEMENT`).
///
/// `CreateOrder` places this kind: it names the Outpost the hardware is for and the
/// catalog items to install on it. A `REPLACEMENT` order is one Amazon Web Services
/// raises to replace hardware already installed — its line items name the item they
/// replace (`LineItem$PreviousLineItemId` / `$PreviousOrderId`) — and no input of
/// this API carries that, so no call here places one.
pub const ORDER_TYPE_OUTPOST: &str = "OUTPOST";

/// How long after `CreateOrder` the order's [`ORDER_STATUS_PREPARING`] ->
/// [`ORDER_STATUS_IN_PROGRESS`] transition falls due.
///
/// AWS publishes no duration for preparing an order, so this span is the emulator's
/// own, chosen for the reason [`OUTPOST_PROVISIONING`]'s is: short enough that a
/// workflow waiting for the order to move on finishes at once, and long enough that a
/// caller reading the order straight after submitting it still sees the status the
/// submit reported. It is never zero — a transition the clock owes is owed at an
/// instant, not on the next read.
pub const ORDER_PREPARATION: std::time::Duration = std::time::Duration::from_millis(500);

/// The `DecommissionRequestStatus` `StartOutpostDecommission` answers when a
/// resource still associated with the Outpost stops the decommission from
/// starting; the types of those resources travel beside it in
/// `BlockingResourceTypes`.
///
/// The three values below are exactly the modelled `DecommissionRequestStatus`
/// enum (see [`crate::ops::types::shared_types::DECOMMISSION_REQUEST_STATUS_VALUES`]).
pub const DECOMMISSION_STATUS_BLOCKED: &str = "BLOCKED";

/// The `DecommissionRequestStatus` of a decommission this call STARTED: nothing
/// blocks it, the caller did not ask to validate only, and the request is now
/// recorded on the Outpost ([`OutpostRecord::decommission_requested_at`]). See
/// [`DECOMMISSION_STATUS_BLOCKED`].
pub const DECOMMISSION_STATUS_REQUESTED: &str = "REQUESTED";

/// The `DecommissionRequestStatus` of a call that found nothing blocking and yet
/// started nothing — because the caller asked to validate the request only
/// (`ValidateOnly`, "Validates the request without starting the decommission
/// process"), or because the Outpost's decommission had already been requested, so
/// there is nothing left to start. See [`DECOMMISSION_STATUS_BLOCKED`].
pub const DECOMMISSION_STATUS_SKIPPED: &str = "SKIPPED";

/// The `BlockingResourceType` an Outpost order still outstanding on an Outpost is
/// reported as, from the modelled enum's values. See
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::blocking_resource_types`], which
/// states which of those values this store can observe and why it reports none of
/// the others.
pub const BLOCKING_OUTPOST_ORDER_CANCELLABLE: &str = "OUTPOST_ORDER_CANCELLABLE";

/// The `BlockingResourceType` an Amazon EC2 instance still running on an Outpost is
/// reported as, from the modelled enum's values.
///
/// AWS documents that an Outpost's instances have to be gone before it can be
/// decommissioned, and the store's record of an instance running on an Outpost is
/// [`OutpostRecord::instances`] — so an Outpost that holds one blocks its own
/// decommission. See
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::blocking_resource_types`].
pub const BLOCKING_EC2_INSTANCE: &str = "EC2_INSTANCE";

/// How long after `CreateQuote` the quote's `ExpirationDate` falls.
///
/// AWS documents the member only as "The date the quote expires" and publishes no
/// validity period, so this span is the emulator's own. It is a resource LIFETIME,
/// not a provisioning span: a quote is created in order to be read and ordered
/// from, so the window has to be long enough that every call a workflow makes
/// against a quote it just created still sees it live, while the quote still
/// carries a real expiry a caller can read and act on.
pub const QUOTE_VALIDITY: std::time::Duration =
    std::time::Duration::from_secs(30 * 24 * 60 * 60);

/// The `PricingResult` a pricing request carries when the service priced it: it
/// answers with the pricing options it found.
///
/// The modelled `PricingResult` enum declares exactly two values — see
/// [`crate::ops::types::shared_types::PRICING_RESULT_VALUES`] — and they are the
/// two outcomes of one question: either the request could be priced, or it could
/// not.
pub const PRICING_RESULT_PRICED: &str = "PRICED";

/// The `PricingResult` a pricing request carries when the service could not price
/// it, and so has no pricing option to report. See [`PRICING_RESULT_PRICED`].
pub const PRICING_RESULT_UNABLE_TO_PRICE: &str = "UNABLE_TO_PRICE";

/// A persisted site: the modelled [`Site`] this service answers with, plus the two
/// whole [`Address`] values `Site` itself does not carry (it exposes only the
/// operating address's country, state and city) and the creation order every list
/// pages in.
#[derive(Debug, Clone)]
pub struct SiteRecord {
    /// Position in this store's creation order — the sort key every list and every
    /// page token is expressed in, so a token stays correct across calls.
    pub sequence: u64,
    /// The region this site was created in. See [`RegionalRecord`].
    pub region: String,
    /// The site as `CreateSite` / `GetSite` / `ListSites` return it.
    pub site: Site,
    /// The full operating address the caller supplied, which `Site` projects down
    /// to `OperatingAddress{CountryCode,StateOrRegion,City}`.
    pub operating_address: Option<Address>,
    /// The full shipping address the caller supplied. `Site` carries no shipping
    /// member at all, so this is the only place it is kept.
    pub shipping_address: Option<Address>,
}

/// Which of a site's two addresses a modelled `AddressType` value names.
///
/// The model declares exactly two (`OPERATING_ADDRESS | SHIPPING_ADDRESS`, see
/// [`crate::ops::types::shared_types::ADDRESS_TYPE_VALUES`]) and a site holds one
/// of each, so every operation that reads or writes a site address selects the
/// slot through this one type rather than matching the wire string itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteAddressKind {
    /// The address the site itself is at: the one `Site` projects its
    /// `OperatingAddress{CountryCode,StateOrRegion,City}` members from.
    Operating,
    /// The address the site's hardware is shipped to. `Site` carries no
    /// projection of it.
    Shipping,
}

impl SiteAddressKind {
    /// The slot a modelled `AddressType` value names, or `None` for a value the
    /// modelled enum does not declare.
    ///
    /// The boundary refuses an unmodelled enum value before a handler runs, so
    /// `None` is the belt-and-braces branch a handler answers as the modelled
    /// `ValidationException` ([`address_type_value_report`]) rather than assuming
    /// one of the two.
    pub fn from_address_type(address_type: &str) -> Option<Self> {
        match address_type {
            ADDRESS_TYPE_OPERATING => Some(Self::Operating),
            ADDRESS_TYPE_SHIPPING => Some(Self::Shipping),
            _ => None,
        }
    }

    /// The modelled `AddressType` value this slot is named by on the wire — what
    /// an operation that reports the type it acted on answers with.
    pub fn address_type(self) -> &'static str {
        match self {
            Self::Operating => ADDRESS_TYPE_OPERATING,
            Self::Shipping => ADDRESS_TYPE_SHIPPING,
        }
    }
}

/// The `AddressType` value naming a site's operating address, as the modelled
/// `AddressType` enum spells it. See [`SiteAddressKind`].
pub const ADDRESS_TYPE_OPERATING: &str = "OPERATING_ADDRESS";

/// The `AddressType` value naming a site's shipping address. See
/// [`ADDRESS_TYPE_OPERATING`].
pub const ADDRESS_TYPE_SHIPPING: &str = "SHIPPING_ADDRESS";

impl SiteRecord {
    /// This site's address of the given kind, or `None` when the site carries
    /// none of that kind.
    ///
    /// `Site` projects only three members of the operating address, so the whole
    /// [`Address`] a caller sent is kept on the record and read back from here —
    /// which is what makes a site address a round trip.
    pub fn address(&self, kind: SiteAddressKind) -> Option<&Address> {
        match kind {
            SiteAddressKind::Operating => self.operating_address.as_ref(),
            SiteAddressKind::Shipping => self.shipping_address.as_ref(),
        }
    }

    /// Store `address` as this site's address of the given kind, replacing
    /// whatever it held before.
    ///
    /// Writing the OPERATING address re-derives the three members `Site` projects
    /// from it ([`operating_address_projection`]), so the site every later
    /// `GetSite` / `ListSites` answers can never disagree with the address
    /// `GetSiteAddress` answers.
    pub fn set_address(&mut self, kind: SiteAddressKind, address: Address) {
        match kind {
            SiteAddressKind::Operating => {
                self.operating_address = Some(address);
                let (country_code, state_or_region, city) =
                    operating_address_projection(self.operating_address.as_ref());
                self.site.operating_address_country_code = country_code;
                self.site.operating_address_state_or_region = state_or_region;
                self.site.operating_address_city = city;
            }
            SiteAddressKind::Shipping => self.shipping_address = Some(address),
        }
    }
}

/// A persisted Outpost: the modelled [`Outpost`] plus its creation order, the
/// instant its lifecycle transition falls due, and the private connectivity
/// configuration created for it.
#[derive(Debug, Clone)]
pub struct OutpostRecord {
    /// See [`SiteRecord::sequence`].
    pub sequence: u64,
    /// The region this Outpost was created in. See [`RegionalRecord`].
    pub region: String,
    /// The Outpost as `CreateOutpost` / `GetOutpost` / `ListOutposts` return it.
    pub outpost: Outpost,
    /// The private connectivity configuration of this Outpost, or `None` for an
    /// Outpost nobody has created one for.
    ///
    /// Private connectivity is an attribute of ONE Outpost — the modelled routes
    /// are `/outposts/{OutpostId}/privateConnectivity` — so it is kept on the
    /// Outpost's own record: the create writes it here, the read answers it from
    /// here, and deleting the Outpost takes it with the Outpost.
    pub private_connectivity: Option<PrivateConnectivityConfig>,
    /// When this Outpost's `PENDING` -> `ACTIVE` transition falls due, or `None`
    /// once it has been materialised and the status is terminal. Stored as an
    /// instant so nothing ever sleeps for it: the transition is applied by the
    /// first call made at or after it, which reports exactly the status the
    /// Outpost would hold had nobody called at all.
    pub active_at: Option<std::time::Instant>,
    /// When `StartOutpostDecommission` started the decommission of this Outpost,
    /// or `None` for an Outpost nobody has asked to decommission.
    ///
    /// A decommission is started once: it is what makes a later
    /// `StartOutpostDecommission` on the same Outpost answer
    /// [`DECOMMISSION_STATUS_SKIPPED`] rather than starting it again. The Outpost's
    /// `LifeCycleStatus` is left alone — the model constrains that member without
    /// declaring a value set and AWS publishes no decommissioning value, so the
    /// emulator does not invent one.
    pub decommission_requested_at: Option<std::time::SystemTime>,
    /// The priced subscription options a RENEWAL of this Outpost's contract can be
    /// bought under — what `GetRenewalPricing` reports for it, and the list a
    /// `CreateRenewal` picks a payment option and term out of.
    ///
    /// Kept on the Outpost's own record for the reason
    /// [`OutpostRecord::private_connectivity`] is: renewal pricing is an attribute
    /// of ONE Outpost (the modelled route is
    /// `/outpost/{OutpostIdentifier}/renewal-pricing`), so deleting the Outpost
    /// takes it with the Outpost.
    ///
    /// A renewal price is a figure from AWS's own price list for the hardware the
    /// Outpost's order installed, which this emulator does not have — the same
    /// reason `CreateQuote` reports no `QuoteOption` — so this holds only priced
    /// options that were actually recorded and never a number the emulator made
    /// up. Nothing in the implemented slice of this API records one: an Outpost's
    /// priced subscriptions arrive with its order, and the order `CreateOrder`
    /// accepts carries no price for the same reason a quote carries none, while
    /// `CreateRenewal` and `GetOutpostBillingInformation` are still scaffold stubs
    /// with issues of their own. So a `CreateOutpost` leaves this empty, and
    /// an Outpost with no recorded option cannot be priced.
    pub renewal_pricing: Vec<PricingOption>,
    /// The hardware assets INSTALLED on this Outpost — what `ListAssets` reports
    /// for it — in the order they were installed, which is the order a page of
    /// them is cut in ([`ASSET_PAGE`]).
    ///
    /// Kept on the Outpost's own record for the reason
    /// [`OutpostRecord::private_connectivity`] and [`OutpostRecord::renewal_pricing`]
    /// are: an asset belongs to exactly ONE Outpost (the modelled route is
    /// `/outposts/{OutpostIdentifier}/assets`), so deleting the Outpost takes its
    /// assets with it.
    ///
    /// An asset is a PHYSICAL unit — a rack, a compute or storage server, a
    /// powershelf, a switch — that Amazon Web Services installs on the site after an
    /// order is fulfilled, so no call in this API creates one and `CreateOutpost`
    /// leaves this empty: an order this service accepts is never carried as far as
    /// being delivered and installed, because that is exactly the record it would
    /// have to invent (see [`ORDER_STATUS_IN_PROGRESS`]). This therefore holds only
    /// the assets that were actually recorded for the Outpost, exactly as
    /// [`OutpostRecord::renewal_pricing`] holds only recorded prices, and an Outpost
    /// with no installed hardware is reported as having none rather than as having
    /// hardware the emulator made up.
    pub assets: Vec<AssetInfo>,
    /// The Amazon EC2 instances RUNNING on this Outpost — what
    /// `ListBlockingInstancesForCapacityTask` weighs a capacity task against, and what
    /// `ListAssetInstances` reports
    /// — in the order they were recorded, which is the order a page of them is cut in
    /// ([`BLOCKING_INSTANCE_PAGE`], [`ASSET_INSTANCE_PAGE`]).
    ///
    /// Kept on the Outpost's own record for the reason [`OutpostRecord::assets`] is: an
    /// instance runs on exactly ONE Outpost (the modelled routes nest both lists of
    /// them under `/outposts/{OutpostIdentifier}/…`), so deleting the Outpost takes its
    /// instances with it.
    ///
    /// An instance is launched through Amazon EC2 — `RunInstances` with the Outpost's
    /// ARN — and not by any call in THIS API, so `CreateOutpost` leaves this empty and
    /// nothing here is ever invented: it holds only the instances that were actually
    /// recorded for the Outpost, exactly as [`OutpostRecord::assets`] holds only
    /// recorded hardware, and an Outpost nobody has launched an instance on is reported
    /// as having none.
    ///
    /// The modelled [`AssetInstance`] is the shape this service reports an instance as
    /// (`ListAssetInstances$AssetInstances`) and carries every member the narrower
    /// `BlockingInstance` projection does, so the two lists read ONE record of an
    /// instance rather than two copies of it.
    pub instances: Vec<AssetInstance>,
}

/// The `CapacityTaskStatus` a capacity task carries the moment `StartCapacityTask`
/// accepts it: "the capacity task was created and is awaiting the next step by
/// Amazon Web Services Outposts", as the model documents the value.
pub const CAPACITY_TASK_REQUESTED: &str = "REQUESTED";

/// The `CapacityTaskStatus` a capacity task carries while it runs — the model's
/// "the capacity task is running and cannot be cancelled".
pub const CAPACITY_TASK_IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal `CapacityTaskStatus` of a capacity task that ran: the model's "the
/// capacity task has completed successfully". No later call moves a task out of it.
pub const CAPACITY_TASK_COMPLETED: &str = "COMPLETED";

/// The terminal `CapacityTaskStatus` of a task that could not be completed.
pub const CAPACITY_TASK_FAILED: &str = "FAILED";

/// The terminal `CapacityTaskStatus` of a cancelled task.
pub const CAPACITY_TASK_CANCELLED: &str = "CANCELLED";

/// Every TERMINAL `CapacityTaskStatus` the model declares: a task in one of these
/// is done, and the Outpost and order it belongs to can take another task. The
/// other three modelled values (`REQUESTED`, `IN_PROGRESS`,
/// `WAITING_FOR_EVACUATION`) are statuses Amazon Web Services Outposts is still
/// working through, so a task in one of them is ACTIVE.
pub const CAPACITY_TASK_TERMINAL: &[&str] = &[
    CAPACITY_TASK_COMPLETED,
    CAPACITY_TASK_FAILED,
    CAPACITY_TASK_CANCELLED,
];

// The `CapacityTaskStatus` values a task this emulator started never reaches, and
// why, stated once so no handler invents one:
//
// * `WAITING_FOR_EVACUATION` and the `CapacityTaskFailure` type
//   `BLOCKING_INSTANCES_NOT_EVACUATED` describe a task held up by running EC2
//   instances. Whether a task is held up is a judgement about the Outpost's
//   INSTALLED instance capacity, which this emulator does not model — the same
//   reason `StartCapacityTask` answers a dry run 501 — so a task it starts runs on
//   the clock and never waits. Which instances would have to stop is reported by
//   `ListBlockingInstancesForCapacityTask`, off [`OutpostRecord::instances`].
// * `FAILED` needs a failure something produced; nothing in this slice can fail a
//   task, so `Failed` stays absent rather than carrying an invented reason.
// * `CANCELLATION_IN_PROGRESS` and `CANCELLED` belong to `CancelCapacityTask`,
//   which is still a scaffold stub.

/// How long after the call that started it each step of a capacity task's
/// `REQUESTED` -> `IN_PROGRESS` -> `COMPLETED` progression falls due.
///
/// AWS publishes no duration for running a capacity task, so — exactly as
/// [`OUTPOST_PROVISIONING`] — this span is the emulator's own: short enough that a
/// workflow polling for a finished task finishes at once, and never zero, so a
/// caller reading straight after `StartCapacityTask` still observes the
/// `REQUESTED` status the start reported.
pub const CAPACITY_TASK_STEP: std::time::Duration = std::time::Duration::from_millis(500);

/// One status transition a capacity task owes the clock.
#[derive(Debug, Clone)]
pub struct CapacityTaskTransition {
    /// When the transition falls due, as a monotonic instant: nothing ever sleeps
    /// for it, the first call made at or after it materialises it.
    pub due: std::time::Instant,
    /// The wall clock the transition is owed AT, which is what the task's
    /// `LastModifiedDate` (and, for the terminal step, `CompletionDate`) reports —
    /// so a late poll reports when the task really changed, not when it was polled.
    pub at: std::time::SystemTime,
    /// The `CapacityTaskStatus` the task holds from that instant on.
    pub status: String,
}

/// A persisted capacity task: every member `StartCapacityTask` / `GetCapacityTask`
/// report, plus its creation order and the transitions the clock still owes it.
#[derive(Debug, Clone)]
pub struct CapacityTaskRecord {
    /// See [`SiteRecord::sequence`].
    pub sequence: u64,
    /// The minted `CapacityTaskId` (`cap-…`) this record is keyed by.
    pub capacity_task_id: String,
    /// The bare `OutpostId` of the Outpost the task runs on — the parent the
    /// task's route nests it under.
    pub outpost_id: String,
    /// The `OrderId` the caller associated the task with, if any.
    pub order_id: Option<String>,
    /// The `AssetId` the caller named, if any.
    pub asset_id: Option<String>,
    /// The instance pools the caller requested, reported back as
    /// `RequestedInstancePools`.
    pub requested_instance_pools: Vec<InstanceTypeCapacity>,
    /// The instances the caller said must not be stopped.
    pub instances_to_exclude: Option<InstancesToExclude>,
    /// Whether the task was requested as a dry run.
    pub dry_run: bool,
    /// What the caller asked for when an instance blocks the task.
    pub task_action_on_blocking_instances: Option<String>,
    /// The `CapacityTaskStatus` the task holds now, as of the last settle.
    pub status: String,
    /// Why the task failed. See [`CAPACITY_TASK_FAILED`].
    pub failed: Option<CapacityTaskFailure>,
    /// When `StartCapacityTask` created the task (`CreationDate`).
    pub created_at: std::time::SystemTime,
    /// When the task last changed (`LastModifiedDate`).
    pub last_modified_at: std::time::SystemTime,
    /// When the task completed (`CompletionDate`), absent until it does.
    pub completed_at: Option<std::time::SystemTime>,
    /// The transitions still owed, in the order they fall due. Empty once the task
    /// has settled in a terminal status.
    pub pending: VecDeque<CapacityTaskTransition>,
}

impl CapacityTaskRecord {
    /// Whether this task is ACTIVE: it is in a status Amazon Web Services Outposts
    /// is still working through, so the Outpost (and the order) it belongs to
    /// cannot take another one. A task in a terminal status is not active.
    pub fn is_active(&self) -> bool {
        !CAPACITY_TASK_TERMINAL.contains(&self.status.as_str())
    }
}

/// The transitions a capacity task started at `now` owes the clock: one
/// [`CAPACITY_TASK_STEP`] to `IN_PROGRESS`, and another to `COMPLETED`.
///
/// Both instants are fixed when the task starts, so the status a read reports is
/// the one the task holds at the instant it is asked for — the same answer whether
/// or not anybody polled in between.
pub fn capacity_task_progression(now: std::time::SystemTime) -> VecDeque<CapacityTaskTransition> {
    let started = std::time::Instant::now();
    [CAPACITY_TASK_IN_PROGRESS, CAPACITY_TASK_COMPLETED]
        .into_iter()
        .enumerate()
        .map(|(step, status)| {
            let elapsed = CAPACITY_TASK_STEP * (u32::try_from(step).unwrap_or(0) + 1);
            CapacityTaskTransition {
                due: started + elapsed,
                at: now + elapsed,
                status: status.to_string(),
            }
        })
        .collect()
}

/// Project a stored [`CapacityTaskRecord`] onto an operation's output shape.
///
/// `StartCapacityTask` and `GetCapacityTask` declare the SAME thirteen members for
/// a capacity task, as two distinct generated structures. Stating the projection
/// once, as a macro over the output type, is what keeps the two from drifting
/// apart on what a capacity task reports, the way [`page`] keeps the two lists from
/// drifting apart on how they page.
///
/// `RequestedInstancePools` is always reported, empty list and all, because the
/// pools are part of the task the caller asked for; `OrderId`, `AssetId`,
/// `InstancesToExclude` and `TaskActionOnBlockingInstances` are reported only when
/// the caller named them, and `Failed` only when the task failed.
#[macro_export]
macro_rules! capacity_task_projection {
    ($output:ident, $record:expr) => {{
        let record: &$crate::state::CapacityTaskRecord = $record;
        $output {
            capacity_task_id: Some(record.capacity_task_id.clone()),
            outpost_id: Some(record.outpost_id.clone()),
            order_id: record.order_id.clone(),
            asset_id: record.asset_id.clone(),
            requested_instance_pools: Some(record.requested_instance_pools.clone()),
            instances_to_exclude: record.instances_to_exclude.clone(),
            dry_run: record.dry_run,
            capacity_task_status: Some(record.status.clone()),
            failed: record.failed.clone(),
            creation_date: Some(::aws_smithy_types::DateTime::from(record.created_at)),
            completion_date: record
                .completed_at
                .map(::aws_smithy_types::DateTime::from),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(record.last_modified_at)),
            task_action_on_blocking_instances: record.task_action_on_blocking_instances.clone(),
        }
    }};
}

/// A persisted quote: the modelled [`Quote`] plus its creation order.
///
/// A quote carries its own `CreatedDate` and `ExpirationDate`, so the instant its
/// `CREATED` -> `EXPIRED` transition falls due is the `ExpirationDate` the stored
/// quote already reports — there is no second copy of it to drift from.
#[derive(Debug, Clone)]
pub struct QuoteRecord {
    /// See [`SiteRecord::sequence`].
    pub sequence: u64,
    /// The region this quote was created in. See [`RegionalRecord`].
    pub region: String,
    /// The quote as `CreateQuote` / `GetQuote` return it.
    pub quote: Quote,
}

/// A stored resource that belongs to ONE region: the region of the endpoint it was
/// created through, which is the only region it is reachable through.
///
/// This service is regional — its `awsService` trait declares an `endpointPrefix`
/// and a `sigv4` configuration, and the modelled `SiteArn` / `OutpostArn` patterns
/// pin a region segment into every ARN it mints — so a resource is part of its own
/// region's inventory and of no other's. Every operation therefore resolves an
/// identifier, and enumerates a list, WITHIN the region the request is addressed to
/// ([`request_region`]), through [`ComAmazonawsOutpostsOutpostsolafserviceState`]'s
/// region-scoped accessors below; a resource of another region is as absent as one
/// that was never created, which is what makes the ARN a read reports carry the
/// region the read was addressed to.
///
/// Only a resource a request can name DIRECTLY carries its region. The resources
/// kept inside an [`OutpostRecord`] (its private connectivity configuration, its
/// assets, its instances) and the capacity tasks nested under it are reachable only
/// by resolving that Outpost first, so the Outpost's region already scopes them and
/// a second copy of it could only drift.
pub trait RegionalRecord {
    /// The region this record belongs to.
    fn region(&self) -> &str;
}

impl RegionalRecord for SiteRecord {
    fn region(&self) -> &str {
        &self.region
    }
}

impl RegionalRecord for OutpostRecord {
    fn region(&self) -> &str {
        &self.region
    }
}

impl RegionalRecord for QuoteRecord {
    fn region(&self) -> &str {
        &self.region
    }
}

impl RegionalRecord for ConnectionRecord {
    fn region(&self) -> &str {
        &self.region
    }
}

impl RegionalRecord for OrderRecord {
    fn region(&self) -> &str {
        &self.region
    }
}

/// The record `id` names IN `region`, or `None` when the store holds no such record
/// in it — including when it holds one of that id in ANOTHER region, which this
/// region's endpoint does not serve. See [`RegionalRecord`].
fn in_region<'a, R: RegionalRecord>(
    records: &'a BTreeMap<String, R>,
    region: &str,
    id: &str,
) -> Option<&'a R> {
    records.get(id).filter(|record| record.region() == region)
}

/// The same record, to CHANGE. See [`in_region`].
fn in_region_mut<'a, R: RegionalRecord>(
    records: &'a mut BTreeMap<String, R>,
    region: &str,
    id: &str,
) -> Option<&'a mut R> {
    records
        .get_mut(id)
        .filter(|record| record.region() == region)
}

/// Every record of `region`, in the store's creation order. See [`RegionalRecord`].
fn all_in_region<'a, R: RegionalRecord>(
    records: &'a BTreeMap<String, R>,
    region: &str,
) -> impl Iterator<Item = &'a R> + 'a {
    // The region is copied into the filter so the iterator borrows only the store:
    // a caller that answers a reference out of it (`outpost_at_site`) is then not
    // holding the request's own value alive for as long as the record.
    let region = region.to_string();
    records
        .values()
        .filter(move |record| record.region() == region.as_str())
}

/// A persisted order: the modelled [`Order`] plus its creation order and the instant
/// the transition the clock owes it falls due.
#[derive(Debug, Clone)]
pub struct OrderRecord {
    /// See [`SiteRecord::sequence`].
    pub sequence: u64,
    /// The region this order was placed in. See [`RegionalRecord`].
    pub region: String,
    /// The order as `CreateOrder` / `GetOrder` return it.
    pub order: Order,
    /// When this order's [`ORDER_STATUS_PREPARING`] -> [`ORDER_STATUS_IN_PROGRESS`]
    /// transition falls due, or `None` once it has been materialised. Stored as an
    /// instant for the reason [`OutpostRecord::active_at`] is: nothing ever sleeps
    /// for it, and the first call made at or after it reports exactly the status the
    /// order would hold had nobody called at all.
    pub in_progress_at: Option<std::time::Instant>,
}

impl OrderRecord {
    /// Whether this order is still OUTSTANDING on its Outpost: it is in neither of
    /// the two statuses in which an order is finished ([`ORDER_STATUS_SETTLED`]).
    pub fn is_outstanding(&self) -> bool {
        self.order
            .status
            .as_deref()
            .is_none_or(|status| !ORDER_STATUS_SETTLED.contains(&status))
    }
}

/// A persisted Outpost-server installation connection: the tunnel
/// `StartConnection` handed out, as `GetConnection` reports it, beside the hardware
/// it was started for.
#[derive(Debug, Clone)]
pub struct ConnectionRecord {
    /// See [`SiteRecord::sequence`]. It is also what this connection's tunnel
    /// addresses and underlay address are allocated from, so no two connections of
    /// one store are handed the same address.
    pub sequence: u64,
    /// The region this connection was started in. See [`RegionalRecord`].
    pub region: String,
    /// The minted `ConnectionId` this record is keyed by.
    pub connection_id: String,
    /// The `AssetId` of the Outpost server being installed — the asset the start
    /// resolved, so the record names the hardware the tunnel belongs to.
    pub asset_id: String,
    /// The serial number of the dongle the caller named, absent when it named none.
    pub device_serial_number: Option<String>,
    /// The device index of the Outpost server's network interface the caller named.
    pub network_interface_device_index: i32,
    /// The `UnderlayIpAddress` the start reported for that network interface.
    pub underlay_ip_address: String,
    /// The connection as `GetConnection` reports it, computed once when the
    /// connection was started: every read answers the tunnel the start handed out,
    /// rather than one recomputed per call.
    pub details: ConnectionDetails,
}

/// The UDP port a connection's `ServerEndpoint` names.
///
/// The tunnel a connection describes is a WireGuard tunnel — both of its keys are
/// modelled `WireGuardPublicKey`s — and 51820 is WireGuard's default listen port.
pub const CONNECTION_SERVER_PORT: u16 = 51820;

/// The host part of every connection's `ServerEndpoint`: the address this store's
/// one tunnel server is reported on (see
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::server_public_key`]).
///
/// It is taken from the block RFC 5737 reserves for documentation and examples
/// (`192.0.2.0/24`), so the endpoint this emulator reports cannot name a host on
/// anybody's real network.
pub const CONNECTION_SERVER_HOST: &str = "192.0.2.1";

/// The first octet of the block a connection's point-to-point tunnel link is cut
/// from: RFC 1918's private `10.0.0.0/8`, where both ends of a tunnel can be
/// numbered without naming a public host.
const TUNNEL_BLOCK: u8 = 10;

/// The prefix length both of a connection's tunnel `CIDR` members report: a /30,
/// the smallest IPv4 prefix that carries two usable host addresses — one for each
/// end of a point-to-point link.
const TUNNEL_PREFIX: u32 = 30;

/// How many addresses that /30 holds, which is the stride from one connection's
/// link to the next.
const TUNNEL_LINK_SIZE: u64 = 1 << (32 - TUNNEL_PREFIX);

/// The first octet of the block a connection's `UnderlayIpAddress` is allocated
/// from, with [`UNDERLAY_BLOCK_SECOND_OCTET`]: RFC 6598's shared address space
/// `100.64.0.0/10`, which holds the addresses a service numbers its own devices in
/// rather than addresses of the caller's network.
const UNDERLAY_BLOCK: u8 = 100;

/// See [`UNDERLAY_BLOCK`]: the base of the `/10`'s second octet, whose low six bits
/// are part of the host number.
const UNDERLAY_BLOCK_SECOND_OCTET: u8 = 64;

/// The dotted quad whose first octet is `first` and whose other three carry `host`:
/// how every address allocated out of a `/8`-shaped block here is rendered, so each
/// one satisfies the dotted-quad patterns the model declares.
///
/// `host` is masked to the 24 bits those three octets hold, so an allocation that
/// runs past the end of the block wraps inside it instead of rendering an octet
/// above 255.
fn dotted_quad(first: u8, host: u64) -> String {
    let host = host & 0x00ff_ffff;
    format!(
        "{first}.{}.{}.{}",
        (host >> 16) & 0xff,
        (host >> 8) & 0xff,
        host & 0xff
    )
}

/// The `UnderlayIpAddress` of the `sequence`-th connection a store created: one
/// address of [`UNDERLAY_BLOCK`]'s shared address space per connection, so two
/// connections are never reported on the same underlay address — for as many
/// connections as the block's 22 host bits number, past which the allocation wraps
/// inside the block (far beyond [`MAX_RESOURCES`], the number of connections one
/// store will take).
pub fn underlay_ip_address(sequence: u64) -> String {
    // The /10 leaves 22 bits of host: the low six of the second octet and the last
    // two octets.
    let host = sequence & 0x003f_ffff;
    dotted_quad(
        UNDERLAY_BLOCK,
        (u64::from(UNDERLAY_BLOCK_SECOND_OCTET) << 16) | host,
    )
}

/// The tunnel [`ConnectionDetails`] of the `sequence`-th connection a store created:
/// between the client whose public key the caller sent and that store's own tunnel
/// server.
///
/// Each connection is numbered on its OWN point-to-point link — the `sequence`-th
/// /30 of [`TUNNEL_BLOCK`], with the server on its first usable address and the
/// client on the second, so two connections of one store never share a tunnel
/// address, for as many connections as the block numbers (see
/// [`underlay_ip_address`]) — while the server endpoint and server key are the
/// store's, the same in every connection it reports.
pub fn connection_details(
    sequence: u64,
    client_public_key: String,
    server_public_key: String,
) -> ConnectionDetails {
    // Wrapping throughout: the link number is an internal counter and the rendering
    // masks it into the block either way, so a count past the block's end wraps
    // inside it rather than failing.
    let link = sequence.wrapping_mul(TUNNEL_LINK_SIZE);
    let server = dotted_quad(TUNNEL_BLOCK, link.wrapping_add(1));
    let client = dotted_quad(TUNNEL_BLOCK, link.wrapping_add(2));
    ConnectionDetails {
        client_public_key: Some(client_public_key),
        server_public_key: Some(server_public_key),
        server_endpoint: Some(format!("{CONNECTION_SERVER_HOST}:{CONNECTION_SERVER_PORT}")),
        client_tunnel_address: Some(format!("{client}/{TUNNEL_PREFIX}")),
        server_tunnel_address: Some(format!("{server}/{TUNNEL_PREFIX}")),
        // What this client may route into the tunnel: the one address at the other
        // end of its own link, as a host prefix.
        allowed_ips: Some(vec![format!("{server}/32")]),
    }
}

/// How many MiB of memory one GiB is, which turns a family's documented
/// GiB-per-vCPU ratio into the `MemoryInMib` the model reports.
const MIB_PER_GIB: i32 = 1024;

/// One EC2 instance FAMILY in the orderable-instance-type catalog: the sizes it is
/// offered in, the hardware every size carries, and where an Outpost can be ordered
/// with it.
///
/// A family is the unit because EC2 publishes a family's specifications as one rule
/// — every size in it carries the same memory per vCPU and the name is
/// `<family>.<size>` — so one row here expands to every size of that family and no
/// size can drift from its family's ratio.
struct OrderableFamily {
    /// The family prefix an instance type in it carries: `c5` in `c5.xlarge`.
    family: &'static str,
    /// GiB of memory per vCPU every size in this family carries.
    ///
    /// Source: the EC2 instance-type specifications AWS publishes — the compute
    /// families (`c5`, `c6gd`, `c6id`) carry 2 GiB per vCPU, the general-purpose
    /// ones (`m5`, `m6id`, `g4dn`) 4, and the memory- and storage-optimized ones
    /// (`r5`, `r6id`, `i3en`) 8.
    memory_gib_per_vcpu: i32,
    /// Every size the family is offered in, as (size suffix, vCPUs, network
    /// performance).
    ///
    /// Source: the same EC2 instance-type specifications — `VCPUs` and
    /// `NetworkPerformance` are per-size figures, so each is spelled out per row
    /// rather than derived.
    sizes: &'static [(&'static str, i32, &'static str)],
    /// The `(FormFactor, OutpostGeneration)` pairs an Outpost carrying this family
    /// can be ordered as — the modelled `FormFactorConfig` list, which is a LIST
    /// because one instance type can be orderable in more than one of them.
    form_factors: &'static [(&'static str, &'static str)],
}

/// The orderable-instance-type catalog this service publishes, by family.
///
/// Two different sources meet in each row. The hardware figures (the sizes, their
/// vCPU counts, the memory ratio and the network performance) are the EC2
/// instance-type specifications AWS publishes. WHICH generation and form factor a
/// family can be ordered in is this emulator's own catalog assignment: AWS publishes
/// the orderable set per order rather than as a machine-readable list, so the
/// families that have been on Outposts racks since launch are assigned to
/// `GENERATION_1` racks, the later-generation families to `GENERATION_2` racks, and
/// the two families AWS Outposts SERVERS are built on (the Graviton2 `c6gd` and the
/// Intel `c6id`) to the `SERVER` form factor. The modelled value sets these name are
/// [`crate::ops::types::FORM_FACTOR_VALUES`] (`RACK` | `SERVER`) and
/// [`crate::ops::types::OUTPOST_GENERATION_VALUES`] (`GENERATION_1` |
/// `GENERATION_2`).
const ORDERABLE_FAMILIES: &[OrderableFamily] = &[
    OrderableFamily {
        family: "c5",
        memory_gib_per_vcpu: 2,
        sizes: &[
            ("large", 2, "Up to 10 Gigabit"),
            ("xlarge", 4, "Up to 10 Gigabit"),
            ("2xlarge", 8, "Up to 10 Gigabit"),
            ("4xlarge", 16, "Up to 10 Gigabit"),
            ("9xlarge", 36, "10 Gigabit"),
            ("12xlarge", 48, "12 Gigabit"),
            ("18xlarge", 72, "25 Gigabit"),
            ("24xlarge", 96, "25 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "m5",
        memory_gib_per_vcpu: 4,
        sizes: &[
            ("large", 2, "Up to 10 Gigabit"),
            ("xlarge", 4, "Up to 10 Gigabit"),
            ("2xlarge", 8, "Up to 10 Gigabit"),
            ("4xlarge", 16, "Up to 10 Gigabit"),
            ("8xlarge", 32, "10 Gigabit"),
            ("12xlarge", 48, "12 Gigabit"),
            ("16xlarge", 64, "20 Gigabit"),
            ("24xlarge", 96, "25 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "r5",
        memory_gib_per_vcpu: 8,
        sizes: &[
            ("large", 2, "Up to 10 Gigabit"),
            ("xlarge", 4, "Up to 10 Gigabit"),
            ("2xlarge", 8, "Up to 10 Gigabit"),
            ("4xlarge", 16, "Up to 10 Gigabit"),
            ("8xlarge", 32, "10 Gigabit"),
            ("12xlarge", 48, "12 Gigabit"),
            ("16xlarge", 64, "20 Gigabit"),
            ("24xlarge", 96, "25 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "i3en",
        memory_gib_per_vcpu: 8,
        sizes: &[
            ("large", 2, "Up to 25 Gigabit"),
            ("xlarge", 4, "Up to 25 Gigabit"),
            ("2xlarge", 8, "Up to 25 Gigabit"),
            ("3xlarge", 12, "Up to 25 Gigabit"),
            ("6xlarge", 24, "25 Gigabit"),
            ("12xlarge", 48, "50 Gigabit"),
            ("24xlarge", 96, "100 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "g4dn",
        memory_gib_per_vcpu: 4,
        sizes: &[
            ("xlarge", 4, "Up to 25 Gigabit"),
            ("2xlarge", 8, "Up to 25 Gigabit"),
            ("4xlarge", 16, "Up to 25 Gigabit"),
            ("8xlarge", 32, "50 Gigabit"),
            ("12xlarge", 48, "50 Gigabit"),
            ("16xlarge", 64, "50 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "c6gd",
        memory_gib_per_vcpu: 2,
        sizes: &[
            ("medium", 1, "Up to 10 Gigabit"),
            ("large", 2, "Up to 10 Gigabit"),
            ("xlarge", 4, "Up to 10 Gigabit"),
            ("2xlarge", 8, "Up to 10 Gigabit"),
            ("4xlarge", 16, "Up to 10 Gigabit"),
            ("8xlarge", 32, "12 Gigabit"),
            ("12xlarge", 48, "20 Gigabit"),
            ("16xlarge", 64, "25 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_SERVER, OUTPOST_GENERATION_1)],
    },
    OrderableFamily {
        family: "c6id",
        memory_gib_per_vcpu: 2,
        sizes: &[
            ("large", 2, "Up to 12.5 Gigabit"),
            ("xlarge", 4, "Up to 12.5 Gigabit"),
            ("2xlarge", 8, "Up to 12.5 Gigabit"),
            ("4xlarge", 16, "Up to 12.5 Gigabit"),
            ("8xlarge", 32, "12.5 Gigabit"),
            ("12xlarge", 48, "18.75 Gigabit"),
            ("16xlarge", 64, "25 Gigabit"),
            ("24xlarge", 96, "37.5 Gigabit"),
            ("32xlarge", 128, "50 Gigabit"),
        ],
        form_factors: &[
            (FORM_FACTOR_SERVER, OUTPOST_GENERATION_1),
            (FORM_FACTOR_RACK, OUTPOST_GENERATION_2),
        ],
    },
    OrderableFamily {
        family: "m6id",
        memory_gib_per_vcpu: 4,
        sizes: &[
            ("large", 2, "Up to 12.5 Gigabit"),
            ("xlarge", 4, "Up to 12.5 Gigabit"),
            ("2xlarge", 8, "Up to 12.5 Gigabit"),
            ("4xlarge", 16, "Up to 12.5 Gigabit"),
            ("8xlarge", 32, "12.5 Gigabit"),
            ("12xlarge", 48, "18.75 Gigabit"),
            ("16xlarge", 64, "25 Gigabit"),
            ("24xlarge", 96, "37.5 Gigabit"),
            ("32xlarge", 128, "50 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_2)],
    },
    OrderableFamily {
        family: "r6id",
        memory_gib_per_vcpu: 8,
        sizes: &[
            ("large", 2, "Up to 12.5 Gigabit"),
            ("xlarge", 4, "Up to 12.5 Gigabit"),
            ("2xlarge", 8, "Up to 12.5 Gigabit"),
            ("4xlarge", 16, "Up to 12.5 Gigabit"),
            ("8xlarge", 32, "12.5 Gigabit"),
            ("12xlarge", 48, "18.75 Gigabit"),
            ("16xlarge", 64, "25 Gigabit"),
            ("24xlarge", 96, "37.5 Gigabit"),
            ("32xlarge", 128, "50 Gigabit"),
        ],
        form_factors: &[(FORM_FACTOR_RACK, OUTPOST_GENERATION_2)],
    },
];

/// The `FormFactor` an Outposts RACK carries, from the modelled value set
/// [`crate::ops::types::FORM_FACTOR_VALUES`].
pub const FORM_FACTOR_RACK: &str = "RACK";

/// The `FormFactor` an Outposts SERVER carries. See [`FORM_FACTOR_RACK`].
pub const FORM_FACTOR_SERVER: &str = "SERVER";

/// The first-generation `OutpostGeneration`, from the modelled value set
/// [`crate::ops::types::OUTPOST_GENERATION_VALUES`] — "`GENERATION_1` for
/// first-generation rack deployments", as
/// `ListOrderableInstanceTypes$OutpostGenerationFilter` documents it.
pub const OUTPOST_GENERATION_1: &str = "GENERATION_1";

/// The second-generation `OutpostGeneration`. See [`OUTPOST_GENERATION_1`].
pub const OUTPOST_GENERATION_2: &str = "GENERATION_2";

/// One catalog entry: the modelled [`DetailedInstanceTypeItem`] plus its position in
/// the catalog's stable order.
///
/// The catalog is REFERENCE data, not a resource a caller creates, so the sequence is
/// the entry's position in that order rather than a creation counter — it is what
/// [`page`] cuts pages at and what a `NextToken` names, exactly as a created
/// resource's [`SiteRecord::sequence`] is.
#[derive(Debug, Clone)]
pub struct OrderableInstanceTypeRecord {
    /// Position in the catalog's stable order. See [`SiteRecord::sequence`].
    pub sequence: u64,
    /// The entry as `ListOrderableInstanceTypes` reports it.
    pub instance_type: DetailedInstanceTypeItem,
}

impl OrderableInstanceTypeRecord {
    /// Whether an Outpost of `generation` can be ordered with this instance type:
    /// whether ANY of its `FormFactorConfigs` names that generation.
    ///
    /// `FormFactorConfigs` is a list of the (form factor, generation) combinations
    /// the instance type is offered in, so one entry naming the generation is what
    /// makes the entry orderable in it — the same "matches any of the values" rule
    /// every filter in this API follows.
    pub fn is_orderable_in(&self, generation: &str) -> bool {
        self.instance_type
            .form_factor_configs
            .iter()
            .flatten()
            .any(|config| config.outpost_generation.as_deref() == Some(generation))
    }
}

/// The whole orderable-instance-type catalog, expanded from [`ORDERABLE_FAMILIES`]
/// and ordered by instance type name.
///
/// Expanding the families here is what keeps every entry consistent with its
/// family's published ratio: `MemoryInMib` is the size's vCPU count times the
/// family's GiB per vCPU times [`MIB_PER_GIB`], never a per-size figure that could
/// disagree with it. The order is by instance type name so it is stable across
/// processes — a page token names a position in it, so the order a caller pages
/// through must not depend on how the table happens to be written.
pub fn orderable_instance_type_catalog() -> Vec<OrderableInstanceTypeRecord> {
    // The catalog may only ever name values the model declares, so the table is
    // checked against the GENERATED value sets rather than trusted to agree with
    // them: a model that renames a `FormFactor` or an `OutpostGeneration` trips this
    // in every debug build instead of letting the catalog report an unmodelled
    // member.
    debug_assert!(
        ORDERABLE_FAMILIES
            .iter()
            .flat_map(|family| family.form_factors)
            .all(|(form_factor, generation)| {
                crate::ops::types::FORM_FACTOR_VALUES.contains(form_factor)
                    && crate::ops::types::OUTPOST_GENERATION_VALUES.contains(generation)
            }),
        "every catalog configuration names a modelled FormFactor and OutpostGeneration"
    );
    let mut items: Vec<DetailedInstanceTypeItem> = ORDERABLE_FAMILIES
        .iter()
        .flat_map(|family| {
            family
                .sizes
                .iter()
                .map(move |(size, vcpus, network_performance)| DetailedInstanceTypeItem {
                    instance_type: Some(format!("{}.{size}", family.family)),
                    vcp_us: Some(*vcpus),
                    memory_in_mib: vcpus * family.memory_gib_per_vcpu * MIB_PER_GIB,
                    network_performance: Some((*network_performance).to_string()),
                    form_factor_configs: Some(
                        family
                            .form_factors
                            .iter()
                            .map(|(form_factor, generation)| FormFactorConfig {
                                form_factor: Some((*form_factor).to_string()),
                                outpost_generation: Some((*generation).to_string()),
                            })
                            .collect(),
                    ),
                })
        })
        .collect();
    items.sort_by(|left, right| left.instance_type.cmp(&right.instance_type));
    items
        .into_iter()
        .enumerate()
        .map(|(index, instance_type)| OrderableInstanceTypeRecord {
            sequence: u64::try_from(index).unwrap_or_default() + 1,
            instance_type,
        })
        .collect()
}

/// Per-service state for com-amazonaws-outposts-outpostsolafservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsOutpostsOutpostsolafserviceState {
    /// Every persisted site, keyed by its bare `SiteId` (`os-…`). A minted id is
    /// unique across the whole store, and a site belongs to the region it was
    /// created in, so `GetSite`, `CreateOutpost` and `DeleteSite` resolve one
    /// through [`Self::site`] rather than off this map directly.
    pub sites: BTreeMap<String, SiteRecord>,
    /// Every persisted Outpost, keyed by its bare `OutpostId` (`op-…`). Resolved
    /// through [`Self::outpost`]; see [`Self::sites`].
    pub outposts: BTreeMap<String, OutpostRecord>,
    /// Every persisted capacity task, keyed by its `CapacityTaskId` (`cap-…`).
    ///
    /// A task is reachable only under the Outpost it runs on, so the Outpost's own
    /// region already scopes it (see [`RegionalRecord`]) and `GetCapacityTask`
    /// resolves it by id once that parent has been resolved.
    pub capacity_tasks: BTreeMap<String, CapacityTaskRecord>,
    /// Every persisted quote, keyed by its bare `QuoteId` (`oq-…`). Resolved through
    /// [`Self::quote`]; see [`Self::sites`].
    pub quotes: BTreeMap<String, QuoteRecord>,
    /// Every persisted order, keyed by the `OrderId` (`oo-…`) `CreateOrder` minted
    /// for it. Resolved through [`Self::order`]; see [`Self::sites`].
    pub orders: BTreeMap<String, OrderRecord>,
    /// Every persisted Outpost-server installation connection, keyed by the
    /// `ConnectionId` `StartConnection` minted for it. Resolved through
    /// [`Self::connection`]; see [`Self::sites`].
    pub connections: BTreeMap<String, ConnectionRecord>,
    /// This store's own tunnel-server public key, or `None` until the first
    /// connection mints it. See [`Self::server_public_key`].
    pub connection_server_public_key: Option<String>,
    /// The orderable-instance-type catalog `ListOrderableInstanceTypes` reports, in
    /// its stable order.
    ///
    /// Reference data rather than a caller's resource: it is seeded in
    /// [`ComAmazonawsOutpostsOutpostsolafserviceState::default`] from
    /// [`orderable_instance_type_catalog`] and no operation mutates it, so every
    /// read of it answers the same catalog.
    pub orderable_instance_types: Vec<OrderableInstanceTypeRecord>,
    /// How many resources this store has created. Doubles as the monotonically
    /// increasing creation order every record records and every page token names,
    /// and as part of each minted id's seed so no two ids collide.
    pub created: u64,
}

/// The store a fresh engine hands a handler: no resources at all, and the
/// orderable-instance-type catalog already seeded, because that catalog is what the
/// service publishes rather than something a caller creates.
impl Default for ComAmazonawsOutpostsOutpostsolafserviceState {
    fn default() -> Self {
        Self {
            sites: BTreeMap::new(),
            outposts: BTreeMap::new(),
            capacity_tasks: BTreeMap::new(),
            quotes: BTreeMap::new(),
            orders: BTreeMap::new(),
            connections: BTreeMap::new(),
            connection_server_public_key: None,
            orderable_instance_types: orderable_instance_type_catalog(),
            created: 0,
        }
    }
}

impl ComAmazonawsOutpostsOutpostsolafserviceState {
    /// The site `site_id` names in `region`, or `None` when this region holds no
    /// such site. See [`RegionalRecord`].
    pub fn site(&self, region: &str, site_id: &str) -> Option<&SiteRecord> {
        in_region(&self.sites, region, site_id)
    }

    /// The same site, to CHANGE. See [`Self::site`].
    pub fn site_mut(&mut self, region: &str, site_id: &str) -> Option<&mut SiteRecord> {
        in_region_mut(&mut self.sites, region, site_id)
    }

    /// Remove the site `site_id` names in `region` and answer the record removed, or
    /// `None` — leaving the store untouched — when this region holds no such site.
    pub fn remove_site(&mut self, region: &str, site_id: &str) -> Option<SiteRecord> {
        in_region(&self.sites, region, site_id)?;
        self.sites.remove(site_id)
    }

    /// Every site of `region`, in the store's creation order — what `ListSites`
    /// pages through. See [`RegionalRecord`].
    pub fn sites_in<'a>(&'a self, region: &str) -> impl Iterator<Item = &'a SiteRecord> + 'a {
        all_in_region(&self.sites, region)
    }

    /// The Outpost `outpost_id` names in `region`. See [`Self::site`].
    pub fn outpost(&self, region: &str, outpost_id: &str) -> Option<&OutpostRecord> {
        in_region(&self.outposts, region, outpost_id)
    }

    /// The same Outpost, to CHANGE. See [`Self::outpost`].
    pub fn outpost_mut(&mut self, region: &str, outpost_id: &str) -> Option<&mut OutpostRecord> {
        in_region_mut(&mut self.outposts, region, outpost_id)
    }

    /// Remove the Outpost `outpost_id` names in `region`. See [`Self::remove_site`].
    pub fn remove_outpost(&mut self, region: &str, outpost_id: &str) -> Option<OutpostRecord> {
        in_region(&self.outposts, region, outpost_id)?;
        self.outposts.remove(outpost_id)
    }

    /// Every Outpost of `region`, in the store's creation order — what
    /// `ListOutposts` pages through. See [`RegionalRecord`].
    pub fn outposts_in<'a>(
        &'a self,
        region: &str,
    ) -> impl Iterator<Item = &'a OutpostRecord> + 'a {
        all_in_region(&self.outposts, region)
    }

    /// The quote `quote_id` names in `region`. See [`Self::site`].
    pub fn quote(&self, region: &str, quote_id: &str) -> Option<&QuoteRecord> {
        in_region(&self.quotes, region, quote_id)
    }

    /// The same quote, to CHANGE. See [`Self::quote`].
    pub fn quote_mut(&mut self, region: &str, quote_id: &str) -> Option<&mut QuoteRecord> {
        in_region_mut(&mut self.quotes, region, quote_id)
    }

    /// The connection `connection_id` names in `region`. See [`Self::site`].
    pub fn connection(&self, region: &str, connection_id: &str) -> Option<&ConnectionRecord> {
        in_region(&self.connections, region, connection_id)
    }

    /// The order `order_id` names in `region`. See [`Self::site`].
    pub fn order(&self, region: &str, order_id: &str) -> Option<&OrderRecord> {
        in_region(&self.orders, region, order_id)
    }

    /// Materialise every Outpost lifecycle transition the clock has already made
    /// due.
    ///
    /// Every operation that reports or filters on an Outpost calls this FIRST, so a
    /// read answers the status the Outpost holds at the instant it is asked for and
    /// never a stale one. It writes only what the clock already owed at that same
    /// instant, so reading changes nothing a later caller would not have seen
    /// anyway, and a settled Outpost is left alone.
    pub fn settle_outposts(&mut self) {
        let now = std::time::Instant::now();
        for record in self.outposts.values_mut() {
            if record.active_at.is_some_and(|due| now >= due) {
                record.outpost.life_cycle_status = Some(LIFE_CYCLE_STATUS_ACTIVE.to_string());
                record.active_at = None;
            }
        }
    }

    /// Materialise every capacity-task transition the clock has already made due.
    ///
    /// Every operation that reports a capacity task calls this FIRST, for the same
    /// reason [`Self::settle_outposts`] exists: a read answers the status the task
    /// holds at the instant it is asked for. It applies only what the clock already
    /// owed at that instant — every step whose due instant has passed, so a caller
    /// that polls late sees the same terminal status, with the same
    /// `LastModifiedDate` and `CompletionDate`, as one that polled all along — and a
    /// task that has settled in a terminal status is left alone.
    pub fn settle_capacity_tasks(&mut self) {
        let now = std::time::Instant::now();
        for record in self.capacity_tasks.values_mut() {
            while record.pending.front().is_some_and(|next| now >= next.due) {
                let Some(next) = record.pending.pop_front() else {
                    break;
                };
                record.last_modified_at = next.at;
                if next.status == CAPACITY_TASK_COMPLETED {
                    record.completed_at = Some(next.at);
                }
                record.status = next.status;
            }
        }
    }

    /// Materialise every quote expiry the clock has already made due.
    ///
    /// Every operation that reports a quote calls this FIRST, for the same reason
    /// [`Self::settle_outposts`] exists: a read answers the status the quote holds
    /// at the instant it is asked for. A quote expires when the
    /// `ExpirationDate` it itself reports has passed, so this writes only what the
    /// clock already owed at that same instant and nothing a later caller would not
    /// have seen anyway.
    ///
    /// Only a quote still in [`QUOTE_STATUS_CREATED`] is moved: the expiry of a
    /// quote an order was already submitted from would be rewriting history, and
    /// one already `EXPIRED` has nowhere left to go.
    pub fn settle_quotes(&mut self) {
        let now = now_timestamp().as_secs_f64();
        for record in self.quotes.values_mut() {
            let due = record
                .quote
                .expiration_date
                .as_ref()
                .is_some_and(|expiration| expiration.as_secs_f64() <= now);
            if due && record.quote.quote_status.as_deref() == Some(QUOTE_STATUS_CREATED) {
                record.quote.quote_status = Some(QUOTE_STATUS_EXPIRED.to_string());
            }
        }
    }

    /// Materialise every order transition the clock has already made due.
    ///
    /// Every operation that reports an order calls this FIRST, for the same reason
    /// [`Self::settle_outposts`] exists: a read answers the status the order holds at
    /// the instant it is asked for. It writes only what the clock already owed at
    /// that same instant, so reading changes nothing a later caller would not have
    /// seen anyway, and an order whose transition has been materialised is left
    /// alone.
    ///
    /// A line item moves with its order: the order's `IN_PROGRESS` is "being built or
    /// shipped. To get more details, see the line item status", so the item-level
    /// status is what carries that detail. Only an item still in the status the
    /// submit gave it is moved.
    pub fn settle_orders(&mut self) {
        let now = std::time::Instant::now();
        for record in self.orders.values_mut() {
            if record.in_progress_at.is_none_or(|due| now < due) {
                continue;
            }
            record.order.status = Some(ORDER_STATUS_IN_PROGRESS.to_string());
            for item in record.order.line_items.iter_mut().flatten() {
                if item.status.as_deref() == Some(LINE_ITEM_STATUS_PREPARING) {
                    item.status = Some(LINE_ITEM_STATUS_BUILDING.to_string());
                }
            }
            record.in_progress_at = None;
        }
    }

    /// The capacity task that is still ACTIVE on `outpost_id`, if any.
    ///
    /// "You can have one active capacity task for each order and each Outpost", so
    /// this is what `StartCapacityTask` refuses a second one against. Callers settle
    /// first: a task the clock has already completed holds nothing up.
    pub fn active_capacity_task_on_outpost(&self, outpost_id: &str) -> Option<&CapacityTaskRecord> {
        self.capacity_tasks
            .values()
            .find(|task| task.outpost_id == outpost_id && task.is_active())
    }

    /// The capacity task that is still ACTIVE on `order_id`, if any. See
    /// [`Self::active_capacity_task_on_outpost`] — the same rule, stated for the
    /// other resource a task is one-at-a-time per.
    pub fn active_capacity_task_on_order(&self, order_id: &str) -> Option<&CapacityTaskRecord> {
        self.capacity_tasks
            .values()
            .find(|task| task.order_id.as_deref() == Some(order_id) && task.is_active())
    }

    /// The Outpost that still belongs to `site_id` in `region`, if the store holds
    /// one.
    ///
    /// An Outpost names the site it lives at, so what this answers is whether that
    /// site still has hardware at it: the fact `DeleteSite` refuses a delete on and
    /// a change to the site's OPERATING address conflicts with. Stated once here so
    /// the two cannot disagree about what "the site still has an Outpost" means.
    pub fn outpost_at_site(&self, region: &str, site_id: &str) -> Option<&OutpostRecord> {
        self.outposts_in(region)
            .find(|record| record.outpost.site_id.as_deref() == Some(site_id))
    }

    /// Every `BlockingResourceType` this store observes still associated with
    /// `outpost_id`, deduplicated and in one stable order — what a `BLOCKED`
    /// `StartOutpostDecommission` reports in `BlockingResourceTypes`.
    ///
    /// The modelled enum names seven kinds of resource (see
    /// [`crate::ops::types::shared_types::BLOCKING_RESOURCE_TYPE_VALUES`]). Two of
    /// them are resources THIS service's store can hold. The first:
    ///
    /// * [`BLOCKING_OUTPOST_ORDER_CANCELLABLE`] — an Outpost order still outstanding
    ///   on the Outpost. This store holds two records of such an order: the order
    ///   itself, which `CreateOrder` persists against the Outpost it names and which
    ///   is outstanding until it is finished ([`OrderRecord::is_outstanding`]), and a
    ///   quote of this Outpost that an order was submitted from — the modelled
    ///   `QuoteStatus` [`QUOTE_STATUS_ORDER_SUBMITTED`], whose `SubmittedOrderId`
    ///   names it. Nothing in this slice moves an order past being prepared or built,
    ///   so an order the store knows about is one that can still be cancelled, and a
    ///   decommission has to wait for it.
    ///
    /// A second is a resource it CAN hold:
    ///
    /// * [`BLOCKING_EC2_INSTANCE`] — an Amazon EC2 instance still running on the
    ///   Outpost, which AWS requires to be gone before an Outpost can be
    ///   decommissioned. The store's record of such an instance is
    ///   [`OutpostRecord::instances`]; an Outpost nobody has launched an instance on
    ///   holds none, so it reports none. The instances a capacity task names in
    ///   `InstancesToExclude` are the caller's own input and never a record of a
    ///   running instance.
    ///
    /// The other five name resources that live nowhere in this store, so reporting one
    /// would be asserting a resource this emulator has no record of:
    ///
    /// * `OUTPOST_RAM_SHARE`, `LGW_ROUTING_DOMAIN`, `LGW_ROUTE_TABLE`,
    ///   `LGW_VIRTUAL_INTERFACE_GROUP` — resource shares and local gateway resources
    ///   belong to other services' APIs; nothing in this service creates or holds
    ///   one.
    /// * `OUTPOST_ORDER_INTERVENTION_REQUIRED` — an order condition nothing in this
    ///   slice produces: no order the store knows about ever fails or stalls, so the
    ///   emulator never claims one needs intervention.
    ///
    /// Callers settle first (see [`Self::settle_quotes`] and [`Self::settle_orders`]),
    /// so the statuses read here are the ones the quotes and orders hold at that
    /// instant.
    pub fn blocking_resource_types(&self, region: &str, outpost_id: &str) -> Vec<String> {
        let mut blocking: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let quote_ordered_from = all_in_region(&self.quotes, region).any(|record| {
            let quote_outpost = record
                .quote
                .outpost_arn
                .as_deref()
                .and_then(resolve_outpost_id);
            quote_outpost.as_deref() == Some(outpost_id)
                && record.quote.quote_status.as_deref() == Some(QUOTE_STATUS_ORDER_SUBMITTED)
        });
        let order_outstanding = all_in_region(&self.orders, region).any(|record| {
            record.order.outpost_id.as_deref() == Some(outpost_id) && record.is_outstanding()
        });
        if quote_ordered_from || order_outstanding {
            blocking.insert(BLOCKING_OUTPOST_ORDER_CANCELLABLE.to_string());
        }
        let instance_running = self
            .outpost(region, outpost_id)
            .is_some_and(|record| !record.instances.is_empty());
        if instance_running {
            blocking.insert(BLOCKING_EC2_INSTANCE.to_string());
        }
        blocking.into_iter().collect()
    }

    /// The tag map of the resource an Outposts resource ARN names, or `None` when
    /// this region's inventory holds no such resource (see [`RegionalRecord`]).
    ///
    /// A resource's tags live ON that resource, so every tagging operation reads
    /// and writes them through the stored record the ARN names — and it does so
    /// through this one place, so which resource kinds are taggable cannot differ
    /// between the operations that report tags and the ones that change them. A
    /// kind this store does not hold is `None`, which each of them answers as the
    /// not-found error it models.
    pub fn tags(
        &self,
        region: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Option<HashMap<String, String>> {
        let tags = match resource_type {
            SITE_RESOURCE_TYPE => &self.site(region, resource_id)?.site.tags,
            OUTPOST_RESOURCE_TYPE => &self.outpost(region, resource_id)?.outpost.tags,
            _ => return None,
        };
        // `None` from this function means NO SUCH RESOURCE. A resource whose
        // record carries no map carries no tags, which is an EMPTY map (see
        // [`tags_member`]), so the two cannot be confused.
        Some(tags.clone().unwrap_or_default())
    }

    /// The same tag map, to CHANGE: what `TagResource` writes into. See
    /// [`Self::tags`].
    ///
    /// The map is materialised when the stored record carries none, so a tag can
    /// be added to a resource created without any. That is not a change anybody
    /// can observe: [`tags_member`] already reports a resource with no tags as an
    /// empty map either way.
    pub fn tags_mut(
        &mut self,
        region: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Option<&mut HashMap<String, String>> {
        let tags = match resource_type {
            SITE_RESOURCE_TYPE => &mut self.site_mut(region, resource_id)?.site.tags,
            OUTPOST_RESOURCE_TYPE => &mut self.outpost_mut(region, resource_id)?.outpost.tags,
            _ => return None,
        };
        Some(tags.get_or_insert_with(HashMap::new))
    }

    /// Mint a fresh resource id of the form `<prefix>-<17 lowercase hex chars>`,
    /// and return it with the creation sequence to record on the new resource.
    ///
    /// `OutpostId` (`^op-[a-f0-9]{17}$`), `SiteId` (`^os-[a-f0-9]{17}$`),
    /// `QuoteId` (`^oq-[a-f0-9]{17}$`), `OrderId` (`^oo-[a-f0-9]{17}$`),
    /// `LineItemId` (`^ooi-[a-f0-9]{17}$`) and `CapacityTaskId`
    /// (`^cap-[a-f0-9]{17}$`) are each a short literal prefix and exactly 17
    /// lowercase hex characters, so one helper serves them all. `created` advances on every call and is hashed
    /// into the seed with the wall clock and the process id, so repeated calls never
    /// return the same id.
    pub fn mint_id(&mut self, prefix: &str) -> (String, u64) {
        let (digest, sequence) = self.mint_digest(prefix);
        let mut hex = hex::encode(&digest[..9]);
        // 9 bytes encode to 18 characters; the patterns want exactly 17.
        hex.truncate(17);
        (format!("{prefix}-{hex}"), sequence)
    }

    /// Mint a fresh `ConnectionId`, and return it with the creation sequence to
    /// record on the new connection.
    ///
    /// A `ConnectionId` is not a `<prefix>-<17 hex>` identifier like the rest of
    /// this service's: its pattern is base64-shaped
    /// (`^[a-zA-Z0-9+/=]{1,1024}$`), so the id is the padded base64 of the minted
    /// digest — 44 characters of that alphabet. See [`Self::mint_id`].
    pub fn mint_connection_id(&mut self) -> (String, u64) {
        let (digest, sequence) = self.mint_digest("connection");
        (::aws_smithy_types::base64::encode(&digest[..]), sequence)
    }

    /// This store's own tunnel-server public key, minted on first use and then
    /// stable for the life of the store.
    ///
    /// Every connection of one emulator terminates on the SAME tunnel server, so
    /// the key — like the endpoint beside it ([`CONNECTION_SERVER_HOST`]) — is a
    /// property of the store and not of a connection: a caller that starts two
    /// connections is told one server key twice. It is the padded base64 of a
    /// 32-byte digest, which is exactly the form the modelled `WireGuardPublicKey`
    /// declares: 43 characters of the base64 alphabet and the `=` that pads 32
    /// bytes.
    pub fn server_public_key(&mut self) -> String {
        self.connection_server_public_key
            .get_or_insert_with(|| {
                let seed = format!("wireguard-server|{}|{}", std::process::id(), seed_nanos());
                ::aws_smithy_types::base64::encode(&Sha256::digest(seed.as_bytes())[..])
            })
            .clone()
    }

    /// The Outpost of `region` holding the hardware asset `asset_id`, if this
    /// store holds one.
    ///
    /// An `AssetId` names one physical unit of hardware, installed in one rack of
    /// one Outpost, so at most one Outpost holds it: an operation that acts on an
    /// asset resolves it here rather than walking the store itself. The hardware of
    /// another region's Outpost is not this region's to act on (see
    /// [`RegionalRecord`]).
    pub fn outpost_with_asset(&self, region: &str, asset_id: &str) -> Option<&OutpostRecord> {
        self.outposts_in(region).find(|record| {
            record
                .assets
                .iter()
                .any(|asset| asset.asset_id.as_deref() == Some(asset_id))
        })
    }

    /// The asset `asset_id` of the Outpost `outpost_id`, if that Outpost holds one.
    ///
    /// An asset is a nested resource of exactly ONE Outpost — the modelled route is
    /// `/outposts/{OutpostIdentifier}/assets` and [`OutpostRecord::assets`] holds
    /// the hardware recorded for it — so an `AssetId` is resolved WITHIN the Outpost
    /// the request names: an operation scoped to an Outpost resolves the asset it
    /// was given here rather than walking the store itself, and `None` means that
    /// Outpost holds no such asset.
    pub fn asset_of_outpost(&self, outpost_id: &str, asset_id: &str) -> Option<&AssetInfo> {
        self.outposts
            .get(outpost_id)?
            .assets
            .iter()
            .find(|asset| asset.asset_id.as_deref() == Some(asset_id))
    }

    /// Whether this store holds the Amazon Web Services Outposts order `order_id`.
    ///
    /// An order is a TOP-LEVEL resource of the account (`CreateOrder` persists one
    /// and `GetOrder` reads it back at `/orders/{OrderId}`), so this asks only
    /// whether the account placed it. An operation that scopes its answer to the
    /// hardware an order is FOR needs the narrower question — whether the order was
    /// placed against the Outpost in hand — and asks it of
    /// [`OrderRecord::order`]'s own `OutpostId`.
    pub fn holds_order(&self, order_id: &str) -> bool {
        self.orders.contains_key(order_id)
    }

    /// Mint a fresh 32-byte digest for a resource of kind `kind`, with the creation
    /// sequence to record on it.
    ///
    /// `created` advances on every call and is hashed into the seed with the wall
    /// clock and the process id, so repeated calls never return the same digest.
    /// The two id forms this service mints ([`Self::mint_id`] and
    /// [`Self::mint_connection_id`]) are two renderings of this one digest.
    fn mint_digest(&mut self, kind: &str) -> ([u8; 32], u64) {
        self.created = self.created.wrapping_add(1);
        let seed = format!(
            "{kind}|{}|{}|{}",
            self.created,
            std::process::id(),
            seed_nanos()
        );
        let digest = Sha256::digest(seed.as_bytes());
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest);
        (bytes, self.created)
    }
}

/// Nanoseconds since the Unix epoch: the wall-clock part of every seed this store
/// hashes, so two mints of one process never agree.
fn seed_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOutpostsOutpostsolafserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOutpostsOutpostsolafserviceState>)
}

/// The region to mint this request's ARNs in: the region of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] when the request is unsigned.
///
/// A scope region is honoured only when it can appear in an Outposts ARN at all —
/// the modelled `SiteArn` / `OutpostArn` patterns pin that segment to `[a-z\d-]+`
/// — so a hostile or nonsense scope can never make this service mint an ARN its
/// own model rejects.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A `SiteArn`: `arn:aws:outposts:<region>:<account>:site/os-…`.
pub fn site_arn(region: &str, site_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{SITE_RESOURCE_TYPE}/{site_id}"),
    )
}

/// An `OutpostArn`: `arn:aws:outposts:<region>:<account>:outpost/op-…`.
pub fn outpost_arn(region: &str, outpost_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{OUTPOST_RESOURCE_TYPE}/{outpost_id}"),
    )
}

/// The bare resource id an Outposts identifier names, whether it arrived as the id
/// or as the full ARN.
///
/// Every identifier member in this API is modelled
/// `^(arn:…:<kind>/)?(<prefix>-[a-f0-9]{17})$` — the ARN prefix is optional — so one
/// resolver serves `SiteId`, `OutpostId` and the ARN forms alike: take the segment
/// after the last `/` when the value is an ARN, then require the bare id to be the
/// `<prefix>-` plus exactly 17 lowercase hex characters the pattern allows.
/// `None` means the value is not an identifier this service could have minted.
pub fn resolve_id(identifier: &str, prefix: &str) -> Option<String> {
    let bare = match identifier.starts_with("arn:") {
        true => identifier.rsplit('/').next()?,
        false => identifier,
    };
    let suffix = bare.strip_prefix(prefix)?.strip_prefix('-')?;
    let well_formed = suffix.len() == 17
        && suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    well_formed.then(|| bare.to_string())
}

/// The `os-…` id a `SiteId` names. See [`resolve_id`].
pub fn resolve_site_id(identifier: &str) -> Option<String> {
    resolve_id(identifier, "os")
}

/// The `op-…` id an `OutpostId` names. See [`resolve_id`].
pub fn resolve_outpost_id(identifier: &str) -> Option<String> {
    resolve_id(identifier, "op")
}

/// The `oq-…` id a `QuoteIdentifier` names. See [`resolve_id`].
pub fn resolve_quote_id(identifier: &str) -> Option<String> {
    resolve_id(identifier, "oq")
}

/// Now, as the modelled `ISO8601Timestamp` members of this API carry an instant.
///
/// One helper so every timestamp this service reports is read off the same clock
/// and in the same units, whether it is being stored or compared against a stored
/// one.
pub fn now_timestamp() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(std::time::SystemTime::now())
}

/// `at` as a modelled `ISO8601Timestamp`. See [`now_timestamp`].
pub fn timestamp(at: std::time::SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The `(resource type, resource id)` an Outposts resource ARN names, e.g.
/// `("site", "os-…")` for `arn:aws:outposts:<region>:<account>:site/os-…`.
///
/// Every `Arn` member in this API is modelled
/// `^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:([a-z\d-]+)/)[a-z]{2,8}-[a-f0-9]{17}$`,
/// so the resource field is always `<type>/<id>` and the service field is always
/// this service's own ARN namespace. `None` means the value is not an ARN of this
/// service at all.
pub fn resource_arn_parts(arn: &str) -> Option<(String, String)> {
    let mut fields = arn.strip_prefix("arn:")?.splitn(5, ':');
    let _partition = fields.next()?;
    let service = fields.next()?;
    let _region = fields.next()?;
    let _account = fields.next()?;
    let (resource_type, resource_id) = fields.next()?.split_once('/')?;
    (service == ARN_NAMESPACE).then(|| (resource_type.to_string(), resource_id.to_string()))
}

/// Which Availability Zone identifiers a request addressed to a region may name.
///
/// AWS names an Availability Zone in two ways, and both pin it to ONE region: the
/// zone NAME is the region code plus a zone letter (`us-east-1a`), while the zone
/// ID is the region's abbreviation plus `-az<n>` (`us-east-1` -> `use1-az1`), which
/// [`region_abbreviation`] derives from the region code itself. So an identifier
/// names a zone of a region only when it has one of those two forms FOR that
/// region: a request addressed to one region cannot name a zone of another, and a
/// value whose zone part names no zone at all — a name with no zone letter, an
/// `az<n>` outside the indices this API numbers zones by — names no zone of any
/// region.
///
/// Whether `zone` is an Availability Zone NAME of `region`: exactly the region
/// code, followed by a zone letter that names one of this region's zones (see
/// [`zone_name_parts`], which also keeps `us-east-1` from claiming `us-east-12a`).
pub fn zone_name_in_region(zone: &str, region: &str) -> bool {
    zone_name_parts(zone).is_some_and(|(named, _)| named == region.to_ascii_lowercase())
}

/// Whether `zone_id` is an Availability Zone ID of `region`: the region's
/// abbreviation followed by an `az<n>` that names one of its zones. A region
/// [`region_abbreviation`] cannot abbreviate is a region no zone id's REGION part
/// can be judged against, so that part is taken on trust there rather than refused
/// on a guess — the zone part is judged either way.
pub fn zone_id_in_region(zone_id: &str, region: &str) -> bool {
    zone_id_parts(zone_id).is_some_and(|(abbreviation, _)| {
        region_abbreviation(region).is_none_or(|expected| expected == abbreviation)
    })
}

/// The `(region code, zone index)` an Availability Zone NAME names, or `None` when
/// the value names no zone.
///
/// A zone name is a region code plus ONE zone letter, and the letters that name a
/// zone are the ones [`zone_index`] pairs with an index this API can number a zone
/// by — so a region code with no letter, or a letter past the last index, names no
/// zone.
fn zone_name_parts(zone: &str) -> Option<(String, u32)> {
    if !zone.is_ascii() || zone.is_empty() {
        return None;
    }
    let zone = zone.to_ascii_lowercase();
    let (region, letter) = zone.split_at(zone.len() - 1);
    let index = zone_index(*letter.as_bytes().first()?)?;
    Some((region.to_string(), index))
}

/// The `(region abbreviation, zone index)` an Availability Zone ID names, or `None`
/// when the value names no zone.
///
/// A zone id is `<region abbreviation>-az<n>` with the zones of a region numbered
/// from `az1`, so an index outside `1..=`[`MAX_ZONE_INDEX`] — `az0` above all —
/// names no zone of any region.
fn zone_id_parts(zone_id: &str) -> Option<(String, u32)> {
    if !zone_id.is_ascii() {
        return None;
    }
    let zone_id = zone_id.to_ascii_lowercase();
    let (abbreviation, index) = zone_id.split_once("-az")?;
    let index: u32 = index.parse().ok()?;
    (1..=MAX_ZONE_INDEX)
        .contains(&index)
        .then(|| (abbreviation.to_string(), index))
}

/// Why the Availability Zone identifiers a request named are not usable in
/// `region`, or `None` when they name one zone of it.
///
/// An Availability Zone belongs to exactly one region and is one of the zones that
/// region has (see [`zone_name_in_region`]), so a request naming a zone of another
/// region, or an identifier that names no zone at all, describes a resource that
/// cannot exist — a bad parameter, which this API answers as its
/// `ValidationException` (400). AWS states a cross-region zone as `<the value as
/// supplied> does not match your region.`
///
/// A value that names THIS region and no zone within it is refused too, but it is
/// not a cross-region mismatch, so it is not worded as one.
///
/// The zone ID is reported first: it is the region-stable identifier of a zone (a
/// zone NAME is per-account), so when a request names both and neither belongs to
/// the region, the ID is the identifier the refusal names.
///
/// Both identities name ONE zone ("You can specify either an Availability one or an
/// AZ ID"), so a request naming BOTH must name the same zone twice: see
/// [`zone_pair_refusal`].
pub fn availability_zone_refusal(
    region: &str,
    zone: Option<&str>,
    zone_id: Option<&str>,
) -> Option<String> {
    zone_id
        .and_then(|zone_id| zone_id_refusal(zone_id, region))
        .or_else(|| zone.and_then(|zone| zone_name_refusal(zone, region)))
        .or_else(|| zone_pair_refusal(zone, zone_id))
}

/// Why `zone` is not an Availability Zone name `region` can place a resource in,
/// or `None` when it is one of its zones.
fn zone_name_refusal(zone: &str, region: &str) -> Option<String> {
    match zone_name_parts(zone) {
        // It names a zone, so the only thing left to refuse is the region it is in.
        Some((named, _)) => (named != region.to_ascii_lowercase()).then(|| wrong_region(zone)),
        // It names no zone at all, so there is no region for it to mismatch.
        None => Some(names_no_zone(zone, region)),
    }
}

/// Why `zone_id` is not an Availability Zone ID `region` can place a resource in,
/// or `None` when it is one of its zones.
fn zone_id_refusal(zone_id: &str, region: &str) -> Option<String> {
    match zone_id_parts(zone_id) {
        Some((abbreviation, _)) => region_abbreviation(region)
            .filter(|expected| *expected != abbreviation)
            .map(|_| wrong_region(zone_id)),
        None => Some(names_no_zone(zone_id, region)),
    }
}

/// Why the two Availability Zone identities a request named do not name the same
/// zone, or `None` when they do — or when the request named at most one of them,
/// which names one zone by itself.
///
/// A zone's name and its id are one zone's two identities (a 1:1 mapping, which
/// this service fixes as the [`zone_index`] bijection and resolves the missing
/// identity with), so a pair the mapping does not pair describes a zone that cannot
/// exist. Either direction is enough to pair them, so a pair one direction cannot
/// map is still confirmed by the other. A pair NEITHER direction can map — both
/// identities in a region whose code carries no zone-ID abbreviation — is a pair
/// this service cannot read as two zones either, so it is taken on trust exactly as
/// the region part of such an id is ([`zone_id_in_region`]).
fn zone_pair_refusal(zone: Option<&str>, zone_id: Option<&str>) -> Option<String> {
    let (zone, zone_id) = (zone?, zone_id?);
    let named_id = availability_zone_id(zone);
    let identified_zone = availability_zone(zone_id);
    let same_zone = named_id
        .as_deref()
        .is_some_and(|resolved| resolved.eq_ignore_ascii_case(zone_id))
        || identified_zone
            .as_deref()
            .is_some_and(|resolved| resolved.eq_ignore_ascii_case(zone));
    let mapped = named_id.is_some() || identified_zone.is_some();
    (mapped && !same_zone).then(|| two_zones(zone, zone_id))
}

/// How this API words a zone of another region, as the recorded response does:
/// the value exactly as the request supplied it, and no more.
fn wrong_region(value: &str) -> String {
    format!("{value} does not match your region.")
}

/// How this service words a value that names the request's own region but no zone
/// within it — refused for the same reason (it is no zone of this region) but
/// stated separately, because calling it a region mismatch would be wrong.
fn names_no_zone(value: &str, region: &str) -> String {
    format!("{value} is not an Availability Zone of {region}.")
}

/// How this service words a pair of identities that name two zones: both values as
/// supplied, since either of them could be the one the caller meant.
fn two_zones(zone: &str, zone_id: &str) -> String {
    format!("{zone} and {zone_id} do not name the same Availability Zone.")
}

/// The `Site` projection of an operating address: the three members `Site`
/// declares for it, each read off the address the caller supplied.
pub fn operating_address_projection(
    address: Option<&Address>,
) -> (Option<String>, Option<String>, Option<String>) {
    match address {
        Some(address) => (
            Some(address.country_code.clone()),
            Some(address.state_or_region.clone()),
            Some(address.city.clone()),
        ),
        None => (None, None, None),
    }
}

/// A taggable resource's tag map as a response member: ALWAYS present, and empty
/// for a resource that carries no tags.
///
/// A resource's own tag map is part of it, so every projection of that resource
/// reports it — a resource with no tags has an empty map, not a missing member, and
/// a client that iterates the member gets an empty iteration rather than a null.
/// `TagMap`'s `length(min: 1)` constrains what a REQUEST may carry (the framework
/// validates input against the model before a handler runs); it does not turn a
/// resource's empty tag set into an absent member on the way out.
pub fn tags_member(tags: &HashMap<String, String>) -> Option<HashMap<String, String>> {
    Some(tags.clone())
}

/// How the Availability-Zone-ID region abbreviation renders each word an AWS region
/// code can carry, compound directions FIRST so `se` is read as `southeast` rather
/// than as `south` followed by `east`.
///
/// AWS writes an Availability Zone ID as `<region abbreviation>-az<n>`, where the
/// abbreviation is the region's two-letter area code, the initials of each word of
/// the rest of the region code, and the region number: the AWS documentation's
/// published examples are `use1-az1` (`us-east-1`), `usw2-az2` (`us-west-2`),
/// `apse2-az1` (`ap-southeast-2`) and `usgw1-az1` (`us-gov-west-1`). A word the
/// table does not name is abbreviated to its own first letter, which is the same
/// rule the published examples follow for `gov`.
const REGION_WORDS: &[(&str, &str)] = &[
    ("northeast", "ne"),
    ("northwest", "nw"),
    ("southeast", "se"),
    ("southwest", "sw"),
    ("central", "c"),
    ("north", "n"),
    ("south", "s"),
    ("east", "e"),
    ("west", "w"),
    ("gov", "g"),
    ("iso", "i"),
];

/// The highest `az<n>` index, and so the number of zone letters, this API can name:
/// the modelled `AvailabilityZoneId` pattern `^[a-zA-Z]+\d-[a-zA-Z]+\d$` ends in
/// exactly ONE digit.
const MAX_ZONE_INDEX: u32 = 9;

/// The Availability-Zone-ID abbreviation of an AWS region code (`us-west-2` ->
/// `usw2`), or `None` when the value is not a region code this rule can abbreviate
/// into the letters-then-one-digit prefix the modelled `AvailabilityZoneId` wants.
fn region_abbreviation(region: &str) -> Option<String> {
    let mut segments = region.split('-');
    let area = segments.next()?;
    if area.len() != 2 || !area.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let mut words: Vec<&str> = segments.collect();
    let number = words.pop()?;
    if number.len() != 1 || !number.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let mut abbreviation = area.to_string();
    for word in words {
        abbreviation.push_str(abbreviate_region_word(word)?);
    }
    abbreviation.push_str(number);
    Some(abbreviation)
}

/// The letters [`region_abbreviation`] renders one word of a region code as. See
/// [`REGION_WORDS`].
fn abbreviate_region_word(word: &str) -> Option<&str> {
    if word.is_empty() || !word.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    REGION_WORDS
        .iter()
        .find(|(name, _)| *name == word)
        .map(|(_, initials)| *initials)
        .or_else(|| word.get(..1))
}

/// The region code an Availability-Zone-ID abbreviation names (`usw2` ->
/// `us-west-2`): the inverse of [`region_abbreviation`], so the two agree on every
/// region whose words [`REGION_WORDS`] names.
fn region_from_abbreviation(abbreviation: &str) -> Option<String> {
    if !abbreviation.is_ascii() || abbreviation.len() < 4 {
        return None;
    }
    let (letters, number) = abbreviation.split_at(abbreviation.len() - 1);
    if !number.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let (area, mut rest) = letters.split_at(2);
    if !area.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let mut region = area.to_string();
    while !rest.is_empty() {
        let (word, initials) = REGION_WORDS
            .iter()
            .find(|(_, initials)| rest.starts_with(*initials))?;
        region.push('-');
        region.push_str(word);
        rest = &rest[initials.len()..];
    }
    region.push('-');
    region.push_str(number);
    Some(region)
}

/// The `az<n>` index the emulator pairs an Availability Zone's letter with.
///
/// AWS maps zone names to zone ids per account and publishes no formula for the
/// index, so this service fixes ONE stable bijection — `…a` <-> `…-az1`, `…b` <->
/// `…-az2`, and so on — and resolves in both directions with it, so a zone named
/// either way always resolves to the same pair.
fn zone_index(letter: u8) -> Option<u32> {
    let index = u32::from(letter.to_ascii_lowercase().checked_sub(b'a')?) + 1;
    (letter.is_ascii_alphabetic() && index <= MAX_ZONE_INDEX).then_some(index)
}

/// The Availability Zone letter an `az<n>` index names. The inverse of
/// [`zone_index`].
fn zone_letter(index: u32) -> Option<char> {
    (1..=MAX_ZONE_INDEX)
        .contains(&index)
        .then(|| char::from(b'a' + u8::try_from(index - 1).unwrap_or(0)))
}

/// The `AvailabilityZoneId` of the zone an `AvailabilityZone` name names
/// (`us-west-2b` -> `usw2-az2`), or `None` when the name carries no region and zone
/// letter to resolve one from.
pub fn availability_zone_id(zone: &str) -> Option<String> {
    let (region, index) = zone_name_parts(zone)?;
    Some(format!("{}-az{index}", region_abbreviation(&region)?))
}

/// The `AvailabilityZone` name of the zone an `AvailabilityZoneId` names
/// (`usw2-az2` -> `us-west-2b`): the inverse of [`availability_zone_id`].
pub fn availability_zone(zone_id: &str) -> Option<String> {
    let (abbreviation, index) = zone_id_parts(zone_id)?;
    let letter = zone_letter(index)?;
    Some(format!(
        "{}{letter}",
        region_from_abbreviation(&abbreviation)?
    ))
}

/// A string member the caller may have sent as the empty string, which carries no
/// value: every optional string in this API is modelled `length(min: 1)` except
/// `OutpostDescription`, so an empty one is omitted rather than stored.
pub fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

/// Apply one member of a partial (PATCH) update to the stored resource.
///
/// Every update in this API is modelled `@http(method: "PATCH")` with optional
/// members, so a member the request carries replaces the stored one and a member
/// it omits leaves the stored one exactly as it was. Shared by the three update
/// operations so they cannot drift apart on that rule.
pub fn apply<T>(stored: &mut Option<T>, update: Option<T>) {
    if let Some(value) = update {
        *stored = Some(value);
    }
}

// ---------------------------------------------------------------------------
// How this service words a refused request
//
// Two rules, each applied to every operation in this crate that states that
// class of failure, so no handler can word one its own way:
//
// * `NotFoundException` — the body is the IDENTIFIER ECHO
//   `<member>=<value as supplied>`. See [`identifier_not_found`].
// * `ValidationException` — the body is the standard AWS validation report, and
//   it never renders the offending value. See [`validation_report`].
// ---------------------------------------------------------------------------

/// How this service states that an identifier names no resource it holds.
///
/// The body is the identifier ECHO: the name of the input member the identifier
/// arrived in, `=`, and the value EXACTLY as the client sent it — so a caller
/// that sent the ARN form reads its ARN back, not the bare id the service
/// resolved it to, and a caller can parse the identifier it sent out of the
/// message. It is not an English sentence, and it never normalises the value.
///
/// `member` is the member's validation-path name (see [`SITE_ID_MEMBER`]);
/// `supplied` is the raw input member, before [`resolve_id`] touches it.
pub fn identifier_not_found(member: &str, supplied: &str) -> String {
    format!("{member}={supplied}")
}

/// The validation-path name of the `SiteId` input member.
///
/// Both message rules name a member the way AWS's validation report names one:
/// lowerCamel, which is the report's own casing convention applied to the Smithy
/// member name (`SiteId` -> `siteId`), not a fact of the model.
pub const SITE_ID_MEMBER: &str = "siteId";

/// The validation-path name of the `OutpostId` input member. See [`SITE_ID_MEMBER`].
pub const OUTPOST_ID_MEMBER: &str = "outpostId";

/// The validation-path name of the `CapacityTaskId` input member. See
/// [`SITE_ID_MEMBER`].
pub const CAPACITY_TASK_ID_MEMBER: &str = "capacityTaskId";

/// The validation-path name of the `NextToken` input member every list takes.
/// See [`SITE_ID_MEMBER`].
pub const NEXT_TOKEN_MEMBER: &str = "nextToken";

/// The validation-path name of the `QuoteIdentifier` input member the quote
/// operations bind. See [`SITE_ID_MEMBER`].
pub const QUOTE_IDENTIFIER_MEMBER: &str = "quoteIdentifier";

/// The validation-path name of the `OutpostIdentifier` input member the quote and
/// capacity-task operations take: a DIFFERENT member from the `OutpostId` the
/// Outpost operations bind, so a refusal about it names it as the request spelled
/// it. See [`SITE_ID_MEMBER`].
pub const OUTPOST_IDENTIFIER_MEMBER: &str = "outpostIdentifier";

/// The validation-path name of the `ResourceArn` input member every tagging
/// operation names a resource by. See [`SITE_ID_MEMBER`].
pub const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// The validation-path name of the `Tags` input member `TagResource` carries.
/// See [`SITE_ID_MEMBER`].
pub const TAGS_MEMBER: &str = "tags";

/// The validation-path name of the `AddressType` input member the two site-address
/// operations take. See [`SITE_ID_MEMBER`].
pub const ADDRESS_TYPE_MEMBER: &str = "addressType";

/// The validation-path name of the `AssetId` input member an operation scopes its
/// answer to one of an Outpost's assets by. See [`SITE_ID_MEMBER`].
pub const ASSET_ID_MEMBER: &str = "assetId";

/// The validation-path name of the `OrderId` input member an operation names an
/// Amazon Web Services Outposts order by. See [`SITE_ID_MEMBER`].
pub const ORDER_ID_MEMBER: &str = "orderId";

/// The validation-path name of the `LineItems` input member `CreateOrder` takes the
/// line items that make up an order in. See [`SITE_ID_MEMBER`].
pub const LINE_ITEMS_MEMBER: &str = "lineItems";

/// The validation-path name of the `ConnectionId` input member an operation names
/// an Outpost-server installation connection by. See [`SITE_ID_MEMBER`].
pub const CONNECTION_ID_MEMBER: &str = "connectionId";

/// How many entries the modelled `TagMap` holds (`@length(min: 1, max: 50)`).
///
/// It is the bound on a RESOURCE's tag map and not only on one request's: the
/// tags a resource carries are reported as that same `TagMap`
/// (`ListTagsForResource$Tags`, `Site$Tags`, `Outpost$Tags`), so a resource
/// holding more than this could not be reported within the model at all. A tag
/// change that would take a resource past it is therefore refused.
pub const TAG_MAP_MAX: i64 = 50;

/// The `@pattern` the model declares on `Arn`, the shape every tagging
/// operation's `ResourceArn` targets, verbatim. See [`SITE_ID_PATTERN`].
pub const RESOURCE_ARN_PATTERN: &str =
    r"^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:([a-z\d-]+)/)[a-z]{2,8}-[a-f0-9]{17}$";

/// The `@pattern` the model declares on `SiteId`, verbatim: a report names a
/// pattern exactly as the model spells it, anchors included.
pub const SITE_ID_PATTERN: &str =
    r"^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:site/)?(os-[a-f0-9]{17})$";

/// The `@pattern` the model declares on `OutpostId`. See [`SITE_ID_PATTERN`].
pub const OUTPOST_ID_PATTERN: &str =
    r"^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:outpost/)?op-[a-f0-9]{17}$";

/// The `@pattern` the model declares on `QuoteIdentifier`. See
/// [`SITE_ID_PATTERN`].
pub const QUOTE_IDENTIFIER_PATTERN: &str =
    r"^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:quote/)?oq-[a-f0-9]{17}$";

/// The `@pattern` the model declares on `OutpostIdentifier`, the member the quote
/// and capacity-task operations name an Outpost by. See [`SITE_ID_PATTERN`].
pub const OUTPOST_IDENTIFIER_PATTERN: &str =
    r"^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:outpost/)?op-[a-f0-9]{17}$";

/// The error code every refusal this service rewords answers with — the same
/// code the modelled `ValidationException` (HTTP 400) carries, so rewording the
/// body never moves a client off the code it branches on.
pub const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The text [`Violation::message`] puts between the offending value's position
/// and the requirement it failed.
///
/// The LAST occurrence is the real separator: a requirement never contains this
/// text, while a caller-supplied value interpolated before it might.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// One clause of the validation report: WHERE the offending value was, and WHAT
/// it had to satisfy. The value itself is never part of a clause.
pub fn clause(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// The standard AWS validation report around `clauses`: the count, then every
/// clause joined with `; `.
///
/// Callers pass at least one clause — a report with nothing to report is not a
/// refusal — and the plural arm renders whatever it is given rather than
/// panicking on an empty slice.
pub fn validation_report(clauses: &[String]) -> String {
    match clauses {
        [single] => format!("1 validation error detected: {single}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The clause a single [`Violation`] states under this service's rule.
///
/// The requirement is lifted out of the framework's OWN rendering of the
/// violation instead of being re-spelled here, so a constraint this service
/// reports is worded exactly as the boundary validator words it — the same
/// bound text, and a `@pattern` reproduced verbatim from the model. What this
/// drops is the rendering of the value: the report names the member and the
/// constraint only.
fn violation_clause(violation: &Violation) -> String {
    let rendered = violation.message();
    match rendered.rsplit_once(CONSTRAINT_SEPARATOR) {
        // A member that was ABSENT has no value to omit, and the framework's own
        // clause already says exactly that (`Value null at '<member>' …`), so it
        // stands unchanged.
        Some((_, requirement)) if violation.value.is_some() => {
            clause(&violation.path, requirement)
        }
        _ => rendered,
    }
}

/// The report for every constraint violation the boundary validator found,
/// in the order it found them. This is what `override_refusal` answers with.
pub fn refusal_report(violations: &[Violation]) -> String {
    let clauses: Vec<String> = violations.iter().map(violation_clause).collect();
    validation_report(&clauses)
}

/// The report for one member that failed `kind`, for a handler that has to state
/// a modelled constraint itself.
pub fn constraint_report(member: &str, kind: ViolationKind) -> String {
    validation_report(&[violation_clause(&Violation {
        path: member.to_string(),
        kind,
        // A PRESENT value selects the value-omitting clause rather than the
        // `Value null` one an absent member gets. What it holds is irrelevant:
        // the value never reaches the body.
        value: Some(String::new()),
    })])
}

/// The report a `SiteId` that is neither a site id nor a site ARN states.
pub fn site_id_pattern_report() -> String {
    constraint_report(
        SITE_ID_MEMBER,
        ViolationKind::Pattern(SITE_ID_PATTERN.to_string()),
    )
}

/// The report an `OutpostId` that is neither an Outpost id nor an Outpost ARN
/// states. See [`site_id_pattern_report`].
pub fn outpost_id_pattern_report() -> String {
    constraint_report(
        OUTPOST_ID_MEMBER,
        ViolationKind::Pattern(OUTPOST_ID_PATTERN.to_string()),
    )
}

/// How this service states that an Outpost (or an order) already has an active
/// capacity task, and so cannot take another.
///
/// AWS publishes no wording for this refusal, so the sentence is the emulator's
/// own; what the caller branches on is the `ConflictException` code (HTTP 409) and
/// the `ResourceId` / `ResourceType` members the model puts on it, which name the
/// resource the conflict is about. The message names the task already holding it,
/// so the caller can poll exactly that one.
pub fn capacity_task_conflict(resource_type: &str, resource_id: &str, active: &str) -> String {
    format!("{resource_type} {resource_id} already has an active capacity task: {active}")
}

/// The report an `AddressType` outside the modelled enum states, for a handler
/// that selects a site's address slot from it itself.
///
/// The boundary refuses such a value first — the modelled `AddressType` is an
/// enum, so its value set is validated before a handler runs — so this is the
/// modelled fallback, worded by the same rule as every other constraint this
/// service reports. See [`site_id_pattern_report`].
pub fn address_type_value_report() -> String {
    constraint_report(
        ADDRESS_TYPE_MEMBER,
        ViolationKind::EnumValue(
            crate::ops::types::ADDRESS_TYPE_VALUES
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        ),
    )
}

/// How this service states that a site's OPERATING address cannot be changed
/// while an Outpost still belongs to that site.
///
/// AWS documents the rule — "You can update the operating address before you
/// place an order at the site, or after all Outposts that belong to the site have
/// been deactivated" (`UpdateSiteAddress`) — but publishes no wording for the
/// refusal, so the sentence is the emulator's own; what a caller branches on is
/// the `ConflictException` code (HTTP 409) and the `ResourceId` / `ResourceType`
/// members naming the Outpost that holds the address, which is the one the caller
/// has to deactivate first.
pub fn site_address_conflict(site_id: &str, outpost_id: &str) -> String {
    format!(
        "the operating address of site {site_id} cannot be updated while Outpost \
         {outpost_id} belongs to it"
    )
}

/// The `ResourceType` a capacity-task conflict on an Outpost names, from the
/// modelled `ResourceType` enum (`ORDER | OUTPOST`).
pub const RESOURCE_TYPE_OUTPOST: &str = "OUTPOST";

/// The `ResourceType` a capacity-task conflict on an order names. See
/// [`RESOURCE_TYPE_OUTPOST`].
pub const RESOURCE_TYPE_ORDER: &str = "ORDER";

/// The report a `QuoteIdentifier` that is neither a quote id nor a quote ARN
/// states. See [`site_id_pattern_report`].
pub fn quote_identifier_pattern_report() -> String {
    constraint_report(
        QUOTE_IDENTIFIER_MEMBER,
        ViolationKind::Pattern(QUOTE_IDENTIFIER_PATTERN.to_string()),
    )
}

/// The report an `OutpostIdentifier` that is neither an Outpost id nor an Outpost
/// ARN states. See [`site_id_pattern_report`].
pub fn outpost_identifier_pattern_report() -> String {
    constraint_report(
        OUTPOST_IDENTIFIER_MEMBER,
        ViolationKind::Pattern(OUTPOST_IDENTIFIER_PATTERN.to_string()),
    )
}

/// The report an order that names neither the line items that make it up nor a quote
/// to take them from states.
///
/// `CreateOrder` models both members optional, but an order has to get its contents
/// from one of the two: an order is built out of catalog items, named either directly
/// or through the quote they were priced on. AWS publishes no wording for the refusal,
/// so the sentence is the emulator's own; what a caller branches on is the modelled
/// `ValidationException` code (HTTP 400). It is worded by the same rule as every
/// other constraint this service reports — the member and the requirement, never the
/// value.
pub fn order_contents_report() -> String {
    validation_report(&[clause(
        LINE_ITEMS_MEMBER,
        "either the line items that make up the order or the quote to take them from \
         must be specified",
    )])
}

/// How this service states that an order cannot be submitted from a quote in the
/// status that quote is in.
///
/// An order is submitted from a quote once: the modelled `QuoteStatus`
/// `ORDER_SUBMITTED` is "An order has been submitted for the quote" and `EXPIRED` is
/// a quote that can no longer be ordered from, so only a quote still `CREATED` can
/// take an order. AWS publishes no wording for the refusal, so the sentence is the
/// emulator's own; what a caller branches on is the modelled `ConflictException`
/// code (HTTP 409) and the `ResourceId` naming the quote. The message names the
/// status the quote is in, so the caller can tell an expired quote from one already
/// ordered from.
pub fn quote_status_conflict(quote_id: &str, status: &str) -> String {
    format!("an order cannot be submitted from quote {quote_id}, which is {status}")
}

/// The report a `ResourceArn` that is not an Outposts resource ARN states for a
/// handler that reads that ARN itself.
///
/// The boundary refuses such a value first, with the ARN-syntax code this service
/// names for it (`EmulatedService::override_refusal`), so this is the modelled
/// fallback a tagging handler states through the errors ITS model carries. See
/// [`site_id_pattern_report`].
pub fn resource_arn_pattern_report() -> String {
    constraint_report(
        RESOURCE_ARN_MEMBER,
        ViolationKind::Pattern(RESOURCE_ARN_PATTERN.to_string()),
    )
}

/// The report a tag change that would take a resource past [`TAG_MAP_MAX`]
/// states: the modelled `TagMap` maximum, named on the member the tags arrived
/// in, worded by the same rule as every other constraint this service reports and
/// rendering none of the tags themselves.
pub fn tag_map_limit_report() -> String {
    constraint_report(TAGS_MEMBER, ViolationKind::MaxLength(TAG_MAP_MAX))
}

/// The report an unusable pagination token states.
///
/// No modelled constraint covers it — the modelled `Token` pattern has already
/// passed by the time the token is decoded — so the requirement is this
/// service's own rule, stated in the report's grammar and naming `operation`,
/// the list the token had to belong to.
pub fn page_token_report(operation: &str) -> String {
    validation_report(&[clause(
        NEXT_TOKEN_MEMBER,
        &format!("Member must be a pagination token this service issued for {operation}"),
    )])
}

/// The stamp a `ListOutposts` page token carries, so a token issued for one list
/// cannot be replayed against another.
pub const OUTPOST_PAGE: &str = "outposts";

/// The stamp a `ListSites` page token carries. See [`OUTPOST_PAGE`].
pub const SITE_PAGE: &str = "sites";

/// The stamp a `ListOrderableInstanceTypes` page token carries. See [`OUTPOST_PAGE`].
pub const ORDERABLE_INSTANCE_TYPE_PAGE: &str = "instanceTypes";

/// The stamp a `ListAssets` page token carries.
///
/// An asset is not a top-level resource with a creation sequence of its own: it is
/// a unit of hardware installed on ONE Outpost, held in that Outpost's own
/// installation order ([`OutpostRecord::assets`]), so the boundary such a token
/// names is a position in THAT list and the route the token is replayed on names
/// the Outpost it belongs to. See [`OUTPOST_PAGE`].
pub const ASSET_PAGE: &str = "assets";

/// The stamp a `ListBlockingInstancesForCapacityTask` page token carries.
///
/// An instance is not a top-level resource with a creation sequence of its own
/// either: it runs on ONE Outpost, held in that Outpost's own recorded order
/// ([`OutpostRecord::instances`]), so the boundary such a token names is a position
/// in THAT list and the route it is replayed on names the Outpost (and the capacity
/// task) it belongs to. See [`ASSET_PAGE`].
pub const BLOCKING_INSTANCE_PAGE: &str = "blockingInstances";

/// The stamp a `ListAssetInstances` page token carries.
///
/// What it pages through is every instance recorded as running on one Outpost,
/// belonging to all accounts and narrowed by its own four filters — a different
/// list from the one `ListBlockingInstancesForCapacityTask` cuts out of the same
/// recorded order ([`BLOCKING_INSTANCE_PAGE`]), which reports only one account's
/// and drops what a capacity task excludes — so a token issued for one of them is
/// not a boundary of the other. See [`ASSET_PAGE`].
pub const ASSET_INSTANCE_PAGE: &str = "assetInstances";

/// The stamp a `GetOutpostSupportedInstanceTypes` page token carries.
///
/// What that report pages through is the instance-type catalog NARROWED to what one
/// Outpost's hardware can support, so it is a different list from the whole catalog
/// `ListOrderableInstanceTypes` pages ([`ORDERABLE_INSTANCE_TYPE_PAGE`]) even though
/// both name a boundary in the same catalog order — and a token issued for one of
/// them is therefore not a boundary of the other. See [`OUTPOST_PAGE`].
pub const SUPPORTED_INSTANCE_TYPE_PAGE: &str = "supportedInstanceTypes";

/// The page size a list uses when the caller named no `MaxResults`, which is also
/// the ceiling the model declares for it (`MaxResults1000`, `range(1..1000)`).
pub const MAX_RESULTS: i32 = 1000;

/// The ceiling on how many sites, how many Outposts, how many quotes, how many
/// orders, and how many capacity tasks this store holds.
///
/// A generated emulator keeps its resources in memory for the life of the process
/// and a create is the only thing that adds one, so the store needs a maximum entry
/// count: neither a TTL nor an eviction policy is available to it, because `GetSite`
/// must still answer what `CreateSite` created, and `GetCapacityTask` must still
/// answer a task that has long since completed. Past the cap the creating operation
/// refuses the request rather than growing: the three creates that model
/// `ServiceQuotaExceededException` (HTTP 402) answer it (it is modelled on exactly
/// them and on no read), `CreateQuote`, whose model declares no quota error, refuses
/// the create as a bad request (400 `ValidationException`) rather than inventing an
/// error the model does not carry, and `StartCapacityTask`, which models no quota
/// error either, answers the `ConflictException` (HTTP 409) it does model, worded by
/// [`store_full`] so the caller can tell it from the one-active-task refusal. The
/// number is the emulator's own bound, not a published AWS default (AWS publishes
/// these quotas as adjustable, so no single value is the contract), and is set high
/// enough that no ordinary workflow meets it.
pub const MAX_RESOURCES: usize = 5_000;

/// How this service states that a store has reached [`MAX_RESOURCES`] and cannot
/// take another resource of that kind.
///
/// The ceiling is the emulator's own, so no AWS rule fixes this wording; what the
/// caller branches on is the modelled code and status. The message names the
/// emulator, the kind of resource and the bound, so the refusal cannot be mistaken
/// for a statement about the caller's request or about a real AWS quota.
pub fn store_full(resource: &str) -> String {
    format!(
        "this emulator holds the maximum of {MAX_RESOURCES} {resource} it can keep in \
         memory, so it cannot take another"
    )
}

/// Encode a page boundary as an opaque `Token`.
///
/// The token carries the creation sequence of the LAST item the page returned, so
/// the next page is everything created strictly after it — a resume point that
/// stays correct when items are added or removed between pages, which a positional
/// offset would not. It is STAMPED with the list it belongs to, so a token replayed
/// against another list is refused rather than honoured as that list's boundary.
/// The rendering is exactly the modelled `Token` pattern `^(\d+)##(\S+)$`.
pub fn encode_page_token(kind: &str, last_sequence: u64) -> String {
    format!("{last_sequence}##{kind}")
}

/// The page boundary a `kind` token names, or `None` when it is not a token this
/// service issued for that list — which the caller is told as a
/// `ValidationException`, never as a silently truncated page.
pub fn decode_page_token(kind: &str, token: &str) -> Option<u64> {
    let (sequence, stamp) = token.split_once("##")?;
    if stamp != kind {
        return None;
    }
    sequence.parse().ok()
}

/// The page size a list serves, given the `MaxResults` the caller sent.
///
/// `MaxResults1000` is modelled `range(1..1000)`, so the framework has already
/// refused anything outside it; an absent value means the whole list up to that
/// same ceiling.
pub fn page_size(max_results: Option<i32>) -> usize {
    usize::try_from(max_results.unwrap_or(MAX_RESULTS))
        .unwrap_or(MAX_RESULTS as usize)
        .clamp(1, MAX_RESULTS as usize)
}

/// Cut one page out of an already-filtered sequence of records, and issue the token
/// that resumes after it.
///
/// Shared by both list operations so they cannot drift apart: the records are
/// ordered by creation sequence, one more than the page would fill is what proves a
/// further page exists, and the token names the last sequence this page returned.
pub fn page<T>(
    kind: &str,
    records: impl Iterator<Item = (u64, T)>,
    after: Option<u64>,
    size: usize,
) -> (Vec<T>, Option<String>) {
    let mut ordered: Vec<(u64, T)> = records
        .filter(|(sequence, _)| after.is_none_or(|boundary| *sequence > boundary))
        .collect();
    ordered.sort_by_key(|(sequence, _)| *sequence);
    let next_token = match ordered.len() > size {
        true => {
            ordered.truncate(size);
            ordered
                .last()
                .map(|(sequence, _)| encode_page_token(kind, *sequence))
        }
        false => None,
    };
    (
        ordered.into_iter().map(|(_, item)| item).collect(),
        next_token,
    )
}
