// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-accountaccess-awsaccountaccess service.
//!
//! One store per emulator, reached by every handler through [`service_state`]. The
//! store holds the account-access control plane: the *applications* a caller creates
//! and the *entitlements* that hang off them, in creation order so listing pages
//! deterministically and deleting an application can cascade.

use std::collections::BTreeMap;

use crate::ops::types::{
    EntitlementFilter, IdentityCenterPrincipal, IdentityCenterPrincipalFilter, Principal,
    PrincipalFilter,
};

/// `CREATE_IN_PROGRESS` — the `Status` an application carries while the service is
/// still creating it and its IAM Identity Center application.
///
/// From `smithy_query(query="shape", shape="Status")`:
/// `CREATE_IN_PROGRESS | ACTIVE | DELETE_IN_PROGRESS | CREATE_FAILED | DELETE_FAILED`.
pub const STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// `ACTIVE` — the `Status` of an application whose creation has finished.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// How long after `CreateApplication` the creation the clock owes falls due, i.e. how
/// long an application stays `CREATE_IN_PROGRESS`.
///
/// The model gives an application a non-terminal `CREATE_IN_PROGRESS` status (and
/// `GetApplication` an `error` member for `CREATE_FAILED`), so creation is
/// asynchronous and a caller polling straight after `CreateApplication` must be able
/// to observe the transient status. No AWS document fixes the span, so this is a
/// CHOSEN value inside the project's 250–750 ms band for a clock-owed transition; it
/// is never zero.
pub const APPLICATION_ACTIVATION_MILLIS: i64 = 500;

/// The region an ARN carries when the request's SigV4 credential scope names none the
/// `ApplicationArn` pattern would accept.
const DEFAULT_REGION: &str = "us-east-1";

/// The page size `ListEntitlements` uses when the request supplies no `maxResults`,
/// and the ceiling it clamps a supplied one to.
///
/// A listing is always bounded: the model bounds `maxResults` to 1..100
/// (`smithy_query(query="operation", operation="ListEntitlements")`), so an absent one
/// pages at that same maximum and hands back a `nextToken`, rather than collecting the
/// whole store into one response.
pub const DEFAULT_MAX_RESULTS: usize = 100;

/// The most applications this emulator's store will hold.
///
/// A bound on the store, not an AWS-documented quota: the emulator cannot resolve the
/// IAM Identity Center instance an application names (it holds no Identity Center
/// state), so nothing else limits how many distinct identity sources a caller can
/// invent. Beyond this the service refuses the create with the `ConflictException` the
/// operation models — the store's current state cannot take another application.
pub const MAX_APPLICATIONS: usize = 100;

/// The most entitlements one application will hold, refused beyond that with the
/// `ServiceQuotaExceededException` `CreateEntitlement` models (HTTP 402). AWS publishes
/// a quota on assignments per application; this value is the emulator's own.
pub const MAX_ENTITLEMENTS_PER_APPLICATION: usize = 500;

/// The one tenant this emulator serves.
///
/// The emulator presents exactly ONE account — `engine::emulator_account_id()` — so its
/// store has no tenant dimension: every application in it belongs to that account. The
/// value is therefore DERIVED when a response reports `tenantId` and is deliberately
/// NOT stored on a record and never a lookup key — there is no second tenant whose
/// resources an ARN could reach, and no record field that could be mistaken for an
/// authorization check. The account boundary is the emulator process.
pub fn tenant_id() -> String {
    format!("t-{}", ::engine::emulator_account_id())
}

/// Wall-clock now, as milliseconds since the Unix epoch.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// The region this request was signed for, for the ARNs the service mints.
///
/// Read from the SigV4 credential scope — the only region a request carries — and
/// refused back to [`DEFAULT_REGION`] when it is not a region the modeled
/// `ApplicationArn` pattern (`[a-z0-9]+(-[a-z0-9]+)*`) would accept, so a minted ARN
/// always satisfies its own pattern.
pub fn request_region(parts: &::http::request::Parts) -> String {
    match ::emulator_server::sigv4_credential_scope(&parts.headers) {
        Some(scope) if region_is_well_formed(&scope.region) => scope.region,
        _ => DEFAULT_REGION.to_string(),
    }
}

/// Whether `region` matches the region segment of the modeled ARN patterns.
fn region_is_well_formed(region: &str) -> bool {
    !region.is_empty()
        && region.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// The 12-digit account id of an IAM role ARN — the `account` member every
/// `PrincipalRoleEntitlementDetails` must carry.
///
/// `None` when the ARN carries no 12-digit account segment, which the modeled
/// `RoleArn` pattern already excludes.
pub fn account_from_role_arn(role_arn: &str) -> Option<&str> {
    let account = role_arn.split(':').nth(4)?;
    (account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit())).then_some(account)
}

/// The instance id (`ins-…` / `ssoins-…`) of an IAM Identity Center instance ARN.
///
/// The minted `IdentityCenterDetails$applicationArn` embeds it:
/// `arn:aws:sso::<account>:application/<instance id>/apl-<16>`.
pub fn identity_center_instance_id(instance_arn: &str) -> Option<&str> {
    instance_arn
        .split_once("instance/")
        .map(|(_, instance_id)| instance_id)
        .filter(|instance_id| !instance_id.is_empty())
}

/// A `nextToken` resuming after `entitlement_id`.
///
/// The token encodes a POSITION, not a secret and not a capability: it names the
/// entitlement the previous page stopped on, which the caller already received, and it
/// is only ever read for the application the request names — so a caller that forges or
/// edits one can reach nothing it could not already list, and
/// [`decode_page_token`]'s caller refuses a token that does not name an entitlement of
/// that application. It is opaque in the sense the API contract needs: clients must not
/// parse or construct it, and its form is free to change.
pub fn encode_page_token(entitlement_id: &str) -> String {
    ::hex::encode(entitlement_id)
}

/// The entitlement id a `nextToken` resumes after, or `None` when the token was not
/// minted by [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// The principal of an entitlement, as stored.
///
/// A flat, comparable form of the modeled `Principal` union
/// (`identityCenter` -> `userId` | `groupId`): `ListEntitlements` filters by equality
/// with a `PrincipalFilter`, which the generated union types cannot do themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredPrincipal {
    /// `Principal$identityCenter` -> `IdentityCenterPrincipal$userId`.
    IdentityCenterUser(String),
    /// `Principal$identityCenter` -> `IdentityCenterPrincipal$groupId`.
    IdentityCenterGroup(String),
}

impl StoredPrincipal {
    /// Store the principal a request supplied.
    pub fn from_modeled(principal: &Principal) -> Self {
        match principal {
            Principal::identityCenter(IdentityCenterPrincipal::userId(user_id)) => {
                Self::IdentityCenterUser(user_id.clone())
            }
            Principal::identityCenter(IdentityCenterPrincipal::groupId(group_id)) => {
                Self::IdentityCenterGroup(group_id.clone())
            }
        }
    }

    /// The stored principal, in the modeled shape a response carries.
    pub fn to_modeled(&self) -> Principal {
        match self {
            Self::IdentityCenterUser(user_id) => Principal::identityCenter(
                IdentityCenterPrincipal::userId(user_id.clone()),
            ),
            Self::IdentityCenterGroup(group_id) => Principal::identityCenter(
                IdentityCenterPrincipal::groupId(group_id.clone()),
            ),
        }
    }

    /// Whether this principal is the one a `PrincipalFilter` names.
    pub fn matches_filter(&self, filter: &PrincipalFilter) -> bool {
        match filter {
            PrincipalFilter::identityCenter(IdentityCenterPrincipalFilter::userId(user_id)) => {
                matches!(self, Self::IdentityCenterUser(stored) if stored == user_id)
            }
            PrincipalFilter::identityCenter(IdentityCenterPrincipalFilter::groupId(group_id)) => {
                matches!(self, Self::IdentityCenterGroup(stored) if stored == group_id)
            }
        }
    }
}

/// Why an application's creation or deletion failed — the `ErrorDetails` member of
/// `GetApplicationResponse`. No path in this slice fails, so it stays `None`.
#[derive(Debug, Clone)]
pub struct ApplicationError {
    pub code: String,
    pub message: String,
}

/// One account-access manager application.
#[derive(Debug, Clone)]
pub struct Application {
    pub application_arn: String,
    pub identity_center_instance_arn: String,
    pub identity_center_application_arn: String,
    /// A `Status` value; see [`STATUS_CREATE_IN_PROGRESS`] / [`STATUS_ACTIVE`].
    pub status: String,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// When the `CREATE_IN_PROGRESS` -> `ACTIVE` transition falls due. Stored, never
    /// slept on: [`ComAmazonawsAccountaccessAwsaccountaccessState::settle`]
    /// materialises it on the next call at or after this instant.
    pub active_at_millis: i64,
    pub tags: BTreeMap<String, String>,
    pub error: Option<ApplicationError>,
}

/// One entitlement of one application.
#[derive(Debug, Clone)]
pub struct EntitlementRecord {
    pub entitlement_id: String,
    pub application_arn: String,
    pub principal: StoredPrincipal,
    pub role_arn: String,
    /// The 12-digit account the role lives in, parsed from `role_arn` at create.
    pub account: String,
    /// The account's friendly name. The emulator holds no account directory, so it
    /// stays `None` rather than carrying an invented name.
    pub account_name: Option<String>,
    pub created_at_millis: i64,
}

impl EntitlementRecord {
    /// Whether this entitlement satisfies an `EntitlementFilter`.
    ///
    /// Per `PrincipalRoleEntitlementFilter`'s doc ("All specified criteria must match
    /// for an entitlement to be returned"), every criterion the filter sets must
    /// match and an unset criterion constrains nothing.
    pub fn matches(&self, filter: &EntitlementFilter) -> bool {
        let Some(principal_role) = filter.principal_role.as_ref() else {
            return true;
        };
        if let Some(principal) = principal_role.principal.as_ref() {
            if !self.principal.matches_filter(principal) {
                return false;
            }
        }
        if let Some(role_arn) = principal_role.role_arn.as_ref() {
            if self.role_arn != *role_arn {
                return false;
            }
        }
        if let Some(account) = principal_role.account.as_ref() {
            if self.account != *account {
                return false;
            }
        }
        true
    }
}

/// Per-service state for com-amazonaws-accountaccess-awsaccountaccess. A handler reaches it through [`service_state`].
///
/// The store holds the resources of exactly one account — the emulator's single
/// `engine::emulator_account_id()` — so an ARN or an entitlement id is a resource's
/// complete identity here: no record carries a tenant discriminator and no accessor
/// takes one, because there is no second tenant whose resources could be reached. The
/// reported `tenantId` is derived at read time; see [`tenant_id`]. Growth is bounded by
/// [`MAX_APPLICATIONS`] and [`MAX_ENTITLEMENTS_PER_APPLICATION`], which the create
/// handlers enforce, and released by `DeleteApplication`.
#[derive(Debug, Default)]
pub struct ComAmazonawsAccountaccessAwsaccountaccessState {
    /// Applications, keyed by the ARN the service minted for them.
    pub applications: BTreeMap<String, Application>,
    /// Entitlements, keyed by entitlement id.
    pub entitlements: BTreeMap<String, EntitlementRecord>,
    /// Entitlement ids per application, in creation order, so `ListEntitlements`
    /// pages deterministically and `DeleteApplication` can cascade.
    pub entitlements_by_application: BTreeMap<String, Vec<String>>,
    /// Monotonic counter behind every id this service mints.
    pub next_id: u64,
}

impl ComAmazonawsAccountaccessAwsaccountaccessState {
    /// The next value of the id counter. Unique for the life of the store.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Materialise every transition the clock has already made due at `now_millis`.
    ///
    /// The only write a reporting operation makes: an application whose creation came
    /// due is `ACTIVE` from that instant onward whether or not anybody called, so its
    /// `updatedAt` is the instant the transition fell due, not the instant it was
    /// observed.
    pub fn settle(&mut self, now_millis: i64) {
        for application in self.applications.values_mut() {
            if application.status == STATUS_CREATE_IN_PROGRESS
                && application.active_at_millis <= now_millis
            {
                application.status = STATUS_ACTIVE.to_string();
                application.updated_at_millis = application.active_at_millis;
            }
        }
    }

    /// The application created for an IAM Identity Center instance, if any. The
    /// identity source is an application's identity, so this is what makes
    /// `CreateApplication` idempotent.
    ///
    /// The instance ARN is the whole key: every application in the store belongs to the
    /// emulator's one account, so there is no tenant to narrow the scan by. See the
    /// note on the struct.
    pub fn application_for_instance(&self, instance_arn: &str) -> Option<&Application> {
        self.applications
            .values()
            .find(|application| application.identity_center_instance_arn == instance_arn)
    }

    /// Persist a newly minted application.
    pub fn insert_application(&mut self, application: Application) {
        self.applications
            .insert(application.application_arn.clone(), application);
    }

    /// Persist a newly minted entitlement, keeping its application's creation order.
    pub fn insert_entitlement(&mut self, record: EntitlementRecord) {
        self.entitlements_by_application
            .entry(record.application_arn.clone())
            .or_default()
            .push(record.entitlement_id.clone());
        self.entitlements
            .insert(record.entitlement_id.clone(), record);
    }

    /// The entitlement ids of one application, in creation order.
    pub fn entitlement_ids(&self, application_arn: &str) -> &[String] {
        match self.entitlements_by_application.get(application_arn) {
            Some(ids) => ids.as_slice(),
            None => &[],
        }
    }

    /// The entitlement of `application_arn` granting `principal` the role `role_arn`,
    /// if one is already stored — what makes `CreateEntitlement` idempotent.
    pub fn find_entitlement(
        &self,
        application_arn: &str,
        principal: &StoredPrincipal,
        role_arn: &str,
    ) -> Option<&EntitlementRecord> {
        self.entitlement_ids(application_arn)
            .iter()
            .filter_map(|id| self.entitlements.get(id))
            .find(|record| record.principal == *principal && record.role_arn == role_arn)
    }

    /// Remove an application and cascade to its entitlements. Answers whether the
    /// application was there to remove.
    pub fn remove_application(&mut self, application_arn: &str) -> bool {
        let removed = self.applications.remove(application_arn).is_some();
        if let Some(ids) = self.entitlements_by_application.remove(application_arn) {
            for id in ids {
                self.entitlements.remove(&id);
            }
        }
        removed
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAccountaccessAwsaccountaccessState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAccountaccessAwsaccountaccessState>)
}
