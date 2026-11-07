Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.deploy-ipset-cfn-blocks-listed-address
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.deploy-ipset-cfn-blocks-listed-address|_deploy_cfn#10|WaiterError|cfn.get_waiter("stack_create_complete").wait(
Issue-Commit: ef1bb1e3a86f6fb50b04ae6c71e141a327380e0c

# wafv2: a CloudFormation stack of AWS::WAFv2::IPSet + WebACL + WebACLAssociation now ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE, so no IaC-deployed WAF can be reached at all

> _Note: this issue was identified against the code base at commit `ef1bb1e3a86f6fb50b04ae6c71e141a327380e0c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause in the CloudFormation-provisioned WAFv2 surface, grounded in a failing behavioral check of the wafv2 service.

Cause 1 of 1: a stack whose resources are AWS::WAFv2::IPSet, AWS::WAFv2::WebACL (a Block rule with an IPSetReferenceStatement whose Arn is Fn::GetAtt <IPSet>.Arn) and AWS::WAFv2::WebACLAssociation fails provisioning and rolls back.

AWS: a CreateStack of that template reaches CREATE_COMPLETE — the WAFv2 resource providers create the three real WAFv2 entities (the control-plane calls they stand for are modelled: CreateIPSet with Name/Scope/IPAddressVersion/Addresses all required, CreateWebACL with Name/Scope/DefaultAction/VisibilityConfig required and Rules carrying the IPSetReferenceStatement, AssociateWebACL with WebACLArn and ResourceArn, each ResourceArn @length 20..2048 @pattern \S), and the stack's Outputs then publish the WebACL and IPSet ARNs. Smithy (com.amazonaws.wafv2#AWSWAF_20190729): CreateIPSet / CreateWebACL / AssociateWebACL as above; every refusal in their error sets is a modelled 400 (WAFInvalidParameterException, WAFNonexistentItemException, WAFInvalidResourceException, WAFUnavailableEntityException, ...). The CREATE_COMPLETE terminal status itself is recorded behavior only.

Emulator: the create never reaches a terminal success. The stack_create_complete waiter matches the terminal path "Stacks[].StackStatus" == "ROLLBACK_COMPLETE", i.e. at least one of the three WAFv2 resources fails its CREATE and CloudFormation rolls the whole stack back. The check therefore dies inside its deploy helper, before it can call GetIPSet or probe the protected endpoint at all. Hypothesis (neither the model nor the recording states which resource failed, and the replay carries no stack-event text): the WebACL provider cannot resolve the IPSet ARN handed to it by Fn::GetAtt <IPSet>.Arn, so CreateWebACL is refused — consistent with the already-filed finding that stack-published WAFv2 identifiers/ARNs are not the entities' real Id/ARN. A fixer should read the stack's resource-status reason to confirm which resource failed.

Why: no WAFv2 protection can be deployed by IaC. A client that templates an IPSet + WebACL + association gets a stack that deletes everything it just made and a ROLLBACK_COMPLETE status, instead of a working, associated web ACL — and it gets this with no modelled WAF error to act on, so the failure is neither recoverable nor diagnosable. This is strictly worse than the previously recorded behaviour, where the stack did deploy and only the published identifiers and the edge decision were wrong.

Fix: the CloudFormation WAFv2 resource providers (AWS::WAFv2::IPSet, AWS::WAFv2::WebACL, AWS::WAFv2::WebACLAssociation) — their create handlers, the physical-resource-id/attribute resolution that feeds Fn::GetAtt <IPSet>.Arn into the WebACL rule, and the wafv2 state those handlers must write — together with whatever refusal the WebACL create currently raises against that resolved ARN.

Related open issue, which does NOT cover this cause: 20261024T065754Z-wafv2-cloudformation-deployed-wafv2-resources-are-not-real-waf.md names this same check, but for a stack that DID deploy (its causes are the non-UUID IPSet identifier, the malformed WebACL GetAtt Arn, and the protected edge never answering, keyed on a _poll_until timeout). The failure here is earlier and different: the stack no longer deploys.

Traces: wafv2.deploy-ipset-cfn-blocks-listed-address.

## Evidence
Check wafv2.deploy-ipset-cfn-blocks-listed-address, verdict [error], raised from the check's _deploy_cfn helper:

  File "waiter.py", line 378, in wait
    raise WaiterError(
  botocore.exceptions.WaiterError: Waiter StackCreateComplete failed: Waiter encountered a terminal failure state: For expression "Stacks[].StackStatus" we matched expected path: "ROLLBACK_COMPLETE" at least once

Expected: DescribeStacks reports StackStatus CREATE_COMPLETE for the stack, the stack's Outputs carry WebACLArn and IPSetArn, wafv2 GetIPSet on the published IPSet Id returns the IPSet with exactly the deployed /32 addresses, and requests from a listed source address to the associated REST stage are answered 403.
Actual: DescribeStacks reports StackStatus ROLLBACK_COMPLETE; the stack's resources are rolled back, so no IPSet, WebACL or association exists and none of the follow-up assertions is ever reached.

The stack name, the API id, the CIDR values and the region are per-run values from the check's own environment and are not part of the requirement; the requirement is the terminal stack status for this template shape.

## Steps to Reproduce
1. Create a REGIONAL mock REST API with a deployed stage and take its stage ARN (arn:aws:apigateway:<region>::/restapis/<api-id>/stages/<stage>).
2. CreateStack with a template containing:
   - AWS::WAFv2::IPSet (Scope REGIONAL, IPAddressVersion IPV4, Addresses = a list of <ip>/32 entries),
   - AWS::WAFv2::WebACL (Scope REGIONAL, DefaultAction Allow, one Rule at Priority 0 with Action Block and Statement.IPSetReferenceStatement.Arn = {"Fn::GetAtt": "<IPSet logical id>.Arn"}, plus VisibilityConfig),
   - AWS::WAFv2::WebACLAssociation (ResourceArn = the stage ARN, WebACLArn = {"Fn::GetAtt": "<WebACL logical id>.Arn"}),
   and Outputs publishing the WebACL and IPSet ARNs.
3. Wait on the stack_create_complete waiter / poll DescribeStacks: the stack reaches ROLLBACK_COMPLETE, where AWS reaches CREATE_COMPLETE.
4. Read the stack's resource statuses (DescribeStackEvents / DescribeStackResources) to see which of the three WAFv2 resources failed CREATE and with what reason.
5. Or replay check wafv2.deploy-ipset-cfn-blocks-listed-address.
