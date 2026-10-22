Issue-Source: trace-sweep-divergence
Issue-Trace: firehose-create-delivery-stream-argument-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: firehose-create-delivery-stream-argument-validation firehose-create-delivery-stream-malformed-role-arn firehose-start-delivery-stream-encryption-key-validation
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|CreateDeliveryStream|4xx->2xx|$.__type
Issue-Key: firehose|CreateDeliveryStream|4xx->2xx|$.message
Issue-Key: firehose|CreateDeliveryStream|4xx->2xx|status
Issue-Key: firehose|CreateDeliveryStream|4xx->4xx|$.__type
Issue-Key: firehose|CreateDeliveryStream|4xx->4xx|$.message
Issue-Key: firehose|StartDeliveryStreamEncryption|4xx->4xx|$.__type
Issue-Key: firehose|StartDeliveryStreamEncryption|4xx->4xx|$.message
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# Firehose argument validation admits invalid input and misclassifies rejections on stream create and encryption start

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
Three distinct root causes in the firehose service's request-validation stage, found by replaying the wire fixture corpus (trace set simparity-wire-aws-v1). In each, the emulator either admits input that real Firehose refuses, or refuses it under an error identity real Firehose cannot produce for that operation. firehose is reported bootstrapped by get_bootstrap_status (service-com-amazonaws-firehose-firehose-20150804), so these are fidelity defects, not a bootstrap gap.

Related but NOT covering: the open behavioral issue 'Firehose CreateDeliveryStream/PutRecordBatch are overly permissive and misclassify rejections (7 causes)' (20261022T064119Z-firehose-createdeliverystream-putrecordbatch-are-overly-permissive-and-misclassify.md) describes the same class of defect from the behavioral suite, but it carries no wire replay key, and the replay report lists no owner for any of the clusters below. The fixture steps named here must be re-replayed to confirm these are closed.

Cause 1 of 3: CreateDeliveryStream does not resolve RoleARN, so a destination role that cannot be assumed is accepted.
AWS: Smithy: CreateDeliveryStream models InvalidArgumentException (errorKind client, HTTP 400, 'The specified input parameter has a value that is not valid'); the destination configurations' RoleARN members carry pattern ^arn:.*:iam::\d{12}:role/[a-zA-Z_0-9+=,.@\-_/]+$. Recorded behavior: when RoleARN names an IAM role that Firehose cannot assume, CreateDeliveryStream is refused synchronously with HTTP 400 and __type InvalidArgumentException; the recorded message is an example of that rule ('Firehose is unable to assume role arn:aws:iam::000000000000:role/sim2real-fh-argval-no-such-role. Please check the role provided.').
Emulator: HTTP 200 with a CreateDeliveryStream success body; no __type and no message are emitted at all. No IAM resolution or assumability check runs.
Why: a client that treats CreateDeliveryStream as the gate for role misconfiguration gets a stream that reports healthy and can never deliver. Negative tests for an unassumable role pass against the emulator and fail against AWS, and the IAM wiring defect only shows up later as silent delivery loss.
Fix: the CreateDeliveryStream handler's validation stage must resolve the destination RoleARN in the IAM state store and verify assumability for the Firehose service principal before admitting the stream, mapping failure to InvalidArgumentException/400.
Traces: firehose-create-delivery-stream-argument-validation#task#1.

Cause 2 of 3: a malformed RoleARN is refused with an error code this operation does not model.
AWS: recorded behavior only for the code: a malformed role ARN is refused with HTTP 400 and __type InvalidParameterValueException, message 'Invalid role ARN.' (example). Smithy: CreateDeliveryStream's modeled error set is InvalidArgumentException, InvalidKMSResourceException, LimitExceededException, ResourceInUseException; ValidationException is not a member of that set, and the shape ValidationException/InvalidParameterValueException is not even present in the service model, so the code the emulator returns cannot come from real Firehose.
Emulator: HTTP 400 with __type ValidationException and a generic Smithy constraint message, e.g. "1 validation error detected: Value at 's3DestinationConfiguration.roleARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:.*:iam::\d{12}:role/[a-zA-Z_0-9+=,.@\-_/]+" (example text).
Why: clients branch on the error code. An unmodeled ValidationException is in no generated SDK's error union for CreateDeliveryStream, so classification and retry logic written against the service's parameter-value error falls through to a generic unmodeled failure, and modelled-error handling cannot be exercised at all.
Fix: the CreateDeliveryStream handler must raise the service's own parameter-value error for ARN-shape violations instead of letting the generic Smithy constraint validator answer; the generic validator must not be the terminal responder for members this operation validates itself.
Traces: firehose-create-delivery-stream-malformed-role-arn#task#1.

Cause 3 of 3: StartDeliveryStreamEncryption does not enforce the KeyARN/KeyType dependency, and answers a resource error instead.
AWS: Smithy: StartDeliveryStreamEncryption takes DeliveryStreamEncryptionConfigurationInput, whose KeyType is @required and whose KeyARN documentation states 'If you set KeyType to CUSTOMER_MANAGED_CMK, you must specify the Amazon Resource Name (ARN) of the CMK'; the operation models InvalidArgumentException (client, HTTP 400) alongside ResourceNotFoundException. Recorded behavior: a StartDeliveryStreamEncryption call with KeyType=CUSTOMER_MANAGED_CMK and no KeyARN is refused with HTTP 400 and __type InvalidArgumentException, message example 'KeyARN has to be specified when KeyType is CUSTOMER_MANAGED_CMK' — i.e. the argument dependency is evaluated and reported as an argument fault.
Emulator: HTTP 400 with __type ResourceNotFoundException and a message reporting the target Firehose stream as not found, even though the fixture created that stream earlier in the same run (state_reset_failures was 0 for this replay, so the divergence is not a leaked-state artifact).
Why: the client receives a resource-identity error for a request whose fault is an argument, so retry/remediation logic takes the wrong branch (re-create the stream rather than supply a KeyARN), and the KeyARN-required rule is never exercised. Hypothesis: either the encryption handler looks the stream up before validating the encryption config, or the stream created earlier in the fixture is not visible to this operation's state lookup; either way the modeled argument error is unreachable.
Fix: the StartDeliveryStreamEncryption handler must validate DeliveryStreamEncryptionConfigurationInput (KeyARN required when KeyType=CUSTOMER_MANAGED_CMK) and raise InvalidArgumentException for the dependency violation; if the stream lookup is also failing for a stream this emulator created, the delivery-stream state lookup used by this handler must be reconciled with the one CreateDeliveryStream writes.
Traces: firehose-start-delivery-stream-encryption-key-validation#task#1.

## Evidence
Cause 1 — fixture firehose-create-delivery-stream-argument-validation, step task#1, operation CreateDeliveryStream. Expected HTTP 400 with $.__type = 'InvalidArgumentException' and a $.message naming the unassumable role. Actual HTTP 200, $.__type missing, $.message missing. Replay detail: "status: expected 400, got 200; $.__type: missing (expected 'InvalidArgumentException', got None); $.message: missing".

Cause 2 — fixture firehose-create-delivery-stream-malformed-role-arn, step task#1, operation CreateDeliveryStream. Expected HTTP 400 with $.__type = 'InvalidParameterValueException'. Actual HTTP 400 with $.__type = 'ValidationException'. Message (example only): expected 'Invalid role ARN.', actual "1 validation error detected: Value at 's3DestinationConfiguration.roleARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:.*:iam::\d{12}:role/[a-zA-Z_0-9+=,.@\-_/]+".

Cause 3 — fixture firehose-start-delivery-stream-encryption-key-validation, step task#1, operation StartDeliveryStreamEncryption. Expected HTTP 400 with $.__type = 'InvalidArgumentException' (message example 'KeyARN has to be specified when KeyType is CUSTOMER_MANAGED_CMK'). Actual HTTP 400 with $.__type = 'ResourceNotFoundException' and a message reporting the Firehose stream as not found. Both statuses are 400; the error code is the divergence.

Replay context: trace set simparity-wire-aws-v1, 14 fixtures replayed, 32 steps, 22 passed, 10 mismatched, score 68.8%, state_reset_failures 0, emulator_up true.

## Steps to Reproduce
1. Clone the repo and boot the emulator from this checkout, then replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture firehose-create-delivery-stream-argument-validation step task#1 — CreateDeliveryStream with a destination RoleARN naming an IAM role that cannot be assumed. Observe HTTP 200 with a success body instead of HTTP 400 / __type InvalidArgumentException.
3. Cause 2: replay fixture firehose-create-delivery-stream-malformed-role-arn step task#1 — CreateDeliveryStream with a RoleARN that violates the IAM role ARN pattern. Read $.__type: observe ValidationException instead of InvalidParameterValueException (HTTP 400 in both).
4. Cause 3: replay fixture firehose-start-delivery-stream-encryption-key-validation step task#1 — StartDeliveryStreamEncryption against the stream the fixture created, with DeliveryStreamEncryptionConfigurationInput.KeyType=CUSTOMER_MANAGED_CMK and no KeyARN. Read $.__type: observe ResourceNotFoundException instead of InvalidArgumentException.
5. All three fixture steps named above must be re-replayed after a fix; each is a separate root cause.
