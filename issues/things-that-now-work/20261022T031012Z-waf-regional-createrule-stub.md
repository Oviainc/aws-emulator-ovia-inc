Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: waf-regional.delete-referenced-ipset-rejected waf-regional.e2e-ipset-rule-rulegroup-workflow waf-regional.metric-name-whitespace-disallowed-rejected waf-regional.rule-group-activated-rules-roundtrip waf-regional.tag-resource-roundtrip
Issue-Shape-Id: com.amazonaws.wafregional#AWSWAF_Regional_20161128
Issue-Key: waf-regional|CreateRule|NotImplemented
Issue-Key: waf-regional.metric-name-whitespace-disallowed-rejected|run#4|AssertionError|assert err == ("WAFDisallowedNameException", "The specified name is not permitted."), err
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Implement waf-regional CreateRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
`CreateRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-wafregional-awswaf-regional-20161128/src/ops/create_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `CreateRule` call, so each one unblocks only when `CreateRule` works: `waf-regional.delete-referenced-ipset-rejected`, `waf-regional.e2e-ipset-rule-rulegroup-workflow`, `waf-regional.metric-name-whitespace-disallowed-rejected`, `waf-regional.rule-group-activated-rules-roundtrip`, `waf-regional.tag-resource-roundtrip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-wafregional-awswaf-regional-20161128/src/ops/create_rule.rs`. Replaying the corpus, the emulator refused the `CreateRule` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateRule operation: operation CreateRule is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the waf-regional emulator
Call CreateRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
