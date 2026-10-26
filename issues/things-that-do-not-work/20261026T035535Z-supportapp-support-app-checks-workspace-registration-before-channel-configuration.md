Issue-Source: trace-sweep-divergence
Issue-Trace: support-app-delete-slack-channel-configuration-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: support-app-delete-slack-channel-configuration-not-found support-app-update-slack-channel-configuration-not-found
Issue-Shape-Id: com.amazonaws.supportapp#SupportApp
Issue-Key: supportapp|DeleteSlackChannelConfiguration|4xx->4xx|$.message
Issue-Key: supportapp|DeleteSlackChannelConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: supportapp|DeleteSlackChannelConfiguration|4xx->4xx|status
Issue-Key: supportapp|UpdateSlackChannelConfiguration|4xx->4xx|$.message
Issue-Key: supportapp|UpdateSlackChannelConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: supportapp|UpdateSlackChannelConfiguration|4xx->4xx|status
Issue-Commit: d958b1b01392fe431b86227462c36506cac78d51

# Support App checks workspace registration before channel-configuration existence on Delete/UpdateSlackChannelConfiguration

> _Note: this issue was identified against the code base at commit `d958b1b01392fe431b86227462c36506cac78d51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSlackChannelConfiguration

## Description
The Support App control plane evaluates its unregistered-Slack-workspace ConflictException gate BEFORE it looks up the (teamId, channelId) SlackChannelConfiguration, so a channel configuration that simply does not exist is refused as a 409 conflict instead of a 404 not-found. Wire replay of the recorded corpus shows real AWS settles resource existence first. This issue bundles 2 causes (one per affected operation) that share this single ordering root cause; BOTH must be closed.

Note: the open behavioral issue 20261026T021120Z-support-app-workspace-registration-conflict-is-checked-before-channel-configuration.md describes the same root cause, but it is grounded in the behavioral suite and carries no wire replay key, so these two wire fixture steps are not gated by it; the open issue 20261024T194403Z-support-app-updateslackchannelconfiguration-error-response.md asks for exactly the gate that this replay contradicts.

Cause 1 of 2: DeleteSlackChannelConfiguration returns ConflictException for an absent channel configuration. Fixture support-app-delete-slack-channel-configuration-not-found, step task#1.
AWS: Smithy (services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json) binds DeleteSlackChannelConfiguration to POST /control/delete-slack-channel-configuration with required teamId and channelId (@length 1..256, pattern ^\S+$) and errors ResourceNotFoundException (httpError 404) and ConflictException (httpError 409). Recorded behavior: for a well-formed teamId/channelId pair that has no stored SlackChannelConfiguration, and whose Slack workspace is not registered for the account, AWS refuses with HTTP 404, header x-amzn-errortype: ResourceNotFoundException, and message "Slack channel configuration for team <teamId> and channel <channelId> not found". Fixed text of that message: "Slack channel configuration for team ", " and channel ", " not found"; the variable parts are the teamId and channelId taken from the request (the recording's concrete ids are a per-run fill of that template, not part of the requirement).
Emulator: answers HTTP 409 with x-amzn-errortype: ConflictException and message "Workspace installation not found for account <accountId> and team <teamId>" (the account id is an environment value and the team id comes from the request; neither makes the message correct). The workspace-registration check runs ahead of the configuration lookup.
Why: a client that branches on the refusal class sees a precondition/conflict answer where AWS reports a definitively absent resource. Idempotent teardown code that tolerates ResourceNotFoundException on delete instead surfaces an unexpected ConflictException and fails, and message-matching clients see a different template entirely.
Fix: the refusal ordering in the DeleteSlackChannelConfiguration handler — resolve the (teamId, channelId) configuration first and return ResourceNotFoundException when it is absent; consult the workspace registry only on paths where a conflict is the applicable answer.
Traces: support-app-delete-slack-channel-configuration-not-found#task#1.

Cause 2 of 2: UpdateSlackChannelConfiguration returns ConflictException for an absent channel configuration. Fixture support-app-update-slack-channel-configuration-not-found, step task#1.
AWS: Smithy binds UpdateSlackChannelConfiguration to POST /control/update-slack-channel-configuration with the same required teamId/channelId (@length 1..256, pattern ^\S+$) and the same modeled ResourceNotFoundException (404) / ConflictException (409). Recorded behavior: a well-formed teamId/channelId pair with no stored configuration is refused with HTTP 404, x-amzn-errortype: ResourceNotFoundException, message "Slack channel configuration for team <teamId> and channel <channelId> not found" — same fixed text, same request-derived variable parts.
Emulator: answers HTTP 409, x-amzn-errortype: ConflictException, message "Workspace installation not found for account <accountId> and team <teamId>", because the unregistered-workspace check precedes the configuration lookup in the update handler too.
Why: callers cannot distinguish "this channel was never configured" from "this workspace is not registered"; update-or-create flows that key off 404 never take the create branch and instead propagate a conflict.
Fix: the refusal ordering in the UpdateSlackChannelConfiguration handler (same change as cause 1); the operation's teamId pattern validation and channelRoleArn checks are correct and must stay where they are.
Traces: support-app-update-slack-channel-configuration-not-found#task#1.

## Evidence
Wire replay, trace set simparity-wire-ga-prod-v1: 17 steps, 15 passed, 2 mismatched (score 88.2%), emulator up and serving.

[mismatch] support-app-delete-slack-channel-configuration-not-found#task#1 — DeleteSlackChannelConfiguration
  status: expected 404, got 409
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException', got 'ConflictException'
  $.message: expected 'Slack channel configuration for team <teamId> and channel <channelId> not found' (recorded with the fixture's probe team/channel ids), got 'Workspace installation not found for account <accountId> and team <teamId>'

[mismatch] support-app-update-slack-channel-configuration-not-found#task#1 — UpdateSlackChannelConfiguration
  status: expected 404, got 409
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException', got 'ConflictException'
  $.message: expected the same 'Slack channel configuration for team <teamId> and channel <channelId> not found' template, got 'Workspace installation not found for account <accountId> and team <teamId>'

Model: smithy_query(query="operation", operation="DeleteSlackChannelConfiguration,UpdateSlackChannelConfiguration") on services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json — both operations declare ResourceNotFoundException httpError 404 and ConflictException httpError 409; ResourceNotFoundException is documented as "The specified resource is missing or doesn't exist, such as an account alias, Slack channel configuration, or Slack workspace configuration."

## Steps to Reproduce
1. Boot the emulator with a fresh account and register NO Slack workspace.
2. Replay fixture support-app-delete-slack-channel-configuration-not-found, step task#1: POST /control/delete-slack-channel-configuration with a well-formed, never-configured {"teamId": ..., "channelId": ...} pair.
   Observed: HTTP 409, x-amzn-errortype: ConflictException, message "Workspace installation not found for account <accountId> and team <teamId>".
   Expected (AWS): HTTP 404, x-amzn-errortype: ResourceNotFoundException, message "Slack channel configuration for team <teamId> and channel <channelId> not found".
3. Replay fixture support-app-update-slack-channel-configuration-not-found, step task#1: POST /control/update-slack-channel-configuration for the same never-configured pair.
   Observed: HTTP 409 ConflictException with the same workspace message; expected HTTP 404 ResourceNotFoundException with the message template above.
4. Controls that already pass and must keep passing: the teamId pattern ValidationException path (400, "Member must satisfy regular expression pattern: \\S+"), and the List* operations answering 200.
5. Re-replay the wire corpus: both steps must pass and the remaining 15 steps must stay green.
