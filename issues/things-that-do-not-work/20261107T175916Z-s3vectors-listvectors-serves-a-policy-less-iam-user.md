Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3vectors.list-vectors-access-denied-without-permission
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors.list-vectors-access-denied-without-permission|_poll_for_access_denied#15|AssertionError|raise AssertionError(f"never observed AccessDeniedException, last saw {last}")
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# s3vectors ListVectors serves a policy-less IAM user with HTTP 200 instead of refusing it with 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListVectors

## Description
One root cause: the s3vectors data plane performs no request authorization, so an identity with no identity-based policy at all is served.

Related open work: 20261023T014718Z-s3vectors-6-behavioural-divergences-no-iam-authorization-on.md describes the same authorization gap, but the suite did not mark this check as owned by it (the check now fails on a different assertion — it reaches its explicit "never observed AccessDeniedException" AssertionError rather than the earlier poll timeout), so it is filed here and must be closed on its own. If that issue's Cause 1 is repaired centrally, re-replay this check id to confirm.

=== Cause 1 of 1: ListVectors is not authorized — an IAM user with no policy gets the index contents, and the subsequent grant is therefore unobservable ===
Failing check id: s3vectors.list-vectors-access-denied-without-permission.

AWS: Smithy (services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json) models AccessDeniedException on ListVectors with errorKind client and httpError 403, and ListVectors requires the caller to hold s3vectors:ListVectors. A freshly created IAM user with no identity-based policy, calling ListVectors(vectorBucketName, indexName) with its own access key, is refused with HTTP 403 AccessDeniedException. The message's fixed text is "User: <user-arn> is not authorized to perform: s3vectors:ListVectors on resource: <index-arn> because no identity-based policy allows the s3vectors:ListVectors action"; its variable parts are the calling principal's ARN (from the signing credentials) and the target index's ARN (built from the request's vectorBucketName/indexName), both values of the caller's own request and state — no literal ARN from any one run is part of the requirement. While IAM propagates a just-minted key the same code may appear with the different cause message "The security token included in the request is invalid.", which is why the check polls rather than asserting once. After s3vectors:ListVectors is attached to that same user, the identical call returns 200 with the index's one key.

Emulator: the call SUCCEEDS for the policy-less user. Replay records HTTP 200 with the full listing: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'vectors': [{'key': 'v1'}]}. No refusal is ever produced, so the check's poll ends in AssertionError: "never observed AccessDeniedException, last saw the call SUCCEEDED and returned {...}". The emulator never resolves the signing principal to a policy and never evaluates one.

Why: every caller is effectively an administrator on the s3vectors data plane. A client that partitions an index with IAM (per-tenant readers, read-only roles, deny-by-default users) sees unauthorized listings succeed, so an authorization regression in the application under test is invisible here and fails only on real AWS; and no negative-path test asserting a 403 can ever pass. The grant leg is equally untestable: since the deny never happens, allowing ListVectors changes nothing observable, so the emulator cannot show that it honours the policy either.

Fix: s3vectors request authorization — the credential/principal resolution and identity-policy evaluation that must run before the ListVectors handler, plus the handler's AccessDeniedException (403) path with the message above assembled from the request's principal and index ARNs.

Traces: s3vectors.list-vectors-access-denied-without-permission.

## Evidence
Expected: HTTP 403, error code AccessDeniedException, message "User: <user-arn> is not authorized to perform: s3vectors:ListVectors on resource: <index-arn> because no identity-based policy allows the s3vectors:ListVectors action" (fixed text as quoted; <user-arn> is the signing IAM user and <index-arn> the index named by vectorBucketName/indexName) — then HTTP 200 with the index's vector keys once s3vectors:ListVectors is attached to the same user.
Actual: HTTP 200 for the policy-less user on the very first attempt and on every retry: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '26', ...}, 'RetryAttempts': 0}, 'vectors': [{'key': 'v1'}]}. Harness evidence: AssertionError: never observed AccessDeniedException, last saw the call SUCCEEDED and returned {...} (raised from _poll_for_access_denied). Bucket/index/user names, the date header and content-length are per-run values and are not requirements.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite) against the built emulator.
2. With the runner credentials: CreateVectorBucket(vectorBucketName=<bucket>), CreateIndex(vectorBucketName=<bucket>, indexName=<index>, dataType=float32, dimension=4, distanceMetric=cosine), PutVectors one vector with key v1, and GetIndex to read indexArn.
3. Positive control: ListVectors(vectorBucketName=<bucket>, indexName=<index>) with the runner credentials succeeds.
4. iam CreateUser for a brand-new user with NO policy attached, CreateAccessKey for it, and build an s3vectors client from those credentials.
5. Call ListVectors(vectorBucketName=<bucket>, indexName=<index>) on that client, retrying for up to a minute to let the key propagate. Observe HTTP 200 with {'vectors': [{'key': 'v1'}]} instead of 403 AccessDeniedException with the message above.
6. Attach an inline policy allowing s3vectors:ListVectors on Resource "*" to that user and repeat — the call still succeeds, so the allow/deny distinction is unobservable.
