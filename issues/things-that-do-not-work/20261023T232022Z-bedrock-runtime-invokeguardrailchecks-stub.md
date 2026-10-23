Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.guardrail-checks-empty-check-config-rejected bedrock-runtime.guardrail-checks-offsets-locate-entity-in-source-text bedrock-runtime.guardrail-checks-results-mirror-requested-check-types
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime|InvokeGuardrailChecks|NotImplemented
Issue-Commit: cd7a0e788c437c5439a918b59f215cef2324a883

# Implement bedrock-runtime InvokeGuardrailChecks: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `cd7a0e788c437c5439a918b59f215cef2324a883`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeGuardrailChecks

## Description
`InvokeGuardrailChecks` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/ops/invoke_guardrail_checks.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `InvokeGuardrailChecks` call, so each one unblocks only when `InvokeGuardrailChecks` works: `bedrock-runtime.guardrail-checks-empty-check-config-rejected`, `bedrock-runtime.guardrail-checks-offsets-locate-entity-in-source-text`, `bedrock-runtime.guardrail-checks-results-mirror-requested-check-types`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/ops/invoke_guardrail_checks.rs`. Replaying the corpus, the emulator refused the `InvokeGuardrailChecks` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the InvokeGuardrailChecks operation: operation InvokeGuardrailChecks is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bedrock-runtime emulator
Call InvokeGuardrailChecks and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
