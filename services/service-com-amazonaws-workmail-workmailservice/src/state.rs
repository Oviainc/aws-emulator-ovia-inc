// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workmail-workmailservice service.
//!
//! The organization is the root of every WorkMail resource: an organization owns its
//! users, and every user-facing operation resolves the organization first. So the
//! organization record owns its users (and the name/email uniqueness indexes over them)
//! rather than there being a second, parallel user map keyed by organization id.

use std::collections::HashMap;
use std::time::SystemTime;

/// How long after `CreateOrganization` the organization's mailbox infrastructure is
/// reported as provisioned. WorkMail's `State` member is a free-form string with no
/// documented value catalog, so this emulator reports the organization ready
/// ([`ORGANIZATION_STATE_ACTIVE`]) from the create call and records the instant as
/// `CompletedDate`.
///
/// It is the only value any implemented operation stores today, so the
/// `OrganizationStateException` guards that compare against it are the place
/// `DeleteOrganization` (still a stub) will become observable rather than live rules.
pub const ORGANIZATION_STATE_ACTIVE: &str = "Active";

/// Directory type reported for the directory WorkMail creates for an organization when
/// the caller supplies no `DirectoryId` — "If you choose not to associate an existing
/// directory with your organization, then we create a new WorkMail directory for you"
/// (`CreateOrganization` documentation).
pub const DIRECTORY_TYPE_WORKMAIL: &str = "VpcDirectory";

/// Directory type reported when the caller associated an existing AWS Directory Service
/// directory with the organization (`CreateOrganization`'s `DirectoryId` member).
pub const DIRECTORY_TYPE_ASSOCIATED: &str = "ADConnector";

/// The suffix of the test mail domain WorkMail gives every organization,
/// `<alias>.awsapps.com`, used as the organization's default mail domain when the
/// caller associates no domain of its own.
pub const DEFAULT_MAIL_DOMAIN_SUFFIX: &str = "awsapps.com";

/// Per-service state for com-amazonaws-workmail-workmailservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWorkmailWorkmailserviceState {
    /// Every organization, keyed by its `OrganizationId` (`m-<32 hex>`).
    pub organizations: HashMap<String, Organization>,
    /// Alias -> organization id, lowercased: an alias identifies at most one
    /// organization, which is what `NameAvailabilityException` reports.
    pub alias_index: HashMap<String, String>,
    /// Directory id -> organization id, so a directory already associated with one
    /// organization is reported with `DirectoryInUseException`.
    pub directory_index: HashMap<String, String>,
    /// `@idempotencyToken` inputs: client token -> the id the first call minted, so a
    /// retry replays that id instead of creating a second resource.
    pub client_tokens: HashMap<String, String>,
}

/// One WorkMail organization and everything it owns.
#[derive(Debug, Clone)]
pub struct Organization {
    pub organization_id: String,
    pub alias: String,
    pub state: String,
    pub directory_id: Option<String>,
    pub directory_type: Option<String>,
    pub domains: Vec<MailDomain>,
    pub default_mail_domain: Option<String>,
    pub kms_key_arn: Option<String>,
    pub interoperability_enabled: bool,
    pub arn: String,
    pub completed_date: SystemTime,
    pub error_message: Option<String>,
    pub users: HashMap<String, UserRecord>,
    /// Lowercased user name -> user id. WorkMail user names are case-insensitive
    /// identifiers (they become the local part of a mail address), so uniqueness is
    /// decided on the folded name.
    pub user_name_index: HashMap<String, String>,
    /// Lowercased email address -> user id, which is what `EmailAddressInUseException`
    /// reports.
    pub email_index: HashMap<String, String>,
}

/// A mail domain associated with an organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailDomain {
    pub domain_name: String,
    pub hosted_zone_id: Option<String>,
}

/// A user of an organization.
#[derive(Debug, Clone)]
pub struct UserRecord {
    pub user_id: String,
    pub name: String,
    pub display_name: String,
    pub email: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub role: String,
    pub state: String,
    pub hidden_from_global_address_list: bool,
    pub enabled_date: Option<SystemTime>,
    pub disabled_date: Option<SystemTime>,
    pub identity_provider_user_id: Option<String>,
    /// Held so a later `ResetPassword` can replace it; never serialized into a
    /// response (`Password` is `@sensitive` and no output shape carries it).
    pub password: Option<String>,
}

impl Organization {
    /// The user named `name`, matched case-insensitively through
    /// [`Organization::user_name_index`].
    pub fn user_id_for_name(&self, name: &str) -> Option<&String> {
        self.user_name_index.get(&name.to_ascii_lowercase())
    }

    /// Insert `user`, maintaining the name and email indexes.
    pub fn insert_user(&mut self, user: UserRecord) {
        self.user_name_index
            .insert(user.name.to_ascii_lowercase(), user.user_id.clone());
        if let Some(email) = &user.email {
            self.email_index
                .insert(email.to_ascii_lowercase(), user.user_id.clone());
        }
        self.users.insert(user.user_id.clone(), user);
    }

    /// Remove the user with `user_id`, dropping its name and email reservations so both
    /// become available again. Returns the removed record, or `None` when no such user
    /// exists — which is what makes `DeleteUser` idempotent.
    pub fn remove_user(&mut self, user_id: &str) -> Option<UserRecord> {
        let user = self.users.remove(user_id)?;
        self.user_name_index.remove(&user.name.to_ascii_lowercase());
        if let Some(email) = &user.email {
            self.email_index.remove(&email.to_ascii_lowercase());
        }
        Some(user)
    }

    /// Whether `domain` is one of this organization's mail domains, compared
    /// case-insensitively (DNS names are case-insensitive).
    pub fn has_mail_domain(&self, domain: &str) -> bool {
        self.domains
            .iter()
            .any(|d| d.domain_name.eq_ignore_ascii_case(domain))
    }

    /// This organization's users ordered by their folded name. A name identifies at most
    /// one user of an organization, so this is a stable total order — which is what lets
    /// a paginated read resume exactly where the previous page stopped.
    pub fn users_by_name(&self) -> Vec<&UserRecord> {
        let mut users: Vec<&UserRecord> = self.users.values().collect();
        users.sort_by_key(|user| user.name.to_ascii_lowercase());
        users
    }
}

/// The three states a WorkMail entity (user, group, resource) can be in, as
/// `EntityState` models them.
///
/// `DELETED` is reachable only once `DeregisterFromWorkMail` / the group and resource
/// operations are implemented; `DeleteUser` removes the record outright, so nothing
/// stores it yet.
pub mod entity_state {
    /// Created but not registered to WorkMail: it has no mailbox and no email address.
    pub const DISABLED: &str = "DISABLED";
    /// Registered to WorkMail: it has a mailbox and a primary email address.
    pub const ENABLED: &str = "ENABLED";
    /// Deregistered and deleted.
    pub const DELETED: &str = "DELETED";
}

/// The default `UserRole` for a user created without one.
pub const DEFAULT_USER_ROLE: &str = "USER";

/// The region an ARN is built for when the request carries no SigV4 credential scope to
/// read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ceiling on how many organizations this store holds, and on how many users one
/// organization holds: the two caps that bound caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process and a
/// create is the only thing that adds one, so the store needs a maximum entry count —
/// neither a TTL nor an eviction policy is available to it, because `DescribeOrganization`
/// must still answer what `CreateOrganization` created. The two operations that grow the
/// store refuse past these caps with a modelled error, and no read is affected. The
/// numbers are the emulator's own bound, not a published AWS default (WorkMail publishes
/// its organization and user quotas as adjustable, so no single value is the contract),
/// and they are set high enough that no ordinary workflow meets one.
///
/// Every other map is derived and bounded by these two: `alias_index`, `directory_index`
/// and `client_tokens` hold at most one entry per *stored* organization (each is written
/// only on the successful-create path, after the cap check), and `user_name_index` /
/// `email_index` at most one entry per stored user. [`MAX_CLIENT_TOKENS`] states that
/// relationship explicitly at the one site a caller chooses the key.
pub const MAX_ORGANIZATIONS: usize = 1_000;

/// See [`MAX_ORGANIZATIONS`].
pub const MAX_USERS_PER_ORGANIZATION: usize = 10_000;

/// See [`MAX_ORGANIZATIONS`]. The `ClientToken` replay table is keyed by a caller-chosen
/// string, so it carries its own explicit cap rather than relying only on the
/// organization cap that already bounds it.
pub const MAX_CLIENT_TOKENS: usize = MAX_ORGANIZATIONS;

/// A fresh `OrganizationId`: `m-` followed by 32 lowercase hex digits, as the
/// `OrganizationId` shape's pattern `^m-[0-9a-f]{32}$` requires.
pub fn new_organization_id() -> String {
    format!("m-{}", uuid::Uuid::new_v4().simple())
}

/// A fresh `WorkMailIdentifier` for an entity (length 12..256).
pub fn new_entity_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}

/// A fresh directory id for the directory WorkMail creates for an organization that
/// associated none of its own: `d-` followed by 10 lowercase hex digits, as the
/// `DirectoryId` shape's pattern `^d-[0-9a-f]{10}$` requires.
pub fn new_directory_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("d-{}", &hex[..10])
}

/// A `SystemTime` as the wire timestamp type every WorkMail output uses.
pub fn as_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The region this request was signed for, which is the region of the ARNs the handler
/// mints. Falls back to the emulator's default region for an unsigned request.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Encode a pagination cursor: the id a page resumes AT, hex-encoded so the token is
/// opaque to the caller and satisfies `NextToken`'s `[a-zA-Z0-9/+=]` form.
pub fn encode_page_token(resume_at: &str) -> String {
    hex::encode(resume_at.as_bytes())
}

/// Decode a cursor produced by [`encode_page_token`]. `None` for anything this service
/// did not mint, which a handler reports as an invalid parameter.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWorkmailWorkmailserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWorkmailWorkmailserviceState>)
}
