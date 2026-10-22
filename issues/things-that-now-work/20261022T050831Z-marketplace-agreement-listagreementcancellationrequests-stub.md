Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-agreement.list-agreement-cancellation-requests-invalid-party-type-rejected marketplace-agreement.list-agreement-cancellation-requests-maxresults-respected
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement.list-agreement-cancellation-requests-invalid-party-type-rejected|run#4|AssertionError|assert bad == want, f"expected {want!r}, got {bad!r}"
Issue-Key: marketplace-agreement|ListAgreementCancellationRequests|NotImplemented
Issue-Commit: 7545d0b8b2e4fb4a41c801fcbb48b523234ffec8

# Implement marketplace-agreement ListAgreementCancellationRequests: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7545d0b8b2e4fb4a41c801fcbb48b523234ffec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAgreementCancellationRequests

## Description
`ListAgreementCancellationRequests` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/ops/list_agreement_cancellation_requests.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListAgreementCancellationRequests` call, so each one unblocks only when `ListAgreementCancellationRequests` works: `marketplace-agreement.list-agreement-cancellation-requests-invalid-party-type-rejected`, `marketplace-agreement.list-agreement-cancellation-requests-maxresults-respected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/ops/list_agreement_cancellation_requests.rs`. Replaying the corpus, the emulator refused the `ListAgreementCancellationRequests` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_marketplace_agreement_list_agreement_cancellation_requests_invalid_party_type_rejected.py", line 26, in run
    assert bad == want, f"expected {want!r}, got {bad!r}"
           ^^^^^^^^^^^
AssertionError: expected ('ValidationException', 'Provided party type is invalid.'), got ('NotImplemented', 'operation ListAgreementCancellationRequests is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the marketplace-agreement emulator
Call ListAgreementCancellationRequests and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
