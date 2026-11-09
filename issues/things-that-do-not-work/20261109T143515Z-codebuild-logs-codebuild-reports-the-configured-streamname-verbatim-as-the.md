Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__logs.codebuild-buildspec-stdout-reaches-the-configured-cloudwatch-log-group codebuild__logs.codebuild-log-delivery-denied-without-putlogevents-permission
Issue-Key: codebuild__logs.codebuild-buildspec-stdout-reaches-the-configured-cloudwatch-log-group|run#23|AssertionError|assert stream == f"{stream_prefix}/{build['id'].split(':')[-1]}", (
Issue-Key: codebuild__logs.codebuild-log-delivery-denied-without-putlogevents-permission|_poll_for_marker#26|Timeout|time.sleep(3)
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# CodeBuild reports the configured streamName verbatim as the build's log stream instead of <streamName>/<buildId>, and the denied-PutLogEvents check never settles (2 causes)

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetBuilds

## Description
Cause 1 of 2: the resolved CloudWatch log stream of a build is the configured prefix, not the prefix plus the build's own id.
AWS: with cloudWatchLogs {groupName, streamName} configured, a build's logs block reports streamName as the stream the build actually wrote, which is the configured stream name followed by the build's id ('<streamName>/<build-uuid>'), so two builds of one project never share a stream (Smithy codebuild LogsLocation.streamName / CloudWatchLogsConfig).
Emulator: the logs block reports the configured value unchanged — streamName 'sim2real-stream' for build '<project>:<build-uuid>' — so every build of the project reports the same stream.
Why: a client cannot locate one build's log events; concurrent builds appear to write the same stream and per-build log links are wrong.
Fix: the logs-location resolution in the codebuild crate (derive the reported stream from the configured streamName plus the build id) and the stream the build's events are written to.
Traces: codebuild__logs.codebuild-buildspec-stdout-reaches-the-configured-cloudwatch-log-group.

Cause 2 of 2: the denied-PutLogEvents negative never reaches a verdict.
AWS: a build whose service role cannot logs:PutLogEvents still runs but delivers no log events, which the check observes inside its budget.
Emulator: 'Check timed out after 180.0s' — the build never settles into a terminal phase for the check to grade.
Why: a client cannot tell 'delivery was denied' from 'the build never ran'.
Fix: the build lifecycle/phase settling in the codebuild crate for a role without log permissions.
Traces: codebuild__logs.codebuild-log-delivery-denied-without-putlogevents-permission.

## Evidence
Cause 1 — 'AssertionError: the resolved stream must be the configured prefix plus this build's own id, reported 'sim2real-stream' for build '<project>:<build-uuid>'; whole logs block {'groupName': ..., 'streamName': 'sim2real-stream', 'cloudWatchLogs': {...}, 's3Logs': {'status': 'DISABLED'}}'.
Cause 2 — 'Error: Check timed out after 180.0s'.

## Steps to Reproduce
Replay codebuild__logs.codebuild-buildspec-stdout-reaches-the-configured-cloudwatch-log-group (start a build with cloudWatchLogs configured and read BatchGetBuilds' logs block) and .codebuild-log-delivery-denied-without-putlogevents-permission.
