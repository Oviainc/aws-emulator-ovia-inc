Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: managedblockchain.create-network-invalid-framework-version-rejected managedblockchain.get-network-not-found managedblockchain.list-nodes-nonexistent-member-rejected managedblockchain.list-proposals-nonexistent-network-not-found managedblockchain.create-member-invitation-not-found
Issue-Shape-Id: com.amazonaws.managedblockchain#TaigaWebService
Issue-Key: managedblockchain.create-network-invalid-framework-version-rejected|_error#3|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: managedblockchain|GetNetwork|ResourceNotFoundException
Issue-Key: managedblockchain.list-nodes-nonexistent-member-rejected|run#6|AssertionError|assert message == (
Issue-Key: managedblockchain.list-proposals-nonexistent-network-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: managedblockchain.create-member-invitation-not-found|run#6|AssertionError|assert code == "ResourceNotFoundException", code
Issue-Commit: cb8455c4b8512950002591de10a2fc79dcf21bbf

# managedblockchain control plane: unvalidated CreateNetwork framework version, missing AWS-owned public networks, wrong ListNodes rejection class, and CreateMember/ListProposals answering NotImplemented (5 root causes)

> _Note: this issue was identified against the code base at commit `cb8455c4b8512950002591de10a2fc79dcf21bbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNetwork

## Description
Five distinct root causes in the managedblockchain (AMB / TaigaWebService) control plane, found by replaying the behavioral suite against the booted emulator (0/23 checks passed for this service). All five are net-new: the only open managedblockchain issue, '20261022T020633Z-managedblockchain-error-responses-do-not-match-modeled-exception.md', covers the error-PAYLOAD shape of GetNetwork/GetNode/ListMembers (non-ARN ResourceName, missing Message, ValidationException code) and none of the five causes below; the stub issues auto-filed for this suite cover CreateAccessor, UpdateNode, ListMembers, CreateProposal and ListInvitations and likewise cover none of these. A fixer must close all FIVE causes before submitting.

Cause 1 of 5: CreateNetwork does not validate the Framework/FrameworkVersion combination and provisions a network for an unsupported version.
AWS: Smithy (com-amazonaws-managedblockchain-taigawebservice): CreateNetwork POST /networks, required members Framework (enum Framework) and FrameworkVersion (FrameworkVersionString, @length 1..8); modeled client errors include InvalidRequestException (errorKind client, httpError 400). The recorded behavior is that a Framework/FrameworkVersion pair AWS does not support (HYPERLEDGER_FABRIC with FrameworkVersion '99.9') is refused with HTTP 400 InvalidRequestException BEFORE anything is provisioned, and the attempted network name never appears in ListNodes/ListNetworks. The message is built from the request and names the rejected version and framework (recorded example: 'Invalid version 99.9 requested for framework HYPERLEDGER_FABRIC.'); the requirement is status 400 + code InvalidRequestException + a Message naming the rejected FrameworkVersion and Framework, not that exact text.
Emulator: accepts the request and answers HTTP 200 with a freshly minted NetworkId and MemberId (replay: {'NetworkId': 'n-IEWA...', 'MemberId': 'm-ML4R...'}), i.e. FrameworkVersion is never checked against the versions supported for the given Framework and a network plus its first member are created.
Why: this is an overly-permissive accept. A client that depends on AWS refusing an unsupported framework version gets a 200 and a network id for a network that cannot exist on AWS, then leaks that network/member (and any dependent nodes) into subsequent state; negative-path tests and provisioning guards that branch on InvalidRequestException never fire.
Fix: CreateNetwork input validation in the operation's handler (services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/create_network.rs) — validate FrameworkVersion against the versions supported for the requested Framework and return InvalidRequestException (400) before any network/member state is written.
Traces: managedblockchain.create-network-invalid-framework-version-rejected.

Cause 2 of 5: GetNetwork does not serve AWS's standing, AWS-owned public networks, so the positive control fails.
AWS: Smithy: GetNetwork GET /networks/{NetworkId} -> GetNetworkOutput.Network; ResourceNotFoundException (client, 404) is the modeled miss. Recorded behavior only: AMB Access exposes standing AWS-owned public blockchain networks that every account can read without creating anything — GetNetwork(NetworkId='n-ethereum-mainnet') returns 200 with Network.Id == 'n-ethereum-mainnet' and Network.Framework == 'ETHEREUM'. These are not account-specific resources; they are the same ids in every account.
Emulator: answers 404 ResourceNotFoundException for 'n-ethereum-mainnet' (replay: 'An error occurred (ResourceNotFoundException) when calling the GetNetwork operation: network n-ethereum-mainnet does not exist on this account'), i.e. the service's state is seeded with no AWS-owned public networks at all.
Why: 'this id is unknown' becomes indistinguishable from 'GetNetwork always fails'. Any client that reads the public Ethereum networks (the normal path for creating ETHEREUM accessors/nodes, and the only way to validate a NetworkType) sees every public network as missing, and GetNetwork's not-found path cannot be told apart from a broken read path.
Why this is not a message-only finding: the divergence is the status and code (200 + Network vs 404 ResourceNotFoundException), not wording.
Fix: the service's initial state / seeded resources (the managedblockchain state module) plus the GetNetwork handler's lookup — seed the AWS-owned public networks (at minimum n-ethereum-mainnet with Framework ETHEREUM) so GetNetwork resolves them for any caller.
Traces: managedblockchain.get-network-not-found.

Cause 3 of 5: ListNodes returns the wrong error CLASS for an unresolvable network/member — ResourceNotFoundException instead of InvalidRequestException — and does not enforce that a MemberId is required.
AWS: Smithy: ListNodes GET /networks/{NetworkId}/nodes, MemberId bound as httpQuery 'memberId' (optional in the model); its modeled client errors are AccessDeniedException(403), InvalidRequestException(400) and ThrottlingException(429) — ResourceNotFoundException is NOT among ListNodes's modeled errors at all. Recorded behavior: with a nonexistent NetworkId and a nonexistent MemberId the call fails 400 InvalidRequestException with a request-derived Message naming the member and network (example: 'Member with Id m-000... not found in network n-000....'), and with MemberId omitted it also fails 400 InvalidRequestException with a Message saying a MemberId is required for that network (example: 'Member Id is required for network n-000....') — never an empty page.
Emulator: answers ResourceNotFoundException (404) for the member-attached call (replay message: 'network n-000000000000000000000000000000 does not exist on this account'), so the request never reaches the member-resolution / required-MemberId validation that produces AWS's 400.
Why: clients dispatch on the error code, and the SDK error union for ListNodes has no ResourceNotFoundException variant — a typed catch on InvalidRequestException never fires and the failure surfaces as an unmodeled/unknown error. Retry and branching logic that treats 404 as 'resource gone' versus 400 as 'bad request' takes the wrong branch, and the 'MemberId is required' rule is not enforced at all.
Fix: ListNodes handler validation order in services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/list_nodes.rs — classify both an unresolvable member-in-network and an absent MemberId as InvalidRequestException (400) with a Message built from the request, instead of emitting the shared network-not-found 404.
Traces: managedblockchain.list-nodes-nonexistent-member-rejected.

Cause 4 of 5: ListProposals answers NotImplemented instead of its modeled not-found rejection.
AWS: Smithy: ListProposals GET /networks/{NetworkId}/proposals, modeled errors AccessDeniedException(403), InternalServiceErrorException(500), InvalidRequestException(400), ResourceNotFoundException(404), ThrottlingException(429). Unlike ListMembers/ListNodes, ListProposals validates NetworkId existence: a well-formed but nonexistent NetworkId yields 404 ResourceNotFoundException with a Message naming the missing network (example: 'Network n-000... not found.').
Emulator: returns error code 'NotImplemented' (replay assertion: 'expected ResourceNotFoundException, got NotImplemented'), i.e. the operation is a scaffold stub and no 404 path exists. NotImplemented is not a modeled error of this service.
Why: every caller of ListProposals — including the normal 'list proposals in my network' happy path — gets an unmodeled error code no SDK error union contains, so nothing can be read or branched on; the proposal surface is entirely unreachable.
Fix: implement the ListProposals handler (services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/list_proposals.rs): resolve NetworkId against state, return ResourceNotFoundException (404) when it does not exist and a paginated Proposals page when it does.
Traces: managedblockchain.list-proposals-nonexistent-network-not-found.

Cause 5 of 5: CreateMember answers NotImplemented instead of its modeled not-found rejection.
AWS: Smithy: CreateMember POST /networks/{NetworkId}/members with required ClientRequestToken, InvitationId, NetworkId and MemberConfiguration; modeled errors include ResourceNotFoundException (client, 404) and InvalidRequestException (400). Recorded behavior: a CreateMember naming a nonexistent NetworkId/InvitationId fails 404 ResourceNotFoundException with a Message naming the missing network (example: 'Network n-000... not found.'), and nothing is created — a subsequent GetNetwork on the same id still reports not found.
Emulator: returns error code 'NotImplemented' (replay assertion: 'AssertionError: NotImplemented' where ResourceNotFoundException was expected), i.e. CreateMember is a scaffold stub with no validation or creation path.
Why: joining a network by invitation is unimplemented, so the member lifecycle (invite -> CreateMember -> GetMember/ListMembers) cannot be exercised, and the error a client does receive is an unmodeled code its SDK error union cannot classify.
Fix: implement the CreateMember handler (services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/create_member.rs): resolve NetworkId and InvitationId against state, return ResourceNotFoundException (404) when either is unknown, and otherwise create the member idempotently under ClientRequestToken.
Traces: managedblockchain.create-member-invitation-not-found.

## Evidence
Behavioral suite replay against the booted emulator (suite simparity-behavioral-aws-v1): managedblockchain 0/23 passed (7 mismatched, 16 errored, 0 skipped, 0 unloadable); bootstrap_signal false — the emulator was serving and answered every call. Checks blocked by the auto-filed stub issues (CreateAccessor, UpdateNode, ListMembers, CreateProposal, ListInvitations) are excluded here.

Cause 1 — managedblockchain.create-network-invalid-framework-version-rejected [mismatch]
  Call: CreateNetwork(Framework='HYPERLEDGER_FABRIC', FrameworkVersion='99.9', VotingPolicy=ApprovalThresholdPolicy{50, 1h, GREATER_THAN}, MemberConfiguration=Fabric admin).
  Expected: ClientError, Code='InvalidRequestException' (HTTP 400), Message naming the rejected version and framework; no network created.
  Actual: HTTP 200 — {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'NetworkId': 'n-IEWA642V6FB3RML4RSLDBOZMX6', 'MemberId': 'm-ML4RSLDBOZMX6CG5CWTHS7XY2X'} (ids are environment-specific; the divergence is the 200 + creation).
  Harness detail: 'AssertionError: expected a ClientError, but the call succeeded: {...}'.

Cause 2 — managedblockchain.get-network-not-found [error]
  Call: GetNetwork(NetworkId='n-ethereum-mainnet') (the check's positive control, before any negative assertion).
  Expected: HTTP 200 with Network.Id == 'n-ethereum-mainnet' and Network.Framework == 'ETHEREUM'.
  Actual: botocore.errorfactory.ResourceNotFoundException — 'An error occurred (ResourceNotFoundException) when calling the GetNetwork operation: network n-ethereum-mainnet does not exist on this account'.

Cause 3 — managedblockchain.list-nodes-nonexistent-member-rejected [mismatch]
  Call: ListNodes(NetworkId='n-'+'0'*30, MemberId='m-'+'0'*30); second call ListNodes(NetworkId='n-'+'0'*30) with MemberId omitted.
  Expected: both fail Code='InvalidRequestException' (HTTP 400), Messages 'Member with Id m-000... not found in network n-000....' and 'Member Id is required for network n-000....' (examples of request-derived text).
  Actual: Code='ResourceNotFoundException' with message 'network n-000000000000000000000000000000 does not exist on this account'; ResourceNotFoundException is not a modeled error of ListNodes.

Cause 4 — managedblockchain.list-proposals-nonexistent-network-not-found [mismatch]
  Call: ListProposals(NetworkId='n-'+'0'*30).
  Expected: Code='ResourceNotFoundException' (HTTP 404).
  Actual: 'AssertionError: expected ResourceNotFoundException, got NotImplemented' — the emulator returned error code NotImplemented ('operation is scaffolded but not implemented in this emulator' class of response).

Cause 5 — managedblockchain.create-member-invitation-not-found [mismatch]
  Call: CreateMember(ClientRequestToken=<token>, InvitationId='in-'+'0'*29, NetworkId='n-'+'0'*30, MemberConfiguration=Fabric admin).
  Expected: Code='ResourceNotFoundException' (HTTP 404); the subsequent GetNetwork on the same id also 404s with nothing created.
  Actual: 'AssertionError: NotImplemented' — error Code was 'NotImplemented' instead of ResourceNotFoundException.

All recorded message texts above are examples; the asserted requirements are the HTTP status, the error code, whether the call is accepted at all, and that a Message is derived from the request.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it (managedblockchain is bootstrapped: services/service-com-amazonaws-managedblockchain-taigawebservice).
2. Point a botocore client at the emulator: client = boto3.client('managedblockchain', region_name='us-east-1', endpoint_url=<emulator>) (AMB Access has no us-west-2 endpoint; the suite pins us-east-1).
3. Cause 1: call create_network(ClientRequestToken=<64-char token>, Name=<unique>, Framework='HYPERLEDGER_FABRIC', FrameworkVersion='99.9', VotingPolicy={'ApprovalThresholdPolicy': {'ThresholdPercentage': 50, 'ProposalDurationInHours': 1, 'ThresholdComparator': 'GREATER_THAN'}}, MemberConfiguration={'Name': 'org1', 'FrameworkConfiguration': {'Fabric': {'AdminUsername': 'AdminUser1', 'AdminPassword': 'Password123'}}}). Observe HTTP 200 with NetworkId/MemberId; expected 400 InvalidRequestException and no network created.
4. Cause 2: call get_network(NetworkId='n-ethereum-mainnet'). Observe 404 ResourceNotFoundException; expected 200 with Network.Id='n-ethereum-mainnet', Network.Framework='ETHEREUM'.
5. Cause 3: call list_nodes(NetworkId='n-'+'0'*30, MemberId='m-'+'0'*30), then list_nodes(NetworkId='n-'+'0'*30). Observe ResourceNotFoundException (404) for the first; expected InvalidRequestException (400) for both, the second complaining that MemberId is required.
6. Cause 4: call list_proposals(NetworkId='n-'+'0'*30). Observe error code NotImplemented; expected 404 ResourceNotFoundException.
7. Cause 5: call create_member(ClientRequestToken=<token>, InvitationId='in-'+'0'*29, NetworkId='n-'+'0'*30, MemberConfiguration={'Name': 'org<suffix>', 'FrameworkConfiguration': {'Fabric': {'AdminUsername': 'AdminUser1', 'AdminPassword': 'Password123'}}}). Observe error code NotImplemented; expected 404 ResourceNotFoundException with nothing created.
8. Equivalently, re-replay the behavioral checks managedblockchain.create-network-invalid-framework-version-rejected, managedblockchain.get-network-not-found, managedblockchain.list-nodes-nonexistent-member-rejected, managedblockchain.list-proposals-nonexistent-network-not-found, managedblockchain.create-member-invitation-not-found.
