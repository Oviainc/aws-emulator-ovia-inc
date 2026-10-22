// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-chatbot-wheatleyorchestration-20171011 service.
//!
//! This module owns the persisted shape of every chat configuration the service
//! stores, plus the few helpers the Slack-channel handlers share (the ARN form, the
//! request's region, and the record -> modelled-shape projection). Handlers reach it
//! through [`service_state`]; nothing here is per-request.
//!
//! It also owns the registry of Slack workspaces AUTHORIZED with the account, which a
//! channel configuration has to name one of — see [`seed_slack_workspaces`] for how a
//! workspace gets into it, given that no operation in this model authorizes one.

use std::collections::BTreeMap;

use crate::ops::types::{SlackChannelConfiguration, SlackWorkspace, Tag};

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. `us-east-1` is the emulator-wide default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `chatbot` ARN service namespace (`OPERATIONS.md`: "ARN namespace: chatbot").
const ARN_SERVICE: &str = "chatbot";

/// `State` of a chat configuration AWS Chatbot reports as usable. A Slack channel
/// configuration is usable the moment it is created — AWS returns no transient
/// status for it — so this is the only state a created record is ever in.
pub const STATE_ENABLED: &str = "ENABLED";

/// The most tags a chat configuration can carry: the model's `TagList` is
/// `length 0..200`, so a merge that would exceed it is refused.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

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

// A `Describe` that names no `MaxResults` answers a region's configurations in ONE page
// with no `NextToken`. That holds only while the store cannot hold more than one page,
// so the two bounds are tied here rather than left to drift apart.
const _: () = assert!(MAX_SLACK_CHANNEL_CONFIGURATIONS <= MAX_PAGE_SIZE);

/// Per-service state for com-amazonaws-chatbot-wheatleyorchestration-20171011. A handler reaches it through [`service_state`].
///
/// `BTreeMap` keyed by `ChatConfigurationArn`, so `DescribeSlackChannelConfigurations`
/// walks the configurations in one stable total order and can resume that walk from
/// an opaque token.
#[derive(Debug)]
pub struct ComAmazonawsChatbotWheatleyorchestration20171011State {
    /// Every Slack channel configuration, keyed by its `ChatConfigurationArn`.
    pub slack_channel_configurations: BTreeMap<String, SlackChannelConfigRecord>,
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
}

impl Default for ComAmazonawsChatbotWheatleyorchestration20171011State {
    fn default() -> Self {
        Self {
            slack_channel_configurations: BTreeMap::new(),
            slack_workspace_authorizations: seed_slack_workspaces()
                .into_iter()
                .map(|workspace| (workspace.slack_team_id.clone(), workspace))
                .collect(),
        }
    }
}

impl ComAmazonawsChatbotWheatleyorchestration20171011State {
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

impl SlackChannelConfigRecord {
    /// The stored tags as the modelled `Tag` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(tag_key, tag_value)| Tag {
                tag_key: tag_key.clone(),
                tag_value: tag_value.clone(),
            })
            .collect()
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

/// The `ChatConfigurationArn` of a Slack channel configuration:
/// `arn:aws:chatbot:{region}:{account}:chat-configuration/slack-channel/{name}`.
///
/// Built through `emulator_server::emulator_arn`, so the account segment is the one
/// canonical emulator account id and no ARN is assembled by hand.
///
/// An ARN fully qualifies a configuration, its region included. That is why the
/// ARN-addressed operations (`Update`, `Delete`, `TagResource`, `ListTagsForResource`)
/// look a record up by ARN alone and ignore the signing region, while a listing — which
/// has no ARN to go on — filters on the region the request is addressed to.
pub fn slack_channel_configuration_arn(region: &str, configuration_name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("chat-configuration/slack-channel/{configuration_name}"),
    )
}

/// The prefix every Slack channel configuration ARN in `region` starts with: the same
/// ARN with an empty configuration name.
///
/// A pagination cursor is one of these ARNs, so this is what tells a token this service
/// issued from a string a caller made up.
pub fn slack_channel_configuration_arn_prefix(region: &str) -> String {
    slack_channel_configuration_arn(region, "")
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
