Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.concurrent-update-domain-name-single-winner appsync.domain-api-association-lifecycle appsync.domain-name-api-association-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.concurrent-update-domain-name-single-winner|run#10|AssertionError|assert code == "BadRequestException", (
Issue-Key: appsync.domain-api-association-lifecycle|run#9|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}: {message}"
Issue-Key: appsync.domain-name-api-association-lifecycle|run#15|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}: {message}"
Issue-Commit: 39e54f9f86d0b102f322a2103cd9927458aab9c3

# Implement appsync CreateDomainName: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `39e54f9f86d0b102f322a2103cd9927458aab9c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
`CreateDomainName` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_domain_name.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateDomainName` call, so each one unblocks only when `CreateDomainName` works: `appsync.concurrent-update-domain-name-single-winner`, `appsync.domain-api-association-lifecycle`, `appsync.domain-name-api-association-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_domain_name.rs`. Replaying the corpus, the emulator refused the `CreateDomainName` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_appsync_concurrent_update_domain_name_single_winner.py", line 59, in run
    assert code == "BadRequestException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected BadRequestException rejecting a non-Amazon-issued certificate, got 'NotImplemented' ('operation CreateDomainName is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the appsync emulator
Call CreateDomainName and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
