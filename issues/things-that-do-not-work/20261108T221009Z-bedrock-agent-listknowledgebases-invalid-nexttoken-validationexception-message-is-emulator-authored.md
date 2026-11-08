Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-kbdeps-list-knowledge-bases-paging-mistakes
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-kbdeps-list-knowledge-bases-paging-mistakes
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|ListKnowledgeBases|4xx->4xx|$.message
Issue-Commit: 93693326b17ec505c65863af1eb22012199ba94a

# ListKnowledgeBases invalid-nextToken ValidationException message is emulator-authored

> _Note: this issue was identified against the code base at commit `93693326b17ec505c65863af1eb22012199ba94a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListKnowledgeBases

## Description
One cause.

Cause 1 of 1: ListKnowledgeBases rejects a nextToken the service never issued with the right error class and status but with the emulator's own prose instead of AWS's fixed message text.

AWS: Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json) models ListKnowledgeBases with input member nextToken (target NextToken, @length min 1 max 2048, @pattern ^\S*$) and error ValidationException (errorKind client, httpError 400, documented 'Input validation failed. Check your request parameters and retry the request.'). The model does not state the message text, so the message itself is recorded behavior only: the recorded AWS response for a nextToken that this service did not mint is HTTP 400, x-amzn-errortype ValidationException, with the message body exactly 'The request contains an invalid next token'. That text is entirely FIXED — it interpolates no value from the request or from service state (it does not echo the offending token, the member name, or any id), so the requirement is the literal string.

Emulator: the replayed step returns the correct HTTP 400 and the correct ValidationException error code, but the message 'nextToken is not a pagination token this service issued'. It additionally populates fieldList with {name: 'nextToken', message: 'not a pagination token this service issued'}, which is the same emulator-authored prose surfaced a second time. Replay detail: $.message: value_mismatch (expected 'The request contains an invalid next token', got 'nextToken is not a pagination token this service issued').

Why: the error message is part of the API contract. Clients, SDK retry/classification wrappers, CLI users and test suites that match on AWS's invalid-pagination-token wording (log scrapers, error-mapping tables, assertions on the documented text) see a string AWS never emits, so an invalid-token path that is handled correctly against AWS is reported as an unrecognized validation failure against the emulator. The correct status and code mean the divergence is invisible until someone reads the message, which makes it an easy source of silently wrong client behaviour.

Fix: belongs in the ListKnowledgeBases handler's pagination-token validation — the ValidationException it constructs when KnowledgeBasePageCursor::parse rejects the token. Emit AWS's fixed message for the invalid-next-token class instead of emulator prose, and make the fieldList entry (if kept) consistent with it rather than restating the emulator's own wording. Note the open issue '20261030T000542Z-bedrock-agent-listknowledgebases-stub.md' (Implement bedrock-agent ListKnowledgeBases: unimplemented scaffold stub) does NOT cover this: the operation is in fact implemented and answers 400 here, so that issue is stale with respect to this code path and carries no replay key for this step.

Traces: bedrock-agent-kbdeps-list-knowledge-bases-paging-mistakes#task#3. Emulator source for reference: services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/list_knowledge_bases.rs lines 167-187 (message at lines 174-181).

## Evidence
Fixture bedrock-agent-kbdeps-list-knowledge-bases-paging-mistakes, step task#3, operation ListKnowledgeBases.

Expected (recorded AWS): HTTP 400, x-amzn-errortype: ValidationException, body $.message = 'The request contains an invalid next token'.
Actual (emulator): HTTP 400, x-amzn-errortype: ValidationException, body $.message = 'nextToken is not a pagination token this service issued'.

Replay verdict: mismatch; divergence_key service=bedrock-agent|ListKnowledgeBases|status=400->400|keys=$.message. Status and error code agree; only the message text diverges. The expected message has no variable parts — its text is fixed in full. The emulator string is hard-coded in services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/list_knowledge_bases.rs:174-181.

## Steps to Reproduce
1. Boot the emulator with the bedrock-agent service mounted.
2. Call ListKnowledgeBases (POST /knowledgebases/) with a nextToken that this emulator never issued, e.g. nextToken set to an arbitrary non-whitespace string that is not a cursor the service minted.
3. Observe HTTP 400 ValidationException with message 'nextToken is not a pagination token this service issued'.
4. AWS answers HTTP 400 ValidationException with message 'The request contains an invalid next token'.
5. Equivalent: replay fixture bedrock-agent-kbdeps-list-knowledge-bases-paging-mistakes step task#3.
