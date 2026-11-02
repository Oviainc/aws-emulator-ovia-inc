Issue-Source: trace-sweep-divergence
Issue-Trace: s3w-bucket-policy-deny-and-owner-guard
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3w-bucket-policy-deny-and-owner-guard s3-bucket-abac-and-session s3-versioning-and-object-lock s3w-object-validation-error-envelopes s3w-list-objects-v2-special-character-keys s3-bucket-access-policy-and-public-access-controls s3-bucket-policy-and-ownership s3-bucket-policy-lifecycle
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetObject|4xx->4xx|$
Issue-Key: s3|GetObject|4xx->4xx|$.Error.Code
Issue-Key: s3|GetObject|4xx->4xx|$.Error.Message
Issue-Key: s3|GetObject|4xx->4xx|status
Issue-Key: s3|HeadObject|4xx->4xx|status
Issue-Key: s3|GetObject|4xx->2xx|$
Issue-Key: s3|GetObject|4xx->2xx|$headers.content-type
Issue-Key: s3|GetObject|4xx->2xx|status
Issue-Key: s3|CreateSession|2xx->4xx|$.ListBucketResult
Issue-Key: s3|CreateSession|2xx->4xx|status
Issue-Key: s3|PutObjectRetention|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|PutObjectRetention|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|PutObjectRetention|4xx->4xx|$.Error.Code
Issue-Key: s3|PutObjectRetention|4xx->4xx|$.Error.Message
Issue-Key: s3|PutObjectRetention|4xx->4xx|status
Issue-Key: s3|GetObject|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|GetObject|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents[*]
Issue-Key: s3|GetBucketPolicy|2xx->2xx|$headers.content-type
Issue-Related: 20261029T192819Z-s3-listobjectsv2-echoes-the-percent-encoded-prefix-instead.md
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 authorization, session and key-encoding paths diverge from AWS

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Eight net-new root causes that do not fit the serializer families filed alongside this issue: bucket-policy authorization is not evaluated at all (three ways), CreateSession is unusable, two refusals pick the wrong status and Code, object keys are double percent-encoded, and GetBucketPolicy serves the wrong content type. A fixer must close all eight.

Cause 1 of 8: a bucket policy with an explicit Deny does not deny GetObject.
AWS: 403 with Code 'AccessDenied' and the message 'User: <caller ARN> is not authorized to perform: s3:ListBucket on resource: "<bucket ARN>" with an explicit deny in a resource-based policy' — fixed text with the caller ARN, the action and the resource ARN as variable parts taken from the request and the policy.
Emulator: 404 NoSuchKey 'The specified key does not exist.'; the harness polled 29 times and never saw a 403.
Why: an explicit deny is the primary guardrail clients test; reporting 404 makes a deny look like a missing object, so a security test passes against an unprotected emulator.
Fix: the request authorizer that must evaluate the bucket policy before the object lookup, and the AccessDenied refusal it raises. Related open issues 20261029T192356Z-iam-s3-s3-getobject-is-authorized-with-neither.md and 20261029T211510Z-s3-accepts-requests-aws-refuses-bucket-policy-conditions.md describe adjacent authorization gaps but carry no wire replay key for these steps.
Traces: s3w-bucket-policy-deny-and-owner-guard#task#5.

Cause 2 of 8: the same explicit deny does not deny HeadObject.
AWS: 403.
Emulator: 404.
Fix: as cause 1, on the HeadObject path.
Traces: s3w-bucket-policy-deny-and-owner-guard#task#6.

Cause 3 of 8: a bucket policy that denies the caller still serves the object.
AWS: 403 AccessDenied with message 'Access Denied' and content-type application/xml.
Emulator: 200 with the object body and content-type binary/octet-stream.
Fix: as cause 1 — this is the positive control, so authorization is not consulted on the success path either.
Traces: s3w-bucket-policy-deny-and-owner-guard#task#10.

Cause 4 of 8: CreateSession refuses a general-purpose bucket session so the subsequent listing cannot run.
AWS: the session is established and the following ListObjects answers 200 with <ListBucketResult> carrying Name, Prefix, Marker, MaxKeys and IsTruncated.
Emulator: 400, and no ListBucketResult. The open issue 20261029T211510Z-s3-express-one-zone-is-unreachable-createsession-answers.md records CreateSession answering NoSuchBucket/InvalidRequest for directory buckets and carries no wire replay key for this step.
Fix: the CreateSession handler and the session credential path the following request uses.
Traces: s3-bucket-abac-and-session#task#4.

Cause 5 of 8: PutObjectRetention with a past RetainUntilDate answers the wrong status and Code.
AWS: 400 InvalidArgument, message 'The retain until date must be in the future!' (fixed text), with <ArgumentName>RetainUntilDate</ArgumentName> and <ArgumentValue> echoing the submitted date.
Emulator: 403 AccessDenied 'Access Denied because object protected by object lock.' and no argument members.
Why: the caller is told it lacks permission when its input is simply invalid, so it retries with different credentials instead of a different date.
Fix: the PutObjectRetention date validation, which must run before the lock-protection check, and its error construction.
Traces: s3-versioning-and-object-lock#task#6.

Cause 6 of 8: GetObject with a syntactically invalid versionId answers not-found instead of invalid-argument.
AWS: 400 InvalidArgument, message 'Invalid version id specified' (fixed text), with <ArgumentName>versionId</ArgumentName> and <ArgumentValue> echoing the rejected value.
Emulator: 404 NoSuchVersion 'The specified version does not exist.'
Why: a client cannot distinguish a malformed version id (its own bug) from a version that has been deleted (a retryable state), so it treats a permanent client error as transient.
Fix: the GetObject versionId syntax validation ahead of the version lookup.
Traces: s3w-object-validation-error-envelopes#task#13.

Cause 7 of 8: ListObjectsV2 returns the prefix and every key percent-encoded a second time under encoding-type=url.
AWS: with encoding-type=url the returned Prefix and Key are the URL-encoding of the real key, so a key literally containing '%26' comes back as 'enc/amp%26key.txt' and a space comes back as '+' — the '/' separators and the already-encoded sequences are not re-encoded.
Emulator: encodes the separators and re-encodes the encoded sequences, giving 'enc%2Famp%26key.txt' and 'enc%2Fsp%20ace.txt'; the Prefix comes back as 'enc%2F' instead of 'enc/'.
Why: a client that URL-decodes the response gets a key that does not exist, so round-tripping a listing into GetObject fails for every key with a special character. The open issue 20261029T192819Z-s3-listobjectsv2-echoes-the-percent-encoded-prefix-instead.md covers a plain (non-encoded) prefix echo and carries no wire replay key for this encoded listing.
Fix: the ListObjectsV2 key/prefix encoding for encoding-type=url.
Traces: s3w-list-objects-v2-special-character-keys#task#8.

Cause 8 of 8: GetBucketPolicy serves the policy as text/plain.
AWS: the response content-type is application/json — the body is the bucket policy JSON document. Smithy: GetBucketPolicyOutput.Policy is the httpPayload.
Emulator: content-type text/plain.
Why: a client or proxy that dispatches on content-type will not parse the body as JSON.
Fix: the GetBucketPolicy serializer's content-type. The open issue 20261029T174654Z-cloudwatch-dynamodb-ecr-events-iam-logs-s3-bucket-policy-does-not-round-trip-getbucketpolicy.md concerns the policy document's contents, not its content type, and carries no wire replay key for this step.
Traces: s3-bucket-access-policy-and-public-access-controls#task#3

## Evidence
All eight clusters are 'mismatch'.
s3w-bucket-policy-deny-and-owner-guard#task#5 (GetObject): status expected 403, got 404; Code expected 'AccessDenied', got 'NoSuchKey'; Message expected the explicit-deny sentence, got 'The specified key does not exist.'; poll: status 404 not in [403] after 29 attempt(s)
s3w-bucket-policy-deny-and-owner-guard#task#6 (HeadObject): status expected 403, got 404
s3w-bucket-policy-deny-and-owner-guard#task#10 (GetObject): status expected 403, got 200; body type_mismatch (expected an AccessDenied envelope, got the object payload); content-type expected 'application/xml', got 'binary/octet-stream'
s3-bucket-abac-and-session#task#4 (CreateSession): status expected 200, got 400; $.ListBucketResult missing (expected Name/Prefix/Marker/MaxKeys/IsTruncated)
s3-versioning-and-object-lock#task#6 (PutObjectRetention): status expected 400, got 403; Code expected 'InvalidArgument', got 'AccessDenied'; Message expected 'The retain until date must be in the future!', got 'Access Denied because object protected by object lock.'; ArgumentName/ArgumentValue missing
s3w-object-validation-error-envelopes#task#13 (GetObject): status expected 400, got 404; Code expected 'InvalidArgument', got 'NoSuchVersion'; Message expected 'Invalid version id specified', got 'The specified version does not exist.'; ArgumentName missing (expected 'versionId'), ArgumentValue missing
s3w-list-objects-v2-special-character-keys#task#8 (ListObjectsV2): Prefix value_mismatch (expected 'enc/', got 'enc%2F'); Contents[0..4] keys double-encoded, e.g. expected 'enc/amp%26key.txt' got 'enc%2Famp%26key.txt', expected 'enc/sp+ace.txt' got 'enc%2Fsp%20ace.txt'
s3-bucket-access-policy-and-public-access-controls#task#3 (GetBucketPolicy): $headers.content-type value_mismatch (expected 'application/json', got 'text/plain')

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand: PutBucketPolicy with an explicit Deny on s3:GetObject/s3:ListBucket for the caller, then GetObject and HeadObject (expect 403 AccessDenied) and GetObject on the allowed control object; CreateSession against a bucket and issue the following ListObjects with the session credentials; PutObjectRetention with a RetainUntilDate in the past (expect 400 InvalidArgument with ArgumentName RetainUntilDate); GetObject with versionId 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA' (expect 400 InvalidArgument 'Invalid version id specified'); PutObject keys containing '&', '#', '+', a space and non-ASCII under a prefix 'enc/', then ListObjectsV2 with encoding-type=url and prefix=enc/ and check the prefix and keys are encoded exactly once; GetBucketPolicy and check content-type is application/json.
