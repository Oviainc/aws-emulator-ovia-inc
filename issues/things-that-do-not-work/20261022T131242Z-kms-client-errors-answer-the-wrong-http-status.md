Issue-Source: trace-sweep-divergence
Issue-Trace: kms-error-paths-invalid-state
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-error-paths-invalid-state kms-grant-and-key-error-paths kms-key-description-update-and-cancel-deletion kms-symmetric-encrypt-decrypt kms-alias-lifecycle
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|ScheduleKeyDeletion|4xx->4xx|$.message
Issue-Key: kms|ScheduleKeyDeletion|4xx->4xx|status
Issue-Key: kms|DisableKey|4xx->4xx|$.message
Issue-Key: kms|DisableKey|4xx->4xx|status
Issue-Key: kms|Encrypt|4xx->4xx|status
Issue-Key: kms|DescribeKey|4xx->4xx|$.message
Issue-Key: kms|DescribeKey|4xx->4xx|status
Issue-Commit: 6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1

# KMS client errors answer the wrong HTTP status: 409 and 404 where the wire returns 400

> _Note: this issue was identified against the code base at commit `6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ScheduleKeyDeletion

## Description
Four replay causes, one root defect: the emulator's KMS error serializer takes the HTTP status from each error shape's modelled @httpError trait (KMSInvalidStateException = 409, NotFoundException = 404) instead of the status KMS actually puts on the wire. Recorded responses show KMS answering HTTP 400 for these client errors while carrying the same __type in the body, which is what an awsJson1_1 service does. The error codes the emulator emits are right; only the status line is wrong, consistently, across every error path exercised.

Cause 1 of 4 — ScheduleKeyDeletion on a key already pending deletion.
AWS: recorded behavior — the response is HTTP 400 with __type KMSInvalidStateException (message in the recording: '<key arn> is pending deletion.'). Smithy models ScheduleKeyDeletion's KMSInvalidStateException with @httpError 409, which the recorded wire does not use; the recorded status is authoritative. The message text is an example, not a requirement.
Emulator: HTTP 409, __type KMSInvalidStateException.
Why: clients and proxies that classify by status treat 409 as a conflict to resolve rather than a 4xx client fault; non-SDK callers and anything with status-based retry/alarm rules diverge from real KMS.
Fix: the KMS error serializer / status mapping for the service, not the individual handler.
Traces: kms-error-paths-invalid-state#task#5 (expected 400, got 409).

Cause 2 of 4 — DisableKey on a key pending deletion.
AWS: recorded behavior — HTTP 400 with __type KMSInvalidStateException (Smithy: DisableKey errors include KMSInvalidStateException @httpError 409, again unused on the wire).
Emulator: HTTP 409 with the same error code.
Why: as Cause 1 — the error is correctly identified but mis-statused, so status-driven client logic takes the wrong branch.
Fix: same error-status mapping.
Traces: kms-error-paths-invalid-state#task#6 (expected 400, got 409).

Cause 3 of 4 — Encrypt against a key in an unusable state.
AWS: recorded behavior — HTTP 400 for the invalid-state rejection of Encrypt (Smithy: Encrypt errors KMSInvalidStateException @httpError 409, DisabledException @httpError 409).
Emulator: HTTP 409.
Why: a data-plane caller that distinguishes 400 (fix the request) from 409 (retry after state settles) will retry forever against a key that will never become usable.
Fix: same error-status mapping; it covers the data-plane operations too.
Traces: kms-symmetric-encrypt-decrypt#task#9 (expected 400, got 409).

Cause 4 of 4 — DescribeKey for an alias that no longer resolves.
AWS: recorded behavior — HTTP 400 with __type NotFoundException (Smithy: DescribeKey error NotFoundException @httpError 404, unused on the wire). The message is built from the request and names the alias the caller passed, resolved to its alias ARN ('Alias arn:aws:kms:<region>:<account>:alias/<name> is not found.' in the recording); the emulator instead reports the lookup in key terms ('KMS cannot find the key with identifier alias/<name> in <region>.'). The status is the requirement here, the alias-naming rule is the secondary one, and the exact wording is only an example.
Emulator: HTTP 404 with the correct NotFoundException code but a key-identifier-shaped message for an alias lookup.
Why: alias-resolution failures are indistinguishable from transport-level 404s to a client, and the message does not tell the caller which alias failed in the form AWS uses.
Fix: the same error-status mapping, plus the alias lookup path in DescribeKey so a failed alias resolution reports the alias.
Traces: kms-alias-lifecycle#task#11 (expected 400, got 404), covering kms-error-paths-invalid-state#task#2 and kms-grant-and-key-error-paths#task#1.

No open issue covers this. state_reset_failures was 0, so these are genuine per-step responses.

## Evidence
Replay of trace set simparity-wire-aws-v1 (64 fixtures, 475 steps, score 37.1%, state_reset_failures=0). Four divergence clusters, all status-only (error codes matched):
- kms-error-paths-invalid-state#task#5 ScheduleKeyDeletion: expected 400, got 409; $.message expected '<key arn> is pending deletion.', got '<key arn> is not in a valid state for this operation. Its key state is PendingDeletion.' (also kms-grant-and-key-error-paths#task#6, kms-key-description-update-and-cancel-deletion#task#9).
- kms-error-paths-invalid-state#task#6 DisableKey: expected 400, got 409, same message divergence.
- kms-symmetric-encrypt-decrypt#task#9 Encrypt: expected 400, got 409.
- kms-alias-lifecycle#task#11 DescribeKey: expected 400, got 404; $.message expected 'Alias arn:aws:kms:us-west-2:000000000000:alias/<name> is not found.', got 'KMS cannot find the key with identifier alias/<name> in us-west-2.' (also kms-error-paths-invalid-state#task#2, kms-grant-and-key-error-paths#task#1).
Smithy (com-amazonaws-kms-trentservice): KMSInvalidStateException @httpError 409 and NotFoundException @httpError 404 on ScheduleKeyDeletion/DisableKey/Encrypt/DescribeKey — the modelled statuses the emulator is emitting, which the recorded wire replaces with 400.

## Steps to Reproduce
1. Boot the emulator from the mainline checkout and replay trace set simparity-wire-aws-v1.
2. CreateKey, then ScheduleKeyDeletion on it, then call ScheduleKeyDeletion again and DisableKey on the same key: both answer HTTP 409 instead of 400 with __type KMSInvalidStateException (Causes 1 and 2).
3. Call Encrypt against that key: HTTP 409 instead of 400 (Cause 3).
4. Call DescribeKey with KeyId set to an alias that does not exist: HTTP 404 instead of 400, with a message describing a key identifier rather than the alias ARN (Cause 4).
5. Replay kms-error-paths-invalid-state, kms-symmetric-encrypt-decrypt and kms-alias-lifecycle to confirm the fix.
