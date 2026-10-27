// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sesv2-simpleemailservice-v2 service.
//!
//! Everything the implemented operations answer with is computed from the records
//! below: the email-identity table, the contact-list table, the configuration-set
//! table, the suppression-list table, the multi-region-endpoint table, the
//! outbound-message log, the per-account send counters and the account-level
//! attributes. Nothing here is per-request scratch — the whole struct lives in the one
//! shared engine (see [`service_state`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, SystemTime};

/// How long after `CreateEmailIdentity` an identity's `PENDING` -> `SUCCESS`
/// verification transition becomes due.
///
/// Real SES completes verification only once it observes something it cannot
/// observe here (the recipient following the link in the verification email, or
/// the Easy DKIM CNAME records appearing in the domain's DNS), so the span is
/// CHOSEN, not documented: it is the emulator's convention for a transition the
/// clock owes, long enough that a caller which reads straight back still sees the
/// transient `PENDING` state and the state errors that go with it. The due instant
/// is stored on the record; nothing ever sleeps for it.
pub const VERIFICATION_SETTLE: Duration = Duration::from_millis(500);

/// How long after a certificate association is created its `PROVISIONING` ->
/// `ACTIVE` transition becomes due, and how long after it is removed its
/// `DEPROVISIONING` clean-up is due.
///
/// Both spans are CHOSEN, not documented: real SES prepares the certificate for
/// signing (and later cleans the association up) on its own schedule, which nothing
/// here can observe. The span is long enough that a caller which reads straight back
/// still sees the transient state and the state rules that go with it. The due
/// instant is stored on the record; nothing ever sleeps for it.
pub const CERTIFICATE_SETTLE: Duration = Duration::from_millis(500);

/// Certificate associations one email identity may hold.
///
/// A CHOSEN bound, not an AWS one: a domain identity takes one association per
/// address in the domain, so a long-lived emulator must not grow without limit while
/// serving them. It is far above what a client signing from a domain's addresses
/// needs.
pub const MAX_CERTIFICATES_PER_IDENTITY: usize = 1_000;

/// Easy DKIM publishes three CNAME tokens for a domain identity.
///
/// From the `DkimAttributes$Tokens` documentation: "this object contains a set of
/// unique strings that you use to create a set of CNAME records that you add to the
/// DNS configuration for your domain".
const EASY_DKIM_TOKEN_COUNT: usize = 3;

/// Length of one Easy DKIM token, in characters.
const EASY_DKIM_TOKEN_LEN: usize = 32;

/// The window the account's sending quota is measured over: SES states the quota
/// as `Max24HourSend`, "the maximum number of emails you can send in a 24-hour
/// period".
pub const SEND_QUOTA_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// Verified identities an account may hold: SES's documented quota is 10,000
/// identities per account per Region.
pub const MAX_IDENTITIES: usize = 10_000;

/// How long after an account asks for production access the decision on that
/// request becomes due.
///
/// Real SES decides the request by a review nothing here can observe (a support
/// case worked by a human), so the span is CHOSEN, not documented: it is the
/// emulator's convention for a transition the clock owes, long enough that a caller
/// which reads straight back still sees the request under review and the rules that
/// go with it. The due instant is stored on the account record; nothing ever sleeps
/// for it.
pub const PRODUCTION_ACCESS_REVIEW_SETTLE: Duration = Duration::from_millis(500);

/// Entries the outbound-message log retains, oldest evicted first.
///
/// A CHOSEN bound, not an AWS one: the log is what lets a caller read back what it
/// sent, and a long-lived emulator must not grow without limit while serving
/// requests. It is well above the number of messages the sending quota lets a
/// caller accept in one window.
pub const MAX_SENT_MESSAGES: usize = 1_000;

/// Characters of a message body the log retains, the rest truncated.
///
/// Also CHOSEN: the body is caller-sized (a raw MIME document has no modelled
/// length bound), so the retained log is bounded by entries times this.
pub const MAX_STORED_BODY_CHARS: usize = 8 * 1024;

/// Contact lists the account may hold.
///
/// A CHOSEN bound, not an AWS one: a contact list is created by a request, so a
/// long-lived emulator must not grow without limit while serving them. It is far
/// above what a client exercising the contact API needs, and `CreateContactList`
/// refuses past it with the `LimitExceededException` its model binds.
pub const MAX_CONTACT_LISTS: usize = 1_000;

/// Contacts one contact list may hold.
///
/// CHOSEN for the same reason, and for the same growth: one record per accepted
/// `CreateContact`.
pub const MAX_CONTACTS_PER_LIST: usize = 10_000;

/// Addresses the account's suppression lists may hold, across the account-level list
/// and every tenant's own.
///
/// CHOSEN for the same reason, and for the same growth: one record per accepted
/// suppression write, under a key the request's own address decides.
/// [`ComAmazonawsSesv2SimpleemailserviceV2State::suppress_destination`] is the one
/// way an entry enters the table and it refuses past this bound, so no sequence of
/// requests can grow the table without limit. It is well above what a client
/// exercising the suppression API needs.
pub const MAX_SUPPRESSED_DESTINATIONS: usize = 10_000;

/// How long after `CreateMultiRegionEndpoint` an endpoint's `CREATING` -> `READY`
/// transition becomes due.
///
/// The create call answers with the non-terminal `CREATING` ("the resource is being
/// provisioned") and the resource becomes `READY` once provisioning finishes, which
/// nothing here performs, so the span is CHOSEN, not documented: it is long enough
/// that a caller reading straight back still sees the transient state, and the due
/// instant is stored on the record so nothing ever sleeps for it.
pub const ENDPOINT_PROVISIONING_SETTLE: Duration = Duration::from_millis(500);

/// How long after `DeleteMultiRegionEndpoint` the deletion that call started is
/// complete.
///
/// The delete answers with the non-terminal `DELETING` ("the resource is being deleted
/// as requested"), so the endpoint is still there, being torn down, until this span
/// falls due: a read reports it as `DELETING`, a second delete collides with the one
/// already in flight, and once the span falls due the endpoint and its name are gone.
/// CHOSEN — AWS documents no duration for a tear-down — but deliberately not
/// sub-second like [`ENDPOINT_PROVISIONING_SETTLE`], which covers a resource becoming
/// usable. What this one covers is a refusal: the collision is a 5xx, so a client's
/// SDK retries the same delete for seconds before surfacing it, and a tear-down that
/// finished inside that retry chain would answer the retrying client `404` for a
/// resource it is still deleting — which is not what the service does. Stored as a due
/// instant, so nothing ever sleeps for it, and stated here ONCE: this constant is the
/// emulator's single knob for the span, and every call and test reads it from here.
pub const ENDPOINT_DELETION_SETTLE: Duration = Duration::from_secs(10);

/// The lifecycle status an endpoint whose deletion is still running is in, as AWS
/// names it when it refuses a second delete of that endpoint.
pub const DEPROVISIONING_STATUS: &str = "DEPROVISIONING";

/// Multi-region endpoints the store holds, across every primary region.
///
/// A CHOSEN bound, not an AWS one (SES states its quota per account per Region, and
/// this emulator serves one account): an endpoint is created by a request, so a
/// long-lived emulator must not grow without limit while serving them, and the bound
/// is on the WHOLE table — the region a record is keyed under comes from the request,
/// so a per-region bound would not bound anything. `CreateMultiRegionEndpoint` refuses
/// past it with the `LimitExceededException` its model binds.
pub const MAX_MULTI_REGION_ENDPOINTS: usize = 1_000;

/// Tags one resource may carry.
///
/// From the model's `Tag` documentation: "A resource can have as many as 50 tags."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The resource TYPE the ARN `resource_arn` names and the id it addresses within that
/// type, or `None` when the value is not a resource ARN of this service naming a
/// resource at all.
///
/// SES writes a resource in the resource field of its ARN as `<type>/<id>`, so a value
/// of another vendor, or one carrying no id (a bare type names no resource), addresses
/// nothing this service can resolve — which is a bad REQUEST, refused on the input,
/// rather than a resource that happens to be missing. The ARN's six fields come from
/// the shared caller-supplied-ARN parse, which is where what an ARN of this vendor is —
/// fields, partition, the resource field's own colons — is decided.
pub fn ses_resource_of(resource_arn: &str) -> Option<(&str, &str)> {
    let resource_type = ::service_common::arn::parse_of(resource_arn, ARN_NAMESPACE)?.resource_type();
    let resource_id = ::service_common::arn::resource_id_of_type(resource_arn, resource_type)
        .filter(|resource_id| !resource_id.is_empty())?;
    Some((resource_type, resource_id))
}

/// How the tagging surface words `NotFoundException` for a `ResourceArn` that names
/// no resource the account holds in this Region.
///
/// One wording for every operation that resolves a `ResourceArn` to a tag set, so a
/// client sees the same sentence whichever of them first tells it the resource is
/// gone. The ARN is the only variable part, taken from the request.
///
/// Neither the model nor a recording of this surface states the sentence, so it is
/// this service's own sentence for a named resource it does not hold — the form
/// [`tenant_not_found`] and [`reputation_entity_not_found`] state for the resources
/// they name — with the resource written as the ARN the request addressed it by,
/// which is the only name this operation is given.
pub fn tagged_resource_not_found(resource_arn: &str) -> String {
    format!("Resource {resource_arn} does not exist.")
}

/// Tenants the account may hold.
///
/// CHOSEN for the same reason as [`MAX_CONTACT_LISTS`]: one record per accepted
/// `CreateTenant`, so a long-lived emulator needs a ceiling. `CreateTenant`
/// refuses past it with the `LimitExceededException` its model binds.
pub const MAX_TENANTS: usize = 1_000;

/// Tenant-resource associations the store holds, across every tenant and resource.
///
/// A CHOSEN bound, not an AWS one: an association is created by a request and the
/// resource it is keyed under is a caller-supplied ARN, so a long-lived emulator must
/// not grow without limit while serving them, and the bound is on the WHOLE table —
/// a per-tenant bound would still leave the table growing with the tenant count.
/// `CreateTenantResourceAssociation` refuses past it with the `BadRequestException`
/// its model binds, which is the only limit-shaped error it binds at all.
pub const MAX_TENANT_RESOURCE_ASSOCIATIONS: usize = 10_000;

/// The longest `TenantName` SES accepts: its documentation states the name "can
/// contain up to 64 alphanumeric characters, including letters, numbers, hyphens
/// (-) and underscores (_) only". The model itself binds only the lower bound
/// (length 1..), so the upper bound and the character set are enforced by the
/// handler.
pub const MAX_TENANT_NAME_CHARS: usize = 64;

/// The ARN namespace this service stamps on a resource ARN it mints: SES's ARN
/// namespace is `ses`.
pub const ARN_NAMESPACE: &str = "ses";

/// The region an ARN carries when the request carries no SigV4 credential scope
/// naming one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `SendingStatus$ENABLED`: "sending is allowed for this entity".
pub const SENDING_STATUS_ENABLED: &str = "ENABLED";

/// `SendingStatus$REINSTATED`: "sending is allowed even if there are active
/// reputation findings".
pub const SENDING_STATUS_REINSTATED: &str = "REINSTATED";

/// `SendingStatus$DISABLED`: "sending is prevented for this entity".
pub const SENDING_STATUS_DISABLED: &str = "DISABLED";

/// Every `SendingStatus` the model binds, for the operations that take one as a
/// plain string and so refuse an unmodelled value themselves.
pub const SENDING_STATUSES: [&str; 3] = [
    SENDING_STATUS_ENABLED,
    SENDING_STATUS_REINSTATED,
    SENDING_STATUS_DISABLED,
];

/// The `SendingStatus` a freshly created tenant carries: sending is allowed for a
/// new tenant, and `ENABLED` is terminal — the clock owes no transition out of it.
pub const TENANT_SENDING_STATUS_CREATED: &str = SENDING_STATUS_ENABLED;

/// The one `ReputationEntityType` the model binds: "currently, only `RESOURCE` type
/// entities are supported, which represent resources in your Amazon SES account that
/// have reputation tracking capabilities".
pub const REPUTATION_ENTITY_TYPE_RESOURCE: &str = "RESOURCE";

/// Every `ReputationEntityFilterKey` the model binds, in the order the enum lists
/// them: a listing filter is a map keyed by one of these.
pub const REPUTATION_ENTITY_FILTER_KEYS: [&str; 4] = [
    "ENTITY_TYPE",
    "REPUTATION_IMPACT",
    "SENDING_STATUS",
    "ENTITY_REFERENCE_PREFIX",
];

/// Every `RecommendationImpact` the model binds, which is what a reputation entity's
/// `ReputationImpact` is one of.
pub const RECOMMENDATION_IMPACTS: [&str; 2] = ["LOW", "HIGH"];

/// The `StatusRecord$Status` of every entity's Amazon-SES-managed status record.
///
/// SES moves that status itself, on a reputation finding it raised against the
/// entity; nothing here raises one — no finding is observed or recorded anywhere in
/// this service — so the Amazon-SES-managed status of every entity permits sending
/// and the aggregate is decided by the customer-managed status alone.
pub const AWS_SES_MANAGED_SENDING_STATUS: &str = SENDING_STATUS_ENABLED;

/// The aggregate `SendingStatus` of an entity whose two status records read
/// `customer_managed` and `aws_ses_managed`.
///
/// `ReputationEntity$SendingStatusAggregate` states the rule: "if either the
/// customer-managed status or the Amazon Web Services Amazon SES-managed status is
/// DISABLED, the aggregate status will be DISABLED and the entity will not be allowed
/// to send emails. When the customer-managed status is set to REINSTATED, the entity
/// can continue sending even if there are active reputation findings, provided the
/// Amazon Web Services Amazon SES-managed status also permits sending."
pub fn aggregate_sending_status(customer_managed: &str, aws_ses_managed: &str) -> String {
    let statuses = [customer_managed, aws_ses_managed];
    if statuses.contains(&SENDING_STATUS_DISABLED) {
        return SENDING_STATUS_DISABLED.to_string();
    }
    if statuses.contains(&SENDING_STATUS_REINSTATED) {
        return SENDING_STATUS_REINSTATED.to_string();
    }
    SENDING_STATUS_ENABLED.to_string()
}

/// How this service words the refusal of a reputation entity reference that names no
/// resource this account holds.
///
/// A reputation entity is not created by a request of its own, so for a write the
/// reference it carries is invalid input rather than a missing resource of that write's
/// own making — which is why the writes model `BadRequestException` and no
/// `NotFoundException` at all. `GetReputationEntity` does model one, and reports the
/// missing entity with it; the wording is this one either way, so a read and a write
/// cannot disagree about what was not found.
pub fn reputation_entity_not_found(reference: &str) -> String {
    format!("Reputation entity {reference} does not exist.")
}

/// The account and Region one call is scoped to, and the check that decides which
/// resource ARNs that call may name.
///
/// The account is the one this emulator serves: a handler runs only after the
/// framework has verified the request's signature, so that account is the caller's
/// account, and the Region is the one the request's own SigV4 credential scope names.
/// Both are read from the call rather than from anything the request body carries.
#[derive(Debug, Clone)]
pub struct CallerScope {
    /// The account id an ARN must carry to be one this caller may name.
    pub account_id: &'static str,
    /// The Region an ARN must carry to be reachable from this call.
    pub region: String,
}

impl CallerScope {
    /// The scope the call `parts` belongs to.
    pub fn of(parts: &::http::request::Parts) -> Self {
        Self {
            account_id: ::emulator_server::emulator_account_id(),
            region: request_region(parts),
        }
    }

    /// Whether `arn` names a resource of this service that this call may reach: an
    /// ARN of this service's namespace whose Region is this call's and whose account
    /// is the caller's.
    ///
    /// A reputation entity is reached only by the ARN of the resource it tracks, and
    /// the reputation operations report and manage "reputation entities in your
    /// Amazon SES account in the current Amazon Web Services Region" — so the
    /// account and Region fields of a reference a request carries are checked here,
    /// against the call, before anything resolves it. A reference failing the check
    /// names no entity this call can reach, whatever it names elsewhere, and is
    /// answered exactly as one that was never created, so the two are
    /// indistinguishable to the caller.
    ///
    /// The ARN itself is read by the shared parse, which is where what an ARN of this
    /// vendor is — fields, partition, the resource field's own colons — is decided.
    pub fn covers_resource(&self, arn: &str) -> bool {
        ::service_common::arn::parse_of(arn, ARN_NAMESPACE)
            .is_some_and(|arn| arn.region == self.region && arn.account == self.account_id)
    }
}

/// The `SuppressionListScope` a tenant gets when the request names none:
/// `TenantSuppressionAttributes$SuppressionScope` documents that "if you don't
/// specify a suppression scope, the tenant defaults to ACCOUNT scope and uses the
/// account-level suppression list".
pub const DEFAULT_SUPPRESSION_SCOPE: &str = "ACCOUNT";

/// The `(SuppressedReasons, SuppressionScope)` pair a write of one tenant's
/// `TenantSuppressionAttributes` stores, from the members the request carried, or
/// the refusal wording for a value outside the modelled set.
///
/// Shared by every operation that writes a tenant's suppression preferences, so the
/// preferences a tenant ends up with do not depend on which one wrote them: a member
/// the request omits takes its documented default rather than whatever happened to be
/// stored — `TenantSuppressionAttributes$SuppressionScope` "defaults to ACCOUNT scope"
/// when the request names none, and a tenant that names no reason adds nothing to its
/// list automatically. Both enums carry their values as plain strings here, so the
/// value set is the caller's to enforce; the wording is the service's shared one (see
/// [`modelled_enum_value`]).
pub fn tenant_suppression_preferences(
    suppressed_reasons: Option<Vec<String>>,
    suppression_scope: Option<String>,
) -> Result<(Vec<String>, String), String> {
    let reasons = suppressed_reasons
        .unwrap_or_default()
        .into_iter()
        .map(|reason| {
            modelled_enum_value(
                "SuppressedReasons",
                reason,
                crate::ops::types::SUPPRESSION_LIST_REASON_VALUES,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let scope = match suppression_scope {
        Some(scope) => modelled_enum_value(
            "SuppressionScope",
            scope,
            crate::ops::types::SUPPRESSION_LIST_SCOPE_VALUES,
        )?,
        None => DEFAULT_SUPPRESSION_SCOPE.to_string(),
    };
    Ok((reasons, scope))
}

/// The AWS-Region this request executes in, which is also the region to stamp into
/// the ARNs it mints: the SigV4 credential scope's region, else [`DEFAULT_REGION`].
///
/// A multi-region endpoint's primary region is "the AWS-Region where the operation is
/// executed", and only endpoints whose primary region is the Region a call executes in
/// are visible to it, so every multi-region-endpoint operation resolves the Region
/// here too.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:ses:<region>:<account>:tenant/<name>/<id>` — the ARN SES reports for a
/// tenant: the resource part names the tenant AND its id, so the ARN identifies
/// the tenant record even after a tenant of the same name is recreated. Minted
/// once at create and read back from the record afterwards.
pub fn tenant_arn(region: &str, tenant_name: &str, tenant_id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("tenant/{tenant_name}/{tenant_id}"),
    )
}

/// `arn:aws:ses:<region>:<account>:identity/<identity>` — the ARN of an email
/// identity, which is how a refusal that must name the resource rather than the
/// request's spelling of it names one.
pub fn identity_arn(region: &str, identity: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("identity/{identity}"))
}

/// Whether `tenant_name` satisfies the documented `TenantName` form: at most
/// [`MAX_TENANT_NAME_CHARS`] characters, each a letter, a digit, `-` or `_`.
pub fn tenant_name_is_well_formed(tenant_name: &str) -> bool {
    let length = tenant_name.chars().count();
    length > 0
        && length <= MAX_TENANT_NAME_CHARS
        && tenant_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// How this service words `BadRequestException` for a `TenantName` that does not
/// satisfy the documented form, stated once for every operation that takes one.
pub fn invalid_tenant_name(tenant_name: &str) -> String {
    format!(
        "Invalid tenant name: {tenant_name}. The name can contain up to \
         {MAX_TENANT_NAME_CHARS} alphanumeric characters, including letters, \
         numbers, hyphens (-) and underscores (_) only."
    )
}

/// How this service words `NotFoundException` for a tenant it does not hold.
///
/// One wording for the operations that read a tenant, or a tenant's suppression
/// list, by `TenantName` — `GetTenant`, `GetSuppressedDestination` and
/// `ListSuppressedDestinations` — so a client sees the same sentence whichever of
/// them first tells it the tenant is gone. The tenant-resource association surface
/// words the same condition differently; see [`requested_tenant_not_found`].
pub fn tenant_not_found(tenant_name: &str) -> String {
    format!("Tenant {tenant_name} does not exist.")
}

/// How this service words `AlreadyExistsException` for a tenant name already taken.
pub fn tenant_already_exists(tenant_name: &str) -> String {
    format!("Tenant with name: {tenant_name} already exists.")
}

/// How the tenant-resource association surface words `NotFoundException` for a
/// tenant the account does not hold: the sentence SES answers a
/// `CreateTenantResourceAssociation` whose `TenantName` resolves to nothing, with the
/// requested name as its only variable part.
pub fn requested_tenant_not_found(tenant_name: &str) -> String {
    format!("The requested tenant <{tenant_name}> does not exist.")
}

/// How this service words `AlreadyExistsException` for a resource already associated
/// with the named tenant, stated once for every operation that creates an
/// association. The resource ARN and the tenant name are the only variable parts,
/// both taken from the request.
pub fn tenant_resource_association_exists(tenant_name: &str, resource_arn: &str) -> String {
    format!("Resources {resource_arn} has already been associated with tenant {tenant_name}")
}

/// The resource kinds a tenant association may name, as (the resource-type token the
/// resource's own SES ARN carries, the modelled `ResourceType` value that kind is
/// reported as).
///
/// `ResourceType` binds exactly these three values —
/// `smithy_query(query="shape",shape="ResourceType")`: `EMAIL_IDENTITY` ("an email
/// address or domain that you use to send email"), `CONFIGURATION_SET` ("a set of
/// rules that you can apply to emails you send") and `EMAIL_TEMPLATE` ("a template
/// that defines the content of an email message") — and SES addresses each of them
/// with `identity/<identity>`, `configuration-set/<name>` and `template/<name>` in
/// the resource field of its ARN.
pub const TENANT_RESOURCE_TYPES: [(&str, &str); 3] = [
    ("identity", "EMAIL_IDENTITY"),
    ("configuration-set", "CONFIGURATION_SET"),
    ("template", "EMAIL_TEMPLATE"),
];

/// The modelled `ResourceType` the ARN `resource_arn` names, or `None` when it names
/// no kind of resource a tenant may hold.
///
/// `CreateTenantResourceAssociation` documents that "resources can be email
/// identities, configuration sets, or email templates", so a value naming anything
/// else — another vendor's resource, an SES resource of another kind, or no ARN at
/// all — addresses nothing this surface can associate. Which values name a resource
/// of this service at all is read by [`ses_resource_of`], so this surface and the
/// tagging surface cannot disagree about it; the kinds a tenant may hold are the
/// narrowing this surface adds.
pub fn tenant_resource_type(resource_arn: &str) -> Option<&'static str> {
    let (resource_type, _) = ses_resource_of(resource_arn)?;
    TENANT_RESOURCE_TYPES
        .iter()
        .find(|(token, _)| resource_type == *token)
        .map(|(_, modelled)| *modelled)
}

/// How this service words `BadRequestException` for a caller-supplied `ResourceArn`
/// in none of the resource-ARN forms the surface reading it addresses: the
/// tenant-resource surface for an ARN naming no resource kind a tenant may hold, the
/// tagging surface for a value that is no SES resource ARN, or one whose resource
/// field names no resource at all.
///
/// The sentence is fixed text with no variable part: the refusal is about the FORM of
/// the value — SES reads the ARN against the resource-ARN forms it addresses, and a
/// value matching none of them (another vendor's resource, an SES resource of a kind
/// the surface cannot address, or no ARN at all) names nothing it can act on. Stated
/// once, so two surfaces cannot word the one condition differently.
pub const UNSUPPORTED_RESOURCE_ARN: &str = "Provided ARN is not in SES resource ARN format";

/// The keys `ListTenantResourcesFilterKey` names: "currently, the only supported
/// filter key is RESOURCE_TYPE". The filter arrives as a map of plain strings, so
/// the handler is what refuses a key the enum does not name.
pub const LIST_TENANT_RESOURCES_FILTER_KEYS: [&str; 1] = ["RESOURCE_TYPE"];

/// Every `ListTenantsFilterKey` the model binds, in the order the enum lists them:
/// "TENANT_NAME_CONTAINS — filter by a substring of the tenant name" and
/// "SENDING_STATUS — filter by sending status". The filter arrives as a map of plain
/// strings, so the handler is what refuses a key the enum does not name.
pub const LIST_TENANTS_FILTER_KEYS: [&str; 2] = ["TENANT_NAME_CONTAINS", "SENDING_STATUS"];

/// Configuration sets the account may hold: SES's documented quota is 10,000
/// configuration sets per account per Region, and `CreateConfigurationSet` refuses
/// past it with the `LimitExceededException` its model binds.
pub const MAX_CONFIGURATION_SETS: usize = 10_000;

/// The longest a `ConfigurationSetName` may be, from the member's own
/// documentation: "The name can contain up to 64 alphanumeric characters, including
/// letters, numbers, hyphens (-) and underscores (_) only."
pub const MAX_CONFIGURATION_SET_NAME_LEN: usize = 64;

/// Event destinations one configuration set may hold.
///
/// A CHOSEN bound, not an AWS one: a destination is created by a request, so a
/// long-lived emulator must not grow without limit while serving them. It is far
/// above what a client publishing a set's events to its destinations needs, and
/// `CreateConfigurationSetEventDestination` refuses past it with the
/// `LimitExceededException` its model binds.
pub const MAX_EVENT_DESTINATIONS_PER_SET: usize = 1_000;

/// Per-service state for com-amazonaws-sesv2-simpleemailservice-v2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSesv2SimpleemailserviceV2State {
    /// Email identities, keyed by [`identity_key`] of the `EmailIdentity` they
    /// were created with. Ordered, so `ListEmailIdentities` enumerates and pages
    /// deterministically.
    pub identities: BTreeMap<String, EmailIdentityRecord>,
    /// Tenants, keyed by the `TenantName` they were created with — an opaque name
    /// the API echoes back as given, so it is keyed exactly as supplied. Ordered,
    /// so `ListTenants` enumerates and pages deterministically.
    pub tenants: BTreeMap<String, TenantRecord>,
    /// Monotonic counter feeding [`Self::next_tenant_id`], so two tenants created
    /// in the same millisecond still get different ids.
    pub tenant_sequence: u64,
    /// Tenant-resource associations, keyed by the resource's `ResourceArn` and then
    /// the `TenantName` it is associated with: one resource may be associated with
    /// several tenants and one tenant with several resources, so the pair is the
    /// identity of an association. Ordered, so `ListResourceTenants` enumerates and
    /// pages one resource's tenants, and `ListTenantResources` one tenant's
    /// resources, deterministically.
    pub tenant_resource_associations: BTreeMap<(String, String), TenantResourceAssociationRecord>,
    /// Contact lists, keyed by the `ContactListName` they were created with — an
    /// opaque name the API echoes back as given, so it is keyed exactly as supplied.
    /// Ordered, so `ListContactLists` enumerates and pages deterministically.
    pub contact_lists: BTreeMap<String, ContactListRecord>,
    /// Multi-region endpoints, keyed by their primary region and then by the
    /// `EndpointName` they were created with. An endpoint is managed only from its
    /// primary region, so the region is part of the key: the same name in another
    /// region names another resource. Ordered, so `ListMultiRegionEndpoints`
    /// enumerates and pages deterministically within one region.
    pub multi_region_endpoints: BTreeMap<(String, String), MultiRegionEndpointRecord>,
    /// Configuration sets, keyed by the `ConfigurationSetName` they were created
    /// with. The name is an identifier the API echoes back as given and compares
    /// exactly, so it is keyed exactly as supplied. Ordered, so
    /// `ListConfigurationSets` enumerates and pages deterministically.
    pub configuration_sets: BTreeMap<String, ConfigurationSetRecord>,
    /// Addresses on a suppression list, keyed by [`suppression_key`]: the list the
    /// entry belongs to (a tenant's, or the account's) and the address within it, so
    /// a tenant's list and the account-level list are separate. Ordered, so
    /// `ListSuppressedDestinations` enumerates and pages deterministically. Bounded
    /// by [`MAX_SUPPRESSED_DESTINATIONS`], which [`Self::suppress_destination`]
    /// enforces.
    pub suppressed_destinations: BTreeMap<String, SuppressedDestinationRecord>,
    /// The outbound-message log, oldest first: one entry per accepted `SendEmail`.
    pub sent_messages: Vec<SentMessage>,
    /// Per-account send counters, bumped by `SendEmail`.
    pub send_counters: SendQuotaState,
    /// The account-level attributes of this account in this Region: the sending
    /// and dedicated-IP switches, the reputation status, the suppression, VDM and
    /// pricing attributes and the account details, as the account operations left
    /// them.
    pub account: AccountState,
    /// Monotonic counter feeding [`Self::next_message_id`], so two sends in the
    /// same microsecond still get different message ids.
    pub message_sequence: u64,
    /// Monotonic counter feeding [`Self::next_endpoint_id`], so two endpoints
    /// created in the same microsecond still get different ids.
    pub endpoint_sequence: u64,
}

impl ComAmazonawsSesv2SimpleemailserviceV2State {
    /// Materialise every transition the clock already owes at `now`: an identity's
    /// verification, an endpoint's provisioning and the completion of an endpoint's
    /// deletion.
    ///
    /// A reporting operation may call this: the transition was due at `now` whether
    /// or not anybody called, so applying it changes nothing a later caller would
    /// otherwise have seen.
    pub fn settle(&mut self, now: SystemTime) {
        for identity in self.identities.values_mut() {
            identity.settle(now);
        }
        for endpoint in self.multi_region_endpoints.values_mut() {
            endpoint.settle(now);
        }
        // A deletion that was due at `now` has completed, whether or not anybody
        // called, so the endpoint it was tearing down — and its name — are gone.
        self.multi_region_endpoints
            .retain(|_, endpoint| !endpoint.deletion_complete(now));
        // `sent_last_24h` is a view of the log over the trailing window, not an
        // independent tally: recompute it here so it cannot drift as the window
        // moves past entries that no longer count.
        let charged = self.recipients_in_quota_window(now);
        self.send_counters.sent_last_24h = charged;
        self.account.settle(now);
    }

    /// Every reputation entity reachable by the call `scope` describes, ordered by
    /// `ReputationEntityReference`.
    ///
    /// No operation creates a reputation entity: one exists for each resource whose
    /// reputation SES tracks and whose sending it can stop, referenced by that
    /// resource's ARN. The table is therefore DERIVED from the resource records on
    /// every read instead of being kept beside them, and a resource's own record
    /// stays the single place its status lives.
    ///
    /// Which resources those are is decided by the one member that carries an
    /// entity's status: a reputation entity's `CustomerManagedStatus` is a
    /// `SendingStatus`, and `Tenant$SendingStatus` is the only resource member in
    /// this service's model that targets that shape — the status the reputation
    /// writes move. A configuration set's `SendingOptions$SendingEnabled` and
    /// `ReputationOptions$ReputationMetricsEnabled` are a boolean switch and a
    /// metrics flag the configuration-set operations own, not a `SendingStatus` a
    /// reputation entity could report or a reputation write could move, and an email
    /// identity carries no sending status at all; so neither is a tracked resource
    /// here, and the derivation spans the tenants.
    ///
    /// The scope is applied here, so a read and a write of one call cannot disagree
    /// about which entities it reaches; see [`CallerScope::covers_resource`].
    pub fn reputation_entities(
        &self,
        scope: &CallerScope,
    ) -> Vec<crate::ops::types::ReputationEntity> {
        let mut entities: Vec<crate::ops::types::ReputationEntity> = self
            .tenants
            .values()
            .filter(|tenant| scope.covers_resource(&tenant.arn))
            .map(TenantRecord::reputation_entity)
            .collect();
        // Ordered by reference, so a listing of them enumerates and pages
        // deterministically whatever order the resources were created in.
        entities.sort_by(|left, right| {
            left.reputation_entity_reference
                .cmp(&right.reputation_entity_reference)
        });
        entities
    }

    /// The tenant a reputation entity reference names, for a write that has to
    /// change its status: the reference of a resource-type entity is that resource's
    /// ARN, which a tenant record carries from the moment it was created.
    ///
    /// `None` unless the reference is one the call `scope` describes may reach — the
    /// same scope [`Self::reputation_entities`] reports over, so a write can only
    /// reach an entity a listing of the same call would have reported.
    pub fn tenant_by_reputation_entity_reference(
        &mut self,
        reference: &str,
        scope: &CallerScope,
    ) -> Option<&mut TenantRecord> {
        if !scope.covers_resource(reference) {
            return None;
        }
        self.tenants
            .values_mut()
            .find(|tenant| tenant.arn == reference)
    }

    /// The tags stored on the resource `resource_arn` names, as the modelled `TagList`
    /// the tagging surface reports them; `None` when the call `scope` describes holds
    /// no such resource.
    ///
    /// SES writes each of its resources in the resource field of its ARN as the
    /// resource TYPE and then the id the resource is held under — an email identity as
    /// `identity/<identity>` (the form [`identity_arn`] mints), a contact list as
    /// `contact-list/<name>`, a configuration set as `configuration-set/<name>` and a
    /// tenant as `tenant/<name>/<id>` (the form [`tenant_arn`] mints) — so the type
    /// picks the table and the id picks the record within it. An identity resolves
    /// through [`identity_key`], so an ARN spelling the domain part in another case
    /// names the same identity the rest of the API does. A tenant's id is part of its
    /// ARN, so the whole value is compared against the ARN the tenant was minted with,
    /// which keeps a recreated tenant of the same name from answering for its
    /// predecessor.
    ///
    /// An ARN naming a resource kind this account holds no record of — a kind nothing
    /// creates here yet, or an id never created — resolves to nothing, exactly as a
    /// name that was never created: the two are indistinguishable to the caller.
    ///
    /// The scope is applied first, so an ARN of another account or another Region
    /// reaches nothing whatever it names elsewhere; see
    /// [`CallerScope::covers_resource`]. Reporting only: the resolution reads the
    /// tables and writes nothing, so a read leaves every later caller seeing exactly
    /// what it saw.
    pub fn resource_tags(
        &self,
        resource_arn: &str,
        scope: &CallerScope,
    ) -> Option<Vec<crate::ops::types::Tag>> {
        if !scope.covers_resource(resource_arn) {
            return None;
        }
        let (resource_type, resource_id) = ses_resource_of(resource_arn)?;
        match resource_type {
            "identity" => self
                .identities
                .get(&identity_key(resource_id))
                .map(EmailIdentityRecord::tag_list),
            "contact-list" => self
                .contact_lists
                .get(resource_id)
                .map(ContactListRecord::tag_list),
            "configuration-set" => self
                .configuration_sets
                .get(resource_id)
                .map(ConfigurationSetRecord::tag_list),
            "tenant" => self
                .tenants
                .values()
                .find(|tenant| tenant.arn == resource_arn)
                .map(TenantRecord::tag_list),
            _ => None,
        }
    }

    /// Put `record` on the suppression list `key` names, refusing a NEW entry once
    /// the table holds [`MAX_SUPPRESSED_DESTINATIONS`].
    ///
    /// Every write to the table goes through here, so the ceiling cannot be bypassed:
    /// `false` means the table was full and nothing was stored, which the calling
    /// operation reports with the limit error its own model binds. Replacing an entry
    /// the table already holds grows nothing, so it is always accepted — which is
    /// what keeps a repeated suppression of one address idempotent.
    pub fn suppress_destination(
        &mut self,
        key: String,
        record: SuppressedDestinationRecord,
    ) -> bool {
        if !self.suppressed_destinations.contains_key(&key)
            && self.suppressed_destinations.len() >= MAX_SUPPRESSED_DESTINATIONS
        {
            return false;
        }
        self.suppressed_destinations.insert(key, record);
        true
    }

    /// The identity that authorises sending from `address`, if any.
    ///
    /// An address identity authorises itself; a domain identity authorises every
    /// address in that domain and in any of its subdomains. The comparison runs on
    /// [`identity_key`], so the domain part matches without regard to case.
    pub fn sending_identity(&self, address: &str) -> Option<&EmailIdentityRecord> {
        let key = identity_key(address);
        if let Some(exact) = self.identities.get(&key) {
            return Some(exact);
        }
        let (_, domain) = key.rsplit_once('@')?;
        // Walk the domain and each of its parent domains: `a.b.example.com` is
        // covered by an identity for `a.b.example.com`, `b.example.com` or
        // `example.com`.
        let mut candidate = Some(domain);
        while let Some(suffix) = candidate {
            if let Some(found) = self
                .identities
                .get(suffix)
                .filter(|record| record.identity_kind == IdentityKind::Domain)
            {
                return Some(found);
            }
            candidate = suffix.split_once('.').map(|(_, parent)| parent);
        }
        None
    }

    /// Recipients charged against the sending quota within the trailing
    /// [`SEND_QUOTA_WINDOW`], derived from the log rather than from a counter that
    /// only ever grows, so the quota frees up as the window moves.
    pub fn recipients_in_quota_window(&self, now: SystemTime) -> f64 {
        let cutoff = now
            .checked_sub(SEND_QUOTA_WINDOW)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        self.sent_messages
            .iter()
            .filter(|message| message.sent_at > cutoff)
            .map(|message| message.recipients().count() as f64)
            .sum()
    }

    /// Append an accepted message to the log, keeping it within
    /// [`MAX_SENT_MESSAGES`] by evicting the oldest entries.
    pub fn record_sent(&mut self, message: SentMessage) {
        self.sent_messages.push(message);
        if self.sent_messages.len() > MAX_SENT_MESSAGES {
            let overflow = self.sent_messages.len() - MAX_SENT_MESSAGES;
            self.sent_messages.drain(..overflow);
        }
    }

    /// The portion of a message body the log retains: at most
    /// [`MAX_STORED_BODY_CHARS`] characters.
    pub fn retained_body(body: &str) -> String {
        body.chars().take(MAX_STORED_BODY_CHARS).collect()
    }

    /// Mint the next outbound message id.
    ///
    /// `OutboundMessageId` carries no documented format, so the shape mirrors what
    /// SES hands back (`0100` + a timestamp + a body + a trailing field) while the
    /// only guarantee this makes is the documented one: the id is unique per
    /// accepted message. Uniqueness is structural — the monotonic sequence number
    /// is part of the id — so two sends in the same millisecond still differ.
    pub fn next_message_id(&mut self, now: SystemTime) -> String {
        self.message_sequence += 1;
        let millis = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let sequence = self.message_sequence;
        format!("0100{millis:012x}-{sequence:016x}-000000")
    }

    /// Mint the id of a new multi-region endpoint.
    ///
    /// `EndpointId` carries no documented format beyond being "the ID of the
    /// multi-region endpoint", so the shape mirrors the dashed 8-4-4-4-12 hex id SES
    /// hands back while the only guarantees this makes are the ones a client depends
    /// on: the id is unique across endpoints and stable for as long as the endpoint
    /// holds it (it is minted once, at create, and read back from the record
    /// afterwards). Uniqueness is structural — the monotonic sequence number is
    /// hashed in — so two endpoints created in the same millisecond still differ.
    pub fn next_endpoint_id(&mut self, name: &str, region: &str, now: SystemTime) -> String {
        self.endpoint_sequence += 1;
        let nanos = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut hex = String::with_capacity(32);
        for salt in [0_u64, u64::from_be_bytes(*b"Endpoint")] {
            let mut hasher = DefaultHasher::new();
            salt.hash(&mut hasher);
            name.hash(&mut hasher);
            region.hash(&mut hasher);
            nanos.hash(&mut hasher);
            self.endpoint_sequence.hash(&mut hasher);
            write!(hex, "{:016x}", hasher.finish()).expect("writing to a String cannot fail");
        }
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )
    }

    /// Mint the next `TenantId`.
    ///
    /// `TenantId` is documented only as "a unique identifier for the tenant" and
    /// carries no modelled format, so the one guarantee this makes is that
    /// documented one: uniqueness is structural, because the monotonic sequence
    /// number is part of the id, so two tenants created in the same millisecond
    /// still differ. The id is minted once, at create, and read back from the
    /// record afterwards.
    pub fn next_tenant_id(&mut self, now: SystemTime) -> String {
        self.tenant_sequence += 1;
        let millis = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let sequence = self.tenant_sequence;
        format!("{millis:016x}{sequence:016x}")
    }
}

/// Whether an `EmailIdentity` names an address or a domain.
///
/// The model's `IdentityType` also carries `MANAGED_DOMAIN`, which its own
/// documentation says is "not supported for email identity types", so nothing here
/// produces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityKind {
    /// `IdentityType` = `EMAIL_ADDRESS`.
    EmailAddress,
    /// `IdentityType` = `DOMAIN`.
    Domain,
}

impl IdentityKind {
    /// An `EmailIdentity` that carries a local part (`@`) is an address; anything
    /// else is a domain.
    pub fn of(identity: &str) -> Self {
        if identity.contains('@') {
            Self::EmailAddress
        } else {
            Self::Domain
        }
    }

    /// The modelled `IdentityType` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmailAddress => "EMAIL_ADDRESS",
            Self::Domain => "DOMAIN",
        }
    }
}

/// The value set shared by the model's `VerificationStatus` and `DkimStatus`
/// enums. Only the states this emulator can reach are represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SesStatus {
    /// The process has not been initiated for this identity.
    NotStarted,
    /// The process was initiated and has not completed.
    Pending,
    /// The process completed successfully.
    Success,
}

impl SesStatus {
    /// The modelled enum value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "NOT_STARTED",
            Self::Pending => "PENDING",
            Self::Success => "SUCCESS",
        }
    }
}

/// How DKIM was configured for an identity (`DkimSigningAttributesOrigin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DkimOrigin {
    /// Easy DKIM: SES generated the key pair and published the tokens.
    AwsSes,
    /// Bring Your Own DKIM: the caller supplied the selector and private key.
    External,
}

impl DkimOrigin {
    /// The modelled enum value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwsSes => "AWS_SES",
            Self::External => "EXTERNAL",
        }
    }
}

/// The DKIM configuration stored for one identity.
#[derive(Debug, Clone)]
pub struct DkimState {
    /// Whether messages from the identity are DKIM-signed.
    pub signing_enabled: bool,
    /// The DKIM authentication status.
    pub status: SesStatus,
    /// Easy DKIM CNAME tokens, or the BYODKIM selector.
    pub tokens: Vec<String>,
    /// How DKIM was configured.
    pub origin: DkimOrigin,
    /// `[Easy DKIM]` the key length of the key pair in use, when SES owns the key.
    pub signing_key_length: Option<String>,
}

impl DkimState {
    /// Easy DKIM for a domain: SES owns the key pair, publishes three CNAME tokens
    /// and waits for them to appear in the domain's DNS.
    pub fn easy_dkim(domain: &str, key_length: Option<String>) -> Self {
        Self {
            signing_enabled: true,
            status: SesStatus::Pending,
            tokens: easy_dkim_tokens(domain),
            origin: DkimOrigin::AwsSes,
            // `DkimSigningKeyLength` has two values; SES generates 2048-bit keys
            // unless the request asks for the other one.
            signing_key_length: Some(key_length.unwrap_or_else(|| "RSA_2048_BIT".to_string())),
        }
    }

    /// BYODKIM for a domain: the caller owns the key pair, so the stored token set
    /// is the selector that names its public key and SES reports no key length.
    pub fn byo_dkim(selector: &str) -> Self {
        Self {
            signing_enabled: true,
            status: SesStatus::Pending,
            tokens: vec![selector.to_string()],
            origin: DkimOrigin::External,
            signing_key_length: None,
        }
    }

    /// An address identity: DKIM signing applies to domains, so nothing is
    /// configured and the authentication process was never initiated.
    pub fn not_started() -> Self {
        Self {
            signing_enabled: false,
            status: SesStatus::NotStarted,
            tokens: Vec::new(),
            origin: DkimOrigin::AwsSes,
            signing_key_length: None,
        }
    }
}

/// The status of one certificate association (`IdentityCertificateStatus`).
///
/// Only the states this emulator can reach are represented. `FAILED` is what SES
/// reports for an association whose certificate "has passed its expiration time",
/// which is read from the ACM certificate the association names: that certificate
/// lives in another service's state, so nothing here produces that status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateStatus {
    /// "The certificate association was created and the certificate is being
    /// prepared for use."
    Provisioning,
    /// "The certificate is ready to use for signing."
    Active,
    /// "The certificate is no longer used for signing" — what an association that has
    /// been disassociated carries, until SES cleans it up.
    Inactive,
    /// "The certificate association is being cleaned up."
    Deprovisioning,
}

impl CertificateStatus {
    /// The modelled enum value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "PROVISIONING",
            Self::Active => "ACTIVE",
            Self::Inactive => "INACTIVE",
            Self::Deprovisioning => "DEPROVISIONING",
        }
    }
}

/// One S/MIME certificate association: the address its certificate signs for, the
/// ACM certificate it names, and where the association is in its lifecycle.
#[derive(Debug, Clone)]
pub struct IdentityCertificateRecord {
    /// `FromAddress`: the address the certificate applies to, as the request spelled
    /// it.
    pub from_address: String,
    /// `CertificateArn`: the ACM certificate the association names.
    pub certificate_arn: String,
    /// Where the association is in its lifecycle.
    pub status: CertificateStatus,
    /// When this status's clock-owed transition falls due: `PROVISIONING` ->
    /// `ACTIVE`, `INACTIVE` -> `DEPROVISIONING`, and the clean-up that removes a
    /// `DEPROVISIONING` association.
    pub status_due_at: SystemTime,
}

impl IdentityCertificateRecord {
    /// A fresh association, as `AssociateEmailIdentityCertificate` stores it: "when
    /// the association is created, the certificate begins provisioning and its status
    /// is PROVISIONING".
    pub fn new(from_address: &str, certificate_arn: &str, now: SystemTime) -> Self {
        Self {
            from_address: from_address.to_string(),
            certificate_arn: certificate_arn.to_string(),
            status: CertificateStatus::Provisioning,
            status_due_at: now.checked_add(CERTIFICATE_SETTLE).unwrap_or(now),
        }
    }

    /// Stop signing with this association, as `DisassociateEmailIdentityCertificate`
    /// does: "after the association is removed, Amazon SES API v2 stops adding an
    /// S/MIME signature to messages sent from that address", so the certificate "is
    /// no longer used for signing" at once, and SES then cleans the association up.
    pub fn deactivate(&mut self, now: SystemTime) {
        self.status = CertificateStatus::Inactive;
        self.status_due_at = now.checked_add(CERTIFICATE_SETTLE).unwrap_or(now);
    }

    /// Whether this association is one the identity signs with: a `PROVISIONING` one
    /// is being prepared for signing and an `ACTIVE` one is ready for it, while a
    /// disassociated one is on its way out and signs nothing.
    pub fn signs(&self) -> bool {
        matches!(
            self.status,
            CertificateStatus::Provisioning | CertificateStatus::Active
        )
    }

    /// Apply every transition the clock owes this association at `now`, and report
    /// whether the association still exists afterwards: a disassociated one is cleaned
    /// up, and once that clean-up falls due it is gone.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        loop {
            if now < self.status_due_at {
                return true;
            }
            match self.status {
                // Both are states the association stays in until a request moves it,
                // so neither owes the clock anything further.
                CertificateStatus::Provisioning => {
                    self.status = CertificateStatus::Active;
                    return true;
                }
                CertificateStatus::Active => return true,
                CertificateStatus::Inactive => {
                    self.status = CertificateStatus::Deprovisioning;
                    // Measured from the instant the clean-up was owed, not from this
                    // call, so a store nobody read for a while settles all the way
                    // through rather than one step per read.
                    self.status_due_at = self
                        .status_due_at
                        .checked_add(CERTIFICATE_SETTLE)
                        .unwrap_or(now);
                }
                CertificateStatus::Deprovisioning => return false,
            }
        }
    }

    /// The modelled `IdentityCertificate` view of this record.
    pub fn view(&self) -> crate::ops::types::IdentityCertificate {
        crate::ops::types::IdentityCertificate {
            from_address: Some(self.from_address.clone()),
            status: Some(self.status.as_str().to_string()),
            certificate_arn: Some(self.certificate_arn.clone()),
            // "The timestamp after which the certificate is no longer valid" is the
            // validity of the ACM certificate this association names, which belongs
            // to that service's state and not to this one: SES reads it from the
            // certificate rather than deciding it, so no expiry is reported here
            // rather than one being made up.
            certificate_expiry_time: None,
        }
    }
}

/// One email identity.
#[derive(Debug, Clone)]
pub struct EmailIdentityRecord {
    /// The `EmailIdentity` this record is keyed by.
    pub name: String,
    /// Address or domain.
    pub identity_kind: IdentityKind,
    /// `VerificationStatus`.
    pub verification_status: SesStatus,
    /// `VerifiedForSendingStatus` / `SendingEnabled`.
    pub verified_for_sending: bool,
    /// `FeedbackForwardingStatus`.
    pub feedback_forwarding: bool,
    /// The identity's DKIM configuration.
    pub dkim: DkimState,
    /// The configuration set used by default when sending from this identity.
    pub configuration_set_name: Option<String>,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// Sending-authorization policies, keyed by policy name.
    pub policies: BTreeMap<String, String>,
    /// When the identity was created.
    pub created_at: SystemTime,
    /// When the `PENDING` -> `SUCCESS` verification transition falls due.
    pub verification_due_at: SystemTime,
    /// The S/MIME certificate associations this identity holds, keyed by
    /// [`identity_key`] of the address each applies to — "each email address can have
    /// only one certificate association". Ordered, so
    /// `ListEmailIdentityCertificates` enumerates and pages deterministically.
    pub certificates: BTreeMap<String, IdentityCertificateRecord>,
}

impl EmailIdentityRecord {
    /// Start verifying `name`, as `CreateEmailIdentity` does. `kind` is the
    /// classification the caller already computed from [`addr_spec`], so the record
    /// and its creator cannot disagree about it.
    pub fn new(name: &str, kind: IdentityKind, dkim: DkimState, now: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            identity_kind: kind,
            // "Starts the process of verifying an email identity": the identity is
            // not verified yet, so it cannot send yet either.
            verification_status: SesStatus::Pending,
            verified_for_sending: false,
            // SES forwards bounces and complaints to the identity's owner unless
            // the caller turns it off with PutEmailIdentityFeedbackAttributes.
            feedback_forwarding: true,
            dkim,
            configuration_set_name: None,
            tags: Vec::new(),
            policies: BTreeMap::new(),
            created_at: now,
            verification_due_at: now.checked_add(VERIFICATION_SETTLE).unwrap_or(now),
            certificates: BTreeMap::new(),
        }
    }

    /// The modelled `DkimAttributes` view of this record.
    ///
    /// One projection shared by every operation that reports DKIM state, so a
    /// create and the read after it cannot disagree.
    pub fn dkim_attributes(&self) -> crate::ops::types::DkimAttributes {
        crate::ops::types::DkimAttributes {
            signing_enabled: self.dkim.signing_enabled,
            status: Some(self.dkim.status.as_str().to_string()),
            // An identity with no DKIM configuration has no token set at all,
            // rather than an empty one.
            tokens: (!self.dkim.tokens.is_empty()).then(|| self.dkim.tokens.clone()),
            // SES reports the publishing hosted zone only for identities whose
            // records it publishes itself; nothing here configures one.
            signing_hosted_zone: None,
            signing_attributes_origin: Some(self.dkim.origin.as_str().to_string()),
            // `NextSigningKeyLength` describes a rotation that has been requested
            // and not yet taken effect; `PutEmailIdentityDkimSigningAttributes`
            // owns that, and nothing here requests one.
            next_signing_key_length: None,
            current_signing_key_length: self.dkim.signing_key_length.clone(),
            // SES generated the Easy DKIM key pair when it configured DKIM for the
            // identity, which is exactly when it published a token set of its own;
            // with BYODKIM the caller owns the key, so SES generated none, and an
            // identity with no DKIM configuration has no key pair either. Whether
            // signing is currently switched on does not come into it: turning
            // signing off generates and destroys no key.
            last_key_generation_timestamp: (self.dkim.origin == DkimOrigin::AwsSes
                && !self.dkim.tokens.is_empty())
                .then(|| ::aws_smithy_types::DateTime::from(self.created_at)),
        }
    }

    /// The modelled `TagList` view of this record's tags.
    pub fn tag_list(&self) -> Vec<crate::ops::types::Tag> {
        self.tags
            .iter()
            .map(|(key, value)| crate::ops::types::Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// Apply the transitions the clock owes this identity at `now`: its verification
    /// transition, and each certificate association's own.
    pub fn settle(&mut self, now: SystemTime) {
        if self.verification_status == SesStatus::Pending && now >= self.verification_due_at {
            self.verification_status = SesStatus::Success;
            self.verified_for_sending = true;
            if self.dkim.status == SesStatus::Pending {
                self.dkim.status = SesStatus::Success;
            }
        }
        self.certificates
            .retain(|_, certificate| certificate.settle(now));
    }
}

/// One contact list: the topics it publishes, its description and tags, and the
/// contacts subscribed to it.
///
/// The contacts live ON the list rather than in a table beside it: a contact is
/// identified by its list and its address together (every contact operation binds
/// both as path labels), and deleting the list takes its contacts with it.
#[derive(Debug, Clone)]
pub struct ContactListRecord {
    /// The `ContactListName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// `Topics`: the interest groups the list publishes, in the order supplied.
    pub topics: Vec<crate::ops::types::Topic>,
    /// `Description`.
    pub description: Option<String>,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// When the list was created.
    pub created_at: SystemTime,
    /// When the list was last changed.
    pub last_updated_at: SystemTime,
    /// The contacts on this list, keyed by [`identity_key`] of their address so a
    /// listing enumerates and pages deterministically.
    pub contacts: BTreeMap<String, ContactRecord>,
}

impl ContactListRecord {
    /// A list as `CreateContactList` stores it.
    pub fn new(name: &str, now: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            topics: Vec::new(),
            description: None,
            tags: Vec::new(),
            created_at: now,
            last_updated_at: now,
            contacts: BTreeMap::new(),
        }
    }

    /// Whether the list publishes a topic called `topic_name`.
    pub fn publishes_topic(&self, topic_name: &str) -> bool {
        self.topics.iter().any(|topic| topic.topic_name == topic_name)
    }

    /// Replace this list's metadata with what `UpdateContactList` supplied — "updates
    /// contact list metadata. This operation does a complete replacement" — and apply
    /// what that replacement means for the contacts on the list.
    ///
    /// `topics` and `description` become exactly what the request supplied, so a member
    /// the request omitted is CLEARED rather than kept, and the topics keep the order
    /// supplied. A contact's stated preference names a topic OF this list (`CreateContact`
    /// refuses one that names anything else), so a preference whose topic the replacement
    /// drops ceases to exist along with the topic: it is removed from every contact on the
    /// list, and declaring that topic again later does not bring it back, because nothing
    /// kept it.
    pub fn replace_metadata(
        &mut self,
        topics: Vec<crate::ops::types::Topic>,
        description: Option<String>,
        now: SystemTime,
    ) {
        self.topics = topics;
        self.description = description;
        self.last_updated_at = now;

        let published: BTreeSet<&str> = self
            .topics
            .iter()
            .map(|topic| topic.topic_name.as_str())
            .collect();
        for contact in self.contacts.values_mut() {
            contact
                .topic_preferences
                .retain(|preference| published.contains(preference.topic_name.as_str()));
            // The contact's own `last_updated_at` stays where its own last write left it:
            // what changed is this list, not the contact, and the dropped preference is
            // the topic's removal showing through rather than a write to the contact.
        }
    }

    /// The default preference the list's own topics give a contact that has stated
    /// none: one `TopicPreference` per topic, carrying that topic's
    /// `DefaultSubscriptionStatus`.
    ///
    /// `Contact$TopicDefaultPreferences` is "the default topic preferences applied to
    /// the contact", which is exactly this projection of the list's topics, so every
    /// operation that reports it computes it here.
    pub fn topic_default_preferences(&self) -> Vec<crate::ops::types::TopicPreference> {
        self.topics
            .iter()
            .map(|topic| crate::ops::types::TopicPreference {
                topic_name: topic.topic_name.clone(),
                subscription_status: topic.default_subscription_status.clone(),
            })
            .collect()
    }

    /// The modelled `ContactList` view of this record, as `ListContactLists` reports
    /// it.
    pub fn summary(&self) -> crate::ops::types::ContactList {
        crate::ops::types::ContactList {
            contact_list_name: Some(self.name.clone()),
            last_updated_timestamp: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
        }
    }

    /// The modelled `TagList` view of this record's tags.
    pub fn tag_list(&self) -> Vec<crate::ops::types::Tag> {
        self.tags
            .iter()
            .map(|(key, value)| crate::ops::types::Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// One contact on a contact list.
#[derive(Debug, Clone)]
pub struct ContactRecord {
    /// The `EmailAddress` this contact was created with, as the request spelled it.
    pub email_address: String,
    /// `TopicPreferences`: the preferences the contact has stated, in the order
    /// supplied. A topic the contact has stated no preference for is answered from
    /// the list's own default instead
    /// ([`ContactListRecord::topic_default_preferences`]).
    pub topic_preferences: Vec<crate::ops::types::TopicPreference>,
    /// `UnsubscribeAll`: the contact is unsubscribed from every topic of the list.
    pub unsubscribe_all: bool,
    /// `AttributesData`: the caller's own JSON document about this contact, kept as
    /// supplied.
    pub attributes_data: Option<String>,
    /// When the contact was created.
    pub created_at: SystemTime,
    /// When the contact was last changed.
    pub last_updated_at: SystemTime,
}

impl ContactRecord {
    /// A contact as `CreateContact` stores it.
    pub fn new(email_address: &str, now: SystemTime) -> Self {
        Self {
            email_address: email_address.to_string(),
            topic_preferences: Vec::new(),
            unsubscribe_all: false,
            attributes_data: None,
            created_at: now,
            last_updated_at: now,
        }
    }

    /// The contact's preference for `topic_name`, when it has stated one.
    pub fn stated_preference(&self, topic_name: &str) -> Option<&str> {
        self.topic_preferences
            .iter()
            .find(|preference| preference.topic_name == topic_name)
            .map(|preference| preference.subscription_status.as_str())
    }

    /// The modelled `Contact` view of this record, as `ListContacts` reports it.
    ///
    /// `list` is the list this contact is on, which supplies the default preferences.
    pub fn summary(&self, list: &ContactListRecord) -> crate::ops::types::Contact {
        crate::ops::types::Contact {
            email_address: Some(self.email_address.clone()),
            topic_preferences: Some(self.topic_preferences.clone()),
            topic_default_preferences: Some(list.topic_default_preferences()),
            unsubscribe_all: self.unsubscribe_all,
            last_updated_timestamp: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
        }
    }
}

/// One tenant: the logical container SES groups resources, reputation and sending
/// status under.
#[derive(Debug, Clone)]
pub struct TenantRecord {
    /// The `TenantName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// `TenantId`, minted once by [`ComAmazonawsSesv2SimpleemailserviceV2State::next_tenant_id`].
    pub tenant_id: String,
    /// `TenantArn`, minted once from the request's region by [`tenant_arn`].
    pub arn: String,
    /// When the tenant was created.
    pub created_at: SystemTime,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// `SendingStatus`: the status the account's own reputation decisions leave the
    /// tenant in, which is the `CustomerManagedStatus` of the tenant's reputation
    /// entity. `UpdateReputationEntityCustomerManagedStatus` is what writes it.
    pub sending_status: String,
    /// When [`Self::sending_status`] was last written, which is the
    /// `StatusRecord$LastUpdatedTimestamp` of the customer-managed status record:
    /// the tenant's creation instant until a caller first changes the status.
    pub sending_status_updated_at: SystemTime,
    /// `TenantSuppressionAttributes$SuppressedReasons`: the reasons an address is
    /// added to this tenant's suppression list, in the order supplied.
    pub suppressed_reasons: Vec<String>,
    /// `TenantSuppressionAttributes$SuppressionScope`.
    pub suppression_scope: String,
}

impl TenantRecord {
    /// A tenant as `CreateTenant` stores it: sending enabled, and the
    /// account-level suppression list until a caller asks for a tenant-level one.
    pub fn new(name: &str, tenant_id: String, arn: String, now: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            tenant_id,
            arn,
            created_at: now,
            tags: Vec::new(),
            sending_status: TENANT_SENDING_STATUS_CREATED.to_string(),
            sending_status_updated_at: now,
            suppressed_reasons: Vec::new(),
            suppression_scope: DEFAULT_SUPPRESSION_SCOPE.to_string(),
        }
    }

    /// The modelled `TenantSuppressionAttributes` view of this record, shared by
    /// every operation that reports a tenant's suppression preferences, so a write
    /// and the read after it cannot disagree.
    pub fn suppression_attributes(&self) -> crate::ops::types::TenantSuppressionAttributes {
        crate::ops::types::TenantSuppressionAttributes {
            // A tenant that adds nothing automatically has no reason set at all,
            // rather than an empty one.
            suppressed_reasons: (!self.suppressed_reasons.is_empty())
                .then(|| self.suppressed_reasons.clone()),
            suppression_scope: Some(self.suppression_scope.clone()),
        }
    }

    /// The modelled `Tenant` view of this record, as `GetTenant` reports it.
    pub fn view(&self) -> crate::ops::types::Tenant {
        crate::ops::types::Tenant {
            tenant_name: Some(self.name.clone()),
            tenant_id: Some(self.tenant_id.clone()),
            tenant_arn: Some(self.arn.clone()),
            created_timestamp: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            tags: Some(self.tag_list()),
            // "The status of sending capability for the tenant" is the tenant's
            // reputation entity's aggregate status, not the customer-managed record
            // alone: a tenant Amazon SES disabled may not send whatever the account
            // asked for.
            sending_status: Some(self.aggregate_sending_status()),
            suppression_attributes: Some(self.suppression_attributes()),
        }
    }

    /// The modelled `TenantInfo` view of this record, as `ListTenants` reports it:
    /// "basic information about each tenant, such as tenant name, ID, ARN, and
    /// creation timestamp".
    ///
    /// Every member it carries is the one [`Self::view`] reports under the same
    /// name, read off the same stored record, so a listing and a read of one tenant
    /// cannot disagree about it; `TenantInfo` simply carries no tags and no
    /// suppression attributes.
    pub fn info_view(&self) -> crate::ops::types::TenantInfo {
        let full = self.view();
        crate::ops::types::TenantInfo {
            tenant_name: full.tenant_name,
            tenant_id: full.tenant_id,
            tenant_arn: full.tenant_arn,
            created_timestamp: full.created_timestamp,
            sending_status: full.sending_status,
        }
    }

    /// The customer-managed status record of this tenant's reputation entity: the
    /// status the account left it in, and when that status was written.
    pub fn customer_managed_status(&self) -> crate::ops::types::StatusRecord {
        crate::ops::types::StatusRecord {
            status: Some(self.sending_status.clone()),
            // `StatusCause` describes "the reason for a status change, such as
            // automated actions taken due to reputation findings or manual status
            // updates"; a status this account set carries no finding to describe.
            cause: None,
            last_updated_timestamp: Some(::aws_smithy_types::DateTime::from(
                self.sending_status_updated_at,
            )),
        }
    }

    /// The Amazon-SES-managed status record of this tenant's reputation entity.
    ///
    /// See [`AWS_SES_MANAGED_SENDING_STATUS`]: nothing here raises a reputation
    /// finding, so this record has permitted sending since the tenant was created.
    pub fn aws_ses_managed_status(&self) -> crate::ops::types::StatusRecord {
        crate::ops::types::StatusRecord {
            status: Some(AWS_SES_MANAGED_SENDING_STATUS.to_string()),
            cause: None,
            last_updated_timestamp: Some(::aws_smithy_types::DateTime::from(self.created_at)),
        }
    }

    /// The aggregate sending status of this tenant's reputation entity, over its two
    /// status records — see [`aggregate_sending_status`].
    pub fn aggregate_sending_status(&self) -> String {
        aggregate_sending_status(&self.sending_status, AWS_SES_MANAGED_SENDING_STATUS)
    }

    /// The modelled `ReputationEntity` view of this tenant, as
    /// `ListReputationEntities` reports it.
    pub fn reputation_entity(&self) -> crate::ops::types::ReputationEntity {
        crate::ops::types::ReputationEntity {
            // "For resource-type entities, this is the Amazon Resource Name (ARN) of
            // the resource."
            reputation_entity_reference: Some(self.arn.clone()),
            reputation_entity_type: Some(REPUTATION_ENTITY_TYPE_RESOURCE.to_string()),
            // A reputation management policy is attached by
            // `UpdateReputationEntityPolicy`; until one is, the entity has none.
            reputation_management_policy: None,
            customer_managed_status: Some(self.customer_managed_status()),
            aws_ses_managed_status: Some(self.aws_ses_managed_status()),
            sending_status_aggregate: Some(self.aggregate_sending_status()),
            // "The reputation impact reflects the highest impact reputation finding
            // for the entity": with no finding against the entity there is no
            // highest one, so the member is absent rather than a level nothing
            // computed.
            reputation_impact: None,
        }
    }

    /// Write this tenant's suppression preferences, as a request that carries
    /// `TenantSuppressionAttributes` asks for.
    ///
    /// Both members are replaced together: the preferences a request states are the
    /// tenant's whole preferences afterwards, never merged with what was there.
    pub fn set_suppression_preferences(&mut self, suppressed_reasons: Vec<String>, scope: String) {
        self.suppressed_reasons = suppressed_reasons;
        self.suppression_scope = scope;
    }

    /// Write the customer-managed sending status of this tenant's reputation entity,
    /// as `UpdateReputationEntityCustomerManagedStatus` asks for, and stamp when it
    /// was written.
    pub fn set_customer_managed_sending_status(&mut self, status: String, now: SystemTime) {
        self.sending_status = status;
        self.sending_status_updated_at = now;
    }

    /// The modelled `TagList` view of this record's tags.
    pub fn tag_list(&self) -> Vec<crate::ops::types::Tag> {
        self.tags
            .iter()
            .map(|(key, value)| crate::ops::types::Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// One tenant-resource association: a resource (an email identity, a configuration
/// set or an email template, named by its ARN) usable when sending on behalf of the
/// tenant it is associated with.
///
/// The tenant's id is copied in at association time, so the association reports the
/// tenant record it was made against even after a tenant of the same name is
/// recreated.
#[derive(Debug, Clone)]
pub struct TenantResourceAssociationRecord {
    /// The `TenantName` the association was made for, as the request spelled it.
    pub tenant_name: String,
    /// The associated tenant's `TenantId`, read off the tenant record.
    pub tenant_id: String,
    /// The `ResourceArn` the association was made for, as the request spelled it.
    pub resource_arn: String,
    /// When the resource was associated with the tenant.
    pub associated_at: SystemTime,
}

impl TenantResourceAssociationRecord {
    /// The modelled `ResourceTenantMetadata` view of this association, as
    /// `ListResourceTenants` reports it.
    pub fn resource_tenant_view(&self) -> crate::ops::types::ResourceTenantMetadata {
        crate::ops::types::ResourceTenantMetadata {
            tenant_name: Some(self.tenant_name.clone()),
            tenant_id: Some(self.tenant_id.clone()),
            resource_arn: Some(self.resource_arn.clone()),
            associated_timestamp: Some(::aws_smithy_types::DateTime::from(self.associated_at)),
        }
    }

    /// The modelled `TenantResource` view of this association, as
    /// `ListTenantResources` reports it: the resource's ARN and the `ResourceType`
    /// that ARN names, which is read off the ARN rather than stored beside it so one
    /// record cannot disagree with itself about what it points at.
    pub fn tenant_resource_view(&self) -> crate::ops::types::TenantResource {
        crate::ops::types::TenantResource {
            resource_type: tenant_resource_type(&self.resource_arn).map(ToString::to_string),
            resource_arn: Some(self.resource_arn.clone()),
        }
    }
}

/// How this service words `NotFoundException` for a contact list it does not hold.
///
/// One wording for every operation that takes a `ContactListName` — the reads, the
/// writes and the contact operations nested under the list — so a client sees the same
/// sentence whichever call first tells it the list is gone.
pub fn contact_list_not_found(contact_list_name: &str) -> String {
    format!("List with name: {contact_list_name} doesn't exist.")
}

/// How this service words `NotFoundException` for a contact the named list does not
/// hold, stated once for every contact operation.
pub fn contact_not_found(contact_list_name: &str, email_address: &str) -> String {
    format!(
        "Contact with email address: {email_address} doesn't exist in list: \
         {contact_list_name}."
    )
}

/// The status of a multi-region endpoint, from the model's `Status` enum.
///
/// The enum also carries `FAILED` ("the resource failed to be provisioned"), which
/// nothing here reaches: provisioning an emulated endpoint is bookkeeping and cannot
/// fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointStatus {
    /// `CREATING`: the resource is being provisioned.
    Creating,
    /// `READY`: the resource is ready to use.
    Ready,
    /// `DELETING`: the resource is being deleted as requested.
    Deleting,
}

impl EndpointStatus {
    /// The modelled `Status` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Ready => "READY",
            Self::Deleting => "DELETING",
        }
    }
}

/// One multi-region endpoint (global-endpoint): the regions its sending traffic is
/// split between, its provisioning status and its tags.
#[derive(Debug, Clone)]
pub struct MultiRegionEndpointRecord {
    /// The `EndpointName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// The `EndpointId` minted for it at create time, stable for the life of the
    /// endpoint.
    pub endpoint_id: String,
    /// The primary region: the Region the create call executed in, which is also
    /// the only Region the endpoint can be read, listed or deleted from.
    pub primary_region: String,
    /// The secondary regions the create request's route details named, in the order
    /// they were supplied.
    pub secondary_regions: Vec<String>,
    /// The provisioning status.
    pub status: EndpointStatus,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// When the endpoint was created.
    pub created_at: SystemTime,
    /// When the endpoint was last changed.
    pub last_updated_at: SystemTime,
    /// When the `CREATING` -> `READY` transition falls due.
    pub ready_due_at: SystemTime,
    /// When the deletion of this endpoint completes, once one has been requested.
    pub deletion_due_at: Option<SystemTime>,
}

impl MultiRegionEndpointRecord {
    /// An endpoint as `CreateMultiRegionEndpoint` stores it: being provisioned, with
    /// its `READY` transition already due at a known instant.
    pub fn new(
        name: &str,
        endpoint_id: String,
        primary_region: &str,
        secondary_regions: Vec<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            endpoint_id,
            primary_region: primary_region.to_string(),
            secondary_regions,
            status: EndpointStatus::Creating,
            tags: Vec::new(),
            created_at: now,
            last_updated_at: now,
            ready_due_at: now
                .checked_add(ENDPOINT_PROVISIONING_SETTLE)
                .unwrap_or(now),
            deletion_due_at: None,
        }
    }

    /// Start tearing this endpoint down, as `DeleteMultiRegionEndpoint` does: it
    /// carries the non-terminal `DELETING` status until the deletion completes.
    pub fn start_deletion(&mut self, now: SystemTime) {
        self.status = EndpointStatus::Deleting;
        self.last_updated_at = now;
        self.deletion_due_at = Some(
            now.checked_add(ENDPOINT_DELETION_SETTLE)
                .unwrap_or(now),
        );
    }

    /// Whether a deletion of this endpoint is still running at `now`.
    pub fn deletion_in_flight(&self, now: SystemTime) -> bool {
        self.deletion_due_at
            .is_some_and(|due_at| due_at > now)
    }

    /// Whether the deletion of this endpoint has completed at `now`, which is when
    /// the endpoint stops existing at all.
    pub fn deletion_complete(&self, now: SystemTime) -> bool {
        self.deletion_due_at.is_some_and(|due_at| due_at <= now)
    }

    /// Apply the provisioning transition if it is due at `now`.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == EndpointStatus::Creating && now >= self.ready_due_at {
            self.status = EndpointStatus::Ready;
            self.last_updated_at = self.ready_due_at;
        }
    }

    /// The regions traffic is split between: the primary region first, then the
    /// secondary regions the create request named.
    ///
    /// "From the data flow standpoint there is no difference between primary and
    /// secondary regions - sending traffic is divided between the two", so both the
    /// `Regions` a listing reports and the `Routes` a read reports are this one
    /// projection, and a create and the reads after it cannot disagree.
    pub fn regions(&self) -> Vec<String> {
        let mut regions = Vec::with_capacity(1 + self.secondary_regions.len());
        regions.push(self.primary_region.clone());
        regions.extend(self.secondary_regions.iter().cloned());
        regions
    }

    /// The modelled `Routes` view of this record.
    pub fn routes(&self) -> Vec<crate::ops::types::Route> {
        self.regions()
            .into_iter()
            .map(|region| crate::ops::types::Route { region })
            .collect()
    }

    /// The modelled `MultiRegionEndpoint` view of this record, as
    /// `ListMultiRegionEndpoints` reports it.
    pub fn summary(&self) -> crate::ops::types::MultiRegionEndpoint {
        crate::ops::types::MultiRegionEndpoint {
            endpoint_name: Some(self.name.clone()),
            status: Some(self.status.as_str().to_string()),
            endpoint_id: Some(self.endpoint_id.clone()),
            regions: Some(self.regions()),
            created_timestamp: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            last_updated_timestamp: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
        }
    }
}

/// How this service words `NotFoundException` for a multi-region endpoint the Region
/// the call executes in does not hold.
///
/// A read answers it with no message at all — the error code is the whole answer —
/// while a delete of an endpoint that is not there says so
/// ([`multi_region_endpoint_not_found_to_delete`]), so this is the one place that
/// states both.
pub fn multi_region_endpoint_not_found() -> crate::ops::types::NotFoundException {
    crate::ops::types::NotFoundException { message: None }
}

/// How this service words `NotFoundException` when a delete names an endpoint the
/// Region does not hold.
pub fn multi_region_endpoint_not_found_to_delete() -> crate::ops::types::NotFoundException {
    crate::ops::types::NotFoundException {
        message: Some("Endpoint does not exist.".to_string()),
    }
}

/// How this service words the refusal of a delete that collides with a tear-down
/// already running: the refusal names the status the resource is in, which for this
/// refusal is always [`DEPROVISIONING_STATUS`] — the one status a delete cannot act
/// on, because it is the one a delete put the resource into.
pub fn unable_to_delete_while_deprovisioning() -> String {
    format!("Unable to delete resource in {DEPROVISIONING_STATUS} status.")
}

/// One configuration set: the rules a send that names it is subject to.
///
/// Every member is the EFFECTIVE configuration, with the documented default already
/// resolved at create time, so a read reports what a send would be subject to rather
/// than re-deriving a default per reader.
#[derive(Debug, Clone)]
pub struct ConfigurationSetRecord {
    /// The `ConfigurationSetName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// `TrackingOptions`: the custom open/click redirect domain, when the caller
    /// configured one. SES tracks through its own domain otherwise, so an
    /// unconfigured set holds nothing here.
    pub tracking_options: Option<crate::ops::types::TrackingOptions>,
    /// `DeliveryOptions$TlsPolicy`.
    pub tls_policy: String,
    /// `DeliveryOptions$MaxDeliverySeconds`, when the caller bounded delivery.
    pub max_delivery_seconds: Option<i64>,
    /// `ReputationOptions$ReputationMetricsEnabled`.
    pub reputation_metrics_enabled: bool,
    /// `SendingOptions$SendingEnabled`.
    pub sending_enabled: bool,
    /// `SuppressionOptions`, as supplied: a set that configures none falls back to
    /// the tenant or account suppression preferences, so nothing is stored.
    pub suppression_options: Option<crate::ops::types::SuppressionOptions>,
    /// `VdmOptions`, as supplied.
    pub vdm_options: Option<crate::ops::types::VdmOptions>,
    /// `MessageSecurityOptions`, as supplied.
    pub message_security_options: Option<crate::ops::types::MessageSecurityOptions>,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// The event destinations this set publishes its events to, keyed by the
    /// `EventDestinationName` they were created with — a name that "identifies the
    /// event destination within the configuration set", so it is keyed exactly as
    /// supplied and unique only within this set. Ordered, so a read enumerates them
    /// deterministically.
    pub event_destinations: BTreeMap<String, EventDestinationRecord>,
    /// When the set was created.
    pub created_at: SystemTime,
    /// When the set's reputation metrics were last given a fresh start.
    ///
    /// `ReputationOptions$LastFreshStart` is "the date and time when the reputation
    /// metrics were last given a fresh start", and SES documents that where sending
    /// has never been disabled and re-enabled this is the time the configuration set
    /// was created \u2014 so it starts at the create instant and moves only when a fresh
    /// start happens.
    pub last_fresh_start: SystemTime,
}

impl ConfigurationSetRecord {
    /// A set as `CreateConfigurationSet` stores it when the request configures
    /// nothing: SES's own defaults.
    pub fn new(name: &str, now: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            tracking_options: None,
            // `TlsPolicy`: "If the value is Require, messages are only delivered if
            // a TLS connection can be established. If the value is Optional,
            // messages can be delivered in plain text if a TLS connection can't be
            // established." SES delivers opportunistically unless a caller asks for
            // the stricter policy, so a set that configures none is Optional.
            tls_policy: TLS_POLICY_OPTIONAL.to_string(),
            max_delivery_seconds: None,
            // The member's modelled default.
            reputation_metrics_enabled: false,
            // Sending through a configuration set is something a caller PAUSES
            // (`PutConfigurationSetSendingOptions` "enable or disable email sending
            // for messages that use this configuration set"), so a newly created set
            // does not itself hold sending back.
            sending_enabled: true,
            suppression_options: None,
            vdm_options: None,
            message_security_options: None,
            tags: Vec::new(),
            // A set publishes events only where a caller attaches a destination to
            // it, so a newly created set has none.
            event_destinations: BTreeMap::new(),
            created_at: now,
            last_fresh_start: now,
        }
    }

    /// The modelled `DeliveryOptions` view of this record.
    ///
    /// One projection shared by every operation that reports a set's delivery
    /// configuration, so a create and the read after it cannot disagree.
    pub fn delivery_options(&self) -> crate::ops::types::DeliveryOptions {
        crate::ops::types::DeliveryOptions {
            tls_policy: Some(self.tls_policy.clone()),
            // The dedicated IP pool a set sends from is a resource no implemented
            // operation of this service keeps, so no set holds one.
            sending_pool_name: None,
            max_delivery_seconds: self.max_delivery_seconds,
        }
    }

    /// The modelled `ReputationOptions` view of this record.
    pub fn reputation_options(&self) -> crate::ops::types::ReputationOptions {
        crate::ops::types::ReputationOptions {
            reputation_metrics_enabled: self.reputation_metrics_enabled,
            last_fresh_start: Some(::aws_smithy_types::DateTime::from(self.last_fresh_start)),
        }
    }

    /// The modelled `SendingOptions` view of this record.
    pub fn sending_options(&self) -> crate::ops::types::SendingOptions {
        crate::ops::types::SendingOptions {
            sending_enabled: self.sending_enabled,
        }
    }

    /// The modelled `TagList` view of this record's tags.
    pub fn tag_list(&self) -> Vec<crate::ops::types::Tag> {
        self.tags
            .iter()
            .map(|(key, value)| crate::ops::types::Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// One event destination of a configuration set: where the events a send through
/// that set produces are published to, and which of them are.
#[derive(Debug, Clone)]
pub struct EventDestinationRecord {
    /// The `EventDestinationName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// `EventDestinationDefinition$Enabled`: whether SES publishes to this
    /// destination at all.
    pub enabled: bool,
    /// `EventDestinationDefinition$MatchingEventTypes`, in the order supplied: the
    /// event types this destination receives. A definition that named none receives
    /// nothing, which is an empty list rather than an absent member — `EventDestination`
    /// models the member as required.
    pub matching_event_types: Vec<String>,
    /// The Kinesis Data Firehose stream this destination publishes to, when it is one.
    pub kinesis_firehose_destination: Option<crate::ops::types::KinesisFirehoseDestination>,
    /// The CloudWatch dimensions this destination publishes, when it is one.
    pub cloud_watch_destination: Option<crate::ops::types::CloudWatchDestination>,
    /// The SNS topic this destination publishes to, when it is one.
    pub sns_destination: Option<crate::ops::types::SnsDestination>,
    /// The EventBridge event bus this destination publishes to, when it is one.
    pub event_bridge_destination: Option<crate::ops::types::EventBridgeDestination>,
    /// The Pinpoint application this destination publishes to, when it is one.
    pub pinpoint_destination: Option<crate::ops::types::PinpointDestination>,
}

impl EventDestinationRecord {
    /// The destination `name` as the definition in a create or update request
    /// describes it.
    pub fn new(name: &str, definition: crate::ops::types::EventDestinationDefinition) -> Self {
        Self {
            name: name.to_string(),
            enabled: definition.enabled,
            matching_event_types: definition.matching_event_types.unwrap_or_default(),
            kinesis_firehose_destination: definition.kinesis_firehose_destination,
            cloud_watch_destination: definition.cloud_watch_destination,
            sns_destination: definition.sns_destination,
            event_bridge_destination: definition.event_bridge_destination,
            pinpoint_destination: definition.pinpoint_destination,
        }
    }

    /// Make `definition` this destination's configuration.
    ///
    /// `UpdateConfigurationSetEventDestination` "update[s] the configuration of an
    /// event destination", and the `EventDestinationDefinition` it requires is a
    /// whole definition of that destination — which services receive events from the
    /// set and which event types they receive — so afterwards the destination
    /// publishes what the definition describes and nothing else. The name is not part
    /// of the definition: it identifies the destination within the set and an update
    /// addresses the destination by it, so it is kept.
    pub fn replace_definition(&mut self, definition: crate::ops::types::EventDestinationDefinition) {
        let name = self.name.clone();
        *self = Self::new(&name, definition);
    }

    /// The modelled `EventDestination` view of this record.
    ///
    /// One projection shared by every operation that reports a set's event
    /// destinations, so a create and the read after it cannot disagree.
    pub fn view(&self) -> crate::ops::types::EventDestination {
        crate::ops::types::EventDestination {
            name: self.name.clone(),
            enabled: self.enabled,
            matching_event_types: self.matching_event_types.clone(),
            kinesis_firehose_destination: self.kinesis_firehose_destination.clone(),
            cloud_watch_destination: self.cloud_watch_destination.clone(),
            sns_destination: self.sns_destination.clone(),
            event_bridge_destination: self.event_bridge_destination.clone(),
            pinpoint_destination: self.pinpoint_destination.clone(),
        }
    }
}

/// How this service words `AlreadyExistsException` for an event-destination name a
/// configuration set already holds, stated once for every operation that attaches
/// one.
pub fn event_destination_already_exists(
    configuration_set_name: &str,
    event_destination_name: &str,
) -> String {
    format!(
        "Event destination {event_destination_name} already exists in configuration set \
         {configuration_set_name}."
    )
}

/// How this service words `NotFoundException` for an event-destination name a
/// configuration set does not hold.
///
/// One wording for every operation that addresses a destination within a set, so a
/// client sees the same sentence whichever call first tells it the destination is
/// not there.
pub fn event_destination_not_found(
    configuration_set_name: &str,
    event_destination_name: &str,
) -> String {
    format!(
        "Event destination {event_destination_name} does not exist in configuration set \
         {configuration_set_name}."
    )
}

/// `definition` once every enum-valued member in it carries one of the model's own
/// values for that member, and the message naming the offending member otherwise.
///
/// One check for every operation that takes an `EventDestinationDefinition`, so the
/// create of a destination and an update of the same destination cannot accept
/// different values. The value sets are the generated consts, so they follow the
/// model.
pub fn checked_event_destination_definition(
    definition: crate::ops::types::EventDestinationDefinition,
) -> Result<crate::ops::types::EventDestinationDefinition, String> {
    let matching_event_types = match definition.matching_event_types {
        Some(event_types) => Some(
            event_types
                .into_iter()
                .map(|event_type| {
                    modelled_enum_value(
                        "MatchingEventTypes",
                        event_type,
                        crate::ops::types::EVENT_TYPE_VALUES,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        None => None,
    };
    let cloud_watch_destination = match definition.cloud_watch_destination {
        Some(destination) => Some(crate::ops::types::CloudWatchDestination {
            dimension_configurations: destination
                .dimension_configurations
                .into_iter()
                .map(|dimension| {
                    Ok(crate::ops::types::CloudWatchDimensionConfiguration {
                        dimension_value_source: modelled_enum_value(
                            "CloudWatchDestination.DimensionConfigurations.DimensionValueSource",
                            dimension.dimension_value_source,
                            crate::ops::types::DIMENSION_VALUE_SOURCE_VALUES,
                        )?,
                        ..dimension
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        }),
        None => None,
    };
    Ok(crate::ops::types::EventDestinationDefinition {
        matching_event_types,
        cloud_watch_destination,
        ..definition
    })
}

/// `value` when it is one of `allowed`, and the refusal naming `member` otherwise.
///
/// The wording an operation lifts into its own input error, so every member of this
/// service refuses an unmodelled value the same way.
pub fn modelled_enum_value(
    member: &str,
    value: String,
    allowed: &[&str],
) -> Result<String, String> {
    if allowed.contains(&value.as_str()) {
        return Ok(value);
    }
    Err(format!(
        "{member} {value} is not one of {}.",
        allowed.join(", ")
    ))
}

/// The `TlsPolicy` value a configuration set holds when the caller configured none.
pub const TLS_POLICY_OPTIONAL: &str = "OPTIONAL";

/// Whether `name` is a `ConfigurationSetName` SES accepts.
///
/// From the member's documentation: "The name can contain up to 64 alphanumeric
/// characters, including letters, numbers, hyphens (-) and underscores (_) only" —
/// so a name is a non-empty run of at most [`MAX_CONFIGURATION_SET_NAME_LEN`] ASCII
/// letters, digits, hyphens and underscores.
pub fn configuration_set_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_CONFIGURATION_SET_NAME_LEN
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

/// How this service words the refusal of a `ConfigurationSetName` outside the
/// documented character and length rule, stated once for every operation that takes
/// one.
pub fn configuration_set_name_invalid(configuration_set_name: &str) -> String {
    format!(
        "Configuration set name {configuration_set_name} is invalid. The name can contain up to \
         {MAX_CONFIGURATION_SET_NAME_LEN} alphanumeric characters, including letters, numbers, \
         hyphens (-) and underscores (_) only."
    )
}

/// How this service words `NotFoundException` for a configuration set it does not
/// hold.
///
/// One wording for every operation that takes a `ConfigurationSetName`, so a client
/// sees the same sentence whichever call first tells it the set is not there. SES
/// delimits the name it interpolated with angle brackets, so the sentence stays
/// readable for a name that is itself a sentence fragment.
pub fn configuration_set_not_found(configuration_set_name: &str) -> String {
    format!("Configuration set <{configuration_set_name}> does not exist.")
}

/// One address on a suppression list.
///
/// SES keeps a suppression list per account and, when a tenant configures one, per
/// tenant; an entry carries the reason the address was suppressed, when the entry
/// was last updated, and optionally what caused it.
#[derive(Debug, Clone)]
pub struct SuppressedDestinationRecord {
    /// `SuppressedDestination$EmailAddress`, as the request that added the entry
    /// spelled it: the local part belongs to the receiving mailbox, so a response
    /// reports it unchanged.
    pub email_address: String,
    /// `SuppressedDestination$Reason`: the `SuppressionListReason` the address was
    /// suppressed for.
    pub reason: String,
    /// `SuppressedDestination$LastUpdateTime`: when the entry was last written.
    pub last_update_time: SystemTime,
    /// `SuppressedDestinationAttributes$MessageId`: the message that caused the
    /// suppression, when one did.
    pub message_id: Option<String>,
    /// `SuppressedDestinationAttributes$FeedbackId`: the identifier generated when
    /// the address was added, when the entry carries one.
    pub feedback_id: Option<String>,
    /// The tenant whose suppression list holds this entry; `None` for the
    /// account-level list. `SuppressedDestination$TenantName` "is present only when
    /// the suppressed destination is on a tenant's suppression list".
    pub tenant_name: Option<String>,
}

impl SuppressedDestinationRecord {
    /// The modelled `SuppressedDestination` view of this record.
    ///
    /// One projection shared by every operation that reports a suppressed address,
    /// so a write and the read after it cannot disagree.
    pub fn detail(&self) -> crate::ops::types::SuppressedDestination {
        crate::ops::types::SuppressedDestination {
            email_address: self.email_address.clone(),
            reason: self.reason.clone(),
            last_update_time: ::aws_smithy_types::DateTime::from(self.last_update_time),
            // "An optional value that can contain additional information about the
            // reasons that the address was added": an entry that knows neither the
            // message nor the feedback id reports no attributes at all, rather than
            // an object whose members are all absent.
            attributes: (self.message_id.is_some() || self.feedback_id.is_some()).then(|| {
                crate::ops::types::SuppressedDestinationAttributes {
                    message_id: self.message_id.clone(),
                    feedback_id: self.feedback_id.clone(),
                }
            }),
            tenant_name: self.tenant_name.clone(),
        }
    }

    /// The modelled `SuppressedDestinationSummary` view of this record.
    ///
    /// The summary is the listing's projection of the same stored entry
    /// [`Self::detail`] reports, so a listed address and the read after it cannot
    /// disagree. `SuppressedDestinationSummary` models only the address, the reason
    /// and the last-update instant — the list the entry belongs to is the one the
    /// request selected, so it is not repeated per summary.
    pub fn summary(&self) -> crate::ops::types::SuppressedDestinationSummary {
        crate::ops::types::SuppressedDestinationSummary {
            email_address: self.email_address.clone(),
            reason: self.reason.clone(),
            last_update_time: ::aws_smithy_types::DateTime::from(self.last_update_time),
        }
    }

    /// Whether this entry's `LastUpdateTime` lies in the window `start`..`end`, each
    /// bound given as epoch seconds and each absent bound leaving that side open.
    ///
    /// The bounds come from `ListSuppressedDestinationsRequest$StartDate` ("only
    /// includes addresses that were added to the list before a specific date" for
    /// `$EndDate`, "after a specific date" for `$StartDate`). Neither the model nor
    /// that wording settles an entry stamped exactly ON a bound, so the reading is
    /// CHOSEN: the window is CLOSED, since a bound names an instant the caller asked
    /// about and an entry stamped there is one the caller asked for.
    pub fn updated_within(&self, start: Option<f64>, end: Option<f64>) -> bool {
        let at = ::aws_smithy_types::DateTime::from(self.last_update_time).as_secs_f64();
        start.is_none_or(|from| at >= from) && end.is_none_or(|to| at <= to)
    }
}

/// The key a suppression-list entry is stored under: the list it belongs to — a
/// tenant's, or the account's when `tenant_name` is `None` — and the address within
/// it.
///
/// The address part is its [`identity_key`], so a lookup matches the domain without
/// regard to case while keeping the local part exactly as supplied.
///
/// The list part is LENGTH-PREFIXED: the number of bytes in the list's name, the
/// separator, then the name itself. Where the split between the two parts falls is
/// therefore decided by a count this function writes, not by a character the name is
/// assumed not to contain, so the key is one-to-one with the (list, address) pair
/// whatever either part holds — no name can collide with another list's keys or forge
/// another list's prefix.
pub fn suppression_key(tenant_name: Option<&str>, email_address: &str) -> String {
    format!(
        "{}{}",
        suppression_list_prefix(tenant_name),
        identity_key(email_address)
    )
}

/// The key prefix every entry of ONE suppression list shares: the length and the name
/// of the tenant whose list it is (the empty name for the account-level list), joined
/// by the separator from [`suppression_key`].
///
/// A key carries this prefix only if it belongs to this very list — a different name
/// either differs in the length that opens the prefix or, at equal length, in the name
/// itself — which is what lets a listing tell its own page token from one minted for a
/// different list.
///
/// Every entry of one list shares the prefix, so within a list the keys are ordered by
/// address, which is the order a page walks. Across lists they group by name length
/// rather than alphabetically; no listing depends on that, since each enumerates the
/// one list its request selects.
pub fn suppression_list_prefix(tenant_name: Option<&str>) -> String {
    let name = tenant_name.unwrap_or_default();
    format!("{}\u{1f}{name}", name.len())
}

/// How this service words `NotFoundException` for an address that is not on the
/// suppression list being read.
///
/// One wording for every operation that looks an address up on a suppression list,
/// so a client sees the same sentence whichever call first tells it the address is
/// not suppressed.
pub fn suppressed_destination_not_found(email_address: &str) -> String {
    format!("Email address {email_address} does not exist on your suppression list.")
}

/// How this service words `InvalidNextTokenException` for a `NextToken` that is not
/// a position in the listing being read.
///
/// The sentence carries no part of the request: a token is refused for being
/// something this listing never handed out, and there is nothing else to say about
/// it. One wording for every listing that models the error.
pub fn invalid_next_token() -> String {
    "Token is invalid.".to_string()
}

/// One accepted outbound message.
#[derive(Debug, Clone)]
pub struct SentMessage {
    /// The `MessageId` returned to the caller.
    pub message_id: String,
    /// The `From` address the message was accepted for.
    pub from: Option<String>,
    /// `Destination$ToAddresses`.
    pub to: Vec<String>,
    /// `Destination$CcAddresses`.
    pub cc: Vec<String>,
    /// `Destination$BccAddresses`.
    pub bcc: Vec<String>,
    /// The subject, for a simple message.
    pub subject: Option<String>,
    /// The body: the text part of a simple message, or the raw MIME data.
    pub body: Option<String>,
    /// The HTML part of a simple message, when the request carried one. A simple
    /// message may carry both parts, and the log records what was accepted.
    pub body_html: Option<String>,
    /// The configuration set the message was sent with.
    pub configuration_set_name: Option<String>,
    /// `ReplyToAddresses`.
    pub reply_to: Vec<String>,
    /// `FeedbackForwardingEmailAddress`: where the caller asked for bounce and
    /// complaint notifications. Nothing here delivers a bounce, so it is recorded
    /// as part of what was accepted rather than acted on.
    pub feedback_forwarding: Option<String>,
    /// `EmailTags`, in the order they were supplied.
    pub email_tags: Vec<(String, String)>,
    /// When the message was accepted.
    pub sent_at: SystemTime,
}

impl SentMessage {
    /// Every recipient of the message, in `To`, `Cc`, `Bcc` order. SES counts one
    /// recipient as one message against the account's sending quota.
    pub fn recipients(&self) -> impl Iterator<Item = &String> {
        self.to.iter().chain(self.cc.iter()).chain(self.bcc.iter())
    }
}

/// Per-account send counters.
#[derive(Debug, Clone, Copy)]
pub struct SendQuotaState {
    /// Recipients accepted within the trailing [`SEND_QUOTA_WINDOW`]. A view of
    /// the state's outbound-message log, recomputed by
    /// [`ComAmazonawsSesv2SimpleemailserviceV2State::settle`] rather than tallied,
    /// so it frees up as the window moves.
    pub sent_last_24h: f64,
    /// The account's 24-hour sending quota.
    pub max_24h: f64,
    /// The account's maximum send rate, in messages per second. Recorded as part
    /// of the account's limits for the reporting operations that answer it
    /// (`GetAccount`); no per-second throttle is applied anywhere in this slice.
    pub max_send_rate: f64,
}

impl SendQuotaState {
    /// The modelled `SendQuota` view of these counters, as the operations that
    /// report the account's sending limits answer it.
    pub fn send_quota(&self) -> crate::ops::types::SendQuota {
        crate::ops::types::SendQuota {
            max24_hour_send: self.max_24h,
            max_send_rate: self.max_send_rate,
            sent_last24_hours: self.sent_last_24h,
        }
    }
}

impl Default for SendQuotaState {
    /// The limits a new account has in the SES sandbox: 200 messages per 24-hour
    /// period at 1 message per second.
    fn default() -> Self {
        Self {
            sent_last_24h: 0.0,
            max_24h: 200.0,
            max_send_rate: 1.0,
        }
    }
}

/// The reputation status of an account nothing has acted against.
///
/// `GetAccount`'s `EnforcementStatus` documents it as "HEALTHY – There are no
/// reputation-related issues that currently impact your account", which is the
/// status of an account whose sending nobody has reviewed or paused.
pub const ENFORCEMENT_STATUS_HEALTHY: &str = "HEALTHY";

/// `ReviewStatus`'s value for a production-access request still under review, as
/// the model spells it (see `crate::ops::types::REVIEW_STATUS_VALUES`).
///
/// `ReviewDetails$Status` documents it as "PENDING – We have received your appeal
/// and are in the process of reviewing it".
pub const REVIEW_STATUS_PENDING: &str = "PENDING";

/// `ReviewStatus`'s value for a production-access request that has been decided in
/// the account's favour: "GRANTED – Your appeal has been reviewed and your
/// production access has been granted".
pub const REVIEW_STATUS_GRANTED: &str = "GRANTED";

/// `FeatureStatus`'s value for a feature that is on, as the model spells it (see
/// `crate::ops::types::FEATURE_STATUS_VALUES`).
pub const FEATURE_ENABLED: &str = "ENABLED";

/// `FeatureStatus`'s value for a feature that is off.
pub const FEATURE_DISABLED: &str = "DISABLED";

/// `PricingPlan`'s value for an account on no paid plan, one of the four values
/// `PutAccountPricingAttributesRequest$Plan` documents.
pub const PRICING_PLAN_NONE: &str = "NONE";

/// The account-level attributes of one SES account in one Region.
///
/// A single record rather than a table: every member of it is per account and
/// Region, and the emulator serves one account in one Region, so the account
/// operations all read and write this one record.
#[derive(Debug, Clone)]
pub struct AccountState {
    /// `SendingEnabled`: whether email sending is enabled for the account.
    pub sending_enabled: bool,
    /// `DedicatedIpAutoWarmupEnabled`: whether dedicated IPs associated with the
    /// account are warmed up automatically.
    pub dedicated_ip_auto_warmup_enabled: bool,
    /// `ProductionAccessEnabled`: false while the account is in the sandbox, where
    /// it may only send to verified identities.
    pub production_access_enabled: bool,
    /// `EnforcementStatus`: the account's reputation status.
    pub enforcement_status: String,
    /// `SuppressionAttributes$SuppressedReasons`: the reasons for which an address
    /// is added to the account-level suppression list automatically.
    pub suppressed_reasons: Vec<String>,
    /// `SuppressionAttributes$ValidationAttributes`, once a request has set them.
    pub suppression_validation: Option<crate::ops::types::SuppressionValidationAttributes>,
    /// `VdmAttributes$VdmEnabled`: the status of the account's VDM configuration.
    pub vdm_enabled: String,
    /// `VdmAttributes$DashboardAttributes`, once a request has carried them, held
    /// as supplied so a read reports exactly what was written.
    pub vdm_dashboard_attributes: Option<crate::ops::types::DashboardAttributes>,
    /// `VdmAttributes$GuardianAttributes`, once a request has carried them, held as
    /// supplied for the same reason.
    pub vdm_guardian_attributes: Option<crate::ops::types::GuardianAttributes>,
    /// `PricingAttributes$CurrentPlan`: the plan currently active on the account.
    pub pricing_plan: String,
    /// `PricingAttributes$NextPlan`: the plan a scheduled change will activate.
    /// `NextPlan` "is empty when no scheduled change is pending", so it is absent
    /// until a change is scheduled.
    pub next_pricing_plan: Option<String>,
    /// `Details`: the account details a request supplied, absent until one does.
    pub details: Option<crate::ops::types::AccountDetails>,
    /// When the decision on a production-access request that is still under review
    /// falls due, and `None` when no request is under review. The request is
    /// reported meanwhile as `Details$ReviewDetails` with the `PENDING` status;
    /// [`Self::settle`] is what turns it into the decision.
    pub production_access_review_due_at: Option<SystemTime>,
    /// Monotonic counter feeding [`Self::next_review_case_id`], so two
    /// production-access requests made in the same millisecond still get different
    /// case ids.
    pub review_case_sequence: u64,
}

impl Default for AccountState {
    /// The attributes of an account nobody has configured yet.
    ///
    /// Email sending and dedicated-IP auto warm-up are on — SES turns both on for
    /// an account and the account operations are what switch them off — the account
    /// is in the sandbox (which is what the sandbox [`SendQuotaState::default`]
    /// limits go with), its reputation is unenforced, and the account-level
    /// suppression list is on for both of its documented reasons. VDM is off until
    /// a request enables it, no pricing plan is active, and no account details have
    /// been supplied.
    fn default() -> Self {
        Self {
            sending_enabled: true,
            dedicated_ip_auto_warmup_enabled: true,
            production_access_enabled: false,
            enforcement_status: ENFORCEMENT_STATUS_HEALTHY.to_string(),
            suppressed_reasons: crate::ops::types::SUPPRESSION_LIST_REASON_VALUES
                .iter()
                .map(|reason| (*reason).to_string())
                .collect(),
            suppression_validation: None,
            vdm_enabled: FEATURE_DISABLED.to_string(),
            vdm_dashboard_attributes: None,
            vdm_guardian_attributes: None,
            pricing_plan: PRICING_PLAN_NONE.to_string(),
            next_pricing_plan: None,
            details: None,
            production_access_review_due_at: None,
            review_case_sequence: 0,
        }
    }
}

impl AccountState {
    /// The modelled `SuppressionAttributes` view of this record.
    ///
    /// One projection shared by every operation that reports the account's
    /// suppression preferences, so a write and the read after it cannot disagree.
    pub fn suppression_attributes(&self) -> crate::ops::types::SuppressionAttributes {
        crate::ops::types::SuppressionAttributes {
            suppressed_reasons: Some(self.suppressed_reasons.clone()),
            validation_attributes: self.suppression_validation.clone(),
        }
    }

    /// The modelled `VdmAttributes` view of this record.
    ///
    /// The nested dashboard and guardian attributes carry one feature status each,
    /// and a sub-attribute nothing has carried is reported as absent rather than as
    /// a status this account does not hold.
    pub fn vdm_attributes(&self) -> crate::ops::types::VdmAttributes {
        crate::ops::types::VdmAttributes {
            vdm_enabled: self.vdm_enabled.clone(),
            dashboard_attributes: self.vdm_dashboard_attributes.clone(),
            guardian_attributes: self.vdm_guardian_attributes.clone(),
        }
    }

    /// The modelled `PricingAttributes` view of this record.
    pub fn pricing_attributes(&self) -> crate::ops::types::PricingAttributes {
        crate::ops::types::PricingAttributes {
            current_plan: Some(self.pricing_plan.clone()),
            next_plan: self.next_pricing_plan.clone(),
        }
    }

    /// Whether a production-access request is still under review at `now`.
    ///
    /// A request whose decision is already due is not under review any more, even
    /// if nothing has read the account since: [`Self::settle`] is what records the
    /// decision, and it is owed at the same instant either way.
    pub fn production_access_review_pending(&self, now: SystemTime) -> bool {
        self.production_access_review_due_at
            .is_some_and(|due_at| now < due_at)
    }

    /// Record the decision the clock owes a production-access request at `now`.
    ///
    /// The review of a request SES has received ends in a decision, so the request
    /// is reported as `PENDING` only until its decision falls due; from then on the
    /// account reports the granted access and the review that granted it, whether or
    /// not anybody called in between. The case the request opened keeps its id: the
    /// decision is on that case, not on a new one.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due_at) = self.production_access_review_due_at else {
            return;
        };
        if now < due_at {
            return;
        }
        self.production_access_review_due_at = None;
        self.production_access_enabled = true;
        if let Some(review) = self
            .details
            .as_mut()
            .and_then(|details| details.review_details.as_mut())
        {
            review.status = Some(REVIEW_STATUS_GRANTED.to_string());
        }
    }

    /// Mint the id of the support case a production-access request opens.
    ///
    /// `ReviewDetails$CaseId` is documented only as "the associated support center
    /// case ID (if any)" and carries no modelled format, so the shape mirrors the
    /// case id AWS Support hands back while the only guarantee this makes is the one
    /// a client depends on: the id identifies one request and is stable for as long
    /// as the account reports that request's review (it is minted once, when the
    /// request is submitted, and read back from the record afterwards). Uniqueness is
    /// structural — the monotonic sequence number is part of the id — so two requests
    /// submitted in the same millisecond still differ.
    pub fn next_review_case_id(&mut self, account_id: &str, now: SystemTime) -> String {
        self.review_case_sequence += 1;
        let millis = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let sequence = self.review_case_sequence;
        format!("case-{account_id}-{millis:012x}-{sequence:08x}")
    }
}

/// The addr-spec of a `From`-style address.
///
/// An email address member may carry RFC 5322's display-name form, `Sender Name
/// <sender@example.com>`, which names the same mailbox as the bare address: the
/// mailbox is what the identity table is keyed by, so the angle-bracketed
/// addr-spec is what a lookup compares. A value with no angle brackets is already
/// an addr-spec.
pub fn addr_spec(address: &str) -> &str {
    match (address.rfind('<'), address.rfind('>')) {
        (Some(open), Some(close)) if open < close => address[open + 1..close].trim(),
        _ => address.trim(),
    }
}

/// How this service words `NotFoundException` for an email identity it does not
/// hold.
///
/// SES quotes the value it refuses in angle brackets here, and this is the sentence
/// shared by the identity readers, the delete and the certificate operations, so a
/// client sees the same wording whichever of them first tells it the identity is
/// gone. Two operations are deliberately NOT routed through it:
/// `PutEmailIdentityFeedbackAttributes` and
/// `PutEmailIdentityConfigurationSetAttributes` answer an unbracketed sentence,
/// which is what their own recorded responses carry, so each words it in its own
/// handler.
pub fn email_identity_not_found(identity: &str) -> String {
    format!("Email identity <{identity}> does not exist.")
}

/// The key an identity is stored and looked up under.
///
/// A domain name is case-insensitive — DNS compares labels without regard to case
/// — so `Example.COM` and `example.com` name one identity, and an address is
/// authorised by the identity for its domain whatever case the request spells the
/// domain in. The local part of an address belongs to the receiving mailbox, so it
/// is kept exactly as supplied, as is the whole value on the record, which is what
/// every response reports.
pub fn identity_key(identity: &str) -> String {
    let identity = addr_spec(identity);
    match identity.rsplit_once('@') {
        Some((local, domain)) => format!("{local}@{}", domain.to_lowercase()),
        None => identity.to_lowercase(),
    }
}

/// Whether `address` is an address in `domain` or in one of its subdomains.
///
/// Both arguments are [`identity_key`]s, so the domain parts compare the way DNS
/// compares them. A domain covers `user@its.own.name` and `user@any.subdomain.of.it`,
/// and nothing else — `example.com` does not cover `notexample.com`.
pub fn address_in_domain(domain: &str, address: &str) -> bool {
    let Some((local, address_domain)) = address.rsplit_once('@') else {
        return false;
    };
    if local.is_empty() {
        return false;
    }
    address_domain == domain
        || address_domain
            .strip_suffix(domain)
            // The remainder is the subdomain part, so it ends at the separating dot
            // and must carry at least one label character before it: `.example.com`
            // names no subdomain of `example.com`.
            .is_some_and(|parent| parent.ends_with('.') && parent.len() > 1)
}

/// The Easy DKIM CNAME tokens SES publishes for `domain`.
///
/// `DkimAttributes$Tokens` is documented as "a set of unique strings" the caller
/// turns into CNAME records, with no format of its own, so each token here is a
/// digest of the domain and the token's index: distinct per domain and per index,
/// and stable for as long as the identity holds it (the set is computed once, at
/// create, and then stored), which is what a client adding CNAME records depends
/// on. Two 64-bit digests make up the [`EASY_DKIM_TOKEN_LEN`] hex characters.
///
/// The digest is `std`'s, whose output is explicitly NOT guaranteed to be the same
/// across Rust releases — nothing here relies on that: a token set is computed once,
/// at create, and read back from the record afterwards, so no caller and no test may
/// assert a literal token value.
pub fn easy_dkim_tokens(domain: &str) -> Vec<String> {
    (0..EASY_DKIM_TOKEN_COUNT)
        .map(|index| {
            let mut token = String::with_capacity(EASY_DKIM_TOKEN_LEN);
            for salt in [0_u64, u64::from_be_bytes(*b"EasyDKIM")] {
                let mut hasher = DefaultHasher::new();
                salt.hash(&mut hasher);
                domain.hash(&mut hasher);
                index.hash(&mut hasher);
                write!(token, "{:016x}", hasher.finish())
                    .expect("writing to a String cannot fail");
            }
            token
        })
        .collect()
}

/// What a `NextToken` of this service's own making starts with, so a token from
/// anywhere else — including one that happens to be valid base64 — is detectably
/// not a position in this listing.
const PAGE_TOKEN_TAG: &str = "sesv2:email-identities:";

/// The tag `ListContactLists`'s own tokens carry.
pub const CONTACT_LISTS_PAGE_TOKEN_TAG: &str = "sesv2:contact-lists:";

/// The tag `ListContacts`'s own tokens carry.
pub const CONTACTS_PAGE_TOKEN_TAG: &str = "sesv2:contacts:";

/// The tag `ListEmailIdentityCertificates`'s own tokens carry.
pub const IDENTITY_CERTIFICATES_PAGE_TOKEN_TAG: &str = "sesv2:identity-certificates:";

/// The tag `ListSuppressedDestinations`'s own tokens carry.
pub const SUPPRESSED_DESTINATIONS_PAGE_TOKEN_TAG: &str = "sesv2:suppressed-destinations:";

/// The tag `ListMultiRegionEndpoints`'s own tokens carry.
pub const MULTI_REGION_ENDPOINTS_PAGE_TOKEN_TAG: &str = "sesv2:multi-region-endpoints:";

/// The tag `ListReputationEntities`'s own tokens carry.
pub const REPUTATION_ENTITIES_PAGE_TOKEN_TAG: &str = "sesv2:reputation-entities:";

/// The tag `ListResourceTenants`'s own tokens carry.
pub const RESOURCE_TENANTS_PAGE_TOKEN_TAG: &str = "sesv2:resource-tenants:";

/// The tag `ListTenantResources`'s own tokens carry.
pub const TENANT_RESOURCES_PAGE_TOKEN_TAG: &str = "sesv2:tenant-resources:";

/// The tag `ListTenants`'s own tokens carry.
pub const TENANTS_PAGE_TOKEN_TAG: &str = "sesv2:tenants:";

/// Encode a `NextToken` that resumes the listing tagged `tag` after `last_returned`.
///
/// The token is opaque to the caller: it is the listing's tag and the last key
/// returned, base64'd — decodable by [`decode_tagged_page_token`] under the same tag
/// and by nothing else, so one listing's token is not a position in another's.
pub fn encode_tagged_page_token(tag: &str, last_returned: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{tag}{last_returned}"))
}

/// Decode a `NextToken` produced by [`encode_tagged_page_token`] under `tag`. `None`
/// when the caller made the token up, or when it belongs to another listing.
pub fn decode_tagged_page_token(tag: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded.strip_prefix(tag).map(ToString::to_string)
}

/// Encode a `ListEmailIdentities` `NextToken` that resumes after `last_returned`.
pub fn encode_page_token(last_returned: &str) -> String {
    encode_tagged_page_token(PAGE_TOKEN_TAG, last_returned)
}

/// Decode a `NextToken` produced by [`encode_page_token`]. `None` when the caller
/// made the token up.
pub fn decode_page_token(token: &str) -> Option<String> {
    decode_tagged_page_token(PAGE_TOKEN_TAG, token)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// `None` means the engine is not the shared in-memory engine, so this service has
/// no store at all; a handler answers that with the scaffold's 501, never with a
/// fresh engine and never with a modelled error that would read as a real service
/// state (a throttle an SDK retries, say).
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSesv2SimpleemailserviceV2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSesv2SimpleemailserviceV2State>)
}
