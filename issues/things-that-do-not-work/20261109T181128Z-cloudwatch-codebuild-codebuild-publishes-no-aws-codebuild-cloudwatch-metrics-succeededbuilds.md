Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__codebuild.build-outcome-splits-succeeded-and-failed-builds-in-the-codebuild-namespace
Issue-Key: cloudwatch__codebuild.build-outcome-splits-succeeded-and-failed-builds-in-the-codebuild-namespace|_poll_until_stable#18|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# CodeBuild publishes no AWS/CodeBuild CloudWatch metrics: SucceededBuilds, FailedBuilds, Builds and Duration never appear for a project that ran builds

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
AWS: recorded behavior only (metric emission is not in the Smithy model). CodeBuild publishes per-project metrics into the CloudWatch namespace 'AWS/CodeBuild' with dimension ProjectName=<project name>. After one build that CodeBuild itself reports SUCCEEDED and one that it reports FAILED on the same project, GetMetricStatistics(Namespace='AWS/CodeBuild', MetricName='SucceededBuilds'|'FailedBuilds', Dimensions=[{ProjectName: <project>}], Period=300, Statistics=['Sum','Maximum','SampleCount']) returns datapoints whose Sums are 1 and 1 respectively (not 2/0), MetricName 'Builds' sums to the number of builds, and 'Duration' is published in Seconds while the build counters are in Count. The variable parts are the project name and the counts/durations of the builds the account actually ran.
Emulator: the builds run and reach SUCCEEDED and FAILED (BatchGetBuilds agrees), but GetMetricStatistics for those metric names in AWS/CodeBuild with that ProjectName dimension returns an empty Datapoints list for the whole 40-minute window, for the entire metric budget of the check — no AWS/CodeBuild metric is ever published for the project.
Why: every client that watches build health through CloudWatch — alarms on FailedBuilds, dashboards summing Builds, a Duration-based regression guard — reads nothing at all and cannot distinguish 'no builds ran' from 'builds ran and failed'. An alarm configured on these metrics stays in INSUFFICIENT_DATA forever, so the emulator silently reports healthy.
Fix: the build-completion path of the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006) must publish the AWS/CodeBuild metric data (Builds, SucceededBuilds, FailedBuilds in Count and Duration in Seconds, dimensioned by ProjectName) into the CloudWatch metric store so GetMetricStatistics/ListMetrics can serve them. Related open issue 20261022T172625Z-cloudwatch-codebuild-builds-fault-at-provisioning-so-aws.md describes the earlier state in which builds FAULTed at PROVISIONING so no outcome could be attributed; on this checkout the builds do reach SUCCEEDED/FAILED and the remaining gap is metric publication, which that issue carries no replay key for.
Traces: cloudwatch__codebuild.build-outcome-splits-succeeded-and-failed-builds-in-the-codebuild-namespace.

## Evidence
Replay of the behavioral suite (simparity-behavioral-aws-v1) against the emulator built from this checkout; per-service line: cloudwatch__codebuild 0/1 (0%), 1 failed.

cloudwatch__codebuild.build-outcome-splits-succeeded-and-failed-builds-in-the-codebuild-namespace [error]: TimeoutError: 'AWS/CodeBuild SucceededBuilds and FailedBuilds for ProjectName=<project> settling on one value: the run budget ended first; not graded'. The check had already driven one build to SUCCEEDED and one to FAILED on that project via StartBuild/BatchGetBuilds (those assertions passed); the failure is in the poll of GetMetricStatistics(Namespace='AWS/CodeBuild', MetricName in {'SucceededBuilds','FailedBuilds'}, Dimensions=[{Name:'ProjectName', Value:<project>}], StartTime=now-40min, Period=300, Statistics=['Sum','Maximum','SampleCount']), which returned no datapoints at all (the Sum helper yields None for an empty Datapoints list) for the whole budget. Expected SucceededBuilds Sum 1.0 and FailedBuilds Sum 1.0, Builds Sum 2.0, Duration in Seconds and the counters in Count.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Create a CodeBuild service role and a NO_SOURCE / NO_ARTIFACTS project (image aws/codebuild/amazonlinux2-x86_64-standard:5.0, BUILD_GENERAL1_SMALL).
3. StartBuild with a buildspec whose build phase runs `exit 0` and wait for SUCCEEDED; StartBuild with a buildspec whose build phase runs `exit 1` and wait for FAILED.
4. Call GetMetricStatistics(Namespace='AWS/CodeBuild', MetricName='SucceededBuilds', Dimensions=[{'Name':'ProjectName','Value':<project>}], StartTime=now-40min, EndTime=now+5min, Period=300, Statistics=['Sum','Maximum','SampleCount']) and the same for 'FailedBuilds', 'Builds' and 'Duration'.
5. Expect datapoints summing to 1, 1 and 2 with Unit Count (Duration in Seconds); observe an empty Datapoints list for every metric.
6. Re-replay cloudwatch__codebuild.build-outcome-splits-succeeded-and-failed-builds-in-the-codebuild-namespace to confirm.
