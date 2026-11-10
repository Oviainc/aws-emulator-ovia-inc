Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.create-delivery-missing-iam-permission-denied
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.create-delivery-missing-iam-permission-denied|run#14|AssertionError|assert code == denied, f"expected {denied!r}, got {code}"
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement logs CreateDelivery: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDelivery

## Description
`CreateDelivery` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_delivery.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateDelivery` call, so each one unblocks only when `CreateDelivery` works: `logs.create-delivery-missing-iam-permission-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_delivery.rs`. Replaying the corpus, the emulator refused the `CreateDelivery` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_logs_create_delivery_missing_iam_permission_denied.py", line 56, in run
    assert code == denied, f"expected {denied!r}, got {code}"
           ^^^^^^^^^^^^^^
AssertionError: expected ('AccessDeniedException', 'User: arn:aws:sts::000000000000:assumed-role/sim2real-behav-cdmp-role-089cfb05153e/sim2real-behav-cdmp-session is not authorized to perform: logs:CreateDelivery on resource: arn:aws:logs:us-west-2:000000000000:delivery-source:sim2real-behav-cdmp-src-3362c55e6a63 with an explicit deny in an identity-based policy'), got ('NotImplemented', 'operation CreateDelivery is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the logs emulator
Call CreateDelivery and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
