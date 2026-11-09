Issue-Source: trace-sweep-divergence
Issue-Trace: emr-serverless-spark-job-run-writes-s3-output
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-serverless-spark-job-run-writes-s3-output emr-serverless-session-endpoint-and-resource-dashboard
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.attemptCreatedAt
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.attemptUpdatedAt
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.billedResourceUtilization
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.configurationOverrides
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.createdAt
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.endedAt
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.extendedSupportEnabled
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.mode
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.queuedDurationMilliseconds
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.retryPolicy
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.startedAt
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.state
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.stateDetails
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.tags
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.totalExecutionDurationSeconds
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.totalResourceUtilization
Issue-Key: emr-serverless|GetJobRun|2xx->2xx|$.jobRun.updatedAt
Issue-Key: emr-serverless|GetSessionEndpoint|2xx->2xx|$.endpoint
Issue-Commit: 41c5ae389f495755d1c1e47282b4b58fd25764aa

# EMR Serverless read responses are unusable: GetJobRun omits most JobRun members and GetSessionEndpoint points off-emulator

> _Note: this issue was identified against the code base at commit `41c5ae389f495755d1c1e47282b4b58fd25764aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJobRun

## Description
Two net-new causes in the emr-serverless read path. Both are response-fidelity defects in GetJobRun/GetSessionEndpoint, not request validation: the emulator answers 200 but the body it serves cannot be consumed the way AWS's can.

Cause 1 of 2 — GetJobRun's JobRun is a skeleton and the run reports a terminal state it never earned (fixture emr-serverless-spark-job-run-writes-s3-output, step task#8).
AWS: Smithy GetJobRun -> GetJobRunResponse.jobRun (required) targets JobRun. JobRun models state (required, JobRunState), stateDetails (required, String256 @length min 1 max 256), plus mode (JobRunMode, values BATCH|STREAMING), retryPolicy (RetryPolicy), tags (TagMap), configurationOverrides (ConfigurationOverrides), totalResourceUtilization (TotalResourceUtilization), billedResourceUtilization (ResourceUtilization with vCPUHour/memoryGBHour/storageGBHour), totalExecutionDurationSeconds (Integer, documented as "only available for job runs in a COMPLETED, FAILED, or CANCELLED state"), startedAt (Date, "the date and time when the job moved to the RUNNING state"), and queuedDurationMilliseconds (Long, "total time for a job in the QUEUED state in milliseconds"). A submitted Spark job that runs to completion reaches state SUCCESS; the recording shows stateDetails as the empty string for a successful run, mode BATCH echoed from the submission, retryPolicy echoed from the submission (maxAttempts as submitted), tags present as a map (empty when none were set), configurationOverrides echoed back including the monitoringConfiguration the caller submitted, extendedSupportEnabled present as a boolean (recorded behavior only — this member is not in the checked-in model), and both utilization structures populated. The recorded utilization numbers, timestamps and durations are per-run values, not requirements; the requirement is that the members are PRESENT and internally consistent (startedAt <= endedAt, totalExecutionDurationSeconds derived from them, utilization non-null for a terminated run).
Emulator: the run reaches terminal state FAILED instead of SUCCESS, and stateDetails carries the emulator-authored sentence "This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running." instead of AWS's empty stateDetails for a successful run. Every optional member listed above is served as null/absent: mode, retryPolicy, tags, configurationOverrides, totalResourceUtilization, billedResourceUtilization, totalExecutionDurationSeconds, startedAt, queuedDurationMilliseconds, extendedSupportEnabled. The poll in the step gave up after 2 attempts because FAILED is in its fail_on set.
Why: a client that submits a job and polls GetJobRun until SUCCESS never terminates (or aborts on FAILED), so no downstream step ever runs. A client that reads mode/retryPolicy/tags/configurationOverrides to confirm what it submitted sees nothing echoed back, and a cost or SLA reporter that reads totalResourceUtilization / billedResourceUtilization / totalExecutionDurationSeconds / queuedDurationMilliseconds gets null where AWS always has a value for a terminated run. Emitting an emulator self-description in stateDetails also leaks the emulator into a field clients log and parse.
Why (hypothesis): the handler records the submission and immediately marks it FAILED rather than driving a lifecycle, so the fields that are only populated by a run (startedAt, durations, utilization) are never set and the submission-time fields are never copied onto the stored run.
Fix: the emr-serverless GetJobRun handler and the job-run state it reads — drive the recorded run through a lifecycle that reaches SUCCESS, persist the submission's mode/retryPolicy/tags/configurationOverrides onto the stored run, and populate the terminal-state timing and utilization members in the response serializer. Related open issues describe the never-executes symptom but do not carry this step's replay key: 20261022T154846Z-emr-serverless-job-runs-are-recorded-but-never-executed.md, 20261024T013257Z-emr-serverless-job-runs-never-execute-the-submitted.md, 20261030T093552Z-emr-serverless-job-run-lifecycle-diverges-runs-never.md; none of them covers the omitted JobRun members or the stateDetails wording, which is why this cause is filed here.
Traces: emr-serverless-spark-job-run-writes-s3-output#task#8.

Cause 2 of 2 — GetSessionEndpoint advertises a real AWS hostname instead of an address the emulator serves (fixture emr-serverless-session-endpoint-and-resource-dashboard, step task#7).
AWS: Smithy GetSessionEndpoint -> GetSessionEndpointResponse.endpoint (required, EndpointUrl @length min 1 max 256), together with applicationId, sessionId, authToken and authTokenExpiresAt (all required). The endpoint is the URL the client then connects to in order to drive the interactive session; in AWS it resolves to the account's live session host.
Emulator: the replay verdict for $.endpoint is host_not_own — the emulator echoes a session URL whose host is an amazonaws.com session host rather than the address the emulator itself is reached on (the data listener the replay connected to). The string is well-formed and length-valid, so the defect is not the shape but the authority: it names a host the emulator does not serve.
Why: a client that does what the API exists for — take endpoint and open a connection to it — leaves the emulator entirely. Against a sandbox that is an egress attempt to a public AWS hostname, which either fails to resolve/connect or, worse, reaches real AWS; either way the interactive-session flow cannot proceed and the failure surfaces far from this call.
Fix: the emr-serverless GetSessionEndpoint handler/serializer must build endpoint from the emulator's own externally reachable base address (the scheme/host/port the request arrived on) rather than emitting an AWS-shaped hostname, keeping the applicationId/sessionId path structure so clients that parse it still work.
Traces: emr-serverless-session-endpoint-and-resource-dashboard#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, 77 steps, 65 passed, score 84.4%).

Cause 1 — fixture emr-serverless-spark-job-run-writes-s3-output step task#8, GetJobRun, expected 200 / got 200:
  $.jobRun.state: expected 'SUCCESS', got 'FAILED'  (poll: 'FAILED' is in fail_on after 2 attempt(s))
  $.jobRun.stateDetails: expected '', got 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.'
  $.jobRun.mode: expected 'BATCH', got None
  $.jobRun.retryPolicy: expected {'maxAttempts': 1, 'maxFailedAttemptsPerHour': None}, got None
  $.jobRun.tags: expected {}, got None
  $.jobRun.extendedSupportEnabled: expected False, got None
  $.jobRun.configurationOverrides: expected the submitted monitoringConfiguration (managedPersistenceMonitoringConfiguration.enabled=True), got None
  $.jobRun.totalResourceUtilization: expected a populated ResourceUtilization, got None
  $.jobRun.billedResourceUtilization: expected a populated ResourceUtilization, got None
  $.jobRun.totalExecutionDurationSeconds: expected present, got None
  $.jobRun.startedAt: expected present, got None
  $.jobRun.queuedDurationMilliseconds: expected present, got None
(Timestamps createdAt/updatedAt/endedAt/attemptCreatedAt/attemptUpdatedAt are flagged only as per-run values and are not claimed as requirements here.)

Cause 2 — fixture emr-serverless-session-endpoint-and-resource-dashboard step task#7, GetSessionEndpoint, expected 200 / got 200:
  $.endpoint: host_not_own — the served endpoint URL's host is an emr-serverless-services.us-west-2.amazonaws.com session host, not the address the emulator is reached on.

Smithy consulted: services/service-com-amazonaws-emrserverless-awstoledowebservice/smithy-model — GetJobRun (output jobRun required; errors InternalServerException 500 / ResourceNotFoundException 404 / ValidationException 400), shape JobRun (state, stateDetails required; mode, retryPolicy, tags, configurationOverrides, totalResourceUtilization, billedResourceUtilization, totalExecutionDurationSeconds, startedAt, queuedDurationMilliseconds optional), shape ResourceUtilization, enum JobRunMode [BATCH, STREAMING], GetSessionEndpoint (output endpoint required, EndpointUrl @length 1..256).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture emr-serverless-spark-job-run-writes-s3-output and stop at step task#8. It creates an EMR Serverless application, calls StartJobRun with a Spark job driver, a retryPolicy and a managed-persistence monitoringConfiguration, then polls GET /applications/{applicationId}/jobruns/{jobRunId}. Observe the run settles in state FAILED (never SUCCESS), stateDetails carries the emulator's own sentence, and mode/retryPolicy/tags/configurationOverrides/totalResourceUtilization/billedResourceUtilization/totalExecutionDurationSeconds/startedAt/queuedDurationMilliseconds are all absent from the jobRun object.
3. Cause 2: replay fixture emr-serverless-session-endpoint-and-resource-dashboard to step task#7, which calls GET /applications/{applicationId}/sessions/{sessionId}/endpoint. Read $.endpoint from the 200 response and compare its scheme/host/port with the address the replay client is talking to: the response names an amazonaws.com session host the emulator does not serve, so connecting to it leaves the emulator.
4. Re-replay both fixture steps after fixing; both must pass with no mismatched keys.
