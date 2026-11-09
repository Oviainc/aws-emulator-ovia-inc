Issue-Source: trace-sweep-divergence
Issue-Trace: direct-connect-gateway-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: direct-connect-gateway-lifecycle
Issue-Shape-Id: com.amazonaws.directconnect#OvertureService
Issue-Key: directconnect|CreateDirectConnectGateway|2xx->2xx|$.directConnectGateway.directConnectGatewayArn
Issue-Key: directconnect|CreateDirectConnectGateway|2xx->2xx|$.directConnectGateway.directConnectGatewayState
Issue-Key: directconnect|CreateDirectConnectGateway|2xx->2xx|$.directConnectGateway.directConnectGatewayType
Issue-Key: directconnect|CreateDirectConnectGateway|2xx->2xx|$.directConnectGateway.tags
Issue-Key: directconnect|DescribeDirectConnectGateways|2xx->2xx|$.directConnectGateways[*].directConnectGatewayArn
Issue-Key: directconnect|DescribeDirectConnectGateways|2xx->2xx|$.directConnectGateways[*].directConnectGatewayState
Issue-Key: directconnect|DescribeDirectConnectGateways|2xx->2xx|$.directConnectGateways[*].directConnectGatewayType
Issue-Key: directconnect|DescribeDirectConnectGateways|2xx->2xx|$.directConnectGateways[*].tags
Issue-Key: directconnect|DescribeDirectConnectGateways|2xx->2xx|$.directConnectGateways[*].directConnectGatewayName
Issue-Commit: eaa123f65a60b02b9ee1461dec055a780335fce9

# directconnect: Direct Connect gateway records omit ARN, type and tags, stay pending, and lose renames

> _Note: this issue was identified against the code base at commit `eaa123f65a60b02b9ee1461dec055a780335fce9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDirectConnectGateway

## Description
The emulator's Direct Connect gateway state record is incomplete and inert: it never populates the ARN/type/tags members AWS returns, it never advances the gateway out of 'pending', and an UpdateDirectConnectGateway rename is not visible to subsequent reads. This bundle has 3 causes; ALL THREE must be fixed before it is closed.

Cause 1 of 3: CreateDirectConnectGateway response member set and initial state (fixture direct-connect-gateway-lifecycle, step task#1).
AWS: Smithy: CreateDirectConnectGateway -> CreateDirectConnectGatewayResult.directConnectGateway : DirectConnectGateway; DirectConnectGatewayState is an enum with values [pending, available, deleting, deleted]. The on-disk model's DirectConnectGateway structure does not declare directConnectGatewayArn or directConnectGatewayType, so for those two members this is recorded behavior only: the recording shows AWS returning, in the 200 CreateDirectConnectGateway body, a directConnectGatewayArn, a directConnectGatewayType whose value is 'Default', a tags list (empty when no tags were supplied), and directConnectGatewayState already 'available'. The ARN's fixed template is the directconnect gateway ARN form 'arn:aws:directconnect::<ownerAccount>:dx-gateway/<directConnectGatewayId-without-prefix>'; the account id and the gateway identifier are values taken from the emulator's state and are NOT part of the requirement (the recorded 'arn:aws:directconnect::000000000000:dx-gateway/a8c7f732-...-000000000001' is one run's fill-in).
Emulator: returns 200 but the gateway object has directConnectGatewayArn missing, directConnectGatewayType missing, tags missing (absent rather than an empty list) and directConnectGatewayState 'pending'.
Why: a client that reads directConnectGatewayArn to tag, share or reference the gateway gets null; a client that keys on directConnectGatewayType cannot tell a Default gateway from any other; and a client (or CloudFormation/waiter) that polls until the state is 'available' never observes that state and hangs or times out, since nothing in the emulator ever advances the gateway.
Fix: CreateDirectConnectGateway's handler and the gateway state record it writes (populate the ARN from the account+gateway id, set the gateway type, default tags to an empty list) plus the result serializer; the state machine must land the gateway in 'available' the way the recording does rather than leaving it at 'pending'.
Traces: direct-connect-gateway-lifecycle#task#1.

Cause 2 of 3: DescribeDirectConnectGateways returns the same incomplete record (fixture direct-connect-gateway-lifecycle, step task#2).
AWS: Smithy: DescribeDirectConnectGateways -> DescribeDirectConnectGatewaysResult.directConnectGateways : DirectConnectGatewayList of DirectConnectGateway, i.e. exactly the same shape Create returns. The recording shows each listed gateway carrying directConnectGatewayArn, directConnectGatewayType 'Default', tags (empty list when untagged) and directConnectGatewayState 'available'.
Emulator: the listed element has directConnectGatewayArn missing, directConnectGatewayType missing, tags missing, and directConnectGatewayState 'pending'.
Why: the read path is where clients discover gateways they did not create in-process, so the same nulls and the stuck 'pending' break describe-based reconciliation and any wait-for-available loop even when Create is bypassed.
Fix: the shared gateway-to-wire projection used by DescribeDirectConnectGateways' serializer (it must emit the same member set as Create) and the gateway state record behind it.
Traces: direct-connect-gateway-lifecycle#task#2.

Cause 3 of 3: a gateway rename is not reflected by DescribeDirectConnectGateways (fixture direct-connect-gateway-lifecycle, steps task#8 and task#10).
AWS: Smithy: UpdateDirectConnectGateway has required input members directConnectGatewayId and newDirectConnectGatewayName and returns the updated DirectConnectGateway. The recording shows that after a successful rename, DescribeDirectConnectGateways reports directConnectGatewayName equal to the new name supplied in newDirectConnectGatewayName (the recorded 'sim2real-probe-dx-gw-renamed-...' suffix is that run's generated value, not a requirement).
Emulator: DescribeDirectConnectGateways keeps returning the ORIGINAL creation name after the rename, i.e. the update is accepted but never persisted to the record the read path serves; the same element also still omits ARN/type/tags and reports 'pending'.
Why: rename is observable configuration. A client that renames and then reads back to confirm sees the stale name, so idempotent 'ensure named X' reconciliation loops rename forever and any assertion on the new name fails.
Fix: UpdateDirectConnectGateway's handler must mutate the stored gateway record's name (the state write), not just shape a response.
Traces: direct-connect-gateway-lifecycle#task#8, direct-connect-gateway-lifecycle#task#10.

Unrelated open issues on this service that do NOT cover these: the DescribeLocations availableProviders issue (catalog seed data) and the CreateDirectConnectGatewayAssociation stub issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 - direct-connect-gateway-lifecycle#task#1, CreateDirectConnectGateway, expected 200 / actual 200:
  $.directConnectGateway.directConnectGatewayArn: missing (expected an ARN, got None)
  $.directConnectGateway.directConnectGatewayState: value_mismatch (expected 'available', got 'pending')
  $.directConnectGateway.directConnectGatewayType: missing (expected 'Default', got None)
  $.directConnectGateway.tags: missing (expected [], got None)

Cause 2 - direct-connect-gateway-lifecycle#task#2, DescribeDirectConnectGateways, expected 200 / actual 200:
  $.directConnectGateways[0].directConnectGatewayArn: missing (expected 'arn:aws:directconnect::<account>:dx-gateway/<id>', got None)
  $.directConnectGateways[0].directConnectGatewayState: value_mismatch (expected 'available', got 'pending')
  $.directConnectGateways[0].directConnectGatewayType: missing (expected 'Default', got None)
  $.directConnectGateways[0].tags: missing (expected [], got None)

Cause 3 - direct-connect-gateway-lifecycle#task#8 (cluster also covers task#10), DescribeDirectConnectGateways, expected 200 / actual 200:
  $.directConnectGateways[0].directConnectGatewayName: value_mismatch (expected the renamed value '<name>-renamed-<suffix>', got the original creation name)
  plus the same directConnectGatewayArn / directConnectGatewayType / tags missing and directConnectGatewayState 'pending' instead of 'available'

## Steps to Reproduce
1. Replay fixture direct-connect-gateway-lifecycle against the emulator.
2. Step task#1: CreateDirectConnectGateway with a gateway name (and an amazonSideAsn). Inspect $.directConnectGateway - directConnectGatewayArn, directConnectGatewayType and tags are absent and directConnectGatewayState is 'pending' instead of 'available'.
3. Step task#2: DescribeDirectConnectGateways and inspect $.directConnectGateways[0] - same four divergences.
4. Call UpdateDirectConnectGateway with that directConnectGatewayId and a new newDirectConnectGatewayName, then step task#8 / task#10: DescribeDirectConnectGateways - $.directConnectGateways[0].directConnectGatewayName is still the original name.
