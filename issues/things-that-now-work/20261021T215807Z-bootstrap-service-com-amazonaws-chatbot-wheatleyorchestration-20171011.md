Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the chatbot emulator

_Filed: 2026-10-21T21:58:07Z_
_Operation: service-com-amazonaws-chatbot-wheatleyorchestration-20171011 | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 34 operations in `services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding no state, so no client workflow works at all. Implement a FIRST minimal, fully stateful slice around the service's primary resource — the Slack channel chat configuration (restJson1, sdkId `chatbot`, sigv4 name `chatbot`).

### Happy path to implement (in order)

1. **CreateSlackChannelConfiguration** — `POST /create-slack-channel-configuration -> 201`. Input: `SlackTeamId` (required, `^[0-9A-Z]{1,255}$`), `SlackChannelId` (required, `^[A-Za-z0-9]+$`), `SlackChannelName`, `IamRoleArn` (required), `ConfigurationName` (required, `^[A-Za-z0-9-_]+$`, 1..128), `SnsTopicArns`, `LoggingLevel` (`ERROR|INFO|NONE`), `GuardrailPolicyArns`, `UserAuthorizationRequired`, `Tags` (list<Tag{TagKey,TagValue}>). Must mint and PERSIST a `ChatConfigurationArn` of the form `arn:aws:chatbot:{region}:{account}:chat-configuration/slack-channel/{ConfigurationName}` and return `ChannelConfiguration` (SlackChannelConfiguration) built from the stored record — including the required members `SlackTeamName`, `SlackTeamId`, `SlackChannelId`, `SlackChannelName`, `ChatConfigurationArn`, `IamRoleArn`, `SnsTopicArns` (empty list when none given) plus `State` = `ENABLED`. Creating a second configuration with a `ConfigurationName` that already exists must fail with `ConflictException` (409).
2. **DescribeSlackChannelConfigurations** — `POST /describe-slack-channel-configurations -> 200`. Must return `SlackChannelConfigurations` read back out of the store (not echoed from the request). When `ChatConfigurationArn` is supplied, return only that configuration (empty list if unknown). Honour `MaxResults` (1..100) by truncating and emitting an opaque `NextToken` that resumes the scan on the next call; a request with no `MaxResults` returns everything with no `NextToken`.
3. **UpdateSlackChannelConfiguration** — `POST /update-slack-channel-configuration -> 200`. Input: `ChatConfigurationArn` (required), `SlackChannelId` (required), optional `SlackChannelName`, `SnsTopicArns`, `IamRoleArn`, `LoggingLevel`, `GuardrailPolicyArns`, `UserAuthorizationRequired`. Must mutate the stored record in place (only the members present in the request) and return the UPDATED `ChannelConfiguration`; a subsequent Describe must show the new values. Unknown ARN -> `ResourceNotFoundException` (404).
4. **DeleteSlackChannelConfiguration** — `POST /delete-slack-channel-configuration -> 204`. Input `ChatConfigurationArn` (required). Must remove the record (and its tags) from the store; a second delete of the same ARN -> `ResourceNotFoundException` (404).
5. **TagResource / ListTagsForResource** — `POST /tag-resource -> 200` and `POST /list-tags-for-resource -> 200`. `TagResource(ResourceARN, Tags)` must merge tags onto the stored configuration keyed by `TagKey`; `ListTagsForResource(ResourceARN)` must return exactly the tags currently stored for that ARN (and the ones passed at create time). Unknown ARN on either -> `ResourceNotFoundException` (404).

All five steps must compute their responses from persisted state: no canned bodies, no echoing the input back, no minting an ARN that is never stored. A create followed by a describe must return what was created; a delete followed by a describe must not.

## State Model

In `src/state.rs`, on the shared engine's service state:

- `slack_channel_configurations: BTreeMap<String /* ChatConfigurationArn */, SlackChannelConfigRecord>` — BTreeMap so Describe/pagination has a stable total order.
- `SlackChannelConfigRecord { chat_configuration_arn: String, configuration_name: String, slack_team_id: String, slack_team_name: String, slack_channel_id: String, slack_channel_name: String, iam_role_arn: String, sns_topic_arns: Vec<String>, logging_level: Option<String>, guardrail_policy_arns: Vec<String>, user_authorization_required: Option<bool>, state: String /* ENABLED|DISABLED */, state_reason: Option<String>, tags: BTreeMap<String /* TagKey */, String /* TagValue */> }`
- `configuration_names: BTreeSet<String>` (or derive from the map) to detect the duplicate-name `ConflictException`.
- Pagination: `NextToken` encodes the last-returned ARN (opaque but matching `^[a-zA-Z0-9=\/+_.\-,#:\\"{}]{4,1276}$`); resume by scanning keys strictly greater than it. No separate cursor table needed.

All other operations (Chime webhook, Microsoft Teams, custom actions, associations, user identities, account preferences, UntagResource, …) stay stubbed at 501 for now and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/OPERATIONS.md` lists all 34 operations with their routes, input/output members and errors — the Slack lifecycle used above is lines for CreateSlackChannelConfiguration (`POST /create-slack-channel-configuration -> 201`, output `ChannelConfiguration`), DescribeSlackChannelConfigurations (`@paginated(inputToken=NextToken, outputToken=NextToken, items=SlackChannelConfigurations)`), UpdateSlackChannelConfiguration, DeleteSlackChannelConfiguration (-> 204, `ResourceNotFoundException`), TagResource and ListTagsForResource (`Tags` list, 0..200, `ResourceNotFoundException`). `smithy_query shape=SlackChannelConfiguration` shows the required output members (`SlackTeamName`, `SlackTeamId`, `SlackChannelId`, `SlackChannelName`, `ChatConfigurationArn`, `IamRoleArn`, `SnsTopicArns`) plus `State`/`StateReason`, and `Tag` = `{TagKey, TagValue}` both required. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 34 handler files, including `create_slack_channel_configuration.rs`, `describe_slack_channel_configurations.rs`, `update_slack_channel_configuration.rs`, `delete_slack_channel_configuration.rs`, `tag_resource.rs` and `list_tags_for_resource.rs` — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No tests exercise any behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI chatbot client at it.
2. `aws chatbot create-slack-channel-configuration --slack-team-id T0123ABCD --slack-channel-id C0123ABCD --configuration-name my-config --iam-role-arn arn:aws:iam::123456789012:role/chatbot-role --sns-topic-arns arn:aws:sns:us-east-1:123456789012:alerts --logging-level INFO --tags TagKey=env,TagValue=dev` — expect 201 with a ChatConfigurationArn; today HTTP 501.
3. `aws chatbot describe-slack-channel-configurations` — expect exactly the configuration just created, with the SNS topic, logging level and channel name as sent.
4. `aws chatbot describe-slack-channel-configurations --chat-configuration-arn <arn>` — expect only that one configuration.
5. `aws chatbot update-slack-channel-configuration --chat-configuration-arn <arn> --slack-channel-id C0123ABCD --logging-level ERROR` — expect 200 with LoggingLevel=ERROR; re-describe and confirm the stored record changed.
6. `aws chatbot tag-resource --resource-arn <arn> --tags TagKey=team,TagValue=sre` then `aws chatbot list-tags-for-resource --resource-arn <arn>` — expect both `env=dev` and `team=sre`.
7. Create a second configuration with `--configuration-name my-config` — expect ConflictException (409).
8. `aws chatbot delete-slack-channel-configuration --chat-configuration-arn <arn>` — expect 204; describe again returns an empty list and a second delete returns ResourceNotFoundException (404).
