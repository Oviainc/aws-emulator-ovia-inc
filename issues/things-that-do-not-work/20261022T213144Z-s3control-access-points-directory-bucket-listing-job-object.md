Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.access-point-alias-and-endpoints-survive-policy-churn s3control.access-point-vpc-origin-denies-internet-read-rejected s3control.access-point-on-nonexistent-bucket-rejected s3control.access-point-delete-severs-data-path-and-keeps-objects s3control.access-point-prefix-scoped-policy-denies-object-outside-prefix-rejected s3control.bucket-policy-delegating-to-access-point-denies-direct-path-rejected s3control.s3c-access-point-policy-explicit-deny-overrides-bucket-allow-rejected s3control.directory-bucket-access-point-listing-excludes-general-purpose s3control.access-point-scope-does-not-answer-for-general-purpose-access-point-rejected s3control.batch-job-describe-and-tagging-bogus-id-rejected s3control.object-lambda-namespace-does-not-resolve-general-purpose-access-point-rejected s3control.untag-resource-removes-specific-bucket-tag-keys
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control.access-point-alias-and-endpoints-survive-policy-churn|run#18|KeyError|assert got["BucketAccountId"] == account, (
Issue-Key: s3control.access-point-vpc-origin-denies-internet-read-rejected|run#20|KeyError|assert re.fullmatch(r"[A-Za-z0-9.-]+(:\d+)?", described["Endpoints"]["ipv4"]), (
Issue-Key: s3control.access-point-on-nonexistent-bucket-rejected|_error#9|AssertionError|raise AssertionError("call unexpectedly succeeded; an error was expected")
Issue-Key: s3control|GetObject|NoSuchBucket
Issue-Key: s3control.s3c-access-point-policy-explicit-deny-overrides-bucket-allow-rejected|run#22|AssertionError|assert denied == "AccessDenied", (
Issue-Key: s3-control|ListAccessPointsForDirectoryBuckets|404
Issue-Key: s3control.batch-job-describe-and-tagging-bogus-id-rejected|run#9|AssertionError|assert code == "InvalidRequest", (
Issue-Key: s3control.object-lambda-namespace-does-not-resolve-general-purpose-access-point-rejected|run#17|AssertionError|assert code == "NoSuchAccessPoint", (
Issue-Key: s3-control|TagResource|ValidationException
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# s3control access points, directory-bucket listing, job/object-lambda lookups and bucket tagging diverge (7 causes)

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessPoint, CreateAccessPoint, ListAccessPointsForDirectoryBuckets, DescribeJob, GetAccessPointForObjectLambda, TagResource/UntagResource/ListTagsForResource (plus S3 GetObject addressed by access point ARN)

## Description
Seven independent root causes in the s3control emulator, all grounded in failing behavioral checks of this suite. Each must be closed before this issue is submitted. No existing open issue covers any of them (the suite marked the GetAccessGrantsInstance / DeletePublicAccessBlock / CreateJob / CreateMultiRegionAccessPoint / GetPublicAccessBlock / PutStorageLensConfiguration / CreateStorageLensGroup stub failures ALREADY FILED, and s3c-batch-copy-job OWNED by 20261022T154846Z-emr-glue-s3-cross-service-suite-cfn-declared.md; those are deliberately excluded here).

Cause 1 of 7: GetAccessPoint omits Endpoints and BucketAccountId.
AWS: Smithy GetAccessPoint (GET /v20180820/accesspoint/{Name}, 200) output GetAccessPointResult declares Name, Bucket, NetworkOrigin, VpcConfiguration, PublicAccessBlockConfiguration, CreationDate, Alias, AccessPointArn, Endpoints (map), BucketAccountId (AccountId, pattern ^\d{12}$), DataSourceId, DataSourceType. A general-purpose access point answer carries the endpoint map and the owning account of the underlying bucket.
Emulator: the response has NetworkOrigin and Bucket but no 'Endpoints' key and no 'BucketAccountId' key — the SDK dict lookup raises KeyError: 'BucketAccountId' and KeyError: 'Endpoints'.
Why: a client that resolves the access point's regional/dualstack/FIPS hostnames out of Endpoints, or that confirms cross-account bucket ownership from BucketAccountId, crashes on a missing key instead of reading a modeled member.
Fix: the GetAccessPoint handler/serializer and the access-point state record that must retain the bucket's owner account and synthesize the endpoint map.
Traces: s3control.access-point-alias-and-endpoints-survive-policy-churn, s3control.access-point-vpc-origin-denies-internet-read-rejected.

Cause 2 of 7: CreateAccessPoint does not validate that the target bucket exists.
AWS: recorded behavior only (the Smithy model binds no errors to CreateAccessPoint). Creating an access point over a bucket that never existed — or over one deleted a moment earlier — is refused with error code InvalidRequest, HTTP 400 (example message: "Amazon S3 AccessPoint can only be created for existing bucket"), and nothing is recorded: the name is absent from ListAccessPoints and GetAccessPoint answers NoSuchAccessPoint 404. The validation reads live bucket state.
Emulator: the call SUCCEEDS ("call unexpectedly succeeded; an error was expected") and registers an access point bound to a non-existent bucket.
Why: a client's negative path never fires, and the emulator accumulates access points whose data path can never work; a just-deleted bucket is indistinguishable from a live one.
Fix: pre-flight bucket-existence validation in the CreateAccessPoint handler, before any state mutation.
Traces: s3control.access-point-on-nonexistent-bucket-rejected.

Cause 3 of 7: an access point ARN is not resolvable as an S3 data-plane bucket name, so the whole access-point data path (and every policy evaluated on it) is dead.
AWS: recorded behavior only (S3 endpoint resolution for access point ARNs). GetObject with Bucket set to arn:aws:s3:<region>:<account>:accesspoint/<name> routes through the access point and returns the underlying object's exact bytes (200); once the access point is deleted the same call becomes error code NoSuchAccessPoint (4xx), while the object stays readable on the bucket path.
Emulator: every read addressed by access point ARN fails with error code NoSuchBucket (message rendered as "Unknown") both before and after the delete, i.e. the ARN form is parsed as a literal bucket name.
Why: access points cannot be used as the alternate path they are for; and because the request never reaches an access point, none of the authorization behavior layered on it is observable — access point policy explicit Deny beating a bucket Allow, prefix-scoped policies, VPC-origin implicit deny, and bucket policies delegating via s3:DataAccessPointAccount/s3:DataAccessPointArn all silently do nothing. Expect further divergences behind this one once ARN addressing resolves.
Fix: access point ARN/alias resolution in the S3 data-plane bucket resolver, feeding the access-point registry owned by s3control, plus access-point policy evaluation on that path.
Traces: s3control.access-point-delete-severs-data-path-and-keeps-objects, s3control.access-point-prefix-scoped-policy-denies-object-outside-prefix-rejected, s3control.bucket-policy-delegating-to-access-point-denies-direct-path-rejected, s3control.s3c-access-point-policy-explicit-deny-overrides-bucket-allow-rejected.

Cause 4 of 7: ListAccessPointsForDirectoryBuckets is not routed.
AWS: Smithy ListAccessPointsForDirectoryBuckets — GET /v20180820/accesspointfordirectory, success code 200, output AccessPointList + NextToken, paginated (inputToken/outputToken NextToken, items AccessPointList, pageSize MaxResults). With no directory-bucket access points it answers 200 with an empty list; general-purpose access points must never appear in it.
Emulator: bare HTTP 404 with reason "Not Found" and no service error body — botocore surfaces the code as "404", so the operation is unreachable rather than wrong.
Why: a client cannot distinguish "no directory-bucket access points" from "operation absent", and the partition between the two access-point collections is unobservable.
Fix: route registration/handler for ListAccessPointsForDirectoryBuckets in the s3control operation table.
Traces: s3control.directory-bucket-access-point-listing-excludes-general-purpose, s3control.access-point-scope-does-not-answer-for-general-purpose-access-point-rejected.

Cause 5 of 7: DescribeJob answers NotImplemented for an unknown job id instead of rejecting it.
AWS: Smithy DescribeJob (GET /v20180820/jobs/{JobId}) binds BadRequestException, NotFoundException, TooManyRequestsException (all client, 400) and InternalServiceException (500); recorded behavior is that an unknown-but-well-formed job id comes back as error code InvalidRequest with HTTP 400 (example message "Job not found"), not as the declared NotFoundException.
Emulator: error code 'NotImplemented'.
Why: a client that branches on InvalidRequest/400 to mean "no such job" treats a normal negative answer as an emulator/service outage, and the job surface cannot be probed at all.
Fix: the DescribeJob handler's job lookup and its not-found error mapping (the job-id surface should reject uniformly across DescribeJob and the job tagging operations).
Traces: s3control.batch-job-describe-and-tagging-bogus-id-rejected.

Cause 6 of 7: GetAccessPointForObjectLambda answers NotImplemented instead of a namespace miss.
AWS: Smithy GetAccessPointForObjectLambda — GET /v20180820/accesspointforobjectlambda/{Name}, 200, output Name/PublicAccessBlockConfiguration/CreationDate/Alias. Recorded behavior: a general-purpose access point name does not exist in the object-lambda namespace, so the lookup is refused with error code NoSuchAccessPoint, HTTP 404 (example message "The specified accesspoint does not exist").
Emulator: error code 'NotImplemented'.
Why: the two namespaces are not separated — a client cannot tell that an object-lambda access point is absent, and a name collision between the two surfaces is undetectable.
Fix: the GetAccessPointForObjectLambda handler plus its NoSuchAccessPoint mapping over the object-lambda registry.
Traces: s3control.object-lambda-namespace-does-not-resolve-general-purpose-access-point-rejected.

Cause 7 of 7: generic resource tagging refuses a general-purpose bucket ARN, and does so with a JSON-protocol-style ValidationException.
AWS: recorded behavior only — TagResource/ListTagsForResource/UntagResource accept ResourceArn=arn:aws:s3:::<bucket> and maintain that bucket's tag set (UntagResource removing only the named keys, leaving siblings by value). Note the model tension worth resolving in the fix: Smithy TagResource.ResourceArn (S3ResourceArn) carries pattern ^arn:[^:]+:s3(express)?:[^:] and its doc names "a directory bucket, S3 Storage Lens group or S3 Access Grants instance, registered location, or grant". Independently of whether the ARN is accepted, s3control is a REST-XML surface whose rejections are XML <Error><Code>…</Code> documents (InvalidRequest/InvalidArgument, 400); a ValidationException with an "N validation errors detected … failed to satisfy constraint" body is a JSON-protocol shape foreign to this service.
Emulator: TagResource(ResourceArn="arn:aws:s3:::<bucket>") fails with error code ValidationException, example message "1 validation error detected: Value 'arn:aws:s3:::sim2real-behav-s3cuntag-…' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]+:s3(express)?:[^:]".
Why: bucket tag sets cannot be written or read through s3control's generic tagging surface at all, and a client that handles s3control's XML error codes never matches the ValidationException code it is given.
Fix: ResourceArn parsing/validation for the tagging handlers (accept the general-purpose bucket ARN form and dispatch it to the bucket tag store) and the s3control error serializer, which must emit this service's XML error envelope rather than a JSON ValidationException.
Traces: s3control.untag-resource-removes-specific-bucket-tag-keys.

## Evidence
Suite: simparity-behavioral-aws-v1 — s3control 1/35 passed (3%).

Cause 1: KeyError: 'BucketAccountId' at check_s3control_access_point_alias_and_endpoints_survive_policy_churn.py:54 (assert got["BucketAccountId"] == account, where got = s3c.get_access_point(...)); KeyError: 'Endpoints' at check_s3control_access_point_vpc_origin_denies_internet_read_rejected.py:62 (described["Endpoints"]["ipv4"], described = s3c.get_access_point(...)). Expected per Smithy GetAccessPointResult: Endpoints (map) and BucketAccountId present. NetworkOrigin=='VPC' asserted immediately before did pass.

Cause 2: AssertionError: call unexpectedly succeeded; an error was expected — at check_s3control_access_point_on_nonexistent_bucket_rejected.py:34, s3c.create_access_point(AccountId=…, Name=…, Bucket=<never-created bucket>). Expected code InvalidRequest / HTTP 400, example message "Amazon S3 AccessPoint can only be created for existing bucket"; the same check then requires the name to be absent from ListAccessPoints and GetAccessPoint to answer NoSuchAccessPoint.

Cause 3: botocore.errorfactory.NoSuchBucket: An error occurred (NoSuchBucket) when calling the GetObject operation: Unknown — raised on s3.get_object(Bucket=ap_arn, Key=…) where ap_arn is the ARN returned by CreateAccessPoint (arn:aws:s3:<region>:<account>:accesspoint/<name>), expected to return the object bytes. Also: "reading 'locked/secret.txt' through the access point produced 'NoSuchBucket', expected 'AccessDenied' from the access point policy's explicit Deny" (check_s3control_s3c_access_point_policy_explicit_deny_overrides_bucket_allow_rejected.py:125).

Cause 4: botocore.exceptions.ClientError: An error occurred (404) when calling the ListAccessPointsForDirectoryBuckets operation: Not Found. Expected: HTTP 200 with AccessPointList (empty when there are none).

Cause 5: AssertionError: DescribeJob on an unknown job id returned 'NotImplemented', expected InvalidRequest (s3control answers 'Job not found' as InvalidRequest 400, not the declared NotFoundException) — check_s3control_batch_job_describe_and_tagging_bogus_id_rejected.py:45.

Cause 6: AssertionError: GetAccessPointForObjectLambda on the general-purpose name 'sim2real-behav-olns-…' returned 'NotImplemented', expected NoSuchAccessPoint — check_s3control_object_lambda_namespace_does_not_resolve_general_purpose_access_point_rejected.py:108.

Cause 7: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the TagResource operation: 1 validation error detected: Value 'arn:aws:s3:::sim2real-behav-s3cuntag-39e57af76a13' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]+:s3(express)?:[^:] — raised on the first TagResource call of the check, so the untag semantics behind it were never exercised.

## Steps to Reproduce
Replay the behavioral suite against the emulator and re-run exactly the checks in trace_ids:

1. Cause 1 — create a bucket, s3control CreateAccessPoint over it, then GetAccessPoint(AccountId, Name) and read Endpoints and BucketAccountId from the response.
2. Cause 2 — s3control CreateAccessPoint(AccountId, Name, Bucket=<a bucket name that was never created>), then the same against a bucket created and immediately deleted; expect InvalidRequest 400 both times and no access point registered.
3. Cause 3 — create a bucket, PutObject keep/me.txt, CreateAccessPoint, then s3 GetObject(Bucket=<AccessPointArn>, Key="keep/me.txt"); expect the object bytes. Then DeleteAccessPoint and repeat; expect NoSuchAccessPoint (4xx), while GetObject on the bucket still serves the object.
4. Cause 4 — s3control ListAccessPointsForDirectoryBuckets(AccountId); expect 200 and an AccessPointList that excludes general-purpose access points.
5. Cause 5 — s3control DescribeJob(AccountId, JobId="00000000-0000-0000-0000-000000000000"-shaped unknown id); expect InvalidRequest 400.
6. Cause 6 — CreateAccessPoint with a general-purpose name, then GetAccessPointForObjectLambda(AccountId, Name=<that same name>); expect NoSuchAccessPoint 404.
7. Cause 7 — create a bucket, s3control TagResource(AccountId, ResourceArn="arn:aws:s3:::<bucket>", Tags=[Keep=me, Remove=me]), ListTagsForResource, UntagResource(TagKeys=["Remove"]), ListTagsForResource; expect only Keep=me to survive.
