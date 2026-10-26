Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1a78f0cc1d943875c45c11a36243ccaf94b7f40f

# Refused instance mutations leave no audit record in DeleteConnectInstanceConfig and DeleteInstanceOnboardingJob, unlike every sibling instance mutator

_Filed: 2026-10-25T21:22:53Z_
_Operation: DeleteConnectInstanceConfig | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1a78f0cc1d943875c45c11a36243ccaf94b7f40f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::audit_onboarding_change` is documented as the one place this service records a state-changing Connect-instance call together with HOW IT ENDED, and every other instance mutator in the crate calls it on its refusal paths as well as on success (naming the outcome: "ValidationException", "ConflictException", "ResourceNotFoundException").

Two handlers break that: `DeleteConnectInstanceConfig` writes an audit record only when the delete succeeds — its ValidationException path (off-catalog `campaignDeletionPolicy`) and its ResourceNotFoundException path return without calling the helper — and `DeleteInstanceOnboardingJob` likewise audits only the `Deleted` arm, leaving the `NoSuchJob` (404) and `NotFailed` (409 InvalidStateException) arms unaudited. The result is an audit trail in which a refused attempt to delete an instance configuration or an onboarding job is invisible, while a refused put/integration/onboarding attempt on the same instance is recorded — so the same "who tried to change this instance, and how did it end" question is answered inconsistently depending on which operation was called.

## Evidence

The helper's own contract, src/state.rs:829-836:
```
/// Record one state-changing Connect-instance operation — onboarding a instance, or
/// configuring one: who asked (the access key id the request presented), which
/// Connect instance it named and how the call ended.
///
/// The one place this service writes an instance audit record, so a later instance
/// mutator records the same fields.
```

Siblings audit their refusals:
- src/ops/start_instance_onboarding_job.rs:217-224 — `if let Err(refusal) = check_encryption_config(...) { crate::state::audit_onboarding_change(ctx.parts(), Self::NAME, &input.connect_instance_id, "ValidationException"); return Err(refusal); }`
- src/ops/put_connect_instance_integration.rs:199-217 — not-onboarded path calls `audit_onboarding_change(... "ResourceNotFoundException")` before returning 404; src/ops/put_connect_instance_integration.rs:233-238 audits `"ConflictException"`.
- src/ops/delete_connect_instance_integration.rs:237-242 — `audit_onboarding_change(... "ResourceNotFoundException")` on the "no integration under that identifier" refusal.
- src/ops/put_instance_communication_limits.rs:218-227 — `audit_onboarding_change(..., if stored { "accepted" } else { "ResourceNotFoundException" })`.

The two that skip it:
- src/ops/delete_connect_instance_config.rs:193-204 returns `ValidationException` with no `audit_onboarding_change` call, and src/ops/delete_connect_instance_config.rs:240-251 returns `ResourceNotFoundException` with none either; the only audit call is the success one at src/ops/delete_connect_instance_config.rs:256-261 (`"accepted"`), whose own comment claims it "leaves the same audit record the other instance mutators leave".
- src/ops/delete_instance_onboarding_job.rs:193-198 audits `"accepted"` for `DeleteOnboardingOutcome::Deleted`, while the `NoSuchJob` arm (src/ops/delete_instance_onboarding_job.rs:201-212) and the `NotFailed` arm (src/ops/delete_instance_onboarding_job.rs:215-223) return their errors with no audit record.

Test coverage: tests/all/connect_instance_config_deletion.rs and tests/all/onboarding_job_deletion.rs exercise both refusal paths over the wire; neither asserts on the audit record, so nothing pins the current (missing) behaviour.

## Steps to Reproduce

1. Call `DELETE /v2/connect-instance/{id}/config?campaignDeletionPolicy=NOPE` for an instance that is not onboarded, and `PUT /v2/connect-instance/{id}/communication-limits` for the same instance.
2. Observe the tracing output: the `PutInstanceCommunicationLimits` refusal emits an `audit = true` record with outcome `ResourceNotFoundException` (put_instance_communication_limits.rs:218-227), while the `DeleteConnectInstanceConfig` refusals emit nothing (delete_connect_instance_config.rs:193-204, 240-251).
3. Repeat with `DELETE /v2/connect-instance/{id}/onboarding` against an instance whose job is IN_PROGRESS/SUCCEEDED: the 409 `InvalidStateException` (delete_instance_onboarding_job.rs:215-223) leaves no audit record, while `DeleteConnectInstanceIntegration`'s 404 (delete_connect_instance_integration.rs:237-242) does.
