Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7cd33b8d616838a49131331fab4e1704473d18dc

# Return ConflictException when updating a channel configuration for an unregistered Slack workspace

_Filed: 2026-10-24T19:44:03Z_
_Operation: UpdateSlackChannelConfiguration | Class: error-response_

> _Note: this issue was identified against the code base at commit `7cd33b8d616838a49131331fab4e1704473d18dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A Slack channel configuration can only exist inside a Slack workspace that is registered to the account. The modeled ConflictException (HTTP 409) documentation enumerates its triggers and names this one first: "Add, update, or delete a Slack channel configuration before you add a Slack workspace to your Amazon Web Services account." So an UpdateSlackChannelConfiguration whose teamId is not in the account's workspace registry is expected to answer HTTP 409 with x-amzn-errortype: ConflictException (the recorded AWS message form being "Workspace installation not found for account <account> and team <teamId>", as the already-fixed create-path issue documents).

Actual: the handler never consults state.slack_workspace_configurations at all. It looks the (teamId, channelId) pair up in slack_channel_configurations only, so an update naming an unregistered workspace falls into the generic miss path and answers HTTP 404 ResourceNotFoundException. ConflictException is a declared variant of UpdateSlackChannelConfigurationErrorModeled that the handler can never produce.

Expected: 409 ConflictException when the request's teamId is absent from the workspace registry; 404 ResourceNotFoundException only when the workspace IS registered but the named channel is not configured in it.

## Evidence

Spec: Smithy model binds ConflictException (httpError 409) to UpdateSlackChannelConfiguration, and the ConflictException shape doc lists "Add, update, or delete a Slack channel configuration before you add a Slack workspace to your Amazon Web Services account" as a trigger. CreateSlackChannelConfiguration already implements exactly this check (services/service-com-amazonaws-supportapp-supportapp/src/ops/create_slack_channel_configuration.rs, the `!store.slack_workspace_configurations.contains_key(&input.team_id)` branch returning ConflictException with state::workspace_not_registered_message), and the fixed trace-sweep issue 20261022T011025Z-supportapp-resource-preconditions-and-alias-state-not-enforced records the real AWS 409 for the create path. Handler: src/ops/update_slack_channel_configuration.rs `handle` — only `store.slack_channel_configurations.get(&key)` is consulted, else ResourceNotFoundException. Tests: tests/all/slack_channel_configurations.rs::an_unconfigured_channel_cannot_be_updated_or_deleted asserts 404/ResourceNotFoundException for unregistered team ids (e.g. "T99999ZZZZZ", "T"), i.e. the current divergent behavior is cemented by a test and no test asserts the documented conflict for update.

## Steps to Reproduce

1. Start the emulator with a fresh account; register no Slack workspace.
2. POST /control/update-slack-channel-configuration with {"teamId": "T012ABCDEFG", "channelId": "C01234A5BCD", "notifyOnCaseSeverity": "none"}.
3. Observe HTTP 404 with x-amzn-errortype: ResourceNotFoundException; AWS answers HTTP 409 with x-amzn-errortype: ConflictException because no Slack workspace was added to the account.
4. For contrast, POST /control/register-slack-workspace-for-organization {"teamId": "T012ABCDEFG"} and repeat the update: now 404 ResourceNotFoundException is the correct answer (workspace exists, channel is not configured).
