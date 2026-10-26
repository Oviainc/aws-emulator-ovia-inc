Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemakerjobruntime.sample-nonexistent-job-not-found sagemakerjobruntime.sample-malformed-job-arn-rejected sagemakerjobruntime.failed-sample-leaves-no-residual-trajectory-state sagemakerjobruntime.sample-with-response-stream-nonexistent-job-not-found sagemakerjobruntime.sample-with-response-stream-malformed-arn-raises-synchronously
Issue-Shape-Id: com.amazonaws.sagemakerjobruntime#AgenticRFTRuntimeService
Issue-Key: sagemakerjobruntime.sample-nonexistent-job-not-found|_error_code#7|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: sagemakerjobruntime.sample-malformed-job-arn-rejected|_error_code#13|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Key: sagemakerjobruntime.failed-sample-leaves-no-residual-trajectory-state|_error_code#7|AssertionError|raise AssertionError(
Issue-Key: sagemakerjobruntime.sample-with-response-stream-nonexistent-job-not-found|run#10|AssertionError|assert response is None, "nonexistent job must not yield a stream response object"
Issue-Key: sagemakerjobruntime.sample-with-response-stream-malformed-arn-raises-synchronously|run#14|AssertionError|assert control_response is None, (
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# Sample and SampleWithResponseStream perform no job-existence lookup: a well-formed but never-created JobArn is served 200 and silently materializes trajectory state (2 causes)

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Sample

## Description
Both data-plane sampling operations now accept ANY JobArn that satisfies the modelled ARN @pattern, with no job lookup at all. The job registry that used to back them was removed outright (both handlers carry the comment "this service's model binds no operation that creates, lists or deletes a job, so there is no registry to resolve it against and nothing to refuse it on"), so the emulator swung from over-refusing to over-accepting. On real AWS the job-existence check for these two operations is folded into request validation and a nonexistent job is refused HTTP 400 ValidationException. Note the two earlier issues in this area: the open issue 20261024T054010Z-sagemakerjobruntime-sample-wrong-behavior and the FIXED issue 20261024T054010Z-sagemakerjobruntime-samplewithresponsestream-wrong-behavior both argued the registry should admit any pattern-valid ARN; the replayed checks show real AWS does NOT, so dropping the lookup entirely is not the right end state — a reachable registration path plus a lookup is. These checks fail on a SUCCESS, not on a refusal, so this is not a reopen of a refused call.

Cause 1 of 2: Sample does not refuse a well-formed but never-created JobArn, and worse, the failed-path call creates durable trajectory state.
AWS: Smithy (com.amazonaws.sagemakerjobruntime#Sample) binds JobArn as @required httpHeader X-Amzn-SageMaker-Job-Arn with @length 1..256 and @pattern ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$, and models ValidationException (HTTP 400) and ResourceNotFoundException (HTTP 404) among its errors. Recorded behavior: a pattern-valid ARN for a job that was never created is refused HTTP 400 code ValidationException; the message rule is built from the request — it names the rejected ARN, e.g. "Could not find job with jobArn arn:aws:sagemaker:<region>:<account>:job/AgentRFT/<name>". Because that Sample is refused, no trajectory is appended, so a following CompleteRollout and UpdateReward on the same JobArn/TrajectoryId are refused HTTP 404 code ResourceNotFoundException (message rule: names the trajectory id and the account, e.g. "Trajectory <id> not found for account <account>"), never ConflictException and never 200.
Emulator: the call returns HTTP 200 with ContentType application/json and a generated inference Body (replay shows a 549-byte 200 response). The ARN-pattern stage does work — a malformed ARN ("not-a-valid-job-arn") is correctly refused 400 ValidationException "Input arn not-a-valid-job-arn is malformed" — but nothing after it resolves the job, so the control arm of the ARN-validation check also succeeds instead of being refused. The successful Sample additionally appends a turn, leaving residual trajectory state that the residual-state check then observes.
Why: a client cannot distinguish a real job from a typo'd or cross-account one — code that relies on the 400 "could not find job" to detect a stale/wrong job ARN (retry, re-resolve, fail fast) instead gets a plausible-looking inference payload and records turns against a job that does not exist, and the downstream CompleteRollout/UpdateReward refusals the client expects never arrive. It also destroys the negative-class signal: every Sample call succeeds regardless of input.
Fix: the Sample handler's validation stage plus the service's state store — restore a job lookup between ARN-pattern validation and append_turn, refusing with the operation's modelled ValidationException, and make the registry reachable (a wire/control-plane registration path) rather than removing the check; the append must not run on the refused path.
Traces: sagemakerjobruntime.sample-nonexistent-job-not-found, sagemakerjobruntime.sample-malformed-job-arn-rejected (control arm), sagemakerjobruntime.failed-sample-leaves-no-residual-trajectory-state.

Cause 2 of 2: SampleWithResponseStream has the same missing lookup and hands back a stream object for a nonexistent job.
AWS: Smithy (com.amazonaws.sagemakerjobruntime#SampleWithResponseStream) binds the same JobArn member and constraints, output Body is a @streaming ResponseStream, and the operation models ValidationException (HTTP 400). Recorded behavior: for a pattern-valid but never-created job the rejection is synchronous on the call itself — HTTP 400 code ValidationException whose message names the ARN ("Could not find job with jobArn <arn>") — and no response/stream object is returned at all, so the error is never deferred to event-stream consumption. The malformed-ARN rejection and the job-not-found rejection are distinct stages and therefore carry distinct messages.
Emulator: the call returns a 200 with a stream Body for a job that was never created (both the dedicated not-found check and the control arm of the synchronous-validation check assert no stream object and get one). The malformed-ARN arm is correct (400 ValidationException "Input arn not-a-valid-job-arn is malformed"), so only the job-resolution stage is missing.
Why: a streaming client is handed a stream for a job that does not exist and consumes synthetic turns; the two rejection classes collapse to one, so no client can tell "bad ARN syntax" from "job gone", and the trajectory is mutated via the shared append path exactly as in cause 1.
Fix: the SampleWithResponseStream handler's validation stage, before the event stream is opened, sharing the same job lookup as Sample (both handlers call the same crate state append_turn path).
Traces: sagemakerjobruntime.sample-with-response-stream-nonexistent-job-not-found, sagemakerjobruntime.sample-with-response-stream-malformed-arn-raises-synchronously (control arm).

## Evidence
Replay output (5/10 sagemakerjobruntime checks failed, all five on this root cause):

1) sagemakerjobruntime.sample-nonexistent-job-not-found — expected 400 ValidationException "Could not find job with jobArn <arn>"; got: AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-type': 'application/json', 'content-length': '549'}, 'ContentType': 'application/json', 'Body': <botocore.response.StreamingBody ...>}.

2) sagemakerjobruntime.sample-malformed-job-arn-rejected — the malformed arm passed (400 ValidationException "Input arn not-a-valid-job-arn is malformed"); the well-formed-but-nonexistent control arm failed: AssertionError: expected the call to be rejected, but it succeeded (expected ("ValidationException", "Could not find job with jobArn <arn>")).

3) sagemakerjobruntime.failed-sample-leaves-no-residual-trajectory-state — AssertionError: expected the call to be rejected (no backing job/trajectory exists), but it SUCCEEDED and returned None. Expected chain: Sample -> 400 ValidationException "Could not find job with jobArn <arn>"; then CompleteRollout and UpdateReward on the same JobArn/TrajectoryId -> 404 ResourceNotFoundException "Trajectory <id> not found for account <account>".

4) sagemakerjobruntime.sample-with-response-stream-nonexistent-job-not-found — AssertionError: nonexistent job must not yield a stream response object (expected 400 ValidationException "Could not find job with jobArn <arn>").

5) sagemakerjobruntime.sample-with-response-stream-malformed-arn-raises-synchronously — malformed arm passed; control arm failed: AssertionError: well-formed but nonexistent job must not yield a stream response object either.

Code state in the checkout: services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/ops/sample.rs lines 165-179 and .../src/ops/sample_with_response_stream.rs lines 167-181 both replace the former job lookup with a comment asserting there is "no registry to resolve it against and nothing to refuse it on", then go straight to the empty-Body check and crate::state::append_turn (sample.rs lines 192-229, sample_with_response_stream.rs lines 196-230). Model: smithy_query operation=Sample,SampleWithResponseStream confirms JobArn's pattern/length and that both operations model ValidationException (400) and ResourceNotFoundException (404).

## Steps to Reproduce
1. Build and boot the emulator from the checkout.
2. POST /sample with headers X-Amzn-SageMaker-Job-Arn: arn:aws:sagemaker:us-east-1:123456789012:job/AgentRFT/never-created-job, X-Amzn-SageMaker-Trajectory-Id: traj-1, Content-Type: application/json and body {"messages":[{"role":"user","content":"hello"}]}. Observe HTTP 200 with an inference payload; real AWS answers HTTP 400 ValidationException whose message names the ARN ("Could not find job with jobArn ...").
3. Now POST /complete-rollout and /update-reward with the same JobArn/TrajectoryId. Observe success/conflict behavior from the turn that step 2 should never have appended; real AWS answers HTTP 404 ResourceNotFoundException naming the trajectory and account.
4. Repeat step 2 with X-Amzn-SageMaker-Job-Arn: not-a-valid-job-arn — correctly refused 400 ValidationException "Input arn not-a-valid-job-arn is malformed", showing only the job-resolution stage is missing, not ARN-pattern validation.
5. POST /sample-with-response-stream with the step-2 well-formed-but-nonexistent ARN. Observe a 200 and a stream body; real AWS refuses synchronously with 400 ValidationException and returns no stream object.
6. Re-replay to confirm: sagemakerjobruntime.sample-nonexistent-job-not-found, sagemakerjobruntime.sample-malformed-job-arn-rejected, sagemakerjobruntime.failed-sample-leaves-no-residual-trajectory-state, sagemakerjobruntime.sample-with-response-stream-nonexistent-job-not-found, sagemakerjobruntime.sample-with-response-stream-malformed-arn-raises-synchronously.
