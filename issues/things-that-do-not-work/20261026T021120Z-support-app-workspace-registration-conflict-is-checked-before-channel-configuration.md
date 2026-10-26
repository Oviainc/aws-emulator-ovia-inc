Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: support-app.delete-slack-channel-configuration-no-workspace-not-found support-app.update-slack-channel-configuration-no-workspace-not-found
Issue-Shape-Id: com.amazonaws.supportapp#SupportApp
Issue-Key: support-app.delete-slack-channel-configuration-no-workspace-not-found|run#5|AssertionError|assert code == "ResourceNotFoundException", f"unexpected code: {code}: {message}"
Issue-Key: support-app.update-slack-channel-configuration-no-workspace-not-found|run#8|AssertionError|assert code == "ResourceNotFoundException", f"unexpected code: {code}: {message}"
Issue-Commit: 13cd51356c86db6d039912819137db7f55e61de7

# Workspace-registration conflict is checked before channel-configuration existence: Delete/UpdateSlackChannelConfiguration answer 409 ConflictException where AWS answers 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `13cd51356c86db6d039912819137db7f55e61de7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSlackChannelConfiguration

## Description
REOPENS 20261024T194403Z-support-app-deleteslackchannelconfiguration-error-response.md: its fix left the call refused. That fix added an unregistered-workspace ConflictException gate AHEAD of the channel-configuration lookup in the delete handler, and the same gate was added to the update handler (which the still-open issue 20261024T194403Z-support-app-updateslackchannelconfiguration-error-response.md asks for). Replay shows real AWS orders these the other way round: the existence of the specific (teamId, channelId) SlackChannelConfiguration resource is settled FIRST, so a pair that is not configured is a 404 ResourceNotFoundException even when the account has registered no Slack workspace at all. The ConflictException the model documents ("Add, update, or delete a Slack channel configuration before you add a Slack workspace...") is reachable on other paths (e.g. the create path, where the named channel config does not and cannot exist), but it must not pre-empt the not-found answer for a channel configuration that is simply absent. This issue bundles 2 causes, one per affected operation; both share this single ordering root cause and both must be closed.

Cause 1 of 2: DeleteSlackChannelConfiguration — workspace conflict pre-empts not-found.
AWS: Smithy (services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json) binds DeleteSlackChannelConfiguration POST /control/delete-slack-channel-configuration with ResourceNotFoundException (httpError 404) — "The specified resource is missing or doesn't exist, such as an account alias, Slack channel configuration, or Slack workspace configuration" — and ConflictException (httpError 409). Recorded behavior: for a well-formed teamId/channelId pair (teamId matching ^\S+$, len 1..256) that has no SlackChannelConfiguration and whose workspace is not registered, AWS refuses with HTTP 404, error code ResourceNotFoundException, message "Slack channel configuration for team <teamId> and channel <channelId> not found". Fixed text of that message: "Slack channel configuration for team ", " and channel ", " not found"; the variable parts are the request's teamId and channelId values — never a recording's literal ids.
Emulator: answers HTTP 409, error code ConflictException, message "Workspace installation not found for account <accountId> and team <teamId>" (account id and team id are per-run values here, not part of any requirement). The handler consults store.unregistered_workspace_conflict(&input.team_id) before store.slack_channel_configurations.remove(&key).
Why: a client that branches on the refusal class sees a retryable/precondition conflict (409) where AWS reports a definitively absent resource (404). Idempotent teardown code that swallows ResourceNotFoundException on delete now sees an unexpected ConflictException and fails; the message is also a different template, so message-matching clients break.
Fix: the DeleteSlackChannelConfiguration handler's refusal ordering — look the (teamId, channelId) configuration up first and return ResourceNotFoundException with state::slack_channel_configuration_not_found; only consult the workspace registry when a conflict is the applicable answer.
Traces: support-app.delete-slack-channel-configuration-no-workspace-not-found.

Cause 2 of 2: UpdateSlackChannelConfiguration — same inversion, and it also masks nothing else (the operation's ValidationException path is correct).
AWS: Smithy binds UpdateSlackChannelConfiguration POST /control/update-slack-channel-configuration with the same ResourceNotFoundException (404) and ConflictException (409); teamId is required, len 1..256, pattern ^\S+$. Recorded behavior: a well-formed teamId/channelId pair with no stored configuration (and no registered workspace) is refused with HTTP 404 ResourceNotFoundException, message "Slack channel configuration for team <teamId> and channel <channelId> not found" (same fixed text, request-derived values). The replay's own control case confirms the validation path is right: teamId "   " is still refused 400 ValidationException with "1 validation error detected: Value '   ' at 'teamId' failed to satisfy constraint: Member must satisfy regular expression pattern: \\S+".
Emulator: answers HTTP 409 ConflictException "Workspace installation not found for account <accountId> and team <teamId>"; the handler runs store.unregistered_workspace_conflict(&input.team_id) before the store.slack_channel_configurations.get(&key) lookup. Note the open issue 20261024T194403Z-support-app-updateslackchannelconfiguration-error-response.md asks for exactly this gate and its premise is contradicted by this replay — it should be closed in favour of AWS's observed ordering.
Why: a client cannot distinguish "this channel was never configured" from "this workspace is not registered"; update-or-create flows that key off 404 never take the create branch and surface a conflict instead.
Fix: the UpdateSlackChannelConfiguration handler's refusal ordering (same change as cause 1); keep the channelRoleArn account check and the teamId pattern validation where they are.
Traces: support-app.update-slack-channel-configuration-no-workspace-not-found.

## Evidence
Suite: support-app 15/17 (88%), 2 mismatches.

[mismatch] support-app.delete-slack-channel-configuration-no-workspace-not-found
  AssertionError: unexpected code: ConflictException: Workspace installation not found for account 000000000000 and team TF655FC4856
  Expected: code == "ResourceNotFoundException" (HTTP 404) with message "Slack channel configuration for team <teamId> and channel <channelId> not found".

[mismatch] support-app.update-slack-channel-configuration-no-workspace-not-found
  AssertionError: unexpected code: ConflictException: Workspace installation not found for account 000000000000 and team T33A749D3D8
  Expected: code == "ResourceNotFoundException" (HTTP 404) with the same message template. The check's ValidationException control (teamId="   ") passed, so the divergence is isolated to the missing-resource path.

Code (citations only):
- services/service-com-amazonaws-supportapp-supportapp/src/ops/delete_slack_channel_configuration.rs lines 170-204: unregistered_workspace_conflict -> ConflictException returned before slack_channel_configurations.remove(&key) -> ResourceNotFoundException.
- services/service-com-amazonaws-supportapp-supportapp/src/ops/update_slack_channel_configuration.rs lines 230-264: unregistered_workspace_conflict -> ConflictException returned before slack_channel_configurations.get(&key) -> ResourceNotFoundException.

Model: smithy_query(query="operation", operation="DeleteSlackChannelConfiguration,UpdateSlackChannelConfiguration") — both declare ResourceNotFoundException httpError 404 and ConflictException httpError 409; ResourceNotFoundException doc: "The specified resource is missing or doesn't exist, such as an account alias, Slack channel configuration, or Slack workspace configuration."

History: the delete-side gate was introduced by the fixed issue 20261024T194403Z-support-app-deleteslackchannelconfiguration-error-response.md; the update-side gate is requested by the still-open 20261024T194403Z-support-app-updateslackchannelconfiguration-error-response.md. Replay contradicts both for the case where the channel configuration itself does not exist. Repo tests cement the old behavior and will need updating: tests/all/slack_channel_configurations.rs::an_unconfigured_channel_cannot_be_updated_or_deleted.

## Steps to Reproduce
1. Boot the emulator with a fresh account; register NO Slack workspace.
2. Delete path: POST /control/delete-slack-channel-configuration with {"teamId": "T<well-formed id>", "channelId": "C<well-formed id>"} for a pair that was never configured.
   Observed: HTTP 409, x-amzn-errortype: ConflictException, message "Workspace installation not found for account <account> and team <teamId>".
   AWS: HTTP 404, x-amzn-errortype: ResourceNotFoundException, message "Slack channel configuration for team <teamId> and channel <channelId> not found".
3. Update path: POST /control/update-slack-channel-configuration with {"teamId": "T<well-formed id>", "channelId": "C<well-formed id>", "notifyOnCaseSeverity": "none"} for the same never-configured pair.
   Observed: HTTP 409 ConflictException; AWS: HTTP 404 ResourceNotFoundException with the message above.
4. Control (already correct, must stay correct): the same update with teamId "   " returns HTTP 400 ValidationException "1 validation error detected: Value '   ' at 'teamId' failed to satisfy constraint: Member must satisfy regular expression pattern: \\S+".
5. Control (already correct): ListSlackWorkspaceConfigurations and ListSlackChannelConfigurations still answer 200 with lists, and no configuration is left behind for the rejected pair.
