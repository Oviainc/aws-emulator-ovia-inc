Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-resource-policy-management
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-resource-policy-management dynamodb-tagging-and-resource-policy
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|GetResourcePolicy|4xx->4xx|$.message
Issue-Key: dynamodb|PutResourcePolicy|4xx->4xx|$.message
Issue-Key: dynamodb|PutResourcePolicy|2xx->4xx|$.RevisionId
Issue-Key: dynamodb|PutResourcePolicy|2xx->4xx|status
Issue-Key: dynamodb|DeleteResourcePolicy|2xx->2xx|$
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# DynamoDB resource-policy refusals name the wrong resource and the delete response is empty

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
Four net-new resource-policy divergences that the existing resource-policy issue does not carry: the not-found message names the wrong thing on both the read and the write path, a policy whose statement carries a Condition cannot be attached at all, and DeleteResourcePolicy answers with an empty body. 4 causes, all dynamodb.

Cause 1 of 4: GetResourcePolicy's not-found message identifies the resource by ARN instead of by table name.
AWS: the recording shows HTTP 400 with 'Requested resource not found: Table: <table name> not found'. The fixed text is 'Requested resource not found: Table: ' and ' not found'; the variable part is the table name parsed out of the request's ResourceArn.
Emulator: 400 with 'Requested resource not found: ResourceArn: <the full table ARN> not found' — a different label and the whole ARN where AWS prints the bare table name.
Why: the message is how a caller identifies which resource was missing when it batches policy reads; tooling that parses the name out of the message, or asserts on the 'Table: ' label, misreads the refusal.
Fix: the GetResourcePolicy handler's ResourceNotFoundException message — label the resource 'Table:' and interpolate the table name extracted from the ARN.
Traces: dynamodb-resource-policy-management#task#3.

Cause 2 of 4: PutResourcePolicy's not-found message has the same defect.
AWS: the same 'Requested resource not found: Table: <table name> not found' sentence for a put against a table that does not exist.
Emulator: the same 'ResourceArn: <arn>' form.
Why: as cause 1, on the write path.
Fix: the same shared message builder, used by the PutResourcePolicy handler.
Traces: dynamodb-resource-policy-management#task#4.

Cause 3 of 4: PutResourcePolicy refuses with 400 a policy whose statement carries a Condition.
AWS: Smithy: PutResourcePolicy returns 200 with RevisionId (PolicyRevisionId, @length 1..255) and ResourcePolicy is an opaque string, so a Condition block is part of the document, not something the attach path interprets. The recording shows the conditioned deny statement (Condition StringEquals aws:RequestedRegion) being attached successfully with a 200 and a RevisionId.
Emulator: 400, with no RevisionId in the body — the policy is never attached.
Why: conditioned statements are the normal form of a DynamoDB resource policy; refusing to store one means the whole fine-grained-policy feature is unusable and every downstream read of the policy fails for a reason that is really an attach failure.
Fix: the PutResourcePolicy handler — accept and store the policy document verbatim regardless of Condition blocks, leaving evaluation to the authorization path.
Traces: dynamodb-tagging-and-resource-policy#task#7.

Cause 4 of 4: DeleteResourcePolicy answers 200 with an entirely empty body.
AWS: Smithy: DeleteResourcePolicyOutput carries RevisionId (PolicyRevisionId, @length 1..255) — the revision of the policy that was deleted. The recording shows 200 with a RevisionId present (its value is a per-run revision token and is not itself a requirement).
Emulator: 200 with {} — RevisionId absent.
Why: callers record the deleted revision for audit and for ExpectedRevisionId-guarded retries; with no RevisionId a delete cannot be confirmed as having removed the revision the caller intended.
Fix: the DeleteResourcePolicy response serializer — return the revision id of the deleted policy.
Traces: dynamodb-tagging-and-resource-policy#task#11.

Related open issues: '20261023T105249Z-dynamodb-resource-based-policy-lifecycle-diverges-policy-unreadable-conditioned.md' owns the policy-unreadable, conditioned-deny-on-GetItem, re-serialization and digest-RevisionId clusters; it does not cover the not-found message wording, the PutResourcePolicy attach refusal, or DeleteResourcePolicy answering an empty body (it reported a digest value in that field, not an absent field). '20261029T180026Z-dynamodb-partiql-resource-policy-arn-addressing-crc32-header.md' covers ARN addressing and CRC32, not these.

## Evidence
Divergence 33 (GetResourcePolicy, 400->400): $.message expected 'Requested resource not found: Table: <name>-missing not found', got 'Requested resource not found: ResourceArn: arn:aws:dynamodb:<region>:<account>:table/<name>-missing not found'.
Divergence 34 (PutResourcePolicy, 400->400): same expected vs actual message.
Divergence 37 (PutResourcePolicy, 200->400): status expected 200 got 400; $.RevisionId missing (the recording returns a revision token).
Divergence 39 (DeleteResourcePolicy, 200->200): $ empty_body — expected a body containing RevisionId, got {}.
Model: smithy_query(operation=DeleteResourcePolicy) -> output RevisionId:PolicyRevisionId @length 1..255; errors PolicyNotFoundException(400), ResourceNotFoundException(400), ResourceInUseException(400), LimitExceededException(400), InternalServerError(500).

## Steps to Reproduce
1. Replay dynamodb-resource-policy-management#task#3 and #task#4: Get/PutResourcePolicy against a non-existent table -> message labels the resource 'ResourceArn: <arn>' instead of 'Table: <name>'.
2. Replay dynamodb-tagging-and-resource-policy#task#7: PutResourcePolicy with a statement carrying a Condition -> 400 with no RevisionId instead of 200.
3. Replay dynamodb-tagging-and-resource-policy#task#11: DeleteResourcePolicy -> 200 with an empty body instead of a body carrying RevisionId.
