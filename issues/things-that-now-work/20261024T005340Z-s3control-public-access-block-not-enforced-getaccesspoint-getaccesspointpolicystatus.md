Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.access-point-public-policy-blocked-by-own-block-public-policy-rejected s3control.account-public-access-block-blocks-public-access-point-policy-rejected s3control.account-public-access-block-restrict-denies-anonymous-object-read-rejected s3control.access-point-policy-status-derives-ispublic s3control.multi-region-access-point-create-and-delete-are-async-operations s3control.multi-region-access-point-duplicate-name-create-races-rejected s3control.s3c-batch-copy-job-copies-manifest-objects-and-leaves-the-rest s3control.s3c-batch-generated-manifest-filter-limits-job-to-matching-objects s3control.batch-job-completion-report-accounts-for-mixed-task-outcomes
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control.access-point-public-policy-blocked-by-own-block-public-policy-rejected|run#12|KeyError|default_pab = s3c.get_access_point(AccountId=account, Name=blocked)[
Issue-Key: s3control.account-public-access-block-blocks-public-access-point-policy-rejected|run#18|Timeout|time.sleep(2)
Issue-Key: s3control.account-public-access-block-restrict-denies-anonymous-object-read-rejected|run#14|Timeout|time.sleep(3)
Issue-Key: s3control.access-point-policy-status-derives-ispublic|run#14|AssertionError|assert code == "NoSuchAccessPointPolicy", (
Issue-Key: s3control.multi-region-access-point-create-and-delete-are-async-operations|run#22|AssertionError|assert regions == {(bucket_west, "us-west-2"), (bucket_east, "us-east-1")}, (
Issue-Key: s3-control|CreateMultiRegionAccessPoint|MultiRegionAccessPointAlreadyOwnedByYou
Issue-Key: s3control.s3c-batch-copy-job-copies-manifest-objects-and-leaves-the-rest|_poll_terminal#19|Timeout|time.sleep(4)
Issue-Key: s3control.s3c-batch-generated-manifest-filter-limits-job-to-matching-objects|_poll_terminal#15|Timeout|time.sleep(4)
Issue-Key: s3control.batch-job-completion-report-accounts-for-mixed-task-outcomes|_poll_terminal#18|Timeout|time.sleep(3)
Issue-Commit: a1b03c4f6e81626c353b3ff836c908b12c596c5c

# s3control: public-access-block not enforced, GetAccessPoint/GetAccessPointPolicyStatus incomplete, MRAP region+race semantics wrong, batch jobs never settle (7 causes)

> _Note: this issue was identified against the code base at commit `a1b03c4f6e81626c353b3ff836c908b12c596c5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessPoint, PutAccessPointPolicy, GetAccessPointPolicyStatus, PutPublicAccessBlock, GetMultiRegionAccessPoint, CreateMultiRegionAccessPoint, CreateJob/DescribeJob

## Description
Seven independent root causes in the s3control emulator, all grounded in failing behavioral checks of suite simparity-behavioral-ga-prod-v1. Each must be closed before this issue is submitted. Checks the suite marked ALREADY FILED (CreateAccessGrantsLocation, GetBucketPolicyStatus, UpdateJobPriority, GetJobTagging, ListStorageLensConfigurations, UpdateStorageLensGroup stubs) and those marked OWNED by 20261022T213144Z-s3control-access-points-directory-bucket-listing-job-object.md are deliberately excluded. Cause 1 touches the same handler as Cause 1 of that open issue (GetAccessPoint omitting Endpoints/BucketAccountId) but is a different omitted member and was not marked owned, so it is filed here.

Cause 1 of 7: GetAccessPoint omits PublicAccessBlockConfiguration for an access point created without an explicit one.
AWS: Smithy GetAccessPoint (GET /v20180820/accesspoint/{Name}, 200) output GetAccessPointResult declares PublicAccessBlockConfiguration (structure PublicAccessBlockConfiguration: BlockPublicAcls, IgnorePublicAcls, BlockPublicPolicy, RestrictPublicBuckets). Recorded behavior: an access point created with no PublicAccessBlockConfiguration still reports one on read — its four flags defaulted on — so the member is always present in the answer.
Emulator: when CreateAccessPoint was called WITHOUT PublicAccessBlockConfiguration, the GetAccessPoint response has no 'PublicAccessBlockConfiguration' key at all and the SDK dict lookup raises KeyError: 'PublicAccessBlockConfiguration'. (When the member IS supplied at create it is echoed back — the sibling check at account-public-access-block-blocks-public-access-point-policy-rejected read it successfully — so the gap is the defaulted record, not the serializer path.)
Why: a client that reads an access point's effective block settings before deciding whether a policy will be accepted crashes on a missing modeled member instead of seeing the service's defaults, and the access point's default-secure posture is invisible.
Fix: the access-point state record (apply the default all-true PublicAccessBlockConfiguration at create time) and the GetAccessPoint handler/serializer that must always emit it.
Traces: s3control.access-point-public-policy-blocked-by-own-block-public-policy-rejected.

Cause 2 of 7: the account-level PublicAccessBlock does not block a public access point policy.
AWS: Smithy PutAccessPointPolicy (PUT /v20180820/accesspoint/{Name}/policy, 200) and PutPublicAccessBlock; recorded behavior: with the account-level PublicAccessBlockConfiguration set to all four flags true, PutAccessPointPolicy of a document whose Statement allows Principal {"AWS":"*"} on <ap-arn>/object/* is refused with error code AccessDenied, HTTP 403 (example message "Access Denied"), the account setting outranking the access point's own BlockPublicPolicy=false; no policy is stored (GetAccessPointPolicy then answers NoSuchAccessPointPolicy, 404), and the identical call succeeds the moment DeletePublicAccessBlock removes the account record.
Emulator: the public policy is accepted. The check polls the rejection for 30 attempts at 2s (deleting any policy that slipped through) and never observes one, so the check is killed at the suite's 60s budget — it never reaches the reversal half.
Why: the whole point of an account-level block is that it outranks per-resource settings; a client testing its guardrails sees a public access point policy accepted where AWS refuses it, which is a security-semantics divergence, not a cosmetic one.
Fix: evaluate the account-level PublicAccessBlock record in the PutAccessPointPolicy handler's validation (public-policy detection), before the policy is stored.
Traces: s3control.account-public-access-block-blocks-public-access-point-policy-rejected.

Cause 3 of 7: RestrictPublicBuckets has no observable effect on the anonymous data path.
AWS: recorded behavior only (Smithy models PutPublicAccessBlock's 200 and the flag's semantics in PublicAccessBlockConfiguration.RestrictPublicBuckets docs: "restricts access to buckets with public policies to only ... authorized users within this account"). Recorded: with a public bucket policy in place, an UNSIGNED GetObject reads the object (200); after PutPublicAccessBlock(AccountId) with RestrictPublicBuckets=true (BlockPublicPolicy left false), the same unsigned read is refused with error code AccessDenied (example message "Access Denied"), while the owner's signed read still succeeds and GetBucketPolicy still returns the identical document — the flag suppresses the effect without editing the policy.
Emulator: the check never observes the state change it polls for: both arms are bounded 20-attempt/3s loops (baseline anonymous read, then the post-restriction denial), and the check is killed at the suite's 60s budget, so either the anonymous read of a public-policy bucket never succeeds or it never stops succeeding under RestrictPublicBuckets. Either way the account-level flag is not evaluated on the data path.
Why: a client that relies on the account block to shut off anonymous access gets the opposite answer — public data stays readable (or was never readable), so the guardrail cannot be exercised at all.
Fix: account-level PublicAccessBlock evaluation in the S3 data-plane authorizer (anonymous/public-policy path), fed by the s3control account PAB state.
Traces: s3control.account-public-access-block-restrict-denies-anonymous-object-read-rejected.

Cause 4 of 7: GetAccessPointPolicyStatus answers NotImplemented.
AWS: Smithy GetAccessPointPolicyStatus (GET /v20180820/accesspoint/{Name}/policyStatus, 200) returns PolicyStatus { IsPublic: boolean }. Recorded behavior: on an access point that carries no policy the call is refused with error code NoSuchAccessPointPolicy, HTTP 404 (example message "The specified accesspoint policy does not exist"); once a policy is attached it answers 200 with IsPublic derived from the document (true for Principal "*"/{"AWS":"*"}, false for an account-scoped principal).
Emulator: error code 'NotImplemented'.
Why: a client cannot tell "no policy" from "service broken", and the publicness derivation that drives every block-public-access decision is unobservable.
Fix: the GetAccessPointPolicyStatus handler — not-found mapping over the access-point policy store plus the IsPublic derivation.
Traces: s3control.access-point-policy-status-derives-ispublic.

Cause 5 of 7: GetMultiRegionAccessPoint reports every member bucket as living in the caller's region.
AWS: Smithy GetMultiRegionAccessPoint (GET /v20180820/mrap/instances/{Name+}, 200) returns MultiRegionAccessPointReport whose Regions is a list of RegionReport { Bucket, Region, BucketAccountId } — "a combination of a bucket and Region that's part of a Multi-Region Access Point". CreateMultiRegionAccessPoint's Details.Regions supplies only Region{Bucket, BucketAccountId}, so the Region value in the report is resolved by the service from each bucket's actual location, not from the request.
Emulator: for an MRAP created over one us-west-2 bucket and one us-east-1 bucket, the report lists [(<east-bucket>, 'us-west-2'), (<west-bucket>, 'us-west-2')] — the request's region is stamped onto every member.
Why: the multi-region-ness of an MRAP is exactly this mapping; a client that routes or verifies membership per region sees a single-region access point and cannot detect a misplaced member bucket.
Fix: resolve each member bucket's location from bucket state when building RegionReport in the GetMultiRegionAccessPoint / ListMultiRegionAccessPoints serializer (and store it at create time).
Traces: s3control.multi-region-access-point-create-and-delete-are-async-operations.

Cause 6 of 7: CreateMultiRegionAccessPoint rejects a duplicate name synchronously instead of accepting the request and failing the async operation.
AWS: Smithy CreateMultiRegionAccessPoint (POST /v20180820/async-requests/mrap/create, 200) returns only RequestTokenARN — the operation is asynchronous and binds no errors; uniqueness is validated by the async operation, not at submission. Recorded behavior: two concurrent creates under the same name are BOTH accepted with 200 and distinct RequestTokenARNs; polling DescribeMultiRegionAccessPointOperation settles exactly one SUCCEEDED and one FAILED whose ResponseDetails.ErrorDetails.Code is MultiRegionAccessPointAlreadyOwnedByYou, leaving exactly one access point under that name.
Emulator: the second CreateMultiRegionAccessPoint raises a synchronous client error with code MultiRegionAccessPointAlreadyOwnedByYou (example message "A Multi-Region Access Point with the same name already exists: <name>"), so the caller never receives a request token and the race cannot be observed where AWS reports it.
Why: a client that submits a create and tracks its request token (the only contract this operation offers) gets an exception from a call AWS answers 200, and conflict handling that inspects the async operation's ErrorDetails never runs.
Fix: move duplicate-name detection out of the CreateMultiRegionAccessPoint handler into the async-operation state machine, so submission always mints a token and the losing operation settles FAILED with that error code.
Traces: s3control.multi-region-access-point-duplicate-name-create-races-rejected.

Cause 7 of 7: a submitted Batch Operations job never reaches a terminal state, so no tasks are executed.
AWS: Smithy CreateJob/DescribeJob (GET /v20180820/jobs/{JobId}, 200, JobDescriptor with Status of enum JobStatus = New|Preparing|Ready|Active|Complete|Failed|Cancelled|...). Recorded behavior: a job walks New -> Preparing -> Active -> Complete, ProgressSummary accounts for every manifest row (TotalNumberOfTasks / NumberOfTasksSucceeded / NumberOfTasksFailed), the operation really runs (S3PutObjectCopy writes byte-identical objects under TargetKeyPrefix; a generated manifest with a KeyNameConstraint enumerates only matching keys and is written out as an object; S3PutObjectTagging tags only the manifest's keys), and an enabled completion report is written under <Prefix>/job-<JobId>/.
Emulator: all three job-executing checks are killed at the suite's 60s budget inside their bounded DescribeJob poll (poll every 4s until Status is Complete/Failed/Cancelled, 600s budget) or inside CreateJob's bounded BadRequestException retry, so the job is never observed reaching a terminal status and none of the post-conditions (copied bytes, generated manifest object, tags, completion report) is reachable. Note for attribution: these checks budget minutes because real AWS takes minutes; an emulator that executes jobs promptly settles in seconds, and nothing before the poll is slow (bucket/object/role setup only). Re-replay the three checks after the job engine settles jobs to confirm.
Why: Batch Operations is only useful as execution over accumulated object state; a client cannot wait for a job, read ProgressSummary, or consume a completion report.
Fix: the batch-job state machine (job progression to a terminal status) and task execution for S3PutObjectCopy / S3PutObjectTagging, generated-manifest enumeration, and completion-report writing. Note one of the three checks (batch-job-completion-report-...) is a DRAFT whose report-layout rules are hypotheses; the load-bearing part grounded here is only that the job settles terminal with an accounted ProgressSummary.
Traces: s3control.s3c-batch-copy-job-copies-manifest-objects-and-leaves-the-rest, s3control.s3c-batch-generated-manifest-filter-limits-job-to-matching-objects, s3control.batch-job-completion-report-accounts-for-mixed-task-outcomes.

## Evidence
Suite simparity-behavioral-ga-prod-v1 — s3control 2/33 passed (6%).

Cause 1: KeyError: 'PublicAccessBlockConfiguration' at check_s3control_access_point_public_policy_blocked_by_own_block_public_policy_rejected.py:42 — default_pab = s3c.get_access_point(AccountId=account, Name=blocked)["PublicAccessBlockConfiguration"], where the access point was created without that member. Expected per Smithy GetAccessPointResult: the member present, with the service's defaults.

Cause 2: "Check timed out after 60.0s" on s3control.account-public-access-block-blocks-public-access-point-policy-rejected. The check sets the account PAB to all-true (polled until GetPublicAccessBlock serves it), then loops 30x/2s issuing PutAccessPointPolicy with Principal {"AWS":"*"} and breaking on the first ClientError; it dropped the policy after each accepted put and never saw a refusal. Expected: AccessDenied / HTTP 403 (example message "Access Denied"), with GetAccessPointPolicy then answering NoSuchAccessPointPolicy.

Cause 3: "Check timed out after 60.0s" on s3control.account-public-access-block-restrict-denies-anonymous-object-read-rejected. Bounded loops only: 20x/3s unsigned GetObject for the baseline read of a bucket carrying a public policy, then 20x/3s unsigned GetObject expecting AccessDenied after PutPublicAccessBlock(RestrictPublicBuckets=true, BlockPublicPolicy=false). Neither arm reached its observable condition inside the budget.

Cause 4: AssertionError: policy status on a policy-less access point returned 'NotImplemented', expected NoSuchAccessPointPolicy — check_s3control_access_point_policy_status_derives_ispublic.py:55.

Cause 5: AssertionError: GetMultiRegionAccessPoint lists regions [('sim2real-behav-mrape-...', 'us-west-2'), ('sim2real-behav-mrapw-...', 'us-west-2')], expected both member buckets: [('sim2real-behav-mrape-...', 'us-east-1'), ('sim2real-behav-mrapw-...', 'us-west-2')] — check_s3control_multi_region_access_point_create_and_delete_are_async_operations.py:75. The east bucket was created in us-east-1 via a us-east-1 S3 client.

Cause 6: botocore.exceptions.ClientError: An error occurred (MultiRegionAccessPointAlreadyOwnedByYou) when calling the CreateMultiRegionAccessPoint operation: A Multi-Region Access Point with the same name already exists: sim2real-behav-mrapdup-... — raised out of one of the two concurrent _create(contested_name) submissions, before the check could compare the two request tokens.

Cause 7: "Check timed out after 60.0s" on s3control.s3c-batch-copy-job-copies-manifest-objects-and-leaves-the-rest, s3control.s3c-batch-generated-manifest-filter-limits-job-to-matching-objects and s3control.batch-job-completion-report-accounts-for-mixed-task-outcomes. All three spend their time in _poll_terminal (describe_job every 4s until Status in Complete/Failed/Cancelled) or in _create_job_surviving_role_propagation's BadRequestException retry; no assertion about job output was ever reached.

## Steps to Reproduce
Replay the behavioral suite against the emulator and re-run exactly the checks in trace_ids.

1. Cause 1 — create a bucket; s3control CreateAccessPoint(AccountId, Name, Bucket) with NO PublicAccessBlockConfiguration; GetAccessPoint(AccountId, Name) and read PublicAccessBlockConfiguration; expect it present with the service defaults.
2. Cause 2 — create a bucket, s3 DeletePublicAccessBlock(Bucket), CreateAccessPoint with all four of its own PAB flags false, s3control PutPublicAccessBlock(AccountId, all four flags true), then PutAccessPointPolicy with a Principal {"AWS":"*"} Allow on <ap-arn>/object/*; expect AccessDenied 403 and GetAccessPointPolicy -> NoSuchAccessPointPolicy. Then DeletePublicAccessBlock(AccountId) and repeat the identical put; expect it to be accepted.
3. Cause 3 — create a bucket, DeletePublicAccessBlock(Bucket), PutObject, PutBucketPolicy allowing s3:GetObject to Principal "*"; unsigned GetObject must return the bytes. Then s3control PutPublicAccessBlock(AccountId, RestrictPublicBuckets=true, BlockPublicPolicy=false); the unsigned GetObject must become AccessDenied while the owner's signed read still succeeds and GetBucketPolicy returns the unchanged document.
4. Cause 4 — CreateAccessPoint, then GetAccessPointPolicyStatus(AccountId, Name) with no policy attached; expect NoSuchAccessPointPolicy 404. Attach a public policy and expect 200 with PolicyStatus.IsPublic=true.
5. Cause 5 — create one bucket in us-west-2 and one in us-east-1; CreateMultiRegionAccessPoint(Details.Regions=[{Bucket: west}, {Bucket: east}]); poll the async operation, then GetMultiRegionAccessPoint and compare each RegionReport's (Bucket, Region) with each bucket's real location.
6. Cause 6 — submit two concurrent CreateMultiRegionAccessPoint calls with the same Details.Name; expect two 200s with distinct RequestTokenARNs, then DescribeMultiRegionAccessPointOperation on each: exactly one SUCCEEDED and one FAILED with ResponseDetails.ErrorDetails.Code == MultiRegionAccessPointAlreadyOwnedByYou, and exactly one access point under that name.
7. Cause 7 — put a two-row manifest.csv, create a batch role, CreateJob with Operation S3PutObjectCopy (and separately with a generated manifest + KeyNameConstraint, and with S3PutObjectTagging over a manifest containing one missing key), then DescribeJob until Status is terminal; expect Complete with ProgressSummary accounting for every row, the copied/tagged objects really present, the generated manifest object listing only matching keys, and the completion report under <Prefix>/job-<JobId>/.
