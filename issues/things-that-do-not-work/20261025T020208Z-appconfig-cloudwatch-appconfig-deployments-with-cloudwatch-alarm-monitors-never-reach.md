Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__cloudwatch.alarm-monitor-role-not-assumable-deployment-rejected appconfig__cloudwatch.cloudwatch-alarm-rollback-restores-previous-version
Issue-Key: appconfig__cloudwatch.alarm-monitor-role-not-assumable-deployment-rejected|_wait_terminal#31|Timeout|time.sleep(4.0)
Issue-Key: appconfig__cloudwatch.cloudwatch-alarm-rollback-restores-previous-version|_wait_terminal#37|Timeout|time.sleep(4.0)
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# AppConfig deployments with CloudWatch alarm Monitors never reach a terminal state, so neither the monitor-role refusal nor the alarm-triggered rollback can be observed (1 cause)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
One net-new root cause.

Cause 1 of 1: an AppConfig deployment configured with CloudWatch alarm Monitors never progresses to a terminal state.
AWS: StartDeployment with Monitors=[{AlarmArn, AlarmRoleArn}] runs the deployment through its bake window and settles in one of the terminal states COMPLETE / ROLLED_BACK / REVERTED, which ListDeployments and GetDeployment report, and the environment leaves its transitional Deploying/RollingBack state. If the named AlarmRoleArn's trust policy does not permit appconfig.amazonaws.com to assume it, the deployment is refused up front; if the monitored CloudWatch alarm goes into ALARM during the bake window, AppConfig rolls the deployment back and the environment serves the previous configuration version again. Recorded behavior: both checks observe a terminal outcome.
Emulator: neither check completes. Both poll for the deployment/environment to leave its transitional state and are killed at the harness 60 s cap, so no terminal state, no up-front monitor-role refusal and no alarm-driven rollback is ever observable. (The alarm leg additionally depends on the CloudWatch alarm evaluation engine, filed separately for service `cloudwatch`.)
Why: a client that starts a monitored deployment and waits for it to settle waits forever; every safe-deployment flow built on AppConfig monitors — bake, auto-rollback, revert — silently never resolves, and teardown of the application is blocked because the environment never leaves Deploying.
Fix: the deployment state machine in the AppConfig StartDeployment handler and its deployment/environment state — the bake-window progression to a terminal state, the AlarmRoleArn assumability precondition, and the alarm-triggered rollback path.
Traces: appconfig__cloudwatch.alarm-monitor-role-not-assumable-deployment-rejected, appconfig__cloudwatch.cloudwatch-alarm-rollback-restores-previous-version.

## Evidence
Both checks: "Error: Check timed out after 60.0s". The checks poll with declared budgets _MONITOR_READY_TIMEOUT_S=900 / _TERMINAL_TIMEOUT_S=480 / _REFUSAL_TIMEOUT_S=300 for `_TERMINAL = ("COMPLETE", "ROLLED_BACK", "REVERTED")` and for the environment to leave `_ENV_TRANSITIONAL = ("Deploying", "RollingBack")`; neither condition is ever met, and cleanup itself would raise `AssertionError(f"environment {env_id} never left {state!r} during cleanup")`.

## Steps to Reproduce
1. CreateApplication / CreateEnvironment / CreateConfigurationProfile + CreateHostedConfigurationVersion.
2. CreateRole whose trust policy allows appconfig.amazonaws.com (and a twin that does not), with cloudwatch:DescribeAlarms inline.
3. PutMetricAlarm for the monitored metric.
4. StartDeployment(..., Monitors=[{AlarmArn: <alarm>, AlarmRoleArn: <role>}], with a 1-minute bake window).
5. Poll GetDeployment / ListDeployments for State in (COMPLETE, ROLLED_BACK, REVERTED) and GetEnvironment for a non-transitional State -> neither ever occurs; the non-assumable-role variant is also never refused.
