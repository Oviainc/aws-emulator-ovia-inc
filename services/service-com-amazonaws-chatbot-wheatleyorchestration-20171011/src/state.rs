// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-chatbot-wheatleyorchestration-20171011 service.
//!
//! This module owns the persisted shape of every chat configuration and every custom
//! action the service stores, plus the few helpers their handlers share (the ARN forms,
//! which chatbot resource an ARN names, the request's region, and the record ->
//! modelled-shape projections). Handlers reach it through [`service_state`]; nothing here
//! is per-request.
//!
//! It also owns the registries of chat workspaces AUTHORIZED with the account — Slack
//! workspaces, which a channel configuration has to name one of, and Microsoft Teams
//! teams, which `ListMicrosoftTeamsConfiguredTeams` reports. An authorization gets into
//! either one only out of band ([`seed_slack_workspaces`],
//! [`authorize_microsoft_teams_team`]), because no operation in this model creates one.
//!
//! It owns the calling account's AWS Chatbot preferences too
//! ([`AccountPreferencesRecord`]), which are account-scoped rather than per-resource, and
//! the resources LINKED to each channel configuration
//! (`ComAmazonawsChatbotWheatleyorchestration20171011State::associate_resource`), which
//! are what `ListAssociations` reports.

use std::collections::{BTreeMap, BTreeSet};

use crate::ops::types::{
    AccountPreferences, ChimeWebhookConfiguration, ConfiguredTeam, CustomAction,
    CustomActionAttachment, CustomActionDefinition, SlackChannelConfiguration, SlackWorkspace, Tag,
};

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. `us-east-1` is the emulator-wide default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `chatbot` ARN service namespace (`OPERATIONS.md`: "ARN namespace: chatbot").
///
/// It is also this service's IAM action prefix — an action is `chatbot:<OperationName>`
/// ("Actions defined by AWS Chatbot", IAM Service Authorization Reference) — so the
/// authorization gate ([`crate::service::authorization`]) names actions through this one
/// constant rather than spelling the namespace a second time.
pub const ARN_SERVICE: &str = "chatbot";

/// The ARN resource-path segment naming the chat client a configuration configures: a
/// Slack channel, or an Amazon Chime webhook. A chat configuration ARN is
/// `chat-configuration/{kind}/{name}`, so the kind is what keeps one client's
/// configuration name from colliding with another's.
const CHAT_CONFIGURATION_KIND_SLACK: &str = "slack-channel";
const CHAT_CONFIGURATION_KIND_CHIME: &str = "chime-webhook";

/// `State` of a chat configuration AWS Chatbot reports as usable. A chat
/// configuration is usable the moment it is created — AWS returns no transient
/// status for one — so this is the only state a created record is ever in.
pub const STATE_ENABLED: &str = "ENABLED";

/// The most tags a chat configuration can carry: the model's `TagList` is
/// `length 0..200`, so a merge that would exceed it is refused.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The most resources ONE channel configuration can hold associations to.
///
/// The model carries no number for it, so this bound is the emulator's own: it is what
/// keeps repeated `AssociateToConfiguration` calls from growing the emulator's memory
/// without limit. Past it an associate is refused the way AWS Chatbot refuses a stored
/// total that has reached its cap — the modelled client-input error class, wording
/// nothing (see `crate::ops::tag_resource`).
pub const MAX_ASSOCIATIONS_PER_CONFIGURATION: usize = MAX_PAGE_SIZE;

/// The largest page a listing serves, and the page it serves when the caller names no
/// `MaxResults`: the model's own `MaxResults` maximum
/// (`DescribeSlackChannelConfigurations`, range 1..100). It bounds how much one
/// response can carry, whatever the store holds.
pub const MAX_PAGE_SIZE: usize = 100;

/// The most Slack channel configurations this store holds AT ALL — across every
/// region, so no request can carve out a fresh budget by signing for another one.
///
/// AWS enforces a service quota on chat configurations per account; the model does not
/// carry the number, so this bound is the emulator's own: it is what keeps repeated
/// creates from growing the emulator's memory without limit, and past it a create is
/// refused with the modelled `LimitExceededException`.
pub const MAX_SLACK_CHANNEL_CONFIGURATIONS: usize = MAX_PAGE_SIZE;

/// The most custom actions this store holds AT ALL, for the same reason as
/// [`MAX_SLACK_CHANNEL_CONFIGURATIONS`]: AWS enforces a service quota on custom actions
/// per account, the model does not carry the number, and this bound is what keeps
/// repeated creates from growing the emulator's memory without limit. Past it a create is
/// refused with the modelled `LimitExceededException`.
pub const MAX_CUSTOM_ACTIONS: usize = MAX_PAGE_SIZE;

/// The most Amazon Chime webhook configurations this store holds AT ALL, for the same
/// reason — and on the same (emulator-owned) grounds — as
/// [`MAX_SLACK_CHANNEL_CONFIGURATIONS`]: AWS enforces a service quota on chat
/// configurations per account, the model does not carry the number, and past this bound
/// a create is refused with the modelled `LimitExceededException` rather than growing
/// the emulator's memory without limit.
pub const MAX_CHIME_WEBHOOK_CONFIGURATIONS: usize = MAX_PAGE_SIZE;

// A `Describe` that names no `MaxResults` answers a region's configurations in ONE page
// with no `NextToken`. That holds only while the store cannot hold more than one page,
// so the two bounds are tied here rather than left to drift apart.
const _: () = assert!(MAX_SLACK_CHANNEL_CONFIGURATIONS <= MAX_PAGE_SIZE);
const _: () = assert!(MAX_CHIME_WEBHOOK_CONFIGURATIONS <= MAX_PAGE_SIZE);
const _: () = assert!(MAX_CUSTOM_ACTIONS <= MAX_PAGE_SIZE);

/// The `custom-action` resource-type segment of a chatbot ARN
/// (`CustomActionArn`'s modelled pattern: `…:custom-action/[a-zA-Z0-9_-]{1,64}`).
const CUSTOM_ACTION_RESOURCE_TYPE: &str = "custom-action";

/// The `chat-configuration` resource-type segment of a chatbot ARN (the ARN
/// [`slack_channel_configuration_arn`] mints).
const CHAT_CONFIGURATION_RESOURCE_TYPE: &str = "chat-configuration";

/// Which chatbot resource an `AmazonResourceName` names, read from the resource-type
/// segment of the ARN itself.
///
/// The tagging operations take any chatbot ARN (`AmazonResourceName`), so they have to tell
/// the kinds apart before they can look anything up: each kind of resource lives in its own
/// map, and a custom-action ARN is additionally checked against the form a custom action of
/// this account is minted under ([`is_mintable_custom_action_arn`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatbotResourceKind {
    /// A chat configuration (`chat-configuration/…`), of either chat client.
    ChatConfiguration,
    /// A custom action (`custom-action/…`).
    CustomAction,
    /// Any other chatbot resource. Nothing in this store holds one, so it is treated as the
    /// resource it is: not found.
    Other,
}

/// The [`ChatbotResourceKind`] `arn` names.
///
/// An ARN is `arn:partition:service:region:account:resource`, and the resource segment
/// starts with the resource type; the model's `AmazonResourceName` pattern already
/// guarantees the five leading segments, so an ARN that reaches a handler has one.
pub fn chatbot_resource_kind(arn: &str) -> ChatbotResourceKind {
    let resource = arn.splitn(6, ':').nth(5).unwrap_or_default();
    match resource.split('/').next().unwrap_or_default() {
        CUSTOM_ACTION_RESOURCE_TYPE => ChatbotResourceKind::CustomAction,
        CHAT_CONFIGURATION_RESOURCE_TYPE => ChatbotResourceKind::ChatConfiguration,
        _ => ChatbotResourceKind::Other,
    }
}

/// The action name a `CustomActionArn` carries: the segment after the resource type
/// (`…:custom-action/{ActionName}`, from `CustomActionArn`'s modelled pattern).
///
/// The name is what a custom-action operation reports back to a caller that addressed an
/// action this account does not hold, so it is read off the ARN the request carried and
/// never off the store.
pub fn custom_action_name_in_arn(custom_action_arn: &str) -> &str {
    custom_action_arn.rsplit('/').next().unwrap_or_default()
}

/// Whether `arn` is an ARN a custom action of THIS account can be minted under.
///
/// A custom action is account-scoped, so [`custom_action_arn`] mints
/// `arn:aws:chatbot::{account}:custom-action/{ActionName}` — an EMPTY region segment and this
/// account's id. An ARN of any other shape (a region segment filled in, another account's
/// id) therefore names no custom action this account could ever hold, however well it
/// satisfies `CustomActionArn`'s modelled pattern, and AWS Chatbot answers it as an
/// authorization failure (403 `UnauthorizedException`, [`UNAUTHORIZED_MESSAGE`]) rather than
/// a not-found: the request's credentials are not valid for a target the account's namespace
/// does not contain, and the decision is made before any existence lookup. An ARN of the
/// minted form that simply holds no action is the not-found case instead.
///
/// The canonical ARN is rebuilt here from the name the ARN carries, so minting and
/// recognising an ARN cannot disagree about its form.
pub fn is_mintable_custom_action_arn(arn: &str) -> bool {
    arn == custom_action_arn(custom_action_name_in_arn(arn))
}

/// The message an authorization refusal carries: AWS Chatbot's recorded text for this error
/// class, which is fixed — it names neither the request's ARN nor the account, because the
/// decision is made against the ARN before anything is looked up.
pub const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The message a SERVER-side refusal carries: AWS Chatbot's recorded text for this error
/// class, which is entirely fixed — the service states the failure and names nothing of the
/// request in it.
pub const INTERNAL_SERVER_ERROR_MESSAGE: &str = "Internal Server Error";

/// The message a custom-action operation (`GetCustomAction`) refuses an ARN this account
/// holds no action under with.
///
/// AWS Chatbot words this error class around the action, not the ARN: the action name comes
/// from the requested `CustomActionArn` and the account from the account serving the
/// request, so nothing here is carried over from any one response.
///
/// `UpdateCustomAction` words the same miss differently
/// ([`custom_action_does_not_exist_message`]): one sentence per error class, not one for the
/// resource.
pub fn custom_action_not_found_message(custom_action_arn: &str) -> String {
    format!(
        "sdk action [{}] not found for account [{}]",
        custom_action_name_in_arn(custom_action_arn),
        ::emulator_server::emulator_account_id()
    )
}

/// The message `UpdateCustomAction` refuses an ARN this account holds no action under with.
///
/// It names the same two values as the read's message
/// ([`custom_action_not_found_message`]) — the action off the requested ARN and the account
/// serving the request — in the words AWS Chatbot gives the write's own error class.
pub fn custom_action_does_not_exist_message(custom_action_arn: &str) -> String {
    format!(
        "Sdk action with id [{}] does not exist in account [{}]",
        custom_action_name_in_arn(custom_action_arn),
        ::emulator_server::emulator_account_id()
    )
}

/// The message an ARN-addressed TAGGING operation (`TagResource`, `UntagResource`,
/// `ListTagsForResource`) refuses an `AmazonResourceName` the store holds nothing under
/// with.
///
/// These operations take any chatbot ARN, and AWS Chatbot words their not-found error
/// around that ARN alone — the same sentence whichever kind of resource the ARN names — so
/// the only variable part is the `ResourceARN` the request carried.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource {resource_arn} does not exist!")
}

/// The message a create refuses with when this account already holds the resource the
/// request would mint.
///
/// AWS Chatbot identifies the colliding resource by its ARN rather than by the name the
/// request carried, so the caller is handed the ARN it can address the existing resource
/// through; the ARN is the stored resource's own.
pub fn resource_already_exists_message(arn: &str) -> String {
    format!("Resource with arn {arn} already exists.")
}

/// The message `DeleteSlackChannelConfiguration` refuses a `ChatConfigurationArn` no Slack
/// channel configuration is stored under with.
///
/// AWS Chatbot words each chat-configuration error class on its own — the delete speaks of
/// the Slack destination behind the channel — so this sentence is the delete's and not
/// shared with the update's; the ARN comes from the request.
pub fn slack_destination_not_found_message(chat_configuration_arn: &str) -> String {
    format!("No Slack Destination exists for Wheatley Channel: {chat_configuration_arn}.")
}

/// The message `UpdateSlackChannelConfiguration` refuses a `ChatConfigurationArn` no Slack
/// channel configuration is stored under with.
///
/// The update speaks of the chat configuration itself, which is why it reads differently
/// from the delete's ([`slack_destination_not_found_message`]); the ARN comes from the
/// request.
pub fn chat_configuration_not_found_message(chat_configuration_arn: &str) -> String {
    format!("Chat Configuration with ARN {chat_configuration_arn} not found.")
}

/// Per-service state for com-amazonaws-chatbot-wheatleyorchestration-20171011. A handler reaches it through [`service_state`].
///
/// `BTreeMap` keyed by `ChatConfigurationArn`, so `DescribeSlackChannelConfigurations`
/// walks the configurations in one stable total order and can resume that walk from
/// an opaque token.
#[derive(Debug)]
pub struct ComAmazonawsChatbotWheatleyorchestration20171011State {
    /// Every Slack channel configuration, keyed by its `ChatConfigurationArn`.
    pub slack_channel_configurations: BTreeMap<String, SlackChannelConfigRecord>,
    /// Every custom action, keyed by its `CustomActionArn`.
    ///
    /// `BTreeMap` for the same reason as the configurations: `ListCustomActions`
    /// paginates over it in one stable total order.
    pub custom_actions: BTreeMap<String, CustomActionRecord>,
    /// Every Amazon Chime webhook configuration, keyed by its `ChatConfigurationArn`,
    /// for the same reason as the Slack ones: `DescribeChimeWebhookConfigurations`
    /// walks them in one stable total order and resumes that walk from an opaque token.
    pub chime_webhook_configurations: BTreeMap<String, ChimeWebhookConfigRecord>,
    /// The Slack workspaces AUTHORIZED with this account, keyed by `SlackTeamId`.
    ///
    /// A Slack channel configuration can only be created for a workspace this map
    /// holds, so this registry — and nothing about the spelling, length or shape of a
    /// `SlackTeamId` — is what decides whether a workspace is authorized.
    ///
    /// `BTreeMap` for the same reason as the configurations: `DescribeSlackWorkspaces`
    /// paginates over it in one stable total order.
    ///
    /// Populated out of band; see [`seed_slack_workspaces`].
    slack_workspace_authorizations: BTreeMap<String, SlackWorkspaceRecord>,
    /// The AWS Chatbot preferences of the calling account — ONE record, not a map:
    /// see [`AccountPreferencesRecord`] for why the account, and not a region or any
    /// resource, is what scopes them.
    account_preferences: AccountPreferencesRecord,
    /// The Microsoft Teams teams AUTHORIZED with this account, keyed by `TeamId`.
    ///
    /// The Microsoft Teams counterpart of `slack_workspace_authorizations`: a team is
    /// authorized by the one-time Microsoft Teams client handshake the AWS Chatbot
    /// console drives, which no operation in this model performs, so membership of this
    /// map — and nothing about the spelling of a `TeamId` — is what decides whether a
    /// team is configured with this account.
    ///
    /// `BTreeMap` so `ListMicrosoftTeamsConfiguredTeams` walks the teams in one stable
    /// total order and can resume that walk from an opaque token.
    ///
    /// EMPTY until something authorizes a team ([`authorize_microsoft_teams_team`]):
    /// an account that has authorized none has none to report, so nothing is seeded
    /// here.
    microsoft_teams_authorizations: BTreeMap<String, MicrosoftTeamsConfiguredTeamRecord>,
    /// The resources LINKED to each channel configuration, keyed by the configuration's
    /// `ChatConfigurationArn` and holding the `ResourceIdentifier` of every resource
    /// associated with it.
    ///
    /// A `BTreeSet` because an association is a LINK and not a count: associating the same
    /// resource twice leaves the configuration linked to it exactly once, and the set's
    /// order is one stable total order `ListAssociations` can paginate over and resume from
    /// an opaque token.
    ///
    /// A configuration with no associations holds no entry here rather than an empty set,
    /// so disassociating the last resource leaves the map as a configuration that was never
    /// associated with anything.
    resource_associations: BTreeMap<String, BTreeSet<String>>,
}

impl Default for ComAmazonawsChatbotWheatleyorchestration20171011State {
    fn default() -> Self {
        Self {
            slack_channel_configurations: BTreeMap::new(),
            custom_actions: BTreeMap::new(),
            chime_webhook_configurations: BTreeMap::new(),
            slack_workspace_authorizations: seed_slack_workspaces()
                .into_iter()
                .map(|workspace| (workspace.slack_team_id.clone(), workspace))
                .collect(),
            account_preferences: AccountPreferencesRecord::default(),
            microsoft_teams_authorizations: BTreeMap::new(),
            resource_associations: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsChatbotWheatleyorchestration20171011State {
    /// The tags of the chat configuration `chat_configuration_arn` names, whichever chat
    /// client it configures, or `None` when no configuration is stored under that ARN.
    ///
    /// A chat configuration ARN carries the client kind (`slack-channel`,
    /// `chime-webhook`) and each kind keeps its own store, so the ARN-addressed tagging
    /// operations resolve one through here rather than knowing the kinds themselves.
    pub fn chat_configuration_tags(
        &self,
        chat_configuration_arn: &str,
    ) -> Option<&BTreeMap<String, String>> {
        if let Some(record) = self.slack_channel_configurations.get(chat_configuration_arn) {
            return Some(&record.tags);
        }
        self.chime_webhook_configurations
            .get(chat_configuration_arn)
            .map(|record| &record.tags)
    }

    /// [`Self::chat_configuration_tags`], for a caller that merges onto them.
    pub fn chat_configuration_tags_mut(
        &mut self,
        chat_configuration_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        if let Some(record) = self
            .slack_channel_configurations
            .get_mut(chat_configuration_arn)
        {
            return Some(&mut record.tags);
        }
        self.chime_webhook_configurations
            .get_mut(chat_configuration_arn)
            .map(|record| &mut record.tags)
    }

    /// The authorized workspace `slack_team_id` names, or `None` when no workspace with
    /// that id is authorized with this account.
    ///
    /// `CreateSlackChannelConfiguration` creates a configuration only for a workspace
    /// this answers `Some` for; for anything else it raises the modelled 400
    /// `InvalidRequestException` carrying
    /// [`unauthorized_slack_workspace_message`] rather than binding a configuration to
    /// a workspace the account never authorized.
    pub fn slack_workspace(&self, slack_team_id: &str) -> Option<&SlackWorkspaceRecord> {
        self.slack_workspace_authorizations.get(slack_team_id)
    }

    /// Every authorized workspace, in `SlackTeamId` order.
    pub fn slack_workspaces(&self) -> impl Iterator<Item = &SlackWorkspaceRecord> {
        self.slack_workspace_authorizations.values()
    }

    /// Record `slack_team_id` as a workspace authorized with this account under the
    /// display name `slack_team_name`, and hand back the record now stored for it.
    ///
    /// This is the out-of-band authorization channel described on
    /// `slack_workspace_authorizations`: on AWS a workspace is authorized by the
    /// one-time Slack client (OAuth) handshake the AWS Chatbot console drives, which no
    /// operation in this model performs, so nothing a client sends over the wire
    /// reaches here. Authorizing a workspace that is already authorized REPLACES its
    /// display name: one workspace id, one authorization.
    pub fn authorize_slack_workspace(
        &mut self,
        slack_team_id: &str,
        slack_team_name: &str,
    ) -> SlackWorkspaceRecord {
        let record = SlackWorkspaceRecord {
            slack_team_id: slack_team_id.to_string(),
            slack_team_name: slack_team_name.to_string(),
            state: STATE_ENABLED.to_string(),
            state_reason: None,
        };
        self.slack_workspace_authorizations
            .insert(slack_team_id.to_string(), record.clone());
        record
    }

    /// The account's current AWS Chatbot preferences.
    pub fn account_preferences(&self) -> AccountPreferencesRecord {
        self.account_preferences
    }

    /// Turn the named preferences on or off and hand back what the account now holds.
    ///
    /// A PARTIAL write: every member of `UpdateAccountPreferences` is optional
    /// (`smithy_query` operation=UpdateAccountPreferences), so a member the request
    /// omits names no new value for that preference and leaves the stored one exactly
    /// as it was — only a member the request carries moves. A preference nothing has
    /// ever written stays UNSET, and a read reports it as absent rather than as off.
    pub fn update_account_preferences(
        &mut self,
        user_authorization_required: Option<bool>,
        training_data_collection_enabled: Option<bool>,
    ) -> AccountPreferencesRecord {
        if let Some(required) = user_authorization_required {
            self.account_preferences.user_authorization_required = Some(required);
        }
        if let Some(enabled) = training_data_collection_enabled {
            self.account_preferences.training_data_collection_enabled = Some(enabled);
        }
        self.account_preferences
    }

    /// The authorized Microsoft Teams team `team_id` names, or `None` when no team with
    /// that id is configured with this account.
    ///
    /// The lookup every Microsoft Teams operation addressed by `TeamId` resolves
    /// through, so one registry decides what is configured.
    pub fn microsoft_teams_configured_team(
        &self,
        team_id: &str,
    ) -> Option<&MicrosoftTeamsConfiguredTeamRecord> {
        self.microsoft_teams_authorizations.get(team_id)
    }

    /// Every configured Microsoft Teams team, in `TeamId` order.
    pub fn microsoft_teams_configured_teams(
        &self,
    ) -> impl Iterator<Item = &MicrosoftTeamsConfiguredTeamRecord> {
        self.microsoft_teams_authorizations.values()
    }

    /// Record `team_id`, in the Microsoft Teams tenant `tenant_id`, as a team authorized
    /// with this account under the display name `team_name`, and hand back the record now
    /// stored for it.
    ///
    /// The out-of-band authorization channel described on
    /// `microsoft_teams_authorizations`: on AWS a team is authorized by the one-time
    /// Microsoft Teams client handshake the AWS Chatbot console drives, which no
    /// operation in this model performs, so nothing a client sends over the wire reaches
    /// here. Authorizing a team that is already authorized REPLACES its tenant and
    /// display name: one team id, one authorization.
    pub fn authorize_microsoft_teams_team(
        &mut self,
        tenant_id: &str,
        team_id: &str,
        team_name: Option<&str>,
    ) -> MicrosoftTeamsConfiguredTeamRecord {
        let record = MicrosoftTeamsConfiguredTeamRecord {
            tenant_id: tenant_id.to_string(),
            team_id: team_id.to_string(),
            team_name: team_name.map(str::to_string),
            state: STATE_ENABLED.to_string(),
            state_reason: None,
        };
        self.microsoft_teams_authorizations
            .insert(team_id.to_string(), record.clone());
        record
    }

    /// Whether the chat configuration `chat_configuration_arn` names is one this account
    /// holds that can HOST an association to a resource.
    ///
    /// A custom action is surfaced to a user as an alias they type, or a button they press,
    /// in a chat CHANNEL — so a channel configuration is what hosts the link. An Amazon
    /// Chime webhook configuration is a webhook DESTINATION with no channel to surface
    /// either in, and AWS Chatbot answers an association against one with its modelled
    /// server error rather than linking anything; an ARN this account holds no
    /// configuration under cannot host the link either.
    ///
    /// A Microsoft Teams channel configuration hosts one exactly as a Slack channel
    /// configuration does, but no operation of this model creates one yet (the create is
    /// still a scaffold), so the store holds none to answer for; the day it does, it joins
    /// the lookup here.
    pub fn hosts_resource_associations(&self, chat_configuration_arn: &str) -> bool {
        self.slack_channel_configurations
            .contains_key(chat_configuration_arn)
    }

    /// Link `resource` to the channel configuration `chat_configuration_arn`, and answer
    /// whether the configuration is now linked to it.
    ///
    /// IDEMPOTENT: linking a resource the configuration already holds changes nothing and
    /// still answers `true`. `false` means the link was NOT made because the configuration
    /// already holds [`MAX_ASSOCIATIONS_PER_CONFIGURATION`] resources — nothing is stored
    /// in that case, so a refused associate leaves no partial state behind.
    pub fn associate_resource(&mut self, chat_configuration_arn: &str, resource: &str) -> bool {
        let linked = self
            .resource_associations
            .entry(chat_configuration_arn.to_string())
            .or_default();
        if linked.contains(resource) {
            return true;
        }
        if linked.len() >= MAX_ASSOCIATIONS_PER_CONFIGURATION {
            // An entry this call created and did not fill would read as a configuration
            // with an empty association set; drop it so the store is exactly as it was.
            if linked.is_empty() {
                self.resource_associations.remove(chat_configuration_arn);
            }
            return false;
        }
        linked.insert(resource.to_string());
        true
    }

    /// Unlink `resource` from the channel configuration `chat_configuration_arn`, and
    /// answer whether a link was there to remove.
    ///
    /// IDEMPOTENT: a resource the configuration never held is simply not linked
    /// afterwards, which is what the caller asked for, so this answers `false` rather than
    /// failing.
    pub fn disassociate_resource(&mut self, chat_configuration_arn: &str, resource: &str) -> bool {
        let Some(linked) = self.resource_associations.get_mut(chat_configuration_arn) else {
            return false;
        };
        let removed = linked.remove(resource);
        if linked.is_empty() {
            self.resource_associations.remove(chat_configuration_arn);
        }
        removed
    }

    /// The resources linked to the channel configuration `chat_configuration_arn`, in one
    /// stable total order; empty for a configuration holding no association.
    pub fn associated_resources(
        &self,
        chat_configuration_arn: &str,
    ) -> impl Iterator<Item = &str> + '_ {
        self.resource_associations
            .get(chat_configuration_arn)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }
}

/// The AWS Chatbot preferences of the calling AWS account: what
/// `GetAccountPreferences` reports and what `UpdateAccountPreferences` writes.
///
/// ACCOUNT-scoped, so the store holds exactly one of these and no region or resource
/// narrows it: the model gives `GetAccountPreferences` an empty input (`smithy_query`
/// operation=GetAccountPreferences: `input {}`) and documents the modelled
/// `AccountPreferences` as "Preferences related to AWS Chatbot usage in the calling AWS
/// account", so there is nothing else a read could key on.
///
/// Each preference is an `Option`, never a plain `bool`: the modelled
/// `AccountPreferences` members are optional booleans with no default (`smithy_query`
/// shape=AccountPreferences), so a preference nothing has written yet is UNSET, and only
/// a write names a value. The two operations project that record differently, as AWS
/// Chatbot answers them: a WRITE reports the preferences the request named
/// ([`Self::view_of_written`]), while a READ reports every preference, an opt-in nothing
/// has turned on reading as off ([`Self::view`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccountPreferencesRecord {
    pub user_authorization_required: Option<bool>,
    pub training_data_collection_enabled: Option<bool>,
}

impl AccountPreferencesRecord {
    /// Project the stored preferences onto the modelled `AccountPreferences` a READ
    /// answers: every preference the model carries, resolved.
    ///
    /// Both are OPT-INS — the model documents `TrainingDataCollectionEnabled` as "Turns
    /// on training data collection" and `UserAuthorizationRequired` as "Enables use of a
    /// user role requirement in your chat configuration" — so one nothing has turned on
    /// is reported off rather than left out: a read answers the account's whole
    /// preference set.
    pub fn view(&self) -> AccountPreferences {
        AccountPreferences {
            user_authorization_required: Some(self.user_authorization_required.unwrap_or(false)),
            training_data_collection_enabled: Some(
                self.training_data_collection_enabled.unwrap_or(false),
            ),
        }
    }

    /// Project the stored preferences onto the modelled `AccountPreferences` a WRITE
    /// answers: the STORED value of each preference the request named, and nothing at all
    /// for one it did not name.
    ///
    /// A write reports what it set, so a preference the request left out is left out of
    /// the answer — which is what keeps a client from reading "not configured" as
    /// "configured off" and pushing an explicit value back. The arguments are the
    /// request's own members, so `Some` means "the request named this preference"; the
    /// value reported for it is the one now in the store, never the one off the request.
    pub fn view_of_written(
        &self,
        user_authorization_required: Option<bool>,
        training_data_collection_enabled: Option<bool>,
    ) -> AccountPreferences {
        AccountPreferences {
            // `Option::and` keeps the stored value where the request named the
            // preference, and `None` where it did not.
            user_authorization_required: user_authorization_required
                .and(self.user_authorization_required),
            training_data_collection_enabled: training_data_collection_enabled
                .and(self.training_data_collection_enabled),
        }
    }
}

/// One authorized Slack workspace: what `DescribeSlackWorkspaces` reports and what a
/// Slack channel configuration takes its `SlackTeamName` from.
#[derive(Debug, Clone, Default)]
pub struct SlackWorkspaceRecord {
    pub slack_team_id: String,
    pub slack_team_name: String,
    /// `ENABLED` or `DISABLED`; see [`STATE_ENABLED`].
    pub state: String,
    pub state_reason: Option<String>,
}

impl SlackWorkspaceRecord {
    /// Project the stored authorization onto the modelled `SlackWorkspace`.
    pub fn view(&self) -> SlackWorkspace {
        SlackWorkspace {
            slack_team_id: self.slack_team_id.clone(),
            slack_team_name: self.slack_team_name.clone(),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
        }
    }
}

/// The Slack workspaces this store starts out holding an authorization for.
///
/// A workspace becomes authorized with an AWS account through the one-time Slack client
/// (OAuth) handshake in the AWS Chatbot console. NO operation in this model performs
/// it: `DescribeSlackWorkspaces` lists the authorizations and
/// `DeleteSlackWorkspaceAuthorization` removes one, and neither creates one. Without a
/// seed the registry would therefore be empty for good and every
/// `CreateSlackChannelConfiguration` would be refused, so the workspaces below are a
/// fixture standing in for that handshake: a client can discover them with
/// `DescribeSlackWorkspaces` and configure a channel in either. A test (or an admin
/// path) registers its own with
/// [`ComAmazonawsChatbotWheatleyorchestration20171011State::authorize_slack_workspace`].
///
/// The ids satisfy the model's `SlackTeamId` pattern (`^[0-9A-Z]{1,255}$`) and the names
/// its `SlackTeamName` length (1..255).
pub fn seed_slack_workspaces() -> Vec<SlackWorkspaceRecord> {
    [
        ("TEMULATOR001", "Emulator Workspace"),
        ("TEMULATOR002", "Emulator Secondary Workspace"),
    ]
    .into_iter()
    .map(|(slack_team_id, slack_team_name)| SlackWorkspaceRecord {
        slack_team_id: slack_team_id.to_string(),
        slack_team_name: slack_team_name.to_string(),
        state: STATE_ENABLED.to_string(),
        state_reason: None,
    })
    .collect()
}

/// The message a create refused for an unauthorized Slack workspace carries: it names
/// the workspace the request asked for and the account it is not authorized with, so a
/// caller can tell WHICH workspace needs the one-time Slack client setup, and points at
/// the guide that describes that setup.
///
/// The sentence around those two values is AWS Chatbot's own, as the AWS Chatbot
/// administrator guide's Slack client setup section is the page it points a caller at;
/// the workspace id comes from the request and the account id from the emulator's
/// account, so nothing here is copied from any one recorded response.
pub fn unauthorized_slack_workspace_message(slack_team_id: &str) -> String {
    format!(
        "Unable to create the configuration because Slack workspace {slack_team_id} is not \
         authorized with AWS account {}. See  \
         https://docs.aws.amazon.com/chatbot/latest/adminguide/slack-setup.html#slack-client-setup",
        ::emulator_server::emulator_account_id()
    )
}

/// One authorized Microsoft Teams team: what `ListMicrosoftTeamsConfiguredTeams`
/// reports and what a Microsoft Teams channel configuration is bound to.
///
/// A team becomes authorized with an AWS account through the one-time Microsoft Teams
/// client handshake in the AWS Chatbot console. NO operation in this model performs it:
/// `ListMicrosoftTeamsConfiguredTeams` lists the authorizations and
/// `DeleteMicrosoftTeamsConfiguredTeam` removes one, and neither creates one — so the
/// registry starts EMPTY and only [`authorize_microsoft_teams_team`] (the out-of-band
/// seam a fixture or an admin path reaches) puts a team in it. An account that has
/// authorized no team therefore lists none.
#[derive(Debug, Clone, Default)]
pub struct MicrosoftTeamsConfiguredTeamRecord {
    /// The Microsoft Teams tenant the team belongs to.
    pub tenant_id: String,
    pub team_id: String,
    /// The team's display name. Optional on the modelled `ConfiguredTeam`, so a stored
    /// authorization that carries none reports none rather than an invented name.
    pub team_name: Option<String>,
    /// `ENABLED` or `DISABLED`; see [`STATE_ENABLED`].
    pub state: String,
    pub state_reason: Option<String>,
}

impl MicrosoftTeamsConfiguredTeamRecord {
    /// Project the stored authorization onto the modelled `ConfiguredTeam`.
    pub fn view(&self) -> ConfiguredTeam {
        ConfiguredTeam {
            tenant_id: self.tenant_id.clone(),
            team_id: self.team_id.clone(),
            team_name: self.team_name.clone(),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
        }
    }
}

/// One stored Slack channel configuration: everything a later `Describe`, `Update`,
/// `Delete` or tag call has to be able to answer from.
#[derive(Debug, Clone, Default)]
pub struct SlackChannelConfigRecord {
    pub chat_configuration_arn: String,
    pub configuration_name: String,
    /// The region the configuration lives in, the one its ARN names. Chat
    /// configurations are regional, so a listing answers only its own region's.
    pub region: String,
    pub slack_team_id: String,
    pub slack_team_name: String,
    pub slack_channel_id: String,
    pub slack_channel_name: String,
    pub iam_role_arn: String,
    pub sns_topic_arns: Vec<String>,
    pub logging_level: Option<String>,
    pub guardrail_policy_arns: Vec<String>,
    pub user_authorization_required: Option<bool>,
    /// `ENABLED` or `DISABLED`; see [`STATE_ENABLED`].
    pub state: String,
    pub state_reason: Option<String>,
    /// Tags keyed by `TagKey`, so `TagResource` merges by key and the list a read
    /// returns has one entry per key in a stable order.
    pub tags: BTreeMap<String, String>,
}

/// A record's stored tags as the modelled `Tag` list, in `TagKey` order.
///
/// Shared by every kind of chat configuration and by a custom action, so one read cannot
/// answer a different tag shape than another.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(tag_key, tag_value)| Tag {
            tag_key: tag_key.clone(),
            tag_value: tag_value.clone(),
        })
        .collect()
}

impl SlackChannelConfigRecord {
    /// The stored tags as the modelled `Tag` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }

    /// Project the stored record onto the modelled `SlackChannelConfiguration` every
    /// Slack-channel operation returns. The single place that mapping lives, so
    /// create, update and describe cannot answer three different shapes for one
    /// record.
    pub fn view(&self) -> SlackChannelConfiguration {
        SlackChannelConfiguration {
            slack_team_name: self.slack_team_name.clone(),
            slack_team_id: self.slack_team_id.clone(),
            slack_channel_id: self.slack_channel_id.clone(),
            slack_channel_name: self.slack_channel_name.clone(),
            chat_configuration_arn: self.chat_configuration_arn.clone(),
            iam_role_arn: self.iam_role_arn.clone(),
            sns_topic_arns: self.sns_topic_arns.clone(),
            configuration_name: Some(self.configuration_name.clone()),
            logging_level: self.logging_level.clone(),
            guardrail_policy_arns: Some(self.guardrail_policy_arns.clone()),
            user_authorization_required: self.user_authorization_required,
            tags: Some(self.tag_list()),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
        }
    }
}

/// One stored custom action: everything a later `Get`, `List`, `Update`, `Delete` or tag
/// call has to be able to answer from.
///
/// No `Default`: the ARN, the name and the command text are all required, so there is no
/// such thing as a default custom action to insert.
#[derive(Debug, Clone)]
pub struct CustomActionRecord {
    pub custom_action_arn: String,
    /// The name the ARN carries, and the thing a second create for this account
    /// collides with.
    pub action_name: String,
    /// `Definition.CommandText` — the command the action runs.
    pub command_text: String,
    pub alias_name: Option<String>,
    /// The attachments exactly as the request carried them, absent when it carried
    /// none: `Attachments` is optional on both the request and the `CustomAction`, so a
    /// create that named no attachments must not read back as having an empty list.
    pub attachments: Option<Vec<CustomActionAttachment>>,
    /// Tags keyed by `TagKey`, so a tag call merges by key and a read answers one entry
    /// per key in a stable order. The `CustomAction` shape carries no tags; they are
    /// what `ListTagsForResource` answers for the action's ARN.
    pub tags: BTreeMap<String, String>,
    /// The `ClientToken` of the create that minted this action, when it carried one.
    ///
    /// `CreateCustomAction`'s `ClientToken` is an `@idempotencyToken`: a retry carrying
    /// the same token answers the original successful create instead of failing on the
    /// name that create already took, so the token the action was minted under has to
    /// outlive the request.
    pub client_token: Option<String>,
}

impl CustomActionRecord {
    /// The stored tags as the modelled `Tag` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }

    /// Project the stored record onto the modelled `CustomAction` the custom-action
    /// operations return. The single place that mapping lives, so create, get, update
    /// and list cannot answer different shapes for one record.
    pub fn view(&self) -> CustomAction {
        CustomAction {
            custom_action_arn: self.custom_action_arn.clone(),
            definition: CustomActionDefinition {
                command_text: self.command_text.clone(),
            },
            alias_name: self.alias_name.clone(),
            attachments: self.attachments.clone(),
            action_name: Some(self.action_name.clone()),
        }
    }
}

/// One stored Amazon Chime webhook configuration: everything a later `Describe`,
/// `Update`, `Delete` or tag call has to be able to answer from.
///
/// `WebhookUrl` is stored even though no response carries it (the modelled
/// `ChimeWebhookConfiguration` has no `WebhookUrl` member, and the input's is
/// `@sensitive`): it is the webhook's destination, so an update has to be able to
/// replace it and a create has to keep it.
#[derive(Debug, Clone, Default)]
pub struct ChimeWebhookConfigRecord {
    pub chat_configuration_arn: String,
    pub configuration_name: String,
    /// The region the configuration lives in, the one its ARN names. Chat
    /// configurations are regional, so a listing answers only its own region's.
    pub region: String,
    pub webhook_description: String,
    pub webhook_url: String,
    pub iam_role_arn: String,
    pub sns_topic_arns: Vec<String>,
    pub logging_level: Option<String>,
    /// `ENABLED` or `DISABLED`; see [`STATE_ENABLED`].
    pub state: String,
    pub state_reason: Option<String>,
    /// Tags keyed by `TagKey`, so `TagResource` merges by key and the list a read
    /// returns has one entry per key in a stable order.
    pub tags: BTreeMap<String, String>,
}

impl ChimeWebhookConfigRecord {
    /// The stored tags as the modelled `Tag` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }

    /// Project the stored record onto the modelled `ChimeWebhookConfiguration` every
    /// Chime-webhook operation returns. The single place that mapping lives, so
    /// create, update and describe cannot answer three different shapes for one record.
    pub fn view(&self) -> ChimeWebhookConfiguration {
        ChimeWebhookConfiguration {
            webhook_description: self.webhook_description.clone(),
            chat_configuration_arn: self.chat_configuration_arn.clone(),
            iam_role_arn: self.iam_role_arn.clone(),
            sns_topic_arns: self.sns_topic_arns.clone(),
            configuration_name: Some(self.configuration_name.clone()),
            logging_level: self.logging_level.clone(),
            tags: Some(self.tag_list()),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
        }
    }
}

/// The `CustomActionArn` of a custom action:
/// `arn:aws:chatbot::{account}:custom-action/{action_name}`.
///
/// A custom action is scoped to the ACCOUNT, not to a region, so its ARN carries an
/// EMPTY region segment — the form AWS Chatbot answers a create with, and the one the
/// model's `CustomActionArn` pattern allows, its region segment being
/// `[A-Za-z0-9_/.-]{0,63}` (`smithy_query` shape=CustomActionArn), i.e. zero length
/// included. One action name is therefore one action for the whole account, whichever
/// region a request is signed for.
///
/// Built through `emulator_server::emulator_arn` with the empty region that function
/// documents for an account-scoped resource, so the account segment is the one canonical
/// emulator account id and no ARN is assembled by hand.
pub fn custom_action_arn(action_name: &str) -> String {
    // The resource-type segment comes from the same constant
    // [`chatbot_resource_kind`] reads an ARN's kind from, so minting and classifying an
    // ARN cannot disagree about it.
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        "",
        &format!("{CUSTOM_ACTION_RESOURCE_TYPE}/{action_name}"),
    )
}

/// The `ChatConfigurationArn` of a chat configuration of kind `kind` (the ARN
/// resource-path segment that names the chat client: `slack-channel`,
/// `chime-webhook`): `arn:aws:chatbot:{region}:{account}:chat-configuration/{kind}/{name}`.
///
/// Built through `emulator_server::emulator_arn`, so the account segment is the one
/// canonical emulator account id and no ARN is assembled by hand.
///
/// An ARN fully qualifies a configuration, its region included. That is why the
/// ARN-addressed operations (`Update`, `Delete`, `TagResource`, `ListTagsForResource`)
/// look a record up by ARN alone and ignore the signing region, while a listing — which
/// has no ARN to go on — filters on the region the request is addressed to.
pub fn chat_configuration_arn(region: &str, kind: &str, configuration_name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("chat-configuration/{kind}/{configuration_name}"),
    )
}

/// The `ChatConfigurationArn` of a Slack channel configuration:
/// `arn:aws:chatbot:{region}:{account}:chat-configuration/slack-channel/{name}`.
pub fn slack_channel_configuration_arn(region: &str, configuration_name: &str) -> String {
    chat_configuration_arn(region, CHAT_CONFIGURATION_KIND_SLACK, configuration_name)
}

/// The `ChatConfigurationArn` of an Amazon Chime webhook configuration:
/// `arn:aws:chatbot:{region}:{account}:chat-configuration/chime-webhook/{name}`.
pub fn chime_webhook_configuration_arn(region: &str, configuration_name: &str) -> String {
    chat_configuration_arn(region, CHAT_CONFIGURATION_KIND_CHIME, configuration_name)
}

/// The prefix every Slack channel configuration ARN in `region` starts with: the same
/// ARN with an empty configuration name.
///
/// A pagination cursor is one of these ARNs, so this is what tells a token this service
/// issued from a string a caller made up.
pub fn slack_channel_configuration_arn_prefix(region: &str) -> String {
    slack_channel_configuration_arn(region, "")
}

/// The prefix every custom-action ARN of this account starts with: the same ARN with an
/// empty action name. As for the configuration prefixes, this is what tells a pagination
/// token this service issued from a string a caller made up.
pub fn custom_action_arn_prefix() -> String {
    custom_action_arn("")
}

/// The prefix every Amazon Chime webhook configuration ARN in `region` starts with: the
/// same ARN with an empty configuration name. As for the Slack prefix, this is what
/// tells a pagination token this service issued from a string a caller made up.
pub fn chime_webhook_configuration_arn_prefix(region: &str) -> String {
    chime_webhook_configuration_arn(region, "")
}

/// The region this request is addressed to: the SigV4 credential scope's region, or
/// [`DEFAULT_REGION`] when the request carries no scope. Resources are regional, so
/// this is the region an ARN minted for the request belongs to.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsChatbotWheatleyorchestration20171011State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsChatbotWheatleyorchestration20171011State>)
}

/// Authorize the Slack workspace `slack_team_id`, under the display name
/// `slack_team_name`, with this account in `engine`'s store.
///
/// The engine-level form of
/// [`ComAmazonawsChatbotWheatleyorchestration20171011State::authorize_slack_workspace`],
/// for a fixture (or an admin path) that holds the shared engine rather than the store:
/// the Slack client (OAuth) handshake that authorizes a workspace on AWS has no
/// operation in this model, so this is the only way one gets authorized.
///
/// Answers `None` when `engine` is not the shared [`::engine::InMemoryEngine`] (so there
/// is no store to write), otherwise the record now stored for the workspace.
pub fn authorize_slack_workspace<Eng: Send + Sync + 'static>(
    engine: &Eng,
    slack_team_id: &str,
    slack_team_name: &str,
) -> Option<SlackWorkspaceRecord> {
    let state = service_state(engine)?;
    let record = state
        .lock()
        .authorize_slack_workspace(slack_team_id, slack_team_name);
    Some(record)
}

/// Authorize the Microsoft Teams team `team_id`, in tenant `tenant_id` and under the
/// display name `team_name`, with this account in `engine`'s store.
///
/// The engine-level form of
/// [`ComAmazonawsChatbotWheatleyorchestration20171011State::authorize_microsoft_teams_team`],
/// for a fixture (or an admin path) that holds the shared engine rather than the store:
/// the Microsoft Teams client handshake that authorizes a team on AWS has no operation in
/// this model, so this is the only way one gets authorized.
///
/// Answers `None` when `engine` is not the shared [`::engine::InMemoryEngine`] (so there
/// is no store to write), otherwise the record now stored for the team.
pub fn authorize_microsoft_teams_team<Eng: Send + Sync + 'static>(
    engine: &Eng,
    tenant_id: &str,
    team_id: &str,
    team_name: Option<&str>,
) -> Option<MicrosoftTeamsConfiguredTeamRecord> {
    let state = service_state(engine)?;
    let record = state
        .lock()
        .authorize_microsoft_teams_team(tenant_id, team_id, team_name);
    Some(record)
}
