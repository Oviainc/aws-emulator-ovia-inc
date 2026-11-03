// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-outposts-outpostsolafservice service, plus
//! the rules more than one operation in the site + Outpost lifecycle applies: id
//! and ARN minting, scoping a resource to the region it was created in, resolving the
//! `id OR ARN` identifier forms the modelled
//! `@httpLabel` patterns accept — against the endpoint the request was addressed to,
//! since an ARN names a partition, a Region and an account of its own —
//! projecting a resource's tag map and an Outpost's
//! two Availability Zone identities, selecting a site's two addresses by the
//! modelled `AddressType` and keeping the `Site` projection of the operating one in
//! step with it, projecting an Outpost's private connectivity configuration
//! together with the IAM roles AWS Outposts uses for it,
//! materialising the Outpost lifecycle transition,
//! the quote expiry, the order's progress and the capacity-task progression the
//! clock owes, allocating the
//! tunnel an Outpost-server installation connection is reported as, cutting an
//! ordered list into pages with the modelled `Token` the next page resumes from, the
//! orderable-instance-type catalog and the catalog of orderable hardware
//! configurations this service publishes, the hardware an Outpost of a given form
//! factor is installed with, the instance types an Outpost's EC2 capacity is
//! configured with, weighing a requested capacity configuration against the instance
//! capacity available to the hardware it is asked of, pricing a subscription on EC2
//! capacity — the renewal of the one an
//! Outpost's orders hold, and recording the contract a renewal of it is bought
//! under, as well as the configuration-and-pricing options a quote is answered with
//! and the checks that must pass before an order can be submitted from it, and
//! the two rules this service words its refusals by — the per-resource-kind
//! `NotFoundException` wordings and the value-omitting `ValidationException`
//! report.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use sha2::{Digest, Sha256};
use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    Address, AssetInfo, AssetInstance, AssetInstanceTypeCapacity, AssetLocation,
    CapacityTaskFailure, CatalogItem,
    ComputeAttributes, ConnectionDetails, DetailedInstanceTypeItem, EC2Capacity, FormFactorConfig,
    InstanceTypeCapacity, InstanceTypeItem, InstancesToExclude, LineItem, LineItemAssetInformation,
    Order, OrderingRequirement,
    Outpost, PricingOption, PrivateConnectivityConfig, Quote, QuoteCapacity, QuoteOption, Site,
    SubscriptionPricingDetails, VpcInformation,
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
/// Minting an ARN ([`site_arn`]) and reading one back ([`resolve_resource_arn`])
/// name the kind through this one constant, so the two can never disagree about
/// what a site's ARN looks like.
pub const SITE_RESOURCE_TYPE: &str = "site";

/// The ARN resource TYPE an Outpost's ARN carries (`…:outpost/op-…`), from the
/// modelled `OutpostArn` pattern. See [`SITE_RESOURCE_TYPE`].
pub const OUTPOST_RESOURCE_TYPE: &str = "outpost";

/// The ARN resource TYPE a quote's ARN carries (`…:quote/oq-…`), from the ARN branch
/// of the modelled `QuoteIdentifier` pattern. See [`SITE_RESOURCE_TYPE`].
pub const QUOTE_RESOURCE_TYPE: &str = "quote";

/// The prefix a site id carries (`os-…`), as the bare-id branch of the modelled
/// `SiteId` pattern spells it.
///
/// Minting an id ([`ComAmazonawsOutpostsOutpostsolafserviceState::mint_id`]) and
/// reading one back ([`resolve_id`]) name the kind through this one constant, so the
/// two can never disagree about what a site id looks like.
pub const SITE_ID_PREFIX: &str = "os";

/// The prefix an Outpost id carries (`op-…`), from the bare-id branch of the modelled
/// `OutpostId` / `OutpostIdentifier` patterns. See [`SITE_ID_PREFIX`].
pub const OUTPOST_ID_PREFIX: &str = "op";

/// The prefix a quote id carries (`oq-…`), from the bare-id branch of the modelled
/// `QuoteIdentifier` pattern. See [`SITE_ID_PREFIX`].
pub const QUOTE_ID_PREFIX: &str = "oq";

/// The `LifeCycleStatus` a freshly created Outpost carries.
///
/// `CreateOutpost` records the Outpost hardware will be installed for; it holds no
/// hardware and no capacity of its own until an order placed on it delivers some
/// ([`order_progression`]), so the status a create reports — and every read reports
/// until the install that order's fulfilment owes falls due — is the non-terminal
/// one. The model constrains `LifeCycleStatus` without declaring a value set
/// (len 1..20, pattern `^[ A-Za-z]+$`), and both values this service uses satisfy it.
pub const LIFE_CYCLE_STATUS_PENDING: &str = "PENDING";

/// The `LifeCycleStatus` an Outpost settles in: the terminal, ready status it holds
/// once the hardware ordered for it has been installed, which no later call moves it
/// out of.
pub const LIFE_CYCLE_STATUS_ACTIVE: &str = "ACTIVE";

/// How long installing an Outpost's hardware takes: the span after which the
/// Outpost's [`LIFE_CYCLE_STATUS_PENDING`] -> [`LIFE_CYCLE_STATUS_ACTIVE`]
/// transition falls due, counted from the hardware reaching the site the Outpost
/// stands at ([`ORDER_STATUS_DELIVERED`]).
///
/// Nothing in this API installs hardware on request, so the span is never counted
/// from the call that created the Outpost: an Outpost is installed with what an order
/// placed on it delivers ([`order_progression`]), and one nobody has ordered hardware
/// for is owed no transition at all.
///
/// AWS publishes no duration for installing an Outpost, so this span is the
/// emulator's own: short enough that a workflow waiting for a usable Outpost finishes
/// at once, and long enough that a caller reading straight after the hardware arrived
/// still observes the pending status. It is never zero — a transition the clock owes
/// is owed at an instant, not on the next read.
pub const OUTPOST_PROVISIONING: std::time::Duration = std::time::Duration::from_secs(3);

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
pub const ORDER_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `OrderStatus` of an order whose hardware has reached the site it was shipped
/// to — the model's "`DELIVERED` - Order was delivered to the Outpost site".
///
/// Hardware at the site is hardware not yet in service, so the Outpost it was
/// ordered for is still being installed from here: the install falls due one
/// [`OUTPOST_PROVISIONING`] later, which is also where this order completes.
pub const ORDER_STATUS_DELIVERED: &str = "DELIVERED";

/// The `OrderStatus` of a cancelled order — the model's "`CANCELLED` - Order is
/// cancelled". `CancelOrder` is what moves an order into it ([`OrderRecord::cancel`]),
/// and it is one of the two statuses in which an order is FINISHED and so no longer
/// outstanding on its Outpost. See [`ORDER_STATUS_SETTLED`].
pub const ORDER_STATUS_CANCELLED: &str = "CANCELLED";

/// The `OrderStatus` of an order that ran to its end — the model's "`COMPLETED` -
/// Order is complete".
///
/// An order is complete when what it was placed for has happened: the hardware its
/// line items name is installed on the Outpost it was placed on. That install is the
/// one the Outpost reaches [`LIFE_CYCLE_STATUS_ACTIVE`] with, so the two fall due
/// together ([`ComAmazonawsOutpostsOutpostsolafserviceState::settle_orders`]), and it
/// is what makes this order's `OrderFulfilledDate` present. See also
/// [`ORDER_STATUS_SETTLED`].
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

/// The `LineItemStatus` a line item moves to with its order's
/// [`ORDER_STATUS_DELIVERED`]: the unit it names is at the site it was shipped to.
pub const LINE_ITEM_STATUS_DELIVERED: &str = "DELIVERED";

/// The `LineItemStatus` a line item moves to with its order's
/// [`ORDER_STATUS_COMPLETED`]: the unit it names is installed on the Outpost the
/// order was placed on, which is what completes the order.
pub const LINE_ITEM_STATUS_INSTALLED: &str = "INSTALLED";

/// The `LineItemStatus` of a line item of a cancelled order, the item-level
/// counterpart of [`ORDER_STATUS_CANCELLED`]: cancelling the order cancels the items
/// it was to deliver, so nothing is left being prepared or built for it. See
/// [`OrderRecord::cancel`].
pub const LINE_ITEM_STATUS_CANCELLED: &str = "CANCELLED";

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

/// How long after an order is being built and shipped its hardware reaches the site
/// it was shipped to: the order's [`ORDER_STATUS_IN_PROGRESS`] ->
/// [`ORDER_STATUS_DELIVERED`] transition.
///
/// AWS publishes no duration for building and shipping Outposts hardware, so this
/// span is the emulator's own, chosen for the reason [`ORDER_PREPARATION`]'s is and
/// longer than it, because what it stands for is the long leg of an order: a caller
/// that places an order and then works with the Outpost it was placed for observes
/// the hardware still on its way and the Outpost still pending, which is what the
/// real service answers for far longer still.
pub const ORDER_DELIVERY: std::time::Duration = std::time::Duration::from_secs(3);

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

    /// Whether the details an order placed at this site is placed AGAINST are
    /// recorded: the address the hardware is operated at and the address it is
    /// shipped to.
    ///
    /// Those are the site details the ordering requirements of this API name
    /// ([`ORDERING_REQUIREMENT_OPERATING_ADDRESS`],
    /// [`ORDERING_REQUIREMENT_SHIPPING_ADDRESS`]), so a site holding both carries
    /// everything an order needs of it and a site missing either is a site no order
    /// can be placed at yet — the one question both order paths ask of a site, asked
    /// the same way (see [`site_details_incomplete`]).
    ///
    /// It is the ADDRESS details only. The modelled `OrderingRequirementType` names a
    /// rack-properties check too, but a site's rack properties are the physical and
    /// logistical details of a RACK at it — every member of the modelled
    /// `RackPhysicalProperties` is optional and a site an Outpost server stands at
    /// needs none of them — so they are not read here, and neither is the postal-code
    /// check, which this service's addresses carry no validity rule for.
    pub fn details_complete(&self) -> bool {
        self.address(SiteAddressKind::Operating).is_some()
            && self.address(SiteAddressKind::Shipping).is_some()
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
    /// When the install this Outpost is owed falls due — its `PENDING` -> `ACTIVE`
    /// transition — or `None` while it is owed none, which is what a just-created
    /// Outpost is owed: nothing in this API installs hardware on request. Stored as an
    /// instant so nothing ever sleeps for it: the transition is applied by the first
    /// call made at or after it, which reports exactly the status the Outpost would
    /// hold had nobody called at all.
    ///
    /// An install is owed by the FULFILMENT of an order placed on this Outpost — the
    /// hardware that order delivers is what gets installed
    /// ([`ComAmazonawsOutpostsOutpostsolafserviceState::settle_orders`]) — and again
    /// whenever the form factor naming the configuration an installed Outpost holds
    /// changes ([`Self::set_supported_hardware_type`]).
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
    /// The hardware assets INSTALLED on this Outpost — what `ListAssets` reports
    /// for it — in the order they were installed, which is the order a page of
    /// them is cut in ([`ASSET_PAGE`]).
    ///
    /// Kept on the Outpost's own record for the reason
    /// [`OutpostRecord::private_connectivity`]
    /// is: an asset belongs to exactly ONE Outpost (the modelled route is
    /// `/outposts/{OutpostIdentifier}/assets`), so deleting the Outpost takes its
    /// assets with it.
    ///
    /// An asset is a PHYSICAL unit — a compute or storage server, a powershelf, a
    /// switch, a networking asset — that Amazon Web Services installs for the Outpost,
    /// so no call in this API creates one: `CreateOutpost` leaves this empty, and the
    /// hardware of the Outpost's own configuration is installed once an order placed on
    /// it has delivered hardware to the site
    /// ([`ComAmazonawsOutpostsOutpostsolafserviceState::settle_orders`],
    /// [`ComAmazonawsOutpostsOutpostsolafserviceState::settle_outposts`],
    /// [`installed_hardware`]) — the install it reaches a settled lifecycle status
    /// by. An Outpost whose form factor names no configuration this service holds is
    /// installed with nothing and reported as holding no asset. What is installed here
    /// is always the configuration the Outpost's CURRENT form factor names: a form
    /// factor recorded on it later names different hardware, which is installed in its
    /// turn ([`OutpostRecord::set_supported_hardware_type`]).
    pub assets: Vec<AssetInfo>,
    /// The creation sequence of the FIRST unit of the installation this Outpost holds,
    /// so the unit at position `n` of [`Self::assets`] was created
    /// `asset_sequence_base + n`th in this store ([`Self::sequenced_assets`]).
    ///
    /// An install takes a block of consecutive sequences out of the store's own
    /// creation order
    /// ([`ComAmazonawsOutpostsOutpostsolafserviceState::reserve_sequences`]) the
    /// moment it places the hardware, exactly as a created resource takes one, so no
    /// two units this store ever installs — on this Outpost or on any other — carry
    /// one sequence. That is what lets a page of [`ASSET_PAGE`] stop AT A UNIT rather
    /// than at a position: the hardware a replacement install places
    /// ([`Self::set_supported_hardware_type`]) is created after every sequence the
    /// store had handed out before it, so a boundary cut from the hardware that came
    /// out names no unit of what went in and the page that resumes from it drops
    /// nothing.
    pub asset_sequence_base: u64,
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
    /// The renewal contracts bought for this Outpost, in the order they were
    /// created.
    ///
    /// Kept on the Outpost's own record for the reason
    /// [`OutpostRecord::private_connectivity`] is: a renewal renews the
    /// subscription of exactly ONE Outpost, so it has no identity outside it and
    /// deleting the Outpost takes its renewals with it. Nothing else creates one:
    /// `CreateOutpost` leaves this empty, and `CreateRenewal` is the call that
    /// records a contract here.
    pub renewals: Vec<RenewalRecord>,
}

impl OutpostRecord {
    /// The `LifeCycleStatus` this Outpost reports NOW.
    ///
    /// An Outpost is registered in the non-terminal [`LIFE_CYCLE_STATUS_PENDING`] and
    /// reaches the terminal [`LIFE_CYCLE_STATUS_ACTIVE`] when the hardware ordered for
    /// it has been installed
    /// ([`ComAmazonawsOutpostsOutpostsolafserviceState::settle_outposts`]), so the
    /// member is never absent on a stored Outpost; a record that somehow carries none
    /// is read as the status a registered Outpost starts in rather than as an empty
    /// status no reader could act on.
    pub fn life_cycle_status(&self) -> &str {
        self.outpost
            .life_cycle_status
            .as_deref()
            .unwrap_or(LIFE_CYCLE_STATUS_PENDING)
    }

    /// Whether this Outpost has reached the terminal [`LIFE_CYCLE_STATUS_ACTIVE`] —
    /// hardware installed and in service.
    ///
    /// The one question every operation that acts on an Outpost's INSTALLED HARDWARE
    /// asks of it, so each asks it the same way.
    pub fn is_active(&self) -> bool {
        self.life_cycle_status() == LIFE_CYCLE_STATUS_ACTIVE
    }

    /// Every hardware asset installed on this Outpost, in installation order, each
    /// with the creation sequence that identifies the UNIT.
    ///
    /// What a page of this Outpost's assets stops at, so the number travels with the
    /// unit and not with the place the unit sits in: see
    /// [`Self::asset_sequence_base`] for why that is what a boundary of such a list
    /// has to be.
    pub fn sequenced_assets(&self) -> impl Iterator<Item = (u64, &AssetInfo)> {
        let base = self.asset_sequence_base;
        self.assets.iter().enumerate().map(move |(position, asset)| {
            (
                base.saturating_add(u64::try_from(position).unwrap_or_default()),
                asset,
            )
        })
    }

    /// Record `form_factor` as this Outpost's hardware type, and with it the install
    /// of the hardware that form factor names.
    ///
    /// `SupportedHardwareType` is not a label on an Outpost: it is the one input that
    /// selects WHICH configuration Amazon Web Services installs on it
    /// ([`installed_hardware`]), so the form factor an Outpost reports and the
    /// hardware it holds are recorded together and never apart. Naming the form
    /// factor the Outpost already carries names the configuration it already holds,
    /// so nothing is installed again; naming another one means the hardware on the
    /// Outpost is no longer the hardware it names, so that hardware comes out and the
    /// newly named configuration is installed in its turn — due
    /// [`OUTPOST_PROVISIONING`] from now, the span installing an Outpost's hardware
    /// takes, and materialised by
    /// [`ComAmazonawsOutpostsOutpostsolafserviceState::settle_outposts`] exactly as
    /// the install an order's fulfilment owes is. Until that instant the Outpost holds
    /// no asset, which is what an Outpost whose hardware is being installed holds. Its
    /// `LifeCycleStatus` is left where it stands: this installs hardware, and
    /// [`LIFE_CYCLE_STATUS_ACTIVE`] is terminal.
    ///
    /// An Outpost nothing has been installed on, and that is owed no install, has
    /// nothing to replace: the form factor it records names the configuration the order
    /// placed on it will install, so recording one neither takes hardware out nor owes
    /// an install, and the Outpost goes on waiting for the first order to deliver
    /// ([`ComAmazonawsOutpostsOutpostsolafserviceState::settle_orders`]). Whether
    /// anything has been installed is what the Outpost's lifecycle status says, so one
    /// whose form factor named no configuration at all — installed, and holding nothing
    /// — is one a newly named form factor does install hardware on.
    pub fn set_supported_hardware_type(&mut self, form_factor: String) {
        if self.outpost.supported_hardware_type.as_deref() == Some(form_factor.as_str()) {
            return;
        }
        self.outpost.supported_hardware_type = Some(form_factor);
        let installed = self.outpost.life_cycle_status.as_deref() == Some(LIFE_CYCLE_STATUS_ACTIVE);
        if !installed && self.active_at.is_none() {
            return;
        }
        self.assets.clear();
        self.active_at = Some(std::time::Instant::now() + OUTPOST_PROVISIONING);
    }
}

/// A renewal contract recorded against an Outpost: the client token the request
/// that bought it carried, and what it was bought at.
#[derive(Debug, Clone)]
pub struct RenewalRecord {
    /// The `ClientToken` of the request that created this contract, or `None` for
    /// one created by a request that carried no token.
    ///
    /// The member is modelled `@idempotencyToken`, so a repeat of the SAME request
    /// — one naming a token this Outpost already holds a contract under — is that
    /// one contract again rather than a second one. A request that names no token
    /// is not identified as a repeat of anything.
    pub client_token: Option<String>,
    /// What this renewal was bought at: the payment option and term it covers, the
    /// price that follows from the hardware under subscription at the instant it was
    /// created ([`ComAmazonawsOutpostsOutpostsolafserviceState::renewal_price`]),
    /// and the currency that price is quoted in.
    ///
    /// Held as the modelled [`SubscriptionPricingDetails`], which carries exactly
    /// those five members, so the one record of a renewal's price serves both the
    /// answer `CreateRenewal` gives and any later report of the subscription it
    /// bought.
    pub pricing: SubscriptionPricingDetails,
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

/// The `CapacityTaskStatus` of a task that cannot run on the capacity free to it
/// now — the model's "the capacity task requires capacity to run. You must stop the
/// recommended EC2 running instances to free up capacity for the task to run".
///
/// It is NOT terminal: Amazon Web Services Outposts is still waiting on the task, so
/// it is still ACTIVE on its Outpost. What it waits for is the instances
/// `ListBlockingInstancesForCapacityTask` recommends stopping being stopped, which an
/// instance's owner does through Amazon EC2 and no call of THIS API does — so the
/// clock owes such a task nothing: it holds this status until the instances are gone
/// or `CancelCapacityTask` ends it, and the emulator invents neither the evacuation
/// nor an outcome for one that never happened.
pub const CAPACITY_TASK_WAITING_FOR_EVACUATION: &str = "WAITING_FOR_EVACUATION";

/// The `CapacityTaskStatus` a cancelled capacity task holds while the service
/// cleans up after it — the model's "the capacity task has been cancelled and is in
/// the process of cleaning up resources".
///
/// It is NOT terminal: the task is one Amazon Web Services Outposts is still working
/// through, so it is still ACTIVE on its Outpost, and it settles in
/// [`CAPACITY_TASK_CANCELLED`] one [`CAPACITY_TASK_STEP`] later
/// ([`capacity_task_cancellation`]).
pub const CAPACITY_TASK_CANCELLATION_IN_PROGRESS: &str = "CANCELLATION_IN_PROGRESS";

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

// Which `CapacityTaskStatus` a task this emulator starts reaches, and off what,
// stated once so no handler invents one:
//
// * `COMPLETED`, `FAILED` and `WAITING_FOR_EVACUATION` are the three outcomes of
//   weighing the configuration the task asks for against the capacity available to
//   the hardware it configures
//   ([`ComAmazonawsOutpostsOutpostsolafserviceState::capacity_task_course`]), which
//   is the same weighing the DRY RUN of that request reports
//   ([`ComAmazonawsOutpostsOutpostsolafserviceState::capacity_verdict`]), so the two
//   never contradict each other. A `FAILED` task carries the `CapacityTaskFailure`
//   that weighing produced and never an invented one.
// * `REQUESTED` is what a just-started task holds, and `IN_PROGRESS` what a task
//   whose configuration fits holds while it runs
//   ([`capacity_task_progression`]).
// * `CANCELLATION_IN_PROGRESS` and `CANCELLED` are reached only by CANCELLING a
//   task ([`CapacityTaskRecord::cancel`]), never by the course a start owes the
//   clock.

/// The `CapacityTaskFailureType` of a capacity configuration the hardware it is
/// asked of cannot be configured with, from the modelled value set
/// [`crate::ops::types::CAPACITY_TASK_FAILURE_TYPE_VALUES`].
///
/// It is the failure type a request for more instance capacity than the hardware has
/// available carries: what the request asks for is not a configuration that hardware
/// can hold. The other four values the enum declares each describe something
/// happening to a task while it runs — a host in an unexpected state, instances that
/// were not evacuated, an internal error, a resource that went away — which is not
/// what weighing a plan against installed capacity answers.
pub const CAPACITY_TASK_FAILURE_UNSUPPORTED_CONFIGURATION: &str =
    "UNSUPPORTED_CAPACITY_CONFIGURATION";

/// The `CapacityTaskFailureType` of a task that could only have run on capacity held
/// by running instances it was not allowed to leave running, from the same modelled
/// value set [`crate::ops::types::CAPACITY_TASK_FAILURE_TYPE_VALUES`].
///
/// It is the failure type the modelled `FAIL_TASK` action carries: the hardware CAN
/// hold the configuration — so it is not the unsupported-configuration failure above
/// — but only once the instances
/// `ListBlockingInstancesForCapacityTask` recommends stopping have been stopped, and
/// `FAIL_TASK` is the caller saying the task fails instead of waiting for them.
pub const CAPACITY_TASK_FAILURE_BLOCKING_INSTANCES_NOT_EVACUATED: &str =
    "BLOCKING_INSTANCES_NOT_EVACUATED";

/// The `TaskActionOnBlockingInstances` that fails a capacity task instances are
/// blocking — the model's "`FAIL_TASK` - The capacity task fails" — from the modelled
/// value set [`crate::ops::types::TASK_ACTION_ON_BLOCKING_INSTANCES_VALUES`].
pub const TASK_ACTION_FAIL_TASK: &str = "FAIL_TASK";

/// The other `TaskActionOnBlockingInstances` the model declares: "`WAIT_FOR_EVACUATION`
/// - Checks every 10 minutes over 48 hours to determine if instances have stopped and
/// capacity is available to complete the task", so a task asked for it waits on those
/// instances ([`CAPACITY_TASK_WAITING_FOR_EVACUATION`]) instead of failing.
///
/// It is also what a request that named NO action asks for: the member is optional and
/// only `FAIL_TASK` asks for a failure, so a caller that named neither has not asked
/// for one.
pub const TASK_ACTION_WAIT_FOR_EVACUATION: &str = "WAIT_FOR_EVACUATION";

/// How long after the call that started it each step of a capacity task's course
/// falls due — the `REQUESTED` -> `IN_PROGRESS` -> `COMPLETED` progression of a
/// configuration that fits, and every other course a start can be weighed into
/// ([`ComAmazonawsOutpostsOutpostsolafserviceState::capacity_task_course`]).
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
    /// The `CapacityTaskFailure` the task carries from that instant on, for a
    /// transition INTO [`CAPACITY_TASK_FAILED`], and `None` for every other.
    ///
    /// The failure travels with the step that fails the task rather than being
    /// written on the record when the task is created, so a task still awaiting its
    /// next step reports no reason for a failure that has not happened
    /// ([`ComAmazonawsOutpostsOutpostsolafserviceState::settle_capacity_tasks`]
    /// applies it with the status).
    pub failed: Option<CapacityTaskFailure>,
}

/// The configuration one capacity task COMPLETING leaves on the hardware it named —
/// what [`ComAmazonawsOutpostsOutpostsolafserviceState::settle_capacity_tasks`]
/// collects while it walks the tasks and applies once it has.
#[derive(Debug, Clone)]
struct SettledConfiguration {
    /// The wall clock the completion was owed AT, which orders two tasks that
    /// configured one piece of hardware.
    at: std::time::SystemTime,
    /// The task's creation order, which orders two completions owed at one instant.
    /// See [`SiteRecord::sequence`].
    sequence: u64,
    /// The bare `OutpostId` of the Outpost whose hardware the task configured.
    outpost_id: String,
    /// The `AssetId` the task named, or `None` for one that named the Outpost itself.
    asset_id: Option<String>,
    /// The per-instance-type capacity the task leaves configured there.
    capacities: Vec<AssetInstanceTypeCapacity>,
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

    /// Whether this task can still be CANCELLED.
    ///
    /// A cancellation takes effect while Amazon Web Services Outposts has neither
    /// begun carrying the task out nor finished with it, which is what the modelled
    /// `CapacityTaskStatus` values say of themselves: `IN_PROGRESS` is "the capacity
    /// task is running and cannot be cancelled", a task in a terminal status
    /// ([`CAPACITY_TASK_TERMINAL`]) has nothing left to cancel, and one already
    /// cancelled is being cleaned up
    /// ([`CAPACITY_TASK_CANCELLATION_IN_PROGRESS`]) so there is nothing a second
    /// cancellation would change. Every other status is one the task is still
    /// awaiting a step in, and a cancellation is what it answers.
    pub fn is_cancellable(&self) -> bool {
        self.is_active()
            && self.status != CAPACITY_TASK_IN_PROGRESS
            && self.status != CAPACITY_TASK_CANCELLATION_IN_PROGRESS
    }

    /// Cancel this task: what `CancelCapacityTask` leaves behind.
    ///
    /// The task moves to [`CAPACITY_TASK_CANCELLATION_IN_PROGRESS`] — cancelled, and
    /// being cleaned up — and the progression the clock still owed it is REPLACED by
    /// the one a cancellation owes ([`capacity_task_cancellation`]), so a cancelled
    /// task never goes on to complete and settles in [`CAPACITY_TASK_CANCELLED`]
    /// instead. `CompletionDate` is "the date the capacity task ran successfully", so
    /// a cancellation leaves it exactly as it was: nothing ran.
    pub fn cancel(&mut self, now: std::time::SystemTime) {
        self.status = CAPACITY_TASK_CANCELLATION_IN_PROGRESS.to_string();
        self.last_modified_at = now;
        self.pending = capacity_task_cancellation(now);
    }
}

/// The transitions a capacity task whose configuration the hardware can hold owes
/// the clock from `now`: one [`CAPACITY_TASK_STEP`] to `IN_PROGRESS`, and another to
/// `COMPLETED`.
///
/// One of the courses
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::capacity_task_course`] chooses
/// between — the one a configuration that fits takes.
///
/// Both instants are fixed when the task starts, so the status a read reports is
/// the one the task holds at the instant it is asked for — the same answer whether
/// or not anybody polled in between.
pub fn capacity_task_progression(now: std::time::SystemTime) -> VecDeque<CapacityTaskTransition> {
    capacity_task_steps(
        now,
        &[CAPACITY_TASK_IN_PROGRESS, CAPACITY_TASK_COMPLETED],
        None,
    )
}

/// The transitions a capacity task that `failure` fails owes the clock from `now`:
/// one [`CAPACITY_TASK_STEP`] to `FAILED`, carrying that failure.
///
/// A failure is weighed before anything is configured, so such a task never reports
/// [`CAPACITY_TASK_IN_PROGRESS`] — the model's "the capacity task is running" — on
/// its way there. The step is still owed the clock rather than applied at once,
/// exactly as the fitting course's first step is: a caller reading straight after
/// the start observes the `REQUESTED` status the start reported.
pub fn capacity_task_failure(
    now: std::time::SystemTime,
    failure: CapacityTaskFailure,
) -> VecDeque<CapacityTaskTransition> {
    capacity_task_steps(now, &[CAPACITY_TASK_FAILED], Some(failure))
}

/// The transitions a capacity task that can run only on capacity its blocking
/// instances hold owes the clock from `now`: one [`CAPACITY_TASK_STEP`] to
/// [`CAPACITY_TASK_WAITING_FOR_EVACUATION`], and nothing after it.
///
/// Nothing after it because what such a task waits for is not the clock: it is the
/// recommended instances being stopped, which their owner does through Amazon EC2 and
/// no call of this API does. So the task holds that status until they are gone or
/// `CancelCapacityTask` ends it, and no outcome is invented for work nothing did.
pub fn capacity_task_evacuation_wait(
    now: std::time::SystemTime,
) -> VecDeque<CapacityTaskTransition> {
    capacity_task_steps(now, &[CAPACITY_TASK_WAITING_FOR_EVACUATION], None)
}

/// The transitions a capacity task cancelled at `now` owes the clock: one
/// [`CAPACITY_TASK_STEP`] of cleaning up, and then `CANCELLED`.
///
/// The same rule [`capacity_task_progression`] follows, for the other course a task
/// can take: a cancelled task is cleaning up resources
/// ([`CAPACITY_TASK_CANCELLATION_IN_PROGRESS`]) and settles in
/// [`CAPACITY_TASK_CANCELLED`] when that step falls due, so a caller reading straight
/// after the cancellation still observes the transient status.
pub fn capacity_task_cancellation(now: std::time::SystemTime) -> VecDeque<CapacityTaskTransition> {
    capacity_task_steps(now, &[CAPACITY_TASK_CANCELLED], None)
}

/// The transitions through `statuses`, each one [`CAPACITY_TASK_STEP`] after the
/// last, counted from `now`, with `failed` carried by the LAST of them — the step
/// that fails the task, for a course that ends in one.
///
/// Every course a task can take is built here, so none can disagree with another
/// about when a step falls due or about what a transition carries: the monotonic
/// instant nothing ever sleeps for, the wall clock the change is dated at, and the
/// failure the status it moves to reports.
fn capacity_task_steps(
    now: std::time::SystemTime,
    statuses: &[&str],
    failed: Option<CapacityTaskFailure>,
) -> VecDeque<CapacityTaskTransition> {
    let started = std::time::Instant::now();
    let last = statuses.len().saturating_sub(1);
    statuses
        .iter()
        .enumerate()
        .map(|(step, status)| {
            let elapsed = CAPACITY_TASK_STEP * (u32::try_from(step).unwrap_or(0) + 1);
            CapacityTaskTransition {
                due: started + elapsed,
                at: now + elapsed,
                status: (*status).to_string(),
                failed: (step == last).then(|| failed.clone()).flatten(),
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

/// What weighing a requested capacity configuration against the instance capacity
/// available to it answers: the `CapacityTaskStatus` the verdict is reported as, and
/// why the configuration cannot be run when it cannot.
///
/// Produced by
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::capacity_verdict`], which states
/// the rule; this carries the two members a report of it fills
/// (`CapacityTaskStatus` and `Failed`), so the status and the failure can never
/// disagree about whether the configuration fits.
#[derive(Debug, Clone)]
pub struct CapacityVerdict {
    /// The modelled `CapacityTaskStatus` the verdict is reported as:
    /// [`CAPACITY_TASK_COMPLETED`] for a configuration the hardware can take,
    /// [`CAPACITY_TASK_FAILED`] for one it cannot.
    pub status: &'static str,
    /// Why the configuration cannot be run, for a verdict that failed, and `None`
    /// for one that did not — exactly as `Failed` is reported.
    pub failed: Option<CapacityTaskFailure>,
}

impl CapacityVerdict {
    /// The verdict on a configuration the hardware it was asked of cannot take: the
    /// modelled [`CAPACITY_TASK_FAILURE_UNSUPPORTED_CONFIGURATION`] failure, worded
    /// by `reason`.
    fn unsupported(reason: String) -> Self {
        Self {
            status: CAPACITY_TASK_FAILED,
            failed: Some(CapacityTaskFailure {
                reason,
                r#type: Some(CAPACITY_TASK_FAILURE_UNSUPPORTED_CONFIGURATION.to_string()),
            }),
        }
    }

    /// The verdict on a configuration the hardware can take: no failure at all.
    fn fits() -> Self {
        Self {
            status: CAPACITY_TASK_COMPLETED,
            failed: None,
        }
    }
}

/// The capacity configuration one request asks of one piece of hardware: everything
/// weighing it reads off the REQUEST.
///
/// Stated once as a value because two readers weigh the same thing — the verdict a DRY
/// RUN reports ([`ComAmazonawsOutpostsOutpostsolafserviceState::capacity_verdict`]) and
/// the course a real task takes
/// ([`ComAmazonawsOutpostsOutpostsolafserviceState::capacity_task_course`]) — so
/// neither can be given a narrower request than the other.
#[derive(Debug, Clone, Copy)]
pub struct CapacityConfiguration<'a> {
    /// The region the request is addressed to, where the Outpost is resolved.
    pub region: &'a str,
    /// The bare `OutpostId` of the Outpost whose hardware is to be configured.
    pub outpost_id: &'a str,
    /// The `AssetId` the request names, which scopes the configuration to that one
    /// piece of the Outpost's hardware, or `None` for a request that names none.
    pub asset_id: Option<&'a str>,
    /// `RequestedInstancePools`: the configuration asked for.
    pub pools: &'a [InstanceTypeCapacity],
    /// `InstancesToExclude`: the running instances the task must not stop, so capacity
    /// they hold is capacity it cannot free.
    pub instances_to_exclude: Option<&'a InstancesToExclude>,
}

/// What weighing a requested capacity configuration against the hardware in its
/// scope measured, in the vCPUs every instance-capacity figure of this service is
/// published in: the one weighing both the DRY RUN's verdict
/// ([`ComAmazonawsOutpostsOutpostsolafserviceState::capacity_verdict`]) and the
/// course a real task takes
/// ([`ComAmazonawsOutpostsOutpostsolafserviceState::capacity_task_course`]) are read
/// off, so a dry run and the identical real request can never disagree about the
/// configuration.
///
/// Produced by
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::weigh_capacity`], which states the
/// rule.
#[derive(Debug, Clone)]
struct CapacityWeighing {
    /// Why the hardware in scope cannot hold the configuration AT ALL, whatever is
    /// free on it — a pool of an instance type it does not run — and `None` when
    /// there is no such pool.
    unsupported: Option<String>,
    /// What the configuration asks for.
    requested: i64,
    /// What the hardware has free with every instance recorded on it still running:
    /// its vCPU ceiling less the capacity all of them hold.
    free: i64,
    /// What it would have free once the instances this task MAY stop
    /// ([`instance_can_be_stopped`]) were stopped: its ceiling less only the capacity
    /// held by instances the task could not stop. Never below [`Self::free`].
    free_after_evacuation: i64,
}

impl CapacityWeighing {
    /// Whether the configuration is one the hardware in scope can be configured
    /// with: the DRY RUN's question, "whether the instance type and instance size
    /// changes is above or below available instance capacity", where what is
    /// available is what the task could have once the instances it may stop are
    /// stopped.
    fn verdict(&self) -> CapacityVerdict {
        if let Some(reason) = &self.unsupported {
            return CapacityVerdict::unsupported(reason.clone());
        }
        if self.requested > self.free_after_evacuation {
            return CapacityVerdict::unsupported(capacity_above_available(
                self.requested,
                self.free_after_evacuation,
            ));
        }
        CapacityVerdict::fits()
    }

    /// Whether the configuration fits only once the instances the task would need
    /// stopped have stopped: the hardware can hold it
    /// ([`Self::verdict`]), but not beside everything running on it now.
    ///
    /// Those instances are exactly the ones
    /// `ListBlockingInstancesForCapacityTask` recommends stopping, so a task is held
    /// up only by instances that list names, and never by an Outpost's other
    /// tenants' ones — capacity THEY hold is capacity the configuration was already
    /// weighed without.
    fn needs_evacuation(&self) -> bool {
        self.unsupported.is_none()
            && self.requested <= self.free_after_evacuation
            && self.requested > self.free
    }
}

/// The longest `CapacityTaskFailure$Reason` the model accepts
/// (`CapacityTaskStatusReason`, `length(min: 0, max: 128)`), which every reason this
/// service words fits by construction.
pub const CAPACITY_TASK_STATUS_REASON_MAX: usize = 128;

/// The instance FAMILY of an EC2 instance type: the part before the size, which is
/// what `ComputeAttributes$InstanceFamilies` names, or `None` for a value carrying no
/// size at all.
pub fn instance_type_family(instance_type: &str) -> Option<&str> {
    instance_type.split_once('.').map(|(family, _)| family)
}

/// Whether compute hardware that recorded `compute` RUNS an instance type of
/// `family` whose default size is `vcpus` vCPUs.
///
/// Two facts a `ComputeAttributes` records answer this, and one it did not record
/// narrows nothing — the emulator does not claim hardware is limited by a figure
/// nobody recorded:
///
/// * `InstanceFamilies`, the instance families the compute asset supports, so an
///   instance type of another family is not one that hardware runs;
/// * `MaxVcpus`, "the maximum number of vCPUs possible for the specified asset", so
///   an instance type larger than that ceiling does not fit on it.
///
/// Stated once here because two reports ask it: which instance types one asset of an
/// Outpost supports (`GetOutpostSupportedInstanceTypes`), and whether the pools a
/// capacity task requests name a configuration the hardware in its scope can hold
/// ([`ComAmazonawsOutpostsOutpostsolafserviceState::capacity_verdict`]).
pub fn compute_runs(compute: &ComputeAttributes, family: Option<&str>, vcpus: Option<i32>) -> bool {
    let in_recorded_family = match compute.instance_families.as_deref() {
        Some(families) if !families.is_empty() => {
            family.is_some_and(|family| families.iter().any(|recorded| recorded == family))
        }
        _ => true,
    };
    let within_vcpu_ceiling = compute
        .max_vcpus
        .is_none_or(|max| vcpus.is_none_or(|vcpus| vcpus <= max));
    in_recorded_family && within_vcpu_ceiling
}

/// One instance family a piece of hardware carries EC2 capacity of, and how large an
/// instance of that family it runs: the pair one `EC2Capacity` the catalog publishes
/// for a hardware configuration states.
///
/// The same two facts about hardware [`compute_runs`] reads off the
/// `ComputeAttributes` one installed compute asset records, as the published catalog
/// states them about a configuration — which is what lets a report scoped to ordered
/// hardware be narrowed by the same rule as one scoped to installed hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FamilyCapacity<'a> {
    /// The instance family this capacity is of, as `EC2Capacity$Family` names it.
    pub family: &'a str,
    /// The number of default vCPUs in "the maximum size of the EC2 capacity"
    /// (`EC2Capacity$MaxSize`), read off the one orderable-instance-type catalog
    /// ([`ComAmazonawsOutpostsOutpostsolafserviceState::published_vcpus`]) so no
    /// report of this service can disagree with it about a size.
    ///
    /// `None` for a size that catalog publishes no vCPU count for: a ceiling the
    /// store does not hold, which narrows nothing.
    pub max_vcpus: Option<i32>,
}

impl FamilyCapacity<'_> {
    /// Whether hardware carrying this capacity RUNS an instance type of `family`
    /// whose default size is `vcpus` vCPUs: one OF this family, and no larger than
    /// the largest size of it this capacity names. The two questions
    /// [`compute_runs`] asks of an installed asset, asked of published capacity.
    pub fn runs(&self, family: Option<&str>, vcpus: Option<i32>) -> bool {
        family == Some(self.family)
            && self
                .max_vcpus
                .is_none_or(|max| vcpus.is_none_or(|vcpus| vcpus <= max))
    }
}

/// Whether hardware whose recorded `AssetId` is `asset` — or an instance recorded as
/// running on that hardware — is within the SCOPE a capacity task that named `scope`
/// configures: the ONE asset the task named (`StartCapacityTask$AssetId`, "the ID of
/// the Outpost asset"), or every asset of the Outpost for a task that named none.
/// Hardware whose asset was never recorded is within no task's named scope.
///
/// Stated once here because every rule read off a task's scope asks it: which hardware
/// the requested configuration is weighed against, how much of that hardware's capacity
/// the task could have, and which instances it would need stopped
/// ([`instance_can_be_stopped`]) — capacity outside the scope is capacity the task
/// neither configures nor frees.
pub fn in_capacity_task_scope(scope: Option<&str>, asset: Option<&str>) -> bool {
    scope.is_none_or(|named| asset == Some(named))
}

/// Whether a capacity task scoped to `scope` ([`in_capacity_task_scope`]) that excludes
/// `exclude` would need `instance` STOPPED — so the capacity that instance holds is
/// capacity the task can free.
///
/// Three conditions, all from what AWS documents for the list of a task's blocking
/// instances:
///
/// * it runs on hardware the task configures. The list is there "to free up capacity to
///   run the capacity task", and a task may name one asset of the Outpost, so stopping
///   an instance running on another of that Outpost's assets frees no capacity the task
///   could use — it is not an instance the task would need stopped.
/// * it belongs to "the account that initiated the capacity task". A capacity task is
///   started by the caller of `StartCapacityTask`, and every request this emulator
///   serves is the one emulator account's (`engine::emulator_account_id`, the account
///   id `Outpost$OwnerId` and `Site$AccountId` carry), so that is the account a task
///   was initiated by. An instance recorded as another account's — one the Outpost is
///   shared with — is on the Outpost but not this caller's to stop. An instance whose
///   owning account was never recorded matches no account, so it is not one either.
/// * the task does not exclude it. `InstancesToExclude` is "user-specified instances
///   that must not be stopped. These instances will not appear in the list of
///   instances that Amazon Web Services recommends to stop in order to free up
///   capacity" — so an excluded instance is one the task cannot stop, however it was
///   named.
///
/// Stated once here because two rules read it: which instances
/// `ListBlockingInstancesForCapacityTask` recommends stopping, and how much of the
/// hardware's capacity a task could have
/// ([`ComAmazonawsOutpostsOutpostsolafserviceState::capacity_verdict`]).
pub fn instance_can_be_stopped(
    instance: &AssetInstance,
    scope: Option<&str>,
    exclude: &ExcludedInstances<'_>,
) -> bool {
    in_capacity_task_scope(scope, instance.asset_id.as_deref())
        && instance.account_id.as_deref() == Some(::engine::emulator_account_id())
        && !exclude.names(instance)
}

/// The instances a capacity task must not stop, read off the modelled
/// `InstancesToExclude` ONCE so that asking about one instance is a lookup rather
/// than a walk of what the caller sent.
///
/// Both callers of [`instance_can_be_stopped`] ask about every instance recorded for
/// an Outpost, so reading the caller's lists per instance would cost the product of
/// the two; built once, the work a request causes is its own length plus a lookup per
/// recorded instance.
#[derive(Debug, Default)]
pub struct ExcludedInstances<'a> {
    /// `InstancesToExclude$Instances`: the instance ids that must not be stopped.
    instances: BTreeSet<&'a str>,
    /// `InstancesToExclude$AccountIds`: "IDs of the accounts that own each instance
    /// that must not be stopped".
    account_ids: BTreeSet<&'a str>,
    /// `InstancesToExclude$Services`: "names of the services that own each instance
    /// that must not be stopped".
    services: BTreeSet<&'a str>,
}

impl<'a> ExcludedInstances<'a> {
    /// What `exclude` names, as the three sets above. A member the request never
    /// carried names nothing, exactly as an empty one does.
    pub fn of(exclude: Option<&'a InstancesToExclude>) -> Self {
        let set = |values: &'a Option<Vec<String>>| -> BTreeSet<&'a str> {
            values
                .iter()
                .flatten()
                .map(std::string::String::as_str)
                .collect()
        };
        match exclude {
            Some(exclude) => Self {
                instances: set(&exclude.instances),
                account_ids: set(&exclude.account_ids),
                services: set(&exclude.services),
            },
            None => Self::default(),
        }
    }

    /// Whether these exclusions name `instance` through ANY of the three sub-lists.
    ///
    /// The model gives `InstancesToExclude` three of them, each naming instances that
    /// must not be stopped by a different attribute: by instance id, by owning account
    /// and by owning service. They are alternatives, so an instance any one of them
    /// names is excluded; a sub-list that is absent (or empty) excludes nothing, and a
    /// member an instance has no recorded value for is named by no list.
    fn names(&self, instance: &AssetInstance) -> bool {
        let named = |excluded: &BTreeSet<&str>, value: Option<&String>| {
            value.is_some_and(|value| excluded.contains(value.as_str()))
        };
        named(&self.instances, instance.instance_id.as_ref())
            || named(&self.account_ids, instance.account_id.as_ref())
            || named(&self.services, instance.aws_service_name.as_ref())
    }
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

impl QuoteRecord {
    /// The Outpost this quote is ASSOCIATED with, as that Outpost's own `op-…` id.
    ///
    /// The association is recorded as the Outpost's `OutpostArn` ([`Quote::outpost_arn`]),
    /// so the id is read back out of it ([`outpost_id_in_arn`]); a quote associated with
    /// no Outpost has none. One accessor, so every caller that has to say WHICH Outpost
    /// a quote belongs to — the submit of an order from it, and the record this store
    /// keeps of an order outstanding on an Outpost — reads the same value.
    pub fn associated_outpost_id(&self) -> Option<&str> {
        self.quote
            .outpost_arn
            .as_deref()
            .and_then(outpost_id_in_arn)
    }

    /// Whether an order has ALREADY been submitted from this quote.
    ///
    /// An order is submitted from a quote once, and that submit is what moves the
    /// quote to [`QUOTE_STATUS_ORDER_SUBMITTED`] — "An order has been submitted for
    /// the quote" — and names the order it minted in the quote's `SubmittedOrderId`.
    /// One accessor, so every caller that has to say whether a quote is a RECORD of an
    /// order — the second record this store keeps of an order outstanding on an
    /// Outpost, and the update that may not rewrite what such an order was submitted
    /// under — reads the same value.
    pub fn records_submitted_order(&self) -> bool {
        self.quote.quote_status.as_deref() == Some(QUOTE_STATUS_ORDER_SUBMITTED)
    }

    /// Whether `quote_option_identifier` is one of the configuration-and-pricing
    /// options THIS quote offers.
    ///
    /// A `QuoteOptionIdentifier` is minted with the quote's options and persisted with
    /// the quote ([`ComAmazonawsOutpostsOutpostsolafserviceState::quote_options`]), so
    /// the options this quote reports are the only ones an order can be submitted
    /// against.
    pub fn offers_option(&self, quote_option_identifier: &str) -> bool {
        self.quote
            .quote_options
            .iter()
            .flatten()
            .any(|option| option.quote_option_identifier.as_deref() == Some(quote_option_identifier))
    }
}

/// One piece of UNFINISHED WORK this store holds on a resource: what it is, and the
/// resource a caller is told the conflict it causes is about.
///
/// Produced by
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::unfinished_work_on_outpost`] and
/// rendered by [`unfinished_work_conflict`], so the refusal's `ResourceId` /
/// `ResourceType` members and its message are read off one value.
#[derive(Debug, Clone)]
pub struct UnfinishedWork {
    /// The modelled `ResourceType` (`ORDER` | `OUTPOST`) of the resource causing
    /// the conflict — [`RESOURCE_TYPE_OUTPOST`] or [`RESOURCE_TYPE_ORDER`].
    pub resource_type: &'static str,
    /// That resource's own id, which is what the refusal's `ResourceId` carries.
    pub resource_id: String,
    /// The work itself, as the refusal message names it, so the caller knows what
    /// to wait for or cancel.
    pub work: String,
}

/// The order placed at a site that a change to that site has to wait for, as a
/// refusal names it.
///
/// Produced by
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::site_order_hold`] so every
/// operation whose contract makes a change to a site wait for an order in progress
/// reports the same resource and names the same order, the way [`UnfinishedWork`]
/// does for a delete.
#[derive(Debug, Clone)]
pub struct SiteOrderHold {
    /// The resource the change waits on, as the refusal names it: the modelled
    /// `ResourceType` [`RESOURCE_TYPE_ORDER`] and the order's own id, together
    /// because one without the other names nothing. Absent when the stored order
    /// carries no id this store can name it by.
    pub resource: Option<(&'static str, String)>,
    /// How the refusal's message names the order the caller has to let complete or
    /// cancel.
    pub order: String,
}

/// What holds a change to one of a site's two addresses up, if anything.
///
/// Produced by
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::site_address_block`] so the
/// refusal's `ResourceId` / `ResourceType` members and its message are read off one
/// value, the way [`UnfinishedWork`] does for a delete.
#[derive(Debug, Clone)]
pub struct SiteAddressBlock {
    /// The resource the change waits on, as the refusal names it: its modelled
    /// `ResourceType` ([`RESOURCE_TYPE_ORDER`] or [`RESOURCE_TYPE_OUTPOST`]) and its
    /// own id, together because one without the other names nothing. Absent when
    /// the blocking resource carries no id this store can name it by.
    pub resource: Option<(&'static str, String)>,
    /// The sentence the refusal states, naming what the caller has to let finish,
    /// cancel or deactivate first.
    pub message: String,
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

/// One transition an order owes the clock: the `OrderStatus` it moves to, and the
/// `LineItemStatus` its line items move to with it — the order's own status says the
/// hardware is being built or shipped, and "to get more details, see the line item
/// status".
#[derive(Debug, Clone)]
pub struct OrderTransition {
    /// When the transition falls due, as a monotonic instant: nothing ever sleeps
    /// for it, the first call made at or after it materialises it.
    pub due: std::time::Instant,
    /// The wall clock the transition is owed AT, which is what the fulfilment of an
    /// order reports as its `OrderFulfilledDate` — so a late read reports when the
    /// order really was fulfilled, not when it was read.
    pub at: std::time::SystemTime,
    /// The `OrderStatus` the order holds from that instant on.
    pub status: String,
    /// The `LineItemStatus` every line item still in flight holds from that instant
    /// on.
    pub line_item_status: String,
}

/// The transitions an order submitted at `now` owes the clock: one
/// [`ORDER_PREPARATION`] to being built and shipped, one [`ORDER_DELIVERY`] to
/// reaching the site, and one [`OUTPOST_PROVISIONING`] to being installed, which is
/// what completes the order.
///
/// Every instant is fixed when the order is submitted, so the status a read reports
/// is the one the order holds at the instant it is asked for — the same answer
/// whether or not anybody polled in between — exactly as
/// [`capacity_task_progression`] fixes a capacity task's.
pub fn order_progression(now: std::time::SystemTime) -> VecDeque<OrderTransition> {
    let submitted = std::time::Instant::now();
    let mut elapsed = std::time::Duration::ZERO;
    let mut progression = VecDeque::new();
    for (span, status, line_item_status) in [
        (
            ORDER_PREPARATION,
            ORDER_STATUS_IN_PROGRESS,
            LINE_ITEM_STATUS_BUILDING,
        ),
        (
            ORDER_DELIVERY,
            ORDER_STATUS_DELIVERED,
            LINE_ITEM_STATUS_DELIVERED,
        ),
        (
            OUTPOST_PROVISIONING,
            ORDER_STATUS_COMPLETED,
            LINE_ITEM_STATUS_INSTALLED,
        ),
    ] {
        elapsed += span;
        progression.push_back(OrderTransition {
            due: submitted + elapsed,
            at: now + elapsed,
            status: status.to_string(),
            line_item_status: line_item_status.to_string(),
        });
    }
    progression
}

/// A persisted order: the modelled [`Order`] plus its creation order and the
/// transitions the clock still owes it.
#[derive(Debug, Clone)]
pub struct OrderRecord {
    /// See [`SiteRecord::sequence`].
    pub sequence: u64,
    /// The region this order was placed in. See [`RegionalRecord`].
    pub region: String,
    /// The order as `CreateOrder` / `GetOrder` return it.
    pub order: Order,
    /// The site the Outpost this order was placed on belongs to, as the Outpost
    /// named it when the order was submitted.
    ///
    /// An order is placed AT a site through an Outpost of it, and what AWS holds a
    /// site address change for is an order in progress at that site
    /// ([`ComAmazonawsOutpostsOutpostsolafserviceState::outstanding_order_at_site`]),
    /// so the site travels on the order rather than being re-derived from an Outpost
    /// record that may no longer be there. `Order` carries no site member of its own,
    /// so this is the only place it is kept; it is absent only for an order whose
    /// Outpost named no site.
    pub site_id: Option<String>,
    /// The transitions this order still owes the clock, soonest first, and empty once
    /// every one of them has been materialised. Stored as instants for the reason
    /// [`OutpostRecord::active_at`] is: nothing ever sleeps for them, and the first
    /// call made at or after one reports exactly the status the order would hold had
    /// nobody called at all.
    pub pending: VecDeque<OrderTransition>,
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

    /// Cancel this order: what `CancelOrder` leaves behind.
    ///
    /// The order settles in [`ORDER_STATUS_CANCELLED`] — one of the two statuses in
    /// which it is finished ([`ORDER_STATUS_SETTLED`]), so it is outstanding on its
    /// Outpost no longer — and every line item of it settles in the item-level status
    /// of the same name: the order's status is "Order is cancelled" and the items are
    /// what it was to deliver, so none of them is still being prepared or built.
    /// The transitions the clock still owed this order are dropped with it: a cancelled
    /// order never goes on to be built, delivered or installed, so [`Self::pending`] is
    /// cleared rather than left to fire against a finished order — and the Outpost it
    /// was placed on is installed with nothing, because nothing was ever delivered to
    /// it.
    pub fn cancel(&mut self) {
        self.order.status = Some(ORDER_STATUS_CANCELLED.to_string());
        for item in self.order.line_items.iter_mut().flatten() {
            item.status = Some(LINE_ITEM_STATUS_CANCELLED.to_string());
        }
        self.pending.clear();
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

    /// Whether this instance type is offered on `form_factor` hardware of
    /// `generation`: whether ANY of its `FormFactorConfigs` names that PAIR.
    ///
    /// The narrower question [`Self::is_orderable_in`] asks of the generation alone,
    /// asked of one whole configuration — which is what tells which instance types a
    /// given piece of hardware runs (see [`catalog_item_catalog`]).
    pub fn is_offered_as(&self, form_factor: &str, generation: &str) -> bool {
        self.instance_type
            .form_factor_configs
            .iter()
            .flatten()
            .any(|config| {
                config.form_factor.as_deref() == Some(form_factor)
                    && config.outpost_generation.as_deref() == Some(generation)
            })
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

/// The `CatalogItemClass` an Outposts RACK configuration is published as, from the
/// modelled value set [`crate::ops::types::CATALOG_ITEM_CLASS_VALUES`] (`RACK` |
/// `SERVER`).
///
/// A class names the same kind of hardware the modelled `FormFactor` does
/// ([`FORM_FACTOR_RACK`]), which is what lets a configuration's class select the
/// instance types an Outpost of that hardware runs (see [`catalog_item_catalog`]).
pub const CATALOG_ITEM_CLASS_RACK: &str = "RACK";

/// The `CatalogItemClass` an Outposts SERVER configuration is published as. See
/// [`CATALOG_ITEM_CLASS_RACK`].
pub const CATALOG_ITEM_CLASS_SERVER: &str = "SERVER";

/// The `CatalogItemStatus` a configuration that can still be ordered carries, from
/// the modelled value set [`crate::ops::types::CATALOG_ITEM_STATUS_VALUES`]
/// (`AVAILABLE` | `DISCONTINUED`).
pub const CATALOG_ITEM_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `CatalogItemStatus` a configuration that is no longer offered carries.
///
/// A discontinued item stays IN the catalog — it is still what an Outpost already
/// installed was ordered as — so a read of the catalog reports it; the operation
/// models no status filter to leave it out with. See
/// [`CATALOG_ITEM_STATUS_AVAILABLE`].
pub const CATALOG_ITEM_STATUS_DISCONTINUED: &str = "DISCONTINUED";

/// The Amazon EBS storage option, from the modelled `SupportedStorageEnum` value set
/// [`crate::ops::types::SUPPORTED_STORAGE_ENUM_VALUES`] (`EBS` | `S3`).
pub const SUPPORTED_STORAGE_EBS: &str = "EBS";

/// The Amazon S3 storage option. See [`SUPPORTED_STORAGE_EBS`].
pub const SUPPORTED_STORAGE_S3: &str = "S3";

/// The bytes one terabyte of published S3 on Outposts capacity holds.
///
/// AWS publishes the S3 on Outposts capacity options of a rack in decimal
/// terabytes, so that is the unit [`CatalogConfiguration::s3_capacity_bytes`]
/// converts from.
const PUBLISHED_TERABYTE: i64 = 1_000_000_000_000;

/// The `RackScalingType` of a configuration that scales as one rack, from the
/// modelled value set [`crate::ops::types::RACK_SCALING_TYPE_VALUES`]
/// (`SINGLE_RACK` | `MULTI_RACK`), which `CatalogItem$RackScalingType` documents as
/// its valid values.
pub const RACK_SCALING_TYPE_SINGLE_RACK: &str = "SINGLE_RACK";

/// The `RackScalingType` of a configuration that scales across racks. See
/// [`RACK_SCALING_TYPE_SINGLE_RACK`].
pub const RACK_SCALING_TYPE_MULTI_RACK: &str = "MULTI_RACK";

/// The literal prefix every `SkuCode` carries, as the modelled pattern
/// `^OR-[A-Z0-9]{7}$` spells it.
const SKU_PREFIX: &str = "OR-";

/// How many characters of [`SKU_ALPHABET`] follow that prefix, from the same pattern.
const SKU_BODY: usize = 7;

/// The alphabet a `SkuCode`'s body is drawn from, from the same pattern's
/// `[A-Z0-9]`.
const SKU_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// The `SkuCode` a hardware configuration is published under, derived from `seed`:
/// the configuration's own facts.
///
/// A SKU is not a resource a caller creates, so it is DERIVED rather than minted —
/// the same configuration is published under the same code in every process, which
/// is what lets a caller that read a SKU out of the catalog read that item back by
/// it later. The rendering is exactly the modelled `SkuCode` pattern.
fn sku_code(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let body: String = digest
        .iter()
        .take(SKU_BODY)
        .map(|byte| char::from(SKU_ALPHABET[usize::from(*byte) % SKU_ALPHABET.len()]))
        .collect();
    format!("{SKU_PREFIX}{body}")
}

/// One hardware configuration in the catalog this service publishes: what it is,
/// what it draws and weighs, how it attaches to the Region, which storage it
/// supports and how much EC2 capacity it carries.
struct CatalogConfiguration {
    /// The `CatalogItemClass` the configuration is published as.
    item_class: &'static str,
    /// The `OutpostGeneration` of its hardware, which together with `item_class`
    /// selects the instance types it runs. Not reported: the modelled `CatalogItem`
    /// carries neither the class nor the generation, only what they imply.
    generation: &'static str,
    /// The `CatalogItemStatus` the configuration is published in.
    item_status: &'static str,
    /// `PowerKva`: the power the configuration draws.
    power_kva: f32,
    /// `WeightLbs`: what the configuration weighs.
    weight_lbs: i32,
    /// `SupportedUplinkGbps`: the uplink speeds it supports for the connection to
    /// the Region.
    supported_uplink_gbps: &'static [i32],
    /// `SupportedStorage`: the storage options it supports.
    supported_storage: &'static [&'static str],
    /// How much Amazon S3 storage capacity, in bytes, this configuration carries —
    /// zero for one that supports no S3 storage, which therefore has no S3 capacity
    /// to provision on an Outpost ordered from it.
    ///
    /// Held beside `supported_storage` because that member says WHICH storage
    /// options the hardware supports and never how much of each: the figure is the
    /// one an S3 on Outposts bucket's capacity is drawn from, which no member of this
    /// API reports. The values are AWS's published S3 on Outposts capacity options
    /// (26, 48, 96, 240 and 380 TB), one per S3-capable configuration in order of the
    /// EC2 capacity it carries — this emulator's own catalog assignment, exactly as
    /// which configurations are offered at all is.
    s3_capacity_bytes: i64,
    /// `RackScalingType`, or `None` for a configuration rack scaling does not apply
    /// to — a single server is not a rack, so it scales as neither one rack nor
    /// several, and the member is omitted rather than given a value the hardware has
    /// no answer for.
    rack_scaling_type: Option<&'static str>,
    /// How many vCPUs of EC2 capacity the configuration carries, which is what the
    /// `Quantity` of each of its EC2 capacities is computed from (see
    /// [`ec2_capacities`]). Not reported: the modelled `CatalogItem` reports the
    /// capacity per family rather than one total.
    capacity_vcpus: i32,
}

impl CatalogConfiguration {
    /// The seed [`sku_code`] derives this configuration's `CatalogItemId` from:
    /// every fact that distinguishes it from another row, so two different
    /// configurations are never published under one SKU.
    fn sku_seed(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{:?}|{:?}|{}",
            self.item_class,
            self.generation,
            self.item_status,
            self.power_kva,
            self.weight_lbs,
            self.capacity_vcpus,
            self.supported_uplink_gbps,
            self.supported_storage,
            self.rack_scaling_type.unwrap_or_default(),
        )
    }
}

/// The catalog of orderable hardware configurations this service publishes.
///
/// Three sources meet in these rows. The uplink speeds are the modelled `UplinkGbps`
/// set (`UPLINK_1G` | `UPLINK_10G` | `UPLINK_40G` | `UPLINK_100G`), read as the
/// Gbps figures the member's own `SupportedUplinkGbps` integers report. A rack's
/// power draws are the modelled `PowerDrawKva` set (`POWER_5_KVA` …
/// `POWER_30_KVA`), which is what a site declares it can provide at a rack position,
/// and a rack's weight stays inside the modelled `MaximumSupportedWeightLbs` set
/// (`MAX_1400_LBS` … `MAX_2000_LBS`, above which the model's own `NO_LIMIT` begins)
/// so a published rack is one a site can declare support for. WHICH configurations
/// are offered — their storage options, how much EC2 capacity each carries and which
/// are still available — is this emulator's own catalog assignment, exactly as
/// [`ORDERABLE_FAMILIES`]'s generation assignment is: AWS prices a configuration per
/// quote rather than publishing a machine-readable catalog. A server is not a rack,
/// so its power and weight are the hardware's own rather than a rack position's.
const CATALOG_ITEMS: &[CatalogConfiguration] = &[
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 5.0,
        weight_lbs: 1400,
        supported_uplink_gbps: &[1, 10, 40, 100],
        supported_storage: &[SUPPORTED_STORAGE_EBS, SUPPORTED_STORAGE_S3],
        s3_capacity_bytes: 26 * PUBLISHED_TERABYTE,
        rack_scaling_type: Some(RACK_SCALING_TYPE_SINGLE_RACK),
        capacity_vcpus: 480,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 10.0,
        weight_lbs: 1600,
        supported_uplink_gbps: &[10, 40, 100],
        supported_storage: &[SUPPORTED_STORAGE_EBS, SUPPORTED_STORAGE_S3],
        s3_capacity_bytes: 48 * PUBLISHED_TERABYTE,
        rack_scaling_type: Some(RACK_SCALING_TYPE_SINGLE_RACK),
        capacity_vcpus: 960,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 15.0,
        weight_lbs: 1800,
        supported_uplink_gbps: &[10, 40, 100],
        supported_storage: &[SUPPORTED_STORAGE_EBS, SUPPORTED_STORAGE_S3],
        s3_capacity_bytes: 240 * PUBLISHED_TERABYTE,
        rack_scaling_type: Some(RACK_SCALING_TYPE_MULTI_RACK),
        capacity_vcpus: 1440,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_DISCONTINUED,
        power_kva: 5.0,
        weight_lbs: 1400,
        supported_uplink_gbps: &[1, 10],
        supported_storage: &[SUPPORTED_STORAGE_EBS],
        s3_capacity_bytes: 0,
        rack_scaling_type: Some(RACK_SCALING_TYPE_SINGLE_RACK),
        capacity_vcpus: 240,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_2,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 10.0,
        weight_lbs: 1800,
        supported_uplink_gbps: &[10, 40, 100],
        supported_storage: &[SUPPORTED_STORAGE_EBS, SUPPORTED_STORAGE_S3],
        s3_capacity_bytes: 96 * PUBLISHED_TERABYTE,
        rack_scaling_type: Some(RACK_SCALING_TYPE_SINGLE_RACK),
        capacity_vcpus: 1280,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_RACK,
        generation: OUTPOST_GENERATION_2,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 30.0,
        weight_lbs: 2000,
        supported_uplink_gbps: &[40, 100],
        supported_storage: &[SUPPORTED_STORAGE_EBS, SUPPORTED_STORAGE_S3],
        s3_capacity_bytes: 380 * PUBLISHED_TERABYTE,
        rack_scaling_type: Some(RACK_SCALING_TYPE_MULTI_RACK),
        capacity_vcpus: 2560,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_SERVER,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 0.5,
        weight_lbs: 50,
        supported_uplink_gbps: &[1, 10],
        supported_storage: &[SUPPORTED_STORAGE_EBS],
        s3_capacity_bytes: 0,
        rack_scaling_type: None,
        capacity_vcpus: 64,
    },
    CatalogConfiguration {
        item_class: CATALOG_ITEM_CLASS_SERVER,
        generation: OUTPOST_GENERATION_1,
        item_status: CATALOG_ITEM_STATUS_AVAILABLE,
        power_kva: 1.0,
        weight_lbs: 70,
        supported_uplink_gbps: &[1, 10],
        supported_storage: &[SUPPORTED_STORAGE_EBS],
        s3_capacity_bytes: 0,
        rack_scaling_type: None,
        capacity_vcpus: 128,
    },
];

/// One entry of the catalog of items: the modelled [`CatalogItem`] as a read of the
/// catalog reports it, plus the entry's position in the catalog's stable order and
/// the one fact a filter selects on that the projection does not carry.
///
/// The catalog is REFERENCE data, not a resource a caller creates, so the sequence is
/// the entry's position in that order rather than a creation counter — exactly as
/// [`OrderableInstanceTypeRecord::sequence`] is.
#[derive(Debug, Clone)]
pub struct CatalogItemRecord {
    /// Position in the catalog's stable order. See [`SiteRecord::sequence`].
    pub sequence: u64,
    /// The `CatalogItemClass` of this item: what `ListCatalogItems$ItemClassFilter`
    /// selects on.
    ///
    /// Held BESIDE the projection because the modelled `CatalogItem` carries no
    /// class member: the class narrows which items an answer holds, and is never
    /// reported.
    pub item_class: String,
    /// How many vCPUs of EC2 capacity this item's hardware carries.
    ///
    /// Held BESIDE the projection for the reason [`Self::item_class`] is: the
    /// modelled `CatalogItem` reports the capacity per instance family rather than
    /// one total ([`ec2_capacities`]). It is the published size of the hardware,
    /// which is what a subscription on it is priced by (see
    /// [`ComAmazonawsOutpostsOutpostsolafserviceState::renewal_pricing`]).
    pub capacity_vcpus: i32,
    /// How much Amazon S3 storage capacity, in bytes, this item's hardware carries.
    ///
    /// Held BESIDE the projection for the reason [`Self::capacity_vcpus`] is: the
    /// modelled `CatalogItem` reports WHICH storage options an item supports and
    /// never how much of each. It is the published S3 capacity of the hardware,
    /// which is what an Outpost ordered from this item has provisioned on it (see
    /// [`CatalogConfiguration::s3_capacity_bytes`]).
    pub s3_capacity_bytes: i64,
    /// The entry as a read of the catalog reports it.
    pub item: CatalogItem,
}

impl CatalogItemRecord {
    /// Whether an order can still be placed for the configuration this item
    /// publishes: whether its published `CatalogItemStatus` is the one a
    /// configuration that can still be ordered carries
    /// ([`CATALOG_ITEM_STATUS_AVAILABLE`]).
    ///
    /// The question is asked of the status the catalog publishes NOW, so the one
    /// operation that orders a published configuration reads it there rather than
    /// keeping a second list of which SKUs are orderable. Anything other than that
    /// status — [`CATALOG_ITEM_STATUS_DISCONTINUED`], or a status this service does
    /// not publish an item in at all — is not a configuration still being offered.
    pub fn is_orderable(&self) -> bool {
        self.item.item_status.as_deref() == Some(CATALOG_ITEM_STATUS_AVAILABLE)
    }

    /// Whether this item is of ANY of the classes a filter names.
    ///
    /// The class is a SINGLE-valued fact about an item, so the selection is the one
    /// rule every list of this service applies to a `*Filter` member — a filter is a
    /// list of accepted values, and one carrying no value at all narrows nothing,
    /// exactly as an absent one does. That rule lives in [`filter_accepts`] and is
    /// stated only there; this method only says which fact the filter selects on.
    pub fn is_class(&self, classes: &[String]) -> bool {
        filter_accepts(Some(classes), Some(self.item_class.as_str()))
    }

    /// Whether this item supports ANY of the storage options a filter names.
    ///
    /// The storage options are a MULTI-valued fact about an item — the item matches
    /// when any of its own values is one of the accepted ones — which is not the
    /// single-valued selection [`filter_accepts`] covers, so the quantifier over the
    /// record's own values is spelled out here.
    pub fn supports_storage(&self, options: &[String]) -> bool {
        options.is_empty()
            || self
                .item
                .supported_storage
                .iter()
                .flatten()
                .any(|supported| options.contains(supported))
    }

    /// Whether this item carries EC2 capacity of ANY of the instance families a
    /// filter names. MULTI-valued, for the reason [`Self::supports_storage`] is.
    pub fn offers_family(&self, families: &[String]) -> bool {
        families.is_empty()
            || self.item.ec2_capacities.iter().flatten().any(|capacity| {
                capacity
                    .family
                    .as_ref()
                    .is_some_and(|family| families.contains(family))
            })
    }
}

/// The EC2 capacity one configuration carries: one entry per instance FAMILY the
/// hardware runs, naming the largest size of that family and how many of that size
/// the configuration's compute capacity provides.
///
/// Which families a configuration runs is not a second catalog: it is the
/// orderable-instance-type catalog ([`orderable_instance_type_catalog`]) narrowed to
/// the configuration's own form factor and generation, so an item can never offer
/// capacity of a family this service does not publish as orderable on that hardware.
fn ec2_capacities(
    instance_types: &[OrderableInstanceTypeRecord],
    config: &CatalogConfiguration,
) -> Vec<EC2Capacity> {
    let mut largest: BTreeMap<String, (i32, String)> = BTreeMap::new();
    for record in instance_types
        .iter()
        .filter(|record| record.is_offered_as(config.item_class, config.generation))
    {
        let Some(instance_type) = record.instance_type.instance_type.as_deref() else {
            continue;
        };
        let Some((family, _)) = instance_type.split_once('.') else {
            continue;
        };
        let vcpus = record.instance_type.vcp_us.unwrap_or_default();
        let entry = largest
            .entry(family.to_string())
            .or_insert_with(|| (vcpus, instance_type.to_string()));
        if vcpus > entry.0 {
            *entry = (vcpus, instance_type.to_string());
        }
    }
    largest
        .into_iter()
        .map(|(family, (vcpus, max_size))| EC2Capacity {
            family: Some(family),
            max_size: Some(max_size),
            // How many instances of that largest size the configuration's compute
            // capacity runs, which is the figure `Quantity` reports: a whole number
            // of hosts, and at least one, because a family the hardware runs at all
            // runs one of them.
            quantity: Some((config.capacity_vcpus / vcpus.max(1)).max(1).to_string()),
        })
        .collect()
}

/// The whole catalog of items this service publishes, expanded from
/// [`CATALOG_ITEMS`] and ordered by `CatalogItemId`.
///
/// The order is by the published SKU so it is stable across processes — a page token
/// names a position in it, so the order a caller pages through must not depend on how
/// the table above happens to be written.
pub fn catalog_item_catalog() -> Vec<CatalogItemRecord> {
    // The catalog may only ever name values the model declares, so the table is
    // checked against the GENERATED value sets rather than trusted to agree with
    // them: a model that renames a class, a status, a storage option or a scaling
    // type trips this in every debug build instead of letting the catalog report an
    // unmodelled member.
    debug_assert!(
        CATALOG_ITEMS.iter().all(|config| {
            crate::ops::types::CATALOG_ITEM_CLASS_VALUES.contains(&config.item_class)
                && crate::ops::types::FORM_FACTOR_VALUES.contains(&config.item_class)
                && crate::ops::types::OUTPOST_GENERATION_VALUES.contains(&config.generation)
                && crate::ops::types::CATALOG_ITEM_STATUS_VALUES.contains(&config.item_status)
                && config
                    .supported_storage
                    .iter()
                    .all(|option| crate::ops::types::SUPPORTED_STORAGE_ENUM_VALUES.contains(option))
                && config.rack_scaling_type.is_none_or(|scaling| {
                    crate::ops::types::RACK_SCALING_TYPE_VALUES.contains(&scaling)
                })
        }),
        "every catalog configuration names a modelled class, status, storage option and \
         scaling type"
    );
    // A configuration carries S3 capacity exactly when it supports S3 storage: a
    // figure on hardware that supports none would be capacity nothing can provision,
    // and an S3-capable configuration with none would be hardware whose S3 storage
    // holds nothing.
    debug_assert!(
        CATALOG_ITEMS.iter().all(|config| {
            config.supported_storage.contains(&SUPPORTED_STORAGE_S3)
                == (config.s3_capacity_bytes > 0)
        }),
        "a configuration carries S3 capacity exactly when it supports S3 storage"
    );
    let instance_types = orderable_instance_type_catalog();
    let mut items: Vec<(String, i32, i64, CatalogItem)> = CATALOG_ITEMS
        .iter()
        .map(|config| {
            (
                config.item_class.to_string(),
                config.capacity_vcpus,
                config.s3_capacity_bytes,
                CatalogItem {
                    catalog_item_id: Some(sku_code(&config.sku_seed())),
                    item_status: Some(config.item_status.to_string()),
                    ec2_capacities: Some(ec2_capacities(&instance_types, config)),
                    power_kva: Some(config.power_kva),
                    weight_lbs: Some(config.weight_lbs),
                    supported_uplink_gbps: Some(config.supported_uplink_gbps.to_vec()),
                    supported_storage: Some(
                        config
                            .supported_storage
                            .iter()
                            .map(|option| (*option).to_string())
                            .collect(),
                    ),
                    rack_scaling_type: config
                        .rack_scaling_type
                        .map(|scaling| scaling.to_string()),
                },
            )
        })
        .collect();
    items.sort_by(|(.., left), (.., right)| left.catalog_item_id.cmp(&right.catalog_item_id));
    debug_assert!(
        items
            .windows(2)
            .all(|pair| pair[0].3.catalog_item_id != pair[1].3.catalog_item_id),
        "no two catalog configurations are published under one SkuCode"
    );
    items
        .into_iter()
        .enumerate()
        .map(
            |(index, (item_class, capacity_vcpus, s3_capacity_bytes, item))| CatalogItemRecord {
                sequence: u64::try_from(index).unwrap_or_default() + 1,
                item_class,
                capacity_vcpus,
                s3_capacity_bytes,
                item,
            },
        )
        .collect()
}

/// The `AssetType` of a server that provides EC2 capacity, from the modelled value
/// set [`crate::ops::types::ASSET_TYPE_VALUES`] — "`COMPUTE` - Server asset used for
/// customer compute", as `ListAssets$AssetTypeFilter` documents it.
///
/// It is the one asset type that carries `ComputeAttributes`: the host, the instance
/// families and the vCPU ceiling are facts about COMPUTE hardware, and the model
/// documents the member as "information about compute hardware assets".
pub const ASSET_TYPE_COMPUTE: &str = "COMPUTE";

/// The `AssetType` of a server used by storage services — the model's "`STORAGE` -
/// Server asset used by storage services". See [`ASSET_TYPE_COMPUTE`].
pub const ASSET_TYPE_STORAGE: &str = "STORAGE";

/// The `AssetType` of a power shelf — the model's "`POWERSHELF` - Powershelf
/// assets". See [`ASSET_TYPE_COMPUTE`].
pub const ASSET_TYPE_POWERSHELF: &str = "POWERSHELF";

/// The `AssetType` of a switch — the model's "`SWITCH` - Switch assets". See
/// [`ASSET_TYPE_COMPUTE`].
pub const ASSET_TYPE_SWITCH: &str = "SWITCH";

/// The `AssetType` of the hardware Amazon Web Services manages for networking
/// purposes — the model's "`NETWORKING` - Asset managed by Amazon Web Services for
/// networking purposes". See [`ASSET_TYPE_COMPUTE`].
pub const ASSET_TYPE_NETWORKING: &str = "NETWORKING";

/// Whether one recorded hardware asset is an Outpost SERVER: a unit of the
/// [`ASSET_TYPE_COMPUTE`] kind.
///
/// `COMPUTE` is "Server asset used for customer compute", and it is the one kind of
/// unit an Outposts server is installed with ([`SERVER_HARDWARE`]) — every other
/// value of the modelled set is a unit an Outposts RACK is built of: a power shelf
/// feeding it, a switch or a networking asset attaching it, or a server belonging to
/// a storage service. An operation whose member names the Outpost SERVER itself
/// therefore accepts a unit of this kind and refuses the rest, and hardware whose
/// kind was never recorded names no server either.
pub fn is_outpost_server(asset: &AssetInfo) -> bool {
    asset.asset_type.as_deref() == Some(ASSET_TYPE_COMPUTE)
}

/// The `ComputeAssetState` of installed compute hardware: the model's "`ACTIVE` - The
/// asset is available and can provide capacity for new compute resources".
///
/// Hardware that has been installed is hardware that is there to be used, so that is
/// the state it is reported in. The other three values the modelled enum declares
/// (see [`crate::ops::types::COMPUTE_ASSET_STATE_VALUES`]) each describe something
/// having happened to a host — it is being installed, isolated, or retired — which
/// this service holds no record of.
pub const COMPUTE_ASSET_STATE_ACTIVE: &str = "ACTIVE";

/// One physical unit of the hardware an Outpost is installed with.
struct HardwareUnit {
    /// The unit's modelled `AssetType`.
    asset_type: &'static str,
    /// `AssetLocation$RackElevation`: where the unit sits in its rack, "measured in
    /// rack units", or `None` for a unit that sits in no rack of this service's — an
    /// Outposts server is installed in the customer's own rack, whose positions
    /// Amazon Web Services does not hold.
    rack_elevation: Option<f32>,
}

/// The hardware an Outposts RACK is installed with, in installation order.
///
/// An Outposts rack is delivered as a whole rack of hardware: the power shelf that
/// feeds it, the redundant pair of switches and the networking asset that attach it to
/// its Region, and the compute and storage servers that carry its capacity. WHICH
/// units one holds is this emulator's own configuration, exactly as which hardware
/// configurations are offered is ([`CATALOG_ITEMS`]) — AWS publishes no
/// machine-readable rack layout — and the positions are bottom-up in rack units,
/// inside the modelled `RackElevation` range (`range(0..99)`), one unit to a position
/// so no two assets of one rack claim the same one.
const RACK_HARDWARE: &[HardwareUnit] = &[
    HardwareUnit {
        asset_type: ASSET_TYPE_POWERSHELF,
        rack_elevation: Some(1.0),
    },
    HardwareUnit {
        asset_type: ASSET_TYPE_SWITCH,
        rack_elevation: Some(2.0),
    },
    HardwareUnit {
        asset_type: ASSET_TYPE_SWITCH,
        rack_elevation: Some(3.0),
    },
    HardwareUnit {
        asset_type: ASSET_TYPE_NETWORKING,
        rack_elevation: Some(4.0),
    },
    HardwareUnit {
        asset_type: ASSET_TYPE_COMPUTE,
        rack_elevation: Some(5.0),
    },
    HardwareUnit {
        asset_type: ASSET_TYPE_COMPUTE,
        rack_elevation: Some(6.0),
    },
    HardwareUnit {
        asset_type: ASSET_TYPE_COMPUTE,
        rack_elevation: Some(7.0),
    },
    HardwareUnit {
        asset_type: ASSET_TYPE_COMPUTE,
        rack_elevation: Some(8.0),
    },
    HardwareUnit {
        asset_type: ASSET_TYPE_COMPUTE,
        rack_elevation: Some(9.0),
    },
    HardwareUnit {
        asset_type: ASSET_TYPE_COMPUTE,
        rack_elevation: Some(10.0),
    },
    HardwareUnit {
        asset_type: ASSET_TYPE_STORAGE,
        rack_elevation: Some(11.0),
    },
    HardwareUnit {
        asset_type: ASSET_TYPE_STORAGE,
        rack_elevation: Some(12.0),
    },
];

/// The hardware an Outposts SERVER is installed with: the one server it is.
///
/// "An Outpost asset can be a single server within an Outposts rack or an Outposts
/// server configuration", as the modelled `AssetId` documents it — so a server
/// Outpost holds exactly one asset, and it is the compute hardware the Outpost exists
/// to provide. It sits in no rack of this service's, so it reports neither a `RackId`
/// nor an `AssetLocation`.
const SERVER_HARDWARE: &[HardwareUnit] = &[HardwareUnit {
    asset_type: ASSET_TYPE_COMPUTE,
    rack_elevation: None,
}];

/// The hardware configuration an Outpost of `form_factor` is installed with, or
/// `None` for a form factor this service holds no configuration for.
///
/// `SupportedHardwareType` is "the hardware type" of the Outpost and names the same
/// two kinds of hardware the modelled `FormFactor` does ([`FORM_FACTOR_RACK`]), so it
/// is what selects the configuration. `CreateOutpost` records only the form factor the
/// caller named and the model documents no default, so an Outpost that named none
/// names no configuration either: there is nothing for the service to install on it
/// and it holds no asset.
fn hardware_configuration(form_factor: &str) -> Option<&'static [HardwareUnit]> {
    match form_factor {
        FORM_FACTOR_RACK => Some(RACK_HARDWARE),
        FORM_FACTOR_SERVER => Some(SERVER_HARDWARE),
        _ => None,
    }
}

/// How many hardware units one Outpost's installation is identified within, which is
/// what keeps one Outpost's asset ids clear of the next's. See [`asset_slot`].
const HARDWARE_UNITS_PER_OUTPOST: u64 = 100;

/// How many values an `AssetIdInput` can take: the modelled `^\d{10}$`.
const ASSET_ID_SPACE: u64 = 10_000_000_000;

/// The ten-digit slot the unit at `position` of the Outpost created `sequence`th is
/// identified by.
///
/// An `AssetId` a caller reads out of `ListAssets` is the value it then names in
/// `StartCapacityTask$AssetId` and `GetOutpostSupportedInstanceTypes$AssetId`, whose
/// modelled `AssetIdInput` is exactly ten digits (`^\d{10}$`) — the narrower form
/// inside the `AssetId` (`len 1..100`, `^(\w+)$`) those outputs report — so that is
/// the form installed hardware is identified in. It is DERIVED from the Outpost's own
/// creation order and the unit's position in its configuration rather than minted:
/// the id of a unit is therefore stable for as long as the Outpost holds it, and two
/// Outposts of one store can never hold one asset id, because their slots are
/// [`HARDWARE_UNITS_PER_OUTPOST`] apart.
fn asset_slot(sequence: u64, position: usize) -> u64 {
    let outpost = sequence % (ASSET_ID_SPACE / HARDWARE_UNITS_PER_OUTPOST);
    let position = u64::try_from(position).unwrap_or_default() % HARDWARE_UNITS_PER_OUTPOST;
    outpost * HARDWARE_UNITS_PER_OUTPOST + position
}

/// The `AssetId` of the unit at `position`. See [`asset_slot`].
fn asset_id(sequence: u64, position: usize) -> String {
    format!("{:010}", asset_slot(sequence, position))
}

/// The `RackId` of the rack an Outpost's hardware is installed in, which every unit
/// of a rack configuration carries: one rack of hardware is one rack.
///
/// Derived from the Outpost's own creation order for the reason [`asset_slot`] is,
/// and rendered to the same ten digits, which the modelled `RackId` (`len 5..20`,
/// `^[\S \n]+$`) accepts.
fn rack_id(sequence: u64) -> String {
    format!("{:010}", asset_slot(sequence, 0))
}

/// The `HostId` of the Dedicated Host on the compute unit at `position`.
///
/// A host id is an Amazon EC2 identifier — `h-` and seventeen hex characters, which
/// the modelled `HostId` (`len 1..50`, `^[A-Za-z0-9-]*$`) accepts — and names the
/// same one unit of hardware its asset id does, so it is derived from the same slot:
/// `ListAssets$HostIdFilter` selects on it, and one host id therefore names one
/// asset.
fn host_id(sequence: u64, position: usize) -> String {
    format!("h-{:017x}", asset_slot(sequence, position))
}

/// What the compute hardware of `form_factor` runs: every instance family this
/// service publishes as orderable on that hardware, and the vCPU count of the largest
/// orderable instance type on it.
///
/// Not a second catalog: it is the orderable-instance-type catalog
/// ([`orderable_instance_type_catalog`]) narrowed to the form factor, so a host can
/// never report a family or a vCPU ceiling this service does not publish as orderable
/// on the Outpost it is installed in — which is what makes
/// `GetOutpostSupportedInstanceTypes` scoped to one of its assets answer the instance
/// types that asset's own `ComputeAttributes` admit.
fn compute_profile(
    instance_types: &[OrderableInstanceTypeRecord],
    form_factor: &str,
) -> (Vec<String>, i32) {
    let offered = instance_types.iter().filter(|record| {
        record
            .instance_type
            .form_factor_configs
            .iter()
            .flatten()
            .any(|config| config.form_factor.as_deref() == Some(form_factor))
    });
    // The families in their own stable order (a host reports the same list on every
    // read, in every process) and the one ceiling the largest of them sets.
    let mut families: BTreeSet<String> = BTreeSet::new();
    let mut max_vcpus = 0;
    for record in offered {
        let Some(instance_type) = record.instance_type.instance_type.as_deref() else {
            continue;
        };
        let Some((family, _)) = instance_type.split_once('.') else {
            continue;
        };
        families.insert(family.to_string());
        max_vcpus = max_vcpus.max(record.instance_type.vcp_us.unwrap_or_default());
    }
    (families.into_iter().collect(), max_vcpus)
}

/// Every hardware asset installed on one Outpost, in installation order.
///
/// Hardware is what an Outpost IS: Amazon Web Services installs the configuration the
/// Outpost's form factor names once the order for it is fulfilled, and the Outpost
/// reaches a settled lifecycle status with that hardware in it — so an Outpost whose
/// ordered hardware has been installed holds one asset per physical unit of its
/// configuration,
/// each identified by the slot it occupies, typed by what the unit is, placed where it
/// sits in its rack, and — for the compute units, the only ones the model carries
/// `ComputeAttributes` for — carrying the Dedicated Host it hosts instances on and the
/// instance families and vCPU ceiling this service publishes as orderable on that
/// hardware. An Outpost whose form factor was never recorded names no configuration,
/// so nothing is installed on it (see [`hardware_configuration`]).
pub fn installed_hardware(
    sequence: u64,
    form_factor: Option<&str>,
    instance_types: &[OrderableInstanceTypeRecord],
) -> Vec<AssetInfo> {
    let Some((form_factor, units)) = form_factor
        .and_then(|form_factor| hardware_configuration(form_factor).map(|units| (form_factor, units)))
    else {
        return Vec::new();
    };
    // A configuration may only ever name values the model declares and positions it
    // allows, so the tables above are checked against the GENERATED value set and the
    // modelled range rather than trusted to agree with them.
    debug_assert!(
        units.iter().all(|unit| {
            crate::ops::types::ASSET_TYPE_VALUES.contains(&unit.asset_type)
                && unit
                    .rack_elevation
                    .is_none_or(|elevation| (0.0..=99.0).contains(&elevation))
        }),
        "every installed unit names a modelled AssetType and a position inside RackElevation"
    );
    debug_assert!(
        units.len() < HARDWARE_UNITS_PER_OUTPOST as usize,
        "one Outpost's hardware fits in the slots its asset ids are identified within"
    );
    let (instance_families, max_vcpus) = compute_profile(instance_types, form_factor);
    // Every unit of ONE rack of hardware is in that one rack; a server is in no rack
    // of this service's, so it carries no rack id (see [`SERVER_HARDWARE`]).
    let rack = (form_factor == FORM_FACTOR_RACK).then(|| rack_id(sequence));
    units
        .iter()
        .enumerate()
        .map(|(index, unit)| {
            // The rack itself takes the first slot of the Outpost's range, so the
            // units take the ones after it.
            let position = index + 1;
            AssetInfo {
                asset_id: Some(asset_id(sequence, position)),
                rack_id: rack.clone(),
                asset_type: Some(unit.asset_type.to_string()),
                compute_attributes: (unit.asset_type == ASSET_TYPE_COMPUTE).then(|| {
                    ComputeAttributes {
                        host_id: Some(host_id(sequence, position)),
                        state: Some(COMPUTE_ASSET_STATE_ACTIVE.to_string()),
                        instance_families: Some(instance_families.clone()),
                        // "The instance type capacities configured for this asset.
                        // This can be changed through a capacity task": hardware
                        // being installed has had no capacity task run on it, so it
                        // carries none until one configures it
                        // ([`ComAmazonawsOutpostsOutpostsolafserviceState::configure_hardware`])
                        // rather than a split nobody asked for.
                        instance_type_capacities: None,
                        max_vcpus: Some(max_vcpus),
                    }
                }),
                asset_location: unit.rack_elevation.map(|rack_elevation| AssetLocation {
                    rack_elevation: Some(rack_elevation),
                }),
            }
        })
        .collect()
}

/// Report `assets` — the hardware standing INSTALLED on an Outpost — on the line
/// items of an order placed on it that have been installed.
///
/// `LineItem$AssetInformationList` is "information about assets", the hardware
/// installed for the item, and an item reaches [`LINE_ITEM_STATUS_INSTALLED`] exactly
/// when the install an order's fulfilment owes has put the Outpost's configuration on
/// it ([`installed_hardware`]) — so an installed item reports that hardware and an item
/// that is still being prepared, built or delivered, or that a cancellation finished,
/// had nothing installed for it and reports none.
///
/// An install delivers one asset per physical unit of the configuration and this
/// service holds no record of WHICH unit a particular line item delivered, so the units
/// are dealt out across the installed items in installation order, one unit to each item
/// in turn: every unit the install delivered is then reported by exactly one item, and no
/// unit is claimed by two, which filling each item up before the next would not give
/// (the first item would hold the whole configuration and the rest nothing). A unit this
/// service holds no `AssetId` for names no asset and is reported by none, and installed
/// items an install delivered nothing for — an Outpost whose form factor names no
/// configuration holds no hardware — report nothing rather than an empty list.
///
/// That invariant is stated of ONE order, which is the scope hardware is installed in:
/// one configuration is installed on an Outpost once, so a second order fulfilled on an
/// Outpost that already holds it delivered nothing new and its items report the hardware
/// standing there — the hardware the Outpost holds, which is what every report of it
/// answers.
fn report_installed_assets(items: &mut [LineItem], assets: &[AssetInfo]) {
    let installed: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.status.as_deref() == Some(LINE_ITEM_STATUS_INSTALLED))
        .map(|(index, _)| index)
        .collect();
    if installed.is_empty() {
        return;
    }
    let mut dealt: Vec<Vec<LineItemAssetInformation>> = vec![Vec::new(); installed.len()];
    for (unit, asset_id) in assets
        .iter()
        .filter_map(|asset| asset.asset_id.clone())
        .enumerate()
    {
        dealt[unit % installed.len()].push(LineItemAssetInformation {
            asset_id: Some(asset_id),
            // "The MAC addresses of the asset": a hardware address belongs to a network
            // interface of a physical unit, and this service records none — no call in
            // this API reports or accepts one, and how many interfaces a unit carries is
            // not a fact it holds either — so the member stays absent.
            mac_address_list: None,
        });
    }
    for (slot, index) in installed.into_iter().enumerate() {
        let reported = std::mem::take(&mut dealt[slot]);
        items[index].asset_information_list = (!reported.is_empty()).then_some(reported);
    }
}

/// The one `QuotePricingType` the model declares (see
/// [`crate::ops::types::QUOTE_PRICING_TYPE_VALUES`]): the pricing of this service is
/// the pricing of a SUBSCRIPTION on hardware, which is the kind of price a
/// `PricingOption` carries its details of.
pub const QUOTE_PRICING_TYPE_SUBSCRIPTION: &str = "SUBSCRIPTION";

/// The one `CurrencyCode` the model declares (see
/// [`crate::ops::types::CURRENCY_CODE_VALUES`]) — "currently only `USD` is
/// supported", as the model documents `SubscriptionPricingDetails$Currency`.
pub const CURRENCY_USD: &str = "USD";

/// How many months a subscription bought on each modelled `PaymentTerm` runs for:
/// the term's own length in years, which is what every value of the modelled set
/// [`crate::ops::types::PAYMENT_TERM_VALUES`] (`ONE_YEAR` | `THREE_YEARS` |
/// `FIVE_YEARS`) names, times the twelve months in a year.
pub const PAYMENT_TERM_MONTHS: &[(&str, i32)] =
    &[("ONE_YEAR", 12), ("THREE_YEARS", 36), ("FIVE_YEARS", 60)];

/// The share of a subscription's whole-term cost that each modelled `PaymentOption`
/// is paid UP FRONT, the rest being spread over the term's months.
///
/// The three values of the modelled set
/// [`crate::ops::types::PAYMENT_OPTION_VALUES`] name the share themselves: all of
/// the cost in advance and nothing monthly, none of it in advance and all of it
/// monthly, or part in advance and the rest monthly. How big that part is for the
/// partial option is not published through this API, so the half below is this
/// emulator's own, exactly as the capacity assignment of [`CATALOG_ITEMS`] is.
pub const PAYMENT_OPTION_UPFRONT_SHARE: &[(&str, f32)] = &[
    ("ALL_UPFRONT", 1.0),
    ("NO_UPFRONT", 0.0),
    ("PARTIAL_UPFRONT", 0.5),
];

/// The monthly list price, in [`CURRENCY_USD`], of one vCPU of the EC2 capacity an
/// Outposts configuration carries — the rate every subscription this service prices
/// is computed from, whether it is quoted for new hardware or renewed on hardware
/// already installed.
///
/// AWS prices a configuration per quote rather than publishing a machine-readable
/// price list, so the rate itself is this emulator's own, exactly as the capacity
/// assignment of [`CATALOG_ITEMS`] is. What an emulated price has to get right is
/// that it FOLLOWS from the capacity it is a price for: the same capacity is priced
/// the same figure in every process and on every read, twice the capacity costs
/// twice as much, and a longer term costs more over the term than a shorter one.
pub const SUBSCRIPTION_PRICE_PER_VCPU_MONTH: f32 = 20.0;

/// `amount` rounded to the cent — the precision a price is quoted to — as the
/// modelled price member carries it, or `None` for an amount no price member can
/// carry.
///
/// A price is modelled a single-precision number (`SubscriptionPricingDetails$UpfrontPrice`
/// and `$MonthlyRecurringPrice` both target `NullableFloat`), so the figure answered
/// has to be a FINITE value of that type: an amount whose rounded value is beyond
/// that range is no figure this service can quote, and the non-finite values such an
/// amount saturates to are not numbers JSON can carry at all. The rounding itself is
/// done in double precision, so it can never push an in-range amount out of range.
fn to_the_cent(amount: f64) -> Option<f32> {
    let rounded = ((amount * 100.0).round() / 100.0) as f32;
    rounded.is_finite().then_some(rounded)
}

/// What a subscription on `vcpus` vCPUs of EC2 capacity costs under `payment_option`
/// and `payment_term` — or `None` when there is nothing to price: no capacity at
/// all, or a payment option or term this service holds no rule for.
///
/// The ONE rule this service prices a subscription by, so what a quote for new
/// hardware quotes ([`ComAmazonawsOutpostsOutpostsolafserviceState::quote_pricing`])
/// and what a renewal of hardware already installed is bought at
/// ([`ComAmazonawsOutpostsOutpostsolafserviceState::renewal_price`]) cannot disagree
/// about the same capacity: the term's months ([`PAYMENT_TERM_MONTHS`]) of the
/// monthly rate that capacity carries ([`SUBSCRIPTION_PRICE_PER_VCPU_MONTH`]) make
/// up the whole-term cost, of which the payment option's share
/// ([`PAYMENT_OPTION_UPFRONT_SHARE`]) is paid up front and the rest in equal monthly
/// instalments. Every figure follows from the capacity it was given, so one price is
/// answered the same in every process and on every call.
///
/// It is also `None` when the figures that rule gives are beyond what a modelled
/// price member can carry ([`to_the_cent`]): a price this service cannot state is not
/// a price, so the option is left out exactly as it is for capacity there is no
/// figure for — never answered as the non-finite value the arithmetic saturates to.
/// The whole computation is carried out in double precision, so that only the
/// FIGURE's own range decides that, and not an intermediate step of a rule stated on
/// amounts.
pub fn subscription_price(
    vcpus: f64,
    payment_option: &str,
    payment_term: &str,
) -> Option<SubscriptionPricingDetails> {
    let (_, share) = PAYMENT_OPTION_UPFRONT_SHARE
        .iter()
        .find(|(option, _)| *option == payment_option)?;
    let share = f64::from(*share);
    let (_, months) = PAYMENT_TERM_MONTHS
        .iter()
        .find(|(term, _)| *term == payment_term)?;
    if vcpus <= 0.0 || !vcpus.is_finite() {
        // There is no capacity here, so there is no subscription on it that could be
        // priced at all.
        return None;
    }
    let monthly_rate = vcpus * f64::from(SUBSCRIPTION_PRICE_PER_VCPU_MONTH);
    let term = monthly_rate * f64::from(*months);
    Some(SubscriptionPricingDetails {
        payment_option: Some(payment_option.to_string()),
        payment_term: Some(payment_term.to_string()),
        upfront_price: Some(to_the_cent(term * share)?),
        monthly_recurring_price: Some(to_the_cent(monthly_rate * (1.0 - share))?),
        currency: Some(CURRENCY_USD.to_string()),
    })
}

/// The id prefix a minted `QuoteOptionIdentifier` carries, from the modelled pattern
/// `^oqo-[a-f0-9]{17}$`. See
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::mint_id`].
pub const QUOTE_OPTION_ID_PREFIX: &str = "oqo";

/// The most options one quote reports, as the modelled `QuoteOptionList`
/// (`length(max: 10)`) allows.
const QUOTE_OPTION_MAX: usize = 10;

/// The most pricing options one quote option reports, as the modelled
/// `PricingOptionList` (`length(max: 9)`) allows — which is exactly the number of
/// (`PaymentOption`, `PaymentTerm`) combinations the two modelled value sets make.
const PRICING_OPTION_MAX: usize = 9;

/// The most ordering requirements one quote reports, as the modelled
/// `OrderingRequirementList` (`length(max: 50)`) allows.
const ORDERING_REQUIREMENT_MAX: usize = 50;

/// The `QuoteCapacityType` whose capacity this service publishes a figure for: the
/// EC2 capacity of an Outposts configuration, which `QuoteCapacity$Unit` names the
/// instance type of. One of the modelled values (see
/// [`crate::ops::types::QUOTE_CAPACITY_TYPE_VALUES`]).
pub const QUOTE_CAPACITY_TYPE_EC2: &str = "EC2";

/// The `OrderingRequirementStatus` a check this service ran and found SATISFIED
/// carries.
///
/// The modelled enum declares three values (see
/// [`crate::ops::types::ORDERING_REQUIREMENT_STATUS_VALUES`]): a check that passed,
/// one that failed, and one the quote is exempt from. Exemption is granted outside
/// this API, so a check this service runs reports one of the first two.
pub const ORDERING_REQUIREMENT_PASS: &str = "PASS";

/// The `OrderingRequirementStatus` a check this service ran and found UNSATISFIED
/// carries. See [`ORDERING_REQUIREMENT_PASS`].
pub const ORDERING_REQUIREMENT_FAIL: &str = "FAIL";

/// The `OrderingRequirementType` of the check that the quote names an Outpost: "the
/// quote must be associated with an Outpost before submitting an order", which is
/// also what makes an order submitted from the quote an order OF an Outpost.
pub const ORDERING_REQUIREMENT_OUTPOST_ID_MISSING: &str = "OUTPOST_ID_MISSING_ON_QUOTE_ERROR";

/// The `OrderingRequirementType` of the check that the Outpost the quote names is
/// usable: "the Outpost must be in an active state", which for this service is the
/// terminal [`LIFE_CYCLE_STATUS_ACTIVE`] an Outpost settles in.
pub const ORDERING_REQUIREMENT_OUTPOST_ACTIVE: &str = "OUTPOST_ACTIVE_CHECK_ERROR";

/// The `OrderingRequirementType` of the check that another order can still be
/// placed: "the maximum number of allowed orders has been reached", which for this
/// service is the store's own entry ceiling ([`MAX_RESOURCES`]).
pub const ORDERING_REQUIREMENT_MAXIMUM_ALLOWED_ORDERS: &str =
    "MAXIMUM_ALLOWED_ORDERS_CHECK_ERROR";

/// The `OrderingRequirementType` of the check that the site has the address it
/// operates at: "the site must have an operating address".
pub const ORDERING_REQUIREMENT_OPERATING_ADDRESS: &str = "OPERATING_ADDRESS_EXISTENCE_CHECK_ERROR";

/// The `OrderingRequirementType` of the check that the site has the address its
/// hardware is shipped to: "the site must have a shipping address".
pub const ORDERING_REQUIREMENT_SHIPPING_ADDRESS: &str = "SHIPPING_ADDRESS_EXISTENCE_CHECK_ERROR";

/// The `OrderingRequirementType` of the check that the quote is for the country the
/// hardware would stand in: "the country code on the quote does not match the
/// Outpost site country".
pub const ORDERING_REQUIREMENT_COUNTRY_CODE_MISMATCH: &str = "COUNTRY_CODE_MISMATCH_CHECK_ERROR";

/// The modelled `OrderingRequirementStatus` a check that was RUN reports: passed
/// when what it checks holds, failed when it does not.
fn pass_or_fail(satisfied: bool) -> &'static str {
    match satisfied {
        true => ORDERING_REQUIREMENT_PASS,
        false => ORDERING_REQUIREMENT_FAIL,
    }
}

/// One requirement of a quote: the check, the outcome [`pass_or_fail`] read off the
/// store, and the message stating what that outcome was read from.
///
/// AWS publishes no wording for `OrderingRequirement$StatusMessage`, so each message
/// is this emulator's own; what a caller branches on is the modelled
/// `OrderingRequirementType` and `OrderingRequirementStatus`, while the message names
/// the resource and the value the check was decided on.
fn ordering_requirement(check: &str, status: &str, status_message: String) -> OrderingRequirement {
    OrderingRequirement {
        status_message: Some(status_message),
        ordering_requirement_type: Some(check.to_string()),
        status: Some(status.to_string()),
    }
}

/// The checks of `requirements` a quote does NOT meet, in the order they are
/// reported.
///
/// A requirement is "a requirement that must be met before an order can be submitted
/// for a quote", so what blocks the submit is the set of checks reporting
/// [`ORDERING_REQUIREMENT_FAIL`]: a check that passed is met, and one the quote is
/// exempt from is nothing to meet. A row naming no check says nothing a caller could
/// act on and blocks nothing.
pub fn unmet_requirements(requirements: &[OrderingRequirement]) -> Vec<&str> {
    requirements
        .iter()
        .filter(|requirement| requirement.status.as_deref() == Some(ORDERING_REQUIREMENT_FAIL))
        .filter_map(|requirement| requirement.ordering_requirement_type.as_deref())
        .collect()
}

/// The values a quote requested of one of the two payment value sets, or every value
/// the model declares when it requested none.
///
/// "The payment options to include in the quote pricing. If not specified, all
/// available payment options are returned", and the same sentence for the payment
/// terms — and a list the modelled `length(min: 0, max: 3)` let arrive empty names no
/// more than an absent member does. A value named twice is one request for it, so the
/// quote is priced under each distinct value once, in the order the request named
/// them in.
///
/// What is kept is what the MODEL declares: a value outside `modelled` names none of
/// the payment values this service prices, so it is dropped here rather than carried
/// into the combinations a quote is priced under — which is what keeps the number of
/// those combinations a figure of the model (`modelled.len()` per set) however long a
/// list arrives, and the membership test a set lookup rather than a walk of
/// everything kept so far.
fn requested_or_modelled<'a>(
    requested: Option<&'a [String]>,
    modelled: &'static [&'static str],
) -> Vec<&'a str> {
    match requested {
        Some(values) if !values.is_empty() => {
            let mut named: Vec<&str> = Vec::new();
            let mut already: BTreeSet<&str> = BTreeSet::new();
            for value in values.iter().map(String::as_str) {
                if modelled.contains(&value) && already.insert(value) {
                    named.push(value);
                }
            }
            named
        }
        _ => modelled.to_vec(),
    }
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
    ///
    /// A connection installs ONE hardware asset of ONE Outpost, so every stored
    /// connection names hardware the store still holds: the Outpost going away takes
    /// the connections started for its assets with it ([`Self::remove_outpost`]).
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
    /// The catalog of items `ListCatalogItems` reports, in its stable order.
    ///
    /// Reference data rather than a caller's resource, exactly as
    /// [`Self::orderable_instance_types`] is: it is seeded in
    /// [`ComAmazonawsOutpostsOutpostsolafserviceState::default`] from
    /// [`catalog_item_catalog`] and no operation mutates it, so every read of it
    /// answers the same catalog.
    pub catalog_items: Vec<CatalogItemRecord>,
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
            catalog_items: catalog_item_catalog(),
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

    /// Remove the Outpost `outpost_id` names in `region`, and with it every record
    /// this store keeps that exists only as part of that Outpost. See
    /// [`Self::remove_site`].
    ///
    /// A record whose whole subject is the Outpost or something inside it has no
    /// existence once the Outpost is gone, so it goes with the Outpost rather than
    /// being left where nothing can reach it: nothing can name it any more, a read of
    /// it would answer for an Outpost the store does not hold, and the slot it holds
    /// in its own map would never come back to the store's ceiling
    /// ([`MAX_RESOURCES`]). This store keeps two such records outside
    /// [`OutpostRecord`] itself:
    ///
    /// * [`Self::capacity_tasks`] — a capacity task is a task OF an Outpost
    ///   ([`CapacityTaskRecord::outpost_id`]), and every capacity-task route resolves
    ///   that Outpost first. Every task left here is finished work: an unfinished one
    ///   is the conflict a delete refuses on ([`Self::unfinished_work_on_outpost`]).
    /// * [`Self::connections`] — a connection is the installation tunnel for ONE
    ///   hardware asset ([`ConnectionRecord::asset_id`]), and an asset belongs to
    ///   exactly one Outpost ([`OutpostRecord::assets`]), so a connection started for
    ///   an asset of this Outpost installs hardware that is going away with it.
    ///
    /// Each is matched the way the operations that read it resolve it: a task by the
    /// Outpost it names, and a connection by the asset it installs within the region
    /// that Outpost's inventory belongs to (see [`RegionalRecord`]).
    pub fn remove_outpost(&mut self, region: &str, outpost_id: &str) -> Option<OutpostRecord> {
        in_region(&self.outposts, region, outpost_id)?;
        let removed = self.outposts.remove(outpost_id)?;
        self.capacity_tasks
            .retain(|_, task| task.outpost_id != outpost_id);
        let installed: BTreeSet<&str> = removed
            .assets
            .iter()
            .filter_map(|asset| asset.asset_id.as_deref())
            .collect();
        self.connections.retain(|_, connection| {
            connection.region != region || !installed.contains(connection.asset_id.as_str())
        });
        Some(removed)
    }

    /// Every Outpost of `region`, in the store's creation order — what
    /// `ListOutposts` pages through. See [`RegionalRecord`].
    pub fn outposts_in<'a>(
        &'a self,
        region: &str,
    ) -> impl Iterator<Item = &'a OutpostRecord> + 'a {
        all_in_region(&self.outposts, region)
    }

    /// Every capacity task of `region`, in no particular order — what
    /// `ListCapacityTasks` pages through.
    ///
    /// A capacity task carries no region of its own: it is a task OF an Outpost, so
    /// the Outpost's region scopes it (see [`RegionalRecord`]), and a task whose
    /// Outpost this region does not hold is none of this region's inventory. Every
    /// stored task has a live parent, because an Outpost going away takes its tasks
    /// with it.
    pub fn capacity_tasks_in<'a>(
        &'a self,
        region: &str,
    ) -> impl Iterator<Item = &'a CapacityTaskRecord> + 'a {
        // The region is copied into the filter for the reason [`all_in_region`]
        // copies it: the iterator then borrows only the store.
        let region = region.to_string();
        self.capacity_tasks
            .values()
            .filter(move |task| self.outpost(region.as_str(), &task.outpost_id).is_some())
    }

    /// The quote `quote_id` names in `region`. See [`Self::site`].
    pub fn quote(&self, region: &str, quote_id: &str) -> Option<&QuoteRecord> {
        in_region(&self.quotes, region, quote_id)
    }

    /// The same quote, to CHANGE. See [`Self::quote`].
    pub fn quote_mut(&mut self, region: &str, quote_id: &str) -> Option<&mut QuoteRecord> {
        in_region_mut(&mut self.quotes, region, quote_id)
    }

    /// Remove the quote `quote_id` names in `region`. See [`Self::remove_site`].
    pub fn remove_quote(&mut self, region: &str, quote_id: &str) -> Option<QuoteRecord> {
        in_region(&self.quotes, region, quote_id)?;
        self.quotes.remove(quote_id)
    }

    /// Every quote of `region`, in the store's creation order — what `ListQuotes`
    /// pages through. See [`RegionalRecord`].
    pub fn quotes_in<'a>(&'a self, region: &str) -> impl Iterator<Item = &'a QuoteRecord> + 'a {
        all_in_region(&self.quotes, region)
    }

    /// The connection `connection_id` names in `region`. See [`Self::site`].
    pub fn connection(&self, region: &str, connection_id: &str) -> Option<&ConnectionRecord> {
        in_region(&self.connections, region, connection_id)
    }

    /// The order `order_id` names in `region`. See [`Self::site`].
    pub fn order(&self, region: &str, order_id: &str) -> Option<&OrderRecord> {
        in_region(&self.orders, region, order_id)
    }

    /// The order `order_id` names in `region` that was PLACED on `outpost_id`, or
    /// `None` when this store holds no such order — whether it holds none under that
    /// id at all, or holds one placed against another Outpost.
    ///
    /// An order is placed ON exactly one Outpost (`Order$OutpostId`, fixed by
    /// `CreateOrder` and never moved), and the hardware it delivers is installed on
    /// that Outpost and on no other. So wherever an `OrderId` arrives on a request
    /// addressed to an Outpost — where the member is the order "of the host
    /// associated with" that Outpost — the order it names has to be one of THAT
    /// Outpost's: an order placed against another Outpost names no hardware here, so
    /// for such a request it is as absent as an id nothing ever minted, and is
    /// refused with the same [`order_not_found`] wording. Stated once here so the
    /// operations that resolve the member in an Outpost's scope cannot disagree about
    /// what it names, exactly as [`Self::order`] states the region scope every order
    /// lookup shares. Scoped to `region` for the reason [`RegionalRecord`] gives.
    pub fn order_of_outpost(
        &self,
        region: &str,
        outpost_id: &str,
        order_id: &str,
    ) -> Option<&OrderRecord> {
        self.order(region, order_id)
            .filter(|record| record.order.outpost_id.as_deref() == Some(outpost_id))
    }

    /// The same order, to CHANGE. See [`Self::order`].
    pub fn order_mut(&mut self, region: &str, order_id: &str) -> Option<&mut OrderRecord> {
        in_region_mut(&mut self.orders, region, order_id)
    }

    /// Every order of `region`, in the store's creation order — what `ListOrders`
    /// pages through. See [`RegionalRecord`], and [`Self::quotes_in`], which is the
    /// same question asked of the other resource a region's inventory holds.
    pub fn orders_in<'a>(&'a self, region: &str) -> impl Iterator<Item = &'a OrderRecord> + 'a {
        all_in_region(&self.orders, region)
    }

    /// The `Order` a read of the stored `record` answers: the order as it is persisted,
    /// with the hardware its fulfilment installed reported on the line items that
    /// hardware was installed for.
    ///
    /// A member whose fact this service only holds once a later step has happened is
    /// absent until that step and reported from the stored fact afterwards — the rule
    /// `OrderFulfilledDate` follows ([`Self::settle_orders`]). The hardware installed
    /// for a line item is the one record of it this store keeps, the assets standing on
    /// the Outpost the order was placed on ([`OutpostRecord::assets`]), so an installed
    /// item is reported from there ([`report_installed_assets`]) and `ListAssets` and a
    /// read of the order can never disagree about which assets are installed. An order
    /// whose Outpost this region holds none of — one since deleted — has no installed
    /// hardware to report.
    pub fn order_report(&self, record: &OrderRecord) -> Order {
        let assets = record
            .order
            .outpost_id
            .as_deref()
            .and_then(|outpost_id| self.outpost(&record.region, outpost_id))
            .map(|outpost| outpost.assets.as_slice())
            .unwrap_or_default();
        let mut order = record.order.clone();
        if let Some(items) = order.line_items.as_deref_mut() {
            report_installed_assets(items, assets);
        }
        order
    }

    /// The hardware configuration this service publishes under the `SkuCode`
    /// `catalog_item_id`, or `None` for a code of that form it publishes nothing
    /// under — which this service holds no published hardware facts for.
    pub fn catalog_item(&self, catalog_item_id: &str) -> Option<&CatalogItemRecord> {
        self.catalog_items
            .iter()
            .find(|record| record.item.catalog_item_id.as_deref() == Some(catalog_item_id))
    }

    /// How many vCPUs of EC2 capacity this service publishes for the catalog item
    /// `catalog_item_id` names, or `None` for a SKU it publishes no configuration
    /// under — a code of the modelled `SkuCode` form that names nothing in the
    /// catalog, which this service holds no published hardware facts for.
    pub fn catalog_item_capacity_vcpus(&self, catalog_item_id: &str) -> Option<i32> {
        self.catalog_item(catalog_item_id)
            .map(|record| record.capacity_vcpus)
    }

    /// How much Amazon S3 storage capacity, in bytes, this service publishes for the
    /// catalog item `catalog_item_id` names, or `None` for a SKU it publishes no
    /// configuration under. See [`Self::catalog_item_capacity_vcpus`].
    pub fn catalog_item_s3_capacity_bytes(&self, catalog_item_id: &str) -> Option<i64> {
        self.catalog_item(catalog_item_id)
            .map(|record| record.s3_capacity_bytes)
    }

    /// The number of DEFAULT vCPUs this service publishes for one instance type, or
    /// `None` for a type its catalog does not publish — a figure the emulator does
    /// not have, and never one it invents.
    ///
    /// Read off the one orderable-instance-type catalog
    /// ([`Self::orderable_instance_types`]), so what any report of this service says
    /// about an instance type cannot disagree with what `ListOrderableInstanceTypes`
    /// says about it.
    pub fn published_vcpus(&self, instance_type: &str) -> Option<i32> {
        self.orderable_instance_types
            .iter()
            .find(|record| record.instance_type.instance_type.as_deref() == Some(instance_type))
            .and_then(|record| record.instance_type.vcp_us)
    }

    /// The EC2 capacity the hardware the order `order` is for carries: one entry per
    /// instance family a configuration its line items subscribe to publishes capacity
    /// of, each naming the largest size of that family that hardware runs, in the
    /// order the order's line items name them.
    ///
    /// What an order is FOR is the hardware configurations its line items name, and
    /// what such a configuration carries capacity of is what this service publishes
    /// for it ([`ec2_capacities`]) — the same records the configuration an Outpost
    /// arrived carrying is read off ([`Self::ordered_instance_type_names`]). A line
    /// item naming a catalog item this service publishes no configuration for
    /// contributes nothing, exactly as it contributes nothing to
    /// [`Self::subscribed_capacity_vcpus`]: the capacity of hardware the catalog does
    /// not describe is not a figure the emulator has, and never one it invents — so an
    /// order whose every line item names one carries no published capacity at all, and
    /// the store holds no hardware fact about the order to narrow anything by.
    ///
    /// Asked of ONE named order rather than of an Outpost's inventory, which is why it
    /// does not skip a cancelled order as [`Self::ordered_instance_type_names`] and
    /// [`Self::subscribed_capacity_vcpus`] do: those two are about the hardware an
    /// Outpost HOLDS or pays for, and nothing was ever delivered for a cancelled order,
    /// while what a named order IS FOR is its own line items whatever status it reached.
    pub fn ordered_ec2_capacity(&self, order: &Order) -> Vec<FamilyCapacity<'_>> {
        let mut capacities: Vec<FamilyCapacity<'_>> = Vec::new();
        for capacity in order
            .line_items
            .iter()
            .flatten()
            .filter_map(|item| self.catalog_item(item.catalog_item_id.as_deref()?))
            .flat_map(|record| record.item.ec2_capacities.iter().flatten())
        {
            let Some(family) = capacity.family.as_deref() else {
                continue;
            };
            let entry = FamilyCapacity {
                family,
                max_vcpus: capacity
                    .max_size
                    .as_deref()
                    .and_then(|max_size| self.published_vcpus(max_size)),
            };
            // Two line items can name configurations that carry capacity of one
            // family, which is one fact about that hardware rather than two.
            if !capacities.contains(&entry) {
                capacities.push(entry);
            }
        }
        capacities
    }

    /// The instance types the EC2 capacity of the Outpost `outpost_id` names in
    /// `region` is CURRENTLY CONFIGURED with, in one stable order and each carrying
    /// the number of default vCPUs the catalog publishes for it.
    ///
    /// What an Outpost's capacity is configured with is a record of this store, not a
    /// fact about what its hardware could run — that wider question is the capability
    /// report, which "will generally include instance types that are not currently
    /// configured". Two records configure it, and a capacity task is the later of
    /// them:
    ///
    /// * a COMPLETED capacity task. "The instance type capacities configured for this
    ///   asset. This can be changed through a capacity task", as
    ///   `ComputeAttributes$InstanceTypeCapacities` documents it, so the pools of such
    ///   a task are the capacity configured on the hardware that task named — one
    ///   asset of the Outpost, or the Outpost itself when the task named none — and on
    ///   each of those the task that completed LAST is the configuration that stands.
    ///   A task still being worked through, or one that failed or was cancelled, has
    ///   configured nothing; and a pool of `Count` zero ("the number of instances for
    ///   the specified instance type") configures no instance of its type, so it names
    ///   no configured instance type.
    /// * the ORDER its hardware arrived on, until a capacity task reconfigures that
    ///   hardware. A published configuration carries EC2 capacity of one or more
    ///   instance families, each at the largest size that hardware runs
    ///   ([`ec2_capacities`]), so an Outpost is configured with those sizes of the
    ///   configurations its uncancelled orders subscribe to — the same records the
    ///   subscribed capacity is read off ([`Self::subscribed_capacity_vcpus`]). A line
    ///   item naming a catalog item this service publishes no configuration for
    ///   contributes nothing. Capacity is configured ON compute hardware, so an
    ///   Outpost holding none — one whose ordered hardware is not installed yet, or
    ///   one whose form factor names no configuration this service installs — is
    ///   configured with nothing.
    ///
    /// Because each record configures the hardware it is about, what the Outpost is
    /// configured with is what stands across ALL of its hardware together: a task
    /// scoped to one asset replaces the configuration of that asset alone, and the
    /// order-derived configuration of every other piece of compute hardware on the
    /// Outpost goes on standing until something reconfigures that hardware too.
    pub fn configured_instance_types(
        &self,
        region: &str,
        outpost_id: &str,
    ) -> Vec<InstanceTypeItem> {
        self.configured_instance_type_names(region, outpost_id)
            .into_iter()
            .map(|instance_type| InstanceTypeItem {
                vcp_us: self.published_vcpus(&instance_type),
                instance_type: Some(instance_type),
            })
            .collect()
    }

    /// The instance types [`Self::configured_instance_types`] reports, as the bare
    /// type names in the order they are reported in.
    ///
    /// Each piece of the Outpost's hardware carries its OWN configuration, so the
    /// report is the union of what stands on each of them: a capacity task scoped to
    /// one asset configured that asset alone and leaves every other piece of hardware
    /// configured as it was, which for hardware no task has named is what it arrived
    /// carrying.
    fn configured_instance_type_names(&self, region: &str, outpost_id: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        // The capacity task that stands on each piece of hardware a task has
        // configured on this Outpost. Walked in the hardware's own key order (the
        // Outpost itself first, then its assets by id) so one store answers the
        // configured types in the same order on every read.
        let mut standing: BTreeMap<Option<&str>, &CapacityTaskRecord> = BTreeMap::new();
        for task in self.capacity_tasks.values().filter(|task| {
            task.outpost_id == outpost_id && task.status == CAPACITY_TASK_COMPLETED
        }) {
            let scope = task.asset_id.as_deref();
            let supersedes = standing.get(&scope).is_none_or(|standing| {
                (standing.completed_at, standing.sequence) < (task.completed_at, task.sequence)
            });
            if supersedes {
                standing.insert(scope, task);
            }
        }
        // What stands on the Outpost ITSELF, reported first: the pools of the task
        // that reconfigured the whole Outpost or, until one does, what the hardware
        // the Outpost's uncancelled orders subscribe to arrived carrying. That
        // order-derived configuration still stands wherever a task has not replaced
        // it, so it is reported while the Outpost holds compute hardware no task of
        // its own has reconfigured — and an Outpost holding no such hardware (none at
        // all, or every piece of it reconfigured) carries none of it.
        match standing.get(&None) {
            Some(task) => push_configured_pools(&mut names, task),
            None => {
                if self.holds_unreconfigured_compute(region, outpost_id, &standing) {
                    for instance_type in self.ordered_instance_type_names(region, outpost_id) {
                        push_once(&mut names, &instance_type);
                    }
                }
            }
        }
        // Then each ASSET a task has reconfigured, which that task configured alone.
        for (_, task) in standing.iter().filter(|(scope, _)| scope.is_some()) {
            push_configured_pools(&mut names, task);
        }
        names
    }

    /// Whether the Outpost `outpost_id` names in `region` holds compute hardware that
    /// none of the capacity tasks `standing` on it has reconfigured — the hardware
    /// whose configuration is still the one it arrived carrying.
    ///
    /// Capacity is configured ON compute hardware, so an Outpost holding none — one
    /// whose ordered hardware is not installed yet, or one whose form factor names no
    /// configuration this service installs — holds nothing configured by its orders;
    /// and neither does one whose every compute asset a task has since reconfigured.
    fn holds_unreconfigured_compute(
        &self,
        region: &str,
        outpost_id: &str,
        standing: &BTreeMap<Option<&str>, &CapacityTaskRecord>,
    ) -> bool {
        self.outpost(region, outpost_id).is_some_and(|record| {
            record.assets.iter().any(|asset| {
                asset.compute_attributes.is_some()
                    && asset
                        .asset_id
                        .as_deref()
                        .is_none_or(|asset_id| !standing.contains_key(&Some(asset_id)))
            })
        })
    }

    /// The orders the Outpost `outpost_id` names in `region` is SUBSCRIBED to the
    /// hardware of: every order placed against that Outpost that was not cancelled, in
    /// the order the hardware was ordered.
    ///
    /// The ONE walk behind every report of what an Outpost's orders bought — the
    /// configuration its hardware arrived carrying
    /// ([`Self::ordered_instance_type_names`]) and the capacity under subscription on
    /// it ([`Self::subscribed_capacity_vcpus`]) — so no two of those reports can
    /// disagree about which orders count. Which they are is [`subscribes_hardware`]:
    /// nothing was ever delivered for a cancelled order, so it subscribes nothing.
    ///
    /// Ordered by the order's own creation sequence, so a report built from it does not
    /// depend on how the order store happens to be keyed.
    fn subscribed_orders(&self, region: &str, outpost_id: &str) -> Vec<&OrderRecord> {
        let mut orders: Vec<&OrderRecord> = all_in_region(&self.orders, region)
            .filter(|record| {
                record.order.outpost_id.as_deref() == Some(outpost_id)
                    && subscribes_hardware(&record.order)
            })
            .collect();
        orders.sort_by_key(|record| record.sequence);
        orders
    }

    /// The instance types the hardware the Outpost `outpost_id` names in `region`
    /// arrived carrying: the largest size of each instance family the configurations
    /// its uncancelled orders subscribe to publish EC2 capacity of, in the order those
    /// orders name them.
    ///
    /// Read off the one walk over those orders ([`Self::subscribed_orders`]). See
    /// [`Self::configured_instance_types`].
    fn ordered_instance_type_names(&self, region: &str, outpost_id: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for record in self.subscribed_orders(region, outpost_id) {
            for item in record.order.line_items.iter().flatten() {
                let Some(catalog_item) = item
                    .catalog_item_id
                    .as_deref()
                    .and_then(|catalog_item_id| self.catalog_item(catalog_item_id))
                else {
                    continue;
                };
                for capacity in catalog_item.item.ec2_capacities.iter().flatten() {
                    if let Some(max_size) = capacity.max_size.as_deref() {
                        push_once(&mut names, max_size);
                    }
                }
            }
        }
        names
    }

    /// The EC2 capacity, in vCPUs, UNDER SUBSCRIPTION on the Outpost `outpost_id`
    /// names in `region`: for every order placed against that Outpost and not
    /// cancelled, the published capacity of each line item's catalog item times the
    /// quantity of it the order carries.
    ///
    /// This is the hardware a renewal of the Outpost's contract covers, and the only
    /// thing the store knows about its size. The orders it is read off are the ones the
    /// Outpost is subscribed to the hardware of ([`Self::subscribed_orders`]) and what
    /// each of their line items subscribes is the one per-line-item fold
    /// ([`subscribed_capacities`]), asked for the vCPU figure the catalog publishes —
    /// so a line item naming a catalog item this service publishes no configuration for
    /// contributes nothing, for the reason stated there.
    pub fn subscribed_capacity_vcpus(&self, region: &str, outpost_id: &str) -> i64 {
        self.subscribed_orders(region, outpost_id)
            .into_iter()
            .flat_map(|record| {
                subscribed_capacities(&record.order, |catalog_item_id| {
                    self.catalog_item_capacity_vcpus(catalog_item_id)
                        .map(i64::from)
                })
            })
            .fold(0, i64::saturating_add)
    }

    /// The Amazon S3 storage capacity, in bytes, PROVISIONED on each Outpost that has
    /// any: for every order placed and not cancelled, the published S3 capacity of
    /// each line item's catalog item times the quantity of it the order carries,
    /// totalled per Outpost and keyed by region and then `OutpostId` — the pair that
    /// names one Outpost, since an order is placed in the region its Outpost belongs
    /// to.
    ///
    /// Storage capacity is installed on an Outpost by ordering hardware that carries
    /// it, so what is provisioned is what the orders on it bought, and an Outpost
    /// nobody ordered S3-capable hardware for is absent rather than present with
    /// zero: it has no S3 storage on it at all. Which orders bought it is the same
    /// question [`Self::subscribed_orders`] answers ([`subscribes_hardware`]), and what
    /// each of their line items bought is the one per-line-item fold
    /// ([`subscribed_capacities`]), asked for the S3 figure the catalog publishes — so
    /// a line item naming a catalog item this service publishes no configuration for
    /// contributes nothing, for the reason it contributes no vCPUs to
    /// [`Self::subscribed_capacity_vcpus`].
    ///
    /// Totalled in ONE pass over the orders, which is why it walks the whole order
    /// store rather than asking [`Self::subscribed_orders`] per Outpost: a caller that
    /// needs the figure for every Outpost at once — the S3 on Outposts inventory does —
    /// pays for the orders the store holds and not for their product with the Outposts.
    pub fn provisioned_s3_capacity(&self) -> BTreeMap<String, BTreeMap<String, i64>> {
        let mut provisioned: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
        for record in self
            .orders
            .values()
            .filter(|record| subscribes_hardware(&record.order))
        {
            let Some(outpost_id) = record.order.outpost_id.as_deref() else {
                continue;
            };
            for capacity in subscribed_capacities(&record.order, |catalog_item_id| {
                self.catalog_item_s3_capacity_bytes(catalog_item_id)
            }) {
                let total = provisioned
                    .entry(record.region.clone())
                    .or_default()
                    .entry(outpost_id.to_string())
                    .or_default();
                *total = total.saturating_add(capacity);
            }
        }
        provisioned
    }

    /// The verdict on the capacity `configuration` a request asks of one Outpost's
    /// hardware: whether what it asks for is at or above the instance capacity
    /// available to it.
    ///
    /// "You can request a dry run to determine if the instance type and instance size
    /// changes is above or below available instance capacity", as the modelled
    /// `DryRun` documents it, so this is [`Self::weigh_capacity`] read as that one
    /// question ([`CapacityWeighing::verdict`]) — the SAME weighing the course a real
    /// task takes is read off ([`Self::capacity_task_course`]), so a dry run and the
    /// identical real request never contradict each other about the configuration.
    ///
    /// A READ: it computes the verdict and writes nothing, which is what lets a dry
    /// run — the one call that asks for it — leave the store exactly as it found it.
    pub fn capacity_verdict(&self, configuration: &CapacityConfiguration<'_>) -> CapacityVerdict {
        self.weigh_capacity(configuration).verdict()
    }

    /// The course a capacity task started NOW on the capacity `configuration` a
    /// request asks owes the clock: what the task does, weighed when it is accepted.
    ///
    /// Every outcome follows from the one weighing of the request against what is
    /// persisted ([`Self::weigh_capacity`]) and from `task_action_on_blocking_instances`:
    ///
    /// * a configuration the hardware in scope cannot be configured with is the
    ///   modelled unsupported-configuration failure — the same answer the DRY RUN of
    ///   that request reports ([`Self::capacity_verdict`]), because it is the same
    ///   weighing;
    /// * a configuration it can hold, but not beside the instances the task would
    ///   have to stop, is what the caller's action decides: `FAIL_TASK` is "the
    ///   capacity task fails", so the task fails naming the capacity still held, while
    ///   `WAIT_FOR_EVACUATION` (and a request that named no action, which asked for no
    ///   failure) waits on them ([`CAPACITY_TASK_WAITING_FOR_EVACUATION`]);
    /// * a configuration that fits beside everything already running runs to
    ///   `COMPLETED` ([`capacity_task_progression`]).
    ///
    /// A READ: it writes nothing. The course it returns is what the caller stores on
    /// the task it is creating, so the weighing happens once, at the instant the task
    /// is accepted, and every later report of that task reads the same course.
    pub fn capacity_task_course(
        &self,
        configuration: &CapacityConfiguration<'_>,
        task_action_on_blocking_instances: Option<&str>,
        now: std::time::SystemTime,
    ) -> VecDeque<CapacityTaskTransition> {
        let weighing = self.weigh_capacity(configuration);
        if let Some(failure) = weighing.verdict().failed {
            return capacity_task_failure(now, failure);
        }
        if weighing.needs_evacuation() {
            if task_action_on_blocking_instances == Some(TASK_ACTION_FAIL_TASK) {
                return capacity_task_failure(
                    now,
                    CapacityTaskFailure {
                        reason: blocking_instances_not_evacuated(
                            weighing.requested,
                            weighing.free,
                        ),
                        r#type: Some(
                            CAPACITY_TASK_FAILURE_BLOCKING_INSTANCES_NOT_EVACUATED.to_string(),
                        ),
                    },
                );
            }
            return capacity_task_evacuation_wait(now);
        }
        capacity_task_progression(now)
    }

    /// Weigh the capacity `configuration` a request asks of one Outpost's hardware
    /// against what is persisted for that hardware.
    ///
    /// Three figures, all in the vCPUs every instance-capacity figure of this service
    /// is published in ([`Self::published_vcpus`]):
    ///
    /// * what the configuration ASKS FOR: each pool's `Count` ("the number of
    ///   instances for the specified instance type") times the default vCPUs the
    ///   catalog publishes for its instance type.
    /// * what the hardware in scope has FREE — the one asset the request names, or
    ///   every compute asset of the Outpost when it names none, because a capacity
    ///   task configures the hardware it names. That is the vCPU ceiling of that
    ///   hardware (`ComputeAttributes$MaxVcpus`) less the vCPUs held by every instance
    ///   recorded as running on it.
    /// * what it would have free once the instances this task MAY stop were stopped:
    ///   the same ceiling less only the capacity held by instances it could not stop —
    ///   an instance the request excludes must not be stopped and one belonging to
    ///   another account is not this caller's to stop
    ///   ([`instance_can_be_stopped`]) — which is the capacity a configuration is
    ///   weighed as being "above or below".
    ///
    /// A pool naming an instance type none of the hardware in scope runs
    /// ([`compute_runs`]), or one this service publishes no vCPU figure for, is a
    /// configuration that hardware cannot hold whatever is free on it. Hardware that
    /// recorded nothing about its capacity carries none and hardware that is not there
    /// carries none either, so an Outpost holding no compute asset can take no pool: a
    /// request is weighed against what is persisted, never against a figure the
    /// emulator invents.
    ///
    /// A READ: it computes the figures and writes nothing.
    ///
    /// What it COSTS is proportional to the request it was given and nothing else: the
    /// published catalog is read into one lookup and the hardware in scope into one
    /// list before any pool is weighed, and whether that hardware runs a given
    /// instance type is decided once per DISTINCT type the catalog publishes — so a
    /// pool costs a lookup and an addition however many pools arrive, and no figure
    /// here multiplies the length of the request by anything that grows.
    fn weigh_capacity(&self, configuration: &CapacityConfiguration<'_>) -> CapacityWeighing {
        let &CapacityConfiguration {
            region,
            outpost_id,
            asset_id,
            pools,
            instances_to_exclude,
        } = configuration;
        let record = self.outpost(region, outpost_id);
        // Which hardware the configuration is asked of: the asset the request named,
        // or all of the Outpost's when it named none. Read once, because every pool
        // is asked of the same hardware.
        let in_scope = |asset: Option<&str>| in_capacity_task_scope(asset_id, asset);
        let hardware: Vec<&ComputeAttributes> = record
            .iter()
            .flat_map(|record| record.assets.iter())
            .filter(|asset| in_scope(asset.asset_id.as_deref()))
            .filter_map(|asset| asset.compute_attributes.as_ref())
            .collect();
        // The vCPUs the catalog publishes for each instance type, as ONE lookup over
        // the catalog this service publishes ([`Self::published_vcpus`] is the same
        // figure, found by walking it) — so weighing a pool is a lookup rather than a
        // walk, whatever a request carries.
        let published: BTreeMap<&str, i32> = self
            .orderable_instance_types
            .iter()
            .filter_map(|record| {
                Some((
                    record.instance_type.instance_type.as_deref()?,
                    record.instance_type.vcp_us?,
                ))
            })
            .collect();
        // Which of those types the hardware in scope runs, decided the first time a
        // pool names one: the catalog publishes finitely many types and only a
        // published one is ever asked about, so what this holds is bounded by the
        // catalog and not by how many pools arrived.
        let mut runs: BTreeMap<&str, bool> = BTreeMap::new();

        // What the configuration asks for, and whether the hardware could hold it at
        // all. A pool of an instance type the catalog publishes no vCPU count for
        // cannot be weighed — the emulator has no size for that type and never
        // invents one — and one of a type that hardware does not run is not a
        // configuration it can hold, whatever capacity happens to be free on it.
        let mut requested: i64 = 0;
        let mut unsupported: Option<String> = None;
        for pool in pools {
            let instance_type = pool.instance_type.as_str();
            let Some(&vcpus) = published.get(instance_type) else {
                unsupported = Some(instance_type_not_run(instance_type));
                break;
            };
            let runs_it = *runs.entry(instance_type).or_insert_with(|| {
                hardware.iter().any(|compute| {
                    compute_runs(compute, instance_type_family(instance_type), Some(vcpus))
                })
            });
            if !runs_it {
                unsupported = Some(instance_type_not_run(instance_type));
                break;
            }
            // `InstanceTypeCount` is modelled `range(0..9999)`, so a count the
            // boundary accepted is never negative. Saturating, so no length of
            // request can carry the total past what the figure is held in.
            requested = requested
                .saturating_add(i64::from(vcpus).saturating_mul(i64::from(pool.count.max(0))));
        }

        // What that hardware carries: the sum of its hosts' vCPU ceilings. A host
        // that recorded no ceiling carries no figure, and so no capacity.
        let ceiling: i64 = hardware
            .iter()
            .filter_map(|compute| compute.max_vcpus)
            .map(i64::from)
            .sum();
        // What of it the instances running on it hold, split by whether this task
        // could free it. What the request excludes is read into [`ExcludedInstances`]
        // once, so an instance costs a lookup in it rather than a walk of it.
        let exclude = ExcludedInstances::of(instances_to_exclude);
        let mut held: i64 = 0;
        let mut held_unstoppable: i64 = 0;
        for instance in record
            .iter()
            .flat_map(|record| record.instances.iter())
            .filter(|instance| in_scope(instance.asset_id.as_deref()))
        {
            let Some(&vcpus) = instance
                .instance_type
                .as_deref()
                .and_then(|instance_type| published.get(instance_type))
            else {
                continue;
            };
            held = held.saturating_add(i64::from(vcpus));
            if !instance_can_be_stopped(instance, asset_id, &exclude) {
                held_unstoppable = held_unstoppable.saturating_add(i64::from(vcpus));
            }
        }
        CapacityWeighing {
            unsupported,
            requested,
            free: (ceiling - held).max(0),
            free_after_evacuation: (ceiling - held_unstoppable).max(0),
        }
    }

    /// How many renewal contracts this store holds, across every Outpost of every
    /// region.
    ///
    /// A renewal is never released — the Outpost it belongs to is what takes it away
    /// — so what `CreateRenewal` grows is capped by the store's own entry ceiling
    /// ([`MAX_RESOURCES`]), and that cap is on the whole store rather than on one
    /// Outpost's share of it.
    pub fn renewals_held(&self) -> usize {
        self.outposts
            .values()
            .map(|record| record.renewals.len())
            .sum()
    }

    /// Every renewal pricing option for the Outpost `outpost_id` names in `region`:
    /// one priced SUBSCRIPTION option per (`PaymentOption`, `PaymentTerm`)
    /// combination the model declares — three of each, so the nine the modelled
    /// `PricingOptionList` maximum (`length(0..9)`) allows — or none at all for an
    /// Outpost with no subscribed hardware to renew.
    ///
    /// Each option is the one subscription on the Outpost's own hardware, quoted
    /// under that combination by [`Self::renewal_price`] — so every figure follows
    /// from what is persisted for the Outpost, and a read answers the same figures
    /// as the next one.
    pub fn renewal_pricing(&self, region: &str, outpost_id: &str) -> Vec<PricingOption> {
        // The tables may only ever name values the model declares, and must name
        // every one of them, so a model that adds or renames a payment option or
        // term trips this in every debug build instead of letting a renewal be
        // priced under fewer combinations than the service offers.
        debug_assert!(
            PAYMENT_OPTION_UPFRONT_SHARE.len() == crate::ops::types::PAYMENT_OPTION_VALUES.len()
                && PAYMENT_OPTION_UPFRONT_SHARE.iter().all(|(option, _)| {
                    crate::ops::types::PAYMENT_OPTION_VALUES.contains(option)
                })
                && PAYMENT_TERM_MONTHS.len() == crate::ops::types::PAYMENT_TERM_VALUES.len()
                && PAYMENT_TERM_MONTHS
                    .iter()
                    .all(|(term, _)| crate::ops::types::PAYMENT_TERM_VALUES.contains(term)),
            "every modelled PaymentOption has an upfront share and every modelled PaymentTerm a \
             length in months"
        );
        // The combinations are walked in the order the model declares the two value
        // sets in, so one Outpost's options are reported in the same order on every
        // read and in every process. Each one is priced by [`Self::renewal_price`],
        // the ONE rule this service prices a renewal by — so what the pricing read
        // reports and what a renewal is actually bought at cannot disagree.
        let options: Vec<PricingOption> = crate::ops::types::PAYMENT_OPTION_VALUES
            .iter()
            .flat_map(|payment_option| {
                crate::ops::types::PAYMENT_TERM_VALUES
                    .iter()
                    .map(move |payment_term| (payment_option, payment_term))
            })
            .filter_map(|(payment_option, payment_term)| {
                Some(PricingOption {
                    pricing_type: Some(QUOTE_PRICING_TYPE_SUBSCRIPTION.to_string()),
                    subscription_pricing_details: Some(self.renewal_price(
                        region,
                        outpost_id,
                        payment_option,
                        payment_term,
                    )?),
                })
            })
            .collect();
        debug_assert!(
            options.len() <= PRICING_OPTION_MAX,
            "a renewal is priced under at most the options PricingOptionList allows"
        );
        options
    }

    /// What a renewal of the subscription on the Outpost `outpost_id` names in
    /// `region`, bought under `payment_option` and `payment_term`, costs — or `None`
    /// when there is nothing to price: an Outpost with no subscribed hardware, or a
    /// payment option or term this service holds no rule for.
    ///
    /// The capacity the Outpost's orders subscribe to
    /// ([`Self::subscribed_capacity_vcpus`]) is what a renewal covers and the only
    /// thing this store knows about its size, so that capacity is what the one
    /// pricing rule ([`subscription_price`]) is applied to — which is why the pricing
    /// read ([`Self::renewal_pricing`]) and the renewal a caller actually buys quote
    /// the same figures.
    pub fn renewal_price(
        &self,
        region: &str,
        outpost_id: &str,
        payment_option: &str,
        payment_term: &str,
    ) -> Option<SubscriptionPricingDetails> {
        subscription_price(
            self.subscribed_capacity_vcpus(region, outpost_id) as f64,
            payment_option,
            payment_term,
        )
    }

    /// The configuration-and-pricing options this service answers a quote for
    /// `capacities` with, priced under the payment options and terms that quote
    /// requests.
    ///
    /// "A quote provides pricing and configuration options based on the requested
    /// capacity", and `CreateOrder`'s `QuoteOptionIdentifier` can only ever be
    /// obtained from one of them, so a quote this service answers carries an option a
    /// caller can order from. The request describes ONE configuration — the capacity
    /// it asks for — so one option is derived for it, well within what the modelled
    /// `QuoteOptionList` ([`QUOTE_OPTION_MAX`]) allows, and it reports the members
    /// that follow from the quote:
    ///
    /// * `QuoteOptionIdentifier` (`^oqo-[a-f0-9]{17}$`), minted here and persisted
    ///   WITH the quote, so every later read of that quote names the same option and
    ///   an order can be submitted against it.
    /// * `Capacities`, "the capacities included in this quote option": the
    ///   configuration is the one the quote asks for, so it provides exactly that
    ///   capacity.
    /// * `PricingOptions`, one per payment combination the quote is priced under
    ///   ([`Self::quote_pricing`]).
    ///
    /// `CapacitySummary` and `Specifications` are absent. The first is a summary of
    /// the capacity that EXISTS against the capacity the option would leave, and this
    /// store holds no record of an Outpost's capacity per capacity unit — EC2
    /// instances per instance type, EBS and S3 volume — so a summary of it would be
    /// the request read back as if it were the Outpost's. The second is the physical
    /// rack or server a configuration is deployed on, which AWS picks when it builds
    /// the order and this store records for no quote.
    pub fn quote_options(
        &mut self,
        capacities: &[QuoteCapacity],
        payment_options: Option<&[String]>,
        payment_terms: Option<&[String]>,
    ) -> Vec<QuoteOption> {
        let pricing_options = self.quote_pricing(capacities, payment_options, payment_terms);
        let (quote_option_identifier, _) = self.mint_id(QUOTE_OPTION_ID_PREFIX);
        let options = vec![QuoteOption {
            quote_option_identifier: Some(quote_option_identifier),
            capacities: Some(capacities.to_vec()),
            capacity_summary: None,
            specifications: None,
            pricing_options: Some(pricing_options),
        }];
        debug_assert!(
            options.len() <= QUOTE_OPTION_MAX,
            "a quote reports at most the options QuoteOptionList allows"
        );
        options
    }

    /// Every pricing option a quote option providing `capacities` is reported under:
    /// one priced SUBSCRIPTION option per combination of the payment options and
    /// payment terms the quote requests ([`requested_or_modelled`], so a quote that
    /// requested neither is priced under every modelled value).
    ///
    /// The combinations are walked in the order the two value sets name them, so one
    /// configuration's options are reported in the same order on every read and in
    /// every process. Each is priced by [`subscription_price`] — the ONE rule this
    /// service prices a subscription by — on the EC2 capacity the option provides
    /// ([`Self::quoted_capacity_vcpus`]), so what a quote quotes and what a renewal
    /// of the same capacity costs cannot disagree. A combination there is nothing to
    /// price under is left out rather than reported at a figure the emulator invented
    /// — including one whose figures are beyond what a modelled price member can
    /// carry, for the reason [`subscription_price`] states.
    fn quote_pricing(
        &self,
        capacities: &[QuoteCapacity],
        payment_options: Option<&[String]>,
        payment_terms: Option<&[String]>,
    ) -> Vec<PricingOption> {
        let vcpus = self.quoted_capacity_vcpus(capacities);
        let options =
            requested_or_modelled(payment_options, crate::ops::types::PAYMENT_OPTION_VALUES);
        let terms = requested_or_modelled(payment_terms, crate::ops::types::PAYMENT_TERM_VALUES);
        let priced: Vec<PricingOption> = options
            .iter()
            .flat_map(|payment_option| {
                terms
                    .iter()
                    .map(move |payment_term| (*payment_option, *payment_term))
            })
            .filter_map(|(payment_option, payment_term)| {
                Some(PricingOption {
                    pricing_type: Some(QUOTE_PRICING_TYPE_SUBSCRIPTION.to_string()),
                    subscription_pricing_details: Some(subscription_price(
                        vcpus,
                        payment_option,
                        payment_term,
                    )?),
                })
            })
            .collect();
        debug_assert!(
            priced.len() <= PRICING_OPTION_MAX,
            "a quote option is priced under at most the options PricingOptionList allows"
        );
        priced
    }

    /// How many vCPUs of EC2 capacity the configuration `capacities` describes
    /// carries, as this service's published catalog figures it: for every EC2
    /// capacity, the default vCPUs published for the instance type its `Unit` names
    /// ([`Self::published_vcpus`]) times the quantity of instances asked for.
    ///
    /// "For Amazon EC2, this is the instance type (for example, `c5.24xlarge`)", as
    /// the model documents `QuoteCapacity$Unit`, and "for Amazon EC2, the quantity is
    /// the number of additional instances to add to the Outpost" — so this is the
    /// capacity of a quoted configuration, read off the same catalog
    /// `ListOrderableInstanceTypes` publishes. A capacity measured in storage rather
    /// than in instances (the modelled `EBS` and `S3` types), one naming a unit the
    /// catalog publishes no instance type for, and one asking for no instances at all
    /// contribute nothing: capacity this service publishes no figure for is not
    /// capacity the emulator can price, and never one it invents.
    ///
    /// Totalled in double precision, which is wide enough for every total the
    /// modelled request can describe — the largest `QuoteCapacity$Quantity` its
    /// single-precision type carries, times the largest vCPU figure the catalog
    /// publishes, over the whole `QuoteCapacityList` — so the capacity a quote is
    /// priced on is the capacity it asked for, and never a value the arithmetic
    /// saturated to.
    pub fn quoted_capacity_vcpus(&self, capacities: &[QuoteCapacity]) -> f64 {
        capacities
            .iter()
            .filter(|capacity| {
                capacity.quote_capacity_type.as_deref() == Some(QUOTE_CAPACITY_TYPE_EC2)
            })
            .filter_map(|capacity| {
                let vcpus = self.published_vcpus(capacity.unit.as_deref()?)?;
                let quantity = capacity
                    .quantity
                    .filter(|quantity| quantity.is_finite() && *quantity > 0.0)?;
                Some(f64::from(quantity) * f64::from(vcpus))
            })
            .sum()
    }

    /// The requirements that must be met before an order can be submitted for a quote
    /// of `country_code` associated with the Outpost `outpost_id` names in `region`
    /// (or with no Outpost at all).
    ///
    /// "The requirements that must be met before an order can be submitted for the
    /// quote", and the modelled `OrderingRequirementType` names the checks AWS runs
    /// (see [`crate::ops::types::ORDERING_REQUIREMENT_TYPE_VALUES`]). A check is
    /// reported only where this store holds the records to decide it, and a reported
    /// one carries the outcome those records give at the instant the quote is priced:
    ///
    /// * [`ORDERING_REQUIREMENT_OUTPOST_ID_MISSING`] — whether the quote names an
    ///   Outpost, which is the association `CreateQuote` and `UpdateQuote` record.
    /// * [`ORDERING_REQUIREMENT_MAXIMUM_ALLOWED_ORDERS`] — whether this store can
    ///   still take an order, which is the ceiling `CreateOrder` refuses at.
    /// * [`ORDERING_REQUIREMENT_OUTPOST_ACTIVE`] — whether that Outpost has settled
    ///   in its terminal status. Callers materialise the lifecycle transition the
    ///   clock owes first (see [`Self::settle_outposts`]), so the status read here is
    ///   the one the Outpost holds at that same instant.
    /// * [`ORDERING_REQUIREMENT_OPERATING_ADDRESS`] and
    ///   [`ORDERING_REQUIREMENT_SHIPPING_ADDRESS`] — whether the site that Outpost
    ///   stands on records each of the two addresses an order is placed against.
    /// * [`ORDERING_REQUIREMENT_COUNTRY_CODE_MISMATCH`] — whether the country the
    ///   quote is for is the country of that site, which only a site with a recorded
    ///   operating address answers.
    ///
    /// Every outcome follows from a record this store holds, so a check is never
    /// reported as passed because nothing was looked at — and the checks that depend
    /// on an Outpost are reported only for a quote that names one, because a quote
    /// associated with no Outpost gives them nothing to look at.
    pub fn ordering_requirements(
        &self,
        region: &str,
        country_code: &str,
        outpost_id: Option<&str>,
    ) -> Vec<OrderingRequirement> {
        let mut requirements = vec![match outpost_id {
            Some(outpost_id) => ordering_requirement(
                ORDERING_REQUIREMENT_OUTPOST_ID_MISSING,
                ORDERING_REQUIREMENT_PASS,
                format!("the quote is associated with Outpost {outpost_id}"),
            ),
            None => ordering_requirement(
                ORDERING_REQUIREMENT_OUTPOST_ID_MISSING,
                ORDERING_REQUIREMENT_FAIL,
                "the quote is associated with no Outpost".to_string(),
            ),
        }];
        let placed = self.orders.len();
        requirements.push(ordering_requirement(
            ORDERING_REQUIREMENT_MAXIMUM_ALLOWED_ORDERS,
            pass_or_fail(placed < MAX_RESOURCES),
            format!("{placed} of the {MAX_RESOURCES} orders this service holds have been placed"),
        ));
        if let Some(record) = outpost_id.and_then(|outpost_id| self.outpost(region, outpost_id)) {
            if let (Some(outpost_id), Some(status)) = (
                record.outpost.outpost_id.as_deref(),
                record.outpost.life_cycle_status.as_deref(),
            ) {
                requirements.push(ordering_requirement(
                    ORDERING_REQUIREMENT_OUTPOST_ACTIVE,
                    pass_or_fail(status == LIFE_CYCLE_STATUS_ACTIVE),
                    format!("Outpost {outpost_id} is {status}"),
                ));
            }
            let site = record
                .outpost
                .site_id
                .as_deref()
                .and_then(|site_id| Some((site_id, self.site(region, site_id)?)));
            if let Some((site_id, site)) = site {
                for (check, kind, address) in [
                    (
                        ORDERING_REQUIREMENT_OPERATING_ADDRESS,
                        SiteAddressKind::Operating,
                        "operating address",
                    ),
                    (
                        ORDERING_REQUIREMENT_SHIPPING_ADDRESS,
                        SiteAddressKind::Shipping,
                        "shipping address",
                    ),
                ] {
                    let recorded = site.address(kind).is_some();
                    requirements.push(ordering_requirement(
                        check,
                        pass_or_fail(recorded),
                        match recorded {
                            true => format!("site {site_id} has a recorded {address}"),
                            false => format!("site {site_id} has no recorded {address}"),
                        },
                    ));
                }
                if let Some(country) = site
                    .address(SiteAddressKind::Operating)
                    .map(|address| address.country_code.as_str())
                {
                    requirements.push(ordering_requirement(
                        ORDERING_REQUIREMENT_COUNTRY_CODE_MISMATCH,
                        pass_or_fail(country == country_code),
                        match country == country_code {
                            true => format!(
                                "the quote's country code {country_code} is the country of site \
                                 {site_id}"
                            ),
                            false => format!(
                                "the quote's country code {country_code} is not {country}, the \
                                 country of site {site_id}"
                            ),
                        },
                    ));
                }
            }
        }
        debug_assert!(
            requirements.len() <= ORDERING_REQUIREMENT_MAX
                && requirements.iter().all(|requirement| {
                    requirement
                        .ordering_requirement_type
                        .as_deref()
                        .is_some_and(|check| {
                            crate::ops::types::ORDERING_REQUIREMENT_TYPE_VALUES.contains(&check)
                        })
                        && requirement.status.as_deref().is_some_and(|status| {
                            crate::ops::types::ORDERING_REQUIREMENT_STATUS_VALUES.contains(&status)
                        })
                }),
            "every requirement reported names a modelled check with a modelled status, and a \
             quote reports at most the requirements OrderingRequirementList allows"
        );
        requirements
    }

    /// Materialise every Outpost lifecycle transition the clock has already made due,
    /// and with it the hardware the Outpost settles with.
    ///
    /// Every operation that reports or filters on an Outpost, or that reads one of its
    /// assets, calls this FIRST, so a read answers the Outpost — and the hardware —
    /// as it stands at the instant it is asked for and never a stale one. It writes
    /// only what the clock already owed at that same instant, so reading changes
    /// nothing a later caller would not have seen anyway, and a settled Outpost is
    /// left alone.
    ///
    /// An Outpost is installed with the hardware an order placed on it delivers, so
    /// the order transitions the clock already owed are materialised first
    /// ([`Self::settle_orders`]): the install applied here is the one an order's
    /// fulfilment owed, and an Outpost nobody has ordered hardware for is owed none
    /// and goes on reporting [`LIFE_CYCLE_STATUS_PENDING`].
    ///
    /// The transition that ends an install is when that hardware goes into service:
    /// the Outpost's configuration is installed on it ([`installed_hardware`]) as it
    /// settles, and an Outpost that already holds hardware keeps exactly the hardware
    /// it holds — one configuration is installed once. The configuration read here is
    /// the one the Outpost's form factor names at this instant, so an install the
    /// Outpost was owed by a later form factor
    /// ([`OutpostRecord::set_supported_hardware_type`]) places that form factor's
    /// hardware.
    pub fn settle_outposts(&mut self) {
        self.settle_orders();
        let now = std::time::Instant::now();
        // What the clock owes each Outpost whose provisioning has come due: the hardware
        // to install on it, or none for one that already holds its own. Worked out
        // against the catalog first and applied after, because the hardware a unit runs
        // is read from the same catalog `ListOrderableInstanceTypes` publishes.
        let settled: Vec<(String, Vec<AssetInfo>)> = self
            .outposts
            .iter()
            .filter(|(_, record)| record.active_at.is_some_and(|due| now >= due))
            .map(|(outpost_id, record)| {
                let hardware = match record.assets.is_empty() {
                    true => installed_hardware(
                        record.sequence,
                        record.outpost.supported_hardware_type.as_deref(),
                        &self.orderable_instance_types,
                    ),
                    false => Vec::new(),
                };
                (outpost_id.clone(), hardware)
            })
            .collect();
        for (outpost_id, hardware) in settled {
            // The install places hardware, so each unit it places takes its own
            // creation sequence out of the store's creation order the moment it is
            // placed (see [`OutpostRecord::asset_sequence_base`]).
            let installed = match hardware.is_empty() {
                true => None,
                false => Some(self.reserve_sequences(hardware.len())),
            };
            let Some(record) = self.outposts.get_mut(&outpost_id) else {
                continue;
            };
            record.outpost.life_cycle_status = Some(LIFE_CYCLE_STATUS_ACTIVE.to_string());
            record.active_at = None;
            if let Some(base) = installed {
                record.asset_sequence_base = base;
                record.assets = hardware;
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
    ///
    /// A step that fails the task carries the `CapacityTaskFailure` the course was
    /// weighed with ([`Self::capacity_task_course`]), so `Failed` appears exactly when
    /// the status it explains does and never before it.
    ///
    /// A task reaching [`CAPACITY_TASK_COMPLETED`] is a task that has RUN, so the step
    /// that completes it also leaves its configuration on the hardware it named
    /// ([`Self::configure_hardware`]): the pools of a completed task are the capacity
    /// configured on that hardware, which is the one thing the configuration a task
    /// leaves behind can be read off. Nothing else here writes outside the task
    /// record.
    pub fn settle_capacity_tasks(&mut self) {
        let now = std::time::Instant::now();
        // What the tasks completing in this settle leave configured on the hardware
        // they named. Collected while the tasks are walked and applied after, because
        // both records live in this one store (as in [`Self::settle_orders`]).
        let mut configured: Vec<SettledConfiguration> = Vec::new();
        for record in self.capacity_tasks.values_mut() {
            while record.pending.front().is_some_and(|next| now >= next.due) {
                let Some(next) = record.pending.pop_front() else {
                    break;
                };
                record.last_modified_at = next.at;
                if next.status == CAPACITY_TASK_COMPLETED {
                    record.completed_at = Some(next.at);
                    configured.push(SettledConfiguration {
                        at: next.at,
                        sequence: record.sequence,
                        outpost_id: record.outpost_id.clone(),
                        asset_id: record.asset_id.clone(),
                        capacities: configured_capacities(record),
                    });
                }
                record.status = next.status;
                record.failed = next.failed;
            }
        }
        // In the order the completions were OWED, so where two tasks configured one
        // piece of hardware the one that completed last is the configuration that
        // stands — the same rule the Outpost-wide report reads them by
        // ([`Self::configured_instance_types`]).
        configured.sort_by(|left, right| {
            (left.at, left.sequence).cmp(&(right.at, right.sequence))
        });
        for settled in configured {
            self.configure_hardware(
                &settled.outpost_id,
                settled.asset_id.as_deref(),
                settled.capacities,
            );
        }
    }

    /// Record `capacities` as the per-instance-type capacity CONFIGURED on the compute
    /// hardware a completed capacity task named on the Outpost `outpost_id`: the asset
    /// `asset_id` names, or the Outpost's own compute hardware when the task named
    /// none.
    ///
    /// "The instance type capacities configured for this asset. This can be changed
    /// through a capacity task", as `ComputeAttributes$InstanceTypeCapacities`
    /// documents it — so the capacity a task leaves behind is reported on the hardware
    /// that task configured, and a task that leaves it configured with no instance of
    /// any type (every pool at a `Count` that configures none) leaves it reporting no
    /// per-type capacity at all, exactly as hardware no task has configured does.
    ///
    /// Capacity is configured ON compute hardware, so a task that named a unit
    /// carrying no `ComputeAttributes` configures nothing. A task that named NO asset
    /// configured the Outpost ITSELF, which is the compute hardware no task of its own
    /// has reconfigured ([`Self::configured_instance_types`] reads the same records by
    /// that same scope): where that hardware is a single asset it is that asset's
    /// configuration, and where it is several the split of the task's pools between
    /// them is not a fact this service holds, so none of them is recorded a split
    /// nobody asked for.
    fn configure_hardware(
        &mut self,
        outpost_id: &str,
        asset_id: Option<&str>,
        capacities: Vec<AssetInstanceTypeCapacity>,
    ) {
        // Which of the Outpost's hardware the task configured, resolved before
        // anything is written because the Outpost-wide case reads the tasks already
        // standing on that Outpost's assets.
        let configured = match asset_id {
            // The one asset the task named.
            Some(asset_id) => Some(asset_id.to_string()),
            // The Outpost itself. See [`Self::sole_unreconfigured_compute`].
            None => self.sole_unreconfigured_compute(outpost_id),
        };
        let Some(record) = self.outposts.get_mut(outpost_id) else {
            return;
        };
        // Capacity is configured ON compute hardware, so only a unit carrying
        // `ComputeAttributes` takes a configuration at all.
        let configured = configured.and_then(|asset_id| {
            record
                .assets
                .iter_mut()
                .filter(|asset| asset.compute_attributes.is_some())
                .find(|asset| asset.asset_id.as_deref() == Some(asset_id.as_str()))
        });
        if let Some(attributes) = configured.and_then(|asset| asset.compute_attributes.as_mut()) {
            attributes.instance_type_capacities = (!capacities.is_empty()).then_some(capacities);
        }
    }

    /// The `AssetId` of the compute hardware a task that named NO asset configured on
    /// the Outpost `outpost_id`, or `None` where no one asset of it is that hardware.
    ///
    /// A task that named no asset configured the Outpost itself, which is the compute
    /// hardware no task of the Outpost's own has reconfigured — the scope
    /// [`Self::configured_instance_types`] reads the same stored tasks by, so the
    /// per-asset view and the Outpost-wide one cannot disagree about which task
    /// configured which hardware. One asset of it is that hardware exactly when the
    /// Outpost holds a single compute asset and no COMPLETED task of its own has named
    /// that asset; an Outpost holding several (or holding that asset reconfigured
    /// already) is hardware whose share of the pools this service does not hold.
    fn sole_unreconfigured_compute(&self, outpost_id: &str) -> Option<String> {
        let mut compute = self
            .outposts
            .get(outpost_id)?
            .assets
            .iter()
            .filter(|asset| asset.compute_attributes.is_some());
        let sole = compute.next()?;
        if compute.next().is_some() {
            return None;
        }
        let asset_id = sole.asset_id.as_deref()?;
        let reconfigured = self.capacity_tasks.values().any(|task| {
            task.outpost_id == outpost_id
                && task.status == CAPACITY_TASK_COMPLETED
                && task.asset_id.as_deref() == Some(asset_id)
        });
        (!reconfigured).then(|| asset_id.to_string())
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
    /// that same instant — every step whose due instant has passed, so a caller that
    /// reads late sees the same status, with the same `OrderFulfilledDate`, as one
    /// that polled all along — and an order that has run out of transitions is left
    /// alone.
    ///
    /// A line item moves with its order: the order's `IN_PROGRESS` is "being built or
    /// shipped. To get more details, see the line item status", so the item-level
    /// status is what carries that detail. An item a cancellation finished is left
    /// where it stands.
    ///
    /// Fulfilling an order is what puts hardware on an Outpost, so the order reaching
    /// [`ORDER_STATUS_COMPLETED`] is also when the install the Outpost it was placed
    /// on is owed falls due ([`OutpostRecord::active_at`]) — at the instant the
    /// completion was owed, not at the instant anybody asked. An Outpost already owed
    /// an install keeps the instant it is already owed it at.
    pub fn settle_orders(&mut self) {
        let now = std::time::Instant::now();
        // The installs the orders that came due owe their Outposts. Collected while
        // the orders are walked and applied after, because both records live in this
        // one store.
        let mut installs: Vec<(String, std::time::Instant)> = Vec::new();
        for record in self.orders.values_mut() {
            while record.pending.front().is_some_and(|next| now >= next.due) {
                let Some(next) = record.pending.pop_front() else {
                    break;
                };
                record.order.status = Some(next.status.clone());
                for item in record.order.line_items.iter_mut().flatten() {
                    if item.status.as_deref() != Some(LINE_ITEM_STATUS_CANCELLED) {
                        item.status = Some(next.line_item_status.clone());
                    }
                }
                if next.status != ORDER_STATUS_COMPLETED {
                    continue;
                }
                // "The fulfillment date for the order": the order is fulfilled by the
                // hardware it was placed for being installed, which is this step.
                record.order.order_fulfilled_date = Some(timestamp(next.at));
                if let Some(outpost_id) = record.order.outpost_id.clone() {
                    installs.push((outpost_id, next.due));
                }
            }
        }
        for (outpost_id, due) in installs {
            let Some(record) = self.outposts.get_mut(&outpost_id) else {
                continue;
            };
            if record.active_at.is_none() {
                record.active_at = Some(due);
            }
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

    /// The capacity task of `region` that is still ACTIVE on `order_id`, if any. See
    /// [`Self::active_capacity_task_on_outpost`] — the same rule, stated for the
    /// other resource a task is one-at-a-time per.
    ///
    /// Scoped to `region` for the reason [`RegionalRecord`] gives: an order belongs
    /// to the region it was placed in, so only that region's tasks can be running on
    /// it, and a conflict is never reported about a task of an inventory the caller's
    /// endpoint does not serve. A task takes its region from its Outpost, which is
    /// what [`Self::capacity_tasks_in`] scopes by.
    pub fn active_capacity_task_on_order(
        &self,
        region: &str,
        order_id: &str,
    ) -> Option<&CapacityTaskRecord> {
        self.capacity_tasks_in(region)
            .find(|task| task.order_id.as_deref() == Some(order_id) && task.is_active())
    }

    /// The order of `region` still OUTSTANDING on `outpost_id`, if this store holds
    /// one.
    ///
    /// An order is outstanding until it is finished
    /// ([`OrderRecord::is_outstanding`]), so this is what the service still owes the
    /// Outpost. Stated once here so the two rules that read it — what blocks a
    /// decommission ([`Self::blocking_resource_types`]) and what blocks a delete
    /// ([`Self::unfinished_work_on_outpost`]) — cannot disagree about what "an order
    /// is still outstanding on the Outpost" means. Scoped to `region` for the reason
    /// [`RegionalRecord`] gives; callers settle first (see [`Self::settle_orders`]),
    /// so the statuses read here are the ones the orders hold at that instant.
    pub fn outstanding_order_on_outpost(
        &self,
        region: &str,
        outpost_id: &str,
    ) -> Option<&OrderRecord> {
        all_in_region(&self.orders, region).find(|record| {
            record.order.outpost_id.as_deref() == Some(outpost_id) && record.is_outstanding()
        })
    }

    /// The order of `region` still OUTSTANDING at `site_id`, if this store holds
    /// one.
    ///
    /// An order is placed at a site through an Outpost of that site
    /// ([`OrderRecord::site_id`]) and is outstanding until it is finished
    /// ([`OrderRecord::is_outstanding`]), so this is what the service still owes the
    /// SITE: the fact a change to that site is held for (see
    /// [`Self::site_order_hold`]). Stated beside
    /// [`Self::outstanding_order_on_outpost`], which is the same question asked of
    /// the other resource an order belongs to. Scoped to `region` for the reason
    /// [`RegionalRecord`] gives; callers settle first (see [`Self::settle_orders`]),
    /// so the statuses read here are the ones the orders hold at that instant.
    pub fn outstanding_order_at_site(&self, region: &str, site_id: &str) -> Option<&OrderRecord> {
        all_in_region(&self.orders, region)
            .find(|record| record.site_id.as_deref() == Some(site_id) && record.is_outstanding())
    }

    /// The order of `region` that has been PLACED at `site_id`, whatever status it
    /// now holds, if this store holds one.
    ///
    /// An order is placed at a site through an Outpost of that site
    /// ([`OrderRecord::site_id`]), and placing it is what happened whether it is
    /// still outstanding or has since completed or been cancelled — so unlike
    /// [`Self::outstanding_order_at_site`], which asks what the service still OWES
    /// the site, this asks whether an order was ever placed there at all: the fact
    /// that closes the "before you place an order at the site" window
    /// ([`Self::site_address_block`]). Scoped to `region` for the reason
    /// [`RegionalRecord`] gives.
    pub fn order_placed_at_site(&self, region: &str, site_id: &str) -> Option<&OrderRecord> {
        all_in_region(&self.orders, region)
            .find(|record| record.site_id.as_deref() == Some(site_id))
    }

    /// The order placed at `site_id` in `region` that a change to that site has to
    /// wait for, or `None` when no order there is still outstanding.
    ///
    /// Several of this service's operations document that a change to a site waits
    /// for an order in progress at it — a change to either of the site's addresses
    /// (`UpdateSiteAddress`) and a change to the rack properties of the site
    /// (`UpdateSiteRackPhysicalProperties`) — so what holds such a change up is
    /// stated ONCE here: the order still outstanding at the site
    /// ([`Self::outstanding_order_at_site`]), together with the resource the refusal
    /// names it by. Each operation then words its own message around
    /// [`SiteOrderHold::order`], and none of them can disagree about whether an order
    /// at the site holds a change up.
    ///
    /// Callers settle first (see [`Self::settle_orders`]), so the order statuses read
    /// here are the ones the orders hold at that instant.
    pub fn site_order_hold(&self, region: &str, site_id: &str) -> Option<SiteOrderHold> {
        let order = self.outstanding_order_at_site(region, site_id)?;
        // The order itself is a resource the modelled `ResourceType` names, so the
        // conflict an order causes names the order. An order record the store holds
        // is keyed by the id it reports, so the fallback is for a record that names
        // no order at all: the refusal still states the limit, naming no resource
        // rather than one it cannot identify.
        Some(match order.order.order_id.as_deref() {
            Some(order_id) => SiteOrderHold {
                resource: Some((RESOURCE_TYPE_ORDER, order_id.to_string())),
                order: format!("order {order_id}"),
            },
            None => SiteOrderHold {
                resource: None,
                order: "an order placed at it".to_string(),
            },
        })
    }

    /// The quote of `outpost_id` an order WAS submitted from, if this region's
    /// inventory holds one: its SECOND record of an order outstanding on that
    /// Outpost.
    ///
    /// A quote reaches [`QUOTE_STATUS_ORDER_SUBMITTED`] by an order being submitted
    /// from it, and its `SubmittedOrderId` names that order. See
    /// [`Self::outstanding_order_on_outpost`], which this stands beside; callers
    /// settle first (see [`Self::settle_quotes`]), so the quote statuses read here
    /// are the ones the quotes hold at that instant.
    ///
    /// A quote is only the SECOND record of the order: where this store holds the
    /// order the quote names, that order's own status is what says whether anything is
    /// still outstanding — an order that has finished
    /// ([`OrderRecord::is_outstanding`]) is finished on both records, and the quote it
    /// was submitted from stays `ORDER_SUBMITTED` because an order WAS submitted for
    /// it. Only a submitted order this store has no record of is answered for by the
    /// quote alone.
    pub fn quote_ordered_from_on_outpost(
        &self,
        region: &str,
        outpost_id: &str,
    ) -> Option<&QuoteRecord> {
        all_in_region(&self.quotes, region).find(|record| {
            if record.associated_outpost_id() != Some(outpost_id)
                || !record.records_submitted_order()
            {
                return false;
            }
            record
                .quote
                .submitted_order_id
                .as_deref()
                .and_then(|order_id| self.orders.get(order_id))
                .is_none_or(OrderRecord::is_outstanding)
        })
    }

    /// The unfinished work this store still holds on `outpost_id`, or `None` when
    /// the Outpost has none and can be removed.
    ///
    /// Work is unfinished while the service is still to carry it out: a capacity
    /// task of the Outpost in a non-terminal status
    /// ([`CapacityTaskRecord::is_active`]), and an order of the Outpost that is
    /// still outstanding — either the order record itself
    /// ([`Self::outstanding_order_on_outpost`]) or a quote an order was submitted
    /// from ([`Self::quote_ordered_from_on_outpost`]), the two records this store
    /// keeps of one. Removing the Outpost would destroy the first and orphan the
    /// second, neither of which anything cancelled or completed — the inconsistent
    /// state the modelled `ConflictException` exists for, so a delete answers it
    /// rather than reporting a success that lost the work.
    ///
    /// What is NOT unfinished work, and so holds nothing up:
    ///
    /// * work in a TERMINAL status — a completed, failed or cancelled capacity task
    ///   ([`CAPACITY_TASK_TERMINAL`]), an order that has finished
    ///   ([`ORDER_STATUS_SETTLED`]), a quote no order was submitted from or one
    ///   that has expired: there is nothing left for the service to carry out.
    /// * the members of the Outpost's OWN record, which are attributes of the
    ///   Outpost rather than work on it and go away with it: its private
    ///   connectivity configuration, its renewal pricing, its installed assets and
    ///   the instances recorded as running on it. None has an identity outside the
    ///   Outpost, so removing the Outpost leaves nothing of them behind to dangle.
    /// * an installation connection started for one of those assets, which is a
    ///   tunnel to hardware of this Outpost rather than work the service owes on it,
    ///   and which the removal takes with the asset it installs
    ///   ([`Self::remove_outpost`]).
    ///
    /// ONE piece of work, since the modelled `ConflictException` names one
    /// resource: an Outpost that holds both a running task and an outstanding order
    /// reports the task, so a caller that lets the task finish and retries then
    /// meets the order. Which is reported first is this emulator's order of
    /// looking, not a statement about the work.
    pub fn unfinished_work_on_outpost(
        &self,
        region: &str,
        outpost_id: &str,
    ) -> Option<UnfinishedWork> {
        if let Some(active) = self.active_capacity_task_on_outpost(outpost_id) {
            // The modelled `ResourceType` names only `OUTPOST` and `ORDER`, so the
            // resource a caller is told the conflict is about is the Outpost the
            // task is running on; the message names the task itself, which is what
            // the caller polls.
            return Some(UnfinishedWork {
                resource_type: RESOURCE_TYPE_OUTPOST,
                resource_id: outpost_id.to_string(),
                work: format!("capacity task {}", active.capacity_task_id),
            });
        }
        if let Some(outstanding) = self.outstanding_order_on_outpost(region, outpost_id) {
            // The order itself is a resource the modelled `ResourceType` names, so
            // a conflict an order causes names the order.
            return Some(match outstanding.order.order_id.as_deref() {
                Some(order_id) => UnfinishedWork {
                    resource_type: RESOURCE_TYPE_ORDER,
                    resource_id: order_id.to_string(),
                    work: format!("order {order_id}"),
                },
                None => UnfinishedWork {
                    resource_type: RESOURCE_TYPE_OUTPOST,
                    resource_id: outpost_id.to_string(),
                    work: "an outstanding order".to_string(),
                },
            });
        }
        let order = self.quote_ordered_from_on_outpost(region, outpost_id)?;
        Some(match order.quote.submitted_order_id.as_deref() {
            // As above: the order the quote was ordered from is what the conflict is
            // about, and the quote reports its id.
            Some(order_id) => UnfinishedWork {
                resource_type: RESOURCE_TYPE_ORDER,
                resource_id: order_id.to_string(),
                work: format!("order {order_id}"),
            },
            // A submitted order with no recorded id leaves the Outpost as the one
            // resource the caller can be given, and the quote as the only name the
            // outstanding order has.
            None => UnfinishedWork {
                resource_type: RESOURCE_TYPE_OUTPOST,
                resource_id: outpost_id.to_string(),
                work: match order.quote.quote_id.as_deref() {
                    Some(quote_id) => format!("order submitted from quote {quote_id}"),
                    None => "a submitted order".to_string(),
                },
            },
        })
    }

    /// The Outpost that still belongs to `site_id` in `region`, if the store holds
    /// one.
    ///
    /// An Outpost names the site it lives at, so what this answers is whether that
    /// site still has hardware at it: the fact `DeleteSite` refuses a delete on, and
    /// the one a change to the site's OPERATING address waits on once an order has
    /// been placed at the site ([`Self::site_address_block`]). Stated once here so
    /// the two cannot disagree about what "the site still has an Outpost" means.
    pub fn outpost_at_site(&self, region: &str, site_id: &str) -> Option<&OutpostRecord> {
        self.outposts_in(region)
            .find(|record| record.outpost.site_id.as_deref() == Some(site_id))
    }

    /// What holds a change to the `kind` address of `site_id` in `region` up, if
    /// anything — the fact `UpdateSiteAddress` answers its modelled
    /// `ConflictException` on.
    ///
    /// AWS documents TWO limits on when a site address may change, and this states
    /// both, in the order a caller meets them:
    ///
    /// * "You can't update a site address if there is an order in progress. You must
    ///   wait for the order to complete or cancel the order." — on an address of
    ///   EITHER kind, while an order placed at the site is still outstanding
    ///   ([`Self::outstanding_order_at_site`]). It is the narrower limit of the two
    ///   and the one with something for the caller to do, so it is reported first
    ///   where both hold.
    /// * "You can update the operating address before you place an order at the
    ///   site, or after all Outposts that belong to the site have been deactivated."
    ///   — on the OPERATING address only, and only OUTSIDE both windows that
    ///   sentence permits: an order has been placed at the site
    ///   ([`Self::order_placed_at_site`]), so the first window has closed, AND an
    ///   Outpost still belongs to the site ([`Self::outpost_at_site`]), so the
    ///   second has not opened. The operating address is where the site itself is,
    ///   so hardware ordered for it depends on it; a site no order was ever placed
    ///   at is inside the first window however many Outposts belong to it, and the
    ///   shipping address is where that hardware ships to and no Outpost at the site
    ///   holds it up.
    ///
    /// Callers settle first (see [`Self::settle_orders`]), so the order statuses read
    /// here are the ones the orders hold at that instant.
    pub fn site_address_block(
        &self,
        region: &str,
        site_id: &str,
        kind: SiteAddressKind,
    ) -> Option<SiteAddressBlock> {
        if let Some(hold) = self.site_order_hold(region, site_id) {
            return Some(SiteAddressBlock {
                resource: hold.resource,
                message: site_address_order_conflict(site_id, &hold.order),
            });
        }
        if kind != SiteAddressKind::Operating {
            return None;
        }
        // The first window is open until an order is placed AT THE SITE, so an
        // Outpost at a site nobody has ordered hardware for holds nothing up.
        self.order_placed_at_site(region, site_id)?;
        let outpost_id = self
            .outpost_at_site(region, site_id)
            .and_then(|record| record.outpost.outpost_id.clone())?;
        Some(SiteAddressBlock {
            message: site_address_conflict(site_id, &outpost_id),
            resource: Some((RESOURCE_TYPE_OUTPOST, outpost_id)),
        })
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
    ///   names it. An order is outstanding until it is finished, so an order the store
    ///   still holds outstanding is one that can still be cancelled, and a
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
        if self.outstanding_order_on_outpost(region, outpost_id).is_some()
            || self
                .quote_ordered_from_on_outpost(region, outpost_id)
                .is_some()
        {
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

    /// The hardware asset `asset_id` an Outpost of `region` holds, if this store
    /// holds one.
    ///
    /// An `AssetId` names one physical unit of hardware, installed in one rack of
    /// one Outpost, so at most one Outpost holds it: an operation that acts on an
    /// asset of this REGION's inventory, rather than of one named Outpost
    /// ([`Self::asset_of_outpost`]), resolves it here rather than walking the store
    /// itself. The hardware of another region's Outpost is not this region's to act
    /// on (see [`RegionalRecord`]).
    ///
    /// It answers the asset ITSELF and not the Outpost around it, because what an
    /// operation decides about a unit of hardware is decided by what that unit is
    /// ([`is_outpost_server`]) and the record is where that is held.
    pub fn asset_in_region(&self, region: &str, asset_id: &str) -> Option<&AssetInfo> {
        self.outposts_in(region).find_map(|record| {
            record
                .assets
                .iter()
                .find(|asset| asset.asset_id.as_deref() == Some(asset_id))
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

    /// Take the next `count` creation sequences of this store, answering the FIRST of
    /// them.
    ///
    /// [`Self::created`] is this store's one creation order, so the hardware this
    /// service installs takes its sequences from it exactly as a created resource
    /// does ([`Self::mint_digest`]): a block of `count` consecutive sequences this
    /// store has never handed out and will never hand out again. That is what makes a
    /// unit's sequence identify that unit — see
    /// [`OutpostRecord::asset_sequence_base`].
    fn reserve_sequences(&mut self, count: usize) -> u64 {
        let count = u64::try_from(count).unwrap_or_default();
        let first = self.created.wrapping_add(1);
        self.created = self.created.wrapping_add(count);
        first
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

/// The ARN partition a region belongs to, as the modelled ARN patterns'
/// `arn:aws([a-z-]+)?:` partition segment allows.
///
/// AWS's published region-to-partition prefixes: `cn-*` is `aws-cn`, `us-gov-*` is
/// `aws-us-gov`, `eusc-de-*` is `aws-eusc`, the ISO regions carry their own
/// partitions (`us-iso-*` -> `aws-iso`, `us-isob-*` -> `aws-iso-b`, `eu-isoe-*` ->
/// `aws-iso-e`, `us-isof-*` -> `aws-iso-f`), and every other (commercial) region is
/// `aws`. Matched longest prefix first, so `us-isob-east-1` is not read as a
/// `us-iso-*` region.
pub fn partition_for(region: &str) -> &'static str {
    const PARTITIONS: &[(&str, &str)] = &[
        ("cn-", "aws-cn"),
        ("us-gov-", "aws-us-gov"),
        ("eusc-de-", "aws-eusc"),
        ("us-isob-", "aws-iso-b"),
        ("us-isof-", "aws-iso-f"),
        ("eu-isoe-", "aws-iso-e"),
        ("us-iso-", "aws-iso"),
    ];
    PARTITIONS
        .iter()
        .find(|(prefix, _)| region.starts_with(prefix))
        .map_or("aws", |(_, partition)| *partition)
}

/// One Outposts resource ARN:
/// `arn:<partition>:outposts:<region>:<account>:<type>/<id>`.
///
/// Built here rather than with `engine::emulator_arn`, whose only format fixes the
/// partition at the literal `aws`: the partition of a resource is a function of its
/// region ([`partition_for`]), which is why every ARN pattern in this model spells
/// that segment `aws([a-z-]+)?` rather than `aws`. The account segment is still the
/// one emulator account (`engine::emulator_account_id`).
fn resource_arn(region: &str, resource_type: &str, resource_id: &str) -> String {
    format!(
        "arn:{}:{ARN_NAMESPACE}:{region}:{}:{resource_type}/{resource_id}",
        partition_for(region),
        ::engine::emulator_account_id()
    )
}

/// A `SiteArn`: `arn:<partition>:outposts:<region>:<account>:site/os-…`.
pub fn site_arn(region: &str, site_id: &str) -> String {
    resource_arn(region, SITE_RESOURCE_TYPE, site_id)
}

/// An `OutpostArn`: `arn:<partition>:outposts:<region>:<account>:outpost/op-…`.
pub fn outpost_arn(region: &str, outpost_id: &str) -> String {
    resource_arn(region, OUTPOST_RESOURCE_TYPE, outpost_id)
}

/// One IAM role ARN: `arn:<partition>:iam::<account>:role/<name>`.
///
/// IAM is a GLOBAL service, so the region segment of its ARNs is empty — which is
/// exactly what the modelled `RoleArn` pattern
/// (`^arn:aws([a-z-]+)?:iam::\d{12}:role\/[a-zA-Z_0-9+=,.@\-_\/]+$`) spells. The
/// partition is still the one the region the role's resource lives in belongs to
/// ([`partition_for`]), and the account the one emulator account, as in
/// [`resource_arn`].
fn iam_role_arn(region: &str, role_name: &str) -> String {
    format!(
        "arn:{}:iam::{}:role/{role_name}",
        partition_for(region),
        ::engine::emulator_account_id()
    )
}

/// The `RoleArn` of an Outpost's private connectivity configuration: the
/// service-linked role AWS Outposts creates to provision and attach the private
/// connectivity network interfaces in the caller's VPC.
///
/// A service-linked role lives under the reserved `aws-service-role/<service
/// principal>/` IAM path and is named for the service that owns it. This one is
/// derived PER OUTPOST because the member's own documentation says its permissions
/// are scoped to the specific Outpost and VPC, so the configuration of one Outpost
/// cannot report the role of another; what the model requires of the value is the
/// `RoleArn` form and that stability, not a particular name.
pub fn private_connectivity_role_arn(region: &str, outpost_id: &str) -> String {
    iam_role_arn(
        region,
        &format!("aws-service-role/outposts.amazonaws.com/AWSServiceRoleForOutposts_{outpost_id}"),
    )
}

/// The `ProvisioningRoleArn` of an Outpost's private connectivity configuration:
/// the role in the caller's own account that AWS Outposts uses to establish the
/// service link connection while the Outpost is installed.
///
/// This one is NOT a service-linked role — the member documents it as a role in
/// the caller's account — so it carries no reserved path. AWS publishes no name for
/// it, so the name is this emulator's own: derived from the Outpost the role
/// provisions, so it is stable across reads and distinct per Outpost, in the
/// modelled `RoleArn` form for the request's partition and the emulator account.
/// What is required of the value is that form and that per-Outpost stability; the
/// literal name follows no published naming, so a reader should not take it for
/// one.
pub fn private_connectivity_provisioning_role_arn(region: &str, outpost_id: &str) -> String {
    iam_role_arn(region, &format!("AWSOutpostsProvisioning_{outpost_id}"))
}

/// The `PrivateConnectivityConfig` an Outpost configured with `vpc_information_list`
/// holds.
///
/// One builder, so the configuration a create reports and the one stored for every
/// later read to answer cannot disagree:
///
/// * the VPC, its subnets and the VPC endpoint are the caller's, kept as sent;
/// * the status is the terminal [`PRIVATE_CONNECTIVITY_ENABLED`];
/// * `RoleArn` is the service-linked role of this Outpost's private connectivity
///   ([`private_connectivity_role_arn`]) — it exists for any configuration, since
///   the network interfaces it provisions are what private connectivity IS;
/// * `ProvisioningRoleArn` is reported only for a configuration that is VPC
///   ENDPOINT-based, which its own documentation makes the single condition on that
///   member, and a configuration is endpoint-based exactly when its VPC information
///   names an interface VPC endpoint.
pub fn private_connectivity_config(
    region: &str,
    outpost_id: &str,
    vpc_information_list: Vec<VpcInformation>,
) -> PrivateConnectivityConfig {
    let endpoint_based = vpc_information_list
        .iter()
        .any(|vpc| vpc.vpc_endpoint_id.is_some());
    PrivateConnectivityConfig {
        role_arn: Some(private_connectivity_role_arn(region, outpost_id)),
        private_connectivity_status: Some(PRIVATE_CONNECTIVITY_ENABLED.to_string()),
        vpc_information_list: Some(vpc_information_list),
        provisioning_role_arn: endpoint_based
            .then(|| private_connectivity_provisioning_role_arn(region, outpost_id)),
    }
}

/// The fields one AWS resource ARN carries:
/// `arn:<partition>:<service>:<region>:<account>:<resource type>/<resource id>`.
///
/// Both ARN shapes this API models are written out to that last field — the `Arn` the
/// tagging operations bind, and the ARN form every identifier member accepts — so one
/// reader serves them both. The fields borrow the value they were read out of, so
/// answering a question about an ARN copies nothing.
struct ResourceArn<'a> {
    partition: &'a str,
    service: &'a str,
    region: &'a str,
    account: &'a str,
    resource_type: &'a str,
    resource_id: &'a str,
}

/// The ARN `value` is, or `None` when it is not an ARN naming a typed resource at
/// all.
fn parse_resource_arn(value: &str) -> Option<ResourceArn<'_>> {
    let mut fields = value.strip_prefix("arn:")?.splitn(5, ':');
    let partition = fields.next()?;
    let service = fields.next()?;
    let region = fields.next()?;
    let account = fields.next()?;
    let (resource_type, resource_id) = fields.next()?.split_once('/')?;
    Some(ResourceArn {
        partition,
        service,
        region,
        account,
        resource_type,
        resource_id,
    })
}

impl ResourceArn<'_> {
    /// Whether this ARN names a resource of the endpoint a request addressed to
    /// `region` reached.
    ///
    /// An ARN is an ABSOLUTE identity: the partition, the Region and the account id
    /// are part of the name, so the resource it names belongs to the endpoint being
    /// called only when all three are that endpoint's own — the Region the request is
    /// addressed to, the partition that Region belongs to ([`partition_for`]) and the
    /// one account this emulator presents, which are exactly the segments
    /// [`resource_arn`] mints the ARN of a local resource from. An ARN differing in
    /// any of them names a resource of some other endpoint, which this one holds no
    /// record of however familiar the bare resource id inside it looks.
    fn names_endpoint(&self, region: &str) -> bool {
        self.partition == partition_for(region)
            && self.region == region
            && self.account == ::engine::emulator_account_id()
    }
}

/// What an identifier member of this API names at the endpoint the request was
/// addressed to.
///
/// Every identifier member is modelled `^(arn:…:<kind>/)?(<prefix>-[a-f0-9]{17})$`,
/// and the two branches do not say the same thing: the bare id names a resource of
/// the endpoint being called, while an ARN states an identity of its own
/// ([`ResourceArn::names_endpoint`]) that may belong to another endpoint entirely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedIdentifier {
    /// A resource of THIS endpoint's inventory, named by the bare id to look it up
    /// by.
    Here(String),
    /// A resource of ANOTHER endpoint: an ARN whose partition, Region or account id
    /// is not this endpoint's own. This endpoint holds no record of it, so a call
    /// naming it is answered exactly as a call naming a resource that never existed.
    Elsewhere,
}

impl ResolvedIdentifier {
    /// The bare id to look this endpoint's inventory up by, or `None` when the value
    /// named another endpoint's resource — which is nothing this endpoint can report
    /// on, change or delete.
    pub fn here(&self) -> Option<&str> {
        match self {
            Self::Here(resource_id) => Some(resource_id),
            Self::Elsewhere => None,
        }
    }
}

/// The resource an Outposts identifier names at the endpoint a request addressed to
/// `region` reached, whether the identifier arrived as the bare id or as the full
/// ARN.
///
/// One resolver serves `SiteId`, `OutpostId` / `OutpostIdentifier` and
/// `QuoteIdentifier` alike, because every one of them is modelled
/// `^(arn:…:<resource_type>/)?(<prefix>-[a-f0-9]{17})$`: read the bare id out of the
/// value, and — for the ARN form — judge the identity that ARN states against this
/// endpoint's own. `None` means the value is not an identifier of this kind at all:
/// not an ARN of this service's namespace, not this kind of resource, or not the
/// `<prefix>-` plus exactly 17 lowercase hex characters the bare-id branch admits.
pub fn resolve_id(
    identifier: &str,
    resource_type: &str,
    prefix: &str,
    region: &str,
) -> Option<ResolvedIdentifier> {
    if !identifier.starts_with(ARN_SCHEME) {
        let resource_id = bare_id(identifier, prefix)?;
        return Some(ResolvedIdentifier::Here(resource_id.to_string()));
    }
    let arn = parse_resource_arn(identifier)?;
    if arn.service != ARN_NAMESPACE || arn.resource_type != resource_type {
        return None;
    }
    let resource_id = bare_id(arn.resource_id, prefix)?;
    Some(match arn.names_endpoint(region) {
        true => ResolvedIdentifier::Here(resource_id.to_string()),
        false => ResolvedIdentifier::Elsewhere,
    })
}

/// The bare `<prefix>-…` resource id `value` is, or `None` when it is not one: the
/// id form every identifier pattern of this API spells `<prefix>-[a-f0-9]{17}`.
fn bare_id<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let suffix = value.strip_prefix(prefix)?.strip_prefix('-')?;
    let well_formed = suffix.len() == 17
        && suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    well_formed.then_some(value)
}

/// The site a `SiteId` names here. See [`resolve_id`].
pub fn resolve_site_id(identifier: &str, region: &str) -> Option<ResolvedIdentifier> {
    resolve_id(identifier, SITE_RESOURCE_TYPE, SITE_ID_PREFIX, region)
}

/// The Outpost an `OutpostId` / `OutpostIdentifier` names here. See [`resolve_id`].
pub fn resolve_outpost_id(identifier: &str, region: &str) -> Option<ResolvedIdentifier> {
    resolve_id(identifier, OUTPOST_RESOURCE_TYPE, OUTPOST_ID_PREFIX, region)
}

/// The quote a `QuoteIdentifier` names here. See [`resolve_id`].
pub fn resolve_quote_id(identifier: &str, region: &str) -> Option<ResolvedIdentifier> {
    resolve_id(identifier, QUOTE_RESOURCE_TYPE, QUOTE_ID_PREFIX, region)
}

/// The `op-…` id an `OutpostArn` this service MINTED carries.
///
/// For an ARN the service stored itself — the `OutpostArn` a quote copies off the
/// Outpost it is associated with — which already names a resource of that resource's
/// own region. An identifier a REQUEST supplied is resolved against the endpoint it
/// was sent to instead ([`resolve_outpost_id`]).
pub fn outpost_id_in_arn(arn: &str) -> Option<&str> {
    let arn = parse_resource_arn(arn)?;
    if arn.service != ARN_NAMESPACE || arn.resource_type != OUTPOST_RESOURCE_TYPE {
        return None;
    }
    bare_id(arn.resource_id, OUTPOST_ID_PREFIX)
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

/// The resource kind an Outposts resource ARN names, and what that ARN resolves to
/// at the endpoint a request addressed to `region` reached — e.g. `("site",
/// Here("os-…"))` for `arn:<this partition>:outposts:<region>:<this account>:site/os-…`.
///
/// Every `Arn` member in this API is modelled
/// `^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:([a-z\d-]+)/)[a-z]{2,8}-[a-f0-9]{17}$`,
/// so the resource field is always `<type>/<id>` and the service field is always
/// this service's own ARN namespace. `None` means the value is not an ARN of this
/// service at all.
///
/// The partition, the Region and the account id are part of the name, exactly as
/// they are in the ARN form of an identifier member ([`resolve_id`]), so an ARN is
/// resolved HERE only when all three are the endpoint's own
/// ([`ResourceArn::names_endpoint`]); one differing in any of them names a resource
/// of some other endpoint ([`ResolvedIdentifier::Elsewhere`]), which this endpoint
/// holds no record of however familiar the bare resource id inside it looks.
pub fn resolve_resource_arn(arn: &str, region: &str) -> Option<(String, ResolvedIdentifier)> {
    let arn = parse_resource_arn(arn)?;
    if arn.service != ARN_NAMESPACE {
        return None;
    }
    let resolved = match arn.names_endpoint(region) {
        true => ResolvedIdentifier::Here(arn.resource_id.to_string()),
        false => ResolvedIdentifier::Elsewhere,
    };
    Some((arn.resource_type.to_string(), resolved))
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
/// the value as the request supplied it, and no more — cut to the room that
/// wording leaves it inside [`ERROR_MESSAGE_MAX`], since the modelled
/// `AvailabilityZone` reaches that bound on its own.
fn wrong_region(value: &str) -> String {
    const WORDING: &str = " does not match your region.";
    let [value] = echoes(WORDING.len(), [value]);
    format!("{value}{WORDING}")
}

/// How this service words a value that names the request's own region but no zone
/// within it — refused for the same reason (it is no zone of this region) but
/// stated separately, because calling it a region mismatch would be wrong.
///
/// Cut by the same rule as [`wrong_region`]: the region is the one this endpoint
/// serves, not something the caller supplied, so it counts towards the fixed part
/// and the whole remaining room is the supplied value's.
fn names_no_zone(value: &str, region: &str) -> String {
    const WORDING: &str = " is not an Availability Zone of ";
    const STOP: &str = ".";
    let [value] = echoes(WORDING.len() + STOP.len() + region.len(), [value]);
    format!("{value}{WORDING}{region}{STOP}")
}

/// How this service words a pair of identities that name two zones: both values as
/// supplied, since either of them could be the one the caller meant — and so both
/// cut, by the same rule as [`wrong_region`], to their share of the room the
/// wording leaves.
fn two_zones(zone: &str, zone_id: &str) -> String {
    const JOIN: &str = " and ";
    const WORDING: &str = " do not name the same Availability Zone.";
    let [zone, zone_id] = echoes(JOIN.len() + WORDING.len(), [zone, zone_id]);
    format!("{zone}{JOIN}{zone_id}{WORDING}")
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
// * `NotFoundException` — the body is the wording Amazon Web Services Outposts
//   answers with on the path that could not resolve the identifier: ONE fixed
//   text per kind of resource, whose only variable part is the identifier taken
//   from the request. The texts are [`order_not_found`], [`quote_not_found`],
//   [`asset_not_found`], [`outpost_not_found`] and [`outpost_arn_not_found`] for
//   the kinds that are worded as a sentence or by the resolved ARN,
//   [`resource_not_found`] for the tagging paths, whose sentence has no variable
//   part at all, and the identifier ECHO `<member>=<value as supplied>` on the
//   site and Outpost registry paths that answer it ([`identifier_not_found`]).
// * `ValidationException` — the body is the standard AWS validation report, and
//   it never renders the offending value. See [`validation_report`].
//
// Whichever rule words it, the body has to FIT the member it is carried in:
// every error shape in this API declares its message as `ErrorMessage`, so a
// body past [`ERROR_MESSAGE_MAX`] is one the model forbids. Where a rule renders
// a value the caller supplied, that echo is what takes the cut ([`echoes`]), so
// the fixed wording survives whatever arrives.
// ---------------------------------------------------------------------------

/// The longest message the model accepts in a failure of this API: every error
/// shape here carries its message as `ErrorMessage`, `length(min: 1, max: 1000)`.
///
/// Several members a refusal echoes are modelled LONGER than this on their own —
/// `ConnectionId` is `length(1..1024)`, the `Arn` every tagging operation binds
/// `length(0..1011)`, `AvailabilityZone` `length(1..1000)` — so a value the
/// boundary accepts can be longer than the body that reports it. The echo of it is
/// therefore cut to the room the refusal's own wording leaves, which is what keeps
/// every refusal this service words inside the model for any value that reaches a
/// handler.
pub const ERROR_MESSAGE_MAX: usize = 1000;

/// The caller-supplied parts of one refusal, each cut to the room the `fixed`
/// bytes of that refusal's own wording leave inside [`ERROR_MESSAGE_MAX`]
/// ([`clip`]).
///
/// A message holding several echoes SHARES that room equally between them, so one
/// long value cannot crowd another out of the body and each echo is reported as
/// far as the bound allows. `fixed` counts everything the message renders that the
/// caller did not supply: its wording, and the member names and stored values this
/// service puts in it.
fn echoes<const N: usize>(fixed: usize, parts: [&str; N]) -> [String; N] {
    let room = ERROR_MESSAGE_MAX.saturating_sub(fixed) / N;
    parts.map(|part| clip(part, room))
}

/// How the site and Outpost REGISTRY paths of this service state that an
/// identifier names no resource they hold.
///
/// The body is the identifier ECHO: the name of the input member the identifier
/// arrived in, `=`, and the value as the client sent it — so a caller that sent
/// the ARN form reads its ARN back, not the bare id the service resolved it to,
/// and a caller can parse the identifier it sent out of the message. It is not an
/// English sentence, and it never normalises the value.
///
/// This is the wording of ONE group of paths, not of the whole API: the kind of
/// resource that could not be resolved is what fixes a not-found text, and the
/// order, quote, asset and capacity paths word theirs differently
/// ([`order_not_found`], [`quote_not_found`], [`asset_not_found`],
/// [`outpost_not_found`], [`outpost_arn_not_found`], [`resource_not_found`]).
///
/// The one thing it does to the value is FIT it: the echo is cut to the room
/// `<member>=` leaves inside [`ERROR_MESSAGE_MAX`] ([`echoes`]), so an identifier
/// modelled longer than the message that reports it — a `ConnectionId`, a tagging
/// `ResourceArn` — is refused within the model instead of overrunning it.
///
/// `member` is the member's validation-path name (see [`SITE_ID_MEMBER`]);
/// `supplied` is the raw input member, before [`resolve_id`] touches it.
pub fn identifier_not_found(member: &str, supplied: &str) -> String {
    const SEPARATOR: &str = "=";
    let [supplied] = echoes(member.len() + SEPARATOR.len(), [supplied]);
    format!("{member}{SEPARATOR}{supplied}")
}

/// How a refusal states that no ORDER is held under the id a request named:
/// `No such order for order id <orderId>`.
///
/// The fixed text is AWS's own, and the only variable part is the order id the
/// request supplied — a bare `oo-…` id in every operation that takes one, since
/// `OrderId` is modelled `^oo-[a-f0-9]{17}$` with no ARN form. The id is cut to
/// the room that fixed text leaves inside [`ERROR_MESSAGE_MAX`] ([`echoes`]), as
/// every wording of a refusal here is.
pub fn order_not_found(order_id: &str) -> String {
    const FIXED: &str = "No such order for order id ";
    let [order_id] = echoes(FIXED.len(), [order_id]);
    format!("{FIXED}{order_id}")
}

/// How a refusal states that no QUOTE is held under the identifier a request
/// named: `Could not find Quote with identifier <quoteIdentifier>`.
///
/// The variable part is the identifier AS SUPPLIED — `QuoteIdentifier` accepts
/// the bare id or the full quote ARN, and a caller that named the quote by ARN
/// reads that ARN back. See [`order_not_found`] for the bound.
pub fn quote_not_found(supplied: &str) -> String {
    const FIXED: &str = "Could not find Quote with identifier ";
    let [supplied] = echoes(FIXED.len(), [supplied]);
    format!("{FIXED}{supplied}")
}

/// How the TAGGING paths of this service state that the `ResourceArn` a request
/// named resolves to no resource they hold: `Resource was not found`.
///
/// The text is WHOLLY fixed — this is the one not-found wording of this API with no
/// variable part at all, so the ARN the caller supplied is not rendered and the body
/// is the same sentence whichever resource kind, partition, Region or account the
/// value named. Nothing is echoed, so nothing can be cut: the body is inside
/// [`ERROR_MESSAGE_MAX`] for every value that reaches a handler.
///
/// Shared by every operation that binds the tagging `ResourceArn` member
/// ([`RESOURCE_ARN_MEMBER`]), so they cannot drift apart on it.
pub fn resource_not_found() -> String {
    "Resource was not found".to_string()
}

/// How a refusal states that no installed ASSET carries the id a request named:
/// `No such asset id <assetId> found.`
///
/// The trailing period is part of AWS's text. The variable part is the `AssetId`
/// the request supplied, which is a bare hardware id in every operation that
/// takes one. See [`order_not_found`] for the bound.
pub fn asset_not_found(asset_id: &str) -> String {
    const OPENING: &str = "No such asset id ";
    const CLOSING: &str = " found.";
    let [asset_id] = echoes(OPENING.len() + CLOSING.len(), [asset_id]);
    format!("{OPENING}{asset_id}{CLOSING}")
}

/// How a refusal states that the site a request named has no ADDRESS of the type it
/// asked for recorded: `<AddressType> not found for SiteId=<siteId>`.
///
/// The fixed text is AWS's own. The variable parts are the modelled `AddressType`
/// value the request asked for and the bare site id its `SiteId` resolved to — the
/// same value the success path reports in `GetSiteAddress$SiteId`, so the refusal
/// names the site that was read whichever identifier form named it. The refusal
/// tracks what the site HOLDS and not what the request said: an address type the
/// site records reads back, and recording the missing one makes the same call
/// succeed. See [`order_not_found`] for the bound.
pub fn site_address_not_found(address_type: &str, site_id: &str) -> String {
    const SEPARATOR: &str = " not found for SiteId=";
    let [address_type, site_id] = echoes(SEPARATOR.len(), [address_type, site_id]);
    format!("{address_type}{SEPARATOR}{site_id}")
}

/// How this service states that an Outpost's lifecycle status does not allow the
/// decommission a request asked for: `Outpost <outpostId> has invalid
/// lifecycleStatus=<status>`.
///
/// The fixed text is AWS's own; the variable parts are the bare Outpost id the
/// request's identifier resolved to and the `LifeCycleStatus` that Outpost holds now
/// ([`OutpostRecord::life_cycle_status`]). See [`order_not_found`] for the bound.
pub fn outpost_lifecycle_invalid(outpost_id: &str, status: &str) -> String {
    const OPENING: &str = "Outpost ";
    const SEPARATOR: &str = " has invalid lifecycleStatus=";
    let [outpost_id, status] = echoes(OPENING.len() + SEPARATOR.len(), [outpost_id, status]);
    format!("{OPENING}{outpost_id}{SEPARATOR}{status}")
}

/// How this service states that no capacity task can be created for an Outpost that
/// has not reached [`LIFE_CYCLE_STATUS_ACTIVE`]: `Failed to create capacity task for
/// <outpostId>. Outpost should have LifeCycleStatus being ACTIVE, but <outpostId> has
/// LifeCycleStatus <status>`.
///
/// The fixed text is AWS's own, and it names the Outpost TWICE: the bare id the
/// request's identifier resolved to, with the status that Outpost holds now
/// ([`OutpostRecord::life_cycle_status`]) as the only other variable part. See
/// [`order_not_found`] for the bound.
pub fn capacity_task_lifecycle_report(outpost_id: &str, status: &str) -> String {
    const OPENING: &str = "Failed to create capacity task for ";
    const REQUIRED: &str = ". Outpost should have LifeCycleStatus being ";
    const JOIN: &str = ", but ";
    const HELD: &str = " has LifeCycleStatus ";
    let [addressed, named, status] = echoes(
        OPENING.len() + REQUIRED.len() + LIFE_CYCLE_STATUS_ACTIVE.len() + JOIN.len() + HELD.len(),
        [outpost_id, outpost_id, status],
    );
    format!(
        "{OPENING}{addressed}{REQUIRED}{LIFE_CYCLE_STATUS_ACTIVE}{JOIN}{named}{HELD}{status}"
    )
}

/// How this service states that an order cannot be placed at a site whose details are
/// not complete: `The site details are incomplete. Reach out to AWS Support for
/// assistance.`
///
/// The text is AWS's own and WHOLLY fixed — it names neither the site nor which of
/// its details is missing — so nothing in it can be cut and the body is inside
/// [`ERROR_MESSAGE_MAX`] for every request that reaches a handler. What a site's
/// details ARE is [`SiteRecord::details_complete`].
pub fn site_details_incomplete() -> String {
    "The site details are incomplete. Reach out to AWS Support for assistance.".to_string()
}

/// How the CAPACITY paths of this service state that no Outpost is held under
/// the identifier a request named: `No Outpost with outpostId=<outpostId>`.
///
/// AWS names the member `outpostId` here whichever member the identifier arrived
/// in ([`OUTPOST_ID_MEMBER`]), and the value is the bare `op-…` id — read out of
/// the ARN when the request named the Outpost by one, since the sentence reports
/// the Outpost rather than echoing the request. See [`order_not_found`] for the
/// bound.
pub fn outpost_not_found(identifier: &str) -> String {
    const OPENING: &str = "No Outpost with ";
    const SEPARATOR: &str = "=";
    let outpost_id = outpost_id_in_arn(identifier).unwrap_or(identifier);
    let [outpost_id] = echoes(
        OPENING.len() + OUTPOST_ID_MEMBER.len() + SEPARATOR.len(),
        [outpost_id],
    );
    format!("{OPENING}{OUTPOST_ID_MEMBER}{SEPARATOR}{outpost_id}")
}

/// How the order and Outpost MANAGEMENT paths of this service state that no
/// Outpost is held under the identifier a request named: `outpostId=<the Outpost
/// ARN that identifier resolves to>`.
///
/// These paths resolve the identifier to an Outpost ARN before looking anything
/// up, and it is that resolved ARN the refusal names: an identifier that arrived
/// AS an ARN already states the partition, Region and account it names a resource
/// of, while a bare id names one of the endpoint the request reached, so the ARN
/// of a bare id is the one [`outpost_arn`] mints for `region` — the same builder
/// the success path reports an Outpost's `OutpostArn` from.
pub fn outpost_arn_not_found(identifier: &str, region: &str) -> String {
    let arn = match identifier.starts_with(ARN_SCHEME) {
        true => identifier.to_string(),
        false => outpost_arn(region, identifier),
    };
    identifier_not_found(OUTPOST_ID_MEMBER, &arn)
}

/// What every ARN starts with, and so what tells an identifier supplied in the
/// ARN form from one supplied as the bare resource id.
const ARN_SCHEME: &str = "arn:";

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

/// The validation-path name of the `QuoteOptionIdentifier` input member `CreateOrder`
/// names the option of a quote an order is submitted against by. See
/// [`SITE_ID_MEMBER`].
pub const QUOTE_OPTION_IDENTIFIER_MEMBER: &str = "quoteOptionIdentifier";

/// The validation-path name of the `OutpostIdentifier` input member the quote and
/// capacity-task operations take: a DIFFERENT member from the `OutpostId` the
/// Outpost operations bind, so a refusal about it names it as the request spelled
/// it. See [`SITE_ID_MEMBER`].
pub const OUTPOST_IDENTIFIER_MEMBER: &str = "outpostIdentifier";

/// The validation-path name of the `OutpostIdentifierFilter` input member a list
/// narrows its results to one Outpost by: a DIFFERENT member from the
/// `OutpostIdentifier` an operation addressed to one Outpost binds, so a refusal
/// about it names it as the request spelled it. See [`SITE_ID_MEMBER`].
pub const OUTPOST_IDENTIFIER_FILTER_MEMBER: &str = "outpostIdentifierFilter";

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

/// The validation-path name of the `ClientToken` input member an operation takes
/// an idempotency token in. See [`SITE_ID_MEMBER`].
pub const CLIENT_TOKEN_MEMBER: &str = "clientToken";

/// The validation-path name of the `CatalogItemId` input member an operation names
/// one published hardware configuration by. See [`SITE_ID_MEMBER`].
pub const CATALOG_ITEM_ID_MEMBER: &str = "catalogItemId";

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
        sensitive: false,
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

/// How this service states that the hardware a capacity configuration was asked of
/// does not run one of the instance types it names.
///
/// One of the two reasons a [`CapacityVerdict`] can fail. AWS publishes no wording
/// for a capacity failure, so the sentence is the emulator's own; what a caller
/// branches on is the modelled `CapacityTaskFailure$Type`
/// ([`CAPACITY_TASK_FAILURE_UNSUPPORTED_CONFIGURATION`]), while the reason names the
/// instance type the configuration cannot be held in, so the caller knows which pool
/// to change.
///
/// The instance type is a value the REQUEST carried, and the reason is a value this
/// service reports, so the echo of it is cut to the room the fixed wording leaves
/// inside [`CAPACITY_TASK_STATUS_REASON_MAX`] ([`clip`]): the reason fits the
/// modelled member whatever arrives, for any value and without a case that cannot be
/// answered.
pub fn instance_type_not_run(instance_type: &str) -> String {
    const WORDING: &str = "the hardware of this capacity task does not run ";
    let room = CAPACITY_TASK_STATUS_REASON_MAX.saturating_sub(WORDING.len());
    format!("{WORDING}{}", clip(instance_type, room))
}

/// `value` cut to at most `max` bytes, on a character boundary.
///
/// Built up character by character rather than sliced, so a value of any length and
/// any encoding is answered rather than being a case this service cannot word.
fn clip(value: &str, max: usize) -> String {
    let mut clipped = String::new();
    for character in value.chars() {
        if clipped.len() + character.len_utf8() > max {
            break;
        }
        clipped.push(character);
    }
    clipped
}

/// How this service states that a capacity configuration asks for more instance
/// capacity than is available to it, both figures in vCPUs.
///
/// The other reason a [`CapacityVerdict`] can fail, worded by the same rule
/// [`instance_type_not_run`] is: the reason names what was asked for and what is
/// available, which is what tells a caller how much of its plan has to go.
pub fn capacity_above_available(requested: i64, available: i64) -> String {
    format!(
        "the requested capacity of {requested} vCPUs is above the {available} vCPUs available"
    )
}

/// How this service states that a capacity task the hardware COULD be configured for
/// was failed because the instances holding the capacity it needs were left running,
/// both figures in vCPUs.
///
/// What the modelled `FAIL_TASK` action leaves behind, worded by the same rule
/// [`instance_type_not_run`] is: the caller branches on the modelled
/// [`CAPACITY_TASK_FAILURE_BLOCKING_INSTANCES_NOT_EVACUATED`] type, while the reason
/// names how much capacity is still held by instances that would have had to stop —
/// which is what tells a caller how much of `ListBlockingInstancesForCapacityTask` it
/// has to act on before asking again.
pub fn blocking_instances_not_evacuated(requested: i64, free: i64) -> String {
    format!(
        "the requested capacity of {requested} vCPUs needs {} vCPUs freed by instances left running",
        (requested - free).max(0)
    )
}

/// How this service states that an Outpost cannot be deleted while unfinished work
/// is still outstanding on it.
///
/// Worded by the same rule as [`capacity_task_conflict`]: AWS publishes no sentence
/// for this refusal, so what the caller branches on is the `ConflictException` code
/// (HTTP 409) and the `ResourceId` / `ResourceType` members naming the resource the
/// conflict is about, while the message names the work ([`UnfinishedWork::work`])
/// the caller has to let finish or cancel first.
pub fn unfinished_work_conflict(outpost_id: &str, work: &str) -> String {
    format!("Outpost {outpost_id} cannot be deleted while {work} is still outstanding on it")
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
/// while an Outpost still belongs to that site, an order having been placed at
/// it.
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

/// How this service states that a change to a site is held while an order placed at
/// that site is still in progress: what is being changed, the site, and the order the
/// caller has to let complete or cancel.
///
/// AWS documents the limit on each operation it applies to but publishes no wording
/// for the refusal, so the sentence is the emulator's own, worded by the same rule as
/// [`site_address_conflict`]: what a caller branches on is the `ConflictException`
/// code (HTTP 409) and the `ResourceId` / `ResourceType` members naming the order
/// ([`SiteOrderHold::resource`]). Stated once here so every operation that waits on an
/// order at the site says the same thing about it.
pub fn site_order_conflict(changed: &str, site_id: &str, order: &str) -> String {
    format!("the {changed} of site {site_id} cannot be updated while {order} is in progress")
}

/// How this service states that a site's address — of EITHER type — cannot be
/// changed while an order placed at that site is still in progress.
///
/// AWS documents the rule — "You can't update a site address if there is an order
/// in progress. You must wait for the order to complete or cancel the order."
/// (`UpdateSiteAddress`) — and the sentence is worded by [`site_order_conflict`].
pub fn site_address_order_conflict(site_id: &str, order: &str) -> String {
    site_order_conflict("address", site_id, order)
}

/// How this service states that the rack properties of a site cannot be changed
/// while an order placed at that site is still in progress.
///
/// AWS documents the rule — "To update a rack at a site with an order of IN_PROGRESS,
/// you must wait for the order to complete or cancel the order."
/// (`UpdateSiteRackPhysicalProperties`) — and the sentence is worded by
/// [`site_order_conflict`], the same one [`site_address_order_conflict`] uses, because
/// it is the same limit on another change to the same site.
pub fn site_rack_properties_order_conflict(site_id: &str, order: &str) -> String {
    site_order_conflict("rack physical properties", site_id, order)
}

/// How this service states that a site cannot be deleted while an Outpost still
/// belongs to it.
///
/// An Outpost names the site it lives at, so deleting that site would leave the
/// Outpost pointing at nothing: the delete is held until no Outpost belongs to the
/// site. Unlike [`site_address_conflict`], AWS does publish the wording for THIS
/// refusal, and this is it: the fixed text `Site ` and ` cannot be deleted because
/// it has one or more outposts.` around the one variable part, the site the request
/// named — the bare id an ARN resolved to. The sentence names no dependent Outpost
/// and no count of them, because what the refusal is about is that the site still
/// has at least one.
pub fn site_has_outposts_report(site_id: &str) -> String {
    format!("Site {site_id} cannot be deleted because it has one or more outposts.")
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

/// The report an `OutpostIdentifierFilter` that is neither an Outpost id nor an
/// Outpost ARN states.
///
/// The boundary refuses such a value first — the member carries the same modelled
/// pattern as every other Outpost identifier — so this is the modelled fallback for
/// a handler that resolves the filter itself. See [`outpost_identifier_pattern_report`].
pub fn outpost_identifier_filter_pattern_report() -> String {
    constraint_report(
        OUTPOST_IDENTIFIER_FILTER_MEMBER,
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

/// The report an order for a published configuration that is no longer offered
/// states.
///
/// A line item orders one of the hardware configurations this service publishes, and
/// the catalog records on each one whether it can still be ordered
/// ([`CatalogItemRecord::is_orderable`]): a configuration published as no longer
/// offered is not one an order can be placed for, so the request is refused with
/// nothing persisted. AWS publishes no wording for the refusal, so the sentence is the
/// emulator's own; what a caller branches on is the modelled `ValidationException`
/// code (HTTP 400). It is worded by the same rule as [`order_contents_report`] — the
/// member and the requirement, never the value, so neither the SKU nor the status it
/// is published in is rendered.
pub fn order_catalog_item_report() -> String {
    validation_report(&[clause(
        LINE_ITEMS_MEMBER,
        "every catalog item ordered must be one the catalog still offers",
    )])
}

/// The report a renewal of an Outpost with nothing under subscription states.
///
/// A renewal renews the subscription an Outpost's orders hold, so an Outpost that
/// subscribes no hardware
/// ([`ComAmazonawsOutpostsOutpostsolafserviceState::subscribed_capacity_vcpus`])
/// has no subscription a contract could renew and no price one could be bought at —
/// which is also what makes the pricing read answer that it could not price the
/// request. `CreateRenewal` has no such outcome to report, so the request is refused
/// as the bad request it is (400 `ValidationException`). AWS publishes no wording for
/// the refusal, so the sentence is the emulator's own, worded by the same rule as
/// every other constraint this service reports — the member and the requirement,
/// never the value.
pub fn renewal_subscription_report() -> String {
    validation_report(&[clause(
        OUTPOST_IDENTIFIER_MEMBER,
        "the Outpost must have a subscription that can be renewed",
    )])
}

/// The report a request that reuses an idempotency token on different terms states.
///
/// A token is what identifies ONE request, so a later request naming a token an
/// Outpost already holds a renewal under is a repeat of that request and is answered
/// that same contract. One that names the token but asks for another payment option
/// or term is not a repeat of it, and the operation models no idempotency-mismatch
/// error, so it is refused as a bad request rather than answered a contract on terms
/// the caller did not ask for. The wording follows [`renewal_subscription_report`],
/// and names neither the token nor the terms.
pub fn renewal_token_report() -> String {
    validation_report(&[clause(
        CLIENT_TOKEN_MEMBER,
        "a client token already used for a renewal of the Outpost must name the same \
         payment option and payment term",
    )])
}

/// The report an installation connection started for hardware that is no Outpost
/// server states.
///
/// `StartConnection` "installs Outpost servers" and its `AssetId` is "the ID of the
/// Outpost server", so a unit of hardware that is not a server
/// ([`is_outpost_server`]) is not hardware this call installs: the request names a
/// resolved asset the operation cannot act on, which is the bad request the modelled
/// `ValidationException` (HTTP 400) states — the asset exists, so it is not the
/// not-found error. AWS publishes no wording for the refusal, so the sentence is the
/// emulator's own, worded by the same rule as every other constraint this service
/// reports — the member and the requirement, never the value.
pub fn connection_asset_report() -> String {
    validation_report(&[clause(
        ASSET_ID_MEMBER,
        "the asset must be an Outpost server",
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

/// How this service states that an order cannot be submitted from a quote whose
/// ordering requirements are not met, naming the checks that are not.
///
/// An `OrderingRequirement` is "a requirement that must be met before an order can be
/// submitted for a quote", so a quote with a check still unmet
/// ([`unmet_requirements`]) cannot take one. AWS publishes no wording for the refusal,
/// so the sentence is the emulator's own, worded by the same rule as
/// [`quote_status_conflict`]: what a caller branches on is the modelled
/// `ConflictException` code (HTTP 409) and the `ResourceId` naming the quote, while
/// the message names the modelled checks that are unmet, so the caller knows what to
/// put right.
pub fn quote_requirements_conflict(quote_id: &str, unmet: &[&str]) -> String {
    format!(
        "an order cannot be submitted from quote {quote_id}, which does not meet {}",
        unmet.join(", ")
    )
}

/// The report an order submitted from a quote associated with ANOTHER Outpost states.
///
/// A quote's Outpost association is what makes an order submitted from it an order OF
/// that Outpost (see [`ORDERING_REQUIREMENT_OUTPOST_ID_MISSING`]), so a request whose
/// `OutpostIdentifier` and whose quote name different Outposts is a request this
/// service cannot place: the two members disagree, which is the bad request the
/// modelled `ValidationException` (HTTP 400) states. AWS publishes no wording for the
/// refusal, so the sentence is the emulator's own, worded by the same rule as every
/// other constraint this service reports — the member and the requirement, never the
/// value.
pub fn quote_outpost_report() -> String {
    validation_report(&[clause(
        QUOTE_IDENTIFIER_MEMBER,
        "the quote must be associated with the Outpost the order is placed on",
    )])
}

/// The report an update that would rewrite what an order was submitted under states.
///
/// A quote an order has already been submitted from
/// ([`QuoteRecord::records_submitted_order`]) is this service's record of that order:
/// the order names the option of that quote it was submitted against
/// ([`QuoteRecord::offers_option`]), and the quote's Outpost association is what made
/// it an order OF an Outpost — the second record this store keeps of an order
/// outstanding on one
/// ([`ComAmazonawsOutpostsOutpostsolafserviceState::quote_ordered_from_on_outpost`]).
/// Re-pricing such a quote, or moving its association, would leave the order naming an
/// option no quote offers and this store's second record of it on an Outpost it was
/// never placed on, so the update is refused with nothing written. The operation models
/// no `ConflictException`, so what the refusal states is the bad request the modelled
/// `ValidationException` (HTTP 400) is for. AWS publishes no wording for it, so the
/// sentence is the emulator's own, worded by the same rule as every other constraint
/// this service reports — the member and the requirement, never the value.
pub fn quote_submitted_order_report() -> String {
    validation_report(&[clause(
        QUOTE_IDENTIFIER_MEMBER,
        "the quote must not have an order submitted from it",
    )])
}

/// The report a `QuoteOptionIdentifier` sent without the quote it is an option of
/// states.
///
/// A quote option has no identity outside the quote it was minted with, so an option
/// named by a request that names no quote names nothing this service could submit an
/// order against. Worded by the same rule as [`quote_outpost_report`].
pub fn quote_option_quote_report() -> String {
    validation_report(&[clause(
        QUOTE_OPTION_IDENTIFIER_MEMBER,
        "the quote the quote option belongs to must be specified",
    )])
}

/// How this service states that an order cannot be cancelled, because it has already
/// finished.
///
/// An order is cancelled while the service still has work to carry out on it; once it
/// is in one of the statuses in which it is FINISHED ([`ORDER_STATUS_SETTLED`]) there
/// is nothing left to cancel, and reporting a success would say the call changed
/// something it did not. AWS publishes no wording for the refusal, so the sentence is
/// the emulator's own, worded by the same rule as [`quote_status_conflict`]: what a
/// caller branches on is the modelled `ConflictException` code (HTTP 409) and the
/// `ResourceId` / `ResourceType` naming the order, while the message names the status
/// the order is in, so the caller can tell an order already cancelled from one that
/// ran to its end.
pub fn order_status_conflict(order_id: &str, status: &str) -> String {
    format!("order {order_id} cannot be cancelled, which is {status}")
}

/// How this service states that a capacity task cannot be cancelled, because of the
/// status it is in.
///
/// A task is cancelled while Amazon Web Services Outposts has not begun carrying it
/// out and has not finished with it ([`CapacityTaskRecord::is_cancellable`]); in any
/// other status there is nothing a cancellation would change, and reporting a success
/// would say the call changed something it did not. AWS publishes no wording for the
/// refusal, so the sentence is the emulator's own, worded by the same rule as
/// [`order_status_conflict`]: what a caller branches on is the modelled
/// `ConflictException` code (HTTP 409) and the `ResourceId` / `ResourceType` naming
/// the resource the task belongs to, while the message names the task and the status
/// it is in, so the caller can tell a running task from one already finished.
pub fn capacity_task_status_conflict(capacity_task_id: &str, status: &str) -> String {
    format!("capacity task {capacity_task_id} cannot be cancelled, which is {status}")
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

/// The stamp a `ListQuotes` page token carries. See [`OUTPOST_PAGE`].
pub const QUOTE_PAGE: &str = "quotes";

/// The stamp a `ListOrders` page token carries.
///
/// What it pages through is every order of one region, narrowed by its own Outpost
/// filter, and an order carries a creation sequence of its own — so the boundary
/// such a token names is a position in the store's creation order. See
/// [`OUTPOST_PAGE`].
pub const ORDER_PAGE: &str = "orders";

/// The stamp a `ListOrderableInstanceTypes` page token carries. See [`OUTPOST_PAGE`].
pub const ORDERABLE_INSTANCE_TYPE_PAGE: &str = "instanceTypes";

/// The stamp a `ListCatalogItems` page token carries.
///
/// What it pages through is the catalog of orderable hardware CONFIGURATIONS, a
/// different list from the instance types those configurations run
/// ([`ORDERABLE_INSTANCE_TYPE_PAGE`]), so a token issued for one of them is not a
/// boundary of the other. See [`OUTPOST_PAGE`].
pub const CATALOG_ITEM_PAGE: &str = "catalogItems";

/// The stamp a `ListCapacityTasks` page token carries.
///
/// What it pages through is every capacity task of one region, narrowed by its own
/// two filters, and a task carries a creation sequence of its own — so the boundary
/// such a token names is a position in the store's creation order. See
/// [`OUTPOST_PAGE`].
pub const CAPACITY_TASK_PAGE: &str = "capacityTasks";

/// The KIND a `ListAssets` page token is stamped with.
///
/// What it pages through is the hardware installed on ONE Outpost
/// ([`OutpostRecord::assets`]), so the list has no existence apart from that
/// Outpost: the stamp such a token carries is this kind together with the Outpost
/// the list was cut from ([`nested_page_kind`]), and the boundary it names is the
/// creation sequence of one installed unit ([`OutpostRecord::sequenced_assets`]).
/// See [`OUTPOST_PAGE`].
pub const ASSET_PAGE: &str = "assets";

/// The KIND a `ListBlockingInstancesForCapacityTask` page token is stamped with.
///
/// What it pages through is what ONE capacity task of ONE Outpost would need
/// stopped, cut out of that Outpost's own recorded order
/// ([`OutpostRecord::instances`]) — so the stamp carries this kind together with
/// both resources the list is cut from ([`nested_page_kind`]). An instance is
/// launched through Amazon EC2 and no call in this API adds one to that order,
/// removes one from it or reorders it, so the n-th instance recorded on an Outpost
/// stays the n-th for as long as the Outpost holds it and the boundary such a token
/// carries names that one instance. See [`ASSET_PAGE`].
pub const BLOCKING_INSTANCE_PAGE: &str = "blockingInstances";

/// The stamp a `ListAssetInstances` page token carries.
///
/// What it pages through is every instance recorded as running on one Outpost,
/// belonging to all accounts and narrowed by its own four filters — a different
/// list from the one `ListBlockingInstancesForCapacityTask` cuts out of the same
/// recorded order ([`BLOCKING_INSTANCE_PAGE`]), which reports only one account's
/// and drops what a capacity task excludes — so a token issued for one of them is
/// not a boundary of the other, and the stamp carries this kind together with the
/// Outpost the list was cut from ([`nested_page_kind`]). See [`ASSET_PAGE`].
pub const ASSET_INSTANCE_PAGE: &str = "assetInstances";

/// The stamp a `GetOutpostSupportedInstanceTypes` page token carries.
///
/// What that report pages through is the instance-type catalog NARROWED to what one
/// Outpost's hardware can support, so it is a different list from the whole catalog
/// `ListOrderableInstanceTypes` pages ([`ORDERABLE_INSTANCE_TYPE_PAGE`]) even though
/// both name a boundary in the same catalog order — and a token issued for one of
/// them is therefore not a boundary of the other. The report is narrowed per
/// Outpost, so the stamp carries this kind together with the Outpost it was cut from
/// ([`nested_page_kind`]), while the boundary stays the catalog entry's own
/// published sequence. See [`OUTPOST_PAGE`].
pub const SUPPORTED_INSTANCE_TYPE_PAGE: &str = "supportedInstanceTypes";

/// The KIND a `GetOutpostInstanceTypes` page token is stamped with.
///
/// What that report pages through is the instance types one Outpost's capacity is
/// CONFIGURED with
/// ([`ComAmazonawsOutpostsOutpostsolafserviceState::configured_instance_types`]) — a
/// different list from the capability report [`SUPPORTED_INSTANCE_TYPE_PAGE`] stamps
/// and from the published catalog [`ORDERABLE_INSTANCE_TYPE_PAGE`] stamps — so a
/// token issued for one of them is not a boundary of another, and the stamp carries
/// this kind together with the Outpost the report was cut from
/// ([`nested_page_kind`]).
///
/// What a boundary of this one names is a place in the report's own derivation order:
/// a reported entry is an instance TYPE the configuration standing on the Outpost's
/// hardware names, not a record this store created, so there is no creation sequence
/// of an entry to name — the order itself is what the derivation fixes, the same on
/// every read of one store. See [`OUTPOST_PAGE`].
pub const CONFIGURED_INSTANCE_TYPE_PAGE: &str = "outpostInstanceTypes";

/// The page size a list uses when the caller named no `MaxResults`, which is also
/// the ceiling the model declares for it (`MaxResults1000`, `range(1..1000)`).
pub const MAX_RESULTS: i32 = 1000;

/// The ceiling on how many sites, how many Outposts, how many quotes, how many
/// orders, how many capacity tasks and how many installation connections this store
/// holds.
///
/// A generated emulator keeps its resources in memory for the life of the process
/// and a create is the only thing that adds one, so the store needs a maximum entry
/// count: neither a TTL nor an eviction policy is available to it, because `GetSite`
/// must still answer what `CreateSite` created, and `GetCapacityTask` must still
/// answer a task that has long since completed. Past the cap the creating operation
/// refuses the request rather than growing, and because the ceiling is a condition of
/// this emulator and not of the request, the refusal is always a CLIENT error the
/// operation's own model declares — never a 5xx server fault, which SDK retry
/// classifiers treat as retryable and so would have a caller repeat a call that can
/// only fail again. The three creates that model `ServiceQuotaExceededException`
/// (HTTP 402) answer it (it is modelled on exactly them and on no read); `CreateQuote`
/// and `CreateRenewal`, whose models declare no quota error, refuse the create as a
/// bad request (400 `ValidationException`) rather than inventing an error the model
/// does not carry, and so does `StartConnection` for the connection store it grows;
/// `StartCapacityTask`, which models no quota error either, answers the
/// `ConflictException` (HTTP 409) it does model, worded by [`store_full`] so the
/// caller can tell it from the one-active-task refusal. The
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
/// for a statement about the caller's request or about a real AWS quota. Every path
/// that refuses a call for this ceiling words it here, whatever modelled error its
/// own operation carries, so one ceiling reads as one refusal across the service.
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
/// offset would not. It is STAMPED with the list it belongs to — for a list nested
/// under a parent resource, with that parent as well ([`nested_page_kind`]) — so a
/// token replayed against another list is refused rather than honoured as that
/// list's boundary. The rendering is exactly the modelled `Token` pattern
/// `^(\d+)##(\S+)$`.
pub fn encode_page_token(kind: &str, last_sequence: u64) -> String {
    format!("{last_sequence}##{kind}")
}

/// The page boundary a `kind` token names, or `None` when it is not a token this
/// service issued for that list — which the caller is told as a
/// `ValidationException`, never as a silently truncated page.
///
/// A `NextToken` is OPAQUE: the only strings that name a resume point are the ones
/// [`encode_page_token`] issues, so a candidate is accepted only when re-encoding the
/// boundary read out of it reproduces the whole token character for character. That
/// is what makes the check "is this a token this service issued", rather than "can an
/// integer be read out of part of it": a value whose digits parse to a boundary but
/// are spelled some other way (`007`, `+7`) is a string this service could never have
/// minted, and honouring it would serve a page with everything up to that boundary
/// silently dropped. The comparison covers the stamp too, so a token issued for
/// another list is refused by the same one rule.
pub fn decode_page_token(kind: &str, token: &str) -> Option<u64> {
    let (sequence, _) = token.split_once("##")?;
    let boundary: u64 = sequence.parse().ok()?;
    (encode_page_token(kind, boundary) == token).then_some(boundary)
}

/// The stamp the page tokens of a list NESTED under one or more parent resources
/// carry: the kind of list, and every resource that list was cut from.
///
/// A nested list has no existence apart from its parents — the modelled route binds
/// them as labels, and the items are held on the parent's own record — so two parents
/// of one store serve two DIFFERENT lists of the same kind, and a boundary of one is
/// no boundary of the other. Binding the parents into the stamp is what has
/// [`decode_page_token`] refuse such a token instead of reading its number as this
/// list's boundary and serving a page with everything up to that number silently
/// dropped. What narrows a list WITHIN one parent — a filter, a scope the caller
/// asked for — is no part of it: the parents are the resources the list belongs to.
///
/// The identifiers are joined by a character none of this service's own identifier
/// forms holds, and carry no whitespace, so the token stays exactly the modelled
/// `Token` pattern ([`encode_page_token`]).
pub fn nested_page_kind(kind: &str, parents: &[&str]) -> String {
    let mut stamp = String::from(kind);
    for parent in parents {
        stamp.push('.');
        stamp.push_str(parent);
    }
    stamp
}

/// The page boundary the caller's `NextToken` names, or the report a token this
/// service cannot resume from is refused with.
///
/// Shared by every paged answer of this service so they cannot drift apart on that
/// rule: a `NextToken` the caller omitted — or sent empty — names no resume point and
/// so starts at the beginning, a non-empty one names the boundary it carries for the
/// `kind` list it was stamped with ([`decode_page_token`]), and one this service never
/// issued for that list is refused as a bad request rather than honoured or silently
/// truncated, worded by [`page_token_report`] for `operation`. Only the stamp and the
/// operation vary between the paged answers; each wraps the report this returns in the
/// `ValidationException` (400) its own model declares.
pub fn page_boundary(
    kind: &str,
    operation: &str,
    next_token: Option<&str>,
) -> Result<Option<u64>, String> {
    match next_token.filter(|token| !token.is_empty()) {
        Some(token) => decode_page_token(kind, token)
            .map(Some)
            .ok_or_else(|| page_token_report(operation)),
        None => Ok(None),
    }
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

/// Whether the order `order` subscribes the Outpost it was placed on to the hardware
/// its line items name: whether it was not cancelled.
///
/// Nothing was ever delivered for a cancelled order, so it installed no hardware and
/// pays for none — which is why every report of what an Outpost's orders bought skips
/// it ([`ComAmazonawsOutpostsOutpostsolafserviceState::subscribed_orders`]).
fn subscribes_hardware(order: &Order) -> bool {
    order.status.as_deref() != Some(ORDER_STATUS_CANCELLED)
}

/// What each line item of `order` subscribes of the capacity figure `published` reads
/// off the published catalog: that figure for the configuration the line item names,
/// times the quantity of that configuration the item carries.
///
/// The ONE per-line-item fold behind every capacity an order buys, parameterised with
/// the catalog figure the caller wants — the EC2 vCPUs a renewal is priced by
/// ([`ComAmazonawsOutpostsOutpostsolafserviceState::subscribed_capacity_vcpus`]) and
/// the Amazon S3 storage bytes an Outpost has provisioned
/// ([`ComAmazonawsOutpostsOutpostsolafserviceState::provisioned_s3_capacity`]) — so one
/// rule answers how much of it an order holds whichever figure is asked for, and the
/// two can never drift apart on the arithmetic.
///
/// A line item contributes nothing when it names no configuration, when it names a
/// catalog item this service publishes no configuration for, or when the figure that
/// configuration publishes is not positive: the capacity of hardware the catalog does
/// not describe is not a figure the emulator has, and never one it invents, and
/// hardware the catalog publishes none of that capacity for carries none.
///
/// Per item rather than one total, so a caller totalling over several orders at once
/// adds up exactly the contributing items and the fold itself holds no total.
fn subscribed_capacities<'a>(
    order: &'a Order,
    published: impl Fn(&str) -> Option<i64> + 'a,
) -> impl Iterator<Item = i64> + 'a {
    order.line_items.iter().flatten().filter_map(move |item| {
        let capacity = published(item.catalog_item_id.as_deref()?)?;
        if capacity <= 0 {
            return None;
        }
        // `LineItem$Quantity` is modelled `range(min: 1)`, so a quantity the boundary
        // accepted is at least one unit of that configuration — which is also what an
        // absent member means. The floor is belt and braces against a value the
        // boundary cannot deliver: a negative quantity would otherwise subtract
        // capacity the order never cancelled. Saturating, so a quantity and a figure
        // whose product no `Long` holds reports the largest capacity one can rather
        // than wrapping to a negative one.
        let quantity = i64::from(item.quantity.unwrap_or(1).max(0));
        Some(capacity.saturating_mul(quantity))
    })
}

/// Append `value` to `values` unless it is already there, keeping the position it
/// first appeared in.
///
/// What a caller is reported is a LIST of distinct values, and more than one record
/// of this store can name the same one (two orders of the same configuration, two
/// pieces of hardware configured with the same instance type), so the first record
/// that names a value fixes where it is reported and the next changes nothing.
fn push_once(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|held| held == value) {
        values.push(value.to_string());
    }
}

/// Append the instance types one COMPLETED capacity task leaves CONFIGURED on the
/// hardware it named, each reported once ([`push_once`]).
///
/// A pool of `Count` zero ("the number of instances for the specified instance type")
/// configures no instance of its type, so it names no configured instance type. See
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::configured_instance_types`].
fn push_configured_pools(values: &mut Vec<String>, task: &CapacityTaskRecord) {
    for pool in &task.requested_instance_pools {
        if pool.count > 0 {
            push_once(values, &pool.instance_type);
        }
    }
}

/// The per-instance-type capacity one COMPLETED capacity task leaves CONFIGURED on
/// the hardware it named: one entry per instance type its pools configure an instance
/// of, carrying how many instances of that type they asked for, in the order the task
/// first named each type.
///
/// The same reading of the same pools [`push_configured_pools`] takes, carrying the
/// counts through: `InstanceTypeCapacity$Count` is "the number of instances for the
/// specified instance type", so pools of one type together configure that many
/// instances of it and a pool of `Count` zero configures none — which is why a type
/// asked for none of is no part of the configuration that stands, here as there. See
/// [`ComAmazonawsOutpostsOutpostsolafserviceState::configure_hardware`].
fn configured_capacities(task: &CapacityTaskRecord) -> Vec<AssetInstanceTypeCapacity> {
    let mut capacities: Vec<AssetInstanceTypeCapacity> = Vec::new();
    for pool in &task.requested_instance_pools {
        if pool.count <= 0 {
            continue;
        }
        match capacities
            .iter_mut()
            .find(|held| held.instance_type == pool.instance_type)
        {
            Some(held) => held.count = held.count.saturating_add(pool.count),
            None => capacities.push(AssetInstanceTypeCapacity {
                instance_type: pool.instance_type.clone(),
                count: pool.count,
            }),
        }
    }
    capacities
}

/// Whether a record's value for ONE filtered member is accepted by the `*Filter`
/// the caller sent for that member.
///
/// Every list in this API states the same selection rule — "if you specify multiple
/// filters, the results include only the resources that match all of the specified
/// filters. For a filter where you can specify multiple values, the results include
/// items that match any of the values that you specify for the filter" — so a filter
/// is a LIST of ACCEPTED VALUES for the member it names: a record matches it when its
/// value for that member is one of them, a filter the caller did not send (or sent
/// empty) narrows nothing, and a record that recorded no value at all for a filtered
/// member matches none of the accepted values, so the filter narrows it out. A
/// handler combines one call per filter member with `&&`, which is what makes several
/// filters narrow rather than widen.
///
/// Shared by every list operation of this service that selects on a single-valued
/// filtered member, so they cannot drift apart on that rule.
pub fn filter_accepts(filter: Option<&[String]>, value: Option<&str>) -> bool {
    match filter {
        Some(accepted) if !accepted.is_empty() => {
            value.is_some_and(|value| accepted.iter().any(|candidate| candidate.as_str() == value))
        }
        _ => true,
    }
}

/// Whether a record's value for ONE member filtered by an IDENTIFIER filter is
/// accepted by the already-resolved filter the caller sent for that member.
///
/// The single-valued counterpart of [`filter_accepts`], for the filters modelled as an
/// identifier rather than as a list of values: such a filter names ONE resource, and
/// because it arrives as `id OR ARN` it is resolved against the endpoint the request
/// reached ([`resolve_id`]) before it selects. The selection rule is the same one —
/// a filter the caller did not send narrows nothing, and a record matches a filter
/// only when its value for the filtered member is the one the filter names, so a
/// record that recorded no value there matches no filter at all. Because the value is
/// RESOLVED, one naming a resource of another endpoint
/// ([`ResolvedIdentifier::Elsewhere`]) names nothing of this inventory and so no
/// record can match it: the caller is reported an empty list rather than a failure,
/// which is what makes a filter a filter and not an identifier the request is
/// addressed to. A handler combines one call per filter member with `&&`, exactly as
/// it does for [`filter_accepts`].
///
/// Shared by the list operations that take an identifier filter so they cannot drift
/// apart on that rule.
pub fn identifier_filter_accepts(filter: Option<&ResolvedIdentifier>, value: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(filter) => filter
            .here()
            .is_some_and(|resource_id| value == Some(resource_id)),
    }
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
