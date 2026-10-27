Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected
Issue-Key: cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected|_error#22|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 6d436f6d30cd83c53e890485fd5b45dc369d520d

# CloudWatch Logs delivery-source state is not region-partitioned: a source created in us-east-1 is served by GetDeliverySource in every other region

> _Note: this issue was identified against the code base at commit `6d436f6d30cd83c53e890485fd5b45dc369d520d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeliverySource

## Description
One net-new root cause.

Cause 1 of 1: the CloudWatch Logs vended-log control plane keeps delivery sources in a store that is not keyed by region, so a delivery source created in one region is visible — and returned with a 200 — to GetDeliverySource issued against a different region's endpoint. Failing check: cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected.

AWS: PutDeliverySource/GetDeliverySource are regional operations on regional resources. Smithy: GetDeliverySource.name @required, target DeliverySourceName @length 1..60, @pattern ^[\w-]*$; modeled errors ResourceNotFoundException, ValidationException, ServiceQuotaExceededException, ThrottlingException, ServiceUnavailableException (all client errors HTTP 400). The model says nothing about region scoping — that is recorded behavior only: a delivery source created by PutDeliverySource in us-east-1 does not exist in any other region, and GetDeliverySource(name=<that name>) against another region's endpoint is refused with HTTP 400, error code ResourceNotFoundException and the message "Requested Delivery Source does not exist in this account." In that message the whole sentence is fixed text — it carries no values from the request or from state.

Emulator: the emulator gets the region pinning of the CloudFront ACCESS_LOGS log type right (PutDeliverySource against the non-home region is correctly refused with ValidationException and the message "PutDeliverySource action is not supported for this LogType in region <region>", and GetDeliverySource in that region then correctly answers ResourceNotFoundException). But after the SAME name is created successfully in us-east-1, GetDeliverySource against the other region's endpoint answers HTTP 200 and returns that us-east-1 source verbatim — including an arn whose region field is us-east-1 (arn:aws:logs:us-east-1:<account>:delivery-source/<name>-shaped value), service "cloudfront", logType "ACCESS_LOGS" and the tags supplied on the home-region call. So the lookup ignores the request's region entirely and reads one global map; the account/region partitioning that the earlier refusal appeared to honour is only an absence of data, not isolation.

Why: a client relying on AWS's regional isolation gets wrong answers in both directions. Code that probes a region to decide whether to create a delivery source there will see one that does not exist and skip creation, leaving the region unconfigured. Code that reads a source to learn where its logs are delivered will act on an ARN belonging to a different region. Any test or deployment that uses "does this exist in this region?" as a guard silently cross-contaminates regions, and the same unkeyed store will make DeleteDeliverySource in one region destroy another region's resource.

Fix: belongs in the CloudWatch Logs delivery-source state (the vended-delivery store in service-com-amazonaws-cloudwatchlogs-logs-20140328) and in the GetDeliverySource handler's lookup: the store key must include the request's region (alongside account) and GetDeliverySource must only resolve names created in the region the request was sent to, raising ResourceNotFoundException ("Requested Delivery Source does not exist in this account.") otherwise. The same keying applies to the sibling operations that read and mutate this state (PutDeliverySource, DeleteDeliverySource, DescribeDeliverySources) so region isolation is uniform.

Traces: cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected (the final assertion, after the positive control created the source in us-east-1).

## Evidence
Check cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected, last assertion:

  still_absent = _error(lambda: away.get_delivery_source(name=name))
  assert still_absent == ("ResourceNotFoundException", "Requested Delivery Source does not exist in this account.")

Expected (AWS): the call against the non-us-east-1 endpoint is refused — HTTP 400, Error.Code "ResourceNotFoundException", Error.Message "Requested Delivery Source does not exist in this account." (entirely fixed text).

Actual (emulator): HTTP 200 with the us-east-1 resource:
  AssertionError: expected the call to be rejected, but it succeeded: {'deliverySource': {'name': '<unique name>', 'arn': 'arn:aws:logs:us-east-1:<account>:delivery-source:<unique name>', 'resourceArns': ['arn:aws:cloudfront::<account>:distribution/<dist id>'], 'service': 'cloudfront', 'logType': 'ACCESS_LOGS', 'tags': {'sim2real': 'behavioral'}}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

The names, ids, account and timestamps above are per-run values; the requirement is the status/code/message and the fact that the non-home region must hold no such resource.

Note the earlier legs of the same check PASSED, which localises the defect: PutDeliverySource(logType=ACCESS_LOGS) in the away region was correctly refused with ValidationException and the exact message, and GetDeliverySource there correctly answered ResourceNotFoundException while no source existed anywhere. Only once the source existed in us-east-1 did the away-region read start succeeding.

## Steps to Reproduce
1. Pick two regions: HOME=us-east-1 and AWAY=us-west-2.
2. Create a CloudFront distribution and take its ARN (any ARN reproduces the state-keying defect; the CloudFront ACCESS_LOGS log type is what pins the home region).
3. logs(AWAY).PutDeliverySource(name=N, resourceArn=<dist arn>, logType="ACCESS_LOGS") -> correctly refused: 400 ValidationException "PutDeliverySource action is not supported for this LogType in region us-west-2".
4. logs(AWAY).GetDeliverySource(name=N) -> correctly refused: 400 ResourceNotFoundException "Requested Delivery Source does not exist in this account.".
5. logs(HOME).PutDeliverySource(name=N, resourceArn=<dist arn>, logType="ACCESS_LOGS", tags={...}) -> 200, source created in us-east-1.
6. logs(AWAY).GetDeliverySource(name=N) again -> EXPECTED 400 ResourceNotFoundException "Requested Delivery Source does not exist in this account."; OBSERVED 200 returning the us-east-1 deliverySource (arn region field = us-east-1).
