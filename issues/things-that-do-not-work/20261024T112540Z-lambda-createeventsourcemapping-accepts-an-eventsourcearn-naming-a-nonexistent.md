Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__mq.refarch-mq-esm-rejected-when-rabbitmq-broker-does-not-exist
Issue-Key: lambda__mq.refarch-mq-esm-rejected-when-rabbitmq-broker-does-not-exist|_error#10|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Lambda CreateEventSourceMapping accepts an EventSourceArn naming a nonexistent Amazon MQ broker instead of rejecting it ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One cause.

Cause 1 of 1: CreateEventSourceMapping does not resolve an Amazon MQ (RABBITMQ) EventSourceArn against the MQ service before creating the mapping, so a mapping is created for a broker that does not exist.

AWS: Smithy (com-amazonaws-lambda-awsgirapiservice) models CreateEventSourceMapping POST /2015-03-31/event-source-mappings with errors InvalidParameterValueException (400), ResourceConflictException (409), ResourceNotFoundException (404), ServiceException (500), TooManyRequestsException (429); EventSourceArn is an `Arn` (length 0..10000, ARN pattern) and Queues is a 1-element list. Real AWS validates the function first and then resolves the event source: an EventSourceArn naming an Amazon MQ broker that does not exist is refused 404 ResourceNotFoundException. Recorded behavior of the refusal message (an example of the rule "the named broker resource does not exist"): "The broker resource: eventSourceArn you requested does not exist." The requirement is the status/code pair 404 + ResourceNotFoundException and the fact that no mapping is created; the exact message text is an example.

Emulator: the call SUCCEEDS. With a real function, a real BASIC_AUTH secret, Queues=["testQueue"], SourceAccessConfigurations BASIC_AUTH + VIRTUAL_HOST and an EventSourceArn of the form arn:aws:mq:<region>:<account>:broker:<name-that-ListBrokers-does-not-return>:b-00000000-0000-0000-0000-000000000000, CreateEventSourceMapping returns a mapping configuration rather than raising. The check's error helper reports "the call succeeded; real AWS rejects it, so there is no error code" — the emulator never consults MQ state for the referenced broker, so the reference boundary between Lambda and Amazon MQ is not enforced at all.

Why: a client that relies on AWS rejecting a bad broker reference gets a mapping that exists in the emulator but could never connect; the configuration error is silently accepted at create time instead of surfacing as a 404. Infrastructure tooling (CDK/CFN/Terraform-style apply-then-verify, or any deploy that treats a successful CreateEventSourceMapping as proof the source resolves) proceeds on a broken estate, and a consumer appears healthy while processing nothing. The subsequent assertions of this check — that ListEventSourceMappings for the fake ARN is empty and that the function has no mapping — are also wrong in the emulator, since the accepted mapping is now listed.

Fix: CreateEventSourceMapping's validation in the Lambda handler — after the function is resolved, resolve an mq: EventSourceArn against Amazon MQ broker state (by broker id/name in the ARN) and raise ResourceNotFoundException (HTTP 404) with no state mutation when no such broker exists; keep the existing validation ORDER (function checked first) so a request with a bad function still reports the function. The positive control in the same check (an SQS queue ARN that does exist must still be accepted and reach Enabled) must keep passing.

Traces: lambda__mq.refarch-mq-esm-rejected-when-rabbitmq-broker-does-not-exist (mismatch; AssertionError raised at the create call, before the ResourceNotFoundException code/message assertions and the ListEventSourceMappings absence assertions). Note: this check id carries the composite token `lambda__mq`; the defect is in the Lambda control plane, so it is filed under `lambda`.

## Evidence
Check lambda__mq.refarch-mq-esm-rejected-when-rabbitmq-broker-does-not-exist.

Expected (real AWS): CreateEventSourceMapping(EventSourceArn=arn:aws:mq:<region>:<account>:broker:<absent-broker>:b-00000000-0000-0000-0000-000000000000, FunctionName=<existing fn>, Queues=["testQueue"], BatchSize=1, SourceAccessConfigurations=[BASIC_AUTH <secret-arn>, VIRTUAL_HOST "/"]) -> HTTP 404, Error.Code = "ResourceNotFoundException" (recorded message example: "The broker resource: eventSourceArn you requested does not exist."), and afterwards ListEventSourceMappings(EventSourceArn=<fake arn>) == [] and ListEventSourceMappings(FunctionName=<fn>) == [].

Actual (emulator): the call returns 2xx with an EventSourceMappingConfiguration; no ClientError is raised. Replay output:
  File "check_lambda__mq_refarch_mq_esm_rejected_when_rabbitmq_broker_does_not_exist.py", line 57, in run
    code, message = _error(lambda: lam.create_event_source_mapping(EventSourceArn=fake_arn, ...))
  File "...", line 129, in _error
    raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
  AssertionError: the call succeeded; real AWS rejects it, so there is no error code

The check first proves the broker really is absent via ListBrokers (no broker with that name is returned), so the ARN genuinely resolves to nothing in the emulator's own MQ state.

## Steps to Reproduce
1. Create an IAM role for Lambda and a Lambda function with it; create a Secrets Manager secret holding {"username":"admin","password":...}.
2. Pick a broker name that ListBrokers does not return (confirm with mq ListBrokers, paginated).
3. Call lambda CreateEventSourceMapping with EventSourceArn=arn:aws:mq:<region>:<account>:broker:<that-name>:b-00000000-0000-0000-0000-000000000000, FunctionName=<the function>, Queues=["testQueue"], BatchSize=1, SourceAccessConfigurations=[{"Type":"BASIC_AUTH","URI":<secret arn>},{"Type":"VIRTUAL_HOST","URI":"/"}].
4. Observe: the emulator returns a mapping. Expected: HTTP 404 ResourceNotFoundException and no mapping created.
5. Call lambda ListEventSourceMappings(EventSourceArn=<the fake arn>) and ListEventSourceMappings(FunctionName=<the function>): both must be empty; in the emulator the accepted mapping is listed.
