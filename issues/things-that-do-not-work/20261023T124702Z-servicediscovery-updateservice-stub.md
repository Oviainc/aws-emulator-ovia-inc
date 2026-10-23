Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: servicediscovery.service-dns-record-type-change-rejected
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery.service-dns-record-type-change-rejected|run#10|AssertionError|assert error.get("Code") == "InvalidInput", f"expected InvalidInput for a DnsConfig record Type change, got {error!r}"
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement servicediscovery UpdateService: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
`UpdateService` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/ops/update_service.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateService` call, so each one unblocks only when `UpdateService` works: `servicediscovery.service-dns-record-type-change-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/ops/update_service.rs`. Replaying the corpus, the emulator refused the `UpdateService` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_servicediscovery_service_dns_record_type_change_rejected.py", line 37, in run
    assert error.get("Code") == "InvalidInput", f"expected InvalidInput for a DnsConfig record Type change, got {error!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidInput for a DnsConfig record Type change, got {'Message': 'operation UpdateService is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the servicediscovery emulator
Call UpdateService and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
