Issue-Source: trace-sweep-divergence
Issue-Trace: entityresolution-rule-based-matching-job-writes-matches
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: entityresolution-rule-based-matching-job-writes-matches
Issue-Shape-Id: com.amazonaws.entityresolution#AWSVeniceService
Issue-Key: entityresolution|GetMatchingJob|2xx->2xx|$.outputSourceConfig
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# entityresolution GetMatchingJob returns outputSourceConfig that AWS omits for jobs started without an output-source override

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMatchingJob

## Description
One cause.

Cause 1 of 1: GetMatchingJob populates the optional `outputSourceConfig` output member on a job that was started with no output-source override, where AWS leaves the member absent.

AWS: Smithy model services/service-com-amazonaws-entityresolution-awsveniceservice/smithy-model/com-amazonaws-entityresolution-awsveniceservice.json — GetMatchingJob (GET /matchingworkflows/{workflowName}/jobs/{jobId}, http 200). Its output shape GetMatchingJobOutput marks only `jobId`, `status` and `startTime` @required; `outputSourceConfig` (target JobOutputSourceConfig, a list with @length min 1 max 1, documented only as "A list of OutputSource objects") is OPTIONAL, and the operation's StartMatchingJob counterpart in this model accepts no outputSourceConfig input at all, so there is no per-job output-source override for the job in question. Recorded behavior: for a matching job started from a workflow by StartMatchingJob with no override, AWS's GetMatchingJob 200 response does NOT contain an `outputSourceConfig` key — the job's output location is a property of the matching workflow (readable via GetMatchingWorkflow), not of the job. Hypothesis: AWS only echoes outputSourceConfig on a job when the job itself carried an output-source override, so the member distinguishes an overridden job from a plain one.

Emulator: the emulator answers 200 but adds the member, copying the parent workflow's output configuration into the job response — replay reports `$.outputSourceConfig: type_mismatch (expected None, got [{'roleArn': ..., 'outputS3Path': ...}])`. Both the status (200) and every other asserted member match; the whole divergence is this one extra key.

Why: `outputSourceConfig` present-vs-absent is the signal a client uses to tell whether a job overrode its workflow's output destination. A client that reads job.outputSourceConfig when present (and otherwise falls back to GetMatchingWorkflow) is told by the emulator that every job overrode its output, so it treats a workflow-level destination as a job-level one; conditional logic and round-trip comparisons that key off the member's absence take the wrong branch against the emulator and the right one against AWS. Strongly typed SDKs also surface the field as populated-Some instead of None, so Option-matching client code diverges without any error being raised.

Fix: the GetMatchingJob handler / its output serializer in the entityresolution service crate. The job record's read path should only emit `outputSourceConfig` when the job was created with an explicit output-source override, and must not inherit or synthesize it from the owning matching workflow's output config; when there is no override the member must be omitted from the JSON body entirely (not emitted as null or as an empty list, which @length min 1 forbids).

Traces: entityresolution-rule-based-matching-job-writes-matches#task#11 (GetMatchingJob after StartMatchingJob on a rule-based matching workflow).

## Evidence
Fixture entityresolution-rule-based-matching-job-writes-matches, step task#11, operation GetMatchingJob, expected status 200, actual status 200.

Expected (AWS recording): no `outputSourceConfig` key in the GetMatchingJob 200 body.
Actual (emulator): `"outputSourceConfig": [{"roleArn": "<workflow job role arn>", "outputS3Path": "s3://<workflow output bucket>/output"}]`.

Replay detail verbatim: `$.outputSourceConfig: type_mismatch (expected None, got [{'roleArn': 'arn:aws:iam::000000000000:role/sim2real-probe-er-role-a6609b5e', 'outputS3Path': 's3://sim2real-probe-er-out-1528b524/output'}])`. The arn, role name, bucket name and path in that value are per-run generated identifiers taken from the probe's own workflow and are not part of the requirement; the requirement is that the member is absent.

Model evidence: GetMatchingJobOutput requires jobId/status/startTime and leaves outputSourceConfig optional (JobOutputSourceConfig, list, @length min 1 max 1); StartMatchingJobInput in this model carries only `workflowName`, so no job-level output-source override existed for this job.

No existing open entityresolution issue covers this: 20261103T143443Z-...-provider-service-catalog-is-incomplete covers ListProviderServices/GetProviderService, 20261108T141905Z-...-matchid-is-minted-as-a-hyphenated-uuid covers matchId formatting, 20261102T065110Z-...-id-namespace-lifecycle covers CreateIdNamespace/DeleteIdNamespace.

## Steps to Reproduce
1. Create an S3 output bucket and an IAM role, then CreateMatchingWorkflow with a rule-based resolution technique and an outputSourceConfig pointing at s3://<bucket>/output.
2. StartMatchingJob on that workflow (StartMatchingJob takes only workflowName — no output-source override is supplied).
3. Poll GET /matchingworkflows/<workflowName>/jobs/<jobId> (GetMatchingJob) until status is COMPLETED.
4. Inspect the 200 JSON body: the emulator includes an `outputSourceConfig` array echoing the workflow's roleArn/outputS3Path; AWS omits the key entirely.

Replay: replay_wire_corpus, fixture entityresolution-rule-based-matching-job-writes-matches, step task#11.
