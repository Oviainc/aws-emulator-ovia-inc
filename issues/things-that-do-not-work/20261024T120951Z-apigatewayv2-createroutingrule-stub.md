Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.create-routing-rule-nonexistent-api-not-found
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.create-routing-rule-nonexistent-api-not-found|run#8|AssertionError|assert code == "NotFoundException", (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Implement apigatewayv2 CreateRoutingRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoutingRule

## Description
`CreateRoutingRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_routing_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateRoutingRule` call, so each one unblocks only when `CreateRoutingRule` works: `apigatewayv2.create-routing-rule-nonexistent-api-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_routing_rule.rs`. Replaying the corpus, the emulator refused the `CreateRoutingRule` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_apigatewayv2_create_routing_rule_nonexistent_api_not_found.py", line 38, in run
    assert code == "NotFoundException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected NotFoundException, got 'NotImplemented': 'operation CreateRoutingRule is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the apigatewayv2 emulator
Call CreateRoutingRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
