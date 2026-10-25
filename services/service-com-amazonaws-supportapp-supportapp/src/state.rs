// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-supportapp-supportapp service.
//!
//! Besides the state struct itself this module holds the few rules the Support App
//! handlers share — the stored-record-to-modelled-shape conversion, the listing
//! pagination rule, the documented `notifyOnCaseSeverity` consistency rule and the
//! wording of the failures several operations can answer with — so the operation files
//! agree on one implementation of each instead of carrying copies that can drift apart.

use std::collections::BTreeMap;

use crate::ops::types::{SlackChannelConfiguration, SlackWorkspaceConfiguration};

/// The most Slack channel configurations this account can hold.
///
/// The service documents the quota on `CreateSlackChannelConfiguration`: "You can add
/// up to 20 Slack channels for your account". It is what bounds the store: a create
/// that would take the account past it is refused with the operation's modelled
/// `ServiceQuotaExceededException`, so no caller can grow this map without end.
pub const MAX_SLACK_CHANNEL_CONFIGURATIONS: usize = 20;

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

/// The longest Slack workspace name this API can report.
///
/// `teamName` is `len 1..256, pattern ^.+$` on both
/// `RegisterSlackWorkspaceForOrganizationResult` and `SlackWorkspaceConfiguration`
/// (`smithy_query(query="shape",shape="teamName")`), so a name this service stores is
/// clipped to it: a longer one could not go on the wire.
pub const MAX_TEAM_NAME_LEN: usize = 256;

/// What a workspace's stored name says it is, after the team id that identifies it.
///
/// A Slack workspace's name lives in Slack, which this emulator cannot ask, so the name
/// it records is built from the one thing the registration carries about the workspace —
/// its team id — plus this. See [`derived_slack_workspace_name`].
const SLACK_WORKSPACE_NAME_SUFFIX: &str = "Slack workspace";

/// The name recorded for the Slack workspace `team_id` identifies.
///
/// A registered workspace HAS a name, which both `RegisterSlackWorkspaceForOrganization`
/// and `ListSlackWorkspaceConfigurations` report in `teamName`. The name is a property of
/// the workspace in Slack, which this emulator cannot query and which no request carries,
/// so it is derived from the workspace's own identifier: one workspace therefore has one
/// name, the same one every time it is reported, and two different workspaces have
/// different names. The result is clipped to [`MAX_TEAM_NAME_LEN`] characters so even the
/// longest legal `teamId` yields a name the model lets this service put on the wire, and
/// it is never empty: `teamId` is itself `len 1..256` and leads the name, and the trim
/// that drops what the clip may leave dangling in the suffix never empties a name it
/// would otherwise be the whole of.
pub fn derived_slack_workspace_name(team_id: &str) -> String {
    let clipped: String = format!("{team_id} {SLACK_WORKSPACE_NAME_SUFFIX}")
        .chars()
        .take(MAX_TEAM_NAME_LEN)
        .collect();
    let trimmed = clipped.trim_end();
    if trimmed.is_empty() {
        clipped
    } else {
        trimmed.to_string()
    }
}

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
    /// The workspace's name, recorded when it was registered (see
    /// [`derived_slack_workspace_name`]). A registered workspace always has one, so this
    /// is not optional even though the modelled member is: every report of this record
    /// carries the same name.
    pub team_name: String,
    pub allow_organization_member_account: Option<bool>,
}

impl SlackWorkspaceConfig {
    /// This record as the modelled `SlackWorkspaceConfiguration`.
    pub fn to_modeled(&self) -> SlackWorkspaceConfiguration {
        SlackWorkspaceConfiguration {
            team_id: self.team_id.clone(),
            team_name: Some(self.team_name.clone()),
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
    /// `RegisterSlackWorkspaceForOrganization`, removed by
    /// `DeleteSlackWorkspaceConfiguration` and bounded by
    /// [`MAX_SLACK_WORKSPACE_CONFIGURATIONS`]. It is the account's workspace registry:
    /// a Slack channel configuration lives INSIDE a registered workspace, so
    /// `CreateSlackChannelConfiguration` refuses a channel whose `teamId` is absent
    /// here (see [`workspace_not_registered_message`]) and deleting a workspace takes
    /// that workspace's channel configurations with it.
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

/// Why the `nextToken` a listing request carried must be refused, or `None` when the
/// request carried none — the pagination rule both listings of this service follow.
///
/// Neither listing splits its answer. Each reports a collection the service's own
/// documented account quota bounds ([`MAX_SLACK_CHANNEL_CONFIGURATIONS`],
/// [`MAX_SLACK_WORKSPACE_CONFIGURATIONS`]) and neither operation takes a page size
/// member, so the whole collection fits the one response and there is never a further
/// page to point at: the output `nextToken` stays absent, exactly as the member
/// documents for the last set of results.
///
/// A listing that mints no token therefore has no position any token could name, so a
/// request that carries one is refused rather than silently read as "from the
/// beginning" — the caller is told the member is wrong instead of being handed a page
/// it did not ask for. Neither paginated operation models a client-input error, so the
/// refusal travels through the scaffold's 400 `ValidationException` channel, worded by
/// [`invalid_pagination_token_message`]; `operation` is the caller's `Operation::NAME`.
pub fn pagination_token_refusal(operation: &str, next_token: Option<&str>) -> Option<String> {
    next_token.map(|_| invalid_pagination_token_message(operation))
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

/// The `ValidationException` message for a `nextToken` no response of this service
/// returned, shared by every paginated operation of this service.
///
/// Stated through [`invalid_request_message`], like every other request-validation
/// refusal this service raises from a handler, so a caller reading only the `message`
/// learns which request was refused and which member of it was wrong. The rule
/// RESTATES what the member documents — "reissue the search request and include the
/// returned token"
/// (`smithy_query(query="doc",shape="ListSlackChannelConfigurationsRequest",member="nextToken")`)
/// — and names the member the way this service's prose names a parameter, in the
/// API's PascalCase spelling. `operation` is the caller's `Operation::NAME`.
pub fn invalid_pagination_token_message(operation: &str) -> String {
    invalid_request_message(
        operation,
        &format!(
            "NextToken must be a pagination token that an earlier {operation} response \
             returned."
        ),
    )
}

/// The `ValidationException` message for a registration past
/// [`MAX_SLACK_WORKSPACE_CONFIGURATIONS`], in the same envelope.
///
/// The rule is the documented quota — "You can add up to 5 Slack workspaces for your
/// account" (`smithy_query(query="doc",shape="CreateSlackChannelConfiguration")`) —
/// restated from the constant that enforces it, so the bound the store applies and the
/// bound the message states cannot drift apart.
pub fn workspace_quota_exceeded_message(operation: &str) -> String {
    invalid_request_message(
        operation,
        &format!(
            "You can add up to {MAX_SLACK_WORKSPACE_CONFIGURATIONS} Slack workspaces for your \
             account."
        ),
    )
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

/// The `ResourceNotFoundException` message for a `teamId` this account holds no Slack
/// workspace configuration for ("The specified resource is missing or doesn't exist,
/// such as an account alias, Slack channel configuration, or Slack workspace
/// configuration" — `ResourceNotFoundException`).
///
/// Worded exactly as [`slack_channel_configuration_not_found`] words a channel miss,
/// naming the resource and the member that identifies it, so the two misses this
/// service can report read the same way and differ only in which resource was looked
/// up.
pub fn slack_workspace_configuration_not_found(team_id: &str) -> String {
    format!("Slack workspace configuration for team {team_id} not found")
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
