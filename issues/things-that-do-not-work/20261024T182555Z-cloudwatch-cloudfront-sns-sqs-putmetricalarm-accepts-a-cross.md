Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__cloudwatch__sns__sqs.cloudfront-4xx-alarm-cross-region-sns-action-rejected cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok
Issue-Key: cloudfront__cloudwatch__sns__sqs.cloudfront-4xx-alarm-cross-region-sns-action-rejected|_error#13|AssertionError|raise AssertionError("the call was ACCEPTED but real AWS rejects it")
Issue-Key: cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 0de7718ade8f368be0e47aca52d0dd18a585e082

# cloudwatch__cloudfront__sns__sqs: PutMetricAlarm accepts a cross-region SNS AlarmAction, and no edge traffic is served so a CloudFront 4xx alarm can never fire

> _Note: this issue was identified against the code base at commit `0de7718ade8f368be0e47aca52d0dd18a585e082`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
This issue bundles TWO net-new root causes for the cloudwatch__cloudfront__sns__sqs service. A fixer must close both before submitting. Neither check is marked ALREADY FILED or OWNED in the replay report; the open issue 20261023T140640Z-cloudwatch-cloudfront-sns-sqs-putmetricalarm-accepts-an-sns-alarmaction-in-another-region.md describes the same defect as cause 1 but does not carry this check's replay key — close them together.

Cause 1 of 2: PutMetricAlarm accepts an AlarmAction naming an SNS topic in another region.
AWS: recorded behavior only (the CloudWatch model puts no constraint on AlarmActions beyond a string list): an alarm action must be in the same region as the alarm, and PutMetricAlarm with a foreign-region topic ARN is refused with a 400 ValidationError; nothing is created.
Emulator: PutMetricAlarm returns success, so there is no error code, and the alarm is created with an action that can never be delivered.
Why: a client ships an alarm whose notification path is silently dead — production refuses the same call, so the emulator certifies a configuration that cannot exist.
Fix: PutMetricAlarm's validation of AlarmActions/OKActions/InsufficientDataActions ARNs against the request's region (the cloudwatch service's put_metric_alarm handler and its validation).
Traces: cloudwatch__cloudfront__sns__sqs.cloudfront-4xx-alarm-cross-region-sns-action-rejected.

Cause 2 of 2: the distribution's edge domain serves nothing, so a CloudFront 4xx-error-rate alarm can never be driven to ALARM and the SQS notification path cannot be observed.
AWS: recorded behavior only. Real viewer requests at the distribution's edge domain — one 200 on a present key, then a burst of 4xx on a missing key — raise AWS/CloudFront 4xxErrorRate for that DistributionId, the alarm transitions to ALARM, SNS fans the notification out to the subscribed SQS queue, and an untouched control alarm stays OK.
Emulator: the check's first edge call `_fetch_ok(session, f"{ctx.endpoint_url(subject['DomainName'])}/{present_key}")` never answers 200; the check polls its own budget and is killed by the suite's 60 s cap, so no metric, no alarm transition and no SNS->SQS delivery is ever exercised.
Why: the whole metric -> alarm -> notification chain for CloudFront is unreachable; an observability trajectory sees a permanently OK alarm regardless of edge behavior.
Fix: the CloudFront data-plane listener for distribution domain names and the AWS/CloudFront metric emission it must drive (see the same cause filed for service `cloudfront`), plus alarm evaluation against those datapoints in the cloudwatch service.
Traces: cloudwatch__cloudfront__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok.

## Evidence
Replay output, verbatim tails:
- cloudwatch__cloudfront__sns__sqs.cloudfront-4xx-alarm-cross-region-sns-action-rejected: AssertionError: the call was ACCEPTED but real AWS rejects it — raised at `code, msg = _error(lambda: cw.put_metric_alarm(AlarmActions=[foreign_topic], **alarm))`. Expected: 400 ValidationError. Actual: success.
- cloudwatch__cloudfront__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok: Error: Check timed out after 60.0s. The check is tagged "concern:data-plane"; its data-plane phase is `ok = _fetch_ok(session, f"{ctx.endpoint_url(subject['DomainName'])}/{present_key}")` followed by 4xx bursts at a missing key, then `_alarm(cw, name)` / describe_alarms for the ALARM transition and an SQS receive. None of that is reached.

## Steps to Reproduce
Cause 1: create an SNS topic in region B; PutMetricAlarm in region A (metric AWS/CloudFront 4xxErrorRate, DistributionId dimension) with AlarmActions=[that topic ARN] -> accepted instead of refused with 400 ValidationError.
Cause 2: create a distribution over an S3 origin, wait for Status Deployed, GET https://<DomainName>/<present_key> -> never answers 200, so the 4xx burst, the alarm transition and the SNS->SQS notification can never be observed.
Or replay the two check ids in trace_ids.
