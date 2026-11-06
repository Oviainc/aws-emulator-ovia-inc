// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudwatchomni-cloudwatchomnifrontend service.
//!
//! One store holds the service's resource hierarchy — a domain, the spaces that
//! belong to it, and the access profiles and dashboards those spaces hold — plus the
//! small amount of bookkeeping the modelled contract needs:
//! a name index for the duplicate-name conflict, the idempotency-token maps for the
//! `@idempotencyToken` creates, and a monotonic sequence so list pagination is
//! deterministic across pages.
//!
//! The derivation helpers below (ids, ARNs, endpoint URLs, timestamps, the opaque
//! page token) live here rather than in one operation's file because several
//! operations read them, and because every one of them is a property of the STORE
//! rather than of a single request.

use std::collections::HashMap;

use crate::ops::types::{
    AccessGrant, AccessProfile, Domain, EncryptionConfiguration, IdentityProviderConfiguration,
    OmniDashboard, Space, SpaceSummary,
};

/// The region a resource is recorded in when the request carries no SigV4
/// credential scope to read one from (an unsigned, in-process call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace — `awsService.arnNamespace` in the Smithy model
/// (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "cloudwatch";

/// This service's endpoint prefix — `awsService.endpointPrefix` in the Smithy model.
pub const ENDPOINT_PREFIX: &str = "cloudwatch-omni";

/// The only value `DomainStatus` declares, so a persisted domain always reports it.
pub const DOMAIN_STATUS_ACTIVE: &str = "ACTIVE";

/// `SpaceStatus` declares ACTIVE, SUSPENDED and MOVING — none of them a transient
/// "creating" state, so a created space is recorded ready.
pub const SPACE_STATUS_ACTIVE: &str = "ACTIVE";

/// `AssumeStatus`'s value for a principal that MAY assume an access profile — its
/// model doc: "The calling principal is authorized to assume the access profile."
pub const ASSUME_STATUS_ALLOWED: &str = "ALLOWED";

/// `AssumeStatus`'s value for a principal that may NOT — its model doc: "The calling
/// principal is not authorized to assume the access profile."
pub const ASSUME_STATUS_DENIED: &str = "DENIED";

/// `AccessProfileType`'s value for a profile a customer created, which is what every
/// profile created through this API is: the model doc for the value reads "Created and
/// owned by a customer", against SERVICE_MANAGED's "Provisioned and owned by the
/// service. Cannot be deleted by customers."
pub const ACCESS_PROFILE_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// `EncryptionStrategy`'s value for a resource encrypted with an AWS owned key.
pub const ENCRYPTION_STRATEGY_AWS_OWNED: &str = "AWS_OWNED";

/// `EncryptionStrategy`'s value for a resource encrypted with a customer managed key.
pub const ENCRYPTION_STRATEGY_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// `IdentityProvider`'s value for an AWS Identity Center domain.
pub const IDENTITY_PROVIDER_IDC: &str = "IDC";

/// The page bound a list operation applies when the request names no `maxResults`.
///
/// `ListSpaces`'s `maxResults` model doc: "The maximum number of spaces to return per
/// page. Defaults to 100." That is also the maximum its `@range(min: 1, max: 100)` allows.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// How many domains the store holds before `CreateDomain` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// CloudWatchOmni publishes an adjustable quota on domains per account per Region, and
/// this emulator models neither Service Quotas nor an account's adjustments — so this
/// is the emulator's OWN bound on the store rather than a claim about any account's
/// limit. It is set far above any workflow an emulator serves, and it is what stops
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::domains`] growing without
/// end. It bounds `domain_name_index` and `create_domain_tokens` with it: both are
/// keyed one-to-one to a live domain and are forgotten by `DeleteDomain`, so neither
/// can outgrow this number.
pub const DOMAIN_QUOTA: usize = 1_000;

/// How many spaces the store holds before `CreateSpace` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// The emulator's OWN bound, for the same reason as [`DOMAIN_QUOTA`]. It bounds
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::spaces`] and, with it,
/// `create_space_tokens` (one entry per live space, forgotten by `DeleteSpace`) — and
/// therefore also the per-request work [`spaces_after`] and [`domain_has_spaces`] do
/// under the store lock, which is linear in the number of stored spaces.
///
/// [`spaces_after`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::spaces_after
/// [`domain_has_spaces`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::domain_has_spaces
pub const SPACE_QUOTA: usize = 10_000;

/// How many access profiles the store holds before `CreateAccessProfile` refuses with
/// the modelled `ServiceQuotaExceededException`.
///
/// The emulator's OWN bound, for the same reason as [`DOMAIN_QUOTA`]. It bounds
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::access_profiles`] and, with
/// it, `create_access_profile_tokens` (one entry per live profile) — and therefore the
/// per-request work [`access_profile_named`] does under the store lock, which is linear
/// in the number of stored profiles.
///
/// [`access_profile_named`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::access_profile_named
pub const ACCESS_PROFILE_QUOTA: usize = 10_000;

/// `AccessGrantType`'s value for a grant a customer created, which is what every grant
/// created through this API is: the model doc for the value reads "Created and owned by
/// a customer", against SERVICE_MANAGED's "Provisioned and owned by the service. Cannot
/// be deleted by customers."
pub const ACCESS_GRANT_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// `AccessGrantPermission`'s value whose model doc reads "Access limited to an explicit
/// list of actions, specified in scopedActions" — the one permission that cannot stand
/// without those actions.
pub const ACCESS_GRANT_PERMISSION_CUSTOM: &str = "CUSTOM";

/// `AccessGrantPrincipalType`'s value for an Identity Center user — the only principal
/// type `AccessGrantPrincipal.principalAttributes` supports ("Supported only for
/// IDC_USER principals").
pub const ACCESS_GRANT_PRINCIPAL_TYPE_IDC_USER: &str = "IDC_USER";

/// The resource type name that names an access profile in a `ResourceScope`.
///
/// `ResourceScope.resourceType`'s model doc is "Resource type name (e.g., \"DataSet\",
/// \"OmniDashboard\")" — the modelled shape's own name, so an access profile is named by
/// `AccessProfile`. It is also the `resourceType` this service's `ResourceNotFound`
/// refusals report for a profile.
pub const ACCESS_PROFILE_RESOURCE_TYPE: &str = "AccessProfile";

/// How many access grants the store holds before `CreateAccessGrant` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// The emulator's OWN bound, for the same reason as [`DOMAIN_QUOTA`]. It bounds
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::access_grants`] and, with
/// it, `create_access_grant_tokens` (one entry per live grant) — and therefore the
/// per-request work [`access_grant_named`] and [`access_profiles_scoped_by`] do under
/// the store lock, which is linear in the number of stored grants and profiles.
///
/// [`access_grant_named`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::access_grant_named
/// [`access_profiles_scoped_by`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::access_profiles_scoped_by
pub const ACCESS_GRANT_QUOTA: usize = 10_000;

/// The resource type an `OmniDashboard`'s ARN carries, as the model's own
/// `CreateOmniDashboard` example documents it
/// (`arn:aws:cloudwatch:<region>:<account>:omni-dashboard/<dashboardId>`).
pub const OMNI_DASHBOARD_ARN_RESOURCE_TYPE: &str = "omni-dashboard";

/// How many dashboards the store holds before `CreateOmniDashboard` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// The emulator's OWN bound, for the same reason as [`DOMAIN_QUOTA`]. It bounds
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::omni_dashboards`] and, with
/// it, `create_omni_dashboard_tokens` (one entry per live dashboard) — and therefore the
/// per-request work [`omni_dashboard_named`] does under the store lock, which is linear
/// in the number of stored dashboards.
///
/// [`omni_dashboard_named`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::omni_dashboard_named
pub const OMNI_DASHBOARD_QUOTA: usize = 10_000;

/// A dashboard as persisted. The wire `OmniDashboard` shape carries no parent id, so
/// the space the dashboard lives in is kept beside it: a dashboard is addressed by its
/// space AND its id.
#[derive(Debug, Clone)]
pub struct StoredOmniDashboard {
    /// Creation order, so `ListOmniDashboards`-style enumeration is stable.
    pub seq: u64,
    /// The id of the space this dashboard belongs to.
    pub space_id: String,
    /// Exactly what `GetOmniDashboard` returns.
    pub dashboard: OmniDashboard,
}

/// A domain as persisted, with the bookkeeping the wire shape does not carry.
#[derive(Debug, Clone)]
pub struct StoredDomain {
    /// Creation order, so `ListDomains`-style enumeration is stable.
    pub seq: u64,
    /// Exactly what `GetDomain` returns.
    pub domain: Domain,
    /// The request's `tags`; no modelled operation reads them back yet.
    pub tags: HashMap<String, String>,
}

/// A space as persisted. The wire `Space` shape carries only the parent's
/// `domainArn`, so the parent's id is kept beside it for `ListSpaces`'s
/// `domainId` filter and for the `DeleteDomain` conflict check.
#[derive(Debug, Clone)]
pub struct StoredSpace {
    /// Creation order, and the cursor `ListSpaces`'s page token resumes from.
    pub seq: u64,
    /// The id of the domain this space belongs to.
    pub domain_id: String,
    /// Exactly what `GetSpace` returns.
    pub space: Space,
    /// The request's `tags`; no modelled operation reads them back yet.
    pub tags: HashMap<String, String>,
}

/// An access profile as persisted. The wire `AccessProfile` shape carries the parent
/// space's id, so the record needs no separate parent field.
#[derive(Debug, Clone)]
pub struct StoredAccessProfile {
    /// Creation order, so `ListAccessProfiles`-style enumeration is stable.
    pub seq: u64,
    /// The profile as persisted. `assumeStatus` is left unset here: it reports the
    /// CALLING principal's authorization rather than a property of the profile, so it
    /// is computed when the profile is reported ([`access_profile_view`]).
    pub profile: AccessProfile,
    /// The ids of the trust grants recorded against this profile — the grants that
    /// authorize a principal to assume it. A create records none, which is why a
    /// profile reports `assumeStatus` DENIED until a grant names it.
    pub assume_grant_ids: Vec<String>,
    /// The request's `tags`; no modelled operation reads them back yet.
    pub tags: HashMap<String, String>,
}

/// An access grant as persisted. The wire `AccessGrant` shape carries its own
/// `domainId` and `spaceId`, so the record needs no separate parent field.
#[derive(Debug, Clone)]
pub struct StoredAccessGrant {
    /// Creation order, so `ListAccessGrants`-style enumeration is stable.
    pub seq: u64,
    /// Exactly what `GetAccessGrant` returns.
    pub grant: AccessGrant,
    /// The request's `tags`; no modelled operation reads them back yet.
    pub tags: HashMap<String, String>,
}

/// Per-service state for com-amazonaws-cloudwatchomni-cloudwatchomnifrontend. A handler reaches it through [`service_state`].
///
/// Every map here is BOUNDED: `domains` (and the two maps keyed one-to-one to a live
/// domain) by [`DOMAIN_QUOTA`], `spaces` (and the token map keyed to a live space) by
/// [`SPACE_QUOTA`], `access_profiles` (and the token map keyed to a live profile) by
/// [`ACCESS_PROFILE_QUOTA`], `access_grants` (and the token map keyed to a live grant)
/// by [`ACCESS_GRANT_QUOTA`], `omni_dashboards` (and the token map keyed to a live
/// dashboard) by [`OMNI_DASHBOARD_QUOTA`]. The creates refuse with the modelled
/// `ServiceQuotaExceededException` once the bound is reached, and the deletes release
/// records and their tokens together, so nothing in the store grows without end.
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudwatchomniCloudwatchomnifrontendState {
    /// Every persisted domain, keyed by its `domainId`.
    pub domains: HashMap<String, StoredDomain>,
    /// `name` -> `domainId`, the index behind the duplicate-name `ConflictException`.
    pub domain_name_index: HashMap<String, String>,
    /// Every persisted space, keyed by its `spaceId`.
    pub spaces: HashMap<String, StoredSpace>,
    /// `CreateDomain` `clientToken` -> the `domainId` that token created.
    pub create_domain_tokens: HashMap<String, String>,
    /// `CreateSpace` `clientToken` -> the `spaceId` that token created.
    pub create_space_tokens: HashMap<String, String>,
    /// Every persisted access profile, keyed by its `profileId`.
    pub access_profiles: HashMap<String, StoredAccessProfile>,
    /// `CreateAccessProfile` `clientToken` -> the `profileId` that token created.
    pub create_access_profile_tokens: HashMap<String, String>,
    /// Every persisted access grant, keyed by its `grantId`.
    pub access_grants: HashMap<String, StoredAccessGrant>,
    /// `CreateAccessGrant` `clientToken` -> the `grantId` that token created.
    pub create_access_grant_tokens: HashMap<String, String>,
    /// Every persisted dashboard, keyed by its `dashboardId`.
    pub omni_dashboards: HashMap<String, StoredOmniDashboard>,
    /// `CreateOmniDashboard` `clientToken` -> the `dashboardId` that token created.
    pub create_omni_dashboard_tokens: HashMap<String, String>,
    /// Hands out [`StoredDomain::seq`] / [`StoredSpace::seq`] / [`StoredAccessProfile::seq`]
    /// / [`StoredAccessGrant::seq`] / [`StoredOmniDashboard::seq`].
    next_seq: u64,
}

impl ComAmazonawsCloudwatchomniCloudwatchomnifrontendState {
    /// The next creation-order sequence number. Strictly increasing and never
    /// reused, so a page token minted from one stays valid as the store grows.
    pub fn next_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// Every stored space in creation order, optionally restricted to one parent
    /// domain, skipping everything at or before `after_seq`.
    ///
    /// Linear in the number of stored spaces, which [`SPACE_QUOTA`] bounds.
    pub fn spaces_after(&self, domain_id: Option<&str>, after_seq: u64) -> Vec<&StoredSpace> {
        let mut selected: Vec<&StoredSpace> = self
            .spaces
            .values()
            .filter(|stored| stored.seq > after_seq)
            .filter(|stored| domain_id.is_none_or(|id| stored.domain_id == id))
            .collect();
        selected.sort_by_key(|stored| stored.seq);
        selected
    }

    /// The access profile `space_id` holds under `name`, if the store holds one.
    ///
    /// Linear in the number of stored profiles, which [`ACCESS_PROFILE_QUOTA`] bounds.
    pub fn access_profile_named(&self, space_id: &str, name: &str) -> Option<&StoredAccessProfile> {
        self.access_profiles
            .values()
            .find(|stored| stored.profile.space_id == space_id && stored.profile.name == name)
    }

    /// The access grant `space_id` holds under `name`, if the store holds one.
    ///
    /// Linear in the number of stored grants, which [`ACCESS_GRANT_QUOTA`] bounds.
    pub fn access_grant_named(&self, space_id: &str, name: &str) -> Option<&StoredAccessGrant> {
        self.access_grants.values().find(|stored| {
            stored.grant.space_id == space_id && stored.grant.name.as_deref() == Some(name)
        })
    }

    /// The ids of the access profiles `grant`'s scoped actions apply to.
    ///
    /// A `ScopedActions` group limits a grant to the resources its `ResourceScope`s
    /// name: `resourceArns` holds "specific resource ARNs" and, per its model doc,
    /// "when absent, defaults to `*`" — every resource of that `resourceType`. So a
    /// grant applies to an access profile when one of its resource scopes names that
    /// profile's ARN, or scopes the `AccessProfile` type without narrowing it to ARNs.
    /// Only profiles of the grant's own space are reached: a grant applies to one space.
    ///
    /// This is what records a grant against a profile, which is what
    /// [`access_profile_view`] reads `assumeStatus` from — a profile no grant applies to
    /// cannot be assumed by anyone.
    ///
    /// Linear in the number of stored profiles, which [`ACCESS_PROFILE_QUOTA`] bounds,
    /// times the grant's resource scopes, which the model bounds at 20 groups of 10.
    pub fn access_profiles_scoped_by(&self, grant: &AccessGrant) -> Vec<String> {
        let scopes = || {
            grant
                .scoped_actions
                .iter()
                .flatten()
                .filter_map(|group| group.resources.as_ref())
                .flatten()
        };
        let whole_profile_type = scopes().any(|scope| {
            scope.resource_type == ACCESS_PROFILE_RESOURCE_TYPE
                && scope
                    .resource_arns
                    .as_ref()
                    .is_none_or(|arns| arns.is_empty())
        });
        let mut named: Vec<String> = self
            .access_profiles
            .values()
            .filter(|stored| stored.profile.space_id == grant.space_id)
            .filter(|stored| {
                whole_profile_type
                    || scopes().any(|scope| {
                        scope
                            .resource_arns
                            .iter()
                            .flatten()
                            .any(|arn| *arn == stored.profile.arn)
                    })
            })
            .map(|stored| stored.profile.profile_id.clone())
            .collect();
        named.sort();
        named
    }

    /// The dashboard `space_id` holds under `name`, if the store holds one.
    ///
    /// Linear in the number of stored dashboards, which [`OMNI_DASHBOARD_QUOTA`] bounds.
    pub fn omni_dashboard_named(&self, space_id: &str, name: &str) -> Option<&StoredOmniDashboard> {
        self.omni_dashboards
            .values()
            .find(|stored| stored.space_id == space_id && stored.dashboard.name == name)
    }

    /// Whether any stored space still belongs to `domain_id`.
    ///
    /// Linear in the number of stored spaces, which [`SPACE_QUOTA`] bounds.
    pub fn domain_has_spaces(&self, domain_id: &str) -> bool {
        self.spaces
            .values()
            .any(|stored| stored.domain_id == domain_id)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudwatchomniCloudwatchomnifrontendState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudwatchomniCloudwatchomnifrontendState>)
}

/// Now, as the modelled `Timestamp` type.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
}

/// The region the request was signed for, which is the region the resource it
/// creates lives in. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Base36 (`0-9a-z`) rendering of a 128-bit value, most significant digit first.
///
/// `36^25 > 2^128`, so the longest output is 25 characters — which is exactly the
/// budget `DomainId`'s `^d-[0-9a-z]{1,25}$` leaves after the `d-` prefix.
fn base36(mut value: u128) -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut digits = Vec::new();
    while value > 0 {
        digits.push(ALPHABET[(value % 36) as usize]);
        value /= 36;
    }
    digits.reverse();
    String::from_utf8(digits).expect("the base36 alphabet is ASCII")
}

/// A fresh v4 UUID, canonical lowercase hyphenated form.
///
/// The randomness is the `uuid` crate's: this reaches it through
/// `emulator_telemetry`'s uuid-backed id newtype (`BatchId::random()` is
/// `uuid::Uuid::new_v4()` rendered canonically), which this crate ALREADY depends on,
/// rather than declaring a second path to the same crate. Nothing here is a
/// hand-rolled generator, and this crate adds no dependency — so the whole service
/// carries no `Cargo.lock` delta, which is what lets it land beside the other fixes
/// that are being merged into the same lock file concurrently.
fn fresh_uuid() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// The 128 bits of a fresh v4 UUID.
fn fresh_uuid_bits() -> u128 {
    let hyphenless: String = fresh_uuid().chars().filter(|c| *c != '-').collect();
    let bytes = ::hex::decode(hyphenless).expect("a canonical UUID is 32 hex digits");
    let octets: [u8; 16] = bytes
        .try_into()
        .expect("a canonical UUID is exactly 16 bytes");
    u128::from_be_bytes(octets)
}

/// A fresh `domainId`.
///
/// `DomainId`'s model doc: "Minted by the backend as \"d-\" + base36(UUID) — at most
/// \"d-\" plus 25 chars", so that is what this mints.
pub fn mint_domain_id() -> String {
    format!("d-{}", base36(fresh_uuid_bits()))
}

/// A fresh `spaceId`. `SpaceId`'s model doc: "a v4 UUID minted by the backend".
pub fn mint_space_id() -> String {
    fresh_uuid()
}

/// A fresh `profileId`.
///
/// `ProfileId` states no format of its own ("assigned when the profile is created")
/// beyond `@length(1..64)` and `^[a-zA-Z0-9_-]+$`; every other id this backend mints
/// and does not document a prefix for is a v4 UUID (`SpaceId`, `GrantId`,
/// `DashboardId`), and a canonical UUID is 36 characters drawn from that character
/// class, so that is what this mints.
pub fn mint_profile_id() -> String {
    fresh_uuid()
}

/// A fresh `dashboardId`. `DashboardId`'s model doc: "a lowercase v4 UUID assigned
/// when the dashboard is created".
pub fn mint_dashboard_id() -> String {
    fresh_uuid()
}

/// A fresh `grantId`. `GrantId`'s model doc: "a v4 UUID assigned when the grant is
/// created".
pub fn mint_grant_id() -> String {
    fresh_uuid()
}

/// The ARN of a domain, in this service's ARN namespace.
pub fn domain_arn(region: &str, domain_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("domain/{domain_id}"))
}

/// The ARN of a space, in this service's ARN namespace.
pub fn space_arn(region: &str, space_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("space/{space_id}"))
}

/// The ARN of an access profile, in this service's ARN namespace.
///
/// An access profile lives IN a space (`AccessProfile.spaceId`: "The ID of the space
/// the profile belongs to"), so its ARN is the parent space's resource path with the
/// profile appended — the ARN of a child resource names its parent.
pub fn access_profile_arn(region: &str, space_id: &str, profile_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("space/{space_id}/access-profile/{profile_id}"),
    )
}

/// The ARN of an access grant, in this service's ARN namespace.
///
/// A grant is scoped to one space (`AccessGrant.spaceId`: "The space this grant applies
/// to"), so, like an access profile's, its ARN is the parent space's resource path with
/// the grant appended — the ARN of a child resource names its parent.
pub fn access_grant_arn(region: &str, space_id: &str, grant_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("space/{space_id}/access-grant/{grant_id}"),
    )
}

/// The ARN of a dashboard, in this service's ARN namespace.
///
/// The form is the one the model's own `CreateOmniDashboard` example documents for
/// `OmniDashboard.arn`: the `omni-dashboard` resource type and the dashboard's id, in
/// the account and the region the dashboard lives in. A dashboard's ARN therefore does
/// not repeat its parent space.
pub fn omni_dashboard_arn(region: &str, dashboard_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{OMNI_DASHBOARD_ARN_RESOURCE_TYPE}/{dashboard_id}"),
    )
}

/// The principal that made this request, for a `createdBy` member.
///
/// Derived from the request's own SigV4 access key id, so the member reports who
/// called rather than a fabricated identity. That id is caller-controlled text, so it
/// is first reduced to the characters an access key id can hold; a request that carries
/// no credential scope (an unsigned, in-process call) reports the emulator's default
/// principal.
pub fn caller_principal(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    let filtered: String = ::engine::extract_akid(&ctx.parts().headers)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(128)
        .collect();
    let access_key_id = if filtered.is_empty() {
        "default"
    } else {
        filtered.as_str()
    };
    // An IAM principal is account-scoped, hence the empty region segment.
    ::engine::emulator_arn("iam", "", &format!("user/{access_key_id}"))
}

/// A domain's HTTPS endpoint URL, built from its id, its region and this service's
/// documented endpoint prefix.
pub fn domain_endpoint_url(region: &str, domain_id: &str) -> String {
    format!("https://{domain_id}.{ENDPOINT_PREFIX}.{region}.amazonaws.com")
}

/// The additional endpoint URLs the model describes as "derived from the domain
/// name": the same endpoint addressed by name rather than by id.
pub fn custom_endpoint_urls(region: &str, name: &str) -> Vec<String> {
    vec![format!(
        "https://{name}.{ENDPOINT_PREFIX}.{region}.amazonaws.com"
    )]
}

/// The Identity Center application ARN for a domain, or `None` — the model says it
/// is "Absent for IAM-only domains", so it exists exactly when the domain lists the
/// `IDC` identity provider. When the request named an Identity Center instance the
/// application is minted under that instance.
pub fn identity_center_application_arn(
    identity_providers: &[String],
    configuration: Option<&IdentityProviderConfiguration>,
) -> Option<String> {
    if !identity_providers
        .iter()
        .any(|provider| provider == IDENTITY_PROVIDER_IDC)
    {
        return None;
    }
    let instance = configuration
        .and_then(|configuration| configuration.identity_center_configuration.as_ref())
        .and_then(|center| center.identity_center_instance_arn.as_deref())
        .and_then(|arn| arn.split('/').next_back())
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("ssoins-{}", short_hex()));
    // Identity Center applications are account-scoped, hence the empty region segment.
    Some(::engine::emulator_arn(
        "sso",
        "",
        &format!("application/{instance}/apl-{}", short_hex()),
    ))
}

/// Sixteen hex characters of a fresh v4 UUID — the body AWS Identity Center ids use.
fn short_hex() -> String {
    fresh_uuid()
        .chars()
        .filter(|c| *c != '-')
        .take(16)
        .collect()
}

/// The `EncryptionConfiguration` a space with no customer managed key reports.
///
/// The `Space.encryptionConfiguration` model doc: "Always populated: a space with no
/// customer managed key reports `encryptionStrategy` AWS_OWNED and no `kmsKeyArn`".
pub fn aws_owned_encryption() -> EncryptionConfiguration {
    EncryptionConfiguration {
        encryption_strategy: ENCRYPTION_STRATEGY_AWS_OWNED.to_string(),
        kms_key_arn: None,
    }
}

/// Project a stored space into the `SpaceSummary` the list operations return.
///
/// Every `SpaceSummary` member is a member of `Space` with the same name, so the
/// projection is a straight copy — a summary never reports a value the full space
/// does not.
pub fn space_summary(space: &Space) -> SpaceSummary {
    SpaceSummary {
        space_id: space.space_id.clone(),
        name: space.name.clone(),
        space_arn: space.space_arn.clone(),
        domain_arn: space.domain_arn.clone(),
        region: space.region.clone(),
        owner_account_id: space.owner_account_id.clone(),
        status: space.status.clone(),
        status_reason: space.status_reason.clone(),
        created_at: space.created_at,
        updated_at: space.updated_at,
    }
}

/// Project a stored access profile into the `AccessProfile` an operation returns.
///
/// Every member but `assumeStatus` is the persisted record verbatim. `assumeStatus`
/// reports "the calling principal's authorization to assume this access profile", and
/// that authorization comes from a trust grant recorded against the profile — so it is
/// read from the grants the store holds for it: a profile no grant names cannot be
/// assumed, and reports DENIED.
pub fn access_profile_view(stored: &StoredAccessProfile) -> AccessProfile {
    let assume_status = if stored.assume_grant_ids.is_empty() {
        ASSUME_STATUS_DENIED
    } else {
        ASSUME_STATUS_ALLOWED
    };
    AccessProfile {
        assume_status: Some(assume_status.to_string()),
        ..stored.profile.clone()
    }
}

/// The member of a dashboard body that carries the dashboard's panels.
pub const DASHBOARD_BODY_PANELS: &str = "panels";

/// Why `body` is not a dashboard definition, or `None` when it is one.
///
/// `body`'s model doc is "The dashboard definition, as a JSON document": a definition
/// is a JSON OBJECT, and the panels it lays out are its `panels` member — a list whose
/// entries are panel objects. Anything else (text that is not JSON at all, a JSON
/// scalar or array, a document with no panels, or a `panels` member that is not a list
/// of objects) is not a dashboard definition, so an operation that takes one refuses it
/// rather than persisting a body no reader can lay out.
///
/// The returned text names the member and what was wrong with it, which is all a
/// caller has to fix the request from.
pub fn dashboard_body_rejection(body: &str) -> Option<String> {
    let document: ::serde_json::Value = match ::serde_json::from_str(body) {
        Ok(document) => document,
        Err(_) => {
            return Some("The dashboard body is not a valid JSON document.".to_string());
        }
    };
    let Some(members) = document.as_object() else {
        return Some(format!(
            "The dashboard body must be a JSON object with a '{DASHBOARD_BODY_PANELS}' array."
        ));
    };
    let Some(panels) = members.get(DASHBOARD_BODY_PANELS) else {
        return Some(format!(
            "The dashboard body must contain a '{DASHBOARD_BODY_PANELS}' array."
        ));
    };
    let Some(panels) = panels.as_array() else {
        return Some(format!(
            "The dashboard body member '{DASHBOARD_BODY_PANELS}' must be an array of panel \
             objects."
        ));
    };
    panels
        .iter()
        .position(|panel| !panel.is_object())
        .map(|index| {
            format!(
                "The dashboard body member '{DASHBOARD_BODY_PANELS}[{index}]' must be a panel \
                 object."
            )
        })
}

/// The prefix a space-list page token carries, so a token minted for one collection
/// cannot be replayed against another.
const SPACE_PAGE_TOKEN_PREFIX: &str = "spaces:";

/// Mint the opaque `nextToken` that resumes a space listing after `seq`.
pub fn encode_space_page_token(seq: u64) -> String {
    ::hex::encode(format!("{SPACE_PAGE_TOKEN_PREFIX}{seq}"))
}

/// Read a space-list `nextToken` back, or `None` when it is not one this service minted.
pub fn decode_space_page_token(token: &str) -> Option<u64> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix(SPACE_PAGE_TOKEN_PREFIX)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base36_never_exceeds_the_domain_id_budget() {
        assert_eq!(base36(0), "0", "zero renders as a single digit");
        assert_eq!(base36(35), "z", "the last alphabet digit");
        assert_eq!(base36(36), "10", "the first two-digit value");
        assert!(
            base36(u128::MAX).len() <= 25,
            "base36(u128::MAX) is {} chars, over DomainId's 25-char budget",
            base36(u128::MAX).len()
        );
    }

    #[test]
    fn a_profile_reports_assume_status_from_the_grants_recorded_against_it() {
        // Rows: no trust grant, one, and several. The status is read from the grant
        // list, so it is DENIED exactly when the list is empty.
        let at = now();
        let profile = AccessProfile {
            profile_id: "profile".to_string(),
            space_id: "space".to_string(),
            arn: "arn:aws:cloudwatch:us-east-1:000000000000:space/space/access-profile/profile"
                .to_string(),
            name: "a profile".to_string(),
            description: None,
            created_at: at,
            updated_at: at,
            assume_status: None,
            profile_type: Some(ACCESS_PROFILE_TYPE_CUSTOMER_MANAGED.to_string()),
        };
        for (grants, expected) in [
            (vec![], ASSUME_STATUS_DENIED),
            (vec!["grant-a".to_string()], ASSUME_STATUS_ALLOWED),
            (
                vec!["grant-a".to_string(), "grant-b".to_string()],
                ASSUME_STATUS_ALLOWED,
            ),
        ] {
            let stored = StoredAccessProfile {
                seq: 1,
                profile: profile.clone(),
                assume_grant_ids: grants.clone(),
                tags: HashMap::new(),
            };
            let view = access_profile_view(&stored);
            assert_eq!(
                view.assume_status.as_deref(),
                Some(expected),
                "case {grants:?}: expected assumeStatus {expected}, got {:?}",
                view.assume_status
            );
            assert_eq!(
                view.profile_id, stored.profile.profile_id,
                "case {grants:?}: the projection must not change profileId, got {:?}",
                view.profile_id
            );
        }
    }

    #[test]
    fn a_page_token_round_trips_and_rejects_foreign_text() {
        for seq in [1_u64, 2, 99, u64::MAX] {
            let token = encode_space_page_token(seq);
            assert_eq!(
                decode_space_page_token(&token),
                Some(seq),
                "case {seq}: token {token} did not round-trip"
            );
        }
        for bogus in ["", "not-hex", &::hex::encode("domains:1")] {
            assert_eq!(
                decode_space_page_token(bogus),
                None,
                "case {bogus:?}: a foreign token must not decode"
            );
        }
    }
}
