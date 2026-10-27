Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.update-reputation-entity-customer-managed-status-stale-conflict-rejected sesv2.update-reputation-entity-policy-concurrent-writes-land-one-of-two
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|GetReputationEntity|NotImplemented
Issue-Commit: 388872f1987f74fe9682288e62ee8e18a6dfb8b9

# Implement sesv2 GetReputationEntity: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `388872f1987f74fe9682288e62ee8e18a6dfb8b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetReputationEntity

## Description
`GetReputationEntity` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_reputation_entity.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `GetReputationEntity` call, so each one unblocks only when `GetReputationEntity` works: `sesv2.update-reputation-entity-customer-managed-status-stale-conflict-rejected`, `sesv2.update-reputation-entity-policy-concurrent-writes-land-one-of-two`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_reputation_entity.rs`. Replaying the corpus, the emulator refused the `GetReputationEntity` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetReputationEntity operation: operation GetReputationEntity is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sesv2 emulator
Call GetReputationEntity and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
