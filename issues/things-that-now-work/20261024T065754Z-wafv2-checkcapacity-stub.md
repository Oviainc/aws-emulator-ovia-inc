Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.dsw-webacl-capacity-equals-check-capacity-and-sums-per-rule wafv2.rule-group-capacity-mismatch-rejected wafv2.rule-group-reference-capacity-derives-from-the-referenced-groups-own-reported-capacity
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|CheckCapacity|NotImplemented
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Implement wafv2 CheckCapacity: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CheckCapacity

## Description
`CheckCapacity` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/check_capacity.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CheckCapacity` call, so each one unblocks only when `CheckCapacity` works: `wafv2.dsw-webacl-capacity-equals-check-capacity-and-sums-per-rule`, `wafv2.rule-group-capacity-mismatch-rejected`, `wafv2.rule-group-reference-capacity-derives-from-the-referenced-groups-own-reported-capacity`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/check_capacity.rs`. Replaying the corpus, the emulator refused the `CheckCapacity` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CheckCapacity operation: operation CheckCapacity is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the wafv2 emulator
Call CheckCapacity and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
