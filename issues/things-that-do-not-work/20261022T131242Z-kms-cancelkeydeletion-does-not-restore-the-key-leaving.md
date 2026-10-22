Issue-Source: trace-sweep-divergence
Issue-Trace: kms-cancel-key-deletion-restores-key
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-cancel-key-deletion-restores-key kms-disable-schedule-cancel-recover kms-schedule-and-cancel-key-deletion kms-update-description-blocked-during-pending-deletion kms-key-description-update-and-cancel-deletion
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|DescribeKey|2xx->2xx|$.KeyMetadata.KeyState
Issue-Key: kms|EnableKey|2xx->4xx|$
Issue-Key: kms|EnableKey|2xx->4xx|status
Issue-Key: kms|DescribeKey|2xx->2xx|$.KeyMetadata.Enabled
Issue-Key: kms|ScheduleKeyDeletion|2xx->4xx|status
Issue-Key: kms|DescribeKey|2xx->2xx|$.KeyMetadata.Description
Issue-Key: kms|ScheduleKeyDeletion|2xx->4xx|$.DeletionDate
Issue-Key: kms|ScheduleKeyDeletion|2xx->4xx|$.KeyId
Issue-Key: kms|ScheduleKeyDeletion|2xx->4xx|$.KeyState
Issue-Key: kms|ScheduleKeyDeletion|2xx->4xx|$.PendingWindowInDays
Issue-Commit: 6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1

# KMS CancelKeyDeletion does not restore the key, leaving it stuck in PendingDeletion

> _Note: this issue was identified against the code base at commit `6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelKeyDeletion

## Description
Six replay causes, all rooted in the same defect: CancelKeyDeletion answers successfully but never clears the key's PendingDeletion state, so every subsequent operation on that key behaves as if the cancellation never happened. The emulator's CancelKeyDeletion handler appears to return the modelled CancelKeyDeletionResponse without mutating the stored key record (no clearing of the deletion date, no transition back to Disabled).

Cause 1 of 6 — DescribeKey after CancelKeyDeletion reports the wrong KeyState.
AWS: ScheduleKeyDeletion moves a key to KeyState PendingDeletion; CancelKeyDeletion (Smithy: CancelKeyDeletion, input KeyId required, output CancelKeyDeletionResponse{KeyId}) reverses that and the key returns to the Disabled state, because a cancelled key is never re-enabled automatically. A following DescribeKey must therefore report KeyMetadata.KeyState = "Disabled" (Smithy: DescribeKey -> KeyMetadata.KeyState, enum KeyState).
Emulator: DescribeKey answers 200 but with KeyMetadata.KeyState = "PendingDeletion".
Why: a client that cancels a pending deletion and then polls DescribeKey for the key to leave PendingDeletion never observes the transition; recovery workflows hang or abort, and callers that gate on KeyState will treat a live key as doomed.
Fix: the CancelKeyDeletion handler / key-state store — the cancellation must write the key back to Disabled and drop the pending deletion date, so DescribeKey projects the restored state.
Traces: kms-cancel-key-deletion-restores-key#task#4 (expected KeyState 'Disabled', got 'PendingDeletion').

Cause 2 of 6 — EnableKey on a cancelled key is refused as invalid-state.
AWS: after CancelKeyDeletion the key is Disabled, and EnableKey on a Disabled key succeeds with HTTP 200 and an empty body (Smithy: EnableKey, output Unit; KMSInvalidStateException is modelled but applies only while the key really is in an unusable state such as PendingDeletion).
Emulator: EnableKey answers 409 with {"__type": "KMSInvalidStateException", "message": "<key arn> is not in a valid state for this operation. Its key state is PendingDeletion."} — the status and the error code are both wrong for this request, because the emulator still believes the key is pending deletion. (The recorded message text is quoted only as an example.)
Why: the client cannot re-enable a key it just rescued; an error is raised where AWS returns success, so the whole recover-a-key flow fails closed.
Fix: same key-state store as Cause 1; EnableKey's state precondition then passes without change.
Traces: kms-cancel-key-deletion-restores-key#task#5 (expected 200 {}, got 409 KMSInvalidStateException).

Cause 3 of 6 — DescribeKey after EnableKey still reports a disabled, pending-deletion key.
AWS: once EnableKey has succeeded, DescribeKey reports KeyMetadata.Enabled = true and KeyMetadata.KeyState = "Enabled".
Emulator: DescribeKey answers 200 with KeyMetadata.Enabled = false and KeyMetadata.KeyState = "PendingDeletion".
Why: the key never becomes usable for Encrypt/Decrypt/Sign from the client's point of view, even though the sequence AWS requires was executed and acknowledged.
Fix: as Cause 1 and 2 — the enable transition must be applied to a key that cancellation actually restored.
Traces: kms-cancel-key-deletion-restores-key#task#6 (expected Enabled true / KeyState 'Enabled', got false / 'PendingDeletion').

Cause 4 of 6 — ScheduleKeyDeletion on a restored key is refused.
AWS: a key that was restored by CancelKeyDeletion can be scheduled for deletion again; ScheduleKeyDeletion answers HTTP 200 with ScheduleKeyDeletionResponse{KeyId, DeletionDate, KeyState, PendingWindowInDays} (Smithy: ScheduleKeyDeletion).
Emulator: ScheduleKeyDeletion answers 409 (KMSInvalidStateException), because the emulator never left PendingDeletion.
Why: idempotent teardown of a rescued key is impossible — the caller gets a hard client error on an operation AWS accepts, so test and cleanup automation cannot converge.
Fix: same restore-on-cancel defect; ScheduleKeyDeletion's own precondition is correct.
Traces: kms-cancel-key-deletion-restores-key#teardown#7 (expected 200, got 409).

Cause 5 of 6 — the restored-state poll never converges, and the key's description is also stale.
AWS: after CancelKeyDeletion the key reports KeyState "Disabled" and carries whatever description the last UpdateKeyDescription set.
Emulator: DescribeKey answers 200 with KeyState "PendingDeletion" for 20 consecutive polls and with the original creation-time description instead of the updated one (the stale-description half is the same defect filed separately for UpdateKeyDescription).
Why: a client polling for the documented post-cancel state loops until it times out; combined with the stale description it cannot tell the key apart from one it never touched.
Fix: the key-state store's cancel transition (and, for the description half, UpdateKeyDescription's write path).
Traces: kms-key-description-update-and-cancel-deletion#task#11 (expected KeyState 'Disabled' + updated description, got 'PendingDeletion' + original description, poll exhausted after 20 attempts).

Cause 6 of 6 — ScheduleKeyDeletion returns an error body instead of the deletion response.
AWS: ScheduleKeyDeletion on a restored key answers 200 and the body carries KeyId, DeletionDate, KeyState = "PendingDeletion" and the echoed PendingWindowInDays.
Emulator: 409, and the whole response body is absent — KeyId, DeletionDate, KeyState and PendingWindowInDays are all missing.
Why: the client gets neither the new deletion date nor the window it must honour, so it cannot report or schedule around the deletion.
Fix: as above — once cancellation restores the key, the handler reaches its success serializer.
Traces: kms-key-description-update-and-cancel-deletion#teardown#12 (expected 200 with KeyId/DeletionDate/KeyState/PendingWindowInDays, got 409 with none of them).

No open issue covers this; the only open kms issue is the TagResource NotImplemented one (20261022T081518Z-kms-implement-kms-tagresource...), which is unrelated. state_reset_failures was 0 for this run, so these are not cross-fixture state leaks.

## Evidence
Replay of trace set simparity-wire-aws-v1 (64 fixtures, 475 steps, score 37.1%, state_reset_failures=0). Six divergence clusters share this root cause:
- kms-cancel-key-deletion-restores-key#task#4 DescribeKey 200/200: $.KeyMetadata.KeyState expected 'Disabled', got 'PendingDeletion' (also kms-disable-schedule-cancel-recover#task#7, kms-schedule-and-cancel-key-deletion#task#5).
- kms-cancel-key-deletion-restores-key#task#5 EnableKey: expected 200 with body {}, got 409 {'__type': 'KMSInvalidStateException', 'message': '<key arn> is not in a valid state for this operation. Its key state is PendingDeletion.'} (also kms-schedule-and-cancel-key-deletion#task#6).
- kms-cancel-key-deletion-restores-key#task#6 DescribeKey 200/200: $.KeyMetadata.Enabled expected True got False; $.KeyMetadata.KeyState expected 'Enabled' got 'PendingDeletion' (also kms-schedule-and-cancel-key-deletion#task#7).
- kms-cancel-key-deletion-restores-key#teardown#7 ScheduleKeyDeletion: expected 200, got 409 (also kms-disable-schedule-cancel-recover#teardown#8, kms-update-description-blocked-during-pending-deletion#teardown#7).
- kms-key-description-update-and-cancel-deletion#task#11 DescribeKey 200/200: $.KeyMetadata.KeyState expected 'Disabled' got 'PendingDeletion' after 20 poll attempts; $.KeyMetadata.Description expected the updated value, got the original (also kms-update-description-blocked-during-pending-deletion#task#6).
- kms-key-description-update-and-cancel-deletion#teardown#12 ScheduleKeyDeletion: expected 200, got 409; $.KeyId, $.DeletionDate, $.KeyState ('PendingDeletion'), $.PendingWindowInDays (7) all missing (also kms-schedule-and-cancel-key-deletion#teardown#8).
Smithy (com-amazonaws-kms-trentservice): CancelKeyDeletion{KeyId required} -> CancelKeyDeletionResponse{KeyId}; EnableKey -> Unit; ScheduleKeyDeletion -> {KeyId, DeletionDate, KeyState, PendingWindowInDays}; DescribeKey -> KeyMetadata{Enabled, KeyState, Description}.

## Steps to Reproduce
1. Boot the emulator from the mainline checkout and replay trace set simparity-wire-aws-v1.
2. Minimal manual sequence: CreateKey -> DisableKey -> ScheduleKeyDeletion(PendingWindowInDays=7) -> CancelKeyDeletion -> DescribeKey.
3. DescribeKey reports KeyMetadata.KeyState 'PendingDeletion' instead of 'Disabled' (Cause 1).
4. Call EnableKey on the same key: 409 KMSInvalidStateException instead of 200 (Cause 2); a following DescribeKey still shows Enabled=false / 'PendingDeletion' (Cause 3).
5. Call ScheduleKeyDeletion on the same key again: 409 instead of 200 with KeyId/DeletionDate/KeyState/PendingWindowInDays (Causes 4 and 6).
6. Replay kms-cancel-key-deletion-restores-key and kms-key-description-update-and-cancel-deletion to confirm the fix; the second fixture's task#11 poll must reach KeyState 'Disabled'.
