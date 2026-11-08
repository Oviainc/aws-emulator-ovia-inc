Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive
Issue-Key: ec2__iot|AttachPolicy|NotImplemented
Issue-Commit: f4d58d8ba640bd38bd7af13aafdf9baa78485a99

# IoT AttachPolicy answers 501 NotImplemented, so an aws:SourceVpce-conditioned thing policy can never be attached or exercised

> _Note: this issue was identified against the code base at commit `f4d58d8ba640bd38bd7af13aafdf9baa78485a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachPolicy

## Description
Cause 1 of 1: the IoT AttachPolicy operation is an unimplemented scaffold stub.

AWS: recorded behavior only for the status. IoT's AttachPolicy attaches a named policy to a target (a certificate ARN or a Cognito identity) and answers HTTP 200 with an empty body; thereafter the policy's statements — including a `Condition` on `aws:SourceVpce` — govern the thing's data-plane Connect/Publish/Subscribe/Receive. Its modelled refusals are the IoT service errors (ResourceNotFoundException for an unknown policy or target, InvalidRequestException, LimitExceededException, ThrottlingException, UnauthorizedException, ServiceUnavailableException, InternalFailureException); `NotImplemented` is not among them and is not an IoT error code.

Emulator: answers the call with error code `NotImplemented` and the message `operation AttachPolicy is scaffolded but not implemented in this emulator` — an emulator-authored string that names the emulator rather than any AWS condition.

Why: no IoT authorization can be established at all. A client cannot bind a policy to a certificate, so every downstream data-plane assertion is unreachable: a policy pinned to a nonexistent `aws:SourceVpce` cannot be shown to be stored as readily as a live one, and the pinned-refuses / unpinned-succeeds distinction cannot be observed. The refusal also leaks the emulator's own vocabulary into the error channel, which no AWS client can classify.

Fix: implement the AttachPolicy handler in the IoT service crate (services/service-com-amazonaws-iot-awsiotservice/src/ops/attach_policy.rs) — record the policy-to-target attachment in IoT state, answer 200 with an empty body, and raise ResourceNotFoundException for an unknown policy name or target instead of NotImplemented.

Traces: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive.

## Evidence
Expected: iot:AttachPolicy(policyName=<name>, target=<certificate ARN>) -> HTTP 200, empty body; the attachment is then visible to the data plane.
Actual: error code 'NotImplemented', message 'operation AttachPolicy is scaffolded but not implemented in this emulator'.
Replay: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AttachPolicy operation.

## Steps to Reproduce
1. Create an IoT thing, a certificate and an IoT policy whose single statement allows iot:Connect/Publish/Subscribe/Receive with a Condition StringEquals aws:SourceVpce.
2. Call iot:AttachPolicy(policyName=<that policy>, target=<certificate ARN>).
3. Observe NotImplemented instead of a 200.
4. Re-replay check ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive to confirm.
