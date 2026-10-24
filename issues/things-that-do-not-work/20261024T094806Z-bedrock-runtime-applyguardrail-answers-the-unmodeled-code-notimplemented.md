Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.apply-guardrail-nonexistent-guardrail-not-found-rejected
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.apply-guardrail-nonexistent-guardrail-not-found-rejected|run#3|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# bedrock-runtime ApplyGuardrail answers the unmodeled code NotImplemented where AWS answers 400 ValidationException for a nonexistent guardrailIdentifier

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApplyGuardrail

## Description
One root cause.

Cause 1 of 1: ApplyGuardrail against a guardrailIdentifier that does not exist is answered with the unmodeled error code NotImplemented instead of a modeled 4xx.
AWS: Smithy (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json) binds ApplyGuardrail to POST /guardrail/{guardrailIdentifier}/version/{guardrailVersion}/apply -> 200, with guardrailIdentifier constrained by pattern ^(|([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))$, and models exactly these errors: AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 400, ServiceUnavailableException 503, ThrottlingException 429, ValidationException 400. 'NotImplemented' is not among them. Recorded behavior: a syntactically valid but nonexistent guardrail identifier is refused with HTTP 400 and code ValidationException.
Emulator: the request is answered with code NotImplemented and the message 'operation ApplyGuardrail is scaffolded but not implemented in this emulator' — an error code absent from the operation's modeled error set, so no SDK models it and client retry/branching logic on ValidationException vs ResourceNotFoundException never fires.
Why: a client that validates its guardrail wiring by probing a bad identifier cannot tell 'this guardrail does not exist' from 'the service is broken'; botocore surfaces an unmodeled ClientError, so typed-exception handling (ValidationException / ResourceNotFoundException) is dead code against the emulator.
Fix: the ApplyGuardrail handler (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/ops/apply_guardrail.rs) and its error mapping — it must resolve guardrailIdentifier/guardrailVersion against guardrail state and raise a modeled 4xx.
Related open issues, neither of which carries this check's replay key: 20261023T232022Z-bedrock-runtime-applyguardrail-stub.md (the scaffold stub itself) and 20261024T031932Z-bedrock-runtime-inline-guardrail-error-taxonomy-applyguardrail-and.md (the same unmodeled-code taxonomy for other guardrail checks). This check is filed separately so the identifier-resolution path has a replay key; close it together with those if one fix covers all three.
Traces: bedrock-runtime.apply-guardrail-nonexistent-guardrail-not-found-rejected.

## Evidence
ApplyGuardrail(guardrailIdentifier=<nonexistent, pattern-valid id>, guardrailVersion='DRAFT', source='INPUT', content=[{'text': {'text': '...'}}])
  Expected (AWS): HTTP 400, Error.Code = 'ValidationException' (ApplyGuardrail models ValidationException 400 and ResourceNotFoundException 404; NotImplemented is not modeled).
  Actual: Error.Code = 'NotImplemented', Error.Message = 'operation ApplyGuardrail is scaffolded but not implemented in this emulator'.
  Suite assertion: AssertionError: expected ValidationException, got 'NotImplemented'

## Steps to Reproduce
1. git_clone, then run_behavioral_suite.
2. Create nothing; call bedrock-runtime.apply_guardrail(guardrailIdentifier='aaaaaaaaaaaa', guardrailVersion='DRAFT', source='INPUT', content=[{'text': {'text': 'hello'}}]).
3. Read the botocore ClientError's Error.Code: the emulator returns 'NotImplemented'; AWS returns a modeled 400 'ValidationException'.
4. Replay key: bedrock-runtime.apply-guardrail-nonexistent-guardrail-not-found-rejected
