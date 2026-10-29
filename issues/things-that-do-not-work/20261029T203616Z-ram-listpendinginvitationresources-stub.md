Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ram.list-pending-invitation-resources-unknown-invitation-rejected
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram.list-pending-invitation-resources-unknown-invitation-rejected|run#4|AssertionError|assert code == "ResourceShareInvitationArnNotFoundException", (
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# Implement ram ListPendingInvitationResources: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPendingInvitationResources

## Description
`ListPendingInvitationResources` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/list_pending_invitation_resources.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListPendingInvitationResources` call, so each one unblocks only when `ListPendingInvitationResources` works: `ram.list-pending-invitation-resources-unknown-invitation-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/list_pending_invitation_resources.rs`. Replaying the corpus, the emulator refused the `ListPendingInvitationResources` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_ram_list_pending_invitation_resources_unknown_invitation_rejected.py", line 50, in run
    assert code == "ResourceShareInvitationArnNotFoundException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceShareInvitationArnNotFoundException, got NotImplemented: 'operation ListPendingInvitationResources is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the ram emulator
Call ListPendingInvitationResources and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
