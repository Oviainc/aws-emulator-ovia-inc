Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.linear-traffic-shift-walks-the-weight-ladder-over-time codedeploy.lambda-canary-diverts-live-invokes codedeploy.alarm-rollback-returns-live-traffic-to-prior-version codedeploy.after-allow-traffic-hook-failure-triggers-deployment-failure-rollback codedeploy.after-allow-traffic-hook-failure-without-auto-rollback-leaves-alias-at-target codedeploy.alarm-breach-auto-rolls-back-deployment-and-restores-alias codedeploy.minimum-healthy-hosts-bounds-observed-concurrent-unavailability
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.linear-traffic-shift-walks-the-weight-ladder-over-time|run#26|AssertionError|assert elapsed >= MIN_ELAPSED_S, (
Issue-Key: codedeploy.lambda-canary-diverts-live-invokes|run#30|AssertionError|assert len(window) >= N_MIN, (
Issue-Key: codedeploy.alarm-rollback-returns-live-traffic-to-prior-version|run#54|AssertionError|assert k3 / n_pb < CANARY_SHARE_MAX, (
Issue-Key: codedeploy.after-allow-traffic-hook-failure-triggers-deployment-failure-rollback|_hook_event_status#15|AssertionError|assert target_ids, f"deployment {dep_id!r} exposed no deployment targets"
Issue-Key: codedeploy.after-allow-traffic-hook-failure-without-auto-rollback-leaves-alias-at-target|run#15|AssertionError|assert bad_failed.get("status") == "Failed", (
Issue-Key: codedeploy.alarm-breach-auto-rolls-back-deployment-and-restores-alias|run#30|AssertionError|assert err.get("message") == (
Issue-Key: codedeploy.minimum-healthy-hosts-bounds-observed-concurrent-unavailability|_wait_fleet_ready#12|AssertionError|assert not pending, (
Issue-Commit: 74ebb92ce9e8b3d0346604c0e135c3e0968fb0a4

# CodeDeploy traffic-shifting is not clock-paced or weight-routed, Lambda deployment targets are absent, a Failed hook still Succeeds, and the ALARM_ACTIVE message is mis-worded (6 causes)

> _Note: this issue was identified against the code base at commit `74ebb92ce9e8b3d0346604c0e135c3e0968fb0a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Six net-new root causes in the CodeDeploy emulator, all grounded in failing behavioral checks of the `codedeploy` suite section. They are independent fixes; close all six before submitting.

Cause 1 of 6: a time-based traffic shift is not paced by the wall clock.
AWS: Smithy (com.amazonaws.codedeploy) TimeBasedCanary.canaryInterval (WaitTimeInMins) is documented as "The number of minutes between the first and second traffic shifts of a TimeBasedCanary deployment", and TimeBasedLinear.linearInterval as "The number of minutes between each incremental traffic shift". So a TimeBasedCanary10Percent/1Minute deployment keeps the alias at the canary weight for about one minute before completing the shift, and a linear config with linearPercentage=25 / linearInterval=1 needs roughly three minutes of wall time to walk 25/50/75/100; the deployment cannot reach Succeeded before those minutes have elapsed.
Emulator: the intervals are consumed immediately. Three 1-minute linear intervals completed in 8.1s (the check's floor is 100s), and in the canary check the window during which the alias carried the canary weight spanned only 114.3s..116.3s — 2.0s wide — so a window trimmed 5s on each side holds 0 of the 748 observed invokes. The progression appears to advance per state read rather than against a deadline.
Why: a client cannot observe or test a canary/linear rollout at all — there is no window in which both versions are live, no time to run a canary validation or trip an alarm, and any client logic that polls during the shift (or that asserts a deployment cannot finish before its configured minutes) sees a deployment that was never incremental.
Fix: the deployment progression driver for Lambda/ECS traffic-shifting deployments — each increment's transition must fire on a wall-clock deadline computed from canaryInterval/linearInterval, not on poll or read count.
Traces: codedeploy.linear-traffic-shift-walks-the-weight-ladder-over-time, codedeploy.lambda-canary-diverts-live-invokes.

Cause 2 of 6: during the canary increment, live invokes are not split according to the weight CodeDeploy wrote.
AWS: for a Lambda TimeBasedCanary deployment CodeDeploy puts the alias into a weighted state (alias RoutingConfig AdditionalVersionWeights = canaryPercentage/100 on the target version), and the invoke path honours it, so roughly canaryPercentage of invokes execute the target version and the rest the original version until the shift completes.
Emulator: every invoke in the canary increment reaches the target version — "301 of 301 invokes before the breach executed '3', a share of 1.000; a 10% canary must stay below 0.35". Either the weighted routing config is not written, or the invoke dispatcher ignores it and resolves the alias straight to the target version.
Why: the canary provides no protection: 100% of production traffic is cut over at the first increment, so a client validating a canary (or measuring per-version error rates before promoting) gets the opposite of AWS's behavior, and a rollback has nothing partial to undo.
Fix: the Lambda-platform traffic-shift step of the deployment driver (writing the weighted alias state) together with the alias-resolution/weighted-routing path that invokes serve from. Related but distinct from open issue 20261107T224322Z-codedeploy-lambda-deployments-reach-succeeded-without-shifting-the.md (there the alias never moves; here it moves fully and immediately) — that issue does not carry this check's replay key.
Traces: codedeploy.alarm-rollback-returns-live-traffic-to-prior-version.

Cause 3 of 6: a started Lambda deployment exposes no deployment targets.
AWS: Smithy ListDeploymentTargets(deploymentId) returns targetIds — for a Lambda deployment exactly one lambdaTarget id — and GetDeploymentTarget(deploymentId,targetId) returns deploymentTarget.lambdaTarget with lifecycleEvents, each a LifecycleEvent{lifecycleEventName,status} whose status is LifecycleEventStatus (Pending|InProgress|Succeeded|Failed|Skipped|Unknown), covering BeforeAllowTraffic / AllowTraffic / AfterAllowTraffic. A deployment that has not started yet is refused with DeploymentNotStartedException (400), never an empty list.
Emulator: ListDeploymentTargets returns 200 with an empty targetIds for a Lambda deployment that is already running/terminal (check reports "deployment 'd-…' exposed no deployment targets" after paginating every page).
Why: the per-target view is the only place a client can read lifecycle-event status and hook outcomes; an empty list makes every target-level observation impossible while looking like a successful read, so client poll loops silently conclude there is nothing to inspect.
Fix: the deployment state must materialise a lambdaTarget per Lambda deployment and the ListDeploymentTargets/GetDeploymentTarget handlers must serve it (including its lifecycleEvents list). Same symptom as checks owned by open issue 20261024T114701Z-codedeploy-deployments-report-success-without-executing-the-lambda.md, which does not carry this check's replay key.
Traces: codedeploy.after-allow-traffic-hook-failure-triggers-deployment-failure-rollback.

Cause 4 of 6: a hook that reports Failed does not fail the deployment.
AWS: when the AppSpec hook function calls PutLifecycleEventHookExecutionStatus with status=Failed for the AfterAllowTraffic event, the deployment terminates with status Failed and errorInformation.code HOOK_EXECUTION_FAILURE (ErrorCode enum includes HOOK_EXECUTION_FAILURE); with the deployment group's auto-rollback disabled no rollback deployment is created and the alias is left at the target version.
Emulator: the deployment on the no-rollback group whose AfterAllowTraffic hook reported 'Failed' ended 'Succeeded' with errorInformation absent (None) — the reported hook status is accepted and then ignored.
Why: a client's validation hook cannot veto a bad release; a deployment that AWS reports as Failed/HOOK_EXECUTION_FAILURE is reported as Succeeded, so CI/CD gates built on the deployment's terminal status promote a release the hook rejected.
Fix: the lifecycle-event/hook execution state must gate the deployment's terminal status — PutLifecycleEventHookExecutionStatus(status=Failed) must drive the deployment to Failed and populate errorInformation.code=HOOK_EXECUTION_FAILURE in the deployment progression driver.
Traces: codedeploy.after-allow-traffic-hook-failure-without-auto-rollback-leaves-alias-at-target.

Cause 5 of 6: the ALARM_ACTIVE errorInformation.message is worded differently from AWS's.
AWS: recorded behavior only (the Smithy model types errorInformation.message as a free ErrorMessage). A deployment stopped by a CloudWatch alarm breach reads status Stopped with errorInformation.code ALARM_ACTIVE and a message whose FIXED text is "One or more alarms have been activated according to the Amazon CloudWatch metrics you selected, and the affected deployments have been stopped. Activated alarms: " followed by the activated alarm's name wrapped in angle brackets — i.e. "<" + alarmName + ">". The only variable part is the alarm name, taken from the deployment group's alarmConfiguration/state; the angle brackets are fixed text.
Emulator: it emits the alarm name bare, with no angle brackets: 'One or more alarms have been activated according to the Amazon CloudWatch metrics you selected, and the affected deployments have been stopped. Activated alarms: sim2real-behav-cd-alarm-9f2130648a7a' (that name is this run's value, not part of the requirement). Status Stopped and code ALARM_ACTIVE are correct.
Why: a client that parses or asserts on the stop reason (log scraping, incident automation, test fixtures) does not match AWS's text, and the delimiting that makes multi-alarm lists parseable is missing.
Fix: the message construction in the alarm-stop path of the deployment driver (the code that writes errorInformation for ALARM_ACTIVE).
Traces: codedeploy.alarm-breach-auto-rolls-back-deployment-and-restores-alias.

Cause 6 of 6: the EC2 hosts a Server-platform minimumHealthyHosts check needs never become Online SSM managed nodes.
AWS: an EC2 instance launched with an SSM-capable instance profile registers with Systems Manager, so ssm:DescribeInstanceInformation returns an entry with PingStatus 'Online', after which the host can be a CodeDeploy instance target and the deployment's concurrent-unavailability bound (minimumHealthyHosts) is observable.
Emulator: the launched instances never appear in SSM — "['i-00000000000000001', 'i-00000000000000002'] never came online in SSM within 600s (last ping statuses {})" — so the deployment's effect cannot be read even if it ran. Instance ids here are per-run values.
Why: the whole Server compute platform (minimumHealthyHosts enforcement included) is unreachable, and the blocking symptom is a silent empty inventory rather than an error, so client poll loops hang.
Fix: the EC2 instance lifecycle must cross-register a running instance into the SSM managed-node state, and the Server-platform instance-target driver must then drive it. This is the same symptom as open issue 20261107T094623Z-codedeploy-no-server-platform-ec2-agent-deployment-check.md, which lists eight other checks and does not carry this check's replay key.
Traces: codedeploy.minimum-healthy-hosts-bounds-observed-concurrent-unavailability.

## Evidence
Cause 1 — Expected: a linear 25%/1-minute shift needs ~3 minutes of wall time; a 10%/1-minute canary holds the canary weight for ~60s. Actual: "AssertionError: three 1-minute linear intervals completed in 8.1s, under the 100s floor; the shift is not paced by the clock" (codedeploy.linear-traffic-shift-walks-the-weight-ladder-over-time) and "AssertionError: the canary window holds only 0 observations, below the 200 the assertions need; the requester produced 748 in total, the canary window spanned 114.3s..116.3s and each window is trimmed 5.0s on both sides" (codedeploy.lambda-canary-diverts-live-invokes).

Cause 2 — Expected: during a 10% canary, well under 35% of invokes execute the target version. Actual: "AssertionError: 301 of 301 invokes before the breach executed '3', a share of 1.000; a 10% canary must stay below 0.35" (codedeploy.alarm-rollback-returns-live-traffic-to-prior-version).

Cause 3 — Expected: ListDeploymentTargets(deploymentId) returns one lambdaTarget id for a started Lambda deployment (or DeploymentNotStartedException, 400, before it starts). Actual: empty targetIds across all pages — "AssertionError: deployment 'd-3YT23UF4A' exposed no deployment targets" (codedeploy.after-allow-traffic-hook-failure-triggers-deployment-failure-rollback).

Cause 4 — Expected: deploymentInfo.status 'Failed' with errorInformation.code 'HOOK_EXECUTION_FAILURE' after the AfterAllowTraffic hook reports status=Failed. Actual: "AssertionError: the deployment on the no-rollback group whose AfterAllowTraffic hook reports 'Failed' ended 'Succeeded', expected 'Failed' (errorInformation=None)" (codedeploy.after-allow-traffic-hook-failure-without-auto-rollback-leaves-alias-at-target).

Cause 5 — Expected errorInformation.message: "One or more alarms have been activated according to the Amazon CloudWatch metrics you selected, and the affected deployments have been stopped. Activated alarms: <ALARM_NAME>" (fixed text plus the alarm name in angle brackets). Actual: "...Activated alarms: sim2real-behav-cd-alarm-9f2130648a7a" — angle brackets missing (codedeploy.alarm-breach-auto-rolls-back-deployment-and-restores-alias). Status Stopped and code ALARM_ACTIVE matched.

Cause 6 — Expected: launched instances report PingStatus 'Online' in ssm:DescribeInstanceInformation within minutes. Actual: "AssertionError: ['i-00000000000000001', 'i-00000000000000002'] never came online in SSM within 600s (last ping statuses {}); the hosts are provisioned but unreachable" (codedeploy.minimum-healthy-hosts-bounds-observed-concurrent-unavailability).

## Steps to Reproduce
Cause 1: create a Lambda application + deployment group with deploymentConfigName CodeDeployDefault.LambdaLinear25PercentEvery1Minute (and separately a canary 10%/1-minute config), CreateDeployment with an AppSpec moving the alias from CurrentVersion to TargetVersion, then poll GetDeployment and the alias routing config — observe the weight ladder walked and Succeeded reached in seconds instead of minutes.
Cause 2: during that canary deployment's first increment, invoke the alias continuously (while sampling GetAlias to bound the window) and tally which version executed — observe 100% of invokes on the target version instead of ~10%.
Cause 3: for any started Lambda deployment call ListDeploymentTargets(deploymentId=<id>) and page to the end — observe an empty targetIds, so GetDeploymentTarget cannot be reached.
Cause 4: deploy to a group with autoRollbackConfiguration disabled, with an AppSpec Hooks entry for AfterAllowTraffic whose function calls PutLifecycleEventHookExecutionStatus(status='Failed'); poll GetDeployment to terminal — observe 'Succeeded' with no errorInformation instead of 'Failed'/HOOK_EXECUTION_FAILURE.
Cause 5: give the deployment group an alarmConfiguration with one CloudWatch alarm, start a canary deployment, force the alarm to ALARM mid-shift, then read GetDeployment(deploymentId).deploymentInfo.errorInformation — compare message against AWS's wording (alarm name in angle brackets).
Cause 6: launch two t3.micro instances with an SSM-capable instance profile, tag them as the deployment group's EC2 targets, and poll ssm:DescribeInstanceInformation — observe no entries and hence no PingStatus at all.
