Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.rds-secret-rotation-lambda-invocation-positive lambda__rds__secretsmanager.secretsmanager-rotation-lambda-wrong-secret-arn-denied lambda__rds__secretsmanager.rds-eventbridge-schedule-invokes-lambda-sdk-positive lambda__rds__secretsmanager.secretsmanager-rotation-eventbridge-schedule-invokes-lambda-positive
Issue-Key: lambda__rds__secretsmanager.rds-secret-rotation-lambda-invocation-positive|_poll_for_new_current_version#12|Timeout|time.sleep(interval_s)
Issue-Key: lambda__rds__secretsmanager.secretsmanager-rotation-lambda-wrong-secret-arn-denied|_poll_for_new_current_version#12|Timeout|time.sleep(interval_s)
Issue-Key: lambda__rds__secretsmanager.rds-eventbridge-schedule-invokes-lambda-sdk-positive|_poll_for_forwarded_messages#13|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Key: lambda__rds__secretsmanager.secretsmanager-rotation-eventbridge-schedule-invokes-lambda-positive|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 6074294cbfccbc2257fa0c432afede02752270ab

# lambda__rds__secretsmanager: 2 net-new root causes — a started secret rotation never runs the rotation Lambda (AWSCURRENT never advances), and an EventBridge rule with a ScheduleExpression never self-triggers its target

> _Note: this issue was identified against the code base at commit `6074294cbfccbc2257fa0c432afede02752270ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Two distinct root causes behind the four unowned lambda__rds__secretsmanager failures. Everything else in this group is marked OWNED (20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md, 20261029T195009Z-lambda-rds-secretsmanager-5-net-new-root-causes.md, 20261026T235825Z-..., 20261024T190718Z-..., 20261023T004328Z-...) and is skipped here. Both causes below sit DOWNSTREAM of the filed Lambda-runtime gap: a fixer should expect to need 20261022T213144Z (Invoke runs no deployment package) first, but neither of the two orchestrators described here exists either, so neither is covered by that issue — nor by 20261029T195009Z cause 5, which is about RotateSecret ignoring a MISSING invoke permission, not about the rotation never running.

Cause 1 of 2: a rotation Secrets Manager accepted never executes the rotation function, so no new secret value is ever created and AWSCURRENT never moves.
AWS: RotateSecret with RotateImmediately=true (the documented default) starts a rotation: Secrets Manager stakes an AWSPENDING version id and then invokes the rotation function four times, once per Step — createSecret, setSecret, testSecret, finishSecret (the documented single-user rotation contract) — passing {SecretId, ClientRequestToken, Step}. finishSecret moves AWSCURRENT to the new version. Observable afterwards: DescribeSecret reports a NEW version carrying AWSCURRENT (different from the version id before the call), AWSPENDING is no longer attached to a non-current version, and LastRotatedDate is set. A rotation function invoked for a secret it is not entitled to (a wrong SecretId) fails its own DescribeSecret/GetSecretValue with AccessDeniedException and the rotation does NOT promote a new version.
Emulator: the rotation is accepted (200 with ARN/Name/VersionId) and AWSPENDING is staked, but nothing is ever invoked; the handler says so itself — services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/rotate_secret.rs: "Starting a rotation is the one thing this operation does that the emulator cannot carry through: the rotation function is user code, and Lambda's `Invoke` in this emulator does not run a deployment package, so no version holding a new secret value is ever created and AWSCURRENT never moves." Both grounding checks therefore poll forever and die on the harness's 60 s cap.
Why: every rotation-dependent flow is silently stuck. A client sees a 200 from RotateSecret, a staked AWSPENDING and no progress: the secret value never changes, LastRotatedDate never appears, and — worse — the staked AWSPENDING makes every LATER RotateSecret refuse with "a previous rotation request is still in progress", so the secret is left permanently unrotatable. The negative half (a rotation function denied for the wrong secret ARN) is equally unobservable, so the permission boundary around rotation cannot be validated at all.
Fix: the RotateSecret handler's rotation orchestration (the four-Step invocation of RotationLambdaARN, and the AWSPENDING -> AWSCURRENT promotion/LastRotatedDate on finishSecret), on top of the Lambda execution path filed as 20261022T213144Z. A rotation that cannot be carried through must at least not leave AWSPENDING staked on a non-current version.
Traces: lambda__rds__secretsmanager.rds-secret-rotation-lambda-invocation-positive, lambda__rds__secretsmanager.secretsmanager-rotation-lambda-wrong-secret-arn-denied.

Cause 2 of 2: an EventBridge rule created with a ScheduleExpression is stored but never self-triggers, so its Lambda target is never invoked.
AWS: PutRule's own documentation (this service's model) states "A rule must contain at least an EventPattern or ScheduleExpression ... Rules with ScheduleExpressions self-trigger based on the given schedule." A rule `ScheduleExpression="rate(1 minute)"`, State=ENABLED, with a Lambda target registered by PutTargets (and lambda:InvokeFunction granted to events.amazonaws.com) invokes that function about once a minute with a `Scheduled Event` envelope; whatever the function does — here calling rds:DescribeDBEngineVersions over the SDK and forwarding the result to SQS, and there driving a Secrets Manager rotation — becomes observable within a couple of schedule ticks.
Emulator: PutRule stores the rule and nothing ever fires it. services/service-com-amazonaws-eventbridge-awsevents/src/ops/put_rule.rs describes itself purely as storage ("Creates or updates a rule on an event bus, storing it in crate::state"); there is no clock-driven dispatcher for ScheduleExpression rules, so no invocation and no downstream SQS signal ever appear and both checks hang to the 60 s cap.
Why: every scheduled workflow is a silent no-op — a client cannot distinguish a schedule that is firing from one that is not, and anything whose only trigger is a schedule (rotation schedules, periodic pollers, cron-driven jobs) never runs while the control plane keeps reporting a healthy ENABLED rule with targets.
Fix: an EventBridge scheduled-rule dispatcher (evaluate rate()/cron() ScheduleExpressions against the clock and deliver a `Scheduled Event` to each target through the same target dispatcher used for pattern-matched events, honouring State=DISABLED). The pattern-matched half of the dispatcher is filed as 20261025T115007Z-events-lambda-rds-sqs-an-eventbridge-rule-on.md (aws.rds CloudTrail-sourced events never reaching a rule); that issue carries neither of these checks nor the schedule path.
Traces: lambda__rds__secretsmanager.rds-eventbridge-schedule-invokes-lambda-sdk-positive, lambda__rds__secretsmanager.secretsmanager-rotation-eventbridge-schedule-invokes-lambda-positive.

## Evidence
Suite simparity-behavioral-aws-v1, service group lambda__rds__secretsmanager 1/67 passed. All four checks below are unmarked in the replay report (neither OWNED nor ALREADY FILED) and all four fail identically with `Error: Check timed out after 60.0s`.

Cause 1:
- lambda__rds__secretsmanager.rds-secret-rotation-lambda-invocation-positive [mismatch] — creates an RDS-shaped secret and a standard single-user rotation function (createSecret/setSecret/testSecret/finishSecret), grants secretsmanager.amazonaws.com invoke permission, calls RotateSecret, then runs `_poll_for_new_current_version(..., timeout_s=120, interval_s=3)`, which raises "rotation never promoted a new AWSCURRENT version within timeout" when DescribeSecret's VersionIdsToStages never shows AWSCURRENT on a version other than the original. Expected: a new AWSCURRENT version within seconds. Actual: no step is ever invoked; the poll never advances.
- lambda__rds__secretsmanager.secretsmanager-rotation-lambda-wrong-secret-arn-denied [mismatch] — same wiring, asserting that the rotation function invoked against a secret ARN it is not entitled to is denied and no new version is promoted. Unobservable for the same reason.
Source confirmation: rotate_secret.rs lines ~334-354 — "no version holding a new secret value is ever created and AWSCURRENT never moves"; `if rotate_immediately { secret.start_rotation(version_id.clone(), now); }` stakes AWSPENDING and returns.

Cause 2:
- lambda__rds__secretsmanager.rds-eventbridge-schedule-invokes-lambda-sdk-positive [mismatch] — `events.put_rule(Name=..., ScheduleExpression="rate(1 minute)", State="ENABLED")` + `put_targets(Rule=..., Targets=[{"Id": "notify-lambda", "Arn": <function arn>}])`, then polls the SQS queue the function forwards rds:DescribeDBEngineVersions results to. Expected: a message within a couple of ticks. Actual: the queue stays empty; nothing fires.
- lambda__rds__secretsmanager.secretsmanager-rotation-eventbridge-schedule-invokes-lambda-positive [mismatch] — same scheduled-rule wiring onto a Secrets Manager rotation function. Same silence.

What the timeout does and does not establish: the harness caps each check at 60 s while the checks' own delivery budgets are 120 s+, so the cap does not localise the stall to one call. It does establish that no rotation progress and no scheduled invocation is observable inside the cap, against sub-minute latency on real AWS; the handler/module sources quoted above show why in both cases. Note the shared prerequisite: with Lambda's Invoke answering NotImplemented (20261022T213144Z) no function body can run, so these two orchestrators cannot be confirmed fixed until that one is too — re-replay the four check ids together.

## Steps to Reproduce
Build and boot the emulator, then replay the checks in trace_ids.

Cause 1, manually:
1. Create a secret holding {"username":..., "password":...}; note DescribeSecret's current AWSCURRENT version id V0.
2. CreateFunction a python3.12 single-user rotation handler (createSecret/setSecret/testSecret/finishSecret), wait for State=Active, and `lambda.add_permission(Principal="secretsmanager.amazonaws.com", Action="lambda:InvokeFunction")`.
3. `secretsmanager.rotate_secret(SecretId=..., RotationLambdaARN=..., RotationRules={"AutomaticallyAfterDays": 30}, RotateImmediately=True)`.
4. Poll `describe_secret(SecretId=...)` -> expect, within seconds, a version V1 != V0 carrying AWSCURRENT, AWSPENDING no longer on a non-current version, and LastRotatedDate set. The emulator leaves AWSCURRENT on V0 forever with AWSPENDING staked on an empty pending version.
5. Call rotate_secret again -> AWS starts another rotation; the emulator refuses with "A previous rotation request is still in progress", i.e. the secret is now permanently unrotatable.
6. Point a rotation function at a secret ARN its role does not allow -> expect the rotation to fail with AccessDeniedException inside the function and NO new AWSCURRENT version.

Cause 2, manually:
1. CreateFunction a function that writes a marker to an SQS queue; wait for Active; grant lambda:InvokeFunction to events.amazonaws.com.
2. `events.put_rule(Name=R, ScheduleExpression="rate(1 minute)", State="ENABLED")`; `events.put_targets(Rule=R, Targets=[{"Id": "t1", "Arn": <function arn>}])`.
3. Poll the queue for ~2-3 minutes -> expect at least one invocation per schedule tick. The emulator delivers nothing, while DescribeRule/ListTargetsByRule keep reporting an ENABLED rule with the target.
4. Set the rule State=DISABLED -> deliveries must stop (the guard the dispatcher needs).
