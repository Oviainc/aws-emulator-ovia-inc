Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.user-import-job-sequential-state-progression
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.user-import-job-sequential-state-progression|run#37|AssertionError|assert msg_double_start == not_created, (msg_double_start, not_created)
Issue-Commit: eb1ede2cbce186d0b7cae6d474f923ddb0f91b8a

# cognito-idp: StartUserImportJob's already-started refusal message is emulator prose instead of AWS's '<JobId> is not in the created state.'

> _Note: this issue was identified against the code base at commit `eb1ede2cbce186d0b7cae6d474f923ddb0f91b8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartUserImportJob

## Description
One net-new root cause, grounded in failing behavioral check cognito-idp.user-import-job-sequential-state-progression, which the replay report marks neither ALREADY FILED nor OWNED (the open issue that lists this check id carries a DIFFERENT replay key for it — run#16, the StopUserImportJob-before-start assertion, which now passes; the check has advanced and now fails at the StartUserImportJob double-start assertion, run line 198).

NOTE on adjacent issues (named so a fixer does not conflate them, none of which covers this failing key): 20261103T075017Z-cognito-idp-eight-refusal-messages-are-emulator-authored.md describes the same StartUserImportJob wrong-state wording as its Cause 7 but binds it to check cognito-idp.user-import-job-start-wrong-state-rejected and binds THIS check only to its Cause 5 (StopUserImportJob wording, now satisfied); 20261030T174613Z-cognito-idp-the-user-import-plane-diverges-a.md covers CSV import, role-trust validation and GetCSVHeader, not refusal wording. This is not a REOPENS: AWS refuses this call too, so the call is not wrongly refused — only its message diverges.

Cause 1 of 1: StartUserImportJob on a job that is no longer in the created state refuses with the right code but the wrong message.
AWS: Smithy (com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService) models StartUserImportJob with input {UserPoolId (required, @length 1..55, pattern ^[\w-]+_[0-9a-zA-Z]+$), JobId (required, @length 1..55, pattern ^import-[0-9a-zA-Z-]+$)} and errors InvalidParameterException 400, NotAuthorizedException 403, OperationNotEnabledException 400, PreconditionNotMetException 400, ResourceNotFoundException 404, TooManyRequestsException 429, InternalErrorException 500. Recorded behavior for the text: starting a job whose status has already left Created (Pending/InProgress/Stopping/Stopped/Succeeded/Failed/Expired) is refused HTTP 400 with error code PreconditionNotMetException and the message '<JobId> is not in the created state.' — the variable part is the request's JobId and it LEADS the message; the fixed text is the trailing ' is not in the created state.' The job's current status does NOT appear in the message, and the rejected call leaves the job's status unchanged. The same message is AWS's answer for a Start on a terminal job.
Emulator: the refusal code is PreconditionNotMetException, but the message is emulator-authored prose of the shape 'User import job <JobId> is in status <Status> and cannot be started.' — it prefixes 'User import job ', interpolates the job's current status, and ends with a different clause. Replay assertion (actual, expected): ('User import job import-ae2cea66-c3bf-41ca-bf99-5da8d6064929 is in status Pending and cannot be started.', 'import-ae2cea66-c3bf-41ca-bf99-5da8d6064929 is not in the created state.') — the ids shown are this run's values and fill the template, they are not part of the requirement.
Why: a client that matches on, or parses the job id out of, AWS's documented refusal text gets no match: the leading token is the literal 'User' instead of the job id, and the sentence ends differently. Code that distinguishes 'already started' from the other PreconditionNotMetException causes on this operation (no CSV uploaded, bad CloudWatch Logs role) by message cannot do so, and fixtures recorded against AWS do not replay. The same message is also AWS's answer for Start-on-terminal-job, so the divergence affects both legs of the state machine.
Fix: the already-started / not-in-created-state precondition branch of the StartUserImportJob handler's refusal message in the cognito-idp service (service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice, src/ops/start_user_import_job.rs, together with the job-status accessor in src/state.rs it reads). Emit PreconditionNotMetException (HTTP 400) with the message formed as the request's JobId followed by ' is not in the created state.', and keep the job's status unchanged.
Traces: cognito-idp.user-import-job-sequential-state-progression (mismatch at run line 198, assert msg_double_start == not_created).

## Evidence
Failing check (behavioral suite, service heading '### cognito-idp'): cognito-idp.user-import-job-sequential-state-progression — [mismatch].

Verbatim replay output:
  File "check_cognito_idp_user_import_job_sequential_state_progression.py", line 198, in run
    assert msg_double_start == not_created, (msg_double_start, not_created)
  AssertionError: ('User import job import-ae2cea66-c3bf-41ca-bf99-5da8d6064929 is in status Pending and cannot be started.', 'import-ae2cea66-c3bf-41ca-bf99-5da8d6064929 is not in the created state.')

Expected (AWS): HTTP 400, Error.Code = PreconditionNotMetException, Error.Message = '<JobId> is not in the created state.' — fixed text ' is not in the created state.' preceded by the request's JobId; no status word, no 'User import job' prefix.
Actual (emulator): HTTP 400, Error.Code = PreconditionNotMetException (code matches — the check's PreconditionNotMetException assertion on the line above passed), Error.Message = 'User import job <JobId> is in status <Status> and cannot be started.'

The earlier legs of the same check now pass, which localises the remaining defect: the StopUserImportJob-before-start refusal already returns AWS's 'Job is not running. The specified job has a status of Created', and the job stays in Created after that rejected Stop. Only the StartUserImportJob wrong-state message diverges. (The check does not reach its later assertions — stop-on-terminal and start-on-terminal, which expect the same '<JobId> is not in the created state.' text — because it aborts here.)

## Steps to Reproduce
1. CreateUserPool (any tier).
2. Create an IAM role whose trust policy allows sts:AssumeRole for Principal Service cognito-idp.amazonaws.com and that may write CloudWatch Logs.
3. CreateUserImportJob(JobName=<name>, UserPoolId=<pool>, CloudWatchLogsRoleArn=<that role>) -> note JobId and PreSignedUrl; Status is 'Created'.
4. GetCSVHeader(UserPoolId=<pool>) and PUT a one-row CSV built from that header (cognito:username, email, email_verified=TRUE) to the job's PreSignedUrl.
5. StartUserImportJob(UserPoolId=<pool>, JobId=<job>) -> succeeds, Status becomes Pending/InProgress.
6. Call StartUserImportJob(UserPoolId=<pool>, JobId=<job>) a SECOND time.
   Expected: HTTP 400 PreconditionNotMetException with Message == '<JobId> is not in the created state.'
   Observed: HTTP 400 PreconditionNotMetException with Message == 'User import job <JobId> is in status Pending and cannot be started.'
7. Drive the job to a terminal status (StopUserImportJob, then poll DescribeUserImportJob) and call StartUserImportJob again: AWS answers the same '<JobId> is not in the created state.' message.

Replay: run_behavioral_suite and re-replay check cognito-idp.user-import-job-sequential-state-progression.
