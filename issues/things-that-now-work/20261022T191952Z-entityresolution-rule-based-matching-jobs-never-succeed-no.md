Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: entityresolution.matching-workflow-job-full-lifecycle entityresolution.add-policy-statement-concurrent-same-id-exactly-one-wins
Issue-Shape-Id: com.amazonaws.entityresolution#AWSVeniceService
Issue-Key: entityresolution.matching-workflow-job-full-lifecycle|run#24|AssertionError|assert final["status"] == "SUCCEEDED", final.get("errorDetails")
Issue-Key: entityresolution.add-policy-statement-concurrent-same-id-exactly-one-wins|run#16|AssertionError|assert len(successes) == 1, f"expected exactly one winner, got {len(successes)}: {results}"
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# entityresolution: rule-based matching jobs never succeed (no matching engine) and AddPolicyStatement duplicate-statementId semantics missing

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMatchingJob

## Description
Two distinct root causes in the entityresolution emulator, both reached through operations the emulator does answer.

Cause 1 of 2: StartMatchingJob / GetMatchingJob never reach SUCCEEDED — the matching data plane is absent.
AWS: Smithy model (services/service-com-amazonaws-entityresolution-awsveniceservice/smithy-model) StartMatchingJob POST /matchingworkflows/{workflowName}/jobs -> StartMatchingJobOutput{jobId}; GetMatchingJob GET /matchingworkflows/{workflowName}/jobs/{jobId} -> {jobId, status (JobStatus, required), startTime, endTime, metrics: JobMetrics, errorDetails, outputSourceConfig}. For a RULE_MATCHING workflow whose inputSourceConfig points at a readable Glue table and whose outputSourceConfig names an S3 prefix, the job progresses QUEUED/RUNNING -> SUCCEEDED, metrics report the input/processed record counts, and the matched records are written under the output S3 prefix with a matchId per resolved group (records sharing the rule's matchingKeys get one matchId, other groups get different matchIds). Recorded behavior confirms the terminal status is SUCCEEDED with metrics.inputRecords == metrics.totalRecordsProcessed == the number of input rows.
Emulator: the job is accepted (StartMatchingJob returns a jobId and the non-terminal ConflictException on DeleteMatchingWorkflow is honored), but the job always lands in a non-SUCCEEDED terminal state carrying errorDetails.errorMessage "Entity Resolution matching is not available in this emulator: no matching engine is attached, so the job could not read the workflow's input sources or write matched records to its output source." (example text). No metrics are produced and nothing is written under the output prefix.
Why: any client that starts a matching job and waits for it — the whole point of a matching workflow — sees a permanently failing job. Pipelines that poll to SUCCEEDED, read matchIds from the S3 output, or assert record counts cannot run at all against the emulator, and error handling paths are exercised instead of success paths.
Fix: the matching data plane behind StartMatchingJob/GetMatchingJob — the job runner/state machine plus the rule-based matcher that reads the workflow's input sources, assigns matchIds by the configured matchingKeys, writes the output objects to the outputS3Path and records JobMetrics. Only the terminal status + metrics + output contract is required; job ids and timestamps are environment-specific.
Traces: entityresolution.matching-workflow-job-full-lifecycle (assertion: final["status"] == "SUCCEEDED", got errorDetails as quoted above).

Cause 2 of 2: AddPolicyStatement is a scaffold stub, so resource-policy statements cannot be added and duplicate-statementId semantics are absent.
AWS: Smithy model AddPolicyStatement POST /policies/{arn}/{statementId}, @idempotent, input {arn, statementId (@length 1-64, @pattern ^[0-9A-Za-z]+$), effect, action, principal, condition}, output {arn, token, policy}; modeled errors include ValidationException (400) and ConflictException (400). Recorded behavior: when several concurrent callers add the identical statementId to the same resource, exactly one call succeeds and the others are rejected with error code ValidationException (not ConflictException), and the resulting policy document contains exactly one Statement whose Sid is that statementId — no lost update, no duplicated statement.
Emulator: every call returns HTTP error code NotImplemented ("operation AddPolicyStatement is scaffolded but not implemented in this emulator"), so all six concurrent racers fail and no policy statement exists at all.
Why: resource-policy management is unusable; a client cannot grant cross-account access, and once the stub is implemented the duplicate-id race must still collapse to exactly one winner with ValidationException for the losers, or callers silently lose statements / see duplicate Sids.
Fix: implement the AddPolicyStatement handler (services/service-com-amazonaws-entityresolution-awsveniceservice/src/ops/add_policy_statement.rs) and make the policy-document update in the service state atomic, rejecting an already-present statementId with ValidationException. Related open issue 20261022T153803Z-entityresolution-*-stub-style issues cover other unimplemented operations; this check was not attributed to one and additionally carries the concurrency/duplicate-id requirement.
Traces: entityresolution.add-policy-statement-concurrent-same-id-exactly-one-wins (expected exactly one winner, got 0; all six results ('error', 'NotImplemented')).

## Evidence
Cause 1 — entityresolution.matching-workflow-job-full-lifecycle:
  expected: GetMatchingJob terminal status == "SUCCEEDED", metrics.inputRecords == 4, metrics.totalRecordsProcessed == 4, output objects under the workflow's outputS3Path grouping the two email-matching records under one matchId and the other two under a different matchId.
  actual:   AssertionError: {'errorMessage': "Entity Resolution matching is not available in this emulator: no matching engine is attached, so the job could not read the workflow's input sources or write matched records to its output source."} — job never reaches SUCCEEDED, no metrics, no S3 output.

Cause 2 — entityresolution.add-policy-statement-concurrent-same-id-exactly-one-wins:
  expected: 1 of 6 concurrent AddPolicyStatement(statementId="raceStmt") calls returns 200; the other 5 fail with error code ValidationException; GetPolicy shows exactly one Statement with Sid "raceStmt".
  actual:   AssertionError: expected exactly one winner, got 0: [('error', 'NotImplemented') x6] — the operation is a scaffolded stub, so no statement is ever added.

## Steps to Reproduce
Cause 1:
1. Create an S3 input bucket with a 4-row CSV, an output bucket, a Glue database/table over the input, and an IAM role Entity Resolution can assume.
2. entityresolution CreateSchemaMapping with fields id (UNIQUE_ID), name (NAME, matchKey name), email (EMAIL_ADDRESS, matchKey email).
3. CreateMatchingWorkflow with inputSourceConfig = the Glue table ARN + that schema, outputSourceConfig = s3://<out>/output, resolutionTechniques = RULE_MATCHING with rule emailRule on matchingKeys ['email'], and the role.
4. StartMatchingJob, then poll GetMatchingJob to a terminal state.
5. Observe status is not SUCCEEDED and errorDetails reports no matching engine; no objects are written under s3://<out>/output.

Cause 2:
1. Create an ID namespace (CreateIdNamespace) and take its idNamespaceArn.
2. From 6 threads concurrently call AddPolicyStatement(arn=<that arn>, statementId="raceStmt", effect="Allow", action=["entityresolution:GetIdNamespace"], principal=[<account>]).
3. Observe all 6 calls fail with NotImplemented instead of one 200 and five ValidationException; GetPolicy shows no raceStmt statement.
