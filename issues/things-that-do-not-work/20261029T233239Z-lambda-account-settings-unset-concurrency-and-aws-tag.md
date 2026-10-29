Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-account-settings-read
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-account-settings-read lambda-function-lifecycle-crud lambda-get-concurrency-unset-empty lambda-function-tagging-lifecycle
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|GetAccountSettings|2xx->2xx|$.HasFunctionWithDeprecatedRuntime
Issue-Key: lambda|GetFunctionConcurrency|2xx->2xx|$
Issue-Key: lambda|TagResource|4xx->2xx|$
Issue-Key: lambda|TagResource|4xx->2xx|$headers.content-length
Issue-Key: lambda|TagResource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|TagResource|4xx->2xx|status
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda account settings, unset concurrency and aws: tag validation diverge from the recorded wire

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountSettings

## Description
Shared fault: three single-operation read/validation paths answer with a different body than the recorded wire. Three net-new clusters.

AWS: (1) GetAccountSettings — recorded behavior only: the response carries a top-level `HasFunctionWithDeprecatedRuntime` boolean (false here) alongside AccountLimit/AccountUsage; the on-disk GetAccountSettingsResponse models only AccountLimit and AccountUsage, so this member is attested by the recording. (2) GetFunctionConcurrency for a function with no reservation — recorded behavior only: the body renders the `ReservedConcurrentExecutions` member (null) rather than an empty object. (3) TagResource with a tag key in the reserved `aws:` namespace is refused 400 with body `{"Type":"User","message":"One or more of your tags had a problem, please ensure none of your tag names start with \"aws:\", none are empty, and there are no duplicate tags"}` — wholly fixed text, no request values interpolated (Smithy: TagResource binds InvalidParameterValueException, httpError 400, {Type, message}; its success is 204 with an empty body).

Emulator: GetAccountSettings omits HasFunctionWithDeprecatedRuntime; GetFunctionConcurrency returns `{}`; TagResource accepts the `aws:`-prefixed key and answers 204.

Why: a client checking whether any function still runs a deprecated runtime cannot; a client reading ReservedConcurrentExecutions off the response gets a missing key; and a client that relies on Lambda rejecting reserved tag keys writes tags AWS would never store, which then reappear on every ListTags and resource-group query.

Fix: GetAccountSettings' serializer (add the deprecated-runtime flag), GetFunctionConcurrency's serializer (render the unset member), and TagResource's validation (reject keys starting `aws:`, empty keys and duplicates with AWS's message).

Cause 1 of 3: GetAccountSettings, 200 — fixture lambda-account-settings-read step task#1; `HasFunctionWithDeprecatedRuntime` omitted.
Cause 2 of 3: GetFunctionConcurrency, 200 — fixture lambda-get-concurrency-unset-empty step task#2; body is `{}` instead of rendering the `ReservedConcurrentExecutions` member.
Cause 3 of 3: TagResource — fixture lambda-function-tagging-lifecycle step task#7; a tag key in the `aws:` namespace expected 400 with the reserved-tag message, emulator answered 204 with an empty body.

## Evidence
Cluster details: account-settings `$.HasFunctionWithDeprecatedRuntime: missing (expected False, got None)`; concurrency `$: empty_body (expected {'ReservedConcurrentExecutions': None}, got {})`; tagging `status: expected 400, got 204` with `$: empty_body (expected {'Type': 'User', 'message': 'One or more of your tags had a problem, please ensure none of your tag names start with "aws:", none are empty, and there are no duplicate tags'}, got {})`. Smithy: TagResource http code 204, binds InvalidParameterValueException (400); GetAccountSettingsResponse does not model the deprecated-runtime flag (recorded behavior only).

## Steps to Reproduce
Replay the corpus. Fixture lambda-function-tagging-lifecycle step task#7: TagResource on a function with a tag key `aws:something` — expect 400, observe 204. Fixture lambda-account-settings-read step task#1: GetAccountSettings and look for HasFunctionWithDeprecatedRuntime.
