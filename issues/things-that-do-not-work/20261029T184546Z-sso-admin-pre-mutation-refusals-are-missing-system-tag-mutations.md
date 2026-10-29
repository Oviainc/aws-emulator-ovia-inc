Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-tagging-error-paths-r2
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-tagging-error-paths-r2 sso-admin-region-read-and-error-paths-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|TagResource|4xx->2xx|$
Issue-Key: sso-admin|TagResource|4xx->2xx|status
Issue-Key: sso-admin|UntagResource|4xx->2xx|$
Issue-Key: sso-admin|UntagResource|4xx->2xx|status
Issue-Key: sso-admin|AddRegion|4xx->4xx|$.Message
Issue-Key: sso-admin|AddRegion|4xx->4xx|$.__type
Issue-Key: sso-admin|RemoveRegion|4xx->4xx|$.Message
Issue-Commit: 5dbb2ab27a0402c002f5157a9a56e61b81adc0a2

# Pre-mutation refusals are missing: system-tag mutations succeed and account-instance region operations raise the wrong error

> _Note: this issue was identified against the code base at commit `5dbb2ab27a0402c002f5157a9a56e61b81adc0a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
FOUR causes sharing one fault: the guard AWS applies BEFORE it mutates - on who the caller is, and on what kind of instance this is - is not modelled, so the emulator either succeeds or reports the wrong refusal. A fixer must close all four before submitting.

Cause 1 of 4 - TagResource accepts a system-tag mutation instead of refusing it (fixture sso-admin-tagging-error-paths-r2, step task#3).
AWS: when the caller is an end user and the request's Tags include a system (service-owned) tag key, TagResource is refused with HTTP 400 and the modelled ValidationException (Smithy: TagResource errors include ValidationException, errorKind client, httpError 400; __type "ValidationException"), Message "Caller is an end user and not allowed to mutate system tags" - that text is wholly fixed, it interpolates nothing from the request. Recorded behavior only: the model documents TagResource as "Associates a set of tags with a specified resource" and does not state the rule. No tag is written.
Emulator: HTTP 200 with an empty body {} - the mutation is applied and no error is raised.
Why: a client (or a test) that relies on AWS refusing a system-tag write sees it succeed, and the emulator's tag state then contains a tag AWS would never have stored, so every later ListTagsForResource diverges too.
Fix: TagResource validation, evaluated before the state mutation (reject system/service-owned tag keys for an end-user caller with ValidationException).
Traces: sso-admin-tagging-error-paths-r2#task#3.

Cause 2 of 4 - UntagResource likewise accepts a system-tag removal (fixture sso-admin-tagging-error-paths-r2, step task#4).
AWS: HTTP 400, __type "ValidationException", Message "Caller is an end user and not allowed to mutate system tags" (same fixed text, no variable parts) when an end-user caller names a system tag key in TagKeys (Smithy: UntagResource errors include ValidationException, httpError 400). Recorded behavior only for the text.
Emulator: HTTP 200 with an empty body {}; the key is removed.
Why: same as Cause 1, in the removal direction - a protected tag disappears from the emulator's state.
Fix: UntagResource validation, same guard as Cause 1 - it belongs in one shared helper rather than per handler.
Traces: sso-admin-tagging-error-paths-r2#task#4.

Cause 3 of 4 - AddRegion reports a region conflict where AWS refuses the operation for an account instance (fixture sso-admin-region-read-and-error-paths-r2, step task#3).
AWS: AddRegion against an account instance of IAM Identity Center is refused with HTTP 400, __type "ValidationException", Message "This operation is not supported for account instances of IAM Identity Center." - fixed text, nothing interpolated. That instance-type guard runs BEFORE any check on whether the Region is already enabled. Recorded behavior only: the model lists both ValidationException (httpError 400) and ConflictException (httpError 409) on AddRegion and documents only the replication workflow.
Emulator: HTTP 400 with __type "ConflictException" and Message "Instance <instance-arn> is already enabled in Region <region-name>" - it skipped the instance-type guard and ran the already-enabled check instead (the arn and region in that message are request/state values; the wrong part is the error class and the rule). Secondary observation at the same site: the emulator serves ConflictException with HTTP 400 although the model gives ConflictException httpError 409.
Why: a client branching on __type retries a ConflictException (the model calls it a propagation race, retryable with backoff) forever, where AWS told it the operation is simply unsupported for this instance type.
Fix: the AddRegion handler's guard ordering/validation - classify account instances first and refuse with ValidationException; and serve ConflictException with its modelled 409.
Traces: sso-admin-region-read-and-error-paths-r2#task#3.

Cause 4 of 4 - RemoveRegion returns the right error class with the wrong rule and message (fixture sso-admin-region-read-and-error-paths-r2, step task#4).
AWS: RemoveRegion against an account instance is refused with HTTP 400, __type "ValidationException", Message "This operation is not supported for account instances of IAM Identity Center." (fixed text). Like Cause 3, the instance-type guard precedes the primary-Region check. Recorded behavior only.
Emulator: HTTP 400 ValidationException (class matched) but Message "RegionName <region-name> is the primary Region of instance <instance-arn> and cannot be removed" - the primary-Region rule, which AWS never reaches for this instance type.
Why: a client or operator reading the message is told to pick a different Region, when no Region can be removed from an account instance at all.
Fix: the RemoveRegion handler's guard ordering - same account-instance check as Cause 3, shared with AddRegion.
Traces: sso-admin-region-read-and-error-paths-r2#task#4.

No open sso-admin issue covers these: 20261028T173235Z-sso-admin-listtagsforresource-validation.md is about serving tags for a CREATE_IN_PROGRESS instance (a read path), not about refusing system-tag mutations or region operations on account instances.

## Evidence
Cause 1 - TagResource, fixture sso-admin-tagging-error-paths-r2 step task#3:
  status: expected 400, got 200
  body: expected {'__type': 'ValidationException', 'Message': 'Caller is an end user and not allowed to mutate system tags'}, got {}

Cause 2 - UntagResource, fixture sso-admin-tagging-error-paths-r2 step task#4:
  status: expected 400, got 200
  body: expected {'__type': 'ValidationException', 'Message': 'Caller is an end user and not allowed to mutate system tags'}, got {}

Cause 3 - AddRegion, fixture sso-admin-region-read-and-error-paths-r2 step task#3, expected 400 / actual 400:
  $.__type: expected 'ValidationException', got 'ConflictException'
  $.Message: expected 'This operation is not supported for account instances of IAM Identity Center.', got 'Instance arn:aws:sso:::instance/ssoins-7223b1d1ac5e1f0e is already enabled in Region us-west-2' (the arn/region are per-run values filling the emulator's own template)

Cause 4 - RemoveRegion, fixture sso-admin-region-read-and-error-paths-r2 step task#4, expected 400 / actual 400:
  $.Message: expected 'This operation is not supported for account instances of IAM Identity Center.', got 'RegionName us-west-2 is the primary Region of instance arn:aws:sso:::instance/ssoins-7223b1d1ac5e1f0e and cannot be removed'

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture sso-admin-tagging-error-paths-r2, step task#3 - TagResource naming a system tag key as an end-user caller. Observe 200 {} instead of 400 ValidationException 'Caller is an end user and not allowed to mutate system tags'.
3. Cause 2: replay the same fixture, step task#4 - UntagResource naming a system tag key. Observe 200 {} instead of the same 400 ValidationException.
4. Cause 3: replay fixture sso-admin-region-read-and-error-paths-r2, step task#3 - AddRegion on the account instance. Observe __type ConflictException with the already-enabled message instead of 400 ValidationException 'This operation is not supported for account instances of IAM Identity Center.'
5. Cause 4: replay the same fixture, step task#4 - RemoveRegion on the account instance. Observe the primary-Region message instead of the account-instance refusal.
