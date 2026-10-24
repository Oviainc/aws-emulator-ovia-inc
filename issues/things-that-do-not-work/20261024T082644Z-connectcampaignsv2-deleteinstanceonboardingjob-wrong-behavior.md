Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a651a577464eaac3ffa428a0c9cc2aa4b0e9484e

# DeleteInstanceOnboardingJob can never succeed: no onboarding job can reach FAILED, so the delete edge is dead

_Filed: 2026-10-24T08:26:44Z_
_Operation: DeleteInstanceOnboardingJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a651a577464eaac3ffa428a0c9cc2aa4b0e9484e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The delete is gated on the stored job's status being `FAILED` (`state.rs::delete_instance_onboarding` returns `NotFailed` for anything else), but nothing in this emulator can ever put a job into `FAILED`: `StartInstanceOnboardingJob` stores a record whose status is computed purely from a due instant — `IN_PROGRESS` before `ONBOARDING_JOB_DURATION`, `SUCCEEDED` after — and `StoredInstanceOnboarding::to_job_status` never sets `failureCode`. `src/state.rs` says so outright: "no stored job is in the modelled `FAILED` state".

Consequence: for EVERY reachable emulator state, `DELETE /v2/connect-instance/{id}/onboarding` answers either HTTP 404 (`ResourceNotFoundException`, no record) or HTTP 409 (`InvalidStateException`, record exists). There is no request sequence that produces the operation's documented success — a client can never tear down a broken onboarding job and re-onboard the instance, and the modelled `InstanceOnboardingJobStatusCode` value `FAILED` plus the whole `InstanceOnboardingJobFailureCode` catalog (`KMS_KEY_NOT_FOUND`, `KMS_ACCESS_DENIED`, `IAM_ACCESS_DENIED`, `EVENT_BRIDGE_ACCESS_DENIED`, `EVENT_BRIDGE_MANAGED_RULE_LIMIT_EXCEEDED`, `INTERNAL_FAILURE`) are unreachable.

Expected: an onboarding job started with an `encryptionConfig` the service cannot use (enabled=true with a `keyArn` naming no KMS key the caller can use) settles into `FAILED` with the matching `failureCode`, `GetInstanceOnboardingJobStatus` reports that, `GetConnectInstanceConfig` stays 404 for the instance, and `DeleteInstanceOnboardingJob` then answers an empty HTTP 200 with the record removed (a following `GetInstanceOnboardingJobStatus` is 404 and the instance can be onboarded again).

Actual: the job always settles `SUCCEEDED` whatever key it was given, and the delete is always refused with HTTP 409 "Instance onboarding job is not in failed status. Only failed job can be deleted.".

## Evidence

Spec: `DeleteInstanceOnboardingJob` (DELETE /v2/connect-instance/{connectInstanceId}/onboarding) is a modelled operation with an empty output and `InvalidStateException` (409) / `ResourceNotFoundException` (404) — the refusal wording the emulator uses (src/state.rs ONBOARDING_JOB_NOT_FAILED_MESSAGE, quoted as recorded AWS text in the fixed issue 20261023T134440Z-connectcampaignsv2-behavioral-suite-blocked-and-diverging-...) states AWS's rule: "Only failed job can be deleted" — which presupposes jobs that CAN fail. `InstanceOnboardingJobStatusCode` declares FAILED and `InstanceOnboardingJobStatus$failureCode` targets `InstanceOnboardingJobFailureCode` (KMS_KEY_NOT_FOUND / KMS_ACCESS_DENIED / IAM_ACCESS_DENIED / ...), none of which this emulator ever emits. Handler/state: src/ops/delete_instance_onboarding_job.rs:179-220 (maps NoSuchJob->404, NotFailed->409, Deleted->200); src/state.rs::delete_instance_onboarding (requires status_at(now) == ONBOARDING_STATUS_FAILED), `StoredInstanceOnboarding::status_at` (only IN_PROGRESS/SUCCEEDED) and `to_job_status` (failureCode always None). A KMS emulator crate exists for the key-existence check (services/service-com-amazonaws-kms-trentservice), and cross-crate state reads are an established pattern in this repo. Test coverage: tests/all/onboarding_job_deletion.rs asserts ONLY the two refusals (`an_instance_with_no_onboarding_job_cannot_have_one_deleted`, `a_job_that_has_not_failed_is_refused_and_left_untouched`); no test exercises a successful delete, because none is possible.

## Steps to Reproduce

1. Boot the emulator with the connectcampaignsv2 crate loaded.
2. PUT /v2/connect-instance/instance-kms/onboarding with {"encryptionConfig":{"enabled":true,"encryptionType":"KMS","keyArn":"arn:aws:kms:us-east-1:000000000000:key/00000000-0000-0000-0000-000000000000"}} (a key that exists nowhere) -> observe HTTP 200 IN_PROGRESS.
3. Wait past the job span and GET /v2/connect-instance/instance-kms/onboarding -> observe status SUCCEEDED with no failureCode; expected status FAILED with a failureCode from InstanceOnboardingJobFailureCode (e.g. KMS_KEY_NOT_FOUND).
4. DELETE /v2/connect-instance/instance-kms/onboarding -> observe HTTP 409 InvalidStateException; expected an empty HTTP 200 with the job record removed.
5. GET /v2/connect-instance/instance-kms/onboarding -> observe the job still stored; expected HTTP 404 ResourceNotFoundException so the instance can be onboarded afresh.
