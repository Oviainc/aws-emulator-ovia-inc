// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-supportapp-supportapp service.
//!
//! Besides the state struct itself this module holds the few rules the Support App
//! handlers share — the stored-record-to-modelled-shape conversion, the list cursor
//! codec and the documented `notifyOnCaseSeverity` consistency rule — so the
//! operation files agree on one implementation of each instead of carrying copies
//! that can drift apart.

use std::collections::BTreeMap;

use crate::ops::types::SlackChannelConfiguration;

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
/// `ListSlackWorkspaceConfigurations` reports it and
/// `RegisterSlackWorkspaceForOrganization` records it. Those operations are still
/// scaffolded and carry their own gap issues; the field below is where they will
/// read and write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackWorkspaceConfig {
    pub team_id: String,
    pub team_name: Option<String>,
    pub allow_organization_member_account: Option<bool>,
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
    /// Every stored Slack workspace configuration, keyed by `teamId`. Its writers
    /// (`RegisterSlackWorkspaceForOrganization`) are still scaffolded; when they are
    /// implemented they must bound this map the way `CreateSlackChannelConfiguration`
    /// bounds the one above, with the documented workspace quota
    /// (`CreateSlackChannelConfiguration`: "You can add up to 5 Slack workspaces for
    /// your account") and the same modelled `ServiceQuotaExceededException`.
    pub slack_workspace_configurations: BTreeMap<String, SlackWorkspaceConfig>,
}

/// The pagination cursor for a page that starts at `(team_id, channel_id)`.
///
/// Hex of the composite key rather than the key itself, so a caller cannot read
/// structure into the token and has nothing to build one from: a `nextToken` is
/// opaque and only ever travels back unchanged. `teamId` and `channelId` are both
/// `pattern ^\S+$`, so a single space separates them unambiguously.
pub fn encode_cursor(team_id: &str, channel_id: &str) -> String {
    ::hex::encode(format!("{team_id} {channel_id}").as_bytes())
}

/// The inclusive start key a cursor minted by [`encode_cursor`] carries, or `None`
/// when the token was not minted by this service.
pub fn decode_cursor(token: &str) -> Option<(String, String)> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (team_id, channel_id) = decoded.split_once(' ')?;
    if team_id.is_empty() || channel_id.is_empty() {
        return None;
    }
    Some((team_id.to_string(), channel_id.to_string()))
}

/// Why `severity` cannot hold together with the three notification flags, or `None`
/// when it can.
///
/// The rule is documented on `notifyOnCaseSeverity`
/// (`smithy_query(query="doc",shape="UpdateSlackChannelConfigurationRequest",member="notifyOnCaseSeverity")`):
/// with `high` or `all` at least one of the three flags must be `true`, and with
/// `none` every one of them must be `false`. It is evaluated over the values the
/// configuration will HOLD — create applies its documented default of `false` to an
/// absent flag, update keeps the stored value — because that is what the
/// notification setting means.
pub fn notification_rule_violation(
    severity: &str,
    notify_on_create_or_reopen_case: bool,
    notify_on_add_correspondence_to_case: bool,
    notify_on_resolve_case: bool,
) -> Option<String> {
    let any_flag_set = notify_on_create_or_reopen_case
        || notify_on_add_correspondence_to_case
        || notify_on_resolve_case;
    match severity {
        "high" | "all" if !any_flag_set => Some(format!(
            "notifyOnCaseSeverity {severity} requires at least one of \
             notifyOnAddCorrespondenceToCase, notifyOnCreateOrReopenCase or \
             notifyOnResolveCase to be true"
        )),
        "none" if any_flag_set => Some(
            "notifyOnCaseSeverity none requires notifyOnAddCorrespondenceToCase, \
             notifyOnCreateOrReopenCase and notifyOnResolveCase to be false"
                .to_string(),
        ),
        _ => None,
    }
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
