Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group
Issue-Key: elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group|_http_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: afd9c28cd10d0281c7cc3c2a5c51ef0c1f3c612b

# elasticbeanstalk__logs: the StreamLogs scenario never reaches an observable state — no PROCESSED version, no Ready environment and no derived CloudWatch Logs group (1 cause)

> _Note: this issue was identified against the code base at commit `afd9c28cd10d0281c7cc3c2a5c51ef0c1f3c612b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEnvironment

## Description
One net-new behavioral root cause grounded in the only failing check of the simparity-behavioral-aws-v1 replay under this heading (elasticbeanstalk__logs 0/1). Related open issues, named for the fixer: 20261029T180136Z-elasticbeanstalk-elastic-beanstalk-application-versions-stay-status-unprocessed-forever.md and 20261029T180136Z-elasticbeanstalk-elastic-beanstalk-lifecycle-and-log-retrieval-operations-are.md record the upstream application-version and lifecycle gaps; this check is not marked OWNED by either, and what it grades is the CloudWatch Logs leg beyond them.

Cause 1 of 1 — the log-streaming option never produces its CloudWatch Logs group, because the environment never becomes an environment that serves.
AWS: with the aws:elasticbeanstalk:cloudwatch:logs option StreamLogs=true, Elastic Beanstalk streams each instance's on-instance log files to CloudWatch Logs under a group whose name is the documented prefix /aws/elasticbeanstalk/<environment-name> plus the on-instance path (for an Amazon Linux 2023 Python platform, /aws/elasticbeanstalk/<env>/var/log/web.stdout.log carries the application's stdout); the RetentionInDays option is applied to that log group itself; a log file the platform does not stream (a Ruby-only path on a Python platform) gets NO group at all; and a line the application prints to stdout arrives as an event in that group, in a stream attributable to one of the instance ids DescribeEnvironmentResources reports. Reference: https://docs.aws.amazon.com/elasticbeanstalk/latest/dg/AWSHowTo.cloudwatchlogs.html (cited by the check). The preconditions AWS satisfies on the way there: CreateApplicationVersion leaves the version at Status PROCESSED, the environment settles at Status exactly "Ready", and the environment's CNAME answers HTTP.
Emulator: the check exhausts its whole budget (timed out after 60.0s) without reaching any of its CloudWatch Logs assertions. None of the observables it waits on is ever produced: the application version leaving Processing for PROCESSED, the environment reaching Ready, the CNAME answering, or the derived log group appearing in DescribeLogGroups. Note the check's own internal poll deadlines are generous (600 s for the log group, 1500 s for Ready), so the timeout is not a tight budget on a slow-but-correct path — an emulator with no instances to provision should produce these states immediately.
Why: a client that turns on StreamLogs gets no log group, no retention setting and no application output in CloudWatch Logs, and cannot observe its application's stdout at all; it also cannot tell "the platform does not stream that file" (a group that must not exist) from "nothing streams anything".
Fix: the elasticbeanstalk crate's environment/application-version state machine (settle a created version to PROCESSED and an environment to Ready, and report its instances) plus the CloudWatch Logs delivery the aws:elasticbeanstalk:cloudwatch:logs option settings must drive — creating the derived group with RetentionInDays applied, and only for the paths the platform streams.
Traces: elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout.

[mismatch] elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group
Error: Check timed out after 60.0s.

The check's gates, in order, none of which was passed: application version leaving Processing with "assert status.upper() == 'PROCESSED'"; the environment reaching Status exactly "Ready"; the app answering on its CNAME (the check marks an unconnectable-but-provisioned environment DATAPLANE_UNIMPLEMENTED: "nothing satisfying the condition came back from <url> within <timeout>s"); then the CloudWatch Logs assertions it never reached:
  assert stdout_group in groups  (stdout_group = /aws/elasticbeanstalk/<env>/var/log/web.stdout.log)
  assert groups[stdout_group].get('retentionInDays') == retention_days
  assert ruby == []  (no group for a path this platform does not stream)
  assert hits  (the exact line "SIM2REAL-LOG-LINE <nonce>" the app printed, delivered as an event)
  assert names and any(instance id in stream name)  (a stream attributable to a real instance)

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. aws elasticbeanstalk create-application, then create-application-version from a bundle for an "Amazon Linux 2023 ... running Python" solution stack and poll describe-application-versions.
   Observed: the version never reaches Status PROCESSED.
3. create-environment on that version with option settings aws:elasticbeanstalk:cloudwatch:logs StreamLogs=true and RetentionInDays=<n>, then poll describe-environments.
   Observed: the environment never settles at Status "Ready" and its CNAME never answers HTTP.
4. aws logs describe-log-groups --log-group-name-prefix /aws/elasticbeanstalk/<env>
   Observed: no group. AWS: /aws/elasticbeanstalk/<env>/var/log/web.stdout.log exists with retentionInDays set, and carries the application's stdout.
5. Re-replay the check id in trace_ids.
