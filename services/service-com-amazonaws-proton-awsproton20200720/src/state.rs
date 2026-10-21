// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-proton-awsproton20200720 service.
//!
//! Every persisted Proton resource lives here, keyed by its name, together with the
//! small helpers several operations need: the projection of a stored record into the
//! modelled output shape, the ARN and timestamp builders, and the opaque pagination
//! token codec. Handlers reach this through [`service_state`] and never keep state of
//! their own.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{Environment, EnvironmentSummary, EnvironmentTemplate};

/// This service's SigV4 signing name, and so the service segment of every ARN it
/// mints (`smithy_query(query="service_info")`: `sigv4Name`/`endpointPrefix` = `proton`).
pub const SIGNING_NAME: &str = "proton";

/// The region an unsigned request is served in; a signed one is served in the region
/// its SigV4 credential scope names (see [`request_region`]).
pub const DEFAULT_REGION: &str = "us-east-1";

/// Members of the modelled `DeploymentStatus` enum
/// (`smithy_query(query="shape",shape="DeploymentStatus")`) that this slice reaches.
pub const DEPLOYMENT_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`DEPLOYMENT_IN_PROGRESS`].
pub const DEPLOYMENT_SUCCEEDED: &str = "SUCCEEDED";
/// See [`DEPLOYMENT_IN_PROGRESS`].
pub const DEPLOYMENT_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";

/// The only member of the modelled `Provisioning` enum
/// (`smithy_query(query="shape",shape="Provisioning")`).
pub const PROVISIONING_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// How long after the call that starts it an environment deployment — or an
/// environment deletion — falls due.
///
/// Proton answers `CreateEnvironment` with `deploymentStatus: IN_PROGRESS` and
/// `DeleteEnvironment` with `DELETE_IN_PROGRESS`, so both completions are owed by the
/// CLOCK rather than by any later call. AWS documents no duration for provisioning,
/// so this span is chosen, not cited: long enough that a caller acting straight after
/// the create still observes the transient status, short enough that no client waits
/// on it. Nothing sleeps for it — the due instant is stored on the record and
/// materialised by the next call (see
/// [`ComAmazonawsProtonAwsproton20200720State::settle_environments`]).
pub const DEPLOYMENT_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// Page size for a list operation whose caller sent no `maxResults`.
///
/// The modelled `MaxPageResults` range is 1..100, so the largest page the model
/// allows is the whole page a caller gets by default.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The number of environment templates one account may hold.
///
/// Proton quotas these resources per account (the modelled
/// `ServiceQuotaExceededException`, HTTP 402, documents itself as "A quota was
/// exceeded. For more information, see Proton Quotas in the Proton User Guide"), so a
/// create past the quota is refused with that error rather than stored. Enforcing it
/// is also what keeps this store from growing without bound in a long-lived emulator:
/// each record is bounded by the modelled `@length` constraints on its members, and
/// the record COUNT is bounded here.
pub const MAX_ENVIRONMENT_TEMPLATES: usize = 100;

/// The number of environments one account may hold. See [`MAX_ENVIRONMENT_TEMPLATES`].
pub const MAX_ENVIRONMENTS: usize = 100;

/// The environment change the clock owes a record, materialised by
/// [`ComAmazonawsProtonAwsproton20200720State::settle_environments`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingEnvironmentChange {
    /// `IN_PROGRESS` becomes `SUCCEEDED`.
    Deploy,
    /// `DELETE_IN_PROGRESS` ends with the record gone.
    Delete,
}

/// A persisted environment template, as `CreateEnvironmentTemplate` stored it.
#[derive(Debug, Clone)]
pub struct EnvironmentTemplateRecord {
    pub name: String,
    pub arn: String,
    pub created_at: SystemTime,
    pub last_modified_at: SystemTime,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub encryption_key: Option<String>,
    pub provisioning: Option<String>,
    pub recommended_version: Option<String>,
    pub tags: Vec<(String, String)>,
}

impl EnvironmentTemplateRecord {
    /// The modelled `EnvironmentTemplate` this record describes. Every member is read
    /// from the record, so a create and a later get answer identically.
    pub fn view(&self) -> EnvironmentTemplate {
        EnvironmentTemplate {
            name: self.name.clone(),
            arn: self.arn.clone(),
            created_at: timestamp(self.created_at),
            last_modified_at: timestamp(self.last_modified_at),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            recommended_version: self.recommended_version.clone(),
            encryption_key: self.encryption_key.clone(),
            provisioning: self.provisioning.clone(),
        }
    }
}

/// A persisted environment, as `CreateEnvironment` stored it.
#[derive(Debug, Clone)]
pub struct EnvironmentRecord {
    pub name: String,
    pub arn: String,
    pub created_at: SystemTime,
    pub last_deployment_attempted_at: SystemTime,
    pub last_deployment_succeeded_at: SystemTime,
    pub template_name: String,
    pub template_major_version: String,
    pub template_minor_version: String,
    pub deployment_status: String,
    pub deployment_status_message: Option<String>,
    pub description: Option<String>,
    pub spec: Option<String>,
    pub provisioning: Option<String>,
    pub proton_service_role_arn: Option<String>,
    pub component_role_arn: Option<String>,
    pub codebuild_role_arn: Option<String>,
    pub tags: Vec<(String, String)>,
    /// The transition the clock owes this environment and the instant it falls due;
    /// `None` once the record has settled.
    pub pending: Option<(PendingEnvironmentChange, SystemTime)>,
}

impl EnvironmentRecord {
    /// The modelled `Environment` this record describes.
    ///
    /// `environmentAccountConnectionId`, `environmentAccountId` and
    /// `provisioningRepository` stay unset because nothing can set them yet: an
    /// environment account connection and a linked repository are created by
    /// `CreateEnvironmentAccountConnection` / `CreateRepository`, which are still
    /// scaffolded, so `CreateEnvironment` refuses a request naming either (the
    /// modelled `ResourceNotFoundException`) rather than inventing one here. The
    /// deployment ids stay unset for the same reason — a `Deployment` resource is
    /// minted by operations this slice does not implement.
    pub fn view(&self) -> Environment {
        Environment {
            name: self.name.clone(),
            description: self.description.clone(),
            created_at: timestamp(self.created_at),
            last_deployment_attempted_at: timestamp(self.last_deployment_attempted_at),
            last_deployment_succeeded_at: timestamp(self.last_deployment_succeeded_at),
            arn: self.arn.clone(),
            template_name: self.template_name.clone(),
            template_major_version: self.template_major_version.clone(),
            template_minor_version: self.template_minor_version.clone(),
            deployment_status: self.deployment_status.clone(),
            deployment_status_message: self.deployment_status_message.clone(),
            proton_service_role_arn: self.proton_service_role_arn.clone(),
            environment_account_connection_id: None,
            environment_account_id: None,
            spec: self.spec.clone(),
            provisioning: self.provisioning.clone(),
            provisioning_repository: None,
            component_role_arn: self.component_role_arn.clone(),
            codebuild_role_arn: self.codebuild_role_arn.clone(),
            last_attempted_deployment_id: None,
            last_succeeded_deployment_id: None,
        }
    }

    /// The modelled `EnvironmentSummary` for this record: the same stored values the
    /// detail view reports, minus the members `EnvironmentSummary` does not model
    /// (`spec`, `provisioningRepository`, `codebuildRoleArn`).
    pub fn summary(&self) -> EnvironmentSummary {
        EnvironmentSummary {
            name: self.name.clone(),
            description: self.description.clone(),
            created_at: timestamp(self.created_at),
            last_deployment_attempted_at: timestamp(self.last_deployment_attempted_at),
            last_deployment_succeeded_at: timestamp(self.last_deployment_succeeded_at),
            arn: self.arn.clone(),
            template_name: self.template_name.clone(),
            template_major_version: self.template_major_version.clone(),
            template_minor_version: self.template_minor_version.clone(),
            deployment_status: self.deployment_status.clone(),
            deployment_status_message: self.deployment_status_message.clone(),
            proton_service_role_arn: self.proton_service_role_arn.clone(),
            environment_account_connection_id: None,
            environment_account_id: None,
            provisioning: self.provisioning.clone(),
            component_role_arn: self.component_role_arn.clone(),
            last_attempted_deployment_id: None,
            last_succeeded_deployment_id: None,
        }
    }
}

/// Per-service state for com-amazonaws-proton-awsproton20200720. A handler reaches it through [`service_state`].
///
/// Both maps are `BTreeMap`s keyed by the resource name: the key is what Get and
/// Delete resolve, and the ordering is what makes a list page — and the token that
/// resumes it — stable across calls.
#[derive(Debug, Default)]
pub struct ComAmazonawsProtonAwsproton20200720State {
    pub environment_templates: BTreeMap<String, EnvironmentTemplateRecord>,
    pub environments: BTreeMap<String, EnvironmentRecord>,
}

impl ComAmazonawsProtonAwsproton20200720State {
    /// Materialise every environment transition the clock has ALREADY made due as of
    /// `now`: an in-flight deployment becomes `SUCCEEDED`, and a deletion that fell
    /// due drops the record.
    ///
    /// Each one was owed at the same instant whether or not anybody called, so a
    /// reporting operation may apply it; nothing here depends on the request.
    pub fn settle_environments(&mut self, now: SystemTime) {
        self.environments.retain(|_, environment| {
            let Some((change, due)) = environment.pending else {
                return true;
            };
            if due > now {
                return true;
            }
            match change {
                PendingEnvironmentChange::Delete => false,
                PendingEnvironmentChange::Deploy => {
                    environment.deployment_status = DEPLOYMENT_SUCCEEDED.to_string();
                    environment.last_deployment_succeeded_at = due;
                    environment.pending = None;
                    true
                }
            }
        });
    }
}

/// A modelled `Timestamp` for a stored instant.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The region this request is served in: the one its SigV4 credential scope names,
/// else [`DEFAULT_REGION`] for a request that carries no scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:proton:<region>:<account>:environment-template/<name>`.
pub fn environment_template_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("environment-template/{name}"),
    )
}

/// `arn:aws:proton:<region>:<account>:environment/<name>`.
pub fn environment_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(SIGNING_NAME, region, &format!("environment/{name}"))
}

/// The opaque `nextToken` that resumes a listing after the item named `after`.
///
/// Hex-encoded, so the token is opaque to the caller and satisfies the modelled
/// `NextToken` pattern `^[A-Za-z0-9+=/]+$` for any resource name.
pub fn encode_page_token(after: &str) -> String {
    hex::encode(after)
}

/// The item name an opaque `nextToken` resumes after, or `None` when the token was
/// not minted by [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsProtonAwsproton20200720State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsProtonAwsproton20200720State>)
}
