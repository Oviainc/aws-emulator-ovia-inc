Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.data-protection-policy-unicode-content-preserved sns.put-data-protection-policy-deleted-topic-not-found
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|PutDataProtectionPolicy|NotImplemented
Issue-Key: sns.put-data-protection-policy-deleted-topic-not-found|run#9|AssertionError|assert live_code == "InvalidAction", f"unexpected rejection code: {live_code} ({live_message})"
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# Implement sns PutDataProtectionPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDataProtectionPolicy

## Description
`PutDataProtectionPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/put_data_protection_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `PutDataProtectionPolicy` call, so each one unblocks only when `PutDataProtectionPolicy` works: `sns.data-protection-policy-unicode-content-preserved`, `sns.put-data-protection-policy-deleted-topic-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/put_data_protection_policy.rs`. Replaying the corpus, the emulator refused the `PutDataProtectionPolicy` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sns_data_protection_policy_unicode_content_preserved.py", line 64, in run
    assert code == "InvalidAction", (
           ^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected PutDataProtectionPolicy to be refused with InvalidAction, got NotImplemented (operation PutDataProtectionPolicy is scaffolded but not implemented in this emulator)

## Steps to Reproduce
Boot the sns emulator
Call PutDataProtectionPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
