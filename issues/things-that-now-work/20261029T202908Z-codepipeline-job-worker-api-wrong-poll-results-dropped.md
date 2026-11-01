Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-action-revision-and-approval-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-action-revision-and-approval-error-paths codepipeline-custom-action-job-worker-api codepipeline-custom-action-type-lifecycle codepipeline-third-party-job-worker-api
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|PollForJobs|4xx->4xx|$.message
Issue-Key: codepipeline|PollForJobs|2xx->2xx|$
Issue-Key: codepipeline|AcknowledgeJob|4xx->4xx|$.message
Issue-Key: codepipeline|GetJobDetails|4xx->4xx|$.message
Issue-Key: codepipeline|PutJobSuccessResult|4xx->4xx|$.message
Issue-Key: codepipeline|PutJobFailureResult|4xx->4xx|$.message
Issue-Key: codepipeline|PollForJobs|2xx->4xx|$.jobs
Issue-Key: codepipeline|PollForJobs|2xx->4xx|status
Issue-Key: codepipeline|GetActionType|4xx->4xx|$.message
Issue-Key: codepipeline|PollForThirdPartyJobs|4xx->4xx|$.Message
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CodePipeline job-worker API: wrong poll results, dropped empty jobs list and non-AWS refusal wording

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PollForJobs

## Description
The job-worker surface (PollForJobs, PollForThirdPartyJobs, AcknowledgeJob, GetJobDetails, PutJobSuccessResult, PutJobFailureResult, GetActionType) diverges in its poll results and in the text/shape of its refusals.

Cause 1 of 9 — PollForJobs for an unregistered action type uses non-AWS wording. AWS: 400 ActionTypeNotFoundException (Smithy: PollForJobs errors = ActionTypeNotFoundException, ValidationException) with message "ActionType (Category: '<category>', Owner: '<owner>', Provider: '<provider>', Version: '<version>') is not available" — fixed text with the four ActionTypeId members taken from the request. Emulator: 400 with "ActionType (Category='<c>', Owner='<o>', Provider='<p>', Version='<v>') is not registered in account '<accountId>'" — different separators, different tail, and an account id AWS does not include. Fix: PollForJobs' action-type lookup refusal message. Traces: codepipeline-action-revision-and-approval-error-paths#task#4 (2 affected steps, incl. codepipeline-custom-action-job-worker-api#task#7).

Cause 2 of 9 — PollForJobs drops the empty jobs list. AWS: 200 with body {"jobs": []} (Smithy: PollForJobsOutput.jobs -> JobList); AWS always serializes the list, empty or not. Emulator: 200 with an empty object {}. Why: a worker that dereferences response.jobs gets null instead of an empty list. Fix: PollForJobs' response serializer — emit jobs unconditionally. Traces: codepipeline-custom-action-job-worker-api#task#2.

Cause 3 of 9 — AcknowledgeJob's JobNotFoundException message has an extra period and quotes the id. AWS: 400 JobNotFoundException (Smithy: AcknowledgeJob errors include JobNotFoundException) with message "Job with id <jobId> does not exist" — unquoted job id, no trailing period. Emulator: "Job with id '<jobId>' does not exist.". Fix: the shared job-lookup refusal message used by AcknowledgeJob. Traces: codepipeline-custom-action-job-worker-api#task#3.

Cause 4 of 9 — GetJobDetails, same divergence as Cause 3: AWS "Job with id <jobId> does not exist" vs emulator "Job with id '<jobId>' does not exist.". Fix: GetJobDetails' refusal message. Traces: codepipeline-custom-action-job-worker-api#task#4.

Cause 5 of 9 — PutJobSuccessResult: AWS quotes the id here but still has NO trailing period — "Job with id '<jobId>' does not exist"; the emulator appends '.'. Fix: PutJobSuccessResult's refusal message (note AWS's wording differs between the acknowledge/details family and the put-result family, so one shared string cannot satisfy both). Traces: codepipeline-custom-action-job-worker-api#task#5.

Cause 6 of 9 — PutJobFailureResult: same as Cause 5 — expected "Job with id '<jobId>' does not exist", emulator adds the trailing period. Fix: PutJobFailureResult's refusal message. Traces: codepipeline-custom-action-job-worker-api#task#6.

Cause 7 of 9 — PollForJobs refuses a poll AWS answers successfully. AWS: 200 with {"jobs": []} for a poll of a registered custom action type with no queued work. Emulator: 400 with no jobs member at all. Why: a worker loop that polls after the earlier steps of the lifecycle gets a hard client error instead of an empty poll and stops. Fix: PollForJobs' action-type resolution — the registered type must stay resolvable for the poll that follows the job-worker lifecycle steps. Traces: codepipeline-custom-action-job-worker-api#task#9.

Cause 8 of 9 — GetActionType's ActionTypeNotFoundException message. AWS: 400 ActionTypeNotFoundException (Smithy: GetActionType errors = ActionTypeNotFoundException, ValidationException) with message "Action type with owner=<owner>, category=<category>, provider=<provider> and version=<version> does not exist." — bare values, trailing period. Emulator: "An action type with owner=[<owner>], category=[<category>], provider=[<provider>] and version=[<version>] doesn't exist. To create an action type, contact the CodePipeline service team." — bracketed values plus a sentence AWS does not emit. Fix: GetActionType's lookup refusal message. Traces: codepipeline-custom-action-type-lifecycle#task#4.

Cause 9 of 9 — PollForThirdPartyJobs refusal carries no message at all. AWS: 400 with a top-level `Message` member reading "Not whitelisted for accessing ActionType.Category='<category>' ActionType.Owner='<owner>' ActionType.Provider='<provider>' ActionType.Version='<version>'" — fixed text with the request's ActionTypeId values (recorded behavior: this operation's refusal uses the capitalized `Message` key). Emulator: the member is absent entirely, so the error body carries no human-readable cause. Fix: PollForThirdPartyJobs' third-party authorization refusal — emit the message member. Traces: codepipeline-third-party-job-worker-api#task#1.

Related but not covering: open issue 20261028T112857Z-codepipeline-job-worker-lifecycle-and-refusal-wording-8.md; the replay reports owned_by=[] for all nine clusters here.

## Evidence
codepipeline-action-revision-and-approval-error-paths#task#4 (PollForJobs, 400->400): expected "ActionType (Category: 'Deploy', Owner: 'Custom', Provider: '<p>', Version: '1') is not available" / got "ActionType (Category='Deploy', Owner='Custom', Provider='<p>', Version='1') is not registered in account '000000000000'".
codepipeline-custom-action-job-worker-api#task#2 (PollForJobs, 200->200): '$: empty_body (expected {"jobs": []}, got {})'.
#task#3 AcknowledgeJob / #task#4 GetJobDetails (400->400): expected 'Job with id <id> does not exist' / got "Job with id '<id>' does not exist.".
#task#5 PutJobSuccessResult / #task#6 PutJobFailureResult (400->400): expected "Job with id '<id>' does not exist" / got "Job with id '<id>' does not exist.".
#task#9 PollForJobs: 'status: expected 200, got 400', '$.jobs: missing (expected [], got None)'.
codepipeline-custom-action-type-lifecycle#task#4 (GetActionType, 400->400): expected 'Action type with owner=ThirdParty, category=Build, provider=<p> and version=1 does not exist.' / got "An action type with owner=[ThirdParty], ... doesn't exist. To create an action type, contact the CodePipeline service team.".
codepipeline-third-party-job-worker-api#task#1 (PollForThirdPartyJobs, 400->400): "$.Message: missing (expected \"Not whitelisted for accessing ActionType.Category='Source' ...\", got None)".

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus.
2. PollForJobs with an unregistered Custom action type -> compare the ActionTypeNotFoundException message; PollForJobs with a registered type and no work -> body must be {"jobs": []}, observe {} (and a 400 at codepipeline-custom-action-job-worker-api#task#9).
3. AcknowledgeJob / GetJobDetails / PutJobSuccessResult / PutJobFailureResult with a random UUID job id -> compare each message's quoting and trailing period.
4. GetActionType for an owner=ThirdParty type that does not exist, and PollForThirdPartyJobs for a non-whitelisted type -> compare message text and the presence of the message member.
5. Re-replay codepipeline-action-revision-and-approval-error-paths#task#4, codepipeline-custom-action-job-worker-api#task#2/#task#3/#task#4/#task#5/#task#6/#task#9, codepipeline-custom-action-type-lifecycle#task#4 and codepipeline-third-party-job-worker-api#task#1.
