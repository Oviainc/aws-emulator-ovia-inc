Issue-Source: trace-sweep-divergence
Issue-Trace: s3-create-bucket-namespace-header-and-account-mismatches
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-create-bucket-namespace-header-and-account-mismatches
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|CreateBucket|4xx->2xx|$
Issue-Key: s3|CreateBucket|4xx->2xx|$headers.location
Issue-Key: s3|CreateBucket|4xx->2xx|status
Issue-Key: s3|HeadBucket|4xx->2xx|status
Issue-Key: s3|CreateBucket|2xx->4xx|$headers.location
Issue-Key: s3|CreateBucket|2xx->4xx|$headers.x-amz-bucket-arn
Issue-Key: s3|CreateBucket|2xx->4xx|status
Issue-Commit: f0b2a1a4859373de2ddca21e0cf7a4e6a954ae11

# s3 account-regional general purpose bucket namespace is not implemented: CreateBucket neither requires nor validates x-amz-bucket-namespace and never returns the bucket ARN

> _Note: this issue was identified against the code base at commit `f0b2a1a4859373de2ddca21e0cf7a4e6a954ae11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBucket

## Description
The emulator has no notion of the general purpose bucket *account regional namespace*. It ignores the x-amz-bucket-namespace header entirely: it does not detect an account-regional bucket name, does not require the header, does not validate the account id embedded in the name against the caller, and does not emit the bucket ARN on a successful account-regional create. Four distinct causes, all in one CreateBucket/HeadBucket path.

Cause 1 of 4 — CreateBucket on an account-regional bucket name with NO x-amz-bucket-namespace header is accepted instead of refused.
AWS: Smithy: CreateBucket input member BucketNamespace is bound to httpHeader x-amz-bucket-namespace, target enum BucketNamespace with values [account-regional, global]; its documentation states that account-regional buckets "must follow the format bucket-name-prefix-accountId-region-an". When the requested name has that account-regional form but the request carries no x-amz-bucket-namespace header, AWS refuses with HTTP 400 and the S3 error envelope Code 'MissingNamespaceHeader', Message 'The requested bucket is an account-regional namespace bucket, but your request is missing the required x-amz-bucket-namespace header.' plus an error element Header whose value is the missing header name, 'x-amz-bucket-namespace' (recorded response; the code/message are not modelled, so this is recorded behavior for the message text). The whole sentence is fixed text — nothing in it is interpolated; the only variable part of the envelope is the Header element (the header name) and the per-request RequestId/HostId.
Emulator: answers HTTP 200 with an empty body, creates the bucket, and returns a Location header for it.
Why: a client that relies on AWS to reject a half-specified account-regional create instead gets a bucket created in the shared global namespace under an account-regional name. It believes it holds an account-regional bucket that has none of the namespace semantics, and the name is now consumed, so the correctly-formed request that follows fails.
Fix: CreateBucket's request validation — detect the account-regional name form and require x-amz-bucket-namespace before the bucket is created.
Traces: s3-create-bucket-namespace-header-and-account-mismatches#task#1.

Cause 2 of 4 — HeadBucket answers 200 for an account-regional bucket name that was never legitimately created.
AWS: HeadBucket on a bucket that does not exist answers HTTP 404 with no body (recorded behavior; HeadBucket models NotFound as a 404 client error).
Emulator: answers HTTP 200, reporting the bucket as existing, because the create in cause 1 was wrongly accepted.
Why: existence probes are the normal way a client decides whether to create or adopt a bucket. A 200 here tells the client an account-regional bucket exists when AWS has none, so it skips creation and then fails on every namespace-dependent call.
Fix: the same CreateBucket validation as cause 1 (so no bucket is in state), and HeadBucket's lookup must not resolve a name the service never accepted.
Traces: s3-create-bucket-namespace-header-and-account-mismatches#task#2.

Cause 3 of 4 — CreateBucket with x-amz-bucket-namespace: account-regional and a FOREIGN account id in the bucket name is accepted instead of refused.
AWS: an account-regional bucket name embeds the caller's own 12-digit account id (Smithy: CreateBucketRequest$BucketNamespace documentation, format bucket-name-prefix-accountId-region-an). When the id in the name is not the caller's, AWS refuses with HTTP 400 and Code 'InvalidBucketNamespace', Message "The requested bucket is an account-regional namespace bucket, but the requested AWS Account ID '<account-id-in-bucket-name>' does not match the caller's AWS Account ID '<caller-account-id>'. Specify the caller's AWS Account ID in the bucket name.", plus an error element BucketNamespace whose value is the requested bucket name (recorded response). The fixed text is everything outside the two quoted ids and the BucketNamespace element value; the account id in the name comes from the request, the caller's account id from the signing credentials, and the BucketNamespace element from the request's bucket name. The recorded ids/names are fillers, not requirements.
Emulator: answers HTTP 200 with an empty body and creates the bucket.
Why: the emulator lets any caller create a bucket inside another account's regional namespace, which is precisely the isolation the feature exists to provide. A client (or a test of cross-account isolation) sees the create succeed where AWS fails it.
Fix: CreateBucket's validation — compare the account id parsed from the account-regional bucket name against the caller's account id.
Traces: s3-create-bucket-namespace-header-and-account-mismatches#task#3.

Cause 4 of 4 — the legitimate account-regional create fails 409 and, when it succeeds, carries neither the path-form Location nor the x-amz-bucket-arn header.
AWS: CreateBucket with x-amz-bucket-namespace: account-regional and a name embedding the caller's own account id answers HTTP 200. Smithy: CreateBucketOutput members Location (httpHeader Location) and BucketArn (httpHeader x-amz-bucket-arn, target S3RegionalOrS3ExpressBucketArnString, pattern ^arn:[^:]+:(s3|s3express):, length 1-128). For an account-regional create the recorded Location is the bucket PATH form '/<bucket-name>' — not a virtual-hosted URL — and x-amz-bucket-arn is 'arn:aws:s3:::<bucket-name>'. Both values are derived from the request's bucket name.
Emulator: answers HTTP 409 (it reports the name as already taken, because the cause-1 create of the same name was wrongly accepted) and emits neither the Location nor the x-amz-bucket-arn response header. Even on the success path the emulator has no x-amz-bucket-arn: the output header is never serialized.
Why: the one request AWS accepts is the one the emulator rejects, so an account-regional bucket cannot be created at all; and a client that reads the new bucket's ARN out of the CreateBucket response (the documented way to get it without a second call) gets nothing.
Fix: CreateBucket's handler and its response serializer — gate creation on the namespace rules above, and emit Location in path form plus the x-amz-bucket-arn output header for an account-regional bucket.
Traces: s3-create-bucket-namespace-header-and-account-mismatches#task#4.

## Evidence
Fixture s3-create-bucket-namespace-header-and-account-mismatches, replayed against the emulator (service s3):

- task#1 CreateBucket (no x-amz-bucket-namespace, account-regional-form name): expected status 400 with body {'Error': {'Code': 'MissingNamespaceHeader', 'Message': 'The requested bucket is an account-regional namespace bucket, but your request is missing the required x-amz-bucket-namespace header.', 'Header': 'x-amz-bucket-namespace', 'RequestId': ..., 'HostId': ...}}; actual status 200, empty body, and a Location response header was emitted where AWS sends none.
- task#2 HeadBucket: expected status 404, actual status 200.
- task#3 CreateBucket (x-amz-bucket-namespace: account-regional, bucket name embedding account id 999999999999 while the caller is 000000000000): expected status 400 with body {'Error': {'Code': 'InvalidBucketNamespace', 'Message': "The requested bucket is an account-regional namespace bucket, but the requested AWS Account ID '999999999999' does not match the caller's AWS Account ID '000000000000'. Specify the caller's AWS Account ID in the bucket name.", 'BucketNamespace': '<requested bucket name>', ...}}; actual status 200 with an empty body.
- task#4 CreateBucket (header present, caller's own account id in the name): expected status 200 with headers location='/<bucket-name>' and x-amz-bucket-arn='arn:aws:s3:::<bucket-name>'; actual status 409 with both headers missing.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture s3-create-bucket-namespace-header-and-account-mismatches, steps task#1 through task#4, diverge as above.
3. By hand: PUT /<prefix>-<your-account-id>-<region>-an with no x-amz-bucket-namespace header -> emulator 200, AWS 400 MissingNamespaceHeader. Repeat with x-amz-bucket-namespace: account-regional and some other account id in the name -> emulator 200, AWS 400 InvalidBucketNamespace. Then PUT the same name with the header and your own account id -> emulator 409 and no x-amz-bucket-arn header, AWS 200 with Location '/<bucket>' and x-amz-bucket-arn.
