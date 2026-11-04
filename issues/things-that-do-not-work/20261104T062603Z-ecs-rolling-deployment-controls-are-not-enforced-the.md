Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.minimum-healthy-percent-floor-holds-during-rolling-update ecs.deep-ecs-3-deploy-fail-maximum-percent-ceiling-never-exceeded-during-rollout ecs.service-deployment-circuit-breaker-rollback-ordering ecs.deployment-circuit-breaker-rolls-back-to-last-good
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.minimum-healthy-percent-floor-holds-during-rolling-update|run#26|AssertionError|assert subject_min >= floor, (
Issue-Key: ecs.deep-ecs-3-deploy-fail-maximum-percent-ceiling-never-exceeded-during-rollout|run#26|AssertionError|assert _peak(control, "capped") > _DESIRED, (
Issue-Key: ecs.service-deployment-circuit-breaker-rollback-ordering|run#14|AssertionError|assert final_dep["status"] in ("ROLLBACK_SUCCESSFUL", "ROLLBACK_FAILED"), (
Issue-Key: ecs.deployment-circuit-breaker-rolls-back-to-last-good|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ECS rolling-deployment controls are not enforced: the minimumHealthyPercent floor is breached, the maximumPercent surge never happens, and the circuit breaker never rolls back (3 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
Cause 1 of 3: minimumHealthyPercent is not a floor.
AWS: with minimumHealthyPercent=100 on desiredCount=2, a rolling update must keep at least 2 tasks RUNNING for the whole deployment — runningCount never drops below the floor.
Emulator: runningCount falls to 0 during the update (sampled [0, 0, 2]) and then recovers, i.e. the scheduler stops the old tasks before the replacements are healthy.
Why: a client that sets a 100% floor to get zero-downtime deployments observes an outage the real service would not produce.
Fix: the service scheduler's rolling-update ordering (respect the floor before stopping old tasks).
Trace: ecs.minimum-healthy-percent-floor-holds-during-rolling-update.

Cause 2 of 3: maximumPercent does not allow (or drive) the surge.
AWS: with maximumPercent=200 and minimumHealthyPercent=100 on desiredCount=2, the deployment starts replacements before stopping old tasks, so more than 2 PENDING/RUNNING tasks exist at some point.
Emulator: the peak over the sampled window is 2 — no surge ever occurs, so the ceiling is never exercised.
Fix: the same rolling-update ordering/surge computation.
Trace: ecs.deep-ecs-3-deploy-fail-maximum-percent-ceiling-never-exceeded-during-rollout.

Cause 3 of 3: the deployment circuit breaker never fires.
AWS: with deploymentCircuitBreaker {enable:true, rollback:true}, a deployment whose tasks keep failing is marked failed and rolled back to the last good revision; the service deployment's status history shows the rollback.
Emulator: the deployment is reported as IN_PROGRESS then SUCCESSFUL with no rollback attempt, and the companion check that waits for the roll-back to the last good revision times out at 60s.
Fix: the service-deployment circuit-breaker evaluation and rollback path.
Traces: ecs.service-deployment-circuit-breaker-rollback-ordering, ecs.deployment-circuit-breaker-rolls-back-to-last-good.

## Evidence
1. `AssertionError: minimumHealthyPercent=100 on desiredCount=2 means at least 2 tasks must stay RUNNING throughout the deployment, but runningCount fell to 0 (samples=[0, 0, 2]) while the identically-updated control dipped to 0 as configured`.
2. `AssertionError: the control (maximumPercent=200, minimumHealthyPercent=100) must start replacements before stopping old tasks, so more than 2 PENDING/RUNNING tasks must be sampled at some point; peak was 2 over 3 samples -- without this the subject's ceiling below is not evidence`.
3. `AssertionError: expected the circuit breaker to trigger a rollback attempt, ended in SUCCESSFUL (history=['IN_PROGRESS', 'SUCCESSFUL'])`; and `Error: Check timed out after 60.0s` for the roll-back-to-last-good check. The sampled counts are per-run; breaching the floor at all, never surging at all, and never rolling back at all are the defects.

## Steps to Reproduce
1. CreateService desiredCount=2, deploymentConfiguration {minimumHealthyPercent:100, maximumPercent:200}; UpdateService to a new task-definition revision and sample DescribeServices.runningCount throughout -> it reaches 0.
2. Same setup, sample pendingCount+runningCount -> never exceeds 2.
3. CreateService with deploymentCircuitBreaker {enable:true, rollback:true}; UpdateService to a revision whose tasks cannot start; poll DescribeServiceDeployments -> status goes SUCCESSFUL, never ROLLBACK_*.
