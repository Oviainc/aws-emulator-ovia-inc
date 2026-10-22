// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workspacesweb-awserminecontrolplaneservice service.
//!
//! This module owns the web-portal control-plane records the implemented operations of
//! this crate read and write, plus the pure derivations they share so no two handlers
//! can disagree about them: ARN and endpoint minting, the portal-status rule, the
//! record -> modelled-shape projections, and the opaque `nextToken` codec.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    BrandingConfiguration, BrandingConfigurationCreateInput, ConflictException, IconImageInput,
    ImageMetadata, InternalServerException, Portal, PortalSummary, ResourceNotFoundException,
    ServiceQuotaExceededException, Tag, UserSettings, ValidationException, ValidationExceptionField,
    WallpaperImageInput, COLOR_THEME_VALUES, LOCALE_VALUES,
};

/// This service's SigV4 signing name, and so the service segment of every ARN it mints
/// (`arn:aws:workspaces-web:...`).
pub const SIGNING_NAME: &str = "workspaces-web";

/// The region an ARN is minted in when the request carries no usable SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The only `RendererType` the model declares.
pub const RENDERER_TYPE: &str = "AppStream";

/// The only `BrowserType` the model declares.
pub const BROWSER_TYPE: &str = "Chrome";

/// The `authenticationType` a portal gets when the request names none: the model's own
/// documentation for `CreatePortal$authenticationType` says "Defaults to Standard".
pub const DEFAULT_AUTHENTICATION_TYPE: &str = "Standard";

/// How many web portals this emulator holds before `CreatePortal` is refused with the
/// modelled `ServiceQuotaExceededException`.
///
/// This is the emulator's CAPACITY bound, not an AWS-published quota: a stored record is
/// process memory and a caller can mint one per request, so without a ceiling a client
/// can grow this process without limit. It is set far above any workflow an emulator is
/// used for, and deliberately not at the (low, per-account adjustable) number AWS
/// defaults to, which would refuse ordinary use.
pub const MAX_PORTALS: usize = 1000;

/// The same capacity bound for user-settings resources, for the same reason.
pub const MAX_USER_SETTINGS: usize = 1000;

/// The largest page `ListPortals` serves, used both as the ceiling on a caller-supplied
/// `maxResults` and as the page size when the request names none.
///
/// `maxResults` is a maximum, not a demand - the pagination contract lets a service
/// return fewer items plus a `nextToken` - so clamping it keeps one small request from
/// being amplified into a response proportional to the whole store. 100 is the page size
/// AWS list APIs of this shape cap at.
pub const MAX_LIST_PAGE_SIZE: usize = 100;

/// The ARN resource kind of a web portal.
pub const PORTAL_KIND: &str = "portal";

/// The ARN resource kind of a user-settings resource.
pub const USER_SETTINGS_KIND: &str = "userSettings";

/// `PortalStatus` while the portal is still missing a required association.
pub const PORTAL_STATUS_INCOMPLETE: &str = "Incomplete";

/// `PortalStatus` while a fully configured portal is still being brought up.
pub const PORTAL_STATUS_PENDING: &str = "Pending";

/// `PortalStatus` once a fully configured portal is ready to stream sessions.
pub const PORTAL_STATUS_ACTIVE: &str = "Active";

/// How long a portal that has just become fully configured reports `Pending` before it
/// reports `Active`.
///
/// AWS documents no duration for this transition, so this span is CHOSEN rather than
/// cited. It is deliberately non-zero, so a caller that reads the portal straight after
/// the association that completed it still observes the transient `Pending` status, and
/// it is short enough that a later read observes `Active` without anything having to
/// sleep: the due instant is stored and compared against the clock on read.
pub const PORTAL_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// A web portal as this emulator persists it.
///
/// Every member of the modelled `Portal` / `PortalSummary` shapes is projected out of
/// this record (see [`PortalRecord::to_portal`]); nothing is echoed from a request.
#[derive(Debug, Clone)]
pub struct PortalRecord {
    pub portal_arn: String,
    pub portal_endpoint: String,
    pub display_name: Option<String>,
    pub creation_date: SystemTime,
    pub authentication_type: String,
    pub instance_type: Option<String>,
    pub max_concurrent_sessions: Option<i32>,
    pub customer_managed_key: Option<String>,
    pub additional_encryption_context: Option<HashMap<String, String>>,
    pub portal_custom_domain: Option<String>,
    pub status_reason: Option<String>,
    pub browser_settings_arn: Option<String>,
    pub network_settings_arn: Option<String>,
    pub user_settings_arn: Option<String>,
    pub trust_store_arn: Option<String>,
    pub ip_access_settings_arn: Option<String>,
    pub data_protection_settings_arn: Option<String>,
    pub user_access_logging_settings_arn: Option<String>,
    pub session_logger_arn: Option<String>,
    pub tags: Vec<Tag>,
    /// The instant the portal last became fully configured, or `None` while it is not.
    /// Set by whichever association completes it; the status rule reads it against the
    /// clock, so the `Pending` -> `Active` move is owed by the clock alone.
    pub fully_configured_since: Option<SystemTime>,
}

impl PortalRecord {
    /// A web portal needs both its network settings and its user settings before it can
    /// serve a session, so those two associations are what take it out of `Incomplete`.
    ///
    /// `AssociateNetworkSettings` is still a scaffold in this crate, so today only the
    /// user-settings half can be satisfied and a portal stays `Incomplete`; the rule is
    /// written over the record rather than over the one implemented association so that
    /// implementing the other half needs no change here.
    pub fn is_fully_configured(&self) -> bool {
        self.network_settings_arn.is_some() && self.user_settings_arn.is_some()
    }

    /// Re-evaluate the completion instant after an association changed.
    ///
    /// Call this from a MUTATING operation only. It starts the clock the first time the
    /// portal is fully configured, keeps the existing instant while it stays configured,
    /// and clears it when an association is removed.
    pub fn note_configuration_change(&mut self, now: SystemTime) {
        if self.is_fully_configured() {
            self.fully_configured_since.get_or_insert(now);
        } else {
            self.fully_configured_since = None;
        }
    }

    /// The portal's status at `now`, derived entirely from the stored record.
    pub fn portal_status(&self, now: SystemTime) -> &'static str {
        match self.fully_configured_since {
            None => PORTAL_STATUS_INCOMPLETE,
            Some(since) if now < since + PORTAL_ACTIVATION_SPAN => PORTAL_STATUS_PENDING,
            Some(_) => PORTAL_STATUS_ACTIVE,
        }
    }

    /// Milliseconds since the epoch the portal was created at: the primary `ListPortals`
    /// sort term and half of the pagination token.
    pub fn creation_millis(&self) -> i64 {
        millis_since_epoch(self.creation_date)
    }

    /// The total order `ListPortals` pages in: creation instant first, ARN to break a tie
    /// between two portals created in the same millisecond.
    pub fn page_key(&self) -> (i64, &str) {
        (self.creation_millis(), self.portal_arn.as_str())
    }

    /// The modelled `Portal` for this record.
    pub fn to_portal(&self, now: SystemTime) -> Portal {
        Portal {
            portal_arn: self.portal_arn.clone(),
            renderer_type: Some(RENDERER_TYPE.to_string()),
            browser_type: Some(BROWSER_TYPE.to_string()),
            portal_status: Some(self.portal_status(now).to_string()),
            portal_endpoint: Some(self.portal_endpoint.clone()),
            display_name: self.display_name.clone(),
            creation_date: Some(to_date_time(self.creation_date)),
            browser_settings_arn: self.browser_settings_arn.clone(),
            data_protection_settings_arn: self.data_protection_settings_arn.clone(),
            user_settings_arn: self.user_settings_arn.clone(),
            network_settings_arn: self.network_settings_arn.clone(),
            session_logger_arn: self.session_logger_arn.clone(),
            trust_store_arn: self.trust_store_arn.clone(),
            status_reason: self.status_reason.clone(),
            user_access_logging_settings_arn: self.user_access_logging_settings_arn.clone(),
            authentication_type: Some(self.authentication_type.clone()),
            ip_access_settings_arn: self.ip_access_settings_arn.clone(),
            customer_managed_key: self.customer_managed_key.clone(),
            additional_encryption_context: self.additional_encryption_context.clone(),
            instance_type: self.instance_type.clone(),
            max_concurrent_sessions: self.max_concurrent_sessions,
            portal_custom_domain: self.portal_custom_domain.clone(),
        }
    }

    /// The modelled `PortalSummary` for this record: the same projection as
    /// [`Self::to_portal`] minus the two members `PortalSummary` does not declare
    /// (`statusReason` and `additionalEncryptionContext`/`customerManagedKey`).
    pub fn to_summary(&self, now: SystemTime) -> PortalSummary {
        PortalSummary {
            portal_arn: self.portal_arn.clone(),
            renderer_type: Some(RENDERER_TYPE.to_string()),
            browser_type: Some(BROWSER_TYPE.to_string()),
            portal_status: Some(self.portal_status(now).to_string()),
            portal_endpoint: Some(self.portal_endpoint.clone()),
            display_name: self.display_name.clone(),
            creation_date: Some(to_date_time(self.creation_date)),
            browser_settings_arn: self.browser_settings_arn.clone(),
            data_protection_settings_arn: self.data_protection_settings_arn.clone(),
            user_settings_arn: self.user_settings_arn.clone(),
            network_settings_arn: self.network_settings_arn.clone(),
            session_logger_arn: self.session_logger_arn.clone(),
            trust_store_arn: self.trust_store_arn.clone(),
            user_access_logging_settings_arn: self.user_access_logging_settings_arn.clone(),
            authentication_type: Some(self.authentication_type.clone()),
            ip_access_settings_arn: self.ip_access_settings_arn.clone(),
            instance_type: self.instance_type.clone(),
            max_concurrent_sessions: self.max_concurrent_sessions,
            portal_custom_domain: self.portal_custom_domain.clone(),
        }
    }
}

/// A user-settings resource as this emulator persists it.
#[derive(Debug, Clone)]
pub struct UserSettingsRecord {
    pub user_settings_arn: String,
    pub copy_allowed: String,
    pub paste_allowed: String,
    pub download_allowed: String,
    pub upload_allowed: String,
    pub print_allowed: String,
    pub disconnect_timeout_in_minutes: Option<i32>,
    pub idle_disconnect_timeout_in_minutes: Option<i32>,
    pub cookie_synchronization_configuration:
        Option<crate::ops::types::CookieSynchronizationConfiguration>,
    pub toolbar_configuration: Option<crate::ops::types::ToolbarConfiguration>,
    pub deep_link_allowed: Option<String>,
    pub web_authn_allowed: Option<String>,
    pub customer_managed_key: Option<String>,
    pub additional_encryption_context: Option<HashMap<String, String>>,
    /// The branding the create call uploaded, already projected into the modelled
    /// `BrandingConfiguration` (the images' own format plus the upload instant).
    pub branding_configuration: Option<BrandingConfiguration>,
    /// The portals this user-settings resource is associated with. Both directions of the
    /// association are stored so a delete can refuse while a portal still references it.
    pub associated_portal_arns: Vec<String>,
    pub tags: Vec<Tag>,
}

impl UserSettingsRecord {
    /// The modelled `UserSettings` for this record.
    pub fn to_user_settings(&self) -> UserSettings {
        let mut associated = self.associated_portal_arns.clone();
        associated.sort();
        UserSettings {
            user_settings_arn: self.user_settings_arn.clone(),
            associated_portal_arns: Some(associated),
            copy_allowed: Some(self.copy_allowed.clone()),
            paste_allowed: Some(self.paste_allowed.clone()),
            download_allowed: Some(self.download_allowed.clone()),
            upload_allowed: Some(self.upload_allowed.clone()),
            print_allowed: Some(self.print_allowed.clone()),
            disconnect_timeout_in_minutes: self.disconnect_timeout_in_minutes,
            idle_disconnect_timeout_in_minutes: self.idle_disconnect_timeout_in_minutes,
            cookie_synchronization_configuration: self.cookie_synchronization_configuration.clone(),
            customer_managed_key: self.customer_managed_key.clone(),
            additional_encryption_context: self.additional_encryption_context.clone(),
            deep_link_allowed: self.deep_link_allowed.clone(),
            toolbar_configuration: self.toolbar_configuration.clone(),
            branding_configuration: self.branding_configuration.clone(),
            web_authn_allowed: self.web_authn_allowed.clone(),
        }
    }
}

/// The hard ceiling on remembered `@idempotencyToken` values.
///
/// Every remembered token names one live resource, and [`IdempotencyLedger::forget_resource`]
/// drops a token with the resource it names, so the ledger can hold no more entries than
/// there are live resources. This constant states that bound explicitly rather than
/// leaving it to be reasoned about: the oldest token is evicted beyond it, so the ledger
/// is bounded whatever a caller does. An evicted token is simply no longer recognised as
/// a replay, which is the behaviour AWS client tokens have once their retention lapses.
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = MAX_PORTALS + MAX_USER_SETTINGS;

/// `(operation, clientToken)` -> the ARN that call minted, so a replayed client token
/// returns the first-created resource instead of minting a second one. The operation is
/// part of the key because a client token is scoped to the operation it was sent to.
///
/// Bounded in two independent ways, because the keys are caller-chosen and a create is
/// request-reachable: a token is dropped when the resource it names is deleted, and the
/// oldest token is evicted once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held.
#[derive(Debug, Default)]
pub struct IdempotencyLedger {
    by_token: HashMap<(String, String), String>,
    /// Insertion order, so eviction beyond the ceiling is FIFO.
    order: VecDeque<(String, String)>,
}

impl IdempotencyLedger {
    /// The ARN `operation` minted for `token`, if it is still remembered.
    pub fn arn_for(&self, operation: &str, token: &str) -> Option<&String> {
        self.by_token
            .get(&(operation.to_string(), token.to_string()))
    }

    /// Remember that `operation` minted `arn` for `token`, evicting the oldest entry once
    /// the ceiling is reached.
    pub fn remember(&mut self, operation: &str, token: &str, arn: &str) {
        let key = (operation.to_string(), token.to_string());
        if self.by_token.insert(key.clone(), arn.to_string()).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > MAX_REMEMBERED_CLIENT_TOKENS {
            if let Some(oldest) = self.order.pop_front() {
                self.by_token.remove(&oldest);
            }
        }
    }

    /// Drop every token that names `arn`: called when that resource is deleted, so a
    /// token's record is reclaimed with the resource it refers to.
    pub fn forget_resource(&mut self, arn: &str) {
        self.by_token.retain(|_, remembered| remembered != arn);
        let live = &self.by_token;
        self.order.retain(|key| live.contains_key(key));
    }

    /// How many tokens are remembered.
    pub fn len(&self) -> usize {
        self.by_token.len()
    }

    /// Whether no token is remembered.
    pub fn is_empty(&self) -> bool {
        self.by_token.is_empty()
    }
}

/// Per-service state for com-amazonaws-workspacesweb-awserminecontrolplaneservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWorkspaceswebAwserminecontrolplaneserviceState {
    /// Portals, keyed by `portalArn`. Capped at [`MAX_PORTALS`].
    pub portals: HashMap<String, PortalRecord>,
    /// User-settings resources, keyed by `userSettingsArn`. Capped at
    /// [`MAX_USER_SETTINGS`].
    pub user_settings: HashMap<String, UserSettingsRecord>,
    /// The bounded record of which client token minted which resource.
    pub idempotency: IdempotencyLedger,
    /// How many resources this store has minted: the monotonic sequence
    /// [`new_resource_id`] mixes into an id, so two resources minted inside one clock
    /// tick still get different ids.
    minted: u64,
}

impl ComAmazonawsWorkspaceswebAwserminecontrolplaneserviceState {
    /// The ARN a previous call with this client token minted, if that resource still
    /// exists. A token whose resource has since been deleted is reported as absent, so
    /// the replay mints a fresh resource rather than naming a dangling ARN.
    pub fn replayed_arn(&self, operation: &str, client_token: Option<&str>) -> Option<String> {
        let arn = self.idempotency.arn_for(operation, client_token?)?;
        (self.portals.contains_key(arn) || self.user_settings.contains_key(arn))
            .then(|| arn.clone())
    }

    /// Record that `operation` minted `arn` for `client_token`.
    pub fn remember_token(&mut self, operation: &str, client_token: Option<&str>, arn: &str) {
        if let Some(token) = client_token {
            self.idempotency.remember(operation, token, arn);
        }
    }

    /// The next value of this store's mint sequence: what [`new_resource_id`] mixes with
    /// the clock, so a second resource minted in the same tick gets a different id.
    pub fn next_mint(&mut self) -> u64 {
        self.minted = self.minted.wrapping_add(1);
        self.minted
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWorkspaceswebAwserminecontrolplaneserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWorkspaceswebAwserminecontrolplaneserviceState>)
}

/// The region to mint ARNs and endpoints in for this request: the region of the SigV4
/// credential scope the caller signed with, falling back to [`DEFAULT_REGION`].
///
/// A scope region that is not an AWS-shaped region token is ignored rather than spliced
/// into an ARN, because the modelled `ARN` pattern constrains that segment to
/// `[a-zA-Z0-9\-]*`.
pub fn region_for(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region.to_ascii_lowercase())
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A freshly minted resource id, in the 36-character hyphenated form the modelled `ARN`
/// pattern's resource segment (`(\/[a-fA-F0-9\-]{36})+`) requires.
///
/// The 16 bytes it renders are SHA-256 — the maintained `sha2` crate, nothing hand-rolled
/// here — over the store's own monotonic mint counter and the instant of the call, each
/// followed by a NUL so no two inputs whose concatenation coincides mix alike. The
/// counter is what makes two ids minted inside one clock tick differ, and nothing of the
/// caller's request goes into it, so an id is never an echo of the input. Truncated to 16
/// bytes it is an OPAQUE id, not an unguessable one: nothing authenticates on it.
pub fn new_resource_id(sequence: u64, at: SystemTime) -> String {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    digest.update(sequence.to_be_bytes());
    digest.update(b"\0");
    digest.update(nanos_since_epoch(at).to_be_bytes());
    digest.update(b"\0");
    let hex = ::hex::encode(&digest.finalize()[..16]);
    // The 8-4-4-4-12 grouping of the 36-character form the modelled pattern admits.
    let mut id = String::with_capacity(36);
    let mut at = 0usize;
    for (index, group) in [8usize, 4, 4, 4, 12].into_iter().enumerate() {
        if index > 0 {
            id.push('-');
        }
        id.push_str(&hex[at..at + group]);
        at += group;
    }
    id
}

/// Nanoseconds since the Unix epoch, saturating rather than panicking on a pre-epoch
/// clock (which only a test harness could produce).
fn nanos_since_epoch(at: SystemTime) -> u128 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0)
}

/// The ARN of a workspaces-web resource: `arn:aws:workspaces-web:<region>:<account>:<kind>/<id>`.
pub fn resource_arn(region: &str, kind: &str, id: &str) -> String {
    ::engine::emulator_arn(SIGNING_NAME, region, &format!("{kind}/{id}"))
}

/// The hostname users reach a portal at, built from the portal's own id and region so it
/// satisfies the modelled `PortalEndpoint` pattern (lowercase DNS labels).
pub fn portal_endpoint_for(region: &str, id: &str) -> String {
    format!("{id}.{SIGNING_NAME}.{region}.amazonaws.com")
}

/// Milliseconds since the Unix epoch, saturating rather than panicking on a pre-epoch
/// clock (which only a test harness could produce).
pub fn millis_since_epoch(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// The modelled timestamp for a stored instant.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis_since_epoch(at))
}

/// Encode a `nextToken` that resumes immediately after the page key it is given.
///
/// The token is opaque to the caller: it is the hex of the page key's text form, which
/// satisfies the modelled `PaginationToken` pattern `^\S+$` for every key.
pub fn encode_page_token(key: (i64, &str)) -> String {
    ::hex::encode(format!("{}#{}", key.0, key.1))
}

/// Decode a `nextToken` back into the page key it resumes after, or `None` when the
/// token is not of the form this service issues.
///
/// It identifies a position, not a grant: any value that decodes to one names a page
/// boundary, which is all a pagination token is for. What it cannot do is name a
/// position in anyone else's collection - the key is matched against this service's own
/// page order, so an unrecognised one simply resumes nothing.
pub fn decode_page_token(token: &str) -> Option<(i64, String)> {
    let text = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (millis, arn) = text.split_once('#')?;
    Some((millis.parse().ok()?, arn.to_string()))
}

/// `ValidationExceptionReason` for a member whose value broke a constraint.
pub const VALIDATION_REASON_FIELD: &str = "fieldValidationFailed";

/// `ValidationExceptionReason` for a value the service could not parse at all.
pub const VALIDATION_REASON_CANNOT_PARSE: &str = "cannotParse";

/// A modelled `ValidationException` naming the member that broke a constraint and the
/// constraint it broke, so the caller can fix the request from the response alone.
pub fn field_validation(member: &str, requirement: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("{member} {requirement}")),
        reason: Some(VALIDATION_REASON_FIELD.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: requirement.to_string(),
        }]),
    }
}

/// A modelled `ValidationException` for a value this service could not parse.
pub fn cannot_parse(member: &str, requirement: &str) -> ValidationException {
    ValidationException {
        message: Some(format!("{member} {requirement}")),
        reason: Some(VALIDATION_REASON_CANNOT_PARSE.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: requirement.to_string(),
        }]),
    }
}

/// Refuse `value` unless it is one of the values the model declares for the enum shape
/// `member` targets.
pub fn require_enum_value(
    member: &str,
    value: &str,
    allowed: &[&str],
) -> Result<(), ValidationException> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(field_validation(
            member,
            &format!(
                "must satisfy enum value set: [{}]; got '{value}'",
                allowed.join(", ")
            ),
        ))
    }
}

/// A modelled `ResourceNotFoundException` for an ARN this service holds no record of.
pub fn not_found(resource_type: &str, arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Could not find {resource_type} with ARN {arn}")),
        resource_id: Some(arn.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// A modelled `ConflictException` for a resource whose current associations forbid the
/// requested change.
pub fn conflict(message: impl Into<String>, resource_type: &str, arn: &str) -> ConflictException {
    ConflictException {
        message: Some(message.into()),
        resource_id: Some(arn.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// Emit the structured audit record a state-changing operation owes: who asked, what
/// changed and how it ended.
///
/// Recorded under the `emulator::audit` target - the target this tree's other services
/// record under, so one operator filter catches them all - and carrying only request
/// metadata: the operation, the access key id the caller presented
/// (`emulator_server::extract_akid` - a public identifier, never a secret), the credential
/// scope's region, the resource the call changed, and the outcome. No request body, no
/// secret and no credential material is recorded.
pub fn audit(operation: &str, parts: &::http::request::Parts, resource: &str, outcome: &str) {
    ::tracing::info!(
        target: "emulator::audit",
        operation,
        akid = %::emulator_server::extract_akid(&parts.headers),
        region = %region_for(&parts.headers),
        resource,
        outcome,
        "state-changing operation"
    );
}

/// A modelled `ServiceQuotaExceededException` for a create refused because this
/// emulator already holds as many of that resource as it stores.
pub fn quota_exceeded(resource_type: &str, limit: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "Cannot create another {resource_type}: this account already holds the \
             maximum of {limit}"
        )),
        resource_id: None,
        resource_type: Some(resource_type.to_string()),
        service_code: Some(SIGNING_NAME.to_string()),
        quota_code: None,
    }
}

/// A modelled `InternalServerException` for the one case a handler cannot proceed through
/// no fault of the request: the shared engine did not hand back this service's store.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: Some("The service state is unavailable".to_string()),
        retry_after_seconds: 0,
    }
}

/// The image format of an uploaded branding image.
///
/// One variant per value the model's `MimeType` enum declares
/// (`smithy_query(query="shape", shape="MimeType")`: `image/jpeg`, `image/png`,
/// `image/x-icon`), because that enum is exactly the set of formats the service accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrandingImageFormat {
    Png,
    Jpeg,
    Icon,
}

impl BrandingImageFormat {
    /// The `MimeType` value for this format.
    pub fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Icon => "image/x-icon",
        }
    }

    /// The file extension that names this format.
    pub fn file_extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Icon => "ico",
        }
    }

    /// The format of an inline image, read from the bytes themselves rather than from
    /// anything the caller asserted: the PNG signature (RFC 2083 section 3.1), the JPEG
    /// start-of-image marker, and the Windows icon header.
    pub fn of_bytes(bytes: &[u8]) -> Option<Self> {
        const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
        const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF];
        const ICON: &[u8] = &[0x00, 0x00, 0x01, 0x00];
        if bytes.starts_with(PNG) {
            Some(Self::Png)
        } else if bytes.starts_with(JPEG) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(ICON) {
            Some(Self::Icon)
        } else {
            None
        }
    }

    /// The format an S3 object names through its key's extension. The bytes are not
    /// reachable from here - this emulator does not read the caller's bucket - so the key
    /// is the only evidence of the format, which is why an unknown extension is refused
    /// rather than guessed. Only the object key's own extension counts: a dot in the
    /// bucket name or a trailing query is not one.
    pub fn of_s3_uri(uri: &str) -> Option<Self> {
        let key = uri.split(['?', '#']).next()?.rsplit('/').next()?;
        let extension = key.rsplit_once('.')?.1.to_ascii_lowercase();
        match extension.as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "ico" => Some(Self::Icon),
            _ => None,
        }
    }

    /// The modelled `ImageMetadata` for an image of this format uploaded at `at`.
    pub fn metadata(self, at: SystemTime) -> ImageMetadata {
        ImageMetadata {
            mime_type: self.mime_type().to_string(),
            file_extension: self.file_extension().to_string(),
            last_upload_timestamp: to_date_time(at),
        }
    }
}

/// The two forms a branding image can arrive in.
///
/// The model declares one union per image kind (`IconImageInput` for the logo and
/// favicon, `WallpaperImageInput` for the wallpaper) with the same two members, so both
/// are read through this one shape and the format rule is written once.
enum UploadedImage<'a> {
    Bytes(&'a [u8]),
    S3Uri(&'a str),
}

impl<'a> From<&'a IconImageInput> for UploadedImage<'a> {
    fn from(image: &'a IconImageInput) -> Self {
        match image {
            IconImageInput::Blob(bytes) => Self::Bytes(bytes),
            IconImageInput::s3Uri(uri) => Self::S3Uri(uri),
        }
    }
}

impl<'a> From<&'a WallpaperImageInput> for UploadedImage<'a> {
    fn from(image: &'a WallpaperImageInput) -> Self {
        match image {
            WallpaperImageInput::Blob(bytes) => Self::Bytes(bytes),
            WallpaperImageInput::s3Uri(uri) => Self::S3Uri(uri),
        }
    }
}

/// The format of an image the request carried, or a `ValidationException` naming the
/// member when neither the bytes nor the key identify one of the accepted formats.
fn image_format(
    member: &str,
    image: UploadedImage<'_>,
) -> Result<BrandingImageFormat, ValidationException> {
    let format = match image {
        UploadedImage::Bytes(bytes) => BrandingImageFormat::of_bytes(bytes),
        UploadedImage::S3Uri(uri) => BrandingImageFormat::of_s3_uri(uri),
    };
    format.ok_or_else(|| {
        field_validation(
            member,
            "must be an image of one of the types MimeType declares (image/png, \
             image/jpeg, image/x-icon), identified by the blob's own signature or by the \
             s3Uri object key's file extension",
        )
    })
}

/// Project a `BrandingConfigurationCreateInput` into the modelled `BrandingConfiguration`
/// the read operations report: every image's metadata is derived from the image itself
/// plus the instant it was uploaded, and nothing is echoed that the request did not carry.
pub fn branding_configuration(
    input: &BrandingConfigurationCreateInput,
    at: SystemTime,
) -> Result<BrandingConfiguration, ValidationException> {
    require_enum_value(
        "brandingConfigurationInput.colorTheme",
        &input.color_theme,
        COLOR_THEME_VALUES,
    )?;
    for locale in input.localized_strings.keys() {
        require_enum_value(
            "brandingConfigurationInput.localizedStrings",
            locale,
            LOCALE_VALUES,
        )?;
    }
    let logo = image_format("brandingConfigurationInput.logo", (&input.logo).into())?.metadata(at);
    let favicon =
        image_format("brandingConfigurationInput.favicon", (&input.favicon).into())?.metadata(at);
    let wallpaper = match input.wallpaper.as_ref() {
        None => None,
        Some(image) => Some(
            image_format("brandingConfigurationInput.wallpaper", image.into())?.metadata(at),
        ),
    };
    Ok(BrandingConfiguration {
        logo,
        wallpaper,
        favicon,
        localized_strings: input.localized_strings.clone(),
        color_theme: input.color_theme.clone(),
        terms_of_service: input.terms_of_service.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A portal record with no association, which is where every create starts.
    fn unconfigured_portal(created_at: SystemTime) -> PortalRecord {
        PortalRecord {
            portal_arn: resource_arn(DEFAULT_REGION, PORTAL_KIND, "unit-test-portal"),
            portal_endpoint: portal_endpoint_for(DEFAULT_REGION, "unit-test-portal"),
            display_name: None,
            creation_date: created_at,
            authentication_type: DEFAULT_AUTHENTICATION_TYPE.to_string(),
            instance_type: None,
            max_concurrent_sessions: None,
            customer_managed_key: None,
            additional_encryption_context: None,
            portal_custom_domain: None,
            status_reason: None,
            browser_settings_arn: None,
            network_settings_arn: None,
            user_settings_arn: None,
            trust_store_arn: None,
            ip_access_settings_arn: None,
            data_protection_settings_arn: None,
            user_access_logging_settings_arn: None,
            session_logger_arn: None,
            tags: Vec::new(),
            fully_configured_since: None,
        }
    }

    /// One row of the status table: case name, the user-settings and network-settings
    /// associations the portal holds, how long after the completing call it is read, and
    /// the status it must report.
    type StatusCase = (
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        Duration,
        &'static str,
    );

    /// The status rule, over the whole class of records rather than the one state the
    /// implemented operations can reach today: a portal missing either required
    /// association is `Incomplete`, one that has just gained both is `Pending` for the
    /// activation span, and `Active` after it. `AssociateNetworkSettings` is still a
    /// scaffold, so this is the only place the second and third rows are exercised.
    #[test]
    fn the_portal_status_rule_holds_for_every_configuration_and_instant() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let span = PORTAL_ACTIVATION_SPAN;

        let cases: &[StatusCase] = &[
            ("no association", None, None, Duration::ZERO, PORTAL_STATUS_INCOMPLETE),
            (
                "user settings only",
                Some("user"),
                None,
                Duration::ZERO,
                PORTAL_STATUS_INCOMPLETE,
            ),
            (
                "network settings only",
                None,
                Some("network"),
                Duration::ZERO,
                PORTAL_STATUS_INCOMPLETE,
            ),
            (
                "both, read immediately",
                Some("user"),
                Some("network"),
                Duration::ZERO,
                PORTAL_STATUS_PENDING,
            ),
            (
                "both, read one millisecond before the span is due",
                Some("user"),
                Some("network"),
                span - Duration::from_millis(1),
                PORTAL_STATUS_PENDING,
            ),
            (
                "both, read when the span is due",
                Some("user"),
                Some("network"),
                span,
                PORTAL_STATUS_ACTIVE,
            ),
            (
                "both, read long after",
                Some("user"),
                Some("network"),
                span * 100,
                PORTAL_STATUS_ACTIVE,
            ),
        ];

        for (case, user_settings, network_settings, elapsed, expected) in cases {
            let mut portal = unconfigured_portal(now);
            portal.user_settings_arn = user_settings.map(|id| {
                resource_arn(DEFAULT_REGION, USER_SETTINGS_KIND, id)
            });
            portal.network_settings_arn =
                network_settings.map(|id| resource_arn(DEFAULT_REGION, "networkSettings", id));
            portal.note_configuration_change(now);

            assert_eq!(
                portal.portal_status(now + *elapsed),
                *expected,
                "case {case:?}: expected {expected}, got {actual}",
                actual = portal.portal_status(now + *elapsed)
            );
        }
    }

    /// Removing an association puts the portal back to `Incomplete` and restarts the
    /// span, so the status is never stale.
    #[test]
    fn losing_an_association_restarts_the_activation_span() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut portal = unconfigured_portal(now);
        portal.user_settings_arn = Some(resource_arn(DEFAULT_REGION, USER_SETTINGS_KIND, "u"));
        portal.network_settings_arn = Some(resource_arn(DEFAULT_REGION, "networkSettings", "n"));
        portal.note_configuration_change(now);
        assert_eq!(
            portal.portal_status(now + PORTAL_ACTIVATION_SPAN),
            PORTAL_STATUS_ACTIVE,
            "case \"configured\": expected Active once the span is due"
        );

        portal.network_settings_arn = None;
        portal.note_configuration_change(now + PORTAL_ACTIVATION_SPAN);
        assert_eq!(
            portal.portal_status(now + PORTAL_ACTIVATION_SPAN),
            PORTAL_STATUS_INCOMPLETE,
            "case \"disassociated\": expected Incomplete once an association is gone"
        );

        let later = now + PORTAL_ACTIVATION_SPAN * 2;
        portal.network_settings_arn = Some(resource_arn(DEFAULT_REGION, "networkSettings", "n"));
        portal.note_configuration_change(later);
        assert_eq!(
            portal.portal_status(later),
            PORTAL_STATUS_PENDING,
            "case \"re-associated\": expected the span to restart, so Pending again"
        );
    }

    /// The pagination token is opaque but exactly reversible, and nothing outside the
    /// form this service issues decodes at all.
    #[test]
    fn a_page_token_round_trips_and_rejects_anything_else() {
        for (case, millis, arn) in [
            ("epoch", 0_i64, "arn:aws:workspaces-web:us-east-1:000000000000:portal/a"),
            ("negative instant", -1, "arn:x"),
            ("large instant", i64::MAX, "arn:aws:workspaces-web::0:portal/b#c"),
        ] {
            let token = encode_page_token((millis, arn));
            assert_eq!(
                decode_page_token(&token),
                Some((millis, arn.to_string())),
                "case {case:?}: the token must decode to the key it encoded"
            );
        }
        for (case, token) in [
            ("not hexadecimal", "zz"),
            ("odd-length hex", "abc"),
            ("hex without the separator", "616263"),
            ("hex with a non-numeric position", "7a23617272"),
        ] {
            assert_eq!(
                decode_page_token(token),
                None,
                "case {case:?}: a value outside the form this service issues must not \
                 decode"
            );
        }
    }
}
