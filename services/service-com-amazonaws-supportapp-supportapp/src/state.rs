// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-supportapp-supportapp service.
//!
//! Besides the state struct itself this module holds the few rules the Support App
//! handlers share — the stored-record-to-modelled-shape conversion, the list cursor
//! codec, the documented `notifyOnCaseSeverity` consistency rule and the wording of
//! the failures several operations can answer with — so the operation files agree on
//! one implementation of each instead of carrying copies that can drift apart.

use std::collections::BTreeMap;

use crate::ops::types::{SlackChannelConfiguration, SlackWorkspaceConfiguration};

/// The most Slack channel configurations this account can hold.
///
/// The service documents the quota on `CreateSlackChannelConfiguration`: "You can add
/// up to 20 Slack channels for your account". It is what bounds the store: a create
/// that would take the account past it is refused with the operation's modelled
/// `ServiceQuotaExceededException`, so no caller can grow this map without end.
pub const MAX_SLACK_CHANNEL_CONFIGURATIONS: usize = 20;

/// How many Slack channel configurations one `ListSlackChannelConfigurations` page
/// reports.
///
/// The service publishes no page size, so this is a CHOSEN value: half the documented
/// account quota above, which keeps the modelled `nextToken` path reachable for an
/// account that is within its quota instead of making it dead code.
pub const CHANNEL_PAGE_SIZE: usize = 10;

/// The most Slack workspaces this account can have registered.
///
/// Documented on `CreateSlackChannelConfiguration`: "You can add up to 5 Slack
/// workspaces for your account". It bounds the workspace registry below, so no caller
/// can grow it without end. `RegisterSlackWorkspaceForOrganization` models no
/// `ServiceQuotaExceededException` (its errors are `AccessDeniedException`,
/// `ConflictException`, `InternalServerException`, `ResourceNotFoundException` and
/// `ValidationException`), so a registration past this quota is refused with that
/// operation's modelled `ValidationException`.
pub const MAX_SLACK_WORKSPACE_CONFIGURATIONS: usize = 5;

/// How many Slack workspace configurations one `ListSlackWorkspaceConfigurations`
/// page reports.
///
/// The service publishes no page size, so this is a CHOSEN value, picked the same way
/// [`CHANNEL_PAGE_SIZE`] is: half the documented quota above, which keeps the modelled
/// `nextToken` path reachable for an account that is within its quota instead of
/// making it dead code.
pub const WORKSPACE_PAGE_SIZE: usize = MAX_SLACK_WORKSPACE_CONFIGURATIONS / 2;

/// Whether the single account this emulator presents is an organization's management
/// or member account, as `RegisterSlackWorkspaceForOrganization` reports it in
/// `accountType`.
///
/// The emulator presents exactly ONE account (`engine::emulator_account_id`), so that
/// account is the management account of its own organization: there is no second
/// account for it to be a member of. One of
/// [`crate::ops::types::ACCOUNT_TYPE_VALUES`].
pub const ACCOUNT_TYPE: &str = "management";

/// One stored Slack channel configuration: everything
/// `CreateSlackChannelConfiguration` was given, after the documented defaults were
/// applied, so `List`/`Update` report the values that were persisted rather than
/// echoing a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackChannelConfig {
    pub team_id: String,
    pub channel_id: String,
    pub channel_name: Option<String>,
    pub notify_on_create_or_reopen_case: Option<bool>,
    pub notify_on_add_correspondence_to_case: Option<bool>,
    pub notify_on_resolve_case: Option<bool>,
    /// One of `NOTIFICATION_SEVERITY_LEVEL_VALUES` (`all` | `high` | `none`); the
    /// member is `@required` on create, so a stored record always carries one.
    pub notify_on_case_severity: String,
    pub channel_role_arn: String,
}

impl SlackChannelConfig {
    /// This record as the modelled `SlackChannelConfiguration`.
    pub fn to_modeled(&self) -> SlackChannelConfiguration {
        SlackChannelConfiguration {
            team_id: self.team_id.clone(),
            channel_id: self.channel_id.clone(),
            channel_name: self.channel_name.clone(),
            notify_on_create_or_reopen_case: self.notify_on_create_or_reopen_case,
            notify_on_add_correspondence_to_case: self.notify_on_add_correspondence_to_case,
            notify_on_resolve_case: self.notify_on_resolve_case,
            notify_on_case_severity: Some(self.notify_on_case_severity.clone()),
            channel_role_arn: Some(self.channel_role_arn.clone()),
        }
    }
}

/// One stored Slack workspace configuration, as
/// `RegisterSlackWorkspaceForOrganization` records it and
/// `ListSlackWorkspaceConfigurations` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackWorkspaceConfig {
    pub team_id: String,
    pub team_name: Option<String>,
    pub allow_organization_member_account: Option<bool>,
}

impl SlackWorkspaceConfig {
    /// This record as the modelled `SlackWorkspaceConfiguration`.
    pub fn to_modeled(&self) -> SlackWorkspaceConfiguration {
        SlackWorkspaceConfiguration {
            team_id: self.team_id.clone(),
            team_name: self.team_name.clone(),
            allow_organization_member_account: self.allow_organization_member_account,
        }
    }
}

/// Per-service state for com-amazonaws-supportapp-supportapp. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSupportappSupportappState {
    /// Every stored Slack channel configuration, keyed by `(teamId, channelId)` —
    /// the pair that identifies a channel configuration in this API. A `BTreeMap`,
    /// so list order, and therefore pagination, is deterministic.
    pub slack_channel_configurations: BTreeMap<(String, String), SlackChannelConfig>,
    /// The account alias `PutAccountAlias` wrote, which `GetAccountAlias` reports
    /// and `DeleteAccountAlias` clears. `None` until one is put: the account has no
    /// alias to report.
    pub account_alias: Option<String>,
    /// Every Slack workspace registered to this account, keyed by `teamId`, written by
    /// `RegisterSlackWorkspaceForOrganization` and bounded by
    /// [`MAX_SLACK_WORKSPACE_CONFIGURATIONS`]. It is the account's workspace registry:
    /// a Slack channel configuration lives INSIDE a registered workspace, so
    /// `CreateSlackChannelConfiguration` refuses a channel whose `teamId` is absent
    /// here (see [`workspace_not_registered_message`]).
    pub slack_workspace_configurations: BTreeMap<String, SlackWorkspaceConfig>,
}

/// The `ConflictException` message for a Slack channel configuration refused because
/// `team_id` is not in this account's workspace registry.
///
/// `ConflictException`'s documentation names the condition — "Add, update, or delete a
/// Slack channel configuration before you add a Slack workspace to your Amazon Web
/// Services account" — and the message is built from the request's `teamId` and the
/// account the emulator presents, so a caller reads the condition it hit.
pub fn workspace_not_registered_message(team_id: &str) -> String {
    format!(
        "Workspace installation not found for account {account} and team {team_id}",
        account = ::engine::emulator_account_id()
    )
}

/// The pagination cursor for a page that resumes after `reported_so_far` records of a
/// listing, shared by every paginated operation of this service.
///
/// Hex of the count rather than the count itself, so a caller cannot read structure
/// into the token and has nothing to build one from: a `nextToken` is opaque and only
/// ever travels back unchanged.
///
/// A COUNT is what the token can carry, rather than the start key itself: every
/// `nextToken` targets `paginationToken`, which is `len 1..256`, while a key-derived
/// token costs two characters per key byte and a `teamId` alone is `len 1..256`
/// (`smithy_query(query="operation",operation="ListSlackWorkspaceConfigurations")`),
/// so a token built from a key leaves the modelled constraint for ids this API
/// accepts. Each listing walks its store in key order (a `BTreeMap`), so the count
/// names the position the next page resumes at.
pub fn encode_cursor(reported_so_far: usize) -> String {
    ::hex::encode(reported_so_far.to_string().as_bytes())
}

/// How many records a cursor minted by [`encode_cursor`] skips, or `None` when the
/// token was not minted by this service.
///
/// Only the canonical spelling this service mints decodes: `parse` alone would also
/// accept a leading `+` or `007`, which no minted token carries.
pub fn decode_cursor(token: &str) -> Option<usize> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let position: usize = decoded.parse().ok()?;
    (position.to_string() == decoded).then_some(position)
}

/// The three notification flags as this service's PROSE names them.
///
/// A Support App error message states a parameter in the API's PascalCase spelling
/// (`NotifyOnCreateOrReopenCase`), not in the lowerCamel spelling the wire member
/// uses, and lists them in this order. The wire members are
/// `notifyOnCreateOrReopenCase`, `notifyOnAddCorrespondenceToCase` and
/// `notifyOnResolveCase` on both
/// `CreateSlackChannelConfigurationRequest` and `UpdateSlackChannelConfigurationRequest`
/// (`smithy_query(query="operation",operation="CreateSlackChannelConfiguration")`).
const NOTIFICATION_FLAG_PARAMETERS: [&str; 3] = [
    "NotifyOnCreateOrReopenCase",
    "NotifyOnAddCorrespondenceToCase",
    "NotifyOnResolveCase",
];

/// A Support App `ValidationException` message: service prose that names the
/// operation whose request was refused, states the rule the request broke, and asks
/// the caller to retry.
///
/// This is the shape every Support App request-validation message has — "Your
/// `<Operation>` request isn't valid. `<rule>` Try your request again." — so the
/// operation's own handler supplies only the clause that states its rule and cannot
/// spell the envelope differently from its neighbour. `rule` is a complete sentence,
/// ending in its own full stop.
pub fn invalid_request_message(operation: &str, rule: &str) -> String {
    format!("Your {operation} request isn't valid. {rule} Try your request again.")
}

/// Why `severity` cannot hold together with the three notification flags, as the
/// message the refusal carries, or `None` when it can.
///
/// The rule is documented on `notifyOnCaseSeverity`
/// (`smithy_query(query="doc",shape="UpdateSlackChannelConfigurationRequest",member="notifyOnCaseSeverity")`):
/// with `high` or `all` at least one of the three flags must be `true`, and with
/// `none` every one of them must be null or `false`. It is evaluated over the values
/// the configuration will HOLD — create applies its documented default of `false` to
/// an absent flag, update keeps the stored value — because that is what the
/// notification setting means.
///
/// The message RESTATES that documented rule in service prose through
/// [`invalid_request_message`], so the operation that was called is named and the
/// parameters are named the way the API names them; `operation` is the caller's
/// `Operation::NAME`.
pub fn notification_rule_violation(
    operation: &str,
    severity: &str,
    notify_on_create_or_reopen_case: bool,
    notify_on_add_correspondence_to_case: bool,
    notify_on_resolve_case: bool,
) -> Option<String> {
    let any_flag_set = notify_on_create_or_reopen_case
        || notify_on_add_correspondence_to_case
        || notify_on_resolve_case;
    let [create_or_reopen, add_correspondence, resolve] = NOTIFICATION_FLAG_PARAMETERS;
    let rule = match severity {
        "high" | "all" if !any_flag_set => format!(
            "If NotifyOnCaseSeverity is all or high, you must enter true for at least \
             one of the following parameters: {create_or_reopen}, {add_correspondence} \
             or {resolve}."
        ),
        "none" if any_flag_set => format!(
            "If NotifyOnCaseSeverity is none, the following parameters must be null \
             or false: {create_or_reopen}, {add_correspondence} or {resolve}."
        ),
        _ => return None,
    };
    Some(invalid_request_message(operation, &rule))
}

/// How this service's prose names the Slack channel configuration a request
/// identified: by the two members that identify it, `teamId` then `channelId`, in
/// the order the request carries them.
///
/// Both members are a Slack id of its own kind — a team id is not a workspace
/// configuration — so each is named for the member it came from. Every message about
/// one configuration is built from here, so the not-found and the already-exists
/// refusals cannot name the same resource two different ways.
pub fn slack_channel_configuration_subject(team_id: &str, channel_id: &str) -> String {
    format!("Slack channel configuration for team {team_id} and channel {channel_id}")
}

/// The `ResourceNotFoundException` message for a `(teamId, channelId)` pair this
/// account holds no configuration for ("The specified resource is missing or doesn't
/// exist, such as an account alias, Slack channel configuration, or Slack workspace
/// configuration" — `ResourceNotFoundException`). Shared by every operation that
/// looks one up, so they all state a miss the same way.
pub fn slack_channel_configuration_not_found(team_id: &str, channel_id: &str) -> String {
    format!(
        "{} not found",
        slack_channel_configuration_subject(team_id, channel_id)
    )
}

/// The `ConflictException` message for a `(teamId, channelId)` pair that is already
/// configured ("Add a Slack channel configuration that already exists in your Amazon
/// Web Services account" — `ConflictException`), naming the resource exactly as
/// [`slack_channel_configuration_not_found`] does.
pub fn slack_channel_configuration_exists(team_id: &str, channel_id: &str) -> String {
    format!(
        "{} already exists",
        slack_channel_configuration_subject(team_id, channel_id)
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSupportappSupportappState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSupportappSupportappState>)
}
