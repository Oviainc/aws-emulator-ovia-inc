Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: servicediscovery.server-side-computed-health-status-from-route53
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery.server-side-computed-health-status-from-route53|run#14|AssertionError|assert error.get("Code") == "CustomHealthNotFound", (
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Implement servicediscovery UpdateInstanceCustomHealthStatus: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateInstanceCustomHealthStatus

## Description
`UpdateInstanceCustomHealthStatus` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/ops/update_instance_custom_health_status.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateInstanceCustomHealthStatus` call, so each one unblocks only when `UpdateInstanceCustomHealthStatus` works: `servicediscovery.server-side-computed-health-status-from-route53`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/ops/update_instance_custom_health_status.rs`. Replaying the corpus, the emulator refused the `UpdateInstanceCustomHealthStatus` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_servicediscovery_server_side_computed_health_status_from_route53.py", line 67, in run
    assert error.get("Code") == "CustomHealthNotFound", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected CustomHealthNotFound (status isn't client-settable for a managed health check), got {'Message': 'operation UpdateInstanceCustomHealthStatus is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the servicediscovery emulator
Call UpdateInstanceCustomHealthStatus and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
