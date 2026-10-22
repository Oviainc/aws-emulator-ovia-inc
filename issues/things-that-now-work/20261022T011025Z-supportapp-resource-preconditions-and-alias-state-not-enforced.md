Issue-Source: trace-sweep-divergence
Issue-Trace: support-app-slack-channel-create-conflict-without-workspace
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: support-app-slack-channel-create-conflict-without-workspace support-app-account-alias-lifecycle
Issue-Shape-Id: com.amazonaws.supportapp#SupportApp
Issue-Key: supportapp|CreateSlackChannelConfiguration|4xx->2xx|$
Issue-Key: supportapp|CreateSlackChannelConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: supportapp|CreateSlackChannelConfiguration|4xx->2xx|status
Issue-Key: supportapp|GetAccountAlias|2xx->2xx|$
Issue-Commit: 872e0ddf6cb614931de58689afe8393c02abd052

# supportapp: resource preconditions and alias state not enforced

> _Note: this issue was identified against the code base at commit `872e0ddf6cb614931de58689afe8393c02abd052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSlackChannelConfiguration

## Description
Two causes in the supportapp emulator, both in the same family: an operation's precondition/state is not tracked, so the operation reports success and the account-level resource state it should have changed (or refused to change) is wrong. 2 causes, both must be fixed.

Cause 1 of 2: CreateSlackChannelConfiguration succeeds when no Slack workspace is registered for the account.
AWS: CreateSlackChannelConfiguration is modeled with ConflictException (httpError 409). Smithy: CreateSlackChannelConfiguration errors = {AccessDeniedException 403, ConflictException 409, InternalServerException 500, ServiceQuotaExceededException 402, ValidationException 400}; ConflictException documentation lists as a trigger "Add, update, or delete a Slack channel configuration before you add a Slack workspace to your Amazon Web Services account." The recording confirms it: HTTP 409 with header x-amzn-errortype: ConflictException and a body carrying a `message` member (recorded example message: "Workspace installation not found for account 000000000000 and team <teamId>" — the account id and team id come from the request/caller, so the text is an example, the 409 + ConflictException are the requirement).
Emulator: returns HTTP 200 with an empty JSON body `{}` and no x-amzn-errortype header, i.e. it creates (or pretends to create) the channel configuration for a workspace that was never registered.
Why: a client that provisions Support App notifications by first registering a workspace and then creating channel configurations cannot discover the missing-workspace condition. Error-handling paths for ConflictException are never exercised, and the client proceeds believing a notification channel exists when against AWS the call would have failed, so the configuration it later reads back or deletes does not exist.
Fix: the CreateSlackChannelConfiguration handler's precondition check plus the service state that records registered Slack workspaces — the handler must look the request's teamId up in that workspace registry and raise ConflictException (409, x-amzn-errortype: ConflictException, body with `message`) when it is absent, instead of falling through to the success path.
Traces: support-app-slack-channel-create-conflict-without-workspace#task#1 (expected 409 ConflictException, got 200 `{}`).

Cause 2 of 2: DeleteAccountAlias does not clear the stored account alias, so GetAccountAlias still returns it.
AWS: GetAccountAlias's output member `accountAlias` is optional (Smithy: GetAccountAliasResult.accountAlias, target awsAccountAlias, @length 1..30, @pattern ^[\\w\\- ]+$, not required; doc "An alias or short name for an Amazon Web Services account"), and DeleteAccountAlias is modeled with empty input and empty output. After a successful DeleteAccountAlias, the alias no longer exists, so GetAccountAlias returns HTTP 200 with a body that omits `accountAlias` entirely — the recording's teardown read is exactly `{}`.
Emulator: after the lifecycle's DeleteAccountAlias, the teardown GetAccountAlias still answers HTTP 200 with `accountAlias` populated (recorded actual: {"accountAlias": "<alias previously put>"}). The delete is accepted but the alias is not removed from state.
Why: the alias is account-global state, so a client (or a later fixture/test) that deletes the alias and re-reads it sees a stale value and will treat the account as still aliased — teardown/cleanup never converges, and a put-then-delete-then-get sequence returns a value AWS would not return. Code branching on the presence of the optional member takes the wrong path.
Fix: the DeleteAccountAlias handler's state mutation — it must clear the alias in the account's supportapp state so GetAccountAlias serializes an output with the optional `accountAlias` member absent (empty object), rather than leaving the stored value in place.
Traces: support-app-account-alias-lifecycle#teardown#7 (GetAccountAlias after DeleteAccountAlias: expected `{}`, got `{"accountAlias": ...}`).

## Evidence
Cause 1 — fixture support-app-slack-channel-create-conflict-without-workspace, step task#1, CreateSlackChannelConfiguration:
  expected: status 409, header x-amzn-errortype: ConflictException, body {"message": "Workspace installation not found for account 000000000000 and team <teamId>"} (message is an example)
  actual:   status 200, body {} , x-amzn-errortype missing (None)
  replay detail: "status: expected 409, got 200 / $: empty_body / $headers.x-amzn-errortype: missing (expected 'ConflictException', got None)"

Cause 2 — fixture support-app-account-alias-lifecycle, step teardown#7, GetAccountAlias:
  expected: status 200, body {}
  actual:   status 200, body {"accountAlias": "sim2real-alias2-2b4a8da5"}
  replay detail: "$: value_mismatch (expected {}, got {'accountAlias': ...})"

Model grounding: Smithy CreateSlackChannelConfiguration binds ConflictException (409) and its doc names "add a channel configuration before you add a Slack workspace" as the conflict; GetAccountAliasResult.accountAlias is an optional member; DeleteAccountAlias has empty output.

## Steps to Reproduce
Cause 1: against a fresh emulator account with NO Slack workspace registered, POST /control/create-slack-channel-configuration with a valid teamId, channelId, channelRoleArn and notifyOnCaseSeverity (e.g. "none"). Observe HTTP 200 and an empty body; AWS answers 409 with x-amzn-errortype: ConflictException. Replay support-app-slack-channel-create-conflict-without-workspace#task#1.

Cause 2: POST /control/put-account-alias with a valid accountAlias, then POST /control/delete-account-alias, then POST /control/get-account-alias. Observe the alias still present in the GetAccountAlias response body; AWS returns {} . Replay support-app-account-alias-lifecycle (step teardown#7).
