Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.e2e-webhook-triggered-build-retry-flow
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|UpdateWebhook|InvalidInputException
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# CodeBuild UpdateWebhook refuses a call that omits buildType on a NO_SOURCE project's existing webhook with InvalidInputException 'Invalid build type provided for NO_SOURCE type webhook'

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWebhook

## Description
REOPENS 20261025T014438Z-codebuild-updatewebhook-stub.md: its fix left the call refused.

AWS: Smithy UpdateWebhook(projectName @required ProjectName @length 2-150 @pattern ^[A-Za-z0-9][A-Za-z0-9\\-_]{1,149}$, branchFilter, rotateSecret @default false, filterGroups, buildType WebhookBuildType @enum [BUILD, BUILD_BATCH, RUNNER_BUILDKITE_BUILD], pullRequestBuildPolicy) -> {webhook}; modeled errors InvalidInputException, OAuthProviderException, ResourceNotFoundException (all client, httpError 400). buildType is OPTIONAL: an UpdateWebhook that names only projectName mutates nothing and succeeds with 200, echoing the stored webhook — its existing buildType (RUNNER_BUILDKITE_BUILD for a webhook created that way on a NO_SOURCE project) and its existing filterGroups unchanged. An absent member is not a buildType of 'BUILD'.
Emulator: UpdateWebhook(projectName=<project with a live RUNNER_BUILDKITE_BUILD webhook>) answers 400 InvalidInputException with message 'Invalid build type provided for NO_SOURCE type webhook' — the fixed sentence the handler emits when it validates a buildType against a NO_SOURCE project. The request carried no buildType at all, so the handler is validating a defaulted/invented value rather than preserving the stored one. The webhook created moments earlier by CreateWebhook(buildType='RUNNER_BUILDKITE_BUILD') on the same project is therefore unmodifiable.
Why: the documented no-op/partial-update form of UpdateWebhook — the only way to rotate a secret or re-read a webhook without rewriting every field — is unreachable, and a client is told its build type is invalid for a value it never sent. Any webhook lifecycle (create, update, delete) on a NO_SOURCE project aborts at the update step, and the downstream ResourceNotFoundException-after-delete behavior cannot be exercised at all.
Fix: the UpdateWebhook handler's buildType handling in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006) — treat an absent buildType as 'leave the stored value unchanged' and only run the NO_SOURCE compatibility check against a buildType the request actually supplied.
Traces: codebuild.e2e-webhook-triggered-build-retry-flow.

## Evidence
Replay of the behavioral suite (simparity-behavioral-aws-v1) against the emulator built from this checkout; per-service line: codebuild 104/145 (72%), 41 failed.

codebuild.e2e-webhook-triggered-build-retry-flow [error]: botocore.errorfactory.InvalidInputException: An error occurred (InvalidInputException) when calling the UpdateWebhook operation: Invalid build type provided for NO_SOURCE type webhook.
The failing call is the check's positive control, `update_webhook(projectName=<project>)` with no other member set, issued against a webhook the same run created with CreateWebhook(buildType='RUNNER_BUILDKITE_BUILD') on a NO_SOURCE project; the check then asserts the response's buildType and filterGroups are unchanged. Expected: HTTP 200 with webhook.buildType 'RUNNER_BUILDKITE_BUILD' and the stored filterGroups. Actual: HTTP 400, __type InvalidInputException, message 'Invalid build type provided for NO_SOURCE type webhook'.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — UpdateWebhook input {projectName @required, branchFilter, rotateSecret, filterGroups, buildType (optional, WebhookBuildType), pullRequestBuildPolicy}, output {webhook}; WebhookBuildType enum [BUILD, BUILD_BATCH, RUNNER_BUILDKITE_BUILD].

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. CreateProject with source type NO_SOURCE and a CodeBuild service role.
3. CreateWebhook(projectName=<project>, buildType='RUNNER_BUILDKITE_BUILD', filterGroups=[...]) — succeeds.
4. Call UpdateWebhook(projectName=<project>) with no other input member.
5. Expect HTTP 200 and a webhook whose buildType is still 'RUNNER_BUILDKITE_BUILD' and whose filterGroups are unchanged; observe HTTP 400 InvalidInputException 'Invalid build type provided for NO_SOURCE type webhook'.
6. Re-replay codebuild.e2e-webhook-triggered-build-retry-flow to confirm.
