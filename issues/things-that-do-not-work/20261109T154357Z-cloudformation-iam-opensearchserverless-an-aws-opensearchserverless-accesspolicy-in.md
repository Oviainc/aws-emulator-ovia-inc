Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__opensearchserverless.cfn-access-policy-data-principal-role-read-back-then-gone cloudformation__iam__opensearchserverless.cfn-access-policy-cross-account-principal-rejected
Issue-Key: cloudformation__iam__opensearchserverless.cfn-access-policy-data-principal-role-read-back-then-gone|run#13|AssertionError|assert listed == [(policy_name, "data")], f"ListAccessPolicies {listed!r}"
Issue-Key: cloudformation__iam__opensearchserverless.cfn-access-policy-cross-account-principal-rejected|run#12|AssertionError|assert _listed(aoss, ok[1]) == [(ok[1], "data")], f"control policy {ok[1]!r} not listed"
Issue-Commit: 44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07

# cloudformation__iam__opensearchserverless: an AWS::OpenSearchServerless::AccessPolicy in a CREATE_COMPLETE stack is never created in OpenSearch Serverless — ListAccessPolicies(type='data') does not list it

> _Note: this issue was identified against the code base at commit `44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE net-new root cause, grounded in two failing behavioral checks listed under the suite heading `### cloudformation__iam__opensearchserverless`.

=== Cause 1 of 1: the AWS::OpenSearchServerless::AccessPolicy resource handler is inert — the stack reaches CREATE_COMPLETE but no data access policy exists in OpenSearch Serverless ===
AWS: CloudFormation's AWS::OpenSearchServerless::AccessPolicy handler calls the Smithy-modeled opensearchserverless CreateAccessPolicy(type: AccessPolicyType (required), name: PolicyName (required, length 3..32, pattern ^[a-z][a-z0-9-]+$), policy: PolicyDocument (required, length 1..20480), description), so a CREATE_COMPLETE stack leaves a real policy behind: ListAccessPolicies(type='data') — modeled, paginated on nextToken, output accessPolicySummaries — reports exactly one summary whose name is the declared Name and whose type is 'data'; GetAccessPolicy(name, type='data') returns accessPolicyDetail with that name/type and the declared Rules with the role ARN as Principal; and the stack's Ref for the resource is the policy's composed identifier `data|<name>` (recorded behavior; the CFN resource's primary identifier is Type|Name). Deleting the stack calls DeleteAccessPolicy, after which GetAccessPolicy is refused ResourceNotFoundException with the message `Policy with name <name> and type data is not found` (fixed text, the name taken from the request). The name and the rules of any one run are request values; the listing's non-emptiness and the Type|Name Ref are the requirement.
Emulator: the stack is reported CREATE_COMPLETE, but OpenSearch Serverless holds no such policy: ListAccessPolicies(type='data'), read across every page, contains no summary with the declared name — `AssertionError: ListAccessPolicies []` and `AssertionError: control policy 'sim2real-apolxok-21d019069d1c' not listed`. The resource is therefore stub-provisioned: CloudFormation succeeds without ever calling CreateAccessPolicy in the opensearchserverless state.
Why: a client deploying its OpenSearch Serverless authorization through CloudFormation (the documented way to manage data access policies as code) gets a green stack and no policy. Nothing grants the data-plane access the template declares; GetAccessPolicy answers not-found for a resource CloudFormation says exists; and because no policy is ever created, OpenSearch Serverless's own refusals cannot run either — the cross-account-principal rejection that must roll a stack back (CreateAccessPolicy ValidationException 'Cross account principal(s) are not allowed in policy. Please update the policy and retry', surfaced as a CREATE_FAILED resource reason with HandlerErrorCode InvalidRequest) can never be produced, so an invalid template that AWS rolls back would be accepted.
Fix: the AWS::OpenSearchServerless::AccessPolicy resource handler in the emulator's CloudFormation resource-type registry — on CREATE it must call the opensearchserverless CreateAccessPolicy path (validating the policy document and the principals, so a cross-account principal fails the resource and rolls the stack back), return Type|Name as the physical resource id, and on DELETE call DeleteAccessPolicy; the created policy must live in the same opensearchserverless state the control-plane readers (ListAccessPolicies/GetAccessPolicy) serve.
Traces: cloudformation__iam__opensearchserverless.cfn-access-policy-data-principal-role-read-back-then-gone, cloudformation__iam__opensearchserverless.cfn-access-policy-cross-account-principal-rejected.

## Evidence
Expected (AWS): a stack whose only resource is
{"Type": "AWS::OpenSearchServerless::AccessPolicy", "Properties": {"Name": <name>, "Type": "data", "Policy": <json rules with the stack's role ARN as Principal>}}
reaches CREATE_COMPLETE, after which ListAccessPolicies(type='data') lists [(<name>, 'data')], the stack output Ref is `data|<name>`, and GetAccessPolicy(name=<name>, type='data').accessPolicyDetail carries the declared Rules and the role ARN as Principal; deleting the stack makes GetAccessPolicy answer ResourceNotFoundException `Policy with name <name> and type data is not found`.

Actual (emulator):
- cloudformation__iam__opensearchserverless.cfn-access-policy-data-principal-role-read-back-then-gone [mismatch]: the stack is CREATE_COMPLETE, then `AssertionError: ListAccessPolicies []` (check line 250) — every page of ListAccessPolicies(type='data') is read and the declared policy is absent.
- cloudformation__iam__opensearchserverless.cfn-access-policy-cross-account-principal-rejected [mismatch]: the positive-control stack is CREATE_COMPLETE, then `AssertionError: control policy 'sim2real-apolxok-21d019069d1c' not listed` (check line 265). The policy name is this run's value; the divergence is that the listing is empty for a policy CloudFormation reports as created.

## Steps to Reproduce
1. cloudformation.create_stack with Capabilities=['CAPABILITY_NAMED_IAM'] and a template containing an AWS::IAM::Role plus
   {"Type": "AWS::OpenSearchServerless::AccessPolicy", "Properties": {"Name": <3..32 lower-case name>, "Type": "data", "Policy": <JSON list of {Rules:[{ResourceType:'index'|'collection', Resource:[...], Permission:[...]}], Principal:[<the role ARN>]}>}}, with Outputs {"PolicyRef": {"Ref": "<the AccessPolicy logical id>"}}.
2. Wait for the stack: it reports CREATE_COMPLETE.
3. opensearchserverless.list_access_policies(type='data'), paging on nextToken.
   -> EXPECTED: one summary with name=<name>, type='data'; the stack output PolicyRef == `data|<name>`.
   -> ACTUAL: the declared policy appears on no page (empty result for that name).
4. opensearchserverless.get_access_policy(name=<name>, type='data') -> expected to return the declared Rules and role Principal; it has nothing to return.
5. Variant: the same template with Principal=[arn:aws:iam::<another account>:role/...] must fail the resource and leave the stack ROLLBACK_COMPLETE with the CreateAccessPolicy cross-account ValidationException as the resource reason; with no handler calling CreateAccessPolicy that refusal is never produced.
