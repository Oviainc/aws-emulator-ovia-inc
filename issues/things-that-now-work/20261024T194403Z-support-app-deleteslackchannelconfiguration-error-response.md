Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7cd33b8d616838a49131331fab4e1704473d18dc

# Return ConflictException when deleting a channel configuration for an unregistered Slack workspace

_Filed: 2026-10-24T19:44:03Z_
_Operation: DeleteSlackChannelConfiguration | Class: error-response_

> _Note: this issue was identified against the code base at commit `7cd33b8d616838a49131331fab4e1704473d18dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ConflictException (HTTP 409) is modeled on DeleteSlackChannelConfiguration and its documentation names the trigger explicitly: "Add, update, or delete a Slack channel configuration before you add a Slack workspace to your Amazon Web Services account." A delete whose teamId is not in the account's Slack workspace registry is therefore expected to answer HTTP 409 with x-amzn-errortype: ConflictException.

Actual: the handler removes the (teamId, channelId) key from slack_channel_configurations and, when nothing was removed, returns HTTP 404 ResourceNotFoundException. It never consults state.slack_workspace_configurations, so the unregistered-workspace conflict is indistinguishable from an unconfigured channel and the modeled ConflictException variant is unreachable for this operation.

Expected: 409 ConflictException when the teamId has no registered workspace; 404 ResourceNotFoundException only when the workspace is registered but that channel is not configured.

## Evidence

Spec: Smithy model binds ConflictException (httpError 409) to DeleteSlackChannelConfiguration; the ConflictException shape doc names "Add, update, or delete a Slack channel configuration before you add a Slack workspace to your Amazon Web Services account". The same rule is already implemented for the create path (src/ops/create_slack_channel_configuration.rs workspace-registry check + state::workspace_not_registered_message) and confirmed against a real recording in the fixed issue 20261022T011025Z-supportapp-resource-preconditions-and-alias-state-not-enforced. Handler: src/ops/delete_slack_channel_configuration.rs `handle` — `state.lock().slack_channel_configurations.remove(&key).is_none()` → ResourceNotFoundException, with no workspace lookup. Tests: tests/all/slack_channel_configurations.rs::an_unconfigured_channel_cannot_be_updated_or_deleted asserts 404 for delete on unregistered team ids, so the divergence is currently locked in by a test; no test covers the documented conflict for delete.

## Steps to Reproduce

1. Start the emulator with a fresh account; register no Slack workspace.
2. POST /control/delete-slack-channel-configuration with {"teamId": "T99999ZZZZZ", "channelId": "C01234A5BCD"}.
3. Observe HTTP 404 ResourceNotFoundException; AWS answers HTTP 409 ConflictException ("Workspace installation not found for account <account> and team T99999ZZZZZ").
4. POST /control/register-slack-workspace-for-organization {"teamId": "T99999ZZZZZ"}, then repeat the delete: 404 ResourceNotFoundException is then the correct answer.
