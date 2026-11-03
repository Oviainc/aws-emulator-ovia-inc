Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.account-public-access-block-blocks-public-bucket-policy-rejected s3control.account-public-access-block-restrict-denies-anonymous-object-read-rejected s3control.s3c-access-point-policy-explicit-deny-overrides-bucket-allow-rejected s3control.batch-job-runs-to-completion-and-tags-objects s3control.batch-job-lifecycle-priority-and-cancel
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control.account-public-access-block-blocks-public-bucket-policy-rejected|_error#18|AssertionError|raise AssertionError("call unexpectedly succeeded; an error was expected")
Issue-Key: s3control.account-public-access-block-restrict-denies-anonymous-object-read-rejected|run#17|AssertionError|assert body == b"anonymously-readable", (
Issue-Key: s3control.s3c-access-point-policy-explicit-deny-overrides-bucket-allow-rejected|run#23|AssertionError|assert denied == "AccessDenied", (
Issue-Key: s3control.batch-job-runs-to-completion-and-tags-objects|_poll_terminal#16|Timeout|time.sleep(3)
Issue-Key: s3control.batch-job-lifecycle-priority-and-cancel|run#29|AssertionError|assert message == "Requested job status forbidden", message
Issue-Commit: d24b6997be17cc686e45f4a7148f10449cf3bf59

# s3control/S3 policy enforcement and Batch Operations diverge: account BlockPublicPolicy and RestrictPublicBuckets not enforced, access-point ARN reads answer NoSuchBucket, jobs never run, terminal-status refusal message interpolated (5 causes)

> _Note: this issue was identified against the code base at commit `d24b6997be17cc686e45f4a7148f10449cf3bf59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutPublicAccessBlock (bundle also covers PutBucketPolicy, GetObject via access point, CreateJob/DescribeJob, UpdateJobStatus)

## Description
Five net-new root causes found by replaying the behavioral suite against the s3control emulator (14/34 checks pass). All five are s3control-attributed checks; none is marked ALREADY FILED or OWNED in the replay report. Where an open issue describes a neighbouring symptom it is named in the cause.

--- Cause 1 of 5: an account-level PublicAccessBlock with BlockPublicPolicy=true does not refuse a public PutBucketPolicy ---
AWS: s3control PutPublicAccessBlock stores a one-per-account PublicAccessBlockConfiguration (Smithy: PutPublicAccessBlockRequest.PublicAccessBlockConfiguration required, AccountId @hostLabel/x-amz-account-id, pattern ^\d{12}$; GetPublicAccessBlock echoes it). While BlockPublicPolicy is true, S3 refuses PutBucketPolicy for any document it classifies as public (a statement with Principal "*" — a Condition such as aws:Referer does not de-publicize it) with HTTP 403 and Code AccessDenied. The message's fixed text is `User: <caller-arn> is not authorized to perform: s3:PutBucketPolicy on resource: "arn:aws:s3:::<bucket>" because public policies are prevented by the BlockPublicPolicy setting in S3 Block Public Access.`; the variable parts are the caller's ARN (from the signing identity) and the bucket name (from the request). The refused Put leaves no policy behind, so GetBucketPolicy then answers NoSuchBucketPolicy with the fixed message `The bucket policy does not exist`. Enforcement is live: after DeletePublicAccessBlock the identical document is accepted again, and a non-public (account-root principal) document is accepted even while the block is on, because the block is evaluated against the DOCUMENT and not against the operation.
Emulator: the same public document is accepted with 200 while GetPublicAccessBlock reports BlockPublicPolicy=true (the check fails at `call unexpectedly succeeded; an error was expected` on PutBucketPolicy).
Why: a client that relies on account-level Block Public Access as a guardrail can publish a world-readable bucket policy on this emulator; code paths that expect and handle the AccessDenied refusal are never taken, and the "guardrail on" state is indistinguishable from "guardrail off".
Fix: the S3 PutBucketPolicy handler's validation must consult the s3control account PublicAccessBlock record and classify the submitted document as public/non-public before storing it. Open issue 20261024T210022Z-s3control-account-level-blockpublicpolicy-not-enforced-on-putbucketpolicy.md reports the same enforcement gap from the wire corpus; it does not carry this behavioral check's replay key.
Traces: s3control.account-public-access-block-blocks-public-bucket-policy-rejected.

--- Cause 2 of 5: an anonymous (unsigned) GetObject allowed by a public bucket policy is never served, so RestrictPublicBuckets cannot be observed ---
AWS: with a bucket policy granting Principal "*" s3:GetObject on arn:aws:s3:::<bucket>/* (conditioned on aws:Referer), an UNSIGNED GetObject carrying that Referer returns 200 and the object bytes. Turning the account-level RestrictPublicBuckets flag on then refuses that same unsigned read with HTTP 403, Code AccessDenied and the fixed message `Access Denied` (no interpolation), while the owner's signed read still returns the bytes, GetBucketPolicy still returns the identical document and GetBucketPolicyStatus still reports IsPublic=true; clearing the flag restores the same anonymous read. Recorded behavior only for the anonymous-path status text; the flag semantics are AWS-documented Block Public Access behaviour.
Emulator: the baseline unsigned read never succeeds — every attempt in a 60 s poll raises a ClientError and the body stays None (`the baseline anonymous read never succeeded (last body None)`). The emulator therefore refuses anonymous requests regardless of a bucket policy that explicitly allows them, and the RestrictPublicBuckets arm is unreachable.
Why: public/static-website style access, pre-flight checks of "is this object world-readable", and any negative test of Block Public Access all behave differently than on AWS: a client cannot distinguish "blocked by the guardrail" from "anonymous access never works". Hypothesis: the S3 data plane requires a signed request and does not evaluate a bucket-policy Allow for the anonymous principal.
Fix: the S3 GetObject request-authorization path — accept unsigned requests and evaluate the bucket policy (including Condition keys such as aws:Referer) for the anonymous principal, then apply the account PublicAccessBlock RestrictPublicBuckets flag to that decision.
Traces: s3control.account-public-access-block-restrict-denies-anonymous-object-read-rejected.

--- Cause 3 of 5: an access-point ARN in the Bucket field answers NoSuchBucket instead of resolving the access point and applying its policy ---
AWS: S3 accepts an access point ARN (arn:aws:s3:<region>:<account>:accesspoint/<name>) in the Bucket field of a data-plane call and resolves it to the underlying bucket. The access point policy's explicit Deny beats the bucket policy's Allow on that path only: GetObject on the denied prefix through the access point is refused with HTTP 403, Code AccessDenied and message whose fixed text is `User: <caller-arn> is not authorized to perform: s3:GetObject on resource: "<access-point-arn>/object/<key>" with an explicit deny in a resource-based policy` (caller ARN, access point ARN and key are the variable parts), while a key the access point policy does not deny is still served 200 through the same access point and the denied key is still served through the bucket path.
Emulator: the read through the access point ARN answers Code NoSuchBucket, i.e. the ARN is treated as a literal bucket name that does not exist; the access point policy is never evaluated.
Why: every client that addresses data through an access point gets a 404-class NoSuchBucket for an object that exists, and the authorization semantics access points exist for (an AP Deny overriding a bucket Allow) are not exercised at all — a client cannot tell a missing bucket from a denied access point.
Fix: the S3 data-plane bucket-field resolver must parse access point ARNs and route to the access point's bucket, and the GetObject authorization path must evaluate the stored access point policy (Deny wins) before serving bytes. Open issue 20261030T064829Z-s3control-an-internet-borne-read-of-a-vpc.md reports the same NoSuchBucket for a VPC-origin access point read and 20261022T213144Z-s3control-access-points-directory-bucket-listing-job-object.md covers other access-point gaps; neither carries this check's replay key.
Traces: s3control.s3c-access-point-policy-explicit-deny-overrides-bucket-allow-rejected.

--- Cause 4 of 5: a ConfirmationRequired=false S3PutObjectTagging job never runs to Complete and never tags the manifest's object ---
AWS: a CreateJob with ConfirmationRequired=false starts on its own (Smithy: CreateJob POST /v20180820/jobs, ConfirmationRequired, Operation/Report/Priority/RoleArn required). DescribeJob eventually reports a terminal Job.Status of Complete with a ProgressSummary of TotalNumberOfTasks=1, NumberOfTasksSucceeded=1, NumberOfTasksFailed=0, and the object named by the CSV manifest ends up carrying exactly the job's TagSet (recorded behavior only for the settling timing).
Emulator: the job never reaches a terminal status — the check runs out its 60 s budget polling DescribeJob (`Check timed out after 60.0s`), so no Complete status, no ProgressSummary and no object tagging ever appear.
Why: any client that submits a Batch Operations job and waits for it polls forever; the job's effect on the data plane (tagging, copying) never happens, so a workflow built on Batch Operations silently does nothing.
Fix: the Batch Operations job execution/state machine in the s3control crate — advance a non-confirmation job through Preparing/Active to Complete, apply the operation to each manifest entry and populate ProgressSummary. Open issues 20261029T195302Z-s3control-batch-operations-a-confirmationrequired-false-s3putobjecttagging-job.md, 20261029T192817Z-s3control-batch-operations-jobs-never-settle-and-refuse.md and 20261102T121736Z-s3control-s3-control-batch-job-s3putobjecttagging-never-reaches-a.md describe this family from other corpora; none carries this check's replay key.
Traces: s3control.batch-job-runs-to-completion-and-tags-objects.

--- Cause 5 of 5: the refusal of a status change out of a terminal job state carries an interpolated message instead of AWS's fixed text ---
AWS: UpdateJobStatus on a job already in a terminal state (here Cancelled) is refused with HTTP 400, Code InvalidRequest and the FIXED message `Requested job status forbidden` — nothing from the request or the job state is interpolated into it, and the job stays in its terminal status. (Smithy: UpdateJobStatus errors BadRequestException / JobStatusException / NotFoundException / TooManyRequestsException, all httpError 400; the wire code AWS sends on this surface is InvalidRequest.)
Emulator: answers 400 InvalidRequest (code correct) but with the message `Job status cannot be updated from Cancelled to Ready`, interpolating the current and requested status into text AWS keeps fixed.
Why: a client that matches or logs the documented refusal text does not recognize the refusal, and the emulator leaks state-machine detail AWS does not expose in that message.
Fix: the UpdateJobStatus handler's terminal-state guard — emit the fixed message for this error class instead of building one from the statuses.
Traces: s3control.batch-job-lifecycle-priority-and-cancel.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), overall 15/36 passed; s3control 14/34 (41%).

Cause 1 — s3control.account-public-access-block-blocks-public-bucket-policy-rejected [mismatch]:
  check_s3control_account_public_access_block_blocks_public_bucket_policy_rejected.py:82
  `code, code_msg = _error(lambda: s3.put_bucket_policy(Bucket=bucket, Policy=public))`
  AssertionError: call unexpectedly succeeded; an error was expected.
  Expected: 403 AccessDenied, Message `User: <caller-arn> is not authorized to perform: s3:PutBucketPolicy on resource: "arn:aws:s3:::<bucket>" because public policies are prevented by the BlockPublicPolicy setting in S3 Block Public Access.` and GetBucketPolicy → NoSuchBucketPolicy / `The bucket policy does not exist`. Got: 200, policy stored. GetPublicAccessBlock had already echoed BlockPublicPolicy=true.

Cause 2 — s3control.account-public-access-block-restrict-denies-anonymous-object-read-rejected [mismatch]:
  check_...py:74 `assert body == b"anonymously-readable"`
  AssertionError: the baseline anonymous read never succeeded (last body None); the restriction below would prove nothing.
  Expected: unsigned GetObject (botocore UNSIGNED, aws:Referer satisfied) → 200 with the object bytes; then under RestrictPublicBuckets=true → 403 AccessDenied / `Access Denied`. Got: every unsigned attempt raised ClientError over a 60 s poll.

Cause 3 — s3control.s3c-access-point-policy-explicit-deny-overrides-bucket-allow-rejected [mismatch]:
  check_...py:122 `assert denied == "AccessDenied"`
  AssertionError: reading 'locked/secret.txt' through the access point produced 'NoSuchBucket', expected 'AccessDenied' from the access point policy's explicit Deny.
  Expected: 403 AccessDenied, Message `User: <caller-arn> is not authorized to perform: s3:GetObject on resource: "<ap-arn>/object/locked/secret.txt" with an explicit deny in a resource-based policy`. Got: NoSuchBucket. GetAccessPointPolicy had read the stored policy back correctly (one Allow + one Deny).

Cause 4 — s3control.batch-job-runs-to-completion-and-tags-objects [mismatch]:
  `Check timed out after 60.0s` while polling DescribeJob for a terminal status.
  Expected: Job.Status Complete, ProgressSummary {TotalNumberOfTasks: 1, NumberOfTasksSucceeded: 1, NumberOfTasksFailed: 0}, and GetObjectTagging on the manifest's object returning the job's TagSet. Got: no terminal status within the budget.

Cause 5 — s3control.batch-job-lifecycle-priority-and-cancel [mismatch]:
  check_...py:143 `assert message == "Requested job status forbidden", message`
  AssertionError: Job status cannot be updated from Cancelled to Ready.
  Expected: 400 InvalidRequest with the fixed message `Requested job status forbidden`. Got: 400 InvalidRequest with `Job status cannot be updated from Cancelled to Ready`. The Suspended settlement, UpdateJobPriority=42 and the cancel all behaved correctly up to that point.

## Steps to Reproduce
1. Cause 1: create a bucket; s3 PutPublicAccessBlock on the bucket with BlockPublicPolicy=false; s3control PutPublicAccessBlock(AccountId) with all four flags true; verify GetPublicAccessBlock echoes BlockPublicPolicy=true; then s3 PutBucketPolicy with a statement Principal "*", Action s3:GetObject, Resource arn:aws:s3:::<bucket>/* (Condition StringEquals aws:Referer <secret>). The emulator answers 200; AWS answers 403 AccessDenied with the BlockPublicPolicy message, and GetBucketPolicy then answers NoSuchBucketPolicy.
2. Cause 2: same bucket setup with the account flags BlockPublicPolicy=false, RestrictPublicBuckets=false; PutObject open.txt; PutBucketPolicy with the public aws:Referer statement; GetObject with an UNSIGNED botocore client sending that Referer. The emulator raises a ClientError on every attempt; AWS returns the object bytes (and returns 403 AccessDenied / `Access Denied` only once account RestrictPublicBuckets is turned on).
3. Cause 3: create a bucket with keys open/allowed.txt and locked/secret.txt and a bucket policy allowing s3:GetObject to arn:aws:iam::<account>:root on <bucket>/*; s3control CreateAccessPoint; PutAccessPointPolicy with Allow on <ap-arn>/object/* and Deny on <ap-arn>/object/locked/*; then s3 GetObject(Bucket=<ap-arn>, Key="locked/secret.txt"). The emulator answers NoSuchBucket; AWS answers 403 AccessDenied with the explicit-deny message, while Key="open/allowed.txt" is served through the same access point.
4. Cause 4: s3control CreateJob with ConfirmationRequired=false, Operation S3PutObjectTagging, a CSV manifest naming one object, a report bucket and a batchoperations.s3.amazonaws.com-trusting role; poll DescribeJob. The emulator never reports a terminal status; AWS reaches Complete with ProgressSummary 1/1/0 and the manifest's object carries the job's tags.
5. Cause 5: create a ConfirmationRequired=true job, let it settle Suspended, UpdateJobStatus(RequestedJobStatus="Cancelled"), wait for Cancelled, then UpdateJobStatus(RequestedJobStatus="Ready"). The emulator refuses with 400 InvalidRequest but message `Job status cannot be updated from Cancelled to Ready`; AWS's message is the fixed `Requested job status forbidden`.
