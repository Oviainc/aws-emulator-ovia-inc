Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream__ec2.asvpce-deleted-vpce-lingers-on-stack-and-reattach-rejected
Issue-Key: appstream__ec2.asvpce-deleted-vpce-lingers-on-stack-and-reattach-rejected|_error#16|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Commit: 506f51337b02a9b5673b5526bcfdc40d4e5c4821

# AppStream CreateStack accepts a STREAMING AccessEndpoint naming a deleted VPC endpoint instead of refusing it InvalidParameterValueException

> _Note: this issue was identified against the code base at commit `506f51337b02a9b5673b5526bcfdc40d4e5c4821`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause.

Cause 1 of 1: CreateStack does not re-validate AccessEndpoints[].VpceId against EC2, so a stale (deleted) interface-endpoint id is stored and the stack is created.

AWS: AppStream resolves every AccessEndpoints[].VpceId when the reference is WRITTEN to a stack; an id that no longer resolves in the caller's account is refused and the stack is not created. The refusal is HTTP 400, error code InvalidParameterValueException, message "The VPC endpoint <vpce-id> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again." — the fixed text is everything but <vpce-id>, which is the VpceId taken verbatim from the request. Smithy (services/service-com-amazonaws-appstream-photonadminproxyservice): CreateStack models InvalidParameterCombinationException / ResourceNotFoundException / InvalidAccountStatusException; the exact liveness-refusal code and wording above are recorded behavior only. AWS also keeps the stale reference on the ALREADY-created stack (deleting the endpoint does not detach it), and UpdateStack with a fresh live endpoint replaces it.

Emulator: once the endpoint is deleted and unresolvable by id through ec2 DescribeVpcEndpoints, appstream CreateStack with that same AccessEndpoints list SUCCEEDS (2xx, no error). No refusal is produced and a stack referencing a dead endpoint exists.

Why: a client relying on AppStream rejecting dead endpoint references silently gets a stack whose streaming endpoint can never serve. Code written against AWS catches InvalidParameterValueException to re-mint the endpoint and repair the stack; against the emulator that branch is unreachable and the fault surfaces only at streaming time.

Fix: CreateStack's AccessEndpoints validation in the appstream service must resolve each VpceId against live EC2 VPC-endpoint state before accepting, and refuse with the code and message above when it does not resolve, leaving no stack behind.

Traces: appstream__ec2.asvpce-deleted-vpce-lingers-on-stack-and-reattach-rejected. Open issue 20261102T120557Z-appstream-ec2-7-net-new-appstream-root-causes.md covers other AccessEndpoints causes (duplicate types, five endpoints, non-AppStream vpce, nonexistent vpce) but not this deleted-but-well-formed id path, and the suite does not mark this check as owned.

## Evidence
Expected (AWS, from the recorded response): CreateStack refused with ('InvalidParameterValueException', "The VPC endpoint <vpce-id> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again."), and DescribeStacks on the new stack name then refused ('ResourceNotFoundException', 'The stack <name> for account <account> does not exist.').
Actual (emulator): the CreateStack call returned successfully. The check's error helper reports: AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error — raised at check_appstream__ec2_asvpce_deleted_vpce_lingers_on_stack_and_reattach_rejected.py line 46 on `appstream.create_stack(Name=other, AccessEndpoints=stale)` after the endpoint had been deleted and waited to be unresolvable by id.

## Steps to Reproduce
1. Create a VPC/subnet and an AppStream STREAMING interface VPC endpoint; note its VpcEndpointId.
2. appstream CreateStack with AccessEndpoints=[{EndpointType: STREAMING, VpceId: <id>}] — succeeds.
3. ec2 DeleteVpcEndpoints on that id and wait until DescribeVpcEndpoints(VpcEndpointIds=[<id>]) raises not-found.
4. appstream CreateStack with a NEW stack name and the same stale AccessEndpoints list.
5. Expected: HTTP 400 InvalidParameterValueException "The VPC endpoint <id> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again.", and DescribeStacks on the new name reports ResourceNotFoundException. Observed: the CreateStack succeeds.
