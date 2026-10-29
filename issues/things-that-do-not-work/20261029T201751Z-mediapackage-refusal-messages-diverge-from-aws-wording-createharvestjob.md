Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackage.create-harvest-job-unassumable-role-forbidden mediapackage.list-origin-endpoints-malformed-next-token-rejected
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage.create-harvest-job-unassumable-role-forbidden|run#26|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: mediapackage.list-origin-endpoints-malformed-next-token-rejected|run#15|AssertionError|assert message == "Invalid NextToken", f"expected message 'Invalid NextToken', got {message!r}"
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# MediaPackage refusal MESSAGES diverge from AWS wording: CreateHarvestJob unassumable-role 422 and ListOriginEndpoints invalid-NextToken 422 (2 causes)

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHarvestJob

## Description
Two independent root causes, both in the mediapackage crate, both about the MESSAGE text of an UnprocessableEntityException the emulator already classifies and statuses correctly. In both cases the HTTP status (422) and the error code (UnprocessableEntityException) match AWS; only the `message` member of the error body is the emulator's own invented prose, so clients that match on the message string diverge.

=== Cause 1 of 2: CreateHarvestJob refusal message for a RoleArn MediaPackage cannot assume ===
AWS: Smithy (services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json) models CreateHarvestJob (POST /harvest_jobs, success 200) with error UnprocessableEntityException, errorKind client, httpError 422, and a required S3Destination member (jsonName s3Destination) whose RoleArn (jsonName roleArn) is required. The model does not prescribe message text, so the requirement is recorded behavior only: when the named role exists but its trust policy does not permit mediapackage.amazonaws.com to sts:AssumeRole, MediaPackage refuses synchronously at create time with code UnprocessableEntityException and the message

    s3Destination.roleArn must be assumable by MediaPackage. Please ensure that its trust policy includes mediapackage.amazonaws.com

That string is entirely FIXED text for this error class — it contains no value taken from the request or from state. The member path prefix `s3Destination.roleArn` is the wire (jsonName) path of the offending member, and the service principal `mediapackage.amazonaws.com` is this service's own fixed principal; neither is a per-run value.
Emulator: answers the right status and code but the message

    s3Destination.roleArn names a role mediapackage.amazonaws.com cannot assume: check the trust policy grants it sts:AssumeRole

Why: a client (or an operator's log-matching/alerting, or an SDK-level retry/branch that string-matches a known AWS refusal) that keys on AWS's documented sentence does not recognise the emulator's rewrite, so the misconfigured-trust-policy case is reported as an unknown 422 instead of the actionable "fix the trust policy to include mediapackage.amazonaws.com" condition. The refusal itself is correct, so this is purely a fidelity defect in the message.
Fix: the message construction for the unassumable-role refusal in the CreateHarvestJob handler's destination pre-flight (the path that consults the IAM role trust policy), in the mediapackage crate's create_harvest_job operation handler / its validation helper in state.rs. Only the literal message text needs to become AWS's sentence; the status, code, and the no-job-persisted behavior the check also asserts are already right (the check never reached its DescribeHarvestJob assertion, so the leak assertion is unverified by this run and is NOT part of this cause).
Related-but-distinct open issue: 20261024T010718Z-mediapackage-createharvestjob-validation.md covers CreateHarvestJob NOT validating the destination bucket and NOT refusing a RoleArn that names no role at all. That issue is about missing validation; this cause is about the wording of the validation that IS implemented. Do not close one with the other.
Traces: mediapackage.create-harvest-job-unassumable-role-forbidden

=== Cause 2 of 2: ListOriginEndpoints refusal message for a NextToken the service never issued ===
AWS: Smithy models ListOriginEndpoints (GET /origin_endpoints, success 200) as paginated with inputToken/outputToken NextToken (httpQuery nextToken), pageSize MaxResults (range 1..1000), and error UnprocessableEntityException, errorKind client, httpError 422. The model does not prescribe message text, so recorded behavior only: a nextToken value that is not an opaque token this API issued is refused with code UnprocessableEntityException and the message

    Invalid NextToken

That whole message is FIXED text — AWS does not echo the offending token or any other request/state value into it.
Emulator: right status and code, but the message

    nextToken is not a pagination token this service issued

Why: the same string-matching break as cause 1, on a much more commonly handled condition — code that detects a stale/corrupt pagination cursor by matching AWS's short "Invalid NextToken" sentence falls through to generic error handling against the emulator, so pagination-resumption logic cannot be exercised faithfully.
Fix: the invalid-pagination-token refusal message in the ListOriginEndpoints handler (or the shared pagination-token decode helper the mediapackage crate's list operations call); emit AWS's fixed text verbatim rather than a descriptive paraphrase. If the paraphrase is produced by a shared token decoder, every list operation in the crate inherits the divergence and the fix belongs there. The positive control in the same check (unpaginated ListOriginEndpoints returning the just-created endpoint) PASSED, so listing and the refusal decision are correct; only the text diverges.
Traces: mediapackage.list-origin-endpoints-malformed-next-token-rejected

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), 51/58 passed; mediapackage 51/58 (88%), 7 failed. Five of the seven are marked OWNED by 20261022T213438Z-mediapackage-data-plane-hls-ingest-url-is-unrouted.md and were skipped. The two remaining mismatches are both message-text divergences:

Cause 1 — check mediapackage.create-harvest-job-unassumable-role-forbidden, check_mediapackage_create_harvest_job_unassumable_role_forbidden.py line 101:
  assert message == expected_message
  AssertionError: expected message 's3Destination.roleArn must be assumable by MediaPackage. Please ensure that its trust policy includes mediapackage.amazonaws.com', got 's3Destination.roleArn names a role mediapackage.amazonaws.com cannot assume: check the trust policy grants it sts:AssumeRole'
  The preceding assertion `code == "UnprocessableEntityException"` PASSED, so status 422 / code are correct and the divergence is isolated to the message.
  Expected (AWS): HTTP 422, Code=UnprocessableEntityException, Message='s3Destination.roleArn must be assumable by MediaPackage. Please ensure that its trust policy includes mediapackage.amazonaws.com' (all fixed text; no request- or state-derived parts).
  Actual (emulator): HTTP 422, Code=UnprocessableEntityException, Message='s3Destination.roleArn names a role mediapackage.amazonaws.com cannot assume: check the trust policy grants it sts:AssumeRole'.

Cause 2 — check mediapackage.list-origin-endpoints-malformed-next-token-rejected, check_mediapackage_list_origin_endpoints_malformed_next_token_rejected.py line 47:
  assert message == "Invalid NextToken"
  AssertionError: expected message 'Invalid NextToken', got 'nextToken is not a pagination token this service issued'
  The preceding assertions (baseline unpaginated list contains the created endpoint; code == "UnprocessableEntityException") PASSED.
  Expected (AWS): HTTP 422, Code=UnprocessableEntityException, Message='Invalid NextToken' (entirely fixed text).
  Actual (emulator): HTTP 422, Code=UnprocessableEntityException, Message='nextToken is not a pagination token this service issued'.

Model grounding (smithy_query query="operation" on services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json): CreateHarvestJob errors include UnprocessableEntityException (client, httpError 422); input S3Destination required, jsonName s3Destination. ListOriginEndpoints errors include UnprocessableEntityException (client, httpError 422); paginated {inputToken: NextToken, outputToken: NextToken, items: OriginEndpoints, pageSize: MaxResults}; NextToken bound to httpQuery nextToken. Neither operation's model constrains the message text, hence "recorded behavior only" for the exact strings.

## Steps to Reproduce
Cause 1 (CreateHarvestJob unassumable-role message):
1. Boot the emulator; point boto3 at it with clients for mediapackage, iam and s3 (any region, e.g. us-west-2).
2. Create an S3 bucket for the destination (with CreateBucketConfiguration.LocationConstraint set outside us-east-1).
3. iam.create_role with an AssumeRolePolicyDocument whose only statement allows Principal Service s3.amazonaws.com to sts:AssumeRole — i.e. the role EXISTS but does not trust mediapackage.amazonaws.com. Keep its Arn.
4. mp.create_channel(Id=<chan>); mp.create_origin_endpoint(Id=<ep>, ChannelId=<chan>, HlsPackage={"SegmentDurationSeconds": 6}, StartoverWindowSeconds=3600).
5. mp.create_harvest_job(Id=<job>, OriginEndpointId=<ep>, StartTime=<10 min ago>, EndTime=<5 min ago>, S3Destination={"BucketName": <bucket>, "ManifestKey": "bad/index.m3u8", "RoleArn": <the s3-only-trust role arn>}).
6. Catch the ClientError. Error.Code is UnprocessableEntityException (correct, 422), but Error.Message is 's3Destination.roleArn names a role mediapackage.amazonaws.com cannot assume: check the trust policy grants it sts:AssumeRole' instead of AWS's 's3Destination.roleArn must be assumable by MediaPackage. Please ensure that its trust policy includes mediapackage.amazonaws.com'.

Cause 2 (ListOriginEndpoints invalid-NextToken message):
1. Boot the emulator; mp.create_channel(Id=<chan>); mp.create_origin_endpoint(Id=<ep>, ChannelId=<chan>, HlsPackage={}).
2. Positive control: mp.list_origin_endpoints(ChannelId=<chan>) succeeds and includes <ep>.
3. mp.list_origin_endpoints(ChannelId=<chan>, NextToken="not-a-real-token!!") — a value the API never issued.
4. Catch the ClientError. Error.Code is UnprocessableEntityException (correct, 422), but Error.Message is 'nextToken is not a pagination token this service issued' instead of AWS's 'Invalid NextToken'.

Re-replay to confirm the fix: mediapackage.create-harvest-job-unassumable-role-forbidden and mediapackage.list-origin-endpoints-malformed-next-token-rejected.
