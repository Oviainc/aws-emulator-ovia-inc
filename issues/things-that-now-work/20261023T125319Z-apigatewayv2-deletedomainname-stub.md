Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.delete-domain-name-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.delete-domain-name-nonexistent-rejected|run#5|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}: {message!r}"
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement apigatewayv2 DeleteDomainName: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDomainName

## Description
`DeleteDomainName` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/delete_domain_name.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteDomainName` call, so each one unblocks only when `DeleteDomainName` works: `apigatewayv2.delete-domain-name-nonexistent-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/delete_domain_name.rs`. Replaying the corpus, the emulator refused the `DeleteDomainName` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_apigatewayv2_delete_domain_name_nonexistent_rejected.py", line 26, in run
    assert code == "NotFoundException", f"expected NotFoundException, got {code!r}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected NotFoundException, got 'NotImplemented': 'operation DeleteDomainName is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the apigatewayv2 emulator
Call DeleteDomainName and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
