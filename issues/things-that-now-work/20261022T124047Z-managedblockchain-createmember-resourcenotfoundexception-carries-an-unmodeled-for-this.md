Issue-Source: trace-sweep-divergence
Issue-Trace: managedblockchain-create-member-invitation-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: managedblockchain-create-member-invitation-not-found
Issue-Shape-Id: com.amazonaws.managedblockchain#TaigaWebService
Issue-Key: managedblockchain|CreateMember|4xx->4xx|$.ResourceName
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# managedblockchain CreateMember ResourceNotFoundException carries an unmodeled-for-this-case ResourceName ARN

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMember

## Description
One cause.

Cause 1 of 1: CreateMember against an invitation id that does not exist returns the right status and error shape, but the emulator adds a `ResourceName` member to the error body that AWS does not emit for this case, and the value it puts there names the wrong resource (the network, which exists) instead of the missing invitation.

AWS: Smithy (services/service-com-amazonaws-managedblockchain-taigawebservice) — CreateMember POST /networks/{NetworkId}/members, errors include ResourceNotFoundException with httpError 404; ResourceNotFoundException members are `Message` (String) and `ResourceName` (ArnString, @length 1..1011, @pattern ^arn:.+:.+:.+:.+:.+$), both OPTIONAL. The model does not state when ResourceName is populated, so for this case it is recorded behavior only: the recorded AWS response is HTTP 404 ResourceNotFoundException with NO `ResourceName` member present at all (the invitation that could not be found is not an ARN-addressable resource, so AWS leaves the member off).

Emulator: replay of fixture managedblockchain-create-member-invitation-not-found step task#1 returns HTTP 404 (status matches) but the body carries `$.ResourceName` populated with the enclosing network's ARN (recorded example: 'arn:aws:managedblockchain:us-east-1:000000000000:networks/n-000000000000000000000000000000'). Replay verdict: mismatch, detail `$.ResourceName: type_mismatch (expected None, got 'arn:aws:...networks/n-...')`.

Why: a client that reads ResourceNotFoundException.ResourceName to decide which resource to re-create, re-resolve or report sees an ARN for a network that exists and is healthy, and will conclude the network is missing rather than the invitation. SDK-level equality/round-trip assertions on the deserialized error also fail because a member AWS leaves absent is present. The generally required behavior: for a CreateMember failure caused by an unknown InvitationId, emit 404 ResourceNotFoundException with ResourceName ABSENT — do not substitute the path-bound NetworkId's ARN.

Fix: the CreateMember handler's invitation lookup / its ResourceNotFoundException construction in the managedblockchain service crate — the not-found branch for the invitation should build the error without a ResourceName rather than defaulting it to the ARN of the request's path resource.

Traces: managedblockchain-create-member-invitation-not-found#task#1 (cluster of 1; divergence_key service=managedblockchain|CreateMember|status=404->404|keys=$.ResourceName).

## Evidence
Replay cluster (verbatim): {"fixture_id": "managedblockchain-create-member-invitation-not-found", "step_key": "task#1", "service": "managedblockchain", "operation": "CreateMember", "verdict": "mismatch", "expected_status": 404, "actual_status": 404, "detail": "$.ResourceName: type_mismatch (expected None, got 'arn:aws:managedblockchain:us-east-1:000000000000:networks/n-000000000000000000000000000000')", "affected": ["managedblockchain-create-member-invitation-not-found#task#1"], "affected_count": 1, "stub_path": "", "owned_by": []}

Expected (recorded AWS): HTTP 404, ResourceNotFoundException, no ResourceName member in the body.
Actual (emulator): HTTP 404, ResourceNotFoundException, ResourceName = the request network's ARN (example value above).
Model citation: ResourceNotFoundException { Message: String, ResourceName: ArnString @length(1,1011) @pattern(^arn:.+:.+:.+:.+:.+$) } — both members optional; CreateMember binds ResourceNotFoundException at httpError 404.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from it.
2. Replay wire fixture 'managedblockchain-create-member-invitation-not-found', step 'task#1': POST /networks/{NetworkId}/members with a valid existing NetworkId, a valid ClientRequestToken and MemberConfiguration, and an InvitationId that does not exist.
3. Observe the 404 ResourceNotFoundException response body.
4. Expected: no `ResourceName` member present. Actual: `ResourceName` present and set to the ARN of the network from the request path.
