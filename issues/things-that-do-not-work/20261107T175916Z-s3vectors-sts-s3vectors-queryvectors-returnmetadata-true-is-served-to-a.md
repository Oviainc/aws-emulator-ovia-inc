Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3vectors__sts.query-vectors-return-metadata-requires-getvectors-permission
Issue-Key: s3vectors__sts.query-vectors-return-metadata-requires-getvectors-permission|_poll#39|AssertionError|raise AssertionError(
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# s3vectors QueryVectors(returnMetadata=true) is served to a session without s3vectors:GetVectors instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
QueryVectors

## Description
One root cause: the parameter-conditional GetVectors gate on QueryVectors is not enforced, and an STS session policy is not intersected with the role's identity policy.

Related open work: 20261023T014718Z-s3vectors-sts-s3vectors-returndata-returnmetadata-are-served-without-s3vectors-getvectors.md describes the same gate for ListVectors' returnData/returnMetadata, but the suite did not mark this QueryVectors check as owned by it (its failure point changed — the check now raises its own "not reached within 60.0s" AssertionError from _poll with a successful 200 as the last observation), so it is filed here and must be closed on its own. If that issue is repaired centrally, re-replay this check id to confirm.

=== Cause 1 of 1: QueryVectors returns metadata to a caller that holds only s3vectors:QueryVectors ===
Failing check id: s3vectors__sts.query-vectors-return-metadata-requires-getvectors-permission.

AWS: QueryVectors' documentation states the permission rule directly — "The request fails with a 403 Forbidden error if you request metadata filtering or metadata without the s3vectors:GetVectors permission" (API_S3VectorBuckets_QueryVectors); the sibling ListVectors Permissions block states the matching rule for returnData/returnMetadata. Smithy (services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json) models AccessDeniedException on QueryVectors with errorKind client and httpError 403, and QueryVectors' returnMetadata member is a Boolean defaulting to false. So the gate is CONDITIONAL ON THE REQUEST PARAMETERS, not on the operation: on a session allowed only s3vectors:QueryVectors, QueryVectors(topK, queryVector) with returnMetadata=false and no filter SUCCEEDS and serves keys only, while the identical call with returnMetadata=true — or with a metadata filter — is refused with HTTP 403 AccessDeniedException; adding s3vectors:GetVectors to the session flips both to success. Because the narrow caller here is an AssumeRole session carrying a session policy, the effective permissions are the intersection of the role's identity policy and that session policy, so the session policy alone must be able to withhold GetVectors.

Emulator: no refusal is ever raised. On the QueryVectors-only session, QueryVectors(returnMetadata=true) answers HTTP 200 and hands back the metadata of every matching vector: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'vectors': [{'key': '<key-a>', 'metadata': {...}}, {'key': '<key-b>', 'metadata': {...}}], 'distanceMetric': 'cosine'}. The check polls for the refusal and ends in AssertionError: "returnMetadata=true without s3vectors:GetVectors became refused with AccessDeniedException not reached within 60.0s (last observed: (True, {...200...}))". Neither the parameter-conditional GetVectors requirement nor the STS session policy is consulted.

Why: returnMetadata and filter are exactly how tenant metadata leaves the service. A client that withholds s3vectors:GetVectors so a principal may search an index but never read the metadata behind it sees that boundary silently removed — the emulator serves tenant-labelled metadata to a session AWS refuses with 403. A privilege-escalation regression in the application is undetectable, the keys-only-vs-metadata distinction cannot be tested, and an STS session policy appears to grant more than it does.

Fix: s3vectors request authorization — the policy evaluation that runs before the QueryVectors handler must take the request parameters into account (returnMetadata, and a metadata filter, each additionally require s3vectors:GetVectors) and refuse with AccessDeniedException (HTTP 403); it must also honour an AssumeRole session policy as the intersection with the role's identity policy. Worth noting while fixing: AccessDeniedException IS modeled on QueryVectors in this service's Smithy model, so the 403 has a modeled shape to serialize.

Traces: s3vectors__sts.query-vectors-return-metadata-requires-getvectors-permission.

## Evidence
Expected: on an AssumeRole session whose session policy allows only s3vectors:QueryVectors, QueryVectors(vectorBucketName, indexName, topK=<n>, queryVector, returnMetadata=false) succeeds with keys only (positive control), while QueryVectors(... returnMetadata=true) and QueryVectors(... filter=<metadata filter>) are each refused with HTTP 403 AccessDeniedException; on a session whose policy also allows s3vectors:GetVectors all of them succeed and the metadata read back equals the metadata written.
Actual: the narrow session's returnMetadata=true call succeeds: HTTP 200 with {'vectors': [{'key': 'qv-alpha', 'metadata': {'tenant': 'tenant-a', 'label': 'alpha'}}, {'key': 'qv-beta', 'metadata': {'tenant': 'tenant-b', 'label': 'beta'}}], 'distanceMetric': 'cosine'}. Harness evidence: AssertionError: returnMetadata=true without s3vectors:GetVectors became refused with AccessDeniedException not reached within 60.0s (last observed: (True, {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, ...})), raised from _poll. The specific keys, metadata values, bucket/index/role names and the date header are this run's fixtures, not requirements — the requirement is the 403 AccessDeniedException on the metadata-bearing request.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite) against the built emulator.
2. sts GetCallerIdentity to read the account id; s3vectors CreateVectorBucket(<bucket>) and CreateIndex(<bucket>, <index>, dataType=float32, dimension=3, distanceMetric=cosine).
3. PutVectors two vectors WITH metadata, and confirm with the runner identity that QueryVectors(returnMetadata=true) reads that metadata back (so a later refusal cannot be "no metadata to return").
4. iam CreateRole trusted by arn:aws:iam::<account>:root with an s3vectors identity policy, then sts AssumeRole twice: once with a session policy allowing ONLY s3vectors:QueryVectors, once with a session policy that also allows s3vectors:GetVectors.
5. On the narrow session call QueryVectors(vectorBucketName=<bucket>, indexName=<index>, topK=2, queryVector={'float32': [...]}) with returnMetadata=false — it should succeed (keys only).
6. On the SAME narrow session call QueryVectors with returnMetadata=true, and then with a metadata filter. Each must be refused with 403 AccessDeniedException; observe HTTP 200 with the vectors' metadata instead.
7. On the wide session the same calls succeed — confirming the missing gate is parameter-conditional and not a blanket denial.
