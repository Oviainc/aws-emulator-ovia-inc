Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.associate-targets-exceeds-limit-rejected iot.associate-targets-job-limit-exceeded-rejected iot.job-targets-limit-exceeded-rejected iot.create-job-nonexistent-target-fails iot.op-create-duplicate-job-id-rejected iot.delete-in-progress-job-requires-force iot.get-job-document-round-trips iot.e2e-package-version-publish-to-job-deployment iot.job-execution-status-per-thing iot.ota-update-lifecycle-and-delete-guard
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|CreateJob|InvalidRequestException
Issue-Key: iot.create-job-nonexistent-target-fails|_err#14|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded with {result!r}")
Issue-Key: iot.op-create-duplicate-job-id-rejected|run#11|AssertionError|assert message == f"Job {job_id} already exists.", message
Issue-Key: iot.delete-in-progress-job-requires-force|run#13|AssertionError|assert refused_in in {"IN_PROGRESS", "QUEUED", "SCHEDULED"}, (
Issue-Key: iot.get-job-document-round-trips|run#15|AssertionError|assert job.get("targetSelection") == "SNAPSHOT", job.get("targetSelection")
Issue-Key: iot.e2e-package-version-publish-to-job-deployment|run#22|KeyError|assert described_job["destinationPackageVersions"] == [package_version_arn], (
Issue-Key: iot.job-execution-status-per-thing|_eventually#15|Timeout|time.sleep(interval_s)
Issue-Key: iot|DeleteOTAUpdate|InvalidRequestException
Issue-Commit: 9ad1a007c88406c445bb3f2d55ab006d23610bba

# iot jobs plane: 8 net-new causes — CreateJob refuses thing-group targets and accepts missing things, job executions are never readable per thing, DescribeJob drops targetSelection/destinationPackageVersions, and DeleteJob/DeleteOTAUpdate guards are emulator-authored

> _Note: this issue was identified against the code base at commit `9ad1a007c88406c445bb3f2d55ab006d23610bba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
Eight net-new root causes in the IoT Jobs (and OTA) control plane, all found under the `iot` service heading. Each cause is independent.

Cause 1 of 8: CreateJob refuses a thing-GROUP ARN target outright.
AWS: Smithy CreateJob.targets is a required JobTargets list of thing or thing-group ARNs; the modelled client errors are InvalidRequestException (400), LimitExceededException (410), ResourceAlreadyExistsException (409) and ResourceNotFoundException (404). A thing-group ARN target is accepted whether or not the group currently has members (a CONTINUOUS job rolls out to members as they join); the only target-count refusal is LimitExceededException `Target list exceeds size limit 100.` past the 100-target quota.
Emulator: CreateJob with thing-group ARNs is refused InvalidRequestException with the emulator-authored message `a thing group target cannot be expanded: this emulator records no membership for thing group <group>, so the devices it names are not known` — a sentence that names the emulator itself and that AWS never emits.
Why: no group-targeted job can be created at all, so the entire AssociateTargetsWithJob/target-quota surface is unreachable and the 100-target LimitExceededException refusal is unobservable.
Fix: CreateJob target resolution — accept thing-group ARN targets without requiring recorded membership (services/service-com-amazonaws-iot-awsiotservice/src/ops/create_job.rs and job/target state).
Traces: iot.associate-targets-exceeds-limit-rejected, iot.associate-targets-job-limit-exceeded-rejected, iot.job-targets-limit-exceeded-rejected.

Cause 2 of 8: CreateJob accepts a target ARN for a thing that does not exist.
AWS: ResourceNotFoundException (modelled, 404) with the fixed text `Thing <thingName> cannot be found.`, and no job is created (DescribeJob then refuses ResourceNotFoundException `Job <jobId> cannot be found.`).
Emulator: the call succeeds with HTTP 200 and returns jobArn/jobId for a job whose only target is a nonexistent thing.
Why: over-permissive — a typo'd target silently produces a job that will never execute, instead of being rejected at create time.
Fix: CreateJob target existence validation (create_job.rs).
Traces: iot.create-job-nonexistent-target-fails.

Cause 3 of 8: CreateJob duplicate jobId refusal message.
AWS: ResourceAlreadyExistsException with the fixed text `Job <jobId> already exists.` (trailing period part of the fixed text; jobId from the request).
Emulator: `Job <jobId> already exists` — no trailing period, so the string does not match AWS.
Why: exact-message assertions fail; the refusal is otherwise correct, so this is purely wording.
Fix: CreateJob's conflict refusal message (create_job.rs).
Traces: iot.op-create-duplicate-job-id-rejected.

Cause 4 of 8: DeleteJob on a non-terminal job answers an emulator-authored message (and does not name the status).
AWS: Smithy DeleteJob models InvalidStateTransitionException (409). Deleting a job in a non-terminal status without force=true refuses with the fixed template `Job <jobId> in status <status> cannot be deleted without force flag.` where <status> is the job's status at refusal time (IN_PROGRESS/QUEUED/SCHEDULED).
Emulator: message is `Job <jobId> has 1 job execution(s) that have not finished; delete it with force to delete them too` — emulator prose that names an execution count instead of the job's status, so the status cannot be recovered from the refusal.
Why: clients that parse the refused status out of the message get nothing, and the wording is not AWS's.
Fix: DeleteJob's state-transition refusal (delete_job.rs).
Traces: iot.delete-in-progress-job-requires-force.

Cause 5 of 8: DescribeJob omits targetSelection.
AWS: Smithy Job.targetSelection (TargetSelection enum) is part of the Job structure DescribeJob returns; a job created without an explicit targetSelection defaults to SNAPSHOT and reports it.
Emulator: DescribeJob's job object has no targetSelection member at all (job.get('targetSelection') is None).
Why: a client cannot tell a SNAPSHOT job from a CONTINUOUS one — the field that decides rollout semantics is missing from every read.
Fix: the Job projection in DescribeJob (and the default applied at CreateJob) — describe_job.rs / job state.
Traces: iot.get-job-document-round-trips.

Cause 6 of 8: destinationPackageVersions is dropped by CreateJob/DescribeJob, and the package placeholder is never substituted.
AWS: Smithy CreateJob.destinationPackageVersions and Job.destinationPackageVersions both exist; DescribeJob echoes the package-version ARNs the job was created with, and GetJobDocument substitutes `${aws:iot:package:<pkg>:version:<ver>:attributes:<attr>}` with the PUBLISHED package version's attribute value.
Emulator: DescribeJob's job object has no `destinationPackageVersions` key (the check dies with KeyError: 'destinationPackageVersions'), so the reference is accepted and discarded and the document substitution it gates cannot happen.
Why: a Software-Package-Catalog deployment looks created but carries no package reference and ships an unsubstituted placeholder to devices.
Fix: persist and project destinationPackageVersions in CreateJob/DescribeJob, and the placeholder substitution in GetJobDocument (create_job.rs, describe_job.rs, get_job_document.rs).
Traces: iot.e2e-package-version-publish-to-job-deployment.

Cause 7 of 8: per-thing job executions are never readable.
AWS: Smithy DescribeJobExecution returns a JobExecution (status QUEUED initially, jobId, thingArn, queuedAt) for each thing a job targets, and ListJobExecutionsForThing lists that job for the thing; an untargeted thing refuses ResourceNotFoundException `Job execution for thing <thingName> cannot be found within job <jobId>.`
Emulator: after CreateJob(targets=[thingArn]) succeeds, a 90s poll of DescribeJobExecution(jobId, thingName) / ListJobExecutionsForThing(thingName) never yields an execution for the targeted thing, so the check times out ("Check timed out after 60.0s").
Why: the per-target execution records the whole Jobs data plane is built on do not exist, so no device-side job flow can be emulated.
Fix: materialize per-target job executions at CreateJob and serve them from DescribeJobExecution/ListJobExecutionsForThing (create_job.rs, describe_job_execution.rs, list_job_executions_for_thing.rs, job-execution state).
Traces: iot.job-execution-status-per-thing.

Cause 8 of 8: DeleteOTAUpdate invents a guard AWS does not apply.
AWS: Smithy DeleteOTAUpdate takes deleteStream and forceDeleteAWSJob (both default false) and models InvalidRequestException/ResourceNotFoundException/VersionConflictException. Recorded behavior: a plain DeleteOTAUpdate (no flags) SUCCEEDS even while the OTA update's AWS IoT job is still IN_PROGRESS — it removes the OTA record and orphans the job; forceDeleteAWSJob=true additionally deletes the job. A second delete of the same id refuses ResourceNotFoundException `Requested otaUpdateJob <otaUpdateId> does not exist`.
Emulator: the unforced delete is refused with InvalidRequestException `OTA update <id> is deployed by the job AFR_OTA-<id>, which has 1 job execution(s) that have not finished; delete it with forceDeleteAWSJob to delete the job too` — a guard and a message AWS does not have.
Why: clients that rely on the documented orphaning behavior (delete the OTA record, reap the job separately) are blocked, and the refusal's wording is emulator-authored.
Fix: remove the unforced-delete guard in DeleteOTAUpdate and keep the forceDeleteAWSJob branch (delete_ota_update.rs).
Traces: iot.ota-update-lifecycle-and-delete-guard.

## Evidence
Expected vs actual, per cause (replayed suite output):
1. CreateJob(targets=[<thing-group ARNs>]) — expected 200 (then LimitExceededException 'Target list exceeds size limit 100.' only past 100 targets); actual InvalidRequestException 'a thing group target cannot be expanded: this emulator records no membership for thing group <group>, so the devices it names are not known'.
2. CreateJob(targets=['arn:aws:iot:<region>:<account>:thing/no-such-thing-…']) — expected ResourceNotFoundException 'Thing <name> cannot be found.'; actual HTTP 200 {'jobArn': 'arn:aws:iot:…:job/<id>', 'jobId': '<id>'}.
3. Duplicate jobId — expected 'Job <id> already exists.'; actual 'Job <id> already exists'.
4. DeleteJob (no force) on IN_PROGRESS job — expected InvalidStateTransitionException 'Job <id> in status IN_PROGRESS cannot be deleted without force flag.'; actual message 'Job <id> has 1 job execution(s) that have not finished; delete it with force to delete them too'.
5. DescribeJob — expected job['targetSelection'] == 'SNAPSHOT'; actual key absent (None).
6. DescribeJob after CreateJob(destinationPackageVersions=[arn]) — expected job['destinationPackageVersions'] == [arn]; actual KeyError: 'destinationPackageVersions'.
7. DescribeJobExecution(jobId, thingName) for a targeted thing — expected execution with status QUEUED/IN_PROGRESS; actual never materialized within the poll budget (check timed out after 60.0s).
8. DeleteOTAUpdate(otaUpdateId) with the job still IN_PROGRESS — expected 200 and the job left orphaned IN_PROGRESS; actual InvalidRequestException 'OTA update <id> is deployed by the job AFR_OTA-<id>, which has 1 job execution(s) that have not finished; delete it with forceDeleteAWSJob to delete the job too'.

## Steps to Reproduce
Replay the listed checks (suite simparity-behavioral-aws-v1):
1. CreateThingGroup(g); CreateJob(jobId=j, targets=[<g's ARN>], targetSelection='CONTINUOUS', document='{"op":"noop"}') — observe the InvalidRequestException.
2. CreateJob(jobId=j, targets=['arn:aws:iot:<region>:<account>:thing/<missing>']) — observe 200.
3. CreateJob(j, …) twice — inspect the duplicate message.
4. CreateJob on a live thing, wait for IN_PROGRESS, DeleteJob(jobId=j) without force — inspect code+message.
5. CreateJob without targetSelection, then DescribeJob — inspect job['targetSelection'].
6. CreatePackage/CreatePackageVersion + PUBLISH, CreateJob(destinationPackageVersions=[arn]), DescribeJob and GetJobDocument.
7. CreateJob(targets=[thingArn]), then poll DescribeJobExecution(jobId, thingName) and ListJobExecutionsForThing(thingName).
8. CreateOTAUpdate to CREATE_COMPLETE with an IN_PROGRESS job, then DeleteOTAUpdate(otaUpdateId) with no flags.
