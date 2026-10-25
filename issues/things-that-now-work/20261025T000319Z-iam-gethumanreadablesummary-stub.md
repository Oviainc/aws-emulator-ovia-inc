Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.human-readable-summary-unknown-entity-rejected
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.human-readable-summary-unknown-entity-rejected|run#10|AssertionError|assert code == "NoSuchEntity", (
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# Implement iam GetHumanReadableSummary: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHumanReadableSummary

## Description
`GetHumanReadableSummary` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_human_readable_summary.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetHumanReadableSummary` call, so each one unblocks only when `GetHumanReadableSummary` works: `iam.human-readable-summary-unknown-entity-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_human_readable_summary.rs`. Replaying the corpus, the emulator refused the `GetHumanReadableSummary` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_iam_human_readable_summary_unknown_entity_rejected.py", line 41, in run
    assert code == "NoSuchEntity", (
           ^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected NoSuchEntity for a never-created delegation-request EntityArn, got NotImplemented: 'operation GetHumanReadableSummary is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the iam emulator
Call GetHumanReadableSummary and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
