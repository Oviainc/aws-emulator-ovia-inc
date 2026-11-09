Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: synthetics.canary-run-completes-publishes-artifacts-and-metrics synthetics.start-canary-transitions-running-then-stopped
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics.canary-run-completes-publishes-artifacts-and-metrics|run#13|AssertionError|assert run_state == "PASSED", f"expected the heartbeat canary run to PASS, got {run_state!r}"
Issue-Key: synthetics.start-canary-transitions-running-then-stopped|_wait_for_canary_state#15|Timeout|return status["State"]
Issue-Commit: 41c5ae389f495755d1c1e47282b4b58fd25764aa

# synthetics: 2 net-new root causes — a successful canary run is recorded FAILED instead of PASSED, and a run-once rate(0 minute) canary never settles out of RUNNING

> _Note: this issue was identified against the code base at commit `41c5ae389f495755d1c1e47282b4b58fd25764aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCanary

## Description
Two root causes in the emulated CloudWatch Synthetics run lifecycle, grounded in the two failing synthetics checks of this replay that were neither marked ALREADY FILED nor marked OWNED by an open issue.

=== Cause 1 of 2: every canary run settles to FAILED; a run that succeeds is never recorded PASSED ===
AWS: Smithy com.amazonaws.synthetics#CanaryRunState is the enum RUNNING | PASSED | FAILED, carried on CanaryRunStatus{State, StateReason, StateReasonCode, TestResult}. StateReason is documented as "If run of the canary failed, this field contains the reason for the error" and StateReasonCode is CANARY_FAILURE | EXECUTION_FAILURE — i.e. both are failure-only fields. A canary whose script completes successfully, whose execution role grants logs:*, s3:PutObject and cloudwatch:PutMetricData for the CloudWatchSynthetics namespace, and whose artifact bucket is writable, is recorded by GetCanaryRuns (POST /canary/{Name}/runs → 200 {CanaryRuns, NextToken}) with `Status.State = "PASSED"` and no StateReason/StateReasonCode; its artifacts land under the configured S3 location and a SuccessPercent=100 datapoint is published in the CloudWatchSynthetics namespace with dimension CanaryName.
Emulator: after CreateCanary(+READY) and StartCanary, the single recorded run settles to `Status.State = "FAILED"` — the run verdict is forced regardless of script, role or bucket, so PASSED is unreachable. The check polled GetCanaryRuns for up to 360 s and read FAILED. Because the run never passes, the two downstream AWS behaviours the check then verifies (an artifact object under `canary/<name>/` in the configured bucket, and a CloudWatchSynthetics SuccessPercent Average=100 datapoint for dimension CanaryName=<name>) are not reached by this replay and are not asserted here.
NOTE: the open issue 20261029T184613Z-synthetics-4-net-new-root-causes-duplicate-associateresource.md describes a forced FAILED/EXECUTION_FAILURE run verdict as its "Cause 3 of 4", but it is grounded in a different check (synthetics.canary-run-fails-without-execution-role-permissions) and this replay does not report the check below as owned by it. It is filed here so the divergence is not lost; a fixer should reconcile the two rather than treat them as independent work.
Why: a client cannot observe a successful canary at all. Health dashboards, waiters and alarms keyed on `Status.State == "PASSED"` / SuccessPercent see a permanently failing canary, and the failure-only fields (StateReason, StateReasonCode) are populated for runs that did not fail, so error handling triggers on healthy runs.
Fix: the run-outcome computation in the synthetics run lifecycle — the StartCanary handler (services/service-com-amazonaws-synthetics-synthetics/src/ops/start_canary.rs) and the run settle logic in services/service-com-amazonaws-synthetics-synthetics/src/state.rs must settle a run that has no failure condition to PASSED with StateReason/StateReasonCode absent, reserving FAILED + CANARY_FAILURE/EXECUTION_FAILURE for an actual script or artifact failure.
Traces: synthetics.canary-run-completes-publishes-artifacts-and-metrics.

=== Cause 2 of 2: a started run-once (rate(0 minute)) canary stays RUNNING forever, so the one-shot lifecycle never completes ===
AWS: CanaryState is CREATING | READY | STARTING | RUNNING | UPDATING | STOPPING | STOPPED | ERROR | DELETING, and the model documents CanaryScheduleOutput$Expression: "Specifying rate(0 minute) or rate(0 hour) is a special value that causes the canary to run only once when it is started." So StartCanary (POST /canary/{Name}/start → 200; modeled errors ConflictException 409, ResourceNotFoundException 404, ValidationException 400, InternalServerException 500) drives READY → STARTING/RUNNING and, for a run-once schedule, the canary settles back to STOPPED by itself once that single run finishes — no StopCanary is involved — after which GetCanaryRuns returns at least one run whose Timeline has Started and Completed.
Emulator: GetCanary keeps answering `Canary.Status.State = "RUNNING"` indefinitely after StartCanary on a rate(0 minute) canary; the poll for {STOPPED, ERROR} never terminates and the harness killed the check with a bare `Check timed out after 180.0s` (no frame). No status or error code is involved — it is a missing state transition.
NOTE: the open issue 20261102T102252Z-synthetics-a-run-once-rate-0-minute-canary.md describes this same transition gap and lists this check in its Issue-Trace-Ids, but this replay does not mark the check as OWNED (its failure signature is now a frameless timeout, so the recorded replay key no longer matches). It is re-grounded here so the still-failing check is not lost; a fixer should close it together with that issue rather than twice.
Why: the on-demand canary lifecycle — run once and wait for it to finish — can never complete. A client blocks until its own timeout instead of observing STOPPED, and nothing distinguishes a canary still working from one whose single run is done.
Fix: the canary lifecycle clock in src/state.rs (`StoredCanary::settle`, beside the existing ready_due/outcome_due handling) together with the StartCanary handler: a started canary on a run-once rate(0 minute)/rate(0 hour) schedule must record a due instant moving RUNNING → STOPPED once its single run completed, while a recurring schedule keeps holding RUNNING until StopCanary.
Traces: synthetics.start-canary-transitions-running-then-stopped.

## Evidence
Suite simparity-behavioral-aws-v1: 15/27 passed. Per-service: synthetics 15/24 (62%), 9 failed; 6 of those are marked OWNED by 20261029T184613Z-synthetics-4-net-new-root-causes-duplicate-associateresource.md / 20261030T021959Z-synthetics-a-dry-run-has-no-lifecycle-of.md and 1 is marked ALREADY FILED (UpdateCanary stub) — the two below are the remainder.

Cause 1 — [mismatch] synthetics.canary-run-completes-publishes-artifacts-and-metrics:
  `assert run_state == "PASSED", f"expected the heartbeat canary run to PASS, got {run_state!r}"` →
  `AssertionError: expected the heartbeat canary run to PASS, got 'FAILED'`.
  The check creates an artifact bucket, an execution role granting logs:CreateLogGroup/CreateLogStream/PutLogEvents, s3:PutObject/GetBucketLocation on that bucket, s3:ListAllMyBuckets and cloudwatch:PutMetricData limited to namespace CloudWatchSynthetics, picks the latest syn-nodejs-puppeteer runtime, creates a canary with a heartbeat Puppeteer script (handler pageLoadBlueprint.handler, Schedule rate(0 minute)), StartCanary, then polls GetCanaryRuns for up to 360 s until State is PASSED or FAILED. Expected `CanaryRuns[0].Status.State == "PASSED"` with no StateReason/StateReasonCode; got FAILED.
  Not reached because of this failure (so not asserted here): at least one object under `s3://<bucket>/canary/<name>/`, and a CloudWatchSynthetics SuccessPercent datapoint with Average == 100.0 for Dimensions=[{Name: CanaryName, Value: <name>}].

Cause 2 — [mismatch] synthetics.start-canary-transitions-running-then-stopped:
  `Error: Check timed out after 180.0s`.
  The check creates a bucket + role, CreateCanary(Schedule={"Expression": "rate(0 minute)"}), waits for READY, StartCanary (200), then `_wait_for_canary_state(ctx, name, {"STOPPED", "ERROR"}, timeout_s=300, interval_s=2)` — GetCanary keeps reporting RUNNING until the harness kills the check. Expected: the canary settles at STOPPED by itself, then GetCanaryRuns returns a run with Status.State PASSED and a Timeline where Completed >= Started.

Model citations: com.amazonaws.synthetics#CanaryRunState = RUNNING|PASSED|FAILED; CanaryRunStatus{State, StateReason ("If run of the canary failed, this field contains the reason for the error"), StateReasonCode = CANARY_FAILURE|EXECUTION_FAILURE, TestResult}; GetCanaryRuns POST /canary/{Name}/runs → 200 {CanaryRuns, NextToken}; StartCanary POST /canary/{Name}/start → 200, errors ConflictException 409 / ResourceNotFoundException 404 / ValidationException 400 / InternalServerException 500; CanaryScheduleOutput$Expression doc on rate(0 minute)/rate(0 hour) running the canary only once.

## Steps to Reproduce
Shared setup: create an S3 bucket for artifacts and an IAM execution role granting logs:CreateLogGroup/CreateLogStream/PutLogEvents, s3:PutObject + s3:GetBucketLocation on that bucket, s3:ListAllMyBuckets, and cloudwatch:PutMetricData for namespace CloudWatchSynthetics; pick the latest non-deprecated syn-nodejs-puppeteer runtime via DescribeRuntimeVersions.

Cause 1:
1. synthetics.CreateCanary(Name=<name>, Code={ZipFile: <zip with a working handler>, Handler:"pageLoadBlueprint.handler"}, ArtifactS3Location="s3://<bucket>", ExecutionRoleArn=<role arn>, RuntimeVersion=<latest>, Schedule={"Expression":"rate(0 minute)"}).
2. Poll GetCanary until Status.State == READY.
3. StartCanary(Name=<name>) → 200.
4. Poll GetCanaryRuns(Name=<name>): CanaryRuns[0].Status.State becomes "FAILED". Expected "PASSED", with StateReason/StateReasonCode absent (they are failure-only fields).

Cause 2:
1. Same CreateCanary with Schedule={"Expression":"rate(0 minute)"}; wait for READY.
2. StartCanary(Name=<name>) → 200.
3. Poll GetCanary(Name=<name>).Canary.Status.State: it reports RUNNING and stays RUNNING indefinitely (observed past 180 s). Expected: the one-shot run finishes and the canary settles to STOPPED with no StopCanary call, after which GetCanaryRuns has at least one run with a Timeline whose Completed >= Started.
4. Contrast: a recurring schedule (e.g. rate(1 minute)) correctly holds RUNNING until StopCanary — only the run-once case never settles.
