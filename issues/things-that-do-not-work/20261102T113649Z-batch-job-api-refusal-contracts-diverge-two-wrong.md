Issue-Source: trace-sweep-divergence
Issue-Trace: batch-batch-diag-describe-and-list-jobs-handle-mistakes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-batch-diag-describe-and-list-jobs-handle-mistakes batch-batch-diag-job-definition-log-configuration-absent-vs-override batch-batch-diag-submit-job-reference-mistakes
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|DescribeJobs|4xx->2xx|$.__type
Issue-Key: batch|DescribeJobs|4xx->2xx|$.message
Issue-Key: batch|DescribeJobs|4xx->2xx|status
Issue-Key: batch|DescribeJobs|4xx->4xx|$.message
Issue-Key: batch|ListJobs|2xx->4xx|$.jobSummaryList
Issue-Key: batch|ListJobs|2xx->4xx|status
Issue-Key: batch|RegisterJobDefinition|4xx->4xx|$.message
Issue-Key: batch|SubmitJob|4xx->4xx|$.message
Issue-Commit: a4ef059b31cd49fc5529baf331c68065b4514ad5

# batch job-API refusal contracts diverge: two wrong verdicts and three emulator-authored messages (5 causes)

> _Note: this issue was identified against the code base at commit `a4ef059b31cd49fc5529baf331c68065b4514ad5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJobs

## Description
Five net-new root causes on the batch job plane, all in the same place: the verdict and the message the emulator produces when a job-plane request is wrong (or when it is right and the emulator thinks it is wrong). Each must be closed before submitting.

Cause 1 of 5 - DescribeJobs answers 200 for a request AWS refuses with ClientException.
AWS: Smithy (com.amazonaws.batch) DescribeJobs, POST /v1/describejobs, success code 200, modeled errors ClientException (httpError 400) and ServerException (500); input member `jobs` is @required (StringList). The recorded response for this step is HTTP 400 with a JSON body carrying `__type` = "ClientException" and a `message` member (the recorded message text is shaped/volatile-redacted by the comparator, so only its presence and class are asserted) - so AWS rejects this `jobs` request outright and describes nothing.
Emulator: HTTP 200 with a body that has neither `$.__type` nor `$.message` - the malformed describe is accepted and answered as a success.
Why: the 400/ClientException is the only signal that the caller's `jobs` argument is unusable. Accepted with 200, a client (or a wrapper SDK that raises on ClientException) concludes the job ids it asked about are simply all absent, so a typo'd or malformed identifier is indistinguishable from a job that does not exist and the mistake surfaces much later, somewhere else.
Fix: DescribeJobs' request validation in the batch crate's DescribeJobs handler - the `jobs` member must be validated before the state lookup and refused with 400 / ClientException.
Traces: batch-batch-diag-describe-and-list-jobs-handle-mistakes#task#3

Cause 2 of 5 - DescribeJobs' refusal for an absent/null `jobs` member carries a message that is not the one AWS sends.
AWS: DescribeJobs with `jobs` absent is refused with HTTP 400 / ClientException (model: `jobs` @required, ClientException httpError 400). The recorded message for that refusal is comparator-redacted (shaped, not asserted literally), but it is of a different class than the text the emulator emits; the status and error class match, the message does not.
Emulator: HTTP 400 with `$.message` = "1 validation error detected: Value null at 'jobs' failed to satisfy constraint: Member must not be null". In that string the fixed text is "1 validation error detected: Value null at '<member>' failed to satisfy constraint: Member must not be null" and the only variable part is the member name taken from the request (`jobs`).
Why: the message is the only machine-readable detail a 400 ClientException carries (Batch has one client error shape for everything), so clients and test suites match on its text to tell a malformed request from a missing resource. A message AWS never sends makes those matches fail against the emulator and pass against AWS, or the reverse.
Hypothesis: AWS wraps this refusal in Batch's own "Error executing request, Exception : <text>, RequestId: <uuid>" envelope rather than emitting the bare constraint sentence - the same envelope whose absence is already filed for a sibling operation in open issue 20261030T204009Z-batch-describeschedulingpolicies-malformed-arn-refusal-is-not-wrapped.md. That issue covers DescribeSchedulingPolicies only and does not carry this step's replay key.
Fix: the message produced by DescribeJobs' input validation in the batch crate (the shared Batch client-error formatting it goes through), not the status, which is already correct.
Traces: batch-batch-diag-describe-and-list-jobs-handle-mistakes#task#4

Cause 3 of 5 - ListJobs refuses with 400 a listing AWS answers 200 with an empty page.
AWS: Smithy ListJobs, POST /v1/listjobs, success 200, output member `jobSummaryList` (JobSummaryList) is @required, operation is @paginated (inputToken/outputToken nextToken, items jobSummaryList). The recording for this step is HTTP 200 with `jobSummaryList` = [] - a listing whose selector matches nothing is an empty page, not an error.
Emulator: HTTP 400 (ClientException) and no `$.jobSummaryList` member at all.
Why: a listing that matches nothing is normal. Turning it into a 400 breaks every paginator and every "list, then act on each" loop: the client raises where AWS would have iterated zero times, so provisioning code that polls for jobs cannot run against the emulator at all.
Fix: ListJobs' handler in the batch crate - resolve the request's selector against state and return a 200 with an empty `jobSummaryList` (and `jobSummaryList` always present, per the @required output member) instead of refusing. The same empty-page-vs-refusal fault is already filed for ListQuotaShares in open issue 20261029T234420Z-batch-refusal-contracts-diverge-not-found-arn-wording.md; that issue does not cover ListJobs and does not carry this step's replay key.
Traces: batch-batch-diag-describe-and-list-jobs-handle-mistakes#task#6

Cause 4 of 5 - RegisterJobDefinition's invalid log-driver refusal is emulator-authored prose inside Batch's request envelope.
AWS: Smithy enum com.amazonaws.batch#LogDriver has exactly the values [json-file, syslog, journald, gelf, fluentd, awslogs, splunk, awsfirelens], and LogConfiguration.logDriver is @required; RegisterJobDefinition models ClientException (400). The recording is HTTP 400 with a message of a different class than the emulator's text (the recorded text is comparator-redacted). So the emulator's accepted value set is right and only the wording of the refusal diverges.
Emulator: HTTP 400 with `$.message` = "Error executing request, Exception : Log driver must be one of [json-file, syslog, journald, gelf, fluentd, awslogs, splunk, awsfirelens]., RequestId: <uuid>". Fixed text there is "Error executing request, Exception : Log driver must be one of [...]., RequestId: "; the trailing uuid is a per-run request id and is not itself the defect.
Why: the sentence "Log driver must be one of [...]" is the emulator's own prose. A client that matches on AWS's constraint wording to decide whether the log driver or something else in the job definition was rejected gets no match, and the refusal does not name the value it rejected, so the caller cannot tell which of several log configurations in the request was bad.
Hypothesis: AWS reports an unmodeled enum value with the standard constraint message - "1 validation error detected: Value '<rejected value>' at 'containerProperties.logConfiguration.logDriver' failed to satisfy constraint: Member must satisfy enum value set: [json-file, syslog, journald, gelf, fluentd, awslogs, splunk, awsfirelens]" - where the rejected value and the member path come from the request and the value set is the model's.
Fix: the logConfiguration validation inside RegisterJobDefinition's validation layer in the batch crate - emit the modeled constraint message for the offending member instead of hand-written prose.
Traces: batch-batch-diag-job-definition-log-configuration-absent-vs-override#task#3

Cause 5 of 5 - SubmitJob's unknown-job-definition refusal uses the wrong noun and omits the ARN AWS names.
AWS: Smithy SubmitJob, POST /v1/submitjob, models ClientException (400); `jobDefinition` is @required. The recording is HTTP 400 with `$.message` = "JobDefinition <job-definition ARN> not found." - the fixed text is "JobDefinition " + " not found." and the one variable part is the job definition the request named, resolved to its full ARN arn:aws:batch:<region>:<account-id>:job-definition/<name>:<revision>. (The ARN in the recording is this run's value filling that template, not a requirement.)
Emulator: HTTP 400 with `$.message` = "JobDefinition name <name>:<revision> does not exist or is not in ACTIVE status." - a different sentence, naming the bare name:revision instead of the ARN and conflating "absent" with "not ACTIVE".
Why: Batch answers every client mistake with the same ClientException, so the message is what a caller matches on. A client that detects a missing job definition by AWS's wording treats the emulator's refusal as an unknown failure, and the message does not echo the ARN that AWS reports, so logs and error surfaces lose the identifier the caller needs to fix the submit. The cluster covers three SubmitJob steps in the same fixture.
Fix: the not-found refusal text in SubmitJob's job-definition resolution in the batch crate's SubmitJob handler.
Traces: batch-batch-diag-submit-job-reference-mistakes#task#3 (cluster also covers task#4 and task#5 of the same fixture)

## Evidence
All five from the simparity-wire-aws-v1 replay against this checkout (18 fixtures, 128 steps, 105 passed, 23 mismatched, score 82%). Expected = recorded AWS response; actual = emulator.

Cause 1 - batch-batch-diag-describe-and-list-jobs-handle-mistakes#task#3, DescribeJobs:
  status: expected 400, got 200
  $.__type: missing (expected 'ClientException', got None)
  $.message: missing (expected '<shaped-not-asserted>', got None)

Cause 2 - batch-batch-diag-describe-and-list-jobs-handle-mistakes#task#4, DescribeJobs (status 400 on both sides):
  $.message: value_mismatch (expected '<shaped-not-asserted>', got "1 validation error detected: Value null at 'jobs' failed to satisfy constraint: Member must not be null")

Cause 3 - batch-batch-diag-describe-and-list-jobs-handle-mistakes#task#6, ListJobs:
  status: expected 200, got 400
  $.jobSummaryList: missing (expected [], got None)

Cause 4 - batch-batch-diag-job-definition-log-configuration-absent-vs-override#task#3, RegisterJobDefinition (status 400 on both sides):
  $.message: value_mismatch (expected '<shaped-not-asserted>', got 'Error executing request, Exception : Log driver must be one of [json-file, syslog, journald, gelf, fluentd, awslogs, splunk, awsfirelens]., RequestId: <uuid>')

Cause 5 - batch-batch-diag-submit-job-reference-mistakes#task#3 (cluster: task#3, task#4, task#5), SubmitJob (status 400 on both sides):
  $.message: value_mismatch (expected 'JobDefinition arn:aws:batch:<region>:<account-id>:job-definition/<name>:<revision> not found.', got 'JobDefinition name <name>:<revision> does not exist or is not in ACTIVE status.')

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), or by hand:

Cause 1: POST /v1/describejobs with the malformed `jobs` request recorded at batch-batch-diag-describe-and-list-jobs-handle-mistakes#task#3 -> emulator 200 with no __type/message; AWS answers 400 with {"__type":"ClientException","message":...}.

Cause 2: POST /v1/describejobs with body {} (no `jobs`) -> 400 with message "1 validation error detected: Value null at 'jobs' failed to satisfy constraint: Member must not be null"; AWS's recorded message for that refusal has a different form.

Cause 3: POST /v1/listjobs with the selector recorded at batch-batch-diag-describe-and-list-jobs-handle-mistakes#task#6 -> emulator 400 ClientException; AWS answers 200 {"jobSummaryList": []}.

Cause 4: aws batch register-job-definition --job-definition-name t --type container --container-properties '{"image":"public.ecr.aws/amazonlinux/amazonlinux","vcpus":1,"memory":512,"logConfiguration":{"logDriver":"not-a-driver"}}' -> message 'Error executing request, Exception : Log driver must be one of [...]., RequestId: ...'; AWS reports the modeled enum constraint violation naming containerProperties.logConfiguration.logDriver and the rejected value.

Cause 5: aws batch submit-job --job-name j --job-queue <existing queue> --job-definition <name>:99 where revision 99 does not exist -> 'JobDefinition name <name>:99 does not exist or is not in ACTIVE status.'; AWS answers 'JobDefinition arn:aws:batch:<region>:<account-id>:job-definition/<name>:99 not found.'

Then re-replay: batch-batch-diag-describe-and-list-jobs-handle-mistakes#task#3, #task#4, #task#6; batch-batch-diag-job-definition-log-configuration-absent-vs-override#task#3; batch-batch-diag-submit-job-reference-mistakes#task#3.
