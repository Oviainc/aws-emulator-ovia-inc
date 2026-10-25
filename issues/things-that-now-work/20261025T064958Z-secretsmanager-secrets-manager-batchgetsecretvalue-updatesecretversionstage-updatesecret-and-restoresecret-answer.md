Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.batch-get-secret-value-duplicate-or-more-than-twenty-ids-rejected secretsmanager.batch-get-secret-value-filters-form-denied-without-list-secrets-permission secretsmanager.update-secret-version-stage-move-awscurrent-without-moveto-rejected secretsmanager.concurrent-update-secret-version-stage-conflicting-move secretsmanager.staging-label-cap-refuses-the-twenty-first-label-on-a-version secretsmanager.secret-scheduled-for-deletion-refuses-tag-policy-and-stage-changes secretsmanager.delete-secret-force-immediate secretsmanager.restore-nonexistent-secret-fails
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.batch-get-secret-value-duplicate-or-more-than-twenty-ids-rejected|run#6|AssertionError|assert code == "InvalidRequestException", f"a duplicate id must be refused, got {code!r}"
Issue-Key: secretsmanager.batch-get-secret-value-filters-form-denied-without-list-secrets-permission|_poll#20|Timeout|time.sleep(interval_s)
Issue-Key: secretsmanager.update-secret-version-stage-move-awscurrent-without-moveto-rejected|run#10|AssertionError|assert code == "InvalidParameterException", f"unexpected rejection code: {code}"
Issue-Key: secretsmanager.concurrent-update-secret-version-stage-conflicting-move|run#13|AssertionError|assert "ok" in results, "codes:" + ",".join(results)
Issue-Key: secretsmanager.staging-label-cap-refuses-the-twenty-first-label-on-a-version|run#16|AssertionError|assert refused_code in _REFUSAL_CODES, (
Issue-Key: secretsmanager.secret-scheduled-for-deletion-refuses-tag-policy-and-stage-changes|run#7|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: secretsmanager.delete-secret-force-immediate|run#9|AssertionError|assert mutate_code == "ResourceNotFoundException", (
Issue-Key: secretsmanager.restore-nonexistent-secret-fails|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Commit: a1aff8283ff7a4c3e09b44355afd91a57915fcd2

# Secrets Manager: BatchGetSecretValue, UpdateSecretVersionStage, UpdateSecret and RestoreSecret answer 501 NotImplemented, so staging-label, update and restore behaviour is unreachable (4 causes)

> _Note: this issue was identified against the code base at commit `a1aff8283ff7a4c3e09b44355afd91a57915fcd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetSecretValue, UpdateSecretVersionStage, UpdateSecret, RestoreSecret

## Description
Four Secrets Manager operations are still scaffold stubs that answer HTTP 501 with error code `NotImplemented` for every request. Eight behavioral checks in this suite were graded `mismatch` (not `error`) because the emulator returns a well-formed modelled-looking error envelope whose code is `NotImplemented`, so the suite's already-filed stub clusters did not pick them up and no open issue carries their replay keys. The four operations themselves are named by the open issues `20261024T190718Z-secretsmanager-batchgetsecretvalue-stub.md`, `20261024T190718Z-secretsmanager-updatesecretversionstage-stub.md`, `20261024T190718Z-secretsmanager-updatesecret-stub.md` and `20261024T190718Z-secretsmanager-restoresecret-stub.md`; this issue exists to attach the eight unowned behavioral checks below to those causes so a fix is actually gated on them.

=== Cause 1 of 4: BatchGetSecretValue answers 501 NotImplemented ===
AWS: Smithy model `com.amazonaws.secretsmanager#BatchGetSecretValue` — input `SecretIdList` (list, @length min 1 max 20), `Filters` (list, @length max 10), `MaxResults` (@range 1..20); output `SecretValues` + `Errors` (APIErrorListType) + `NextToken`; modelled errors `InvalidParameterException`, `InvalidRequestException`, `ResourceNotFoundException`, `InvalidNextTokenException`, `DecryptionFailure`, `InternalServiceError` (all HTTP 400 except InternalServiceError 500). The recorded behaviour is that request-level problems fail the whole call — a repeated SecretId is refused with `InvalidRequestException` (recorded message example: "Duplicate secret ID encountered: <name>. You cannot enter duplicate secrets in the secret ID list."), a list longer than 20 with `InvalidParameterException` (example: "The secret ID list has too many IDs. You can include up to 20 secret IDs."), and a request giving neither `SecretIdList` nor `Filters` with `InvalidParameterException` (example: "You must specify a list of Secret IDs or a filter value."), while a *missing* secret inside an otherwise valid list is a per-entry `Errors` item with `ErrorCode` `ResourceNotFoundException` inside a 200. The `Filters` form additionally requires `secretsmanager:ListSecrets` on the caller and is refused with `AccessDeniedException` without it, whereas the `SecretIdList` form is not.
Emulator: every BatchGetSecretValue call is refused with HTTP 501 / code `NotImplemented`. The duplicate-id check asserts `InvalidRequestException` and observes `'NotImplemented'`; the Filters/permission check never gets a single successful batch read, so its convergence poll never settles and the check is killed by the 60s harness timeout.
Why: a batched config loader cannot read any secret at all, and it cannot distinguish a request-level refusal (duplicate / over-long / empty request) from a per-entry not-found, so retry and fan-out logic built on the documented 200-with-Errors contract has nothing to exercise.
Fix: implement the operation handler `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/batch_get_secret_value.rs` (currently carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Scaffolded::not_implemented`), including its request-level validation (duplicate detection, 20-id cap, SecretIdList-or-Filters requirement) and the per-entry `Errors` channel, plus the ListSecrets authorization gate on the Filters form.
Traces: secretsmanager.batch-get-secret-value-duplicate-or-more-than-twenty-ids-rejected, secretsmanager.batch-get-secret-value-filters-form-denied-without-list-secrets-permission.

=== Cause 2 of 4: UpdateSecretVersionStage answers 501 NotImplemented ===
AWS: Smithy model `com.amazonaws.secretsmanager#UpdateSecretVersionStage` — input `SecretId` (required), `VersionStage` (required), `MoveToVersionId`, `RemoveFromVersionId`; output `ARN`/`Name`; modelled errors `InvalidParameterException`, `InvalidRequestException`, `LimitExceededException`, `ResourceNotFoundException` (HTTP 400) and `InternalServiceError` (500). The model documents that if the staging label is already attached to a different version then `RemoveFromVersionId` must be supplied and must match the current holder, or the operation fails. Recorded behaviour: (a) moving `AWSCURRENT` with `MoveToVersionId` but no `RemoveFromVersionId` is refused with `InvalidParameterException` and nothing changes (message built from the request, naming the parameter and the version that currently holds the label — recorded example: "The parameter RemoveFromVersionId can't be empty. Staging label AWSCURRENT is currently attached to version <v0>, so you must explicitly reference that version in RemoveFromVersionId."); (b) two concurrent same-source moves of `AWSCURRENT` resolve with exactly one winner and the loser refused with `InvalidRequestException`/`InvalidParameterException`, never a dual-AWSCURRENT state; (c) the staging-label quota is enforced on custom labels only (`AWSCURRENT` does not consume a slot) and the over-cap attachment is refused with `LimitExceededException`; (d) while a secret is scheduled for deletion the call is refused with `InvalidRequestException` (recorded example message: "You can't perform this operation on the secret because it was marked for deletion.").
Emulator: every call is refused with HTTP 501 / `NotImplemented`, so each of the four graded outcomes above reports `NotImplemented` instead of the modelled code. Note that in the scheduled-for-deletion check the TagResource / UntagResource / PutResourcePolicy / GetResourcePolicy gates already behave correctly; only the UpdateSecretVersionStage leg diverges.
Why: staging labels are the whole rotation protocol. A client can never promote `AWSPENDING` to `AWSCURRENT`, never observe the stale-`RemoveFromVersionId` refusal it is required to handle, and never sees the quota or marked-for-deletion refusal — so rotation code and label-management tooling validate against nothing.
Fix: implement the operation handler `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/update_secret_version_stage.rs` and the version/staging-label state it mutates, including the RemoveFromVersionId precondition, the single-owner-per-label invariant under concurrency, the custom-label cap, and the scheduled-for-deletion state gate.
Traces: secretsmanager.update-secret-version-stage-move-awscurrent-without-moveto-rejected, secretsmanager.concurrent-update-secret-version-stage-conflicting-move, secretsmanager.staging-label-cap-refuses-the-twenty-first-label-on-a-version, secretsmanager.secret-scheduled-for-deletion-refuses-tag-policy-and-stage-changes.

=== Cause 3 of 4: UpdateSecret answers 501 NotImplemented ===
AWS: Smithy model `com.amazonaws.secretsmanager#UpdateSecret` — input `SecretId` (required), `Description`, `KmsKeyId`, `SecretString`/`SecretBinary` (@length 1..65536, sensitive); output `ARN`/`Name`/`VersionId`; modelled errors include `ResourceNotFoundException` (HTTP 400), documented as "Secrets Manager can't find the resource that you asked for". Recorded behaviour: after `DeleteSecret` with `ForceDeleteWithoutRecovery=true`, a mutation addressed to the dead id is refused with `ResourceNotFoundException` (recorded message example: "Secrets Manager can't find the specified secret.") — the same specific code a read gets, not a generic error.
Emulator: `UpdateSecret` on the force-deleted id is refused with HTTP 501 / `NotImplemented`. The preceding legs of that check pass (the force-delete does take effect and `DescribeSecret` correctly reports `ResourceNotFoundException`), so the divergence is solely that the mutation path is a stub.
Why: a caller cannot change a secret's description, KMS key or value at all, and cleanup/reconciliation code that keys off `ResourceNotFoundException` on a mutation to detect an already-deleted secret instead sees an unmodelled 501 and treats it as a server fault worth retrying.
Fix: implement the operation handler `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/update_secret.rs`, including lookup of the SecretId and the `ResourceNotFoundException` refusal for an absent or force-deleted secret.
Traces: secretsmanager.delete-secret-force-immediate.

=== Cause 4 of 4: RestoreSecret answers 501 NotImplemented ===
AWS: Smithy model `com.amazonaws.secretsmanager#RestoreSecret` — input `SecretId` (required, @length 1..2048); output `ARN`/`Name`; modelled errors `ResourceNotFoundException`, `InvalidParameterException`, `InvalidRequestException` (HTTP 400) and `InternalServiceError` (500). Recorded behaviour: `RestoreSecret` on a SecretId that was never created is refused with `ResourceNotFoundException` (recorded message example: "Secrets Manager can't find the specified secret.").
Emulator: the call is refused with HTTP 501 / `NotImplemented` instead of the modelled 400 `ResourceNotFoundException`.
Why: a scheduled deletion can never be undone, and a client probing restorability cannot tell "no such secret" from a server fault, so it retries a refusal that will never change.
Fix: implement the operation handler `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/restore_secret.rs`, including the not-found refusal and clearing of the secret's `DeletedDate` on success.
Traces: secretsmanager.restore-nonexistent-secret-fails.

## Evidence
Suite: simparity-behavioral-ga-prod-v1, 53/225 passed. The eight checks below were reported as failures with NO "ALREADY FILED" and NO "OWNED by" marker, i.e. no open issue carries their replay key.

Cause 1 — BatchGetSecretValue:
- secretsmanager.batch-get-secret-value-duplicate-or-more-than-twenty-ids-rejected [mismatch]
  Expected: code == "InvalidRequestException" for BatchGetSecretValue(SecretIdList=[name, name])
  Actual:   AssertionError: a duplicate id must be refused, got 'NotImplemented'
- secretsmanager.batch-get-secret-value-filters-form-denied-without-list-secrets-permission [mismatch]
  Expected: BatchGetSecretValue(SecretIdList=[arn]) to succeed three times in a row, then the Filters form to be refused with AccessDeniedException
  Actual:   "Check timed out after 60.0s" — the convergence poll never observes a successful batch read because every call is refused NotImplemented

Cause 2 — UpdateSecretVersionStage:
- secretsmanager.update-secret-version-stage-move-awscurrent-without-moveto-rejected [mismatch]
  Expected: code == "InvalidParameterException"
  Actual:   AssertionError: unexpected rejection code: NotImplemented
- secretsmanager.concurrent-update-secret-version-stage-conflicting-move [mismatch]
  Expected: exactly one of two racing moves returns ok, the loser InvalidRequestException/InvalidParameterException
  Actual:   AssertionError: codes:NotImplemented,NotImplemented
- secretsmanager.staging-label-cap-refuses-the-twenty-first-label-on-a-version [mismatch]
  Expected: the over-cap attachment refused with LimitExceededException
  Actual:   AssertionError: ... must be refused with one of ['LimitExceededException'] ...; got 'NotImplemented'
- secretsmanager.secret-scheduled-for-deletion-refuses-tag-policy-and-stage-changes [mismatch]
  Expected: code == "InvalidRequestException" for UpdateSecretVersionStage on a secret with RecoveryWindowInDays=7 pending
  Actual:   AssertionError: UpdateSecretVersionStage on a secret scheduled for deletion must be refused, got 'NotImplemented' (the TagResource / UntagResource / PutResourcePolicy / GetResourcePolicy legs of the same check passed)

Cause 3 — UpdateSecret:
- secretsmanager.delete-secret-force-immediate [mismatch]
  Expected: UpdateSecret(SecretId=<force-deleted name>, Description=...) -> ResourceNotFoundException
  Actual:   AssertionError: expected ResourceNotFoundException, got NotImplemented

Cause 4 — RestoreSecret:
- secretsmanager.restore-nonexistent-secret-fails [mismatch]
  Expected: RestoreSecret(SecretId=<never created>) -> ResourceNotFoundException
  Actual:   AssertionError: expected ResourceNotFoundException, got NotImplemented

Source confirmation in the checkout: all four handler files still carry the generator marker "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`" and end in `Err(crate::scaffolded::Scaffolded::not_implemented("<Op>"))`:
- services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/batch_get_secret_value.rs
- services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/update_secret_version_stage.rs (lines 4, 195)
- services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/update_secret.rs
- services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/restore_secret.rs

## Steps to Reproduce
1. git_clone() the repo and boot the emulator.
2. Replay the eight checks listed in trace_ids with the behavioral suite.
3. Or by hand against the emulator endpoint, with a secretsmanager client:
   a. CreateSecret(Name=n, SecretString="v"); then BatchGetSecretValue(SecretIdList=[n, n]) — observe HTTP 501 / Error.Code "NotImplemented" instead of HTTP 400 / "InvalidRequestException". Repeat with 21 ids (expect 400 InvalidParameterException) and with no arguments at all (expect 400 InvalidParameterException).
   b. PutSecretValue(SecretId=n, ClientRequestToken=v1, SecretString="v1", VersionStages=["AWSPENDING"]); then UpdateSecretVersionStage(SecretId=n, VersionStage="AWSCURRENT", MoveToVersionId=v1) with no RemoveFromVersionId — observe 501 "NotImplemented" instead of 400 "InvalidParameterException".
   c. DeleteSecret(SecretId=n, RecoveryWindowInDays=7); then UpdateSecretVersionStage(SecretId=n, VersionStage="blue", MoveToVersionId=<v0>) — observe 501 "NotImplemented" instead of 400 "InvalidRequestException".
   d. DeleteSecret(SecretId=n, ForceDeleteWithoutRecovery=true); wait for DescribeSecret to report ResourceNotFoundException; then UpdateSecret(SecretId=n, Description="post-mortem") — observe 501 "NotImplemented" instead of 400 "ResourceNotFoundException".
   e. RestoreSecret(SecretId=<a name never created>) — observe 501 "NotImplemented" instead of 400 "ResourceNotFoundException".
