Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-medical-scribe-job-workflow
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-medical-scribe-job-workflow
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe|GetMedicalScribeJob|4xx->4xx|$.Message
Issue-Key: transcribe|StartMedicalScribeJob|4xx->4xx|$.Message
Issue-Key: transcribe|ListMedicalScribeJobs|4xx->2xx|$.Message
Issue-Key: transcribe|ListMedicalScribeJobs|4xx->2xx|$.__type
Issue-Key: transcribe|ListMedicalScribeJobs|4xx->2xx|status
Issue-Key: transcribe|DeleteMedicalScribeJob|4xx->4xx|$.Message
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# Medical Scribe operations are served normally instead of refused as account-not-authorized

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMedicalScribeJob

## Description
The recorded AWS account refuses the ENTIRE Medical Scribe (HealthScribe) API with one authorization refusal: every operation of the family — read, start, list and delete — answers BadRequestException at HTTP 400 with the fixed text "Your account isn't authorized to call this operation. Check your account permissions and try your request again.", regardless of whether the named job exists and regardless of whether the request would otherwise validate. The emulator instead implements the family as an ordinary job API: it runs each operation's own validation and returns per-operation not-found / bad-URI refusals, and for the list operation it succeeds outright. 4 causes, one per operation.

Smithy: GetMedicalScribeJob, StartMedicalScribeJob, ListMedicalScribeJobs and DeleteMedicalScribeJob all model BadRequestException (400, Message -> FailureReason) — the carrier AWS uses here — so the code and status are modeled, while the authorization-gate behavior and the message text are recorded behavior only. Hypothesis: the family is gated behind an account-level entitlement (HealthScribe must be enabled for the account), which is why the refusal precedes every per-operation check; the recording only shows that the refusal is uniform and unconditional.

Cause 1 of 4: GetMedicalScribeJob.
AWS: BadRequestException, 400, message exactly "Your account isn't authorized to call this operation. Check your account permissions and try your request again." (entirely fixed; the MedicalScribeJobName is not interpolated), returned for a job name that was never created.
Emulator: BadRequestException, 400, but "The requested job couldn't be found. Check the job name and try your request again." — it performed the job lookup instead of refusing the call.
Why: the client is told the job is missing rather than that the account may not call the API, so it retries or creates instead of surfacing the entitlement problem. Code that distinguishes a permissions failure from a missing resource takes the wrong branch.
Fix: the Medical Scribe operation group in the transcribe handler needs the account-authorization gate applied before any per-operation validation or state lookup, emitting BadRequestException with the fixed message above.
Traces: transcribe-medical-scribe-job-workflow#task#1

Cause 2 of 4: StartMedicalScribeJob.
AWS: the same BadRequestException/400 and the same fixed message, even though the request carried Media, OutputBucketName, DataAccessRoleArn and Settings — the gate fires before input validation.
Emulator: BadRequestException, 400, with "The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again." — it ran media resolution and reported that instead.
Why: the caller is pointed at its media URI as the fault when the real refusal is authorization; the message is also the only discriminator, since both answers are BadRequestException/400.
Fix: same gate, applied ahead of StartMedicalScribeJob's media/role validation.
Traces: transcribe-medical-scribe-job-workflow#task#2

Cause 3 of 4: ListMedicalScribeJobs — wrong STATUS, not just wrong text.
AWS: BadRequestException at HTTP 400 with the same fixed message; the list is refused like every other operation of the family.
Emulator: HTTP 200 with a successful list response ($.__type and $.Message both absent).
Why: the most damaging of the four — a negative path becomes a success. A client that enumerates Medical Scribe jobs to decide whether the feature is usable concludes it is, and no error-handling path is ever exercised.
Fix: same gate; it must cover the collection operations, not only the ones that take a job name.
Traces: transcribe-medical-scribe-job-workflow#task#3

Cause 4 of 4: DeleteMedicalScribeJob.
AWS: the same BadRequestException/400 with the same fixed message.
Emulator: BadRequestException, 400, with "The requested job couldn't be found. Check the job name and try your request again.".
Why: same as cause 1, on the delete path; a cleanup routine treats the refusal as "already gone" and proceeds.
Fix: same gate, ahead of DeleteMedicalScribeJob's lookup.
Traces: transcribe-medical-scribe-job-workflow#task#4

## Evidence
The expected message is byte-identical on all four causes and is entirely fixed text with no request- or state-derived parts: "Your account isn't authorized to call this operation. Check your account permissions and try your request again." (ASCII apostrophe in "isn't").
Cause 1 (task#1, GetMedicalScribeJob, 400 -> 400): $.Message value_mismatch — got "The requested job couldn't be found. Check the job name and try your request again.".
Cause 2 (task#2, StartMedicalScribeJob, 400 -> 400): $.Message value_mismatch — got "The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again.".
Cause 3 (task#3, ListMedicalScribeJobs): status expected 400, got 200; $.__type missing (expected 'BadRequestException'); $.Message missing (expected the authorization text).
Cause 4 (task#4, DeleteMedicalScribeJob, 400 -> 400): $.Message value_mismatch — got "The requested job couldn't be found. Check the job name and try your request again.".

## Steps to Reproduce
Against a fresh account/state, with no Medical Scribe job created:
1. GetMedicalScribeJob(MedicalScribeJobName=<any name>) — expect BadRequestException/400 with the authorization message; observe the not-found message.
2. StartMedicalScribeJob(MedicalScribeJobName=<new>, Media={MediaFileUri:'s3://<bucket>/<key>'}, OutputBucketName=<bucket>, DataAccessRoleArn='arn:aws:iam::000000000000:role/<role>', Settings={...}) — expect the same authorization refusal; observe the S3-object message.
3. ListMedicalScribeJobs() — expect the same authorization refusal at 400; observe HTTP 200 with a list body.
4. DeleteMedicalScribeJob(MedicalScribeJobName=<any name>) — expect the same authorization refusal; observe the not-found message.
Replay: transcribe-medical-scribe-job-workflow steps task#1 through task#4.
