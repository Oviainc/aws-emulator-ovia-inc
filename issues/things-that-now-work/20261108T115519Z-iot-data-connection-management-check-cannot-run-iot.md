Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-data.connected-client-connection-apis-reflect-live-session
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data|AttachPolicy|NotImplemented
Issue-Commit: 274c242a07c2045a47799c7436640bb31a4ba660

# iot-data connection-management check cannot run: iot AttachPolicy answers NotImplemented, so no mTLS client can ever be authorized to connect

> _Note: this issue was identified against the code base at commit `274c242a07c2045a47799c7436640bb31a4ba660`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachPolicy

## Description
Cause 1 of 1: the one iot-data check in this suite that exercises the live-session connection APIs (GetConnection / ListSubscriptions / SendDirectMessage) never reaches them, because the prerequisite authorization call is refused.

AWS: Smithy (com.amazonaws.iot#AttachPolicy) models AttachPolicy as PUT /target-policies/{policyName} with input members policyName (@required, @httpLabel, @length min 1 max 128, @pattern ^[\w+=,.@-]+$) and target (@required, PolicyTarget), an empty output shape (Unit) and success code 200. Its only modeled errors are InternalFailureException (500), InvalidRequestException (400), LimitExceededException (410), ResourceNotFoundException (404), ServiceUnavailableException (503), ThrottlingException (400) and UnauthorizedException (401). For an existing policy and an existing, ACTIVE certificate ARN as target, AWS returns 200 with an empty body and the policy becomes attached (subsequently visible via ListAttachedPolicies / ListTargetsForPolicy), which is what makes the certificate usable for an authorized MQTT connect.

Emulator: the call is refused with HTTP 501 and error code `NotImplemented`, message `operation AttachPolicy is scaffolded but not implemented in this emulator` (fixed text; the variable part is the operation name taken from the request's routed operation). Observed as botocore `ClientError: An error occurred (NotImplemented) when calling the AttachPolicy operation: operation AttachPolicy is scaffolded but not implemented in this emulator`. `NotImplemented` is not a member of AttachPolicy's modeled error set at all, and 501 is not one of its modeled statuses.

Why: a client that follows the normal IoT device-onboarding sequence — CreateKeysAndCertificate, CreatePolicy, AttachPolicy, DescribeEndpoint, MQTT connect — is stopped at step 3 by an error code it cannot interpret or retry (it is outside the modeled errors, so generated SDK error handling falls through to a generic client error). Because the certificate is never authorized, nothing downstream can be reached: the whole iot-data connection-management surface (GetConnection with includeSocketInformation, ListSubscriptions, SendDirectMessage, and the connected=false transition after disconnect) is unreachable and therefore unverified in this emulator.

Fix: implement the AttachPolicy handler and its state in the iot service crate — `services/service-com-amazonaws-iot-awsiotservice/src/ops/attach_policy.rs` — so it validates policyName/target, returns 200 with an empty body, records the policy→target attachment in the iot policy state that ListAttachedPolicies / ListTargetsForPolicy / DetachPolicy read, and returns ResourceNotFoundException (404) for an unknown policy or target rather than a non-modeled status. Note for the fixer: two OPEN issues already describe this same stub from the iot side — `20261106T201842Z-iot-attachpolicy-stub.md` and `20261108T011133Z-ec2-iot-iot-attachpolicy-answers-501-notimplemented-so-an-aws.md`; this issue exists because the iot-data check below is not covered by either one's replay key, so it must be re-replayed before this is closed. Once AttachPolicy answers, re-replaying the check may expose further iot-data divergences in GetConnection / ListSubscriptions / SendDirectMessage, which are still unexercised.

Traces: iot-data.connected-client-connection-apis-reflect-live-session.

## Evidence
Failing check: iot-data.connected-client-connection-apis-reflect-live-session (verdict: error).

Expected (AWS / Smithy com.amazonaws.iot#AttachPolicy): HTTP 200, empty body, policy attached to the certificate ARN; the check then proceeds to DescribeEndpoint, MQTT connect over mTLS, and asserts GetConnection(clientId, includeSocketInformation=True) returns connected=true with a non-empty sourceIp and sourcePort > 0, ListSubscriptions names the subscribed topic filter, SendDirectMessage returns a traceId and the bytes arrive on the connected session, and GetConnection reports connected=false after disconnect.

Actual (emulator): the check aborts at `iot.attach_policy(policyName=<policy>, target=<certificateArn>)` with
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AttachPolicy operation: operation AttachPolicy is scaffolded but not implemented in this emulator
HTTP status 501; error code `NotImplemented` — not in AttachPolicy's modeled error set (InternalFailureException, InvalidRequestException, LimitExceededException, ResourceNotFoundException, ServiceUnavailableException, ThrottlingException, UnauthorizedException). Fixed text of the emulator message: "operation <OperationName> is scaffolded but not implemented in this emulator"; the only variable part is the operation name. None of the iot-data assertions were ever evaluated.

## Steps to Reproduce
1. iot.create_keys_and_certificate(setAsActive=True) -> note certificateArn.
2. iot.create_policy(policyName=<name>, policyDocument='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iot:*","Resource":"*"}]}').
3. iot.attach_policy(policyName=<name>, target=<certificateArn>)  -> emulator answers HTTP 501 NotImplemented instead of 200 with an empty body.
4. (unreachable today) iot.describe_endpoint(endpointType='iot:Data-ATS'), MQTT connect on port 8883 with the certificate/key, subscribe, then iot_data.get_connection(clientId=..., includeSocketInformation=True), iot_data.list_subscriptions(clientId=...), iot_data.send_direct_message(clientId=..., topic=..., payload=b'...').
Replay: behavioral check iot-data.connected-client-connection-apis-reflect-live-session.
