Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3vectors.list-vectors-insufficient-permission-denied s3vectors.list-vectors-access-denied-without-permission s3vectors.deploy-vector-bucket-cloudformation-behaves s3vectors.deploy-vector-bucket-and-index-behaves-via-cloudformation s3vectors.get-vector-bucket-policy-missing-not-found s3vectors.get-vector-bucket-policy-missing-rejected s3vectors.non-filterable-metadata-exempt-from-filterable-size-limit s3vectors.query-vectors-pagination-preserves-distance-order-across-pages
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors.list-vectors-insufficient-permission-denied|_error#21|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: s3vectors.list-vectors-access-denied-without-permission|_poll_for_access_denied#15|Timeout|time.sleep(interval_s)
Issue-Key: s3vectors|PutVectors|NotFoundException
Issue-Key: s3vectors|GetIndex|ValidationException
Issue-Key: s3vectors.get-vector-bucket-policy-missing-not-found|run#7|AssertionError|assert code == "NotFoundException", f"expected NotFoundException for a bucket with no policy, got {code!r}"
Issue-Key: s3vectors.get-vector-bucket-policy-missing-rejected|run#6|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Key: s3vectors.non-filterable-metadata-exempt-from-filterable-size-limit|_client_error#29|AssertionError|raise AssertionError(
Issue-Key: s3vectors.query-vectors-pagination-preserves-distance-order-across-pages|run#26|AssertionError|assert len(page) <= _RESULTS_PER_PAGE_CAP, (
Issue-Commit: c1cdf4b562f81ba1ac84179127e5df6b90873b05

# s3vectors: 6 behavioural divergences — no IAM authorization on ListVectors, CloudFormation-deployed vector buckets/indexes are not real, GetVectorBucketPolicy 501, unenforced filterable-metadata budget, QueryVectors page cap ignored

> _Note: this issue was identified against the code base at commit `c1cdf4b562f81ba1ac84179127e5df6b90873b05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListVectors

## Description
Six independent root causes found in the s3vectors emulator by the behavioral suite. Each is numbered and carries its own failing check ids. All six must be closed before this issue is submitted.

=== Cause 1 of 6: ListVectors serves an unauthorized principal instead of refusing it (403 AccessDeniedException) ===
AWS: Smithy (services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json) models AccessDeniedException (HTTP 403, errorKind client, doc "Access denied.") on ListVectors, and the operation documents that the caller must hold the s3vectors:ListVectors permission. A principal with no identity-based policy at all, and a principal granted only s3vectors:GetIndex + s3vectors:ListIndexes, are both refused with 403 AccessDeniedException. Recorded message example (built from the request, so the RULE is the requirement, not this text): "User: <user-arn> is not authorized to perform: s3vectors:ListVectors on resource: <index-arn> because no identity-based policy allows the s3vectors:ListVectors action".
Emulator: the call succeeds. Replay of s3vectors.list-vectors-insufficient-permission-denied shows HTTP 200 with the index contents for a principal that was never granted ListVectors: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'vectors': [{'key': 'v1'}, {'key': 'v2'}]}. s3vectors.list-vectors-access-denied-without-permission never observes any refusal for a policy-less IAM user — its propagation poll keeps seeing a SUCCESS and the check hits its 60s wall clock.
Why: every caller is effectively an administrator. A client that relies on IAM to partition an index (per-tenant readers, read-only roles, deny-by-default users) sees unauthorized listings succeed, so an authorization regression in the application under test is invisible, and any negative-path test for a 403 can never pass.
Fix: request authorization for the s3vectors data plane — the credential/principal resolution and policy evaluation that should run before ListVectors' handler, and the handler's AccessDeniedException path.
Traces: s3vectors.list-vectors-insufficient-permission-denied, s3vectors.list-vectors-access-denied-without-permission.

=== Cause 2 of 6: a CloudFormation-deployed AWS::S3Vectors::VectorBucket reaches CREATE_COMPLETE but no vector bucket exists in s3vectors ===
AWS: a stack declaring AWS::S3Vectors::VectorBucket + AWS::S3Vectors::Index reaches CREATE_COMPLETE only once the resources exist in S3 Vectors, after which PutVectors/QueryVectors addressed by vectorBucketName/indexName operate on them normally (Smithy: PutVectors models NotFoundException HTTP 404 only for a resource that can't be found).
Emulator: the stack reaches CREATE_COMPLETE, then the very next data-plane call fails — PutVectors answers 404 NotFoundException (recorded message example: "vector bucket not found: <bucket-name>") for the bucket the stack just reported as created.
Why: CREATE_COMPLETE is a lie. A client that deploys its vector store by template and then writes to it gets a not-found on the first write, and deploy-then-use flows (the normal IaC pattern) cannot be exercised at all.
Fix: the CloudFormation resource handler/provisioner for the AWS::S3Vectors::VectorBucket and AWS::S3Vectors::Index resource types — it must create the resources in s3vectors' state (or fail the resource) rather than only recording a stack row.
Traces: s3vectors.deploy-vector-bucket-cloudformation-behaves.

=== Cause 3 of 6: GetAtt <Index>.IndexArn returns an ARN that violates the modeled IndexArn pattern ===
AWS: Smithy IndexArn (CreateIndex output member indexArn, ListVectors/PutVectors/QueryVectors/GetIndex input member indexArn) carries pattern ^arn:aws[-a-z0-9]*:s3vectors:[a-z0-9-]+:[0-9]{12}:bucket/[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]/index/[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]$ — an index ARN always nests the index under its bucket (…:bucket/<bucket>/index/<index>). The value CloudFormation returns for GetAtt on an Index must therefore be accepted by GetIndex(indexArn=…).
Emulator: the stack output is of the form arn:aws:s3vectors:<region>:<account>:index/<index-name> — the bucket/<bucket>/ segment is missing — and feeding it straight back into GetIndex is refused with 400 ValidationException: "1 validation error detected: Value '<arn>' at 'indexArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[-a-z0-9]*:s3vectors:…:bucket/…/index/…$".
Why: the emulator's own ARN minting contradicts its own modeled constraint, so an ARN handed out by one surface is rejected by every operation that consumes it. Any template that wires an index ARN into another resource (a Lambda env var, an IAM policy resource, a knowledge base) propagates an unusable value.
Fix: the ARN construction for s3vectors indexes used to populate the CloudFormation resource's physical id / GetAtt IndexArn (and the same minting used by CreateIndex/GetIndex) so it matches the modeled IndexArn pattern.
Traces: s3vectors.deploy-vector-bucket-and-index-behaves-via-cloudformation.

=== Cause 4 of 6: GetVectorBucketPolicy answers 501 NotImplemented instead of 404 NotFoundException ===
AWS: Smithy GetVectorBucketPolicy models NotFoundException (HTTP 404, "The request was rejected because the specified resource can't be found.") and an output whose only member is policy. A vector bucket that never had a policy set has no policy resource, so the call is refused with NotFoundException — it does not return an empty or absent policy, and it certainly does not return a server-side 501.
Emulator: error code 'NotImplemented'. The operation is an unimplemented scaffold stub: services/service-com-amazonaws-s3vectors-s3vectors/src/ops/get_vector_bucket_policy.rs still carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and its handle() body returns Err(crate::scaffolded::Scaffolded::not_implemented("GetVectorBucketPolicy")). Note: unlike the sibling policy operations, this stub is NOT covered by an auto-filed stub issue in this run, which is why it is reported here.
Why: the whole vector-bucket-policy read path is unreachable. A client cannot distinguish "no policy" from "service broken", cannot implement get-modify-put on a bucket policy, and sees a 501 where its retry/error handling expects a 4xx.
Fix: implement GetVectorBucketPolicy in its handler file (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/get_vector_bucket_policy.rs): resolve the bucket by vectorBucketName or vectorBucketArn, return the stored policy, and return NotFoundException when the bucket exists but holds no policy; then drop the SCAFFOLDED marker.
Traces: s3vectors.get-vector-bucket-policy-missing-not-found, s3vectors.get-vector-bucket-policy-missing-rejected.

=== Cause 5 of 6: PutVectors does not enforce the per-vector filterable-metadata budget ===
AWS: the S3 Vectors limits table (Amazon S3 User Guide, s3-vectors-limitations) states two separate per-vector budgets — filterable metadata up to 2 KB, and total metadata (filterable + non-filterable) up to 40 KB — and PutVectors refuses an over-budget request with 400 (Smithy: PutVectors models ValidationException, HTTP 400). A metadata value of ~8 KiB is therefore ACCEPTED when its key is declared in the index's nonFilterableMetadataKeys and REFUSED when the key is filterable. (The check carries this as a documented-limits hypothesis; the divergence it records is over-permissiveness, which holds either way the exact boundary is read.)
Emulator: both legs succeed — an ~8 KiB value under an undeclared, therefore filterable, metadata key is accepted with no refusal at all ("the call was expected to be REFUSED but it succeeded -- filterable metadata above the documented per-vector cap must not be accepted").
Why: the filterable budget is the reason nonFilterableMetadataKeys exists. A client that sizes its metadata against the 2 KB filterable cap gets no feedback here and will be refused by real AWS on the same payload, and the distinction between filterable and non-filterable metadata is unverifiable.
Fix: PutVectors' input validation in its handler — measure the filterable (not declared non-filterable) portion of each vector's metadata against the per-vector cap, and the combined portion against the total cap, and refuse with ValidationException (HTTP 400).
Traces: s3vectors.non-filterable-metadata-exempt-from-filterable-size-limit.

=== Cause 6 of 6: QueryVectors returns every result in one response, ignoring the per-response cap and nextToken ===
AWS: QueryVectors is @paginated (Smithy: inputToken/outputToken nextToken, items vectors; nextToken length 1..4096) and a single response carries at most 100 results; a topK above that is drained over pages with nextToken, and distance order is preserved across the page boundary. This per-page cap was confirmed against real AWS by the check's own recording (topK=130 → 100 on page 0 plus a nextToken).
Emulator: page 0 carries 130 results — "page 0 carries 130 results, above the documented per-response cap of 100" — i.e. the whole topK is returned in one response and no nextToken is ever minted.
Why: a client written against AWS drains QueryVectors with a nextToken loop and sizes buffers to the 100-result page. The emulator never exercises that loop, so a paging bug in the client (dropped pages, ignored token, unbounded response handling) passes here and fails on real AWS.
Fix: QueryVectors' handler/serializer — cap the returned vectors list at the per-response limit and emit nextToken, resuming the same distance-ordered result set on the following call.
Traces: s3vectors.query-vectors-pagination-preserves-distance-order-across-pages.

## Evidence
Cause 1 — expected: HTTP 403 AccessDeniedException for ListVectors by a principal without s3vectors:ListVectors; actual: HTTP 200 with {'vectors': [{'key': 'v1'}, {'key': 'v2'}]} (list-vectors-insufficient-permission-denied), and no refusal ever observed for a policy-less IAM user (list-vectors-access-denied-without-permission timed out after 60s while the call kept succeeding).
Cause 2 — expected: after CREATE_COMPLETE on a stack with AWS::S3Vectors::VectorBucket + AWS::S3Vectors::Index, PutVectors(vectorBucketName=<bucket>, indexName=<index>) succeeds; actual: botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the PutVectors operation: vector bucket not found: <bucket-name>.
Cause 3 — expected: GetAtt Index.IndexArn is an ARN matching ^arn:aws[-a-z0-9]*:s3vectors:[a-z0-9-]+:[0-9]{12}:bucket/…/index/…$ and GetIndex(indexArn=…) returns the index; actual: the stack output is arn:aws:s3vectors:<region>:<account>:index/<index-name> and GetIndex answers 400 ValidationException "1 validation error detected: Value '…' at 'indexArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[-a-z0-9]*:s3vectors:[a-z0-9-]+:[0-9]{12}:bucket/[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]/index/[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]$".
Cause 4 — expected: error code NotFoundException (HTTP 404) from GetVectorBucketPolicy on a bucket with no policy; actual: error code 'NotImplemented' (scaffold stub returns Scaffolded::not_implemented("GetVectorBucketPolicy")).
Cause 5 — expected: a ClientError (ValidationException, HTTP 400) from PutVectors for ~8 KiB of metadata under a filterable key; actual: the call succeeded.
Cause 6 — expected: at most 100 vectors per QueryVectors response plus a nextToken for topK=130; actual: page 0 carried 130 results and no token.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite) against the built emulator.
2. Cause 1: create a vector bucket + index, PutVectors two vectors; create an IAM user with no policy (and one granted only s3vectors:GetIndex/ListIndexes), mint an access key, and call ListVectors(vectorBucketName, indexName) with those credentials — observe HTTP 200 with the vector keys instead of 403 AccessDeniedException.
3. Cause 2: CreateStack with Resources {VecBucket: AWS::S3Vectors::VectorBucket, VecIndex: AWS::S3Vectors::Index}, wait for CREATE_COMPLETE, then PutVectors(vectorBucketName=<bucket>, indexName=<index>) — observe 404 NotFoundException.
4. Cause 3: same stack with Outputs IndexArn: !GetAtt VectorIndex.IndexArn; read the output and call GetIndex(indexArn=<output>) — observe 400 ValidationException on the indexArn pattern.
5. Cause 4: CreateVectorBucket, then GetVectorBucketPolicy(vectorBucketName=<bucket>) without ever setting a policy — observe error code NotImplemented instead of NotFoundException.
6. Cause 5: CreateIndex with metadataConfiguration.nonFilterableMetadataKeys=[<key>], then PutVectors with ~8 KiB of metadata under an UNDECLARED key — observe success instead of a 400 ValidationException.
7. Cause 6: PutVectors 130 vectors into one index, then QueryVectors(topK=130) — observe 130 vectors in a single response and no nextToken.
