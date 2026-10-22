// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-grafana-awsgrafanacontrolplane service.
//!
//! The workspace is the primary resource of the Amazon Managed Grafana control
//! plane: `CreateWorkspace` mints one, `DescribeWorkspace` / `ListWorkspaces`
//! report it, `UpdateWorkspace` mutates it and `DeleteWorkspace` removes it.
//! The record below is the one copy of a workspace every one of those handlers
//! reads and writes, plus the two projections the model asks for
//! (`WorkspaceDescription` and `WorkspaceSummary`).

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AuthenticationDescription, AuthenticationSummary, AwsSsoAuthentication, ConflictException,
    NetworkAccessConfiguration, ResourceNotFoundException, SamlAuthentication, SamlConfiguration,
    ServiceAccountSummary, ValidationException, ValidationExceptionField, VpcConfiguration,
    WorkspaceDescription, WorkspaceSummary,
};

/// The Grafana versions this emulator supports, OLDEST FIRST.
///
/// This is the emulator's single statement of the supported set: the catalogue
/// `ListVersions` publishes ("Lists available versions of Grafana. These are
/// available when calling CreateWorkspace") and the one
/// [`DEFAULT_GRAFANA_VERSION`] reads "latest" off. The entries are the major
/// versions Amazon Managed Grafana names in this service's own model docs —
/// `CreateWorkspaceRequest$grafanaVersion` ("defaults to the latest version (for
/// example, 10.4)") and `UpdateWorkspaceConfigurationRequest$grafanaVersion`
/// ("Can only be used to upgrade (for example, from 8.4 to 9.4)").
///
/// Oldest first is what makes the slice its own total order, so a `ListVersions`
/// page can be cut at any index and resumed from the version its `nextToken`
/// names.
pub const SUPPORTED_GRAFANA_VERSIONS: &[&str] = &["8.4", "9.4", "10.4"];

/// The Grafana version a workspace runs when `CreateWorkspace` names none.
///
/// `CreateWorkspaceRequest$grafanaVersion` documents the default as "the latest
/// version (for example, 10.4)", so it is the newest entry of
/// [`SUPPORTED_GRAFANA_VERSIONS`] rather than a second, independent constant.
pub const DEFAULT_GRAFANA_VERSION: &str =
    SUPPORTED_GRAFANA_VERSIONS[SUPPORTED_GRAFANA_VERSIONS.len() - 1];

/// The workspace configuration stored when `CreateWorkspace` names none.
///
/// `OverridableConfigurationJson` is a JSON document bounded to 2..65536 bytes;
/// an empty object is the document that overrides nothing.
pub const DEFAULT_WORKSPACE_CONFIGURATION: &str = "{}";

/// The region a workspace endpoint is built for when the request carries no
/// SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `AuthenticationProviderTypes` value naming SAML 2.0 authentication, one
/// of the two that enum admits (`AWS_SSO`, `SAML`).
pub const SAML_PROVIDER: &str = "SAML";

/// The `AuthenticationProviderTypes` value naming IAM Identity Center
/// authentication.
pub const AWS_SSO_PROVIDER: &str = "AWS_SSO";

/// `SamlConfigurationStatus$CONFIGURED`: "SAML on an AMG workspace is enabled
/// and has been configured".
pub const SAML_CONFIGURED: &str = "CONFIGURED";

/// `SamlConfigurationStatus$NOT_CONFIGURED`: "SAML on an AMG workspace is
/// enabled but has not been configured".
pub const SAML_NOT_CONFIGURED: &str = "NOT_CONFIGURED";

/// The `resourceType` a `ResourceNotFoundException` reports for a workspace id
/// that names no workspace.
pub const WORKSPACE_RESOURCE_TYPE: &str = "Workspace";

/// The `resourceType` a `ResourceNotFoundException` or a `ConflictException`
/// reports for a workspace API key.
pub const API_KEY_RESOURCE_TYPE: &str = "ApiKey";

/// The permission levels a workspace API key can carry.
///
/// `CreateWorkspaceApiKeyRequest$keyRole` is modelled as a plain `String`, but it
/// documents "Valid values: ADMIN|EDITOR|VIEWER" — the same three values this
/// service's own `Role` enum shape declares. A role outside this set is what
/// `CreateWorkspaceApiKey` refuses with the modelled `ValidationException`.
pub const API_KEY_ROLES: [&str; 3] = ["ADMIN", "EDITOR", "VIEWER"];

/// How many API keys one workspace holds before `CreateWorkspaceApiKey` refuses
/// with the modelled `ServiceQuotaExceededException`.
///
/// Like [`WORKSPACE_QUOTA`] this is the emulator's OWN bound on the store rather
/// than a claim about any account's quota: it is what stops the per-workspace key
/// map growing without end. Every record it holds is released by
/// `DeleteWorkspaceApiKey`, by the key's own expiry, or by `DeleteWorkspace`,
/// which drops the workspace record the keys live on.
pub const API_KEY_QUOTA: usize = 100;

/// The `resourceType` an error reports for a service account of a workspace.
pub const SERVICE_ACCOUNT_RESOURCE_TYPE: &str = "ServiceAccount";

/// The lowest Grafana major version a workspace must run for it to hold service
/// accounts.
///
/// `CreateWorkspaceServiceAccount` documents the rule: "You can only create
/// service accounts for workspaces that are compatible with Grafana version 9
/// and above."
pub const SERVICE_ACCOUNT_MIN_GRAFANA_MAJOR: u64 = 9;

/// How many service accounts one workspace holds before a create is refused with
/// the modelled `ServiceQuotaExceededException`.
///
/// Like [`WORKSPACE_QUOTA`], this is the emulator's OWN bound on the store rather
/// than a claim about any account's Service Quotas value: it keeps the state from
/// growing without end, and sits far above any workflow an emulator serves. Every
/// record it bounds is released with its workspace, which owns it.
pub const SERVICE_ACCOUNT_QUOTA: usize = 100;

/// The largest page `ListWorkspaces` serves, and the page size it uses when the
/// request names no `maxResults`: `ListWorkspacesRequest$maxResults` is modelled
/// `@range(min: 1, max: 100)`.
pub const WORKSPACE_PAGE_LIMIT: i32 = 100;

/// The largest page `ListVersions` serves, and the page size it uses when the
/// request names no `maxResults`: `ListVersionsRequest$maxResults` is modelled
/// `@range(min: 1, max: 100)`.
pub const VERSION_PAGE_LIMIT: i32 = 100;

/// The most tags one resource holds.
///
/// `TagMap` is modelled `@length(max: 50)`, so a single request can carry 50 tag
/// pairs; the same bound is what a tagged resource can hold, and `TagResource`
/// refuses a request that would take a workspace past it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The shortest `TagKey` the model admits: `@length(min: 1, max: 128)`.
pub const MIN_TAG_KEY_LENGTH: usize = 1;

/// The longest `TagKey` the model admits: `@length(min: 1, max: 128)`.
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The largest `TagValue` the model admits: `@length(min: 0, max: 256)`, so an
/// empty value is a value and there is no lower bound to check.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The resource-type segment of an Amazon Managed Grafana workspace ARN:
/// `arn:aws:grafana:<region>:<account>:/workspaces/<workspace-id>`.
pub const WORKSPACE_ARN_RESOURCE_TYPE: &str = "workspaces";

/// How many workspaces the store holds before `CreateWorkspace` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// Amazon Managed Grafana publishes an adjustable quota on workspaces per account
/// per Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is set far above any workflow an emulator
/// serves, and it is what stops the state growing without end: every record it
/// holds is released by `DeleteWorkspace`, which also forgets that workspace's
/// client tokens, so the token map can never outgrow this bound either.
pub const WORKSPACE_QUOTA: usize = 1_000;

/// How long a workspace stays in the status the call that started the work
/// returned (`CREATING` after a create, `UPDATING` after an update) before the
/// clock owes it `ACTIVE`.
///
/// Amazon Managed Grafana takes minutes; the emulator chooses a span, and it is
/// deliberately NOT zero so a caller acting straight after the call still
/// observes the transient status. The instant is stored on the record and
/// materialised by whichever later call reads it — nothing sleeps.
pub const LIFECYCLE_TRANSITION: Duration = Duration::from_millis(500);

/// One Grafana API key of a workspace, as this emulator persists it.
///
/// The key TOKEN is not here: it is a secret handed to the caller once, by
/// `CreateWorkspaceApiKey`, and no operation reports it again — so nothing in the
/// store needs to hold it.
#[derive(Debug, Clone)]
pub struct ApiKey {
    /// `ApiKeyName`, unique within the workspace ("Keynames must be unique to
    /// the workspace").
    pub name: String,
    /// One of [`API_KEY_ROLES`].
    pub role: String,
    pub created: SystemTime,
    /// `created` plus the request's `secondsToLive` ("the time in seconds until
    /// the key expires"). Past this instant the key is no longer a key of the
    /// workspace.
    pub expires: SystemTime,
}

/// A status change the clock owes a workspace: the instant it falls due, and the
/// status the workspace carries from then on.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due: SystemTime,
    pub status: String,
}

/// One Amazon Managed Grafana workspace, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// `^g-[0-9a-f]{10}$`, minted by [`ComAmazonawsGrafanaAwsgrafanacontrolplaneState::mint_workspace_id`].
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub account_access_type: String,
    pub permission_type: String,
    pub authentication_providers: Vec<String>,
    /// One of `WorkspaceStatus`'s values.
    pub status: String,
    /// The status change the clock owes this workspace, if any.
    pub pending: Option<PendingTransition>,
    pub endpoint: String,
    pub grafana_version: String,
    pub data_sources: Vec<String>,
    pub notification_destinations: Vec<String>,
    pub organizational_units: Vec<String>,
    pub organization_role_name: Option<String>,
    pub stack_set_name: Option<String>,
    pub workspace_role_arn: Option<String>,
    pub vpc_configuration: Option<VpcConfiguration>,
    pub network_access_control: Option<NetworkAccessConfiguration>,
    pub ip_address_type: Option<String>,
    pub kms_key_id: Option<String>,
    /// The `OverridableConfigurationJson` document the workspace was created or
    /// updated with.
    pub configuration: String,
    /// The SAML configuration stored for this workspace, if one has been
    /// stored. `UpdateWorkspaceAuthentication` is the only operation that
    /// carries a `samlConfiguration`, so this is unset until it is called —
    /// which is what makes a SAML workspace's `samlConfigurationStatus`
    /// `NOT_CONFIGURED` until then.
    pub saml_configuration: Option<SamlConfiguration>,
    /// Set by `AssociateLicense`; unset until then.
    pub license_type: Option<String>,
    /// This workspace's Grafana API keys, keyed by their name: what
    /// `CreateWorkspaceApiKey` mints and `DeleteWorkspaceApiKey` removes. They
    /// live on the workspace record, so deleting the workspace forgets them too.
    pub api_keys: BTreeMap<String, ApiKey>,
    pub tags: BTreeMap<String, String>,
    /// Every service account of this workspace, in the order
    /// `CreateWorkspaceServiceAccount` minted them — the stable total order
    /// `ListWorkspaceServiceAccounts` pages through. They live on the workspace
    /// record because they belong to it: `DeleteWorkspace` dropping the
    /// workspace releases them with it.
    pub service_accounts: Vec<ServiceAccount>,
    /// The counter this workspace's service-account ids are minted from.
    pub service_account_id_seq: u64,
    pub created: SystemTime,
    pub modified: SystemTime,
}

impl Workspace {
    /// Materialise everything the clock owes this workspace by `now`: the status
    /// change that has fallen due, and the expiry of any API key whose
    /// `secondsToLive` has run out.
    ///
    /// Only those move: `modified` reports the last call that changed the
    /// workspace, and the asynchronous work this settles was started by a call
    /// that already bumped it.
    pub fn settle(&mut self, now: SystemTime) {
        let due = self
            .pending
            .as_ref()
            .filter(|pending| now >= pending.due)
            .map(|pending| pending.status.clone());
        if let Some(status) = due {
            self.status = status;
            self.pending = None;
        }
        // An expired key is gone whether or not anybody called: the instant its
        // `secondsToLive` ran out is when it stopped being a key of this
        // workspace.
        self.api_keys.retain(|_, key| now < key.expires);
    }

    /// Whether this workspace may hold service accounts at all.
    ///
    /// `CreateWorkspaceServiceAccount` documents that only workspaces
    /// "compatible with Grafana version 9 and above" can have them, so the major
    /// component of the version decides it. The comparison goes through
    /// [`version_order`], the one place this crate reads a Grafana version as
    /// numbers: a component that is not a number contributes 0, so a version
    /// this emulator cannot classify is below the floor rather than clearing it.
    pub fn supports_service_accounts(&self) -> bool {
        version_order(&self.grafana_version)
            .first()
            .is_some_and(|major| *major >= SERVICE_ACCOUNT_MIN_GRAFANA_MAJOR)
    }

    /// This workspace's service account named `name`, if it holds one.
    ///
    /// `CreateWorkspaceServiceAccountRequest$name` documents that "the name must
    /// be unique within the workspace", so this is what a create consults
    /// before minting another.
    pub fn service_account_named(&self, name: &str) -> Option<&ServiceAccount> {
        self.service_accounts
            .iter()
            .find(|account| account.name == name)
    }

    /// Mint an id no service account of this workspace holds.
    ///
    /// Grafana identifies a service account by a positive integer, rendered as a
    /// string on the wire (`ServiceAccountSummary$id` is a plain `String`), so
    /// the id is the decimal counter value. The loop skips an id already taken,
    /// which is what makes the wrap safe.
    pub fn mint_service_account_id(&mut self) -> String {
        loop {
            self.service_account_id_seq = self.service_account_id_seq.wrapping_add(1);
            let id = self.service_account_id_seq.to_string();
            if !self.service_accounts.iter().any(|account| account.id == id) {
                return id;
            }
        }
    }

    /// True when `provider` is one of this workspace's stored authentication
    /// providers.
    fn authenticates_with(&self, provider: &str) -> bool {
        self.authentication_providers.iter().any(|p| p == provider)
    }

    /// `SamlConfigurationStatus` for this workspace: "whether the workspace's
    /// SAML configuration is complete", which it is exactly once a SAML
    /// configuration has been stored for it.
    fn saml_configuration_status(&self) -> String {
        if self.saml_configuration.is_some() {
            SAML_CONFIGURED.to_string()
        } else {
            SAML_NOT_CONFIGURED.to_string()
        }
    }

    /// `AuthenticationSummary` for this workspace.
    ///
    /// `samlConfigurationStatus` reports whether the workspace's SAML
    /// configuration is complete, so it is reported only for a workspace that
    /// authenticates with SAML at all.
    fn authentication(&self) -> AuthenticationSummary {
        AuthenticationSummary {
            providers: self.authentication_providers.clone(),
            saml_configuration_status: self
                .authenticates_with(SAML_PROVIDER)
                .then(|| self.saml_configuration_status()),
        }
    }

    /// The `AuthenticationDescription` `DescribeWorkspaceAuthentication` and
    /// `UpdateWorkspaceAuthentication` return.
    ///
    /// `providers` is the stored provider list; `saml` describes "how this
    /// workspace works with SAML" and `awsSso` "how this workspace works with
    /// IAM Identity Center", so each is reported exactly when the workspace
    /// authenticates with that provider.
    pub fn to_authentication_description(&self) -> AuthenticationDescription {
        AuthenticationDescription {
            providers: self.authentication_providers.clone(),
            saml: self
                .authenticates_with(SAML_PROVIDER)
                .then(|| SamlAuthentication {
                    status: self.saml_configuration_status(),
                    configuration: self.saml_configuration.clone(),
                }),
            aws_sso: self
                .authenticates_with(AWS_SSO_PROVIDER)
                .then_some(AwsSsoAuthentication {
                    // `ssoClientId` is "the ID of the IAM Identity Center-managed
                    // application that is created by Amazon Managed Grafana".
                    // That application is minted in IAM Identity Center, which
                    // this emulator does not model, so the optional member is
                    // omitted rather than invented.
                    sso_client_id: None,
                }),
        }
    }

    /// This workspace's tags, as the `TagMap` every operation that reports them
    /// serialises: `WorkspaceDescription$tags`, `WorkspaceSummary$tags` and
    /// `ListTagsForResourceResponse$tags` are all this one map.
    pub fn tag_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// The `WorkspaceDescription` the create / describe / update / delete
    /// operations return.
    pub fn to_description(&self) -> WorkspaceDescription {
        WorkspaceDescription {
            account_access_type: Some(self.account_access_type.clone()),
            created: ::aws_smithy_types::DateTime::from(self.created),
            data_sources: self.data_sources.clone(),
            description: self.description.clone(),
            endpoint: self.endpoint.clone(),
            grafana_version: self.grafana_version.clone(),
            id: self.id.clone(),
            modified: ::aws_smithy_types::DateTime::from(self.modified),
            name: self.name.clone(),
            organization_role_name: self.organization_role_name.clone(),
            notification_destinations: Some(self.notification_destinations.clone()),
            organizational_units: Some(self.organizational_units.clone()),
            permission_type: Some(self.permission_type.clone()),
            stack_set_name: self.stack_set_name.clone(),
            status: self.status.clone(),
            workspace_role_arn: self.workspace_role_arn.clone(),
            license_type: self.license_type.clone(),
            // A free trial is consumed by associating an `ENTERPRISE_FREE_TRIAL`
            // license, so the answer is whatever licence this workspace holds.
            free_trial_consumed: Some(self.license_type.as_deref() == Some("ENTERPRISE_FREE_TRIAL")),
            // Both expirations belong to an associated Grafana Enterprise
            // licence; `AssociateLicense` is the operation that would set one.
            license_expiration: None,
            free_trial_expiration: None,
            authentication: self.authentication(),
            tags: Some(self.tag_map()),
            vpc_configuration: self.vpc_configuration.clone(),
            network_access_control: self.network_access_control.clone(),
            // The Grafana Labs token is minted by `AssociateLicense`.
            grafana_token: None,
            ip_address_type: self.ip_address_type.clone(),
            kms_key_id: self.kms_key_id.clone(),
            // Only reported "if the workspace is in the DEGRADED status".
            degraded_workspace_reason: None,
        }
    }

    /// The `WorkspaceSummary` projection `ListWorkspaces` returns.
    pub fn to_summary(&self) -> WorkspaceSummary {
        WorkspaceSummary {
            created: ::aws_smithy_types::DateTime::from(self.created),
            description: self.description.clone(),
            endpoint: self.endpoint.clone(),
            grafana_version: self.grafana_version.clone(),
            id: self.id.clone(),
            modified: ::aws_smithy_types::DateTime::from(self.modified),
            name: self.name.clone(),
            notification_destinations: Some(self.notification_destinations.clone()),
            status: self.status.clone(),
            authentication: self.authentication(),
            tags: Some(self.tag_map()),
            license_type: self.license_type.clone(),
            grafana_token: None,
        }
    }
}

/// One Grafana service account of a workspace, as this emulator persists it.
///
/// A service account is the identity `CreateWorkspaceServiceAccountToken` mints
/// tokens for; `ListWorkspaceServiceAccounts` reports it as a
/// [`ServiceAccountSummary`].
#[derive(Debug, Clone)]
pub struct ServiceAccount {
    /// The id minted by [`Workspace::mint_service_account_id`].
    pub id: String,
    /// The name the create named, unique within the workspace.
    pub name: String,
    /// One of `Role`'s values: `ADMIN`, `EDITOR` or `VIEWER`.
    pub grafana_role: String,
    /// Service accounts "can be disabled and enabled in the Amazon Managed
    /// Grafana console", which this emulator does not model, so an account is
    /// enabled from the moment it is created.
    pub disabled: bool,
    pub created: SystemTime,
}

impl ServiceAccount {
    /// The `ServiceAccountSummary` projection `ListWorkspaceServiceAccounts`
    /// returns.
    ///
    /// `isDisabled` is modelled as a `String` ("Returns true if the service
    /// account is disabled"), so the boolean is rendered as `true` / `false`.
    pub fn to_summary(&self) -> ServiceAccountSummary {
        ServiceAccountSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            is_disabled: self.disabled.to_string(),
            grafana_role: self.grafana_role.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-grafana-awsgrafanacontrolplane. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGrafanaAwsgrafanacontrolplaneState {
    /// Every workspace, keyed by its id. A `BTreeMap` is what gives
    /// `ListWorkspaces` the stable total order its pagination token indexes
    /// into — ids sort in the order they were minted.
    pub workspaces: BTreeMap<String, Workspace>,
    /// A `CreateWorkspace` `clientToken` to the workspace that token created,
    /// so a repeated create answers with the first workspace instead of a
    /// second one.
    pub idempotency_tokens: HashMap<String, String>,
    /// The counter workspace ids are minted from.
    pub workspace_id_seq: u64,
}

impl ComAmazonawsGrafanaAwsgrafanacontrolplaneState {
    /// Mint an unused workspace id matching `WorkspaceId`'s `^g-[0-9a-f]{10}$`.
    ///
    /// Ten hex digits hold 40 bits, so the counter is masked to that width; the
    /// loop skips an id already taken, which is what makes the wrap safe.
    pub fn mint_workspace_id(&mut self) -> String {
        loop {
            self.workspace_id_seq = self.workspace_id_seq.wrapping_add(1);
            let id = format!("g-{:010x}", self.workspace_id_seq & 0xff_ffff_ffff);
            if !self.workspaces.contains_key(&id) {
                return id;
            }
        }
    }

    /// The workspace `id` names, with any status change the clock owes it
    /// already materialised.
    pub fn settled_mut(&mut self, id: &str, now: SystemTime) -> Option<&mut Workspace> {
        let workspace = self.workspaces.get_mut(id)?;
        workspace.settle(now);
        Some(workspace)
    }

    /// Materialise every status change the clock owes, for a caller that is
    /// about to report on all of them.
    pub fn settle_all(&mut self, now: SystemTime) {
        for workspace in self.workspaces.values_mut() {
            workspace.settle(now);
        }
    }
}

/// The URL users access the Grafana console in a workspace through.
pub fn workspace_endpoint(id: &str, region: &str) -> String {
    format!("https://{id}.grafana-workspace.{region}.amazonaws.com")
}

/// True when `value` satisfies `WorkspaceId`'s `^g-[0-9a-f]{10}$`.
///
/// The boundary validator already refuses a modelled `WorkspaceId` member, so
/// this is for a value the model does not constrain — a `nextToken`, which this
/// service's pagination carries a workspace id in.
pub fn is_workspace_id(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("g-") else {
        return false;
    };
    hex.len() == 10 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The sort key of a Grafana version: its dot-separated components as numbers.
///
/// A Grafana version is written `major.minor` (`GrafanaVersion` is otherwise an
/// unconstrained 1..255 string), so the components must be compared as NUMBERS:
/// byte order puts "10.4" before "9.4", which would report a newer version as
/// older. A component that is not a number contributes 0, which keeps the key
/// total over any string a workspace may carry.
pub fn version_order(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|component| {
            let digits: String = component.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().unwrap_or(0)
        })
        .collect()
}

/// True when moving a workspace from `current` to `requested` is a DOWNGRADE.
///
/// `UpdateWorkspaceConfigurationRequest$grafanaVersion` "[c]an only be used to
/// upgrade (for example, from 8.4 to 9.4), not downgrade (for example, from 9.4
/// to 8.4)", so the version a workspace already runs is admitted and only a
/// strictly older one is refused. It reads the SAME [`version_order`] key as
/// [`upgrade_targets`], so the refusal and the catalogue `ListVersions` publishes
/// can never disagree about which way round two versions are.
pub fn is_downgrade(current: &str, requested: &str) -> bool {
    version_order(requested) < version_order(current)
}

/// The versions a workspace running `current` can be upgraded to: the supported
/// versions strictly newer than it, oldest first.
///
/// `UpdateWorkspaceConfigurationRequest$grafanaVersion` is what consumes this
/// set — it "[c]an only be used to upgrade ..., not downgrade" — so the current
/// version and every older one is not an upgrade target.
pub fn upgrade_targets(current: &str) -> Vec<String> {
    let current = version_order(current);
    SUPPORTED_GRAFANA_VERSIONS
        .iter()
        .filter(|candidate| version_order(candidate) > current)
        .map(|candidate| (*candidate).to_string())
        .collect()
}

/// The region this request was signed for — what a workspace endpoint names.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// True when `value` parses as a JSON document.
///
/// `OverridableConfigurationJson` is `@mediaType("application/json")`, so every
/// member targeting it (`CreateWorkspaceRequest$configuration`,
/// `UpdateWorkspaceConfigurationRequest$configuration`) carries a JSON document
/// and one that does not parse is refused rather than stored.
pub fn is_json_document(value: &str) -> bool {
    ::serde_json::from_str::<::serde_json::Value>(value).is_ok()
}

/// The `ValidationException` payload for a `configuration` that is not a JSON
/// document, shared by every operation that accepts the member so the two cannot
/// refuse the same request in two different ways.
pub fn configuration_not_json() -> ValidationException {
    ValidationException {
        message: "configuration must be a JSON document".to_string(),
        reason: "FIELD_VALIDATION_FAILED".to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: "configuration".to_string(),
            message: "not a JSON document".to_string(),
        }]),
    }
}

/// The `ConflictException` payload for a service-account name a workspace
/// already holds: the existing account is the resource in the way.
pub fn service_account_name_taken(
    workspace_id: &str,
    name: &str,
    existing_id: &str,
) -> crate::ops::types::ConflictException {
    crate::ops::types::ConflictException {
        message: format!(
            "workspace {workspace_id} already holds a service account named {name}"
        ),
        resource_id: existing_id.to_string(),
        resource_type: SERVICE_ACCOUNT_RESOURCE_TYPE.to_string(),
    }
}

/// The `ResourceNotFoundException` payload for a workspace id that names no
/// workspace.
pub fn workspace_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Workspace {id} not found"),
        resource_id: id.to_string(),
        resource_type: WORKSPACE_RESOURCE_TYPE.to_string(),
    }
}

/// The `ResourceNotFoundException` payload for a key name that names no API key
/// of `workspace_id` — one never created, already deleted, or expired.
pub fn api_key_not_found(workspace_id: &str, key_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("API key {key_name} not found in workspace {workspace_id}"),
        resource_id: key_name.to_string(),
        resource_type: API_KEY_RESOURCE_TYPE.to_string(),
    }
}

/// The `ConflictException` payload for a key name the workspace already holds:
/// "Keynames must be unique to the workspace."
pub fn api_key_name_taken(workspace_id: &str, key_name: &str) -> ConflictException {
    ConflictException {
        message: format!("workspace {workspace_id} already holds an API key named {key_name}"),
        resource_id: key_name.to_string(),
        resource_type: API_KEY_RESOURCE_TYPE.to_string(),
    }
}

/// A fresh Grafana API key token, for `CreateWorkspaceApiKey`.
///
/// The token authenticates to the workspace's own Grafana HTTP API, NOT to this
/// emulator, so it must never be accepted as a SigV4 credential — but it is still
/// a secret handed to a caller, and nothing in a service may construct a secret
/// itself. It is therefore minted by the one component that generates secrets,
/// the emulator's credential store, and then removed from that store again: the
/// caller gets the secret, the emulator keeps no credential it would accept, and
/// nothing accumulates per call.
///
/// `None` when the store minted no session-token secret, which leaves the
/// operation to refuse rather than report a token it made up.
pub fn mint_api_key_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(str::to_string);
    // The token is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    token
}

/// The workspace id the resource ARN `arn` names, or `None` when `arn` does not
/// name a Grafana workspace at all.
///
/// The tagging operations take a `resourceArn` the model constrains no further
/// than `String`, and the only resource Amazon Managed Grafana can tag is a
/// workspace — so the ARN is accepted when it is an ARN of THIS service whose
/// resource section is `workspaces/<workspace-id>` (the leading `/` the service's
/// ARNs carry is optional here, since it separates nothing). The partition, the
/// region and the account segments are not matched: this emulator serves one
/// account, and the workspace id is what identifies the resource within it.
pub fn workspace_id_in_arn(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    // The partition, region and account segments must be PRESENT (an ARN has
    // six colon-separated fields) but are not matched against this request.
    let _partition = fields.next()?;
    if fields.next()? != "grafana" {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let resource = fields.next()?;
    let resource = resource.strip_prefix('/').unwrap_or(resource);
    let (resource_type, id) = resource.split_once('/')?;
    (resource_type == WORKSPACE_ARN_RESOURCE_TYPE && is_workspace_id(id)).then_some(id)
}

/// The modelled `ValidationException` for a tag whose key or value is outside the
/// bounds `TagKey` / `TagValue` are modelled with, or `None` when the pair is
/// within them.
///
/// The request-boundary validator refuses a modelled constraint on a member, but
/// does not walk a map's KEY and VALUE constraints, so the handler that writes
/// tags is what enforces these two. Lengths are counted in characters, as
/// `@length` on a string is defined.
pub fn out_of_bounds_tag(key: &str, value: &str) -> Option<ValidationException> {
    let key_length = key.chars().count();
    let value_length = value.chars().count();
    let message = if !(MIN_TAG_KEY_LENGTH..=MAX_TAG_KEY_LENGTH).contains(&key_length) {
        format!(
            "tag key {key:?} is {key_length} characters long; a tag key is \
             {MIN_TAG_KEY_LENGTH} to {MAX_TAG_KEY_LENGTH} characters"
        )
    } else if value_length > MAX_TAG_VALUE_LENGTH {
        format!(
            "the value of tag key {key:?} is {value_length} characters long; a tag \
             value is at most {MAX_TAG_VALUE_LENGTH} characters"
        )
    } else {
        return None;
    };
    Some(ValidationException {
        message: message.clone(),
        reason: "FIELD_VALIDATION_FAILED".to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: "tags".to_string(),
            message,
        }]),
    })
}

/// The modelled `ValidationException` for a `resourceArn` that names no taggable
/// Grafana resource.
///
/// `ValidationException$reason` is `FIELD_VALIDATION_FAILED`, and the field list
/// names the member at fault, exactly as the other refusals in this service do.
pub fn not_a_workspace_arn(arn: &str) -> ValidationException {
    ValidationException {
        message: format!("{arn} is not the ARN of an Amazon Managed Grafana workspace"),
        reason: "FIELD_VALIDATION_FAILED".to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: "resourceArn".to_string(),
            message: format!(
                "must be an ARN of the form arn:aws:grafana:<region>:<account>:/{WORKSPACE_ARN_RESOURCE_TYPE}/<workspaceId>"
            ),
        }]),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGrafanaAwsgrafanacontrolplaneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGrafanaAwsgrafanacontrolplaneState>)
}
