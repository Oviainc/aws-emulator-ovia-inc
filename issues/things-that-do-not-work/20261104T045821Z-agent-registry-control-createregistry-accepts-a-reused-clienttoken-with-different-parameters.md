Issue-Source: trace-sweep-divergence
Issue-Trace: agent-registry-control-registry-idempotency-and-not-ready-conflict
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: agent-registry-control-registry-idempotency-and-not-ready-conflict
Issue-Shape-Id: com.amazonaws.agentregistrycontrol#AgentRegistryControl
Issue-Key: agent-registry-control|CreateRegistry|4xx->2xx|$.message
Issue-Key: agent-registry-control|CreateRegistry|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: agent-registry-control|CreateRegistry|4xx->2xx|status
Issue-Commit: 024100b485faef6e8a7d69224b42c4d112173d44

# CreateRegistry accepts a reused clientToken with different parameters instead of ConflictException

> _Note: this issue was identified against the code base at commit `024100b485faef6e8a7d69224b42c4d112173d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRegistry

## Description
1 cause.

Cause 1 of 1 - idempotency-token reuse with mismatched request parameters is not detected (fixture agent-registry-control-registry-idempotency-and-not-ready-conflict, step task#3).
AWS: Smithy: CreateRegistry POST /registries, success code 202; input member clientToken -> ClientToken carries @idempotencyToken (@length 33..256, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$); modeled errors include ConflictException (httpError 409, member message -> NonBlankString). A repeat CreateRegistry with the SAME clientToken and the SAME parameters is idempotent and replays the original 202 result, but a repeat with the same clientToken and DIFFERENT parameters is refused. Recorded behavior: HTTP 409 with the x-amzn-errortype header identifying the modeled error and message "Idempotency token already used with different request parameters" - that text is entirely fixed, with no request- or state-derived substitution.
Emulator: answers 202 and creates a second registry. No error is produced at all: the response carries no $.message and no x-amzn-errortype header, so the emulator keeps no record of which parameters a clientToken was first used with (or does not compare them).
Why: the idempotency contract is inverted exactly where it protects callers. A client retrying a create after a timeout, but with a drifted request body, is told its create succeeded and silently ends up with a duplicate registry instead of the 409 that tells it its token is already bound to a different request - and the retry can no longer be distinguished from a genuine first call.
Fix: clientToken bookkeeping in the CreateRegistry handler of the agentregistrycontrol service crate - persist the token together with a fingerprint of the request parameters in service state, replay the stored result on an exact match, and raise ConflictException (409) with the message above when the token matches but the parameters do not.
Traces: agent-registry-control-registry-idempotency-and-not-ready-conflict#task#3.

## Evidence
Replay of simparity-wire-aws-v1, cluster service=agent-registry-control|CreateRegistry|status=409->202, fixture agent-registry-control-registry-idempotency-and-not-ready-conflict step task#3:
  status: expected 409, got 202
  $.message: missing (expected 'Idempotency token already used with different request parameters', got None)
  $headers.x-amzn-errortype: missing (expected the modeled error type, got None)
Model grounding: CreateRegistry.clientToken is @idempotencyToken; ConflictException is a modeled CreateRegistry error with httpError 409 and a `message` member.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Call CreateRegistry with some clientToken and a set of parameters; it returns 202.
3. Call CreateRegistry again with the SAME clientToken but different parameters (e.g. a different name/description) - fixture agent-registry-control-registry-idempotency-and-not-ready-conflict, step task#3.
4. Observe 202 plus a second registry; AWS returns 409 ConflictException with message 'Idempotency token already used with different request parameters' and the x-amzn-errortype header set to the modeled error.
