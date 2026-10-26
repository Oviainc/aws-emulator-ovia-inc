Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime.rerank-permutes-one-source-list-per-query-not-per-document
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime.rerank-permutes-one-source-list-per-query-not-per-document|run#30|AssertionError|assert many_message == "1 validation error detected: Value at 'queries' failed to satisfy constraint: Member must have length less than or equal to 1", (
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# Rerank: list-length ValidationException message injects a synthetic "'N items'" value token where AWS emits no value for a collection member

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Rerank

## Description
One cause.

Cause 1 of 1: the boundary constraint validator renders a COLLECTION (list/map) @length violation with the same `Value '<x>' at '<path>'` shape it uses for scalars, substituting a synthesized stand-in for the value (`'2 items'`). AWS's validation-message grammar omits the value entirely for a collection member, so the two messages differ by that injected token. Failing check: bedrock-agent-runtime.rerank-permutes-one-source-list-per-query-not-per-document.

AWS: Smithy (services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json) models RerankRequest.queries -> RerankQueriesList, type list, @required, @length(min 1, max 1), and the list AND its member are @sensitive; Rerank models ValidationException (HTTP 400). The recorded ground truth (check blessed 2026-09-17, replayed twice against real AWS in us-west-2) shows AWS refuses two `queries` entries with HTTP 400 ValidationException whose message follows AWS's standard framework-validation grammar for a COLLECTION member — the value is NOT rendered:
  "1 validation error detected: Value at 'queries' failed to satisfy constraint: Member must have length less than or equal to 1"
The rule is known and request-derived: for a scalar constraint AWS emits `Value '<submitted value>' at '<member path>' failed to satisfy constraint: <requirement>`; for a list/map length constraint it emits `Value at '<member path>' failed to satisfy constraint: Member must have length less than or equal to <max>`, with no value clause. The same check confirms the emulator already gets the scalar form exactly right in the same request family (numberOfResults=0 -> "1 validation error detected: Value '0' at 'rerankingConfiguration.bedrockRerankingConfiguration.numberOfResults' failed to satisfy constraint: Member must have value greater than or equal to 1" passes), so this is specifically the collection branch of the renderer. Hypothesis: AWS omits the value for collections because there is no scalar rendering for a list (and here the shape is @sensitive besides).

Emulator: the status (400) and the error code (ValidationException) match, but the message is
  "1 validation error detected: Value '2 items' at 'queries' failed to satisfy constraint: Member must have length less than or equal to 1"
i.e. the validator fabricates a value clause of the form `Value '<count> items'` for the list where AWS renders no value clause at all. The constraint path ('queries'), the constraint wording ("Member must have length less than or equal to 1") and the error count prefix are already correct — only the injected `'N items'` token diverges.

Why: a client that keys on the validation message — a request-shaping SDK wrapper, a test asserting the service's refusal text, or a UI that surfaces AWS's validator string verbatim — sees a message AWS never produces for any list- or map-length violation. Because the renderer is the shared boundary validator, every operation of the service (and any service using it) that bounds a list or map by @length emits the same fabricated clause, so the divergence is wide rather than specific to Rerank. The fabricated clause also implies a value was echoed for a shape the model marks @sensitive, which is the opposite of what the sensitivity trait asks for.

Fix: the shared boundary constraint-validation message renderer invoked before the handler runs (the deserialize -> validate step of the centralized request walk, not the Rerank handler — crate::ops::rerank only builds cross-member refusals of its own and never sees this one). Give the collection (list/map) @length branch its own rendering that drops the `Value '<...>' ` clause and starts the clause at `Value at '<path>' failed to satisfy constraint: ...`, leaving the scalar branch (which is already correct) unchanged.

Traces: bedrock-agent-runtime.rerank-permutes-one-source-list-per-query-not-per-document (hypothesis H6, check line 241).

## Evidence
Check: bedrock-agent-runtime.rerank-permutes-one-source-list-per-query-not-per-document (verdict: mismatch).

Request: Rerank with two entries in `queries` (RerankQueriesList @length max 1), sent through a botocore client built with Config(parameter_validation=False) so the service's own refusal is observed rather than the SDK's local one.

Expected (recorded from real AWS, us-west-2, blessed replay x2):
  HTTP 400, Error.Code = "ValidationException"
  Error.Message = "1 validation error detected: Value at 'queries' failed to satisfy constraint: Member must have length less than or equal to 1"

Actual (emulator):
  HTTP 400, Error.Code = "ValidationException"  <- matches
  Error.Message = "1 validation error detected: Value '2 items' at 'queries' failed to satisfy constraint: Member must have length less than or equal to 1"

AssertionError raised at check_bedrock_agent_runtime_rerank_permutes_one_source_list_per_query_not_per_document.py line 216/241:
  "expected the exact ValidationException message AWS returns, got \"1 validation error detected: Value '2 items' at 'queries' failed to satisfy constraint: Member must have length less than or equal to 1\""

Control in the same check (passes, showing the scalar branch is correct): numberOfResults=0 -> "1 validation error detected: Value '0' at 'rerankingConfiguration.bedrockRerankingConfiguration.numberOfResults' failed to satisfy constraint: Member must have value greater than or equal to 1".

Model evidence (smithy_query shape=RerankQueriesList,RerankRequest): RerankQueriesList = {type: list, length: {min: 1, max: 1}, sensitive: true, member target RerankQuery (sensitive: true)}; RerankRequest.queries required.

## Steps to Reproduce
1. Build a bedrock-agent-runtime client against the emulator with botocore Config(parameter_validation=False) so the SDK does not refuse the request locally.
2. Call Rerank with a valid rerankingConfiguration (type=BEDROCK_RERANKING_MODEL, bedrockRerankingConfiguration.modelConfiguration.modelArn=amazon.rerank-v1:0), one or more INLINE TEXT sources, and TWO entries in `queries`:
   queries=[{"type":"TEXT","textQuery":{"text":"What is the capital of France?"}},{"type":"TEXT","textQuery":{"text":"Which fruit is high in potassium?"}}]
3. Catch the ClientError and read response["Error"]["Code"] and response["Error"]["Message"].
4. Observe Code == "ValidationException" (correct) but Message contains the injected value clause "Value '2 items' at 'queries'" where AWS emits "Value at 'queries'".
5. Re-replay the whole check to confirm: bedrock-agent-runtime.rerank-permutes-one-source-list-per-query-not-per-document.
