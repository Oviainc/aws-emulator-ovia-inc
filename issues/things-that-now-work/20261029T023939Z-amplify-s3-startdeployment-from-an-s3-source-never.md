Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify__s3.start-deployment-without-an-amplify-bucket-grant-is-rejected amplify__s3.bucket-root-deploy-of-the-misaligned-seed-is-rejected amplify__s3.zip-deploy-of-a-missing-s3-object-is-rejected amplify__s3.bucket-prefix-job-needs-list-bucket-as-well-as-get-object amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files
Issue-Key: amplify__s3.start-deployment-without-an-amplify-bucket-grant-is-rejected|_start_from_s3#13|Timeout|time.sleep(5.0)
Issue-Key: amplify__s3.bucket-root-deploy-of-the-misaligned-seed-is-rejected|_start_from_s3#8|Timeout|time.sleep(5.0)
Issue-Key: amplify__s3.zip-deploy-of-a-missing-s3-object-is-rejected|_start_from_s3#8|Timeout|time.sleep(5.0)
Issue-Key: amplify__s3.bucket-prefix-job-needs-list-bucket-as-well-as-get-object|_start_from_s3#10|Timeout|time.sleep(5.0)
Issue-Key: amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files|_start_from_s3#9|Timeout|time.sleep(5.0)
Issue-Commit: 2271003192907dc435917d10fce42c387f42c2e4

# amplify__s3: StartDeployment from an s3:// source never answers — every Amplify manual-deployment check runs out the 60 s harness budget (1 cause)

> _Note: this issue was identified against the code base at commit `2271003192907dc435917d10fce42c387f42c2e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
Cause 1 of 1: Amplify's manual-deployment path that reads an Amazon S3 source produces NO response at all. All five amplify__s3 behavioral checks are killed by the harness with "Check timed out after 60.0s"; the seven amplify-only checks in the same run all pass (7/7). The one step the five share and the seven do not is the cross-service S3 read that StartDeployment performs before it answers.

AWS: StartDeployment is modelled `POST /apps/{appId}/branches/{branchName}/deployments/start`, success status 200, output `jobSummary` (@required), errors BadRequestException (400), UnauthorizedException (401), NotFoundException (404), LimitExceededException (429), InternalFailureException (500) (Smithy: com.amazonaws.amplify#StartDeployment; input `sourceUrl` len 0..3000 pattern ^(s3|https|http)://, `sourceUrlType` enum ZIP|BUCKET_PREFIX, default ZIP). AWS resolves the named S3 source AT CALL TIME and answers synchronously, in seconds: either a 200 carrying a PENDING jobSummary whose job GetJob then reports reaching SUCCEED/FAILED, or one of the modelled refusals. Recorded behavior only for the refusal wordings the checks assert — each is fixed text with no variable part: 401 UnauthorizedException "The bucket policy is either missing or has insufficient permissions for this operation" (no Amplify grant on the source bucket); 400 BadRequestException "The bucket must have an index.html file at the path provided in the sourceUrl" (BUCKET_PREFIX with no index.html at exactly that prefix); 400 BadRequestException "The provided S3 source URL is not valid, please make sure the specified bucket and object exists." (ZIP naming an object the bucket does not hold). The one point of note is that no AWS answer here is unbounded: the call returns a status in all cases.

Emulator: the call never returns. No HTTP status, no error code, no body is ever produced — the check is cut off at the 60.0 s cap while still inside the first StartDeployment against an `s3://` source (check start-deployment-without-an-amplify-bucket-grant-is-rejected hangs on its very first StartDeployment, whose only job is to take the modelled 401). Because nothing answers, every downstream assertion of these five checks is unreachable and unverified: the ListBucket-vs-GetObject grant differential, the two BadRequest refusal messages above, the job's terminal status via GetJob, ListJobs ordering/retention, and the customRules rewrite serving at the branch URL.

Why: a client that calls StartDeployment with an S3 source hangs until its own socket timeout instead of receiving the modelled 200/400/401. Manual (non-Git) Amplify deployment is therefore unusable end to end — no job can be started, no job can be polled to a conclusion, and no S3-source authorization or validation refusal can ever be observed. Code that branches on the 401 or on either BadRequest message can never take that branch.

Fix: the operation's handler and the cross-service client it builds — services/service-com-amazonaws-amplify-amplify/src/ops/start_deployment.rs, in `read_source` and the helpers it awaits (`bucket_policy`, `prefix_deployment`, `archive_deployment`, `reserved_deployment`) and in `s3_client`. That client is constructed with `retry_config(RetryConfig::disabled())`, `timeout_config(TimeoutConfig::disabled())` and `stalled_stream_protection(...disabled())`, pointed at `endpoint_url("https://s3.{region}.amazonaws.com")` over `ctx.callback().http_client()`; with every bound disabled, an S3 request that is not delivered on the address the emulator is actually served on leaves the handler awaiting forever and the request unanswered. The handler must reach the emulator's own S3 on the address it is reached on, and every await on the source read must be bounded so the operation always produces one of its modelled answers (a read that cannot complete is what the job's FAILED conclusion already exists to report). The same path must then be checked for the Amplify state lock: it is correctly released before the await today, so the hang is in the S3 leg, not in the store.

Traces: amplify__s3.start-deployment-without-an-amplify-bucket-grant-is-rejected, amplify__s3.bucket-root-deploy-of-the-misaligned-seed-is-rejected, amplify__s3.zip-deploy-of-a-missing-s3-object-is-rejected, amplify__s3.bucket-prefix-job-needs-list-bucket-as-well-as-get-object, amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files.

## Evidence
Suite run: 7/12 passed. amplify 7/7 (100%) ✓; amplify__s3 0/5 (0%), 5 failed.

All five failures carry the identical verdict and detail:
  [mismatch] amplify__s3.bucket-prefix-job-needs-list-bucket-as-well-as-get-object — Error: Check timed out after 60.0s
  [mismatch] amplify__s3.bucket-root-deploy-of-the-misaligned-seed-is-rejected — Error: Check timed out after 60.0s
  [mismatch] amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files — Error: Check timed out after 60.0s
  [mismatch] amplify__s3.start-deployment-without-an-amplify-bucket-grant-is-rejected — Error: Check timed out after 60.0s
  [mismatch] amplify__s3.zip-deploy-of-a-missing-s3-object-is-rejected — Error: Check timed out after 60.0s

Expected (AWS): StartDeployment answers within seconds with HTTP 200 + jobSummary{jobId, status "PENDING", jobType "MANUAL", sourceUrl, sourceUrlType}, or with HTTP 401 UnauthorizedException "The bucket policy is either missing or has insufficient permissions for this operation", or HTTP 400 BadRequestException "The bucket must have an index.html file at the path provided in the sourceUrl" / "The provided S3 source URL is not valid, please make sure the specified bucket and object exists." GetJob then reports the job settling to SUCCEED (source read) or FAILED (source unreadable).

Actual (emulator): no response is ever produced for StartDeployment with an `s3://` sourceUrl — no status, no error code, no body. The checks' own waits are far longer than the harness cap (_JOB_DEADLINE_S=240 s, _GRANT_DEADLINE_S=240 s), and the emulator's job clock settles in 500 ms (JOB_SETTLE_MILLIS in services/service-com-amazonaws-amplify-amplify/src/state.rs, applied by BranchJob::settle from ops/get_job.rs), so the stall is not job polling: it is the StartDeployment request itself, whose handler awaits an S3 read built with retry, timeout and stalled-stream bounds all disabled against endpoint_url "https://s3.<region>.amazonaws.com" (services/service-com-amazonaws-amplify-amplify/src/ops/start_deployment.rs, fn s3_client / fn bucket_policy / fn read_source).

Every per-check assertion after that first call was never evaluated, so those behaviors are unverified rather than confirmed correct.

## Steps to Reproduce
1. Create an S3 bucket, enable versioning + SSE-S3 + full public-access block, and put a bucket policy containing only the aws:SecureTransport=false Deny statement.
2. Put objects src/index.html, styles/main.css, scripts/app.js into it (any content).
3. Append an Allow statement to the bucket policy for Principal {"Service": "amplify.amazonaws.com"} with Action s3:GetObject (+ s3:ListBucket) on arn:aws:s3:::<bucket>/* (and arn:aws:s3:::<bucket>).
4. amplify CreateApp, then CreateBranch(branchName="main").
5. Call amplify StartDeployment(appId=<id>, branchName="main", sourceUrl="s3://<bucket>/src/", sourceUrlType="BUCKET_PREFIX").
   Expected: an answer within seconds — HTTP 200 with a PENDING jobSummary (grant present), or HTTP 401 UnauthorizedException "The bucket policy is either missing or has insufficient permissions for this operation" (step 3 skipped).
   Actual: the request never completes; the client blocks until its own timeout. Same for sourceUrlType="ZIP" with sourceUrl="s3://<bucket>/site.zip".
6. Because step 5 never returns, GetJob/ListJobs on that branch are never reached, and the BadRequest refusals for a root prefix with no index.html (sourceUrl="s3://<bucket>/") and for a missing archive object (sourceUrl="s3://<bucket>/bundle/site2.zip") cannot be exercised at all.
