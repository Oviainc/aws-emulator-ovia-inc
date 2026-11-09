Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-runtime-knowledge-base-retrieve-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-runtime-knowledge-base-retrieve-not-found bedrock-agent-runtime-retrieve-from-knowledge-base
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|Retrieve|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|Retrieve|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|Retrieve|4xx->4xx|status
Issue-Commit: dd62406f8379c1f6ec445456da967cb3a28bbbbc

# bedrock-agent-runtime: Retrieve's unknown-knowledge-base refusal is re-worded and pre-empted by an unmodeled userContext rejection

> _Note: this issue was identified against the code base at commit `dd62406f8379c1f6ec445456da967cb3a28bbbbc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Retrieve

## Description
Two net-new causes (2 causes), both on bedrock-agent-runtime Retrieve (POST /knowledgebases/{knowledgeBaseId}/retrieve). They share one fault: the emulator's Retrieve entry path does not produce the service's ResourceNotFoundException refusal for a knowledge base that does not exist — in one case it serves that refusal with emulator-authored wording, in the other it never reaches the existence check at all because it rejects a modeled optional input member first. A fixer must close BOTH before submitting.

Related open issues that do NOT cover these: 20261109T023107Z-bedrock-agent-runtime-retrieve-refuses-every-opensearch-serverless.md covers the 424 DependencyFailedException refusal of an OPENSEARCH_SERVERLESS-backed knowledge base; 20261029T171944Z-bedrock-agent-runtime-retrieve-stub.md and 20261023T004328Z-bedrock-agent-runtime-retrieve-optimizeprompt-getagentmemory-deleteagentmemory-and.md cover the earlier NotImplemented/stub symptom. Neither carries the replay key of the two steps below, and neither describes the message wording or the userContext rejection.

=== Cause 1 of 2: the ResourceNotFoundException message for an unknown knowledgeBaseId is re-worded ===
AWS: Smithy (services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model): Retrieve = POST /knowledgebases/{knowledgeBaseId}/retrieve, @readonly, success 200; knowledgeBaseId is an @required @httpLabel KnowledgeBaseIdentifier (pattern ^[0-9a-zA-Z]{10}$|^arn:aws(-[^:]+)?:bedrock:...:knowledge-base/[0-9a-zA-Z]{10}$); modeled errors include ResourceNotFoundException (client, httpError 404, doc "The specified resource Amazon Resource Name (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again."). A well-formed knowledgeBaseId that names no knowledge base is refused with HTTP 404, x-amzn-errortype ResourceNotFoundException, and the recorded message "Knowledge Base with id <knowledgeBaseId> does not exist". The requirement is that template: the fixed text is "Knowledge Base with id " ... " does not exist" and the only variable part is the knowledgeBaseId taken verbatim from the request path (the recording's KBBD995DA3 is one filled-in value, not part of the requirement).
Emulator: answers HTTP 404 with x-amzn-errortype ResourceNotFoundException but the message "Could not find knowledge base <knowledgeBaseId>." — different fixed text, different word order, a trailing period, and lower-cased "knowledge base".
Why: Bedrock RAG clients and test suites match on the refusal text (and SDK/CLI users read it) to tell "knowledge base not created / wrong id" apart from other 404s; a re-worded message makes those assertions and log-scrapers fail against the emulator while passing against AWS, and the message a user is shown is not the one AWS emits.
Fix: the Retrieve handler's not-found refusal in the bedrock-agent-runtime crate (the operation's handler/error construction for the unknown-knowledge-base branch) must emit the service's ResourceNotFoundException template, formatting the request's knowledgeBaseId into it.
Traces: bedrock-agent-runtime-knowledge-base-retrieve-not-found#task#1.

=== Cause 2 of 2: a Retrieve carrying the modeled userContext member is refused 400 ValidationException instead of being served (and so the unknown-knowledge-base 404 is never reached) ===
AWS: Smithy: RetrieveRequest has an OPTIONAL member userContext -> UserContext (@sensitive structure with @required userId), documented "Contains information about the user making the request. Use this to pass user identity information for access control filtering, so that retrieval results only include documents the user is authorized to access." It is a modeled, accepted input member; ValidationException (httpError 400, doc "Input validation failed. Check your request parameters and retry the request.") is for inputs that violate the model's constraints, not for a member the model defines. In the recording the same call — a well-formed knowledgeBaseId that was never provisioned, with userContext present — is refused with HTTP 404, x-amzn-errortype ResourceNotFoundException, message "Knowledge Base with id <knowledgeBaseId> does not exist": resource existence is checked before any retrieval-shaping input is interpreted, and userContext itself draws no rejection.
Emulator: answers HTTP 400, x-amzn-errortype ValidationException, with the entirely emulator-authored message "userContext: this service applies no per-document access control to a retrieval, so a retrieval for one user cannot be narrowed to the documents that user may read; omit userContext to retrieve over every ingested document". That text is fixed emulator prose (only the leading member name "userContext" comes from the request) and corresponds to no AWS error class, status or wording. Because it is raised ahead of knowledge-base resolution, the 404 ResourceNotFoundException path is unobservable for any request that sets userContext.
Why: a client doing access-control-filtered retrieval (the documented use of userContext) gets its request rejected outright instead of served, so identity-scoped RAG cannot run at all against the emulator; and because the refusal pre-empts existence checking, error handling keyed on ResourceNotFoundException never fires — a typo'd or deleted knowledgeBaseId is reported as an input-validation failure (400) rather than a missing resource (404), which also breaks retry/fallback logic that distinguishes the two.
Fix: the Retrieve operation's input validation in the bedrock-agent-runtime crate must stop rejecting the modeled userContext member, and the handler must resolve/validate the knowledgeBaseId (emitting ResourceNotFoundException 404 with the Cause 1 template) before interpreting retrieval-shaping input; retrieval state should accept userContext and, where no per-document ACL data exists, retrieve over the ingested documents rather than refuse.
Traces: bedrock-agent-runtime-retrieve-from-knowledge-base#task#1.

## Evidence
Cause 1 — fixture bedrock-agent-runtime-knowledge-base-retrieve-not-found, step task#1 (Retrieve):
  Expected: HTTP 404, x-amzn-errortype ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agent.runtimeservice/, $.message = "Knowledge Base with id KBBD995DA3 does not exist" (fixed text "Knowledge Base with id "/" does not exist"; variable part = the request's knowledgeBaseId).
  Actual: HTTP 404, ResourceNotFoundException, $.message = "Could not find knowledge base KBBD995DA3."

Cause 2 — fixture bedrock-agent-runtime-retrieve-from-knowledge-base, step task#1 (Retrieve, request carries userContext and a well-formed but unprovisioned knowledgeBaseId):
  Expected: HTTP 404, x-amzn-errortype ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agent.runtimeservice/, $.message = "Knowledge Base with id KBNOTFND01 does not exist".
  Actual: HTTP 400, x-amzn-errortype ValidationException, $.message = "userContext: this service applies no per-document access control to a retrieval, so a retrieval for one user cannot be narrowed to the documents that user may read; omit userContext to retrieve over every ingested document".

Replay summary: trace set simparity-wire-aws-v1, emulator up (health 200), 49 fixtures replayed; both clusters have verdict=mismatch with no stub_path and no owning issue.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS SDK/CLI bedrock-agent-runtime client at its data listener.
2. Cause 1: call Retrieve(knowledgeBaseId="KBBD995DA3" — any well-formed 10-character id that was never created, retrievalQuery={"text": "..."}). Observe HTTP 404 ResourceNotFoundException with message "Could not find knowledge base KBBD995DA3." instead of "Knowledge Base with id KBBD995DA3 does not exist".
3. Cause 2: call Retrieve(knowledgeBaseId="KBNOTFND01", retrievalQuery={"text": "..."}, userContext={"userId": "<any user id>"}). Observe HTTP 400 ValidationException refusing the userContext member instead of HTTP 404 ResourceNotFoundException "Knowledge Base with id KBNOTFND01 does not exist"; repeat with an existing knowledge base and observe the same 400 instead of a 200 retrieval.
4. Re-replay to confirm: fixture bedrock-agent-runtime-knowledge-base-retrieve-not-found step task#1 and fixture bedrock-agent-runtime-retrieve-from-knowledge-base step task#1.
