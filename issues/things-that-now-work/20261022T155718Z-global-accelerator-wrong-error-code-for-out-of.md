Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: globalaccelerator.traffic-dial-rejects-percentage-above-hundred globalaccelerator.accelerator-delete-rejects-enabled-accelerator
Issue-Shape-Id: com.amazonaws.globalaccelerator#GlobalAccelerator_V20180706
Issue-Key: globalaccelerator.traffic-dial-rejects-percentage-above-hundred|run#7|AssertionError|assert code == "InvalidArgumentException", (
Issue-Key: globalaccelerator.accelerator-delete-rejects-enabled-accelerator|run#9|AssertionError|assert message == expected, (
Issue-Commit: 7b5968e5380062291ec1b05b736535203332753c

# Global Accelerator: wrong error code for out-of-range TrafficDialPercentage, and AcceleratorNotDisabledException message does not name the accelerator

> _Note: this issue was identified against the code base at commit `7b5968e5380062291ec1b05b736535203332753c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEndpointGroup

## Description
Two distinct root causes in the Global Accelerator emulator, both grounded in failing behavioral checks. The CreateEndpointGroup stub issue already filed by the platform (UNIMPLEMENTED CreateEndpointGroup, services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/src/ops/create_endpoint_group.rs) covers only the not-implemented path for global-accelerator.endpoint-group-rejects-duplicate-region; Cause 1 below is a different defect: the request IS rejected, but with an error code that is not in the operation's modeled error set.

Cause 1 of 2: CreateEndpointGroup / UpdateEndpointGroup reject TrafficDialPercentage > 100 with the wrong error code.
AWS: Smithy (com-amazonaws-globalaccelerator-globalaccelerator-v20180706) models CreateEndpointGroup.TrafficDialPercentage -> TrafficDialPercentage @range{min:0,max:100}, and the operation's modeled client errors are AcceleratorNotFoundException(404), AccessDeniedException(403), EndpointGroupAlreadyExistsException(400), InvalidArgumentException(400), LimitExceededException(403), ListenerNotFoundException(404). UpdateEndpointGroup models the same @range{0,100} and the client errors AccessDeniedException(403), EndpointGroupNotFoundException(404), InvalidArgumentException(400), LimitExceededException(403). There is no ValidationException anywhere in either operation's error set. The recorded AWS behavior for a dial of 150 is HTTP 400 with code InvalidArgumentException; the recorded message is an example of the constraint-report form: "1 validation error detected: Value '150.0' at 'trafficRetentionPercentage' failed to satisfy constraint: Member must have value less than or equal to 100" (a message built from the request: the rejected value, the member, and the modeled upper bound).
Emulator: CreateEndpointGroup with TrafficDialPercentage=150.0 returns error code 'ValidationException' instead of 'InvalidArgumentException'. Because the check fails at the first assertion, the UpdateEndpointGroup leg of the same range rule (which must reject 150.0 with InvalidArgumentException and leave the stored dial unchanged) was never reached and is unverified — it must be fixed on both operations.
Why: a client catching the modeled InvalidArgumentException (as generated SDKs and retry/validation wrappers do, dispatching on the modeled error union) never matches 'ValidationException'. The error surfaces as an unmodeled/unknown service error, so input-validation handling degrades to a generic failure and any code branching on InvalidArgumentException is dead against the emulator.
Fix: the range validation of CreateEndpointGroup and UpdateEndpointGroup input (the operations' validation/error-mapping layer) must raise the service's modeled InvalidArgumentException with HTTP 400 for a TrafficDialPercentage outside [0,100], not a generic ValidationException. The message should be generated from the request (rejected value, member name, bound) rather than hard-coded.
Traces: global-accelerator.traffic-dial-rejects-percentage-above-hundred

Cause 2 of 2: DeleteAccelerator's AcceleratorNotDisabledException message is a fixed string that does not identify the accelerator.
AWS: Smithy models DeleteAccelerator with error AcceleratorNotDisabledException (client, httpError 400) — doc "The accelerator that you specified could not be disabled." The recorded AWS response for deleting an enabled accelerator is HTTP 400 / AcceleratorNotDisabledException with a message built from the request's AcceleratorArn: "Please disable accelerator <AcceleratorArn> before deleting it." — i.e. the rule is a template parameterised by the ARN the caller passed, which the emulator can reproduce exactly.
Emulator: the error code is correct, but the message is the static string 'the accelerator must be disabled before it can be deleted', which contains no accelerator ARN at all, so no per-request value is interpolated. Because the check asserts the message immediately after the code, the remainder of the relationship it grades — that the refused delete is a no-op (accelerator still present and Enabled=true), that UpdateAccelerator(Enabled=false) is not synchronous so DeleteAccelerator keeps returning AcceleratorNotDisabledException for a while, and that DescribeAccelerator afterwards returns AcceleratorNotFoundException with "Accelerator <arn> not found." — was never exercised and is unverified.
Why: callers and operators that parse the refusal to learn which accelerator is blocking a teardown (common in multi-accelerator cleanup loops) get no identifier from the emulator, so the diagnostic cannot be attributed to a resource; log/assert comparisons against AWS's documented wording also fail.
Fix: the DeleteAccelerator handler's AcceleratorNotDisabledException construction (its error-formatting path) must build the message from the request's AcceleratorArn instead of returning a constant.
Traces: global-accelerator.accelerator-delete-rejects-enabled-accelerator

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), global-accelerator 0/6 passed.

Cause 1 — [mismatch] global-accelerator.traffic-dial-rejects-percentage-above-hundred:
  AssertionError: CreateEndpointGroup with TrafficDialPercentage=150 returned 'ValidationException', expected 'InvalidArgumentException'
  Expected (recorded AWS): HTTP 400, Code=InvalidArgumentException, Message (example) "1 validation error detected: Value '150.0' at 'trafficRetentionPercentage' failed to satisfy constraint: Member must have value less than or equal to 100"
  Actual (emulator): Code=ValidationException
  Model: CreateEndpointGroup/UpdateEndpointGroup TrafficDialPercentage @range{min:0,max:100}; error set contains InvalidArgumentException(400) and no ValidationException.

Cause 2 — [mismatch] global-accelerator.accelerator-delete-rejects-enabled-accelerator:
  AssertionError: DeleteAccelerator on an enabled accelerator returned message 'the accelerator must be disabled before it can be deleted', expected 'Please disable accelerator arn:aws:globalaccelerator::000000000000:accelerator/<id> before deleting it.'
  Expected (recorded AWS): HTTP 400, Code=AcceleratorNotDisabledException (matched by the emulator), message templated on the request's AcceleratorArn.
  Actual (emulator): same code, constant message with no ARN interpolated.
  Model: DeleteAccelerator errors include AcceleratorNotDisabledException (client, httpError 400).

## Steps to Reproduce
Cause 1:
1. globalaccelerator CreateAccelerator(Name=<unique>, IpAddressType=IPV4, Enabled=false, IdempotencyToken=<uuid>).
2. CreateListener(AcceleratorArn=<arn>, PortRanges=[{FromPort:80,ToPort:80}], Protocol=TCP, IdempotencyToken=<uuid>).
3. CreateEndpointGroup(ListenerArn=<listener>, EndpointGroupRegion=<region>, TrafficDialPercentage=150.0, IdempotencyToken=<uuid>) -> observe Error.Code is 'ValidationException'; AWS returns HTTP 400 InvalidArgumentException.
4. (same rule, unverified leg) create the group with TrafficDialPercentage=100.0, then UpdateEndpointGroup(EndpointGroupArn=<group>, TrafficDialPercentage=150.0) -> must also be HTTP 400 InvalidArgumentException with the stored dial unchanged at 100.0.

Cause 2:
1. globalaccelerator CreateAccelerator(Name=<unique>, IpAddressType=IPV4, Enabled=true, IdempotencyToken=<uuid>).
2. DeleteAccelerator(AcceleratorArn=<arn>) -> HTTP 400 AcceleratorNotDisabledException; inspect Error.Message: the emulator returns a constant string with no ARN, AWS returns "Please disable accelerator <arn> before deleting it."
3. DescribeAccelerator(AcceleratorArn=<arn>) must still show Enabled=true (refused delete is a no-op), then UpdateAccelerator(Enabled=false) and retry DeleteAccelerator until it succeeds.
