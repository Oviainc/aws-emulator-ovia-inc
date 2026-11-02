Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok
Issue-Key: cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok|run#18|AssertionError|assert "NonBreaching" in settled["StateReason"], (
Issue-Commit: 35f0669fbed72946ad7349f4657cbc8fac9f7c2b

# cloudwatch__cloudfront__sns__sqs: an alarm with TreatMissingData=notBreaching over a metric with no datapoints reports a graded-datapoint OK instead of the missing-data reason (1 cause)

> _Note: this issue was identified against the code base at commit `35f0669fbed72946ad7349f4657cbc8fac9f7c2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
=== Cause 1 of 1: the missing-data arm is not reported in the evaluated StateReason of a control alarm ===
AWS: Smithy: MetricAlarm.TreatMissingData -> TreatMissingData @length 1..255, documented values breaching / notBreaching / ignore / missing; MetricAlarm.StateReason -> StateReason @length 0..1023 ("An explanation for the alarm state, in text format."). The exact wording is recorded behavior only: measured on real AWS, an alarm that settles to OK because its evaluation window had NO datapoints and TreatMissingData=notBreaching reports that it received no data and names the arm it applied - the reason contains 'NonBreaching'. The requirement is that a hole is reported as a hole and the arm that decided the state is named.
Emulator: the untouched control alarm does reach OK, but for the wrong stated reason - 'Threshold not Crossed: 0 out of the last 1 datapoints satisfied GreaterThanThreshold with the threshold (20), fewer than the 1 datapoints required for the alarm to be in ALARM state.' It claims to have graded an evaluation window rather than to have applied notBreaching to a missing period, and it uses a 'Threshold not Crossed' prefix that AWS does not emit for an evaluated state.
Why: the control arm of every "alarm fires on real traffic, twin stays OK on no traffic" pattern is indistinguishable from an alarm that genuinely saw clean data, so a client cannot tell "no CloudFront 4xx occurred" from "CloudFront published nothing at all" - which are different incidents. Note the open issue 20261024T182555Z-cloudwatch-cloudfront-sns-sqs-putmetricalarm-accepts-a-cross.md covers the cross-region AlarmAction acceptance and the absent CloudFront edge data plane for this service; it does not cover the StateReason the control alarm reports, which is what fails here.
Fix: the missing-datapoint classification and StateReason renderer in the cloudwatch alarm evaluator (report 'no datapoints were received ... treated as [NonBreaching]' rather than a graded 0-of-n verdict).
Traces: cloudwatch__cloudfront__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok

## Evidence
Replayed behavioral suite (simparity-behavioral-ga-prod-v1).

AssertionError: alarm sim2real-xsvc-cf4xx-subj-16093c65ddb8 reached OK for a reason other than missing data treated as notBreaching: 'Threshold not Crossed: 0 out of the last 1 datapoints satisfied GreaterThanThreshold with the threshold (20), fewer than the 1 datapoints required for the alarm to be in ALARM state.'

Expected: a StateReason containing 'NonBreaching' (AWS reports that no datapoints were received and names the arm applied).

## Steps to Reproduce
1. Clone the repo, boot the emulator, replay the behavioral suite.
2. PutMetricAlarm in us-east-1 over an AWS/CloudFront 4xx metric with TreatMissingData=notBreaching, EvaluationPeriods=1, and do not publish any datapoint for it.
3. Poll DescribeAlarms until StateValue is OK, then read StateReason.
4. Re-replay cloudwatch__cloudfront__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok.
