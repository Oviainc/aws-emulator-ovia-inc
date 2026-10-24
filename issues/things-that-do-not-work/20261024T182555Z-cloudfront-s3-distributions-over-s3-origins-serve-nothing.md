Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__s3.default-root-object-serves-root-but-tenant-directory-rejected cloudfront__s3.oai-origin-edge-rejects-a-missing-key-with-access-denied cloudfront__s3.unpublished-or-origin-event-function-association-rejected cloudfront__s3.bucket-policy-rejects-an-origin-identity-that-does-not-exist
Issue-Key: cloudfront__s3.default-root-object-serves-root-but-tenant-directory-rejected|_get_until#12|Timeout|time.sleep(5)
Issue-Key: cloudfront__s3.oai-origin-edge-rejects-a-missing-key-with-access-denied|_get_until#10|Timeout|time.sleep(5)
Issue-Key: cloudfront__s3.unpublished-or-origin-event-function-association-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudfront__s3.bucket-policy-rejects-an-origin-identity-that-does-not-exist|_error#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 0de7718ade8f368be0e47aca52d0dd18a585e082

# cloudfront__s3: distributions over S3 origins serve nothing at the edge, UpdateDistribution accepts an unpublished function association, and S3 PutBucketPolicy accepts an unknown origin-identity principal

> _Note: this issue was identified against the code base at commit `0de7718ade8f368be0e47aca52d0dd18a585e082`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDistribution

## Description
This issue bundles THREE net-new root causes for the cloudfront__s3 service. A fixer must close all three before submitting. The open issue 20261023T140640Z-cloudfront-s3-a-cfn-deployed-distribution-does-not.md belongs to the same service but covers the CFN-deploy/standard-logging path, not these three; none of the checks below is marked ALREADY FILED or OWNED.

Cause 1 of 3: a distribution over an S3 origin serves nothing at its edge domain.
AWS: recorded behavior only. With DefaultRootObject=index.html and an origin-identity/OAC-bound bucket: GET https://<DomainName>/ answers 200 with the bucket-root index.html; GET /<prefix>/index.html answers 200 with that object; GET /<prefix>/ answers 403 (the root object maps only the distribution root); and a GET of a key that does not exist answers 403 with S3's <Code>AccessDenied</Code> body, because the identity cannot ListBucket.
Emulator: the distribution's DomainName answers nothing at all; both checks poll for a first 200 and are killed by the suite's 60 s cap.
Why: the whole OAI/OAC-over-S3 behavior set — root-object mapping, 403-not-404 for missing keys — is unobservable, so no S3-origin CDN workflow can be exercised.
Fix: a data-plane listener for distribution domain names that resolves the behavior/origin and proxies to the S3 origin under the configured identity (new data-plane component alongside the cloudfront service crate).
Traces: cloudfront__s3.default-root-object-serves-root-but-tenant-directory-rejected, cloudfront__s3.oai-origin-edge-rejects-a-missing-key-with-access-denied.

Cause 2 of 3: UpdateDistribution accepts a FunctionAssociation naming a function that is not published.
AWS: Smithy: UpdateDistribution (PUT /2020-05-31/distribution/{Id}/config) models InvalidFunctionAssociation (400) and InvalidArgument (400). Recorded behavior: associating a CloudFront Function still in the DEVELOPMENT stage is refused 400 InvalidArgument with message "The parameter FunctionAssociationArn is invalid; <name> was not found or is not published.", and the behavior's FunctionAssociations stays at Quantity 0.
Emulator: the update succeeds, so there is no error code at all, and the association is stored.
Why: a client wires edge code that does not exist at the edge and gets a 400 only in production; the publish step the real API forces is skippable.
Fix: UpdateDistribution's FunctionAssociations validation against the function store's LIVE stage (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/update_distribution.rs).
Traces: cloudfront__s3.unpublished-or-origin-event-function-association-rejected.

Cause 3 of 3: S3 PutBucketPolicy accepts a bucket policy whose principal is a CloudFront origin-access identity that does not exist.
AWS: recorded behavior only (cross-service reference S3 resolves against CloudFront): a bucket policy naming an unknown OAI canonical-user/principal is refused 400 MalformedPolicy with message "Invalid principal in policy", and the policy is not stored; a real identity in the same position is accepted.
Emulator: PutBucketPolicy succeeds, so there is no error code, and the ghost-principal policy is stored.
Why: a caller believes it has granted the CDN read access to the bucket when the grant names nobody; the misconfiguration surfaces only as 403s at the edge in production.
Fix: S3's PutBucketPolicy principal validation (the policy-document validator in the S3 service crate), which must resolve cloudfront origin-access-identity principals against the CloudFront identity store.
Traces: cloudfront__s3.bucket-policy-rejects-an-origin-identity-that-does-not-exist.

## Evidence
Replay output, verbatim tails:
- cloudfront__s3.default-root-object-serves-root-but-tenant-directory-rejected: Error: Check timed out after 60.0s (hangs in `_get_until(f"{ctx.endpoint_url(domain)}/", lambda st,h,b: st == 200, "the edge serving the root object")`).
- cloudfront__s3.oai-origin-edge-rejects-a-missing-key-with-access-denied: Error: Check timed out after 60.0s (hangs in `_get_until(f"{ctx.endpoint_url(domain)}/page1.html", ...)`; the later assertion would be `st == 403` and `"<Code>AccessDenied</Code>" in body` for a missing key).
- cloudfront__s3.unpublished-or-origin-event-function-association-rejected: AssertionError: the call succeeded; real AWS rejects it, so there is no error code — raised from `_attempt("viewer-request")`, i.e. cf.update_distribution with FunctionAssociations={'Quantity':1,'Items':[{'FunctionARN': <unpublished fn>, 'EventType':'viewer-request'}]}; expected code 'InvalidArgument'.
- cloudfront__s3.bucket-policy-rejects-an-origin-identity-that-does-not-exist: AssertionError: the call succeeded; real AWS rejects it, so there is no error code — raised from s3.put_bucket_policy(Bucket=<bucket>, Policy=<policy naming ghost OAI id>); expected code 'MalformedPolicy', message 'Invalid principal in policy'. The positive control (a real identity) had already succeeded in the same run.

## Steps to Reproduce
1. Create a bucket, seed index.html and page1.html, create an OAI/OAC, create a distribution over the bucket with DefaultRootObject=index.html, wait for Status Deployed.
2. GET https://<DomainName>/ -> nothing ever answers (cause 1).
3. CreateFunction without PublishFunction, then UpdateDistribution with a FunctionAssociations item naming that function at viewer-request -> 200 instead of 400 InvalidArgument (cause 2).
4. PutBucketPolicy with a principal naming a non-existent CloudFront origin-access-identity id -> 200 instead of 400 MalformedPolicy, and GetBucketPolicy shows the ghost id stored (cause 3).
Or replay the four check ids in trace_ids.
