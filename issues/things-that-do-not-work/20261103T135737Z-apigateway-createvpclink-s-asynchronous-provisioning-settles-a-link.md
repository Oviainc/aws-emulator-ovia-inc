Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.create-vpc-link-empty-target-list-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.create-vpc-link-empty-target-list-rejected|run#22|AssertionError|assert fetched["status"] == "FAILED", f"expected the link to a nonexistent NLB to settle FAILED, got {fetched}"
Issue-Commit: f9571cbe65aa0d276fa5ca35992cbed3efbd51b6

# apigateway: CreateVpcLink's asynchronous provisioning settles a link whose target NLB does not exist in AVAILABLE with no statusMessage, instead of FAILED / "NLB ARN not found"

> _Note: this issue was identified against the code base at commit `f9571cbe65aa0d276fa5ca35992cbed3efbd51b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcLink

## Description
One net-new root cause in the apigateway service, grounded in one failing behavioral check that is neither marked ALREADY FILED nor OWNED by any open issue.

Related open issue, named for the fixer: 20261028T131753Z-apigateway-3-net-new-root-causes-a-vpclink.md (open) describes the SAME root cause as its "Cause 1 of 3", but it does not carry this check's replay key (its Issue-Trace-Ids are apigateway.create-vpc-link-invalid-target-arn-returns-badrequest, apigateway.delete-vpc-link-in-use-by-integration-conflict, apigateway.stage-repoint-unblocks-old-deployment-delete, apigateway.update-stage-conflict-during-deployment) and the replay report therefore did not attribute this check to it. A fixer closing that issue must re-replay THIS check id too; it exercises an additional observable the other check does not assert, namely the statusMessage AWS attaches to the settled FAILED link. This is not a REOPENS: no fixed issue claimed this operation's settled status.

Cause 1 of 1: a VpcLink created with a syntactically well-formed, same-account target ARN that names a network load balancer which does not exist converges to status AVAILABLE and carries no statusMessage, where AWS settles it in FAILED with a statusMessage explaining why.
AWS: Smithy (com.amazonaws.apigateway#BackplaneControlService): CreateVpcLink returns the VpcLink shape; VpcLink$status targets enum VpcLinkStatus whose values are exactly AVAILABLE | PENDING | DELETING | FAILED and is documented "The status of the VPC link. The valid values are AVAILABLE, PENDING, DELETING, or FAILED. Deploying an API will wait if the status is PENDING and will fail if the status is DELETING."; VpcLink$statusMessage is documented "A description about the VPC link status."; VpcLink$targetArns is documented "The ARN of the network load balancer of the VPC targeted by the VPC link. The network load balancer must be owned by the same Amazon Web Services account of the API owner." The model does not state which terminal state an unresolvable target produces, so for that: recorded behavior only — CreateVpcLink is ACCEPTED and returns status PENDING (provisioning is asynchronous), and the asynchronous provisioning then settles the link in status FAILED, never AVAILABLE, when the targetArn names a load balancer that does not exist in the account; the settled link carries statusMessage "NLB ARN not found". That statusMessage is entirely FIXED text — it has no variable part and in particular does not echo the ARN, the link id or the link name. GetVpcLink reports that terminal FAILED/statusMessage pair. What decides the terminal state is whether the target RESOLVES, not whether its ARN parses.
Emulator: CreateVpcLink is accepted and returns status PENDING, then GetVpcLink, polled to a terminal value, returns status "AVAILABLE" with no statusMessage member at all (HTTP 200; the response body contains id, name, targetArns, status, tags only). The FAILED branch of the VpcLink state machine is unreachable for this condition because the settled outcome is decided from ARN SHAPE alone: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_vpc_link.rs decides provisioning success by parsing the ARN (vendor elasticloadbalancing, resource prefix loadbalancer/net/) without consulting Elastic Load Balancing state, so any well-formed ARN passes.
Why: a client that uses VpcLink status as its readiness/validation gate is told the link is usable while its backend does not exist. Deploy pipelines that wait out PENDING and proceed on AVAILABLE proceed against a link pointing at nothing, and the misconfiguration resurfaces later as unexplained HTTP_PROXY/VPC_LINK integration failures with no resource reporting the fault — whereas AWS surfaces it on the VpcLink itself, FAILED, with the reason in statusMessage. Any consumer reading statusMessage to explain a failure reads an absent member, and negative tests asserting that a bad target is rejected can never pass.
Fix: the CreateVpcLink handler's asynchronous provisioning outcome in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_vpc_link.rs (the provisioning-failure decision and the VpcLink state it settles into) must resolve every targetArn against Elastic Load Balancing state and settle the link in status FAILED with statusMessage set when the named network load balancer does not exist; and GetVpcLink's serializer must emit statusMessage whenever the stored record has one. ARN syntax alone must not be treated as provisioning success.
Note on scope: the same check's empty-targetArns leg PASSED — CreateVpcLink(targetArns=[]) is already refused with BadRequestException and the message "Need at least one target arn to create a vpc link", and no VpcLink is left behind. Only the asynchronous settling of a well-formed-but-unresolvable target diverges.
Traces: apigateway.create-vpc-link-empty-target-list-rejected (suite section "### apigateway"; failing assertion at check_apigateway_create_vpc_link_empty_target_list_rejected.py line 71).

## Evidence
Failing check: apigateway.create-vpc-link-empty-target-list-rejected [mismatch]

Passing legs (for scope): CreateVpcLink(name=<unique>, targetArns=[]) was refused with Code 'BadRequestException' and Message 'Need at least one target arn to create a vpc link'; GetVpcLinks then showed no link with that name; the positive-control create returned status 'PENDING' with the submitted name and targetArns echoed back.

Diverging leg — the asynchronous settle:
  Request: apigw.create_vpc_link(name='<unique>', targetArns=['arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/net/sim2real-dummy-nlb/50dc6c495c0c9188'], tags={'sim2real': '<check>'}) — no such load balancer exists in the account.
  Then apigw.get_vpc_link(vpcLinkId=<id>) polled at 5 s intervals (budget 600 s) until status is AVAILABLE or FAILED.
  expected: status == 'FAILED' and statusMessage == 'NLB ARN not found'  (both constants; no variable parts)
  actual:   HTTP 200, body {'id': '<id>', 'name': '<name>', 'targetArns': ['<the submitted arn>'], 'status': 'AVAILABLE', 'tags': {...}} — 'AVAILABLE' is terminal and no 'statusMessage' member is present.
  AssertionError: expected the link to a nonexistent NLB to settle FAILED, got {... 'status': 'AVAILABLE' ...}  (check_apigateway_create_vpc_link_empty_target_list_rejected.py line 71 / run body line 179 of the embedded source: assert fetched["status"] == "FAILED")
  The next assertion, assert fetched.get("statusMessage") == "NLB ARN not found", is unreachable for the same reason.

The host, region, account id, generated link id/name and the NLB name/hash in the ARN above are per-run values and are not part of the requirement; the requirement is the terminal status FAILED and the constant statusMessage 'NLB ARN not found' for any targetArn that does not resolve to an existing network load balancer.

## Steps to Reproduce
1. sts.get_caller_identity() to obtain the account id.
2. Confirm the refusal leg still holds: apigw.create_vpc_link(name='<unique-bad>', targetArns=[]) must raise BadRequestException with Message exactly 'Need at least one target arn to create a vpc link', and apigw.get_vpc_links(limit=500) must contain no item whose name is '<unique-bad>'.
3. Build a well-formed, same-account target ARN for a network load balancer that does NOT exist: 'arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/net/<nonexistent-nlb-name>/<16-hex>'.
4. created = apigw.create_vpc_link(name='<unique-ok>', targetArns=[<that arn>], tags={'sim2real': 'create-vpc-link-empty-target-list-rejected'}); observe created['status'] == 'PENDING' (correct).
5. Poll apigw.get_vpc_link(vpcLinkId=created['id']) every 5 s until status is in ('AVAILABLE', 'FAILED').
6. Observe the emulator settles status 'AVAILABLE' with no statusMessage member. AWS settles status 'FAILED' with statusMessage 'NLB ARN not found'.
7. Replay apigateway.create-vpc-link-empty-target-list-rejected to confirm the fix (and re-replay apigateway.create-vpc-link-invalid-target-arn-returns-badrequest, owned by 20261028T131753Z-apigateway-3-net-new-root-causes-a-vpclink.md, which exercises the same settling path).
