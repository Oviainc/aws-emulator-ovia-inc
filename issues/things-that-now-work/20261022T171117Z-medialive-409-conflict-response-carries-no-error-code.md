Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-cloudwatch-alarm-template-group-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-cloudwatch-alarm-template-group-lifecycle medialive-template-error-paths
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|CreateCloudWatchAlarmTemplateGroup|4xx->4xx|$.code
Issue-Key: medialive|CreateCloudWatchAlarmTemplateGroup|4xx->4xx|$.message
Issue-Key: medialive|CreateCloudWatchAlarmTemplateGroup|4xx->4xx|$.requestId
Issue-Key: medialive|CreateCloudWatchAlarmTemplateGroup|4xx->4xx|$.retryable
Issue-Key: medialive|CreateCloudWatchAlarmTemplateGroup|4xx->4xx|$.statusCode
Issue-Key: medialive|CreateCloudWatchAlarmTemplateGroup|4xx->4xx|$.time
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# medialive 409 conflict response carries no error code or request id

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCloudWatchAlarmTemplateGroup

## Description
Cause 1 of 1: creating a CloudWatch alarm template group whose name is already taken is correctly refused with HTTP 409, but the response is not identifiable as a ConflictException — the error type and request id are absent from the payload.
AWS: Smithy: CreateCloudWatchAlarmTemplateGroup (POST /prod/cloudwatch-alarm-template-groups) models ConflictException with errorKind client and httpError 409. The recorded refusal is HTTP 409 carrying the error code 'ConflictException', a request id and a time, and is non-retryable. The recorded message text ('Name already in use') is an example, not a requirement.
Emulator: answers HTTP 409 (correct) but the body has no error-type discriminator at all: $.code is absent (expected 'ConflictException'), as are $.statusCode (409), $.retryable (false), $.requestId and $.time. The only field present is a message, 'a cloudwatch alarm template group named <name> already exists in us-west-2 (id 1000000)'.
Why: a restJson1 client maps a 4xx to a modeled exception from the error-type discriminator; with none present, the SDK surfaces an opaque/unknown error instead of ConflictException, so a caller's `catch (ConflictException)` — the normal idempotent create-or-reuse path — never fires and the create fails hard. The missing request id also removes the only handle for correlating the failure.
Fix: the medialive error serializer for the conflict path — emit the modeled error type so the SDK can resolve it to ConflictException, and populate the request id on error responses (the status 409 itself is already right).
Traces: medialive-cloudwatch-alarm-template-group-lifecycle#task#6, medialive-template-error-paths#task#3.

## Evidence
Replay of simparity-wire-aws-v1, cluster service=medialive|CreateCloudWatchAlarmTemplateGroup|status=409->409 (verdict=mismatch, 2 affected steps):
  $.code: missing (expected 'ConflictException', got None)
  $.statusCode: missing (expected 409, got None)
  $.retryable: missing (expected False, got None)
  $.requestId: missing (expected shaped value, got None)
  $.time: missing (expected shaped value, got None)
  $.message: value_mismatch (expected 'Name already in use', got 'a cloudwatch alarm template group named sim2real-probe-medialive-cwgroup-7f786eeb already exists in us-west-2 (id 1000000)')  <- message text is an example only
Smithy: CreateCloudWatchAlarmTemplateGroup errors include ConflictException, errorKind client, httpError 409.

## Steps to Reproduce
1. POST /prod/cloudwatch-alarm-template-groups with name N — succeeds (201).
2. POST /prod/cloudwatch-alarm-template-groups again with the same name N.
3. The response is HTTP 409 but carries no error-type discriminator, so an SDK cannot resolve it to ConflictException, and no request id is returned.
4. Equivalent replay: fixture medialive-cloudwatch-alarm-template-group-lifecycle step task#6 (also medialive-template-error-paths#task#3).
