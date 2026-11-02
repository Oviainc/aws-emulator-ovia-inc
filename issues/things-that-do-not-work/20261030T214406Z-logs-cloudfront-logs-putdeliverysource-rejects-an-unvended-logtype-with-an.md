Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__logs.obs-cloudfront-3-delivery-source-unsupported-log-type-rejected
Issue-Key: cloudfront__logs.obs-cloudfront-3-delivery-source-unsupported-log-type-rejected|run#11|AssertionError|assert message == "This service is not allowed for this LogSource.", (
Issue-Commit: 7e40a03527fe455980798ee437e03ca50107a3c6

# logs PutDeliverySource rejects an unvended logType with an emulator-authored ValidationException message instead of AWS's "This service is not allowed for this LogSource."

> _Note: this issue was identified against the code base at commit `7e40a03527fe455980798ee437e03ca50107a3c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliverySource

## Description
One net-new root cause.

Cause 1 of 1: PutDeliverySource's refusal message for a logType the named resource's service does not vend is emulator-authored text, not AWS's text. Failing check id: logs__cloudfront.obs-cloudfront-3-delivery-source-unsupported-log-type-rejected.

AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json) models PutDeliverySource.logType as a required LogType with @length min 1 max 255 and @pattern ^[\w]*$, and models ValidationException among its errors. The model therefore accepts any word-character log type; the narrowing to the log types the *resource's* service actually vends (for a CloudFront distribution ARN, only ACCESS_LOGS, as DescribeConfigurationTemplates(service=cloudfront) reports) is a service rule the model does not state — recorded behavior only. The recording shows real CloudWatch Logs refusing such a call with HTTP 400, error code ValidationException and the message, verbatim and fixed in full:
  "This service is not allowed for this LogSource."
That message is entirely fixed text: it has NO variable parts — it does not interpolate the rejected log type, the resource ARN, the delivery-source name, or the list of supported types. AWS phrases the refusal in terms of the service/LogSource mismatch (the resource named by resourceArn belongs to a service that does not vend the requested log type), not in terms of an enum of allowed values.

Emulator: the emulator returns the right error code (ValidationException) but rewrites the message to its own text: "Provided LogType is not valid. Supported options are [ACCESS_LOGS]." — different fixed text, and it additionally interpolates a supported-options list that AWS never sends for this error class. The replay assertion `assert message == "This service is not allowed for this LogSource."` fails with: AssertionError: the rejection must name the log-type/service mismatch, got 'Provided LogType is not valid. Supported options are [ACCESS_LOGS].'

Why: a client that branches on the refusal text — error-message assertions in deployment tooling, CloudFormation/CDK custom resources that classify delivery-source failures, support runbooks, and log scrapers — sees a message that never occurs on real AWS. It also misleads: AWS's wording says the *service* of the named resource is not allowed for this LogSource, whereas the emulator's wording implies a closed enum on logType, which the model explicitly does not have (the pattern is ^[\w]*$). Anything that pattern-matches AWS's sentence fails against the emulator and vice versa, so a fix validated against the emulator will not hold against AWS.

Fix: the message belongs in PutDeliverySource's validation in the CloudWatch Logs handler — services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_delivery_source.rs (and whatever shared delivery-source validation it calls). Keep the HTTP 400 / ValidationException classification and keep rejecting the call (the check's follow-up GetDeliverySource(name) correctly answered ResourceNotFoundException, so the "creates nothing" half already matches); replace only the message text with AWS's fixed sentence for the service/log-type mismatch class, with no interpolated values.

Note: the open issue 20261025T014519Z-logs-cloudwatch-logs-vended-delivery-and-subscription-plumbing-is.md reports that PutDeliverySource validated nothing. It does not cover this: the emulator now does reject the unvended log type, and the surviving divergence is purely the refusal message, which that issue does not name. The report did not mark this check as owned by it.

Traces: logs__cloudfront.obs-cloudfront-3-delivery-source-unsupported-log-type-rejected (verdict: mismatch).

## Evidence
Check: logs__cloudfront.obs-cloudfront-3-delivery-source-unsupported-log-type-rejected [mismatch]

Request (region us-east-1, the only region CloudFront's vended-log delivery plane exists in):
  logs.PutDeliverySource(name=<unique>, resourceArn=<arn of a freshly created CloudFront distribution>, logType="ERROR_LOGS", tags={...})
  "ERROR_LOGS" is model-valid (matches ^[\w]*$, length 1..255) but is not a log type CloudFront vends.

Expected (real AWS, recorded):
  HTTP 400
  Error.Code    = "ValidationException"
  Error.Message = "This service is not allowed for this LogSource."   <- fixed text in full; no variable parts
  and GetDeliverySource(name) -> ResourceNotFoundException (nothing created)

Actual (emulator):
  HTTP 400
  Error.Code    = "ValidationException"            (matches)
  Error.Message = "Provided LogType is not valid. Supported options are [ACCESS_LOGS]."   (diverges)
  GetDeliverySource(name) -> ResourceNotFoundException (matches)

Replay failure detail:
  File "check_logs__cloudfront_obs_cloudfront_3_delivery_source_unsupported_log_type_rejected.py", line 153, in run
    assert message == "This service is not allowed for this LogSource.", (
  AssertionError: the rejection must name the log-type/service mismatch, got 'Provided LogType is not valid. Supported options are [ACCESS_LOGS].'

Model citation: Smithy PutDeliverySource.logType -> LogType, required, @length(min=1,max=255), @pattern ^[\w]*$; errors include ValidationException (client, httpError 400). The per-service restriction on which log types are vendable is not in the model — recorded behavior only.

## Steps to Reproduce
1. Point a botocore/boto3 client at the emulator.
2. cf = client("cloudfront"); create any CloudFront distribution (it may be Enabled=false) and keep its ARN:
   dist = cf.create_distribution(DistributionConfig={... "Enabled": False ...})["Distribution"]; dist_arn = dist["ARN"]
3. logs = client("logs", region_name="us-east-1")  # the delivery plane for a global service lives only in us-east-1
4. Call PutDeliverySource with a model-valid but unvended log type:
   logs.put_delivery_source(name="any-unique-name", resourceArn=dist_arn, logType="ERROR_LOGS")
5. Catch the ClientError and inspect it:
   - expected: HTTP 400, Error.Code == "ValidationException", Error.Message == "This service is not allowed for this LogSource."
   - observed: HTTP 400, Error.Code == "ValidationException", Error.Message == "Provided LogType is not valid. Supported options are [ACCESS_LOGS]."
6. Positive control (already passing, keep it passing): the same ARN with logType="ACCESS_LOGS" is accepted and GetDeliverySource reads back service="cloudfront", logType="ACCESS_LOGS", resourceArns=[dist_arn]; DescribeConfigurationTemplates(service="cloudfront") lists ACCESS_LOGS and not ERROR_LOGS.
7. Re-replay logs__cloudfront.obs-cloudfront-3-delivery-source-unsupported-log-type-rejected to confirm the fix.
