Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected
Issue-Key: cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected|run#13|AssertionError|assert message == expected, f"the rejection message is {message!r}, expected {expected!r}"
Issue-Commit: 949cf2f2a2f42313b725a3f96914da0fa4cf14da

# logs PutDeliverySource refuses a global-service (CloudFront) resource outside us-east-1 with an emulator-authored validation message instead of AWS's "not supported for this LogType in region <region>"

> _Note: this issue was identified against the code base at commit `949cf2f2a2f42313b725a3f96914da0fa4cf14da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliverySource

## Description
One net-new root cause. The region pinning itself is now implemented and the error code is right; only the message diverges, and it diverges structurally (different sentence, and it splices request values AWS's message does not carry).

=== Cause 1 of 1: PutDeliverySource's region-pin ValidationException message ===
AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json): PutDeliverySource takes name, resourceArn (Arn, free-form) and logType (required, pattern ^[\w]*$), and declares ValidationException, errorKind client, httpError 400, doc "One of the parameters for the request is not valid." The region rule is NOT in the model — PutDeliverySource is declared for every CloudWatch Logs region and resourceArn is a free-form ARN — so this is recorded behavior only: a vended-log source for a CloudFront distribution's ACCESS_LOGS can be created only in us-east-1, and the same call in any other region is refused with HTTP 400, code ValidationException and the message "PutDeliverySource action is not supported for this LogType in region <region>". Fixed text: "PutDeliverySource action is not supported for this LogType in region " (it names the operation and the word LogType, NOT the logType value); the only variable part is <region>, the region the request was signed for / reached on (us-west-2 in the recording). The ARN and the home region do NOT appear.
Emulator: same code (ValidationException) and status, but the message is an invented validation sentence that embeds the resourceArn and both regions: "1 validation error detected: resourceArn <distribution ARN> names a resource of a global service, which is reached only through us-east-1, so its delivery source must be created in us-east-1 and not in <region>". It is produced by crate::state::misplaced_delivery_source_message("resourceArn", resourceArn, region) in the PutDeliverySource handler; it also mimics AWS's constraint-violation prefix ("1 validation error detected: ..."), which AWS does not use for this refusal.
Why: a client that keys on the refusal text to decide "retry this put in the home region" / "this log type is not supported here" sees text AWS never emits, and the emulator additionally leaks a distribution ARN into an error body AWS keeps free of it, so recorded-response comparison and any message-based branch diverge. The code and absence-of-side-effect legs already behave correctly, so this is a pure message-fidelity defect.
Fix: the PutDeliverySource handler's region-pin rejection and the shared message helper it calls — keep the refusal and its ValidationException classification, emit AWS's fixed sentence with only the request's region interpolated. Related but distinct open issue: 20261025T014519Z-logs-cloudwatch-logs-vended-delivery-and-subscription-plumbing-is.md says PutDeliverySource "validates nothing"; validation now happens, and this check is not owned by it — only the wording of the refusal remains wrong.
Traces: cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected. Emulator code: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_delivery_source.rs lines 213-225 (the misplaced_delivery_source_message guard).

## Evidence
Expected (AWS, recorded): HTTP 400, code ValidationException, message "PutDeliverySource action is not supported for this LogType in region us-west-2" (the trailing token is the request's region; the rest is fixed text).
Actual (emulator): HTTP 400, code ValidationException, message "1 validation error detected: resourceArn arn:aws:cloudfront::<account>:distribution/<id> names a resource of a global service, which is reached only through us-east-1, so its delivery source must be created in us-east-1 and not in us-west-2".
Replay detail: AssertionError: the rejection message is '1 validation error detected: resourceArn arn:aws:cloudfront::000000000000:distribution/E7IYCGPKTANSF3 names a resource of a global service, which is reached only through us-east-1, so its delivery source must be created in us-east-1 and not in us-west-2', expected 'PutDeliverySource action is not supported for this LogType in region us-west-2'. The preceding assertion on code == ValidationException passed, so only the message text diverges. (The quoted account id, distribution id and the particular away-region are per-run values filling the template, not requirements.)

## Steps to Reproduce
1. cloudfront.CreateDistributionWithTags for a disabled distribution (any custom origin); note Distribution.ARN.
2. In a region other than us-east-1 (e.g. us-west-2), call logs.PutDeliverySource(name=<name>, resourceArn=<distribution ARN>, logType="ACCESS_LOGS").
3. Expect HTTP 400, ValidationException, message "PutDeliverySource action is not supported for this LogType in region us-west-2". The emulator returns ValidationException with the "1 validation error detected: resourceArn ... names a resource of a global service ..." sentence instead.
4. Controls that already pass and must keep passing: logs.GetDeliverySource(name=<name>) in the away region answers 400 ResourceNotFoundException "Requested Delivery Source does not exist in this account." (nothing was created), and the identical PutDeliverySource in us-east-1 succeeds with deliverySource.service="cloudfront", logType="ACCESS_LOGS" and an ARN in us-east-1.
