// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions service.
//!
//! One store holds every resource the service's operations share: identity-provider
//! registrations, the product users subscribed through them, the EC2 instances that
//! provide those subscriptions, the users associated to those instances, the license
//! server endpoints created for a registration, and the tags written against any of
//! those resources' ARNs. The two fleets this service resolves a request against but
//! does not own — the account's EC2 instances and its AWS Directory Service directories —
//! live here too, each behind its own seed seam. The
//! list/lookup/pagination/filter helpers live here as well, so the operation handlers in
//! `ops/` all resolve a resource the same way instead of each growing its own copy.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Filter, IdentityProvider, IdentityProviderSummary, InstanceSummary, InstanceUserSummary,
    LicenseServerEndpoint, LicenseServerSettings, ProductUserSummary, ServerEndpoint, Settings,
};

/// The status a registration carries while its VPC endpoints are being provisioned.
pub const STATUS_REGISTERING: &str = "REGISTERING";
/// The status a registration carries once provisioning completed.
pub const STATUS_REGISTERED: &str = "REGISTERED";
/// The status returned by `DeregisterIdentityProvider` for the registration it removed.
pub const STATUS_DEREGISTERING: &str = "DEREGISTERING";
/// The status a product user carries while its subscription is active.
pub const STATUS_SUBSCRIBED: &str = "SUBSCRIBED";
/// The status returned by `StopProductSubscription` for the subscription it ended.
pub const STATUS_UNSUBSCRIBED: &str = "UNSUBSCRIBED";
/// The status a user association carries while the instance is being configured.
pub const STATUS_ASSOCIATING: &str = "ASSOCIATING";
/// The status a user association carries once configuration completed.
pub const STATUS_ASSOCIATED: &str = "ASSOCIATED";
/// The status returned by `DisassociateUser` for the association it ended.
pub const STATUS_DISASSOCIATING: &str = "DISASSOCIATING";
/// The status an instance carries in this service's view while at least one user is
/// associated with it.
///
/// `InstanceSummary.Status` is a free-form string in the model (no enum to honour), and
/// the only thing this service knows about an instance is that a user association brought
/// it into view; so the one writer records this one value rather than inventing a
/// per-instance status nothing computes.
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// The `LicenseServerEndpointProvisioningStatus` an endpoint carries while its network
/// endpoint is being provisioned.
pub const PROVISIONING_STATUS_PROVISIONING: &str = "PROVISIONING";
/// The `LicenseServerEndpointProvisioningStatus` an endpoint carries once provisioning
/// completed.
pub const PROVISIONING_STATUS_PROVISIONED: &str = "PROVISIONED";
/// The `LicenseServerEndpointProvisioningStatus` an endpoint carries from the moment
/// `DeleteLicenseServerEndpoint` accepted its deletion until that deletion completes.
pub const PROVISIONING_STATUS_DELETING: &str = "DELETING";
/// The terminal `LicenseServerEndpointProvisioningStatus` an endpoint reaches once the
/// deletion that put it in [`PROVISIONING_STATUS_DELETING`] has completed.
pub const PROVISIONING_STATUS_DELETED: &str = "DELETED";

/// Whether `status` is a `LicenseServerEndpointProvisioningStatus` an endpoint carries
/// because its deletion was accepted — `DELETING` while the deletion runs, `DELETED` once
/// it finished.
///
/// An endpoint in either status is still REPORTED (that is the point of modelling the two
/// statuses) but is no longer a live resource: it cannot be addressed by
/// `(IdentityProviderArn, ServerType)`, it is not taggable, and it does not count against
/// the account's endpoint quota.
pub fn endpoint_deletion_started(status: &str) -> bool {
    status == PROVISIONING_STATUS_DELETING || status == PROVISIONING_STATUS_DELETED
}

/// How long after `RegisterIdentityProvider` the registration becomes `REGISTERED`.
///
/// A chosen span (AWS documents no duration for it), kept inside the emulator's
/// 250-750 ms window for a clock-owed transition: the registration is returned in the
/// non-terminal `REGISTERING` status, so a caller that reads it straight away still
/// observes that transient status, and any later read materialises the transition the
/// clock already owed.
const REGISTRATION_SPAN: Duration = Duration::from_millis(500);

/// How long after `AssociateUser` the association becomes `ASSOCIATED`.
///
/// A chosen span, on the same footing as [`REGISTRATION_SPAN`]: AWS documents no
/// duration for it, the association is returned in the non-terminal `ASSOCIATING`
/// status, and the span is inside the emulator's 250-750 ms window so a caller that
/// reads straight back still observes the transient status.
const ASSOCIATION_SPAN: Duration = Duration::from_millis(500);

/// How long after `CreateLicenseServerEndpoint` the endpoint becomes `PROVISIONED`.
///
/// A chosen span (AWS documents no duration for it), kept inside the emulator's
/// 250-750 ms window for a clock-owed transition: the endpoint is created in the
/// non-terminal `PROVISIONING` status, so a caller that lists it straight away still
/// observes that transient status, and a later read materialises the transition the
/// clock already owed.
const ENDPOINT_PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// How long after `DeleteLicenseServerEndpoint` the endpoint reaches the terminal
/// `DELETED`.
///
/// A chosen span (AWS documents no duration for it), kept inside the emulator's
/// 250-750 ms window for a clock-owed transition: the delete call answers with the
/// non-terminal `DELETING` status, so a caller that lists the endpoint straight away
/// still observes that transient status, and a later read materialises the terminal
/// `DELETED` the clock already owed.
const ENDPOINT_DELETION_SPAN: Duration = Duration::from_millis(500);

/// The most endpoints whose deletion has completed that the store keeps reporting.
///
/// A deleted endpoint stays observable in its terminal `DELETED` status rather than
/// vanishing between two reads, so the records cannot simply be dropped — but a caller
/// looping create/delete must not grow the store without bound either. Beyond this many,
/// the endpoints deleted EARLIEST are dropped, which keeps the retained set bounded by
/// the same number as the live one ([`MAX_RESOURCES_PER_KIND`]).
pub const MAX_DELETED_ENDPOINTS_RETAINED: usize = MAX_RESOURCES_PER_KIND;

/// The most subnets one `UpdateSettings` payload may name in `AddSubnets` or in
/// `RemoveSubnets`.
///
/// `Subnets` carries `@length(min: 1)` and NO maximum in the model, so the bound is the
/// account's rather than the shape's: EC2's default quota is 200 subnets per VPC, so a
/// payload naming more subnets than a VPC can hold could not be satisfied whatever the
/// store holds. Bounding the payload is also what keeps the reconciliation in
/// [`apply_update_settings`] — which runs with the service-state lock held — linear and
/// bounded in the request.
pub const MAX_SUBNETS_PER_UPDATE: usize = 200;

/// The most resources of one kind (registrations, user associations, license server
/// endpoints) this emulator holds for an account.
///
/// AWS bounds these resources per account and models the refusal:
/// `RegisterIdentityProvider`, `AssociateUser` and `CreateLicenseServerEndpoint` all
/// declare `ServiceQuotaExceededException` ("The request failed because a service quota
/// is exceeded"). The NUMBER here is the emulator's own — AWS publishes its quotas
/// separately and they change — chosen so that a caller minting distinct resources cannot
/// grow the in-memory store without bound, and far above anything a test or a recorded
/// fixture creates.
pub const MAX_RESOURCES_PER_KIND: usize = 1_000;

/// The most tags one of this service's resources carries.
///
/// The model bounds a tagging PAYLOAD (`Tags` has `@length(min: 0, max: 50)`) but says
/// nothing about the ACCUMULATION of repeated writes, and AWS's own limit is 50 tags per
/// resource — so 50 is the bound on the MERGED set too, enforced before a merge so a
/// repeated `TagResource` cannot grow one resource's tag map without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most characters a tag KEY holds.
///
/// `Tags` is `map<String, String>` and carries `@length(min: 0, max: 50)` on the MAP only
/// (`smithy_query(query="shape", shape="Tags")`), so nothing in the model constrains an
/// individual key and the boundary validator checks none. The rule is the
/// service-independent one from the AWS tagging reference ("Tag key length: Maximum 128
/// Unicode characters in UTF-8"), which every AWS tagging API enforces, counted in
/// CHARACTERS and not bytes for exactly that reason.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The most characters a tag VALUE holds: the same tagging reference's "Tag value length:
/// Maximum 256 Unicode characters in UTF-8". A value may be EMPTY (unlike a key), which
/// is why only a maximum is stated here.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The fewest characters a tag KEY holds: a key is required, so the empty string is not
/// one ("Tag key length: Minimum 1" in the same reference).
pub const MIN_TAG_KEY_CHARS: usize = 1;

/// The tag-key namespace AWS keeps for itself: "Do not use the `aws:` prefix in your tag
/// names or values because it is reserved for AWS use", so a caller can neither set nor
/// remove a tag in it. Compared case-INSENSITIVELY, because AWS reserves the namespace
/// rather than one spelling of it.
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// Check the tag set a request asks this service to WRITE — `TagResource`'s `Tags`, and
/// the `Tags` map of every operation that creates a resource with tags.
///
/// Refuses, with [`StateError::Validation`] (every such operation models
/// `ValidationException`, HTTP 400), a key outside
/// [`MIN_TAG_KEY_CHARS`]..=[`MAX_TAG_KEY_CHARS`], a value longer than
/// [`MAX_TAG_VALUE_CHARS`], and any key in the [`RESERVED_TAG_PREFIX`] namespace. The
/// whole request is refused so a payload carrying one illegal tag writes NONE of its
/// tags. Keys are examined in sorted order, so a payload breaking several rules names the
/// same tag every time rather than whichever one the map happened to yield first.
pub fn validate_tags(tags: &HashMap<String, String>) -> Result<(), StateError> {
    let mut keys: Vec<&String> = tags.keys().collect();
    keys.sort_unstable();
    for key in keys {
        validate_tag_key("Tags", key)?;
        let value = &tags[key];
        let length = value.chars().count();
        if length > MAX_TAG_VALUE_CHARS {
            return Err(StateError::Validation(format!(
                "Tags: the value of tag key {key:?} must hold at most \
                 {MAX_TAG_VALUE_CHARS} characters, but holds {length}"
            )));
        }
    }
    Ok(())
}

/// Check the tag keys a request asks this service to REMOVE (`UntagResource`'s `TagKeys`).
///
/// The same key rule as [`validate_tags`], because it is a rule about what a tag key IS:
/// a key AWS reserves cannot be removed any more than it can be set, and a key outside
/// the length bounds names no tag any resource could carry. A legal key the resource does
/// not carry is NOT an error — removal stays idempotent.
pub fn validate_tag_keys(keys: &[String]) -> Result<(), StateError> {
    for key in keys {
        validate_tag_key("TagKeys", key)?;
    }
    Ok(())
}

/// The rules every tag key obeys, whichever member carries it. `member` is named in the
/// refusal so a caller sees which part of the request to fix.
fn validate_tag_key(member: &str, key: &str) -> Result<(), StateError> {
    let length = key.chars().count();
    if !(MIN_TAG_KEY_CHARS..=MAX_TAG_KEY_CHARS).contains(&length) {
        return Err(StateError::Validation(format!(
            "{member}: a tag key must hold between {MIN_TAG_KEY_CHARS} and \
             {MAX_TAG_KEY_CHARS} characters, but {key:?} holds {length}"
        )));
    }
    // `RESERVED_TAG_PREFIX` is spelled in lowercase, so lowercasing the key is what makes
    // the comparison case-insensitive.
    if key.to_lowercase().starts_with(RESERVED_TAG_PREFIX) {
        return Err(StateError::Validation(format!(
            "{member}: tag key {key:?} uses the {RESERVED_TAG_PREFIX} prefix, which is \
             reserved for AWS use"
        )));
    }
    Ok(())
}

/// What the operations that RESOLVE an identity-provider reference say about one that
/// resolves to no registration: the provider named is not a valid parameter value.
///
/// One sentence in one place, because it is one resolution: `CreateLicenseServerEndpoint`
/// and `UpdateIdentityProviderSettings` both refuse an unresolvable reference with it,
/// and neither models a not-found for this surface.
pub const IDENTITY_PROVIDER_NOT_VALID: &str = "Identity provider is not valid";

/// Why an operation could not be served from state.
///
/// Each handler maps this onto its OWN modelled error enum (the generated enums are
/// per-operation types), so the lookup rules themselves live here exactly once.
#[derive(Debug, Clone)]
pub enum StateError {
    /// The addressed resource is not in the store — HTTP 404 `ResourceNotFoundException`.
    NotFound(String),
    /// The resource already exists in a conflicting state — `ConflictException`.
    Conflict(String),
    /// The request is malformed in a way the model cannot express — HTTP 400
    /// `ValidationException`.
    Validation(String),
    /// The identity provider the request addressed — by `IdentityProviderArn`, or by the
    /// `(IdentityProvider, Product)` pair it would have been registered under — is not a
    /// registration of this account.
    ///
    /// Carries no message, because the two planes of this service word it differently and
    /// neither wording belongs to the lookup:
    ///
    /// * the operations that manage or report the REGISTRATION itself
    ///   (`DeregisterIdentityProvider`, `ListProductSubscriptions`) report the identifier
    ///   as an invalid parameter — the modelled `ValidationException` (HTTP 400) carrying
    ///   [`IDENTITY_PROVIDER_NOT_VALID`];
    /// * the operations that start or end a user's PRODUCT SUBSCRIPTION
    ///   (`StartProductSubscription`, `StopProductSubscription`) fail closed with
    ///   `UnauthorizedException` instead, disclosing nothing about whether the provider
    ///   exists.
    ///
    /// `AssociateUser` is neither: it resolves its registration itself and answers its own
    /// modelled `ResourceNotFoundException`, because the fact its authorization is about —
    /// the caller's ownership of the EC2 instance — has already been established by the
    /// time the directory is resolved, so there is nothing left to withhold.
    ProviderNotRegistered,
}

/// The user-based subscription products this service admits.
///
/// `Product` is modelled as a plain `String`, so the boundary validator enforces nothing;
/// the set comes from `RegisterIdentityProvider.Product`'s own documentation — "Valid
/// values: VISUAL_STUDIO_ENTERPRISE | VISUAL_STUDIO_PROFESSIONAL |
/// OFFICE_PROFESSIONAL_PLUS | OFFICE_STANDARD | REMOTE_DESKTOP_SERVICES" — which the
/// `Product` member of `StartProductSubscription`, `StopProductSubscription`,
/// `DeregisterIdentityProvider` and `ListProductSubscriptions` repeats verbatim.
pub const PRODUCTS: [&str; 5] = [
    "VISUAL_STUDIO_ENTERPRISE",
    "VISUAL_STUDIO_PROFESSIONAL",
    "OFFICE_PROFESSIONAL_PLUS",
    "OFFICE_STANDARD",
    "REMOTE_DESKTOP_SERVICES",
];

/// Refuse a `Product` outside [`PRODUCTS`], naming the rejected value so a caller sees
/// which value was refused.
///
/// Every operation that takes a `Product` on the wire calls this before it resolves
/// anything in the store: whether a product name is one this service serves does not
/// depend on what is registered.
pub fn validate_product(product: &str) -> Result<(), StateError> {
    if PRODUCTS.contains(&product) {
        Ok(())
    } else {
        Err(StateError::Validation(format!(
            "Invalid product {product}"
        )))
    }
}

/// How this service states that a request named none of a set of members it had to name.
///
/// Several operations address a resource either by its ARN or by the members that identify
/// it, and refuse a request supplying neither by LISTING those members — the form
/// `StopProductSubscription` (`Username`/`IdentityProvider`/`Product`) and
/// `DisassociateUser` (`Username`/`InstanceId`/`IdentityProvider`) both use. The members
/// are listed in alphabetical order, as both of those sentences order them, so the
/// sentence does not depend on the order a handler happens to pass them in.
pub fn members_must_be_provided(members: &[&str]) -> String {
    let mut names: Vec<&str> = members.to_vec();
    names.sort_unstable();
    let listed = match names.split_last() {
        // A refusal that names no member states no requirement, so there is no sentence to
        // build: every call site passes a literal, non-empty list of the members the
        // operation's model declares.
        None => unreachable!("a refusal must name the members it requires"),
        Some((last, [])) => (*last).to_string(),
        Some((last, [only])) => format!("{only} and {last}"),
        Some((last, rest)) => format!("{}, and {last}", rest.join(", ")),
    };
    format!("{listed} must be provided")
}

/// A registered identity provider, as `RegisterIdentityProvider` stored it.
#[derive(Debug, Clone)]
pub struct IdentityProviderRecord {
    /// The minted `IdentityProviderArn`; the key of `identity_providers`.
    pub arn: String,
    /// The provider union exactly as registered.
    pub identity_provider: IdentityProvider,
    /// The user-based subscription product this registration is for.
    pub product: String,
    /// The product-related configuration settings (subnets + security group).
    pub settings: Settings,
    /// `REGISTERING` until [`REGISTRATION_SPAN`] has elapsed, then `REGISTERED`.
    pub status: String,
    /// Set only when provisioning failed; never fabricated.
    pub failure_message: Option<String>,
    /// The account that owns the registration.
    pub owner_account_id: String,
    /// When the registration was created.
    pub created_at: SystemTime,
    /// When the `REGISTERING` -> `REGISTERED` transition falls due.
    pub registered_due_at: SystemTime,
}

/// An EC2 instance that provides user-based subscriptions.
///
/// An instance is not created by an API of this service: it enters the service's view
/// through the user-association plane (`AssociateUser` names the `InstanceId` a user is
/// associated with), and `ListInstances` only REPORTS what is held here. So this record
/// carries no value `ListInstances` invents — every member is what the writer persisted.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    /// The EC2 instance id; the key of `instances`.
    pub instance_id: String,
    /// The status of the instance resource, as the writer recorded it. The model
    /// (`InstanceSummary.Status`) is a free-form string, so no vocabulary is imposed here.
    pub status: String,
    /// The user-based subscription products this instance provides, in the order they
    /// were added.
    pub products: Vec<String>,
    /// Set only when the service has something to report; never fabricated.
    pub status_message: Option<String>,
    /// The account that owns the instance.
    pub owner_account_id: String,
    /// The registration the instance's users were resolved through, when one is known.
    pub identity_provider: Option<IdentityProvider>,
    /// When the instance's status was last checked, if it ever was. A status check is
    /// work this service performs, so a read never invents one.
    pub last_status_check_at: Option<SystemTime>,
}

/// A product user, as `StartProductSubscription` stored it.
#[derive(Debug, Clone)]
pub struct ProductUserRecord {
    /// The minted `ProductUserArn`; the key of `product_users`.
    pub arn: String,
    /// The user name from the identity provider.
    pub username: String,
    /// The user-based subscription product.
    pub product: String,
    /// The ARN of the identity provider this user was resolved through.
    pub identity_provider_arn: String,
    /// `SUBSCRIBED` while the subscription is active.
    pub status: String,
    /// Set only when the service has something to report; never fabricated.
    pub status_message: Option<String>,
    /// The Active Directory domain the user information came from.
    pub domain: Option<String>,
    /// ISO-8601 instant the subscription started.
    pub subscription_start_date: Option<String>,
    /// ISO-8601 instant the subscription ended, once it has.
    pub subscription_end_date: Option<String>,
    /// Only products backed by a license server endpoint carry one, so this stays
    /// `None` for a subscription this slice creates.
    pub license_expiration_date: Option<String>,
}

/// A user association to an EC2 instance, as `AssociateUser` stored it.
///
/// An association is to a DIRECTORY, not to a product registration:
/// `AssociateUser`/`DisassociateUser` carry an `IdentityProvider` and no `Product`
/// (`smithy_query(query="operation", operation="AssociateUser")`), so the record is keyed
/// by the same [`provider_key`] a registration is indexed under rather than by one
/// registration's ARN.
#[derive(Debug, Clone)]
pub struct InstanceUserRecord {
    /// The minted `InstanceUserArn`; the key of `instance_users`.
    pub arn: String,
    /// The user name from the identity provider.
    pub username: String,
    /// The EC2 instance the user was associated with.
    pub instance_id: String,
    /// The provider union exactly as the association named it.
    pub identity_provider: IdentityProvider,
    /// [`provider_key`] of `identity_provider`: what `instance_user_index` keys on.
    pub provider_key: String,
    /// `ASSOCIATING` until [`ASSOCIATION_SPAN`] has elapsed, then `ASSOCIATED`.
    pub status: String,
    /// Set only when the service has something to report; never fabricated.
    pub status_message: Option<String>,
    /// The Active Directory domain the user information came from.
    pub domain: Option<String>,
    /// ISO-8601 instant the association was created.
    pub association_date: Option<String>,
    /// ISO-8601 instant the association ended, once it has.
    pub disassociation_date: Option<String>,
    /// When the `ASSOCIATING` -> `ASSOCIATED` transition falls due.
    pub associated_due_at: SystemTime,
}

/// A license server endpoint, as `CreateLicenseServerEndpoint` stored it.
#[derive(Debug, Clone)]
pub struct LicenseServerEndpointRecord {
    /// The minted `LicenseServerEndpointArn`; the key of `license_server_endpoints`.
    pub arn: String,
    /// The minted `LicenseServerEndpointId`.
    pub id: String,
    /// The registration this endpoint was created for.
    pub identity_provider_arn: String,
    /// The `ServerType` the endpoint serves (the model's only value is `RDS_SAL`).
    pub server_type: String,
    /// The license-server settings exactly as created, so nothing the caller supplied is
    /// lost between operations.
    pub settings: LicenseServerSettings,
    /// The network address of the endpoint, reported only once it is `PROVISIONED`:
    /// there is no address to reach while it is still being provisioned.
    pub endpoint: String,
    /// `PROVISIONING` until [`ENDPOINT_PROVISIONING_SPAN`] has elapsed, then
    /// `PROVISIONED`; `DELETING` from the moment a deletion was accepted until
    /// [`ENDPOINT_DELETION_SPAN`] has elapsed, then the terminal `DELETED`.
    pub provisioning_status: String,
    /// Set only when the service has something to report; never fabricated.
    pub status_message: Option<String>,
    /// When the endpoint was created.
    pub created_at: SystemTime,
    /// When the `PROVISIONING` -> `PROVISIONED` transition falls due.
    pub provisioned_due_at: SystemTime,
    /// When the deletion was accepted, or `None` while the endpoint is still live. The
    /// order deletions were accepted in is what bounds the retained set (see
    /// [`MAX_DELETED_ENDPOINTS_RETAINED`]).
    pub deletion_started_at: Option<SystemTime>,
    /// When the `DELETING` -> `DELETED` transition falls due, or `None` while the
    /// endpoint is still live.
    pub deleted_due_at: Option<SystemTime>,
}

/// Per-service state for com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState {
    /// Registrations by `IdentityProviderArn`.
    pub identity_providers: HashMap<String, IdentityProviderRecord>,
    /// Registration ARNs in registration order, so a list is stable and pageable.
    pub identity_provider_order: Vec<String>,
    /// `(provider key, product) -> IdentityProviderArn`, so a caller that addresses a
    /// registration by `(IdentityProvider, Product)` resolves the same record an ARN does.
    pub identity_provider_index: HashMap<(String, String), String>,
    /// Product users by `ProductUserArn`.
    pub product_users: HashMap<String, ProductUserRecord>,
    /// Product-user ARNs in subscription order, so a list is stable and pageable.
    pub product_user_order: Vec<String>,
    /// `(IdentityProviderArn, product, username) -> ProductUserArn`.
    pub product_user_index: HashMap<(String, String, String), String>,
    /// License server endpoints by `LicenseServerEndpointArn`.
    pub license_server_endpoints: HashMap<String, LicenseServerEndpointRecord>,
    /// Endpoint ARNs in creation order, so a list is stable and pageable.
    pub license_server_endpoint_order: Vec<String>,
    /// `(IdentityProviderArn, ServerType) -> LicenseServerEndpointArn`, so a repeat
    /// create resolves the endpoint already in state instead of minting a second one.
    pub license_server_endpoint_index: HashMap<(String, String), String>,
    /// Instances providing user-based subscriptions, by EC2 instance id.
    pub instances: HashMap<String, InstanceRecord>,
    /// Instance ids in the order the instances entered the store, so a list is stable
    /// and pageable.
    pub instance_order: Vec<String>,
    /// Tags by resource ARN, written by whichever operation accepted them, so a later
    /// tagging operation reads the same store.
    pub tags: HashMap<String, HashMap<String, String>>,
    /// User associations by `InstanceUserArn`.
    pub instance_users: HashMap<String, InstanceUserRecord>,
    /// Association ARNs in association order, so a list is stable and pageable.
    pub instance_user_order: Vec<String>,
    /// `(provider key, InstanceId, Username) -> InstanceUserArn`, so a caller that
    /// addresses an association by its identifier triple resolves the same record its
    /// ARN does.
    pub instance_user_index: HashMap<(String, String, String), String>,
    /// The EC2 instances this account has, keyed by instance id.
    ///
    /// Whether the caller may associate a user with an instance is a fact about the
    /// ACCOUNT's EC2 fleet, not about this service: this service models no operation
    /// that launches an instance, so without a seed nothing could ever be authorized.
    /// [`seed_account_instances`] is therefore a fixture standing in for EC2's own
    /// `RunInstances`, exactly as `ec2instanceconnect`'s `seed_instances` is for its
    /// instance registry, and it is the single place an EC2 emulator with an instance
    /// store would publish instances for this service to read. A test registers its own
    /// with [`ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::register_account_instance`].
    pub account_instances: HashMap<String, String>,
    /// The AWS Directory Service directories this account has, keyed by `DirectoryId`,
    /// each carrying the `DirectoryStage` it is in.
    ///
    /// Whether an identity provider can be registered for a directory is a fact about the
    /// ACCOUNT's directories, not about this service: this service models no operation
    /// that creates one, so — exactly like [`account_instances`](Self::account_instances)
    /// above — [`seed_account_directories`] is the fixture standing in for Directory
    /// Service's own `CreateDirectory`, and the single place a directory store would
    /// publish directories for this service to read. A test registers its own with
    /// [`ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::register_account_directory`].
    pub account_directories: HashMap<String, String>,
    /// The earliest instant at which a stored record owes the clock a status transition,
    /// or `None` when no record is in a non-terminal status.
    ///
    /// [`materialise_due`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::materialise_due)
    /// runs on EVERY request, so it must not sweep every stored record every time: this
    /// is the O(1) guard that makes the sweep happen only when the clock actually owes
    /// something. Every writer that stores a record in a non-terminal status calls
    /// [`schedule_transition`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::schedule_transition)
    /// with that record's due instant; forgetting to would delay the transition until
    /// another record falls due, never corrupt a record.
    pub pending_transition_due_at: Option<SystemTime>,
    /// Source of the resource ids this service mints.
    pub next_id: u64,
}

/// The EC2 instances the account starts with, as `(instance id, Availability Zone)`.
///
/// Three instances in the Region the emulator writes into the SDK shared config
/// (`us-east-1`, see `vendor/server/src/lib.rs`), with ids of EC2's own
/// `i-[0-9a-f]{17}` form. The set is deliberately small and fixed: it decides only
/// whether an instance EXISTS, which is all this service's authorization gate reads.
pub fn seed_account_instances() -> Vec<(String, String)> {
    [
        ("i-0f1e2d3c4b5a69788", "us-east-1a"),
        ("i-0e2d3c4b5a6978899", "us-east-1b"),
        ("i-0d3c4b5a697889900", "us-east-1c"),
    ]
    .into_iter()
    .map(|(id, zone)| (id.to_string(), zone.to_string()))
    .collect()
}

/// The stage a directory has to be in before an identity provider can be registered for
/// it: `Active`, one of the values AWS Directory Service's `DirectoryStage` publishes
/// (`Requested`, `Creating`, `Created`, `Active`, `Inoperable`, `Impaired`, `Restoring`,
/// `RestoreFailed`, `Deleting`, `Deleted`, `Failed`). A directory still being created, or
/// being deleted, is not one a registration can be built on.
pub const DIRECTORY_STAGE_ACTIVE: &str = "Active";

/// The directories the account starts with, as `(DirectoryId, DirectoryStage)`.
///
/// Three `Active` directories with ids of Directory Service's own `d-[0-9a-f]{10}` form
/// (the `@pattern` `ActiveDirectoryIdentityProvider.DirectoryId` carries). The set is
/// deliberately small and fixed: it decides only whether a directory of that id EXISTS
/// and is usable, which is all a registration reads.
pub fn seed_account_directories() -> Vec<(String, String)> {
    ["d-9f8e7d6c5b", "d-1a2b3c4d5e", "d-4455667788"]
        .into_iter()
        .map(|id| (id.to_string(), DIRECTORY_STAGE_ACTIVE.to_string()))
        .collect()
}

impl Default for ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState {
    fn default() -> Self {
        Self {
            identity_providers: HashMap::new(),
            identity_provider_order: Vec::new(),
            identity_provider_index: HashMap::new(),
            product_users: HashMap::new(),
            product_user_order: Vec::new(),
            product_user_index: HashMap::new(),
            instances: HashMap::new(),
            instance_order: Vec::new(),
            tags: HashMap::new(),
            instance_users: HashMap::new(),
            instance_user_order: Vec::new(),
            instance_user_index: HashMap::new(),
            license_server_endpoints: HashMap::new(),
            license_server_endpoint_order: Vec::new(),
            license_server_endpoint_index: HashMap::new(),
            account_instances: seed_account_instances().into_iter().collect(),
            account_directories: seed_account_directories().into_iter().collect(),
            pending_transition_due_at: None,
            next_id: 0,
        }
    }
}

impl ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState {
    /// Mint the resource-id portion of an ARN: `<prefix>-<12 lowercase hex digits>`.
    ///
    /// Monotonic, so ids are unique within a run and the mint order is the list order.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{:012x}", self.next_id)
    }

    /// Materialise every transition the clock already made due at `now`.
    ///
    /// Only transitions that were owed at `now` whether or not anybody called: a
    /// registration whose [`REGISTRATION_SPAN`] has elapsed becomes `REGISTERED`, a user
    /// association whose [`ASSOCIATION_SPAN`] has elapsed becomes `ASSOCIATED`, and a
    /// license server endpoint whose [`ENDPOINT_PROVISIONING_SPAN`] has elapsed becomes
    /// `PROVISIONED`, and one whose deletion was accepted [`ENDPOINT_DELETION_SPAN`] ago
    /// reaches the terminal `DELETED`. Reporting operations call this so a read reflects
    /// the clock without the read itself changing anything a later caller would not have
    /// seen anyway.
    ///
    /// Every request runs this, so the common case must not scale with how much the store
    /// holds: `pending_transition_due_at` is the O(1) guard, and a record is visited only
    /// on the one request that finds a transition due.
    pub fn materialise_due(&mut self, now: SystemTime) {
        match self.pending_transition_due_at {
            Some(due) if now >= due => {}
            // Either nothing is in a non-terminal status, or the earliest transition is
            // still in the future: there is nothing to materialise, and nothing to scan.
            _ => return,
        }

        // The sweep also recomputes the guard, from the records that are still pending.
        let mut next: Option<SystemTime> = None;
        let still_pending = |due_at: SystemTime, next: &mut Option<SystemTime>| {
            *next = Some(next.map_or(due_at, |earliest: SystemTime| earliest.min(due_at)));
        };
        for record in self.identity_providers.values_mut() {
            if record.status != STATUS_REGISTERING {
                continue;
            }
            if now >= record.registered_due_at {
                record.status = STATUS_REGISTERED.to_string();
            } else {
                still_pending(record.registered_due_at, &mut next);
            }
        }
        for record in self.instance_users.values_mut() {
            if record.status != STATUS_ASSOCIATING {
                continue;
            }
            if now >= record.associated_due_at {
                record.status = STATUS_ASSOCIATED.to_string();
            } else {
                still_pending(record.associated_due_at, &mut next);
            }
        }
        for record in self.license_server_endpoints.values_mut() {
            if record.provisioning_status == PROVISIONING_STATUS_PROVISIONING {
                if now >= record.provisioned_due_at {
                    record.provisioning_status = PROVISIONING_STATUS_PROVISIONED.to_string();
                } else {
                    still_pending(record.provisioned_due_at, &mut next);
                }
            } else if record.provisioning_status == PROVISIONING_STATUS_DELETING {
                // The deletion the clock owes: a `DELETING` endpoint reaches the terminal
                // `DELETED` once its span has elapsed, whether or not anybody called.
                if let Some(due_at) = record.deleted_due_at {
                    if now >= due_at {
                        record.provisioning_status = PROVISIONING_STATUS_DELETED.to_string();
                    } else {
                        still_pending(due_at, &mut next);
                    }
                }
            }
        }
        self.pending_transition_due_at = next;
    }

    /// Record that a stored resource owes the clock a status transition at `due_at`.
    ///
    /// Called by every writer that stores a record in a non-terminal status; keeping the
    /// EARLIEST due instant is what lets [`materialise_due`](Self::materialise_due) skip
    /// the sweep on a request that owes nothing.
    pub fn schedule_transition(&mut self, due_at: SystemTime) {
        self.pending_transition_due_at = Some(
            self.pending_transition_due_at
                .map_or(due_at, |earliest| earliest.min(due_at)),
        );
    }

    /// Whether `count` stored resources of one kind is already this emulator's limit.
    ///
    /// The caller answers the modelled `ServiceQuotaExceededException` when it is; see
    /// [`MAX_RESOURCES_PER_KIND`] for why there is a limit at all.
    pub fn at_resource_limit(count: usize) -> bool {
        count >= MAX_RESOURCES_PER_KIND
    }

    /// Register an EC2 instance the account has — the seam
    /// [`seed_account_instances`] fills and a test adds to.
    pub fn register_account_instance(
        &mut self,
        instance_id: impl Into<String>,
        availability_zone: impl Into<String>,
    ) -> Option<String> {
        self.account_instances
            .insert(instance_id.into(), availability_zone.into())
    }

    /// Register a directory the account has, in `stage` — the seam
    /// [`seed_account_directories`] fills and a test adds to.
    pub fn register_account_directory(
        &mut self,
        directory_id: impl Into<String>,
        stage: impl Into<String>,
    ) -> Option<String> {
        self.account_directories
            .insert(directory_id.into(), stage.into())
    }

    /// Whether `directory_id` names a directory of this account that an identity provider
    /// can be registered for: one the account holds, in the `Active` stage
    /// ([`DIRECTORY_STAGE_ACTIVE`]).
    ///
    /// Every other outcome — no directory of that id, or one in any other stage — is the
    /// one refusal, so a caller is told which id was refused and nothing else.
    pub fn resolves_active_directory(&self, directory_id: &str) -> bool {
        self.account_directories
            .get(directory_id)
            .is_some_and(|stage| stage == DIRECTORY_STAGE_ACTIVE)
    }

    /// Whether the caller may be shown to own `instance_id`: true exactly when the
    /// account's EC2 fleet holds it.
    ///
    /// The gate every operation that acts on an instance runs BEFORE it looks anything
    /// else up, so a refusal discloses nothing about this service's own resources.
    pub fn authorizes_instance(&self, instance_id: &str) -> bool {
        self.account_instances.contains_key(instance_id)
    }

    /// Whether any registration in state is for the directory `key` names.
    ///
    /// `AssociateUser` carries no `Product`, so the registration it associates through is
    /// addressed by directory alone: a directory with a registration for ANY product is
    /// registered for the purposes of a user association.
    pub fn provider_key_is_registered(&self, key: &str) -> bool {
        self.identity_provider_index
            .keys()
            .any(|(indexed, _product)| indexed == key)
    }

    /// The user associations in association order.
    pub fn instance_users_in_order(&self) -> Vec<InstanceUserRecord> {
        self.instance_user_order
            .iter()
            .filter_map(|arn| self.instance_users.get(arn).cloned())
            .collect()
    }

    /// Resolve a user association from the two ways `DisassociateUser` can address one:
    /// its `InstanceUserArn`, or the `(IdentityProvider, InstanceId, Username)` triple it
    /// was associated with, optionally narrowed by `Domain`.
    ///
    /// `None` when state holds no such association — the caller decides how to refuse,
    /// because this surface fails CLOSED rather than reporting a not-found.
    pub fn resolve_instance_user(
        &self,
        instance_user_arn: Option<&str>,
        identity_provider: Option<&IdentityProvider>,
        instance_id: Option<&str>,
        username: Option<&str>,
        domain: Option<&str>,
    ) -> Option<&InstanceUserRecord> {
        let record = if let Some(arn) = instance_user_arn {
            self.instance_users.get(arn)?
        } else {
            let key = (
                provider_key(identity_provider?).ok()?,
                instance_id?.to_string(),
                username?.to_string(),
            );
            let arn = self.instance_user_index.get(&key)?;
            self.instance_users.get(arn)?
        };
        // `Domain` narrows the addressing rather than being ignored: an association from
        // another domain is not the one the caller named.
        match domain.filter(|d| !d.is_empty()) {
            Some(domain) if record.domain.as_deref() != Some(domain) => None,
            _ => Some(record),
        }
    }

    /// Remove one user association from the store and from every index that reaches it.
    pub fn remove_instance_user(&mut self, arn: &str) -> Option<InstanceUserRecord> {
        let record = self.instance_users.remove(arn)?;
        self.instance_user_order.retain(|a| a != arn);
        self.instance_user_index.retain(|_, indexed| indexed != arn);
        self.tags.remove(arn);
        Some(record)
    }

    /// The license server endpoints in creation order.
    pub fn license_server_endpoints_in_order(&self) -> Vec<LicenseServerEndpointRecord> {
        self.license_server_endpoint_order
            .iter()
            .filter_map(|arn| self.license_server_endpoints.get(arn).cloned())
            .collect()
    }

    /// How many LIVE license server endpoints the account holds: an endpoint whose
    /// deletion was accepted ([`endpoint_deletion_started`]) is still reported, but it no
    /// longer occupies a slot in the account's quota.
    pub fn live_license_server_endpoint_count(&self) -> usize {
        self.license_server_endpoints
            .values()
            .filter(|record| !endpoint_deletion_started(&record.provisioning_status))
            .count()
    }

    /// Accept the deletion of one license server endpoint: it enters `DELETING` now and
    /// owes the clock the terminal `DELETED` at [`ENDPOINT_DELETION_SPAN`].
    ///
    /// Deletion is ASYNCHRONOUS — that is what `LicenseServerEndpointProvisioningStatus`
    /// models `DELETING` and `DELETED` for — so the record is NOT dropped here: a caller
    /// polling `ListLicenseServerEndpoints` keeps seeing the endpoint in `DELETING` and
    /// then in `DELETED`, instead of it vanishing between two reads. What the deletion
    /// does take away immediately is everything that made the endpoint a LIVE resource:
    /// the `(IdentityProviderArn, ServerType)` index entry, so a create for the same pair
    /// mints a new endpoint rather than resolving this one, and the tags, so the tagging
    /// surface no longer resolves it.
    ///
    /// Returns the record as the deletion left it (`DELETING`), or `None` when the ARN
    /// names no stored endpoint. An endpoint whose deletion was ALREADY accepted is
    /// returned unchanged, which is what this operation's `@idempotent` trait asks for:
    /// a repeat delete neither restarts the deletion nor moves the terminal status back.
    pub fn begin_license_server_endpoint_deletion(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<LicenseServerEndpointRecord> {
        let record = self.license_server_endpoints.get_mut(arn)?;
        if endpoint_deletion_started(&record.provisioning_status) {
            return Some(record.clone());
        }
        record.provisioning_status = PROVISIONING_STATUS_DELETING.to_string();
        record.deletion_started_at = Some(now);
        let due_at = now + ENDPOINT_DELETION_SPAN;
        record.deleted_due_at = Some(due_at);
        let deleting = record.clone();
        self.license_server_endpoint_index
            .retain(|_, indexed| indexed != arn);
        self.tags.remove(arn);
        // The `DELETING` -> `DELETED` transition the clock now owes.
        self.schedule_transition(due_at);
        self.prune_deleted_license_server_endpoints();
        Some(deleting)
    }

    /// Drop the endpoints whose deletion was accepted EARLIEST, so no more than
    /// [`MAX_DELETED_ENDPOINTS_RETAINED`] deleted endpoints are retained.
    fn prune_deleted_license_server_endpoints(&mut self) {
        let mut deleted: Vec<(SystemTime, String)> = self
            .license_server_endpoints
            .values()
            .filter(|record| endpoint_deletion_started(&record.provisioning_status))
            .map(|record| {
                (
                    record.deletion_started_at.unwrap_or(record.created_at),
                    record.arn.clone(),
                )
            })
            .collect();
        if deleted.len() <= MAX_DELETED_ENDPOINTS_RETAINED {
            return;
        }
        deleted.sort();
        let excess = deleted.len() - MAX_DELETED_ENDPOINTS_RETAINED;
        for (_, arn) in deleted.into_iter().take(excess) {
            self.remove_license_server_endpoint(&arn);
        }
    }

    /// Remove one license server endpoint from the store, the order it is listed in and
    /// the `(provider, server type)` index that reaches it.
    ///
    /// This is the FINAL drop, which stops reporting the endpoint altogether; a deletion
    /// the caller asked for goes through
    /// [`begin_license_server_endpoint_deletion`](Self::begin_license_server_endpoint_deletion)
    /// instead, so the statuses the model defines for it stay observable.
    pub fn remove_license_server_endpoint(
        &mut self,
        arn: &str,
    ) -> Option<LicenseServerEndpointRecord> {
        let record = self.license_server_endpoints.remove(arn)?;
        self.license_server_endpoint_order.retain(|a| a != arn);
        self.license_server_endpoint_index
            .retain(|_, indexed| indexed != arn);
        self.tags.remove(arn);
        Some(record)
    }

    /// Whether `arn` names a resource THIS service holds — the one resolution the
    /// tagging surface runs, whose `ResourceArn` may name any of this service's
    /// resource kinds.
    ///
    /// A license server endpoint whose deletion was accepted is NOT one: it is still
    /// reported in its `DELETING`/`DELETED` status, but it is on its way out and carries
    /// no tags any more, so the tagging surface resolves it exactly as it resolves an
    /// endpoint that is gone.
    pub fn holds_resource(&self, arn: &str) -> bool {
        self.identity_providers.contains_key(arn)
            || self.product_users.contains_key(arn)
            || self.instance_users.contains_key(arn)
            || self
                .license_server_endpoints
                .get(arn)
                .is_some_and(|record| !endpoint_deletion_started(&record.provisioning_status))
    }

    /// Merge `tags` into the tags held against `arn`.
    ///
    /// Every tag the request carries has to be a legal tag ([`validate_tags`]) and the
    /// merged set is bounded by [`MAX_TAGS_PER_RESOURCE`]: a request that breaks either
    /// rule is refused WHOLE, leaving the stored map exactly as it was, so neither an
    /// illegal tag nor unbounded growth can reach the store. A key the resource already
    /// carries is overwritten and does not count as growth, which is also what the
    /// operation's `@idempotent` trait asks for.
    pub fn merge_tags(
        &mut self,
        arn: &str,
        tags: &HashMap<String, String>,
    ) -> Result<(), StateError> {
        // Whether a tag is legal is a fact about the request alone, so it is settled
        // before the stored set is even read.
        validate_tags(tags)?;
        let merged = match self.tags.get(arn) {
            Some(held) => {
                held.len() + tags.keys().filter(|key| !held.contains_key(*key)).count()
            }
            None => tags.len(),
        };
        if merged > MAX_TAGS_PER_RESOURCE {
            return Err(StateError::Validation(format!(
                "the resource would carry {merged} tags; a resource carries at most \
                 {MAX_TAGS_PER_RESOURCE}"
            )));
        }
        let held = self.tags.entry(arn.to_string()).or_default();
        for (key, value) in tags {
            held.insert(key.clone(), value.clone());
        }
        Ok(())
    }

    /// The registrations in registration order.
    pub fn identity_providers_in_order(&self) -> Vec<IdentityProviderRecord> {
        self.identity_provider_order
            .iter()
            .filter_map(|arn| self.identity_providers.get(arn).cloned())
            .collect()
    }

    /// The instances in the order they entered the store.
    pub fn instances_in_order(&self) -> Vec<InstanceRecord> {
        self.instance_order
            .iter()
            .filter_map(|id| self.instances.get(id).cloned())
            .collect()
    }

    /// Record an instance, keeping the list order the order of first appearance: a
    /// re-recorded instance updates its record in place rather than moving in the list.
    ///
    /// Every writer that brings an instance into the service's view goes through here, so
    /// `instances` and `instance_order` cannot disagree.
    pub fn upsert_instance(&mut self, record: InstanceRecord) {
        let id = record.instance_id.clone();
        if self.instances.insert(id.clone(), record).is_none() {
            self.instance_order.push(id);
        }
    }

    /// Remove an instance from the store and from the list order.
    pub fn remove_instance(&mut self, instance_id: &str) -> Option<InstanceRecord> {
        let record = self.instances.remove(instance_id)?;
        self.instance_order.retain(|id| id != instance_id);
        Some(record)
    }

    /// Recompute what the service's view says about one instance, from the associations
    /// that still name it — or drop it from the view when none does.
    ///
    /// This is the writer [`InstanceRecord`] documents: an instance is not created by any
    /// API of this service, it enters the view because a user was associated with it and
    /// leaves with the last association that names it. Everything reported is a FUNCTION
    /// of the state at the call: the products are the ones the naming associations'
    /// DIRECTORIES are registered for (`AssociateUser` carries no `Product`, so they are
    /// read from the registrations rather than taken from the request), so a product stops
    /// being reported as soon as the last association or registration behind it is gone.
    /// Nothing else is recorded: no status check has been performed, so none is reported.
    pub fn refresh_instance_view(&mut self, instance_id: &str) {
        let naming: Vec<InstanceUserRecord> = self
            .instance_user_order
            .iter()
            .filter_map(|arn| self.instance_users.get(arn))
            .filter(|association| association.instance_id == instance_id)
            .cloned()
            .collect();
        let Some(first) = naming.first() else {
            self.remove_instance(instance_id);
            return;
        };

        let mut products: Vec<String> = Vec::new();
        for association in &naming {
            for product in self.products_registered_for(&association.provider_key) {
                if !products.contains(&product) {
                    products.push(product);
                }
            }
        }
        let last_status_check_at = self
            .instances
            .get(instance_id)
            .and_then(|record| record.last_status_check_at);
        self.upsert_instance(InstanceRecord {
            instance_id: instance_id.to_string(),
            status: STATUS_ACTIVE.to_string(),
            products,
            status_message: None,
            owner_account_id: ::engine::emulator_account_id().to_string(),
            identity_provider: Some(first.identity_provider.clone()),
            last_status_check_at,
        });
    }

    /// Recompute the view of every instance an association names: what a registration's
    /// removal changes is which PRODUCTS those instances provide.
    pub fn refresh_every_instance_view(&mut self) {
        let instances: Vec<String> = self.instance_order.clone();
        for instance_id in instances {
            self.refresh_instance_view(&instance_id);
        }
    }

    /// The products the directory `key` names is registered for, in registration order.
    pub fn products_registered_for(&self, key: &str) -> Vec<String> {
        self.identity_provider_order
            .iter()
            .filter_map(|arn| self.identity_providers.get(arn))
            .filter(|record| provider_key(&record.identity_provider).is_ok_and(|k| k == key))
            .map(|record| record.product.clone())
            .collect()
    }

    /// The product users of one registration (optionally one product), in subscription order.
    pub fn product_users_in_order(
        &self,
        identity_provider_arn: &str,
        product: Option<&str>,
    ) -> Vec<ProductUserRecord> {
        self.product_user_order
            .iter()
            .filter_map(|arn| self.product_users.get(arn))
            .filter(|record| record.identity_provider_arn == identity_provider_arn)
            .filter(|record| product.is_none_or(|p| record.product == p))
            .cloned()
            .collect()
    }

    /// Resolve a registration from the two ways every operation can address one: its
    /// ARN, or the `(IdentityProvider, Product)` pair it was registered with.
    ///
    /// `ValidationException` when neither addressing is complete, `ResourceNotFoundException`
    /// when the addressed registration is not registered.
    pub fn resolve_identity_provider(
        &self,
        arn: Option<&str>,
        identity_provider: Option<&IdentityProvider>,
        product: Option<&str>,
    ) -> Result<&IdentityProviderRecord, StateError> {
        if let Some(arn) = arn {
            return self
                .identity_providers
                .get(arn)
                .ok_or(StateError::ProviderNotRegistered);
        }
        let (Some(identity_provider), Some(product)) = (identity_provider, product) else {
            return Err(StateError::Validation(members_must_be_provided(&[
                "IdentityProvider",
                "Product",
            ])));
        };
        let key = provider_key(identity_provider)?;
        let arn = self
            .identity_provider_index
            .get(&(key, product.to_string()))
            .ok_or(StateError::ProviderNotRegistered)?;
        self.identity_providers
            .get(arn)
            .ok_or(StateError::ProviderNotRegistered)
    }

    /// Resolve a product user from the two ways `StopProductSubscription` can address
    /// one: its `ProductUserArn`, or the `(Username, IdentityProvider, Product)` triple
    /// it was subscribed with, optionally narrowed by `Domain`.
    ///
    /// `ValidationException` when neither addressing is complete, `ResourceNotFoundException`
    /// when no such subscription is in state.
    pub fn resolve_product_user(
        &self,
        product_user_arn: Option<&str>,
        username: Option<&str>,
        identity_provider: Option<&IdentityProvider>,
        product: Option<&str>,
        domain: Option<&str>,
    ) -> Result<&ProductUserRecord, StateError> {
        let record = if let Some(arn) = product_user_arn {
            self.product_users.get(arn).ok_or_else(|| {
                StateError::NotFound(format!("product user {arn} has no subscription"))
            })?
        } else {
            let (Some(username), Some(identity_provider), Some(product)) =
                (username, identity_provider, product)
            else {
                return Err(StateError::Validation(members_must_be_provided(&[
                    "Username",
                    "IdentityProvider",
                    "Product",
                ])));
            };
            let provider =
                self.resolve_identity_provider(None, Some(identity_provider), Some(product))?;
            let arn = self
                .product_user_index
                .get(&(
                    provider.arn.clone(),
                    product.to_string(),
                    username.to_string(),
                ))
                .ok_or_else(|| {
                    StateError::NotFound(format!(
                        "user {username} has no subscription to product {product}"
                    ))
                })?;
            self.product_users.get(arn).ok_or_else(|| {
                StateError::NotFound(format!("product user {arn} has no subscription"))
            })?
        };
        // `Domain` narrows the addressing rather than being ignored: a subscription from
        // another domain is not the one the caller named.
        if let Some(domain) = domain.filter(|d| !d.is_empty()) {
            if record.domain.as_deref() != Some(domain) {
                return Err(StateError::NotFound(format!(
                    "user {} has no subscription in domain {domain}",
                    record.username
                )));
            }
        }
        Ok(record)
    }

    /// Remove a registration, and with it the product users that were subscribed
    /// through it and the license server endpoints created for it (both are only
    /// reachable through their provider).
    ///
    /// The endpoints are DELETED the same asynchronous way `DeleteLicenseServerEndpoint`
    /// deletes one — they enter `DELETING` and reach `DELETED` when the clock owes it —
    /// because how an endpoint's deletion becomes observable does not depend on what
    /// asked for it.
    ///
    /// User ASSOCIATIONS are not removed: an association names a directory and no
    /// product (`AssociateUser` carries no `Product`), so it outlives the registration
    /// of any one product for that directory.
    pub fn remove_identity_provider(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<IdentityProviderRecord> {
        let record = self.identity_providers.remove(arn)?;
        self.identity_provider_order.retain(|a| a != arn);
        self.identity_provider_index
            .retain(|_, indexed| indexed != arn);
        let orphaned: Vec<String> = self
            .product_users
            .values()
            .filter(|user| user.identity_provider_arn == arn)
            .map(|user| user.arn.clone())
            .collect();
        for user_arn in orphaned {
            self.remove_product_user(&user_arn);
        }
        let orphaned: Vec<String> = self
            .license_server_endpoints
            .values()
            .filter(|endpoint| endpoint.identity_provider_arn == arn)
            .map(|endpoint| endpoint.arn.clone())
            .collect();
        for endpoint_arn in orphaned {
            self.begin_license_server_endpoint_deletion(&endpoint_arn, now);
        }
        // Which products the instances in view provide is read from the registrations, so
        // removing one changes those views.
        self.refresh_every_instance_view();
        self.tags.remove(arn);
        Some(record)
    }

    /// Remove one product user from the store and from every index that reaches it.
    pub fn remove_product_user(&mut self, arn: &str) -> Option<ProductUserRecord> {
        let record = self.product_users.remove(arn)?;
        self.product_user_order.retain(|a| a != arn);
        self.product_user_index.retain(|_, indexed| indexed != arn);
        self.tags.remove(arn);
        Some(record)
    }
}

/// The key a registration is indexed by, derived from the provider union.
///
/// `IdentityProvider` is a union whose only member is `ActiveDirectoryIdentityProvider`,
/// which identifies a directory either by `DirectoryId` (an AWS Managed directory) or by
/// the domain name in `ActiveDirectorySettings` (a self-managed one). One of the two must
/// be present, or the provider names no directory at all and nothing can be registered
/// for it — a combination the model cannot express, so it is refused here.
pub fn provider_key(identity_provider: &IdentityProvider) -> Result<String, StateError> {
    match identity_provider {
        IdentityProvider::ActiveDirectoryIdentityProvider(ad) => {
            if let Some(directory_id) = ad.directory_id.as_deref().filter(|id| !id.is_empty()) {
                return Ok(format!("directory/{directory_id}"));
            }
            let domain = ad
                .active_directory_settings
                .as_ref()
                .and_then(|settings| settings.domain_name.as_deref())
                .filter(|name| !name.is_empty());
            match domain {
                Some(domain) => Ok(format!("domain/{domain}")),
                None => Err(StateError::Validation(
                    "IdentityProvider.ActiveDirectoryIdentityProvider must specify either \
                     DirectoryId or ActiveDirectorySettings.DomainName"
                        .to_string(),
                )),
            }
        }
    }
}

/// The `DirectoryId` of a provider union, when it names one.
///
/// `ListIdentityProviders` documents `DirectoryId` as a filterable attribute, so the
/// value a filter compares against comes from here.
pub fn directory_id(identity_provider: &IdentityProvider) -> Option<&str> {
    match identity_provider {
        IdentityProvider::ActiveDirectoryIdentityProvider(ad) => ad.directory_id.as_deref(),
    }
}

/// Format `at` the way this service's date members carry an instant: ISO-8601 UTC at
/// whole-second precision, e.g. `2025-03-15T00:00:00Z` — the format the model's
/// `LicenseExpirationDate` documentation shows.
pub fn iso8601(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs() as i64)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// The settings a registration carries after an `UpdateSettings` payload is applied to
/// them: the subnets it had, plus `AddSubnets`, minus `RemoveSubnets`, with
/// `SecurityGroupId` replaced when the payload names one.
///
/// Two payloads cannot be applied at all, and both are refused as invalid rather than
/// half-applied:
///
/// * a subnet in BOTH `AddSubnets` and `RemoveSubnets` asks for two contradictory things
///   at once, so there is no resulting set;
/// * a payload that would leave no subnet at all contradicts `Settings$Subnets`, which
///   the model declares `@required` with `@length(min: 1)`.
///
/// Order is preserved and duplicates collapse, so the result is the SET the caller asked
/// for however the payload spelled it; removing a subnet the registration does not carry
/// is a no-op, which is what the operation's `@idempotent` trait asks for.
///
/// Every step is LINEAR in the lists involved, and each list is bounded by
/// [`MAX_SUBNETS_PER_UPDATE`] before any of it runs: this function is called with the
/// service-state lock held, so the caller must not be able to buy quadratic (or
/// unbounded) work under it with one request.
pub fn apply_update_settings(
    settings: &Settings,
    add_subnets: &[String],
    remove_subnets: &[String],
    security_group_id: Option<&str>,
) -> Result<Settings, StateError> {
    for (member, list) in [
        ("AddSubnets", add_subnets),
        ("RemoveSubnets", remove_subnets),
    ] {
        if list.len() > MAX_SUBNETS_PER_UPDATE {
            return Err(StateError::Validation(format!(
                "UpdateSettings.{member} names {} subnets; at most {MAX_SUBNETS_PER_UPDATE} \
                 may be named in one update",
                list.len()
            )));
        }
    }

    let removed: HashSet<&str> = remove_subnets.iter().map(String::as_str).collect();
    let overlapping: Vec<&str> = add_subnets
        .iter()
        .map(String::as_str)
        .filter(|subnet| removed.contains(subnet))
        .collect();
    if !overlapping.is_empty() {
        return Err(StateError::Validation(format!(
            "UpdateSettings.AddSubnets and UpdateSettings.RemoveSubnets both name {}; \
             a subnet cannot be added and removed by one update",
            overlapping.join(", ")
        )));
    }

    let mut subnets: Vec<String> = Vec::new();
    let mut kept: HashSet<&str> = HashSet::new();
    for subnet in settings.subnets.iter().chain(add_subnets) {
        if !removed.contains(subnet.as_str()) && kept.insert(subnet.as_str()) {
            subnets.push(subnet.clone());
        }
    }
    if subnets.is_empty() {
        return Err(StateError::Validation(
            "UpdateSettings would leave the registration with no subnet, and \
             Settings.Subnets requires at least one"
                .to_string(),
        ));
    }

    Ok(Settings {
        subnets,
        security_group_id: security_group_id
            .map(str::to_string)
            .unwrap_or_else(|| settings.security_group_id.clone()),
    })
}

/// The `IdentityProviderSummary` the model requires, built entirely from the stored
/// record — every member is what registration persisted.
pub fn identity_provider_summary(record: &IdentityProviderRecord) -> IdentityProviderSummary {
    IdentityProviderSummary {
        identity_provider: record.identity_provider.clone(),
        settings: record.settings.clone(),
        product: record.product.clone(),
        status: record.status.clone(),
        identity_provider_arn: Some(record.arn.clone()),
        failure_message: record.failure_message.clone(),
        owner_account_id: Some(record.owner_account_id.clone()),
    }
}

/// The `InstanceSummary` the model requires, built entirely from the stored record —
/// every member is what the writer persisted, and an absent one stays absent.
pub fn instance_summary(record: &InstanceRecord) -> InstanceSummary {
    InstanceSummary {
        instance_id: record.instance_id.clone(),
        status: record.status.clone(),
        products: record.products.clone(),
        last_status_check_date: record.last_status_check_at.map(iso8601),
        status_message: record.status_message.clone(),
        owner_account_id: Some(record.owner_account_id.clone()),
        identity_provider: record.identity_provider.clone(),
    }
}

/// The `ProductUserSummary` the model requires, built from the stored product user and
/// the provider union of the registration it was subscribed through.
pub fn product_user_summary(
    record: &ProductUserRecord,
    identity_provider: &IdentityProvider,
) -> ProductUserSummary {
    ProductUserSummary {
        username: record.username.clone(),
        product: record.product.clone(),
        identity_provider: identity_provider.clone(),
        status: record.status.clone(),
        product_user_arn: Some(record.arn.clone()),
        status_message: record.status_message.clone(),
        domain: record.domain.clone(),
        subscription_start_date: record.subscription_start_date.clone(),
        subscription_end_date: record.subscription_end_date.clone(),
        license_expiration_date: record.license_expiration_date.clone(),
    }
}

/// The region this request was signed for, or the emulator's default when the request
/// carries no usable SigV4 credential scope.
///
/// Values outside the ARN grammar's region segment (`[a-z0-9-.]{1,63}`) are ignored, so
/// a minted ARN always satisfies the model's `Arn` pattern.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 63
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        })
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// An ARN for one of this service's resources: `arn:aws:<signing name>:<region>:<account>:<kind>/<id>`.
pub fn resource_arn(signing_name: &str, region: &str, kind: &str, id: &str) -> String {
    ::engine::emulator_arn(signing_name, region, &format!("{kind}/{id}"))
}

/// The instant a registration created at `created_at` becomes `REGISTERED`.
pub fn registration_due_at(created_at: SystemTime) -> SystemTime {
    created_at + REGISTRATION_SPAN
}

/// The instant an association created at `created_at` becomes `ASSOCIATED`.
pub fn association_due_at(created_at: SystemTime) -> SystemTime {
    created_at + ASSOCIATION_SPAN
}

/// The `InstanceUserSummary` the model requires, built entirely from the stored
/// association — every member is what the association persisted.
pub fn instance_user_summary(record: &InstanceUserRecord) -> InstanceUserSummary {
    InstanceUserSummary {
        username: record.username.clone(),
        instance_id: record.instance_id.clone(),
        identity_provider: record.identity_provider.clone(),
        status: record.status.clone(),
        instance_user_arn: Some(record.arn.clone()),
        status_message: record.status_message.clone(),
        domain: record.domain.clone(),
        association_date: record.association_date.clone(),
        disassociation_date: record.disassociation_date.clone(),
    }
}

/// The instant a license server endpoint created at `created_at` becomes `PROVISIONED`.
pub fn endpoint_provisioned_due_at(created_at: SystemTime) -> SystemTime {
    created_at + ENDPOINT_PROVISIONING_SPAN
}

/// The network address of the license server endpoint `id`, in the regional form this
/// emulator mints for a service-owned endpoint: `<id>.<signing name>.<region>.amazonaws.com`.
pub fn endpoint_address(signing_name: &str, region: &str, id: &str) -> String {
    format!("{id}.{signing_name}.{region}.amazonaws.com")
}

/// The `LicenseServerEndpoint` the model describes, built entirely from the stored
/// record.
///
/// `ServerEndpoint` is reported only once the endpoint is `PROVISIONED` — there is no
/// address to connect to while it is still being provisioned. `LicenseServers` is left
/// absent rather than invented: this emulator provisions no license-server hosts, so
/// there are none to report. `StatusMessage` is only what the record carries.
pub fn license_server_endpoint(record: &LicenseServerEndpointRecord) -> LicenseServerEndpoint {
    let provisioned = record.provisioning_status == PROVISIONING_STATUS_PROVISIONED;
    LicenseServerEndpoint {
        identity_provider_arn: Some(record.identity_provider_arn.clone()),
        server_type: Some(record.server_type.clone()),
        server_endpoint: provisioned.then(|| ServerEndpoint {
            endpoint: Some(record.endpoint.clone()),
        }),
        status_message: record.status_message.clone(),
        license_server_endpoint_id: Some(record.id.clone()),
        license_server_endpoint_arn: Some(record.arn.clone()),
        license_server_endpoint_provisioning_status: Some(record.provisioning_status.clone()),
        license_servers: None,
        creation_time: Some(::aws_smithy_types::DateTime::from(record.created_at)),
    }
}

/// One page of `items`, plus the token that reaches the next one.
///
/// `MaxResults` is the page size; a `NextToken` is this service's opaque cursor, minted
/// by the previous page of the SAME list (`kind` names that list: the operation and
/// whatever selected it) and rejected otherwise — a token is not a client-chosen value,
/// so an unparseable or foreign one is a `ValidationException` rather than a silent
/// restart from the beginning. A page size below 1 would make no progress, so it is
/// refused too.
pub fn paginate<T: Clone>(
    items: &[T],
    kind: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<(Vec<T>, Option<String>), StateError> {
    let offset = match next_token {
        Some(token) => decode_page_token(kind, token)?,
        None => 0,
    };
    if offset > items.len() {
        return Err(StateError::Validation(
            "NextToken does not refer to a position in this list".to_string(),
        ));
    }
    let page_size = match max_results {
        Some(max) if max < 1 => {
            return Err(StateError::Validation(format!(
                "MaxResults must be at least 1, got {max}"
            )))
        }
        Some(max) => max as usize,
        None => items.len(),
    };
    let end = offset.saturating_add(page_size).min(items.len());
    let page = items[offset..end].to_vec();
    let token = if end < items.len() {
        Some(encode_page_token(kind, end))
    } else {
        None
    };
    Ok((page, token))
}

/// This service's opaque pagination cursor: the list it belongs to and the offset it
/// resumes at.
///
/// Hex is an encoding, not a secret — a client can decode one. What makes a cursor
/// trustworthy here is the `kind` check in [`decode_page_token`]: a token is only honoured
/// by the list that minted it, so a forged or stale one is refused rather than applied to
/// a different list.
fn encode_page_token(kind: &str, offset: usize) -> String {
    ::hex::encode(format!("{kind}:{offset}"))
}

/// Decode a cursor minted by [`encode_page_token`] for the same list.
///
/// The offset is split off the END, so a list identity may itself contain `:`.
fn decode_page_token(kind: &str, token: &str) -> Result<usize, StateError> {
    let invalid = || StateError::Validation("NextToken is not a valid pagination token".to_string());
    let decoded = ::hex::decode(token).map_err(|_| invalid())?;
    let decoded = String::from_utf8(decoded).map_err(|_| invalid())?;
    let (token_kind, offset) = decoded.rsplit_once(':').ok_or_else(invalid)?;
    if token_kind != kind {
        return Err(invalid());
    }
    offset.parse::<usize>().map_err(|_| invalid())
}

/// A deterministic rendering of a filter set, for the list identity a pagination cursor
/// is minted against: a cursor is a position in ONE list, so it must not be accepted for
/// a list selected by different filters.
///
/// Every component is LENGTH-PREFIXED, so no two different filter sets can render to the
/// same signature — a separator appearing inside an attribute, operation or value cannot
/// make one set look like another.
pub fn filter_signature(filters: &[Filter]) -> String {
    let mut signature = String::new();
    for filter in filters {
        for part in [
            filter.attribute.as_deref().unwrap_or_default(),
            filter.operation.as_deref().unwrap_or_default(),
            filter.value.as_deref().unwrap_or_default(),
        ] {
            signature.push_str(&format!("{}.{part}", part.len()));
        }
    }
    signature
}

/// True when `attributes` satisfies every filter in `filters`.
///
/// `Filter.Attribute` names one of the attributes the calling operation's model
/// documents as filterable (`Product`/`DirectoryId` for `ListIdentityProviders`,
/// `Status`/`Username`/`Domain` for `ListProductSubscriptions`, `IdentityProviderArn`
/// for `ListLicenseServerEndpoints`): the caller passes those
/// as `(name, value)` pairs, matched EXACTLY as the documentation spells them, so a typo
/// is refused rather than silently honoured. `Filter.Operation` is the kind of search —
/// the model documents it only as "the type of search (For example, eq, geq, leq)", so
/// the comparisons that are meaningful for these string attributes are accepted under
/// either the documented lowercase spelling or its PascalCase name, and anything else is
/// refused; an absent operation is equality.
///
/// Whether a filter is WELL FORMED does not depend on what the store holds, so
/// [`validate_filters`] is what a list operation calls first, before it looks at a single
/// record: otherwise an empty (or fully filtered-out) store would answer 200 for a
/// request this service refuses.
pub fn filters_match(
    filters: &[Filter],
    attributes: &[(&str, Option<&str>)],
) -> Result<bool, StateError> {
    for filter in filters {
        let (index, operation) = parse_filter(filter, &attribute_names(attributes))?;
        let actual = attributes[index].1.unwrap_or_default();
        let wanted = filter.value.as_deref().unwrap_or_default();
        let matched = match operation {
            FilterOperation::Equals => actual == wanted,
            FilterOperation::NotEquals => actual != wanted,
            FilterOperation::Contains => actual.contains(wanted),
            FilterOperation::BeginsWith => actual.starts_with(wanted),
        };
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Refuse a filter set that names an attribute this operation does not expose, or a
/// comparison this service does not implement — independently of the records in state.
pub fn validate_filters(filters: &[Filter], attribute_names: &[&str]) -> Result<(), StateError> {
    for filter in filters {
        parse_filter(filter, attribute_names)?;
    }
    Ok(())
}

/// The comparison a `Filter.Operation` names.
#[derive(Debug, Clone, Copy)]
enum FilterOperation {
    Equals,
    NotEquals,
    Contains,
    BeginsWith,
}

/// The names of the attributes a list operation exposes, in the order it passed them.
fn attribute_names<'a>(attributes: &[(&'a str, Option<&str>)]) -> Vec<&'a str> {
    attributes.iter().map(|(name, _)| *name).collect()
}

/// Resolve one filter against the attributes a list operation exposes: which attribute it
/// names (by index) and which comparison it asks for.
fn parse_filter(
    filter: &Filter,
    attribute_names: &[&str],
) -> Result<(usize, FilterOperation), StateError> {
    let Some(attribute) = filter.attribute.as_deref() else {
        return Err(StateError::Validation(
            "Filter.Attribute is required".to_string(),
        ));
    };
    let index = attribute_names
        .iter()
        .position(|name| *name == attribute)
        .ok_or_else(|| {
            StateError::Validation(format!(
                "Filter.Attribute {attribute} is not filterable; supported attributes: {}",
                attribute_names.join(", ")
            ))
        })?;
    let operation = filter.operation.as_deref().unwrap_or("Equals");
    let operation = match operation.to_ascii_lowercase().as_str() {
        "equals" | "eq" => FilterOperation::Equals,
        "notequals" | "ne" => FilterOperation::NotEquals,
        "contains" => FilterOperation::Contains,
        "beginswith" => FilterOperation::BeginsWith,
        _ => {
            return Err(StateError::Validation(format!(
                "Filter.Operation {operation} is not supported; supported operations: \
                 Equals, NotEquals, Contains, BeginsWith"
            )))
        }
    };
    Ok((index, operation))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>)
}
