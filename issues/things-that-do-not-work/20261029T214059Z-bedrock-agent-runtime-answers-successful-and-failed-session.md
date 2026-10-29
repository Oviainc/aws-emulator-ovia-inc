Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-runtime-invocation-and-step-errors-on-unknown-session
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-runtime-invocation-and-step-errors-on-unknown-session bedrock-agent-runtime-invocation-error-paths bedrock-agent-runtime-session-delete-then-operations-fail bedrock-agent-runtime-session-tag-then-untag-to-empty bedrock-agent-runtime-rerank-inline-text-documents bedrock-agent-runtime-rerank-workflow
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|GetInvocationStep|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|CreateInvocation|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|PutInvocationStep|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|ListTagsForResource|2xx->2xx|$
Issue-Key: bedrock-agent-runtime|Rerank|2xx->2xx|$.results[*].index
Issue-Key: bedrock-agent-runtime|Rerank|2xx->2xx|$.results[*].relevanceScore
Issue-Key: bedrock-agent-runtime|Rerank|4xx->2xx|$.message
Issue-Key: bedrock-agent-runtime|Rerank|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|Rerank|4xx->2xx|status
Issue-Related: 20261029T214059Z-bedrock-agent-runtime-validationexception-constraint-messages-quote-the.md
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# bedrock-agent-runtime answers successful and failed session, tagging and rerank calls with emulator-authored bodies instead of the service's

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInvocationStep

## Description
EIGHT net-new root causes, all reached through replayed bedrock-agent-runtime wire fixtures. All eight share one fault class: the emulator reaches the right handler and (mostly) the right HTTP status, but the BODY it serves is its own — re-worded refusal messages that leak state, an omitted response member, a ranking that is not relevance-driven, and one missing refusal altogether. Smithy model consulted: services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json. The open behavioral issue 20261029T171944Z-bedrock-agent-runtime-ten-net-new-causes-session.md reports overlapping wording faults from a different corpus but carries NONE of the wire replay keys below, so none of these steps are gated by it; Causes 6 and 8 are net-new findings not present there at all.

=== Cause 1 of 8: GetInvocationStep's not-found message is re-worded and discloses which scope missed ===
AWS: Smithy: GetInvocationStep is POST /sessions/{sessionIdentifier}/invocationSteps/{invocationStepId} with required invocationIdentifier, and models ResourceNotFoundException @httpError 404. Recorded behavior: a step that cannot be resolved under the given invocation is refused 404 with Message `Combination of Invocation with ID <invocationIdentifier> and Invcoation Step with ID <invocationStepId> not found`. Fixed text: `Combination of Invocation with ID ` / ` and Invcoation Step with ID ` / ` not found` — including AWS's own misspelling `Invcoation`, which is part of the fixed text and must be reproduced verbatim. Variable parts: the two ids, both from the request. No session identifier appears, and no trailing period. AWS deliberately does not say which of the two scopes missed.
Emulator: 404 with `Invocation step <stepId> was not found in invocation <invocationId>.` — different fixed text, reversed id order, a trailing period, and it names the scope that missed.
Why: a client matching or logging on the refusal text never matches, and turning one opaque refusal into a scope oracle lets a caller enumerate which ids exist under which parent, which the real service does not permit.
Fix: the GetInvocationStep handler's lookup/not-found branch in the bedrock-agent-runtime crate — emit the single AWS-worded refusal for every miss shape.
Traces: bedrock-agent-runtime-invocation-and-step-errors-on-unknown-session#task#4.

=== Cause 2 of 8: CreateInvocation's duplicate-invocationId conflict message is re-worded and leaks the session id ===
AWS: Smithy: CreateInvocation is PUT /sessions/{sessionIdentifier}/invocations/ (@http code 201, @idempotent) with an optional client-supplied invocationId, and models ConflictException @httpError 409. Recorded behavior: re-creating an invocationId that already exists is refused 409 with Message `Invocation with ID <invocationId> already exists` — fixed text `Invocation with ID ` ... ` already exists`, the single variable part being the invocationId from the request; no session identifier and no trailing period.
Emulator: 409 with `Invocation <invocationId> already exists in session <sessionId>.` — different fixed text, an extra state-derived session id, and a trailing period.
Why: a client that branches or alerts on the refusal text to tell a duplicate-invocation replay from other 409s never matches, and the message discloses the session id AWS keeps out of it.
Fix: the CreateInvocation handler's conflict branch in the bedrock-agent-runtime crate.
Traces: bedrock-agent-runtime-invocation-error-paths#task#4.

=== Cause 3 of 8: PutInvocationStep's unknown-invocation message is re-worded and leaks the session id ===
AWS: Smithy: PutInvocationStep is PUT /sessions/{sessionIdentifier}/invocationSteps/ (@http code 201) with required invocationIdentifier, invocationStepTime and payload, and models ResourceNotFoundException @httpError 404. Recorded behavior: writing a step under an invocationIdentifier that does not exist is refused 404 with Message `Invocation with ID <invocationIdentifier> not found` — fixed text `Invocation with ID ` ... ` not found`, the single variable part being the invocation id from the request; no session identifier, no trailing period.
Emulator: 404 with `Invocation <invocationId> was not found in session <sessionId>.` — different fixed text, an extra state-derived session id, and a trailing period.
Why: as Cause 2 — unmatched text plus disclosure of a parent identifier the service's message withholds.
Fix: the PutInvocationStep handler's invocation-resolution branch in the bedrock-agent-runtime crate.
Traces: bedrock-agent-runtime-invocation-error-paths#task#5.

=== Cause 4 of 8: TagResource's not-found refusal uses generic cross-service wording and echoes the ARN ===
AWS: Smithy: TagResource is POST /tags/{resourceArn} with resourceArn -> TaggableResourcesArn (documented 'ARN of Taggable resources: [session]') and models ResourceNotFoundException @httpError 404. Recorded behavior: tagging a well-formed session ARN that names no existing session (for example one whose session has been deleted) is refused 404 with Message `Session resource not found.` — ENTIRELY fixed text, including the trailing period; the ARN is NOT echoed and the resource kind is named in words.
Emulator: 404 with `Resource <resourceArn> was not found. Check the Amazon Resource Name (ARN) and try your request again.` — generic wording borrowed from another service family, naming no resource kind and echoing the whole request ARN.
Why: a client that distinguishes a deleted session from other 404s by the refusal text never matches, and the emulator's message reflects back the full ARN AWS deliberately omits.
Fix: the TagResource handler's resource-resolution branch in the bedrock-agent-runtime crate — raise the session-specific fixed-text refusal.
Traces: bedrock-agent-runtime-session-delete-then-operations-fail#task#4.

=== Cause 5 of 8: ListTagsForResource's not-found refusal uses the same generic wording ===
AWS: Smithy: ListTagsForResource is GET /tags/{resourceArn}, @readonly, and models ResourceNotFoundException @httpError 404. Recorded behavior: reading tags of a session ARN that names no existing session is refused 404 with the same entirely-fixed Message `Session resource not found.`
Emulator: the same generic `Resource <resourceArn> was not found. Check the Amazon Resource Name (ARN) and try your request again.`
Why: as Cause 4, on the read path — so a fixer must correct the refusal on each tagging operation, not only on TagResource.
Fix: the ListTagsForResource handler's resource-resolution branch.
Traces: bedrock-agent-runtime-session-delete-then-operations-fail#task#5.

=== Cause 6 of 8: ListTagsForResource omits the `tags` member entirely when a resource has no tags, instead of returning an empty map ===
AWS: Smithy: ListTagsForResource's output member `tags` -> TagsMap (a map of TagKey to TagValue). Recorded behavior: after every tag key has been removed from a session, a successful ListTagsForResource answers HTTP 200 with the body `{"tags": {}}` — the member is PRESENT and its value is an empty map. (The on-disk TagsMap carries @length(1,200), which would preclude an empty map; the recording is authoritative here, so: recorded behavior only.)
Emulator: HTTP 200 with the body `{}` — the `tags` member is absent altogether.
Why: `tags` is the operation's only payload. An SDK that unmarshals it into a non-optional map, or client code that iterates `response['tags']` without a presence check, sees a missing key rather than an empty collection — so the untag-to-empty path raises a KeyError/None dereference on the emulator while it returns cleanly on AWS. The distinction between 'no tags' and 'member not served' is lost.
Fix: the ListTagsForResource serializer in the bedrock-agent-runtime crate — always serialize `tags`, emitting an empty JSON object when the resource carries none.
Traces: bedrock-agent-runtime-session-tag-then-untag-to-empty#task#5.

=== Cause 7 of 8: Rerank's relevanceScore is not relevance-driven, so results are ordered wrongly and ties are left unbroken ===
AWS: Smithy: Rerank is POST /rerank with required queries (@length 1..1), sources (@length 1..1000) and rerankingConfiguration; output `results` is a required list of RerankResult {index (the original index into the input sources array, required), relevanceScore (required Float)}. Recorded behavior: reranking inline text documents against a text query returns the sources ordered by decreasing semantic relevance to the query, with distinct, sharply-separated scores that decay across the ranking (the strongest match scoring orders of magnitude above the weakest). Both the ORDER of `index` values and the relative separation of `relevanceScore` are the contract; the absolute score values of any one run are not.
Emulator: the ordering disagrees with the recording — positions 2 and 3 of the result list carry each other's source index — and the scores are near-uniform, with two entries emitting the exact same score (an unbroken tie) and the whole range compressed into a narrow band instead of decaying. Hypothesis: the score is a lexical/token-overlap similarity rather than a semantic one, and exact ties are not broken deterministically.
Why: Rerank exists to produce the ordering. A client that reranks retrieved passages and takes the top-k gets different passages than on AWS, so any RAG pipeline built against the emulator grounds its answer in the wrong documents, and the tied scores make the ordering of those entries arbitrary.
Fix: the Rerank handler's scoring function in the bedrock-agent-runtime crate — the relevanceScore computation must rank by query relevance and break ties deterministically rather than emitting equal scores.
Traces: bedrock-agent-runtime-rerank-inline-text-documents#task#1.

=== Cause 8 of 8: Rerank ACCEPTS an invalid reranking model ARN with 200 instead of refusing it 400 ValidationException ===
AWS: Smithy: Rerank's required rerankingConfiguration carries the Bedrock reranking model configuration's modelArn, and Rerank models ValidationException @httpError 400. Recorded behavior: a modelArn that is not a valid Bedrock foundation-model ARN is refused HTTP 400 ValidationException with header `x-amzn-errortype: ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock.agent.runtimeservice/` — the service's own errortype namespace — and the entirely fixed Message `The provided model ARN for reranking is invalid. Please, use a valid model ARN. Also check the model ARN follows this format: arn:<partition>:bedrock:<region>::foundation-model/<model-id>`. The angle-bracket placeholders are literal text in AWS's message; the rejected ARN is not echoed and no member name is named.
Emulator: HTTP 200 with a results body — no `message`, no `x-amzn-errortype`. The invalid model ARN is never validated, so the request succeeds.
Why: this is an overly-permissive accept. A client wiring up the wrong reranker ARN gets plausible-looking rankings from the emulator and a hard 400 the first time it runs against AWS, and no code path exists to exercise the refusal the SDK models.
Fix: the Rerank handler's rerankingConfiguration validation in the bedrock-agent-runtime crate — resolve the reranking modelArn before scoring and raise the fixed-text ValidationException under the bedrock.agent.runtimeservice errortype namespace when it is not a valid foundation-model ARN.
Traces: bedrock-agent-runtime-rerank-workflow#task#1.

## Evidence
Causes 1-6 returned the expected status; the divergence is the body. The uuids/ARNs below are per-run values filling the templates.
Cause 1 — bedrock-agent-runtime-invocation-and-step-errors-on-unknown-session#task#4 (404 matched): $.message expected 'Combination of Invocation with ID c3ee5c14-8a86-447e-8071-1cffd84a5c44 and Invcoation Step with ID 955e08c9-32f9-4968-9f6b-75284efb4264 not found', got 'Invocation step 955e08c9-32f9-4968-9f6b-75284efb4264 was not found in invocation c3ee5c14-8a86-447e-8071-1cffd84a5c44.'
Cause 2 — bedrock-agent-runtime-invocation-error-paths#task#4 (409 matched): expected 'Invocation with ID a3dec590-df3d-4968-b8e7-a88f6c574de0 already exists', got 'Invocation a3dec590-df3d-4968-b8e7-a88f6c574de0 already exists in session af6bf0a0-c8d5-44b7-a283-d99c043bdd73.'
Cause 3 — bedrock-agent-runtime-invocation-error-paths#task#5 (404 matched): expected 'Invocation with ID f1f5874d-8d39-4ed6-a2e9-4abb73a0951b not found', got 'Invocation f1f5874d-8d39-4ed6-a2e9-4abb73a0951b was not found in session af6bf0a0-c8d5-44b7-a283-d99c043bdd73.'
Cause 4 — bedrock-agent-runtime-session-delete-then-operations-fail#task#4 (TagResource, 404 matched): expected 'Session resource not found.', got 'Resource arn:aws:bedrock:us-west-2:000000000000:session/b14d5934-9271-4bca-8499-04dd712fe110 was not found. Check the Amazon Resource Name (ARN) and try your request again.'
Cause 5 — bedrock-agent-runtime-session-delete-then-operations-fail#task#5 (ListTagsForResource, 404 matched): same expected/actual pair as Cause 4.
Cause 6 — bedrock-agent-runtime-session-tag-then-untag-to-empty#task#5 (200 matched): body expected {'tags': {}}, got {} (reported as empty_body at $).
Cause 7 — bedrock-agent-runtime-rerank-inline-text-documents#task#1 (200 matched): $.results[0].relevanceScore expected 0.0261556189507246, got 0.2828427255153656; $.results[1].index expected 2, got 1; $.results[1].relevanceScore expected 0.00033930179779417813, got 0.2828427255153656; $.results[2].index expected 1, got 2; $.results[2].relevanceScore expected 2.541350113460794e-06, got 0.14907120168209076. Note the emulator emits the identical score 0.2828427255153656 for two different sources (an unbroken tie) and compresses the whole range into 0.149..0.283, where the recording decays across four orders of magnitude; results[1] and results[2] are transposed relative to the recording. The absolute score values are not the requirement; the ordering of `index` and the separation of the scores are.
Cause 8 — bedrock-agent-runtime-rerank-workflow#task#1: status expected 400, got 200; $.message expected 'The provided model ARN for reranking is invalid. Please, use a valid model ARN. Also check the model ARN follows this format: arn:<partition>:bedrock:<region>::foundation-model/<model-id>', got None; $headers.x-amzn-errortype expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock.agent.runtimeservice/', got None.

## Steps to Reproduce
Replay the wire corpus simparity-wire-aws-v1; each cause is one step below.
Cause 1: CreateSession; CreateInvocation; then POST /sessions/{sessionIdentifier}/invocationSteps/{invocationStepId} with an invocationIdentifier/invocationStepId combination that was never stored -> inspect the 404 Message. (bedrock-agent-runtime-invocation-and-step-errors-on-unknown-session#task#4)
Cause 2: CreateSession; PUT /sessions/{sid}/invocations/ with invocationId=<uuid>; repeat with the same invocationId -> inspect the 409 ConflictException Message. (bedrock-agent-runtime-invocation-error-paths#task#4)
Cause 3: CreateSession; PUT /sessions/{sid}/invocationSteps/ with an invocationIdentifier that was never created -> inspect the 404 ResourceNotFoundException Message. (bedrock-agent-runtime-invocation-error-paths#task#5)
Cause 4: CreateSession; DeleteSession; POST /tags/<the session ARN> with tags -> inspect the 404 Message. (bedrock-agent-runtime-session-delete-then-operations-fail#task#4)
Cause 5: same setup, then GET /tags/<the session ARN> -> inspect the 404 Message. (bedrock-agent-runtime-session-delete-then-operations-fail#task#5)
Cause 6: CreateSession; TagResource with one or more tags; UntagResource removing every key; GET /tags/<session ARN> -> body must be {"tags": {}}, emulator returns {}. (bedrock-agent-runtime-session-tag-then-untag-to-empty#task#5)
Cause 7: POST /rerank with one text query and the fixture's inline text sources using the Bedrock reranking model configuration -> compare the order of results[].index and the spread of results[].relevanceScore against the recording; the emulator transposes results[1]/results[2] and ties two scores. (bedrock-agent-runtime-rerank-inline-text-documents#task#1)
Cause 8: POST /rerank with rerankingConfiguration whose bedrockRerankingConfiguration.modelConfiguration.modelArn is not a valid foundation-model ARN -> expect 400 ValidationException with the fixed 'The provided model ARN for reranking is invalid...' Message; emulator answers 200 with results. (bedrock-agent-runtime-rerank-workflow#task#1)
