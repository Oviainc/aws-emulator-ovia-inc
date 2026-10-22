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
    AuthenticationDescription, AuthenticationSummary, AwsSsoAuthentication,
    NetworkAccessConfiguration, ResourceNotFoundException, SamlAuthentication, SamlConfiguration,
    VpcConfiguration, WorkspaceDescription, WorkspaceSummary,
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

/// The largest page `ListWorkspaces` serves, and the page size it uses when the
/// request names no `maxResults`: `ListWorkspacesRequest$maxResults` is modelled
/// `@range(min: 1, max: 100)`.
pub const WORKSPACE_PAGE_LIMIT: i32 = 100;

/// The largest page `ListVersions` serves, and the page size it uses when the
/// request names no `maxResults`: `ListVersionsRequest$maxResults` is modelled
/// `@range(min: 1, max: 100)`.
pub const VERSION_PAGE_LIMIT: i32 = 100;

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
    pub tags: BTreeMap<String, String>,
    pub created: SystemTime,
    pub modified: SystemTime,
}

impl Workspace {
    /// Materialise the status change the clock owes this workspace, if `now` is
    /// past its due instant.
    ///
    /// Only `status` moves: `modified` reports the last call that changed the
    /// workspace, and the asynchronous work this settles was started by a call
    /// that already bumped it.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.status = pending.status.clone();
        self.pending = None;
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

    fn tag_map(&self) -> HashMap<String, String> {
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

/// The `ResourceNotFoundException` payload for a workspace id that names no
/// workspace.
pub fn workspace_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Workspace {id} not found"),
        resource_id: id.to_string(),
        resource_type: WORKSPACE_RESOURCE_TYPE.to_string(),
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
