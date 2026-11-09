Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.gateway-rate-limit-lifecycle bedrock-agentcore-control.gateway-rate-limit-lifecycle-transitions bedrock-agentcore-control.update-gateway-rate-limit-optimistic-lock-conflict
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|UpdateGatewayRateLimit|NotImplemented
Issue-Key: bedrock-agentcore-control.update-gateway-rate-limit-optimistic-lock-conflict|run#15|AssertionError|assert any(ok for ok, _ in results), f"both concurrent updaters were rejected: {results}"
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# Implement bedrock-agentcore-control UpdateGatewayRateLimit: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateGatewayRateLimit

## Description
`UpdateGatewayRateLimit` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/update_gateway_rate_limit.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `UpdateGatewayRateLimit` call, so each one unblocks only when `UpdateGatewayRateLimit` works: `bedrock-agentcore-control.gateway-rate-limit-lifecycle`, `bedrock-agentcore-control.gateway-rate-limit-lifecycle-transitions`, `bedrock-agentcore-control.update-gateway-rate-limit-optimistic-lock-conflict`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/update_gateway_rate_limit.rs`. Replaying the corpus, the emulator refused the `UpdateGatewayRateLimit` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateGatewayRateLimit operation: operation UpdateGatewayRateLimit is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bedrock-agentcore-control emulator
Call UpdateGatewayRateLimit and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
