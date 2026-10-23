Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: waf-regional.deploy-cfn-ipset-lifecycle
Issue-Shape-Id: com.amazonaws.wafregional#AWSWAF_Regional_20161128
Issue-Key: waf-regional|CreateStack|ValidationError
Issue-Commit: d3f6311bb5ff647942e0262416bf393388d03174

# CloudFormation rejects AWS::WAFRegional::IPSet as an unrecognized resource type, so no WAF-Regional resource can be provisioned by a stack

> _Note: this issue was identified against the code base at commit `d3f6311bb5ff647942e0262416bf393388d03174`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause (1 cause) surviving in the bootstrapped waf-regional surface (crate services/service-com-amazonaws-wafregional-awswaf-regional-20161128, driven through the CloudFormation emulator's resource-type registry).

Relation to open issues: the suite did NOT mark waf-regional.deploy-cfn-ipset-lifecycle as OWNED, because its failure point has moved again and no open issue's replay key matches it. Two open issues describe EARLIER, different failure modes of this same check and do not cover this one: 20261022T031012Z-waf-regional-exception-messages-are-synthesized-from-the.md (its cause 4: the stack reached CREATE_COMPLETE but exposed NO Outputs at all) and 20261023T123052Z-waf-regional-createwebacl-is-accepted-instead-of-refused.md (its cause 2: the Output was present but resolved to the template's Name instead of a minted IPSetId). In this run the stack is never created at all — CreateStack itself refuses the template — so this is a third, strictly earlier failure point (a regression relative to both of those states) and must be closed on its own. The remaining waf-regional failures in this run are all either ALREADY FILED unimplemented-stub clusters (UpdateRuleGroup, PutPermissionPolicy, UpdateRateBasedRule, UpdateRegexPatternSet, UpdateSizeConstraintSet, DeleteRule, AssociateWebACL) or OWNED by an open issue, and are deliberately not duplicated here.

=== Cause 1 of 1: CreateStack refuses a template declaring AWS::WAFRegional::IPSet with ValidationError 'Unrecognized resource types' ===
AWS: recorded behavior only for the CFN resource-type side — AWS::WAFRegional::IPSet is a supported CloudFormation resource type, so a template declaring one such resource with a Name property and an Output {"Value": {"Ref": <logical id>}} is ACCEPTED by CreateStack (HTTP 200 with a StackId) and the stack proceeds to CREATE_COMPLETE; the provider then calls the waf-regional control plane so the resource's Ref is the minted IPSetId and GetIPSet reads it back. Smithy (CloudFormation model, CreateStack): the operation's modeled errors are AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException and TokenAlreadyExistsException (all errorKind client, httpError 400, awsQueryError codes of the same names) — a supported resource type produces none of them, and in particular produces no template-format ValidationError. Smithy (waf-regional model services/service-com-amazonaws-wafregional-awswaf-regional-20161128/smithy-model/com-amazonaws-wafregional-awswaf-regional-20161128.json): CreateIPSet(Name required ResourceName 1..128, ChangeToken required) returns IPSet + ChangeToken; GetIPSet(IPSetId required ResourceId 1..128) errors WAFNonexistentItemException (client, httpError 400 defaulted); DeleteIPSet(IPSetId, ChangeToken) errors WAFNonexistentItemException / WAFNonEmptyEntityException / WAFReferencedItemException (all client, 400) — these are the state calls the CFN provider must drive on create and delete.
Emulator: CreateStack is refused outright with HTTP 400, error code ValidationError, message example 'Template format error: Unrecognized resource types: [AWS::WAFRegional::IPSet]'. The type is absent from the emulator's CloudFormation resource-type registry, so the stack is never created, CREATE_COMPLETE is never reached, no Outputs exist, no CreateIPSet is ever issued, and every later leg of the contract (Ref resolution, GetIPSet readback, delete propagation, and the post-delete UpdateIPSet refusal with WAFNonexistentItemException or WAFNonexistentContainerException) is unreachable.
Why: a client that deploys WAF-Regional IPSets through CloudFormation cannot deploy at all against the emulator — a template that AWS accepts is rejected at submission with a template-format ValidationError, so infrastructure-as-code paths, nested stacks and any downstream consumer of the Ref/Output are untestable, and CI that validates templates against the emulator reports a false template error.
Fix: the CloudFormation resource-type registry / template validation in the CloudFormation emulator must recognize AWS::WAFRegional::IPSet, plus the resource provider for that type: its create path must call the waf-regional CreateIPSet state and register the returned IPSetId as the resource's physical id so {"Ref": ...} and stack Outputs resolve to it, and its delete path must call DeleteIPSet for that id. The waf-regional CreateIPSet/GetIPSet/DeleteIPSet handlers are the dependency it drives, not the code to change. The requirements are CreateStack's acceptance (no ValidationError for a supported type), the stack reaching CREATE_COMPLETE, and the Ref resolving to a GetIPSet-readable IPSetId; the quoted message is the recorded example only.
Traces: waf-regional.deploy-cfn-ipset-lifecycle [error].

## Evidence
Behavioral suite run (simparity-behavioral-aws-v1): waf-regional 8/24 passed (33%), 16 failed. After skipping every ALREADY FILED unimplemented-stub cluster and every check marked OWNED by an open issue, one net-new root cause survives (two further surviving mismatches are error-message-text-only and are not filed — see the run notes).

Cause 1 — waf-regional.deploy-cfn-ipset-lifecycle [error]
  template submitted: {'Resources': {'TestIPSet': {'Type': 'AWS::WAFRegional::IPSet', 'Properties': {'Name': <ipset-name>}}}, 'Outputs': {'IPSetId': {'Value': {'Ref': 'TestIPSet'}}}}
  call: cfn.create_stack(StackName=<stack>, TemplateBody=json.dumps(template), Tags=[{'Key': 'sim2real', 'Value': 'deploy-cfn-ipset-lifecycle'}])
  expected: HTTP 200 with a StackId; the stack then reaches CREATE_COMPLETE and DescribeStacks exposes Outputs['IPSetId'] whose value GetIPSet resolves to an IPSet with that IPSetId and Name == <ipset-name>.
  actual:   botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::WAFRegional::IPSet]
  Status 400 + code ValidationError are the divergence; the message string is the recorded example. The check aborts on this first call, so the CREATE_COMPLETE assertion, the Ref/Output resolution, the GetIPSet readback, the delete-propagation leg and the post-delete UpdateIPSet refusal leg were never exercised in this run.

Stack names, IPSet names and generated ids are environment-specific and are not requirements.

Smithy citations used: CloudFormation CreateStack — input StackName (required), TemplateBody (length min 1), Outputs StackId/OperationId; modeled errors AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException, TokenAlreadyExistsException (client, httpError 400, awsQueryError same codes) — no template-format ValidationError for a supported resource type. waf-regional CreateIPSet / GetIPSet / DeleteIPSet signatures and error sets as quoted in the description.

## Steps to Reproduce
Boot the emulator (the waf-regional service crate is bootstrapped and answering; the CloudFormation crate is bootstrapped too) and run the check waf-regional.deploy-cfn-ipset-lifecycle, or reproduce by hand with botocore:

1. cfn = client('cloudformation'); waf = client('waf-regional'); ipset_name = unique(); stack_name = unique()
2. template = {'Resources': {'TestIPSet': {'Type': 'AWS::WAFRegional::IPSet', 'Properties': {'Name': ipset_name}}}, 'Outputs': {'IPSetId': {'Value': {'Ref': 'TestIPSet'}}}}
3. cfn.create_stack(StackName=stack_name, TemplateBody=json.dumps(template))
   -> expect HTTP 200 with a StackId; observe HTTP 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::WAFRegional::IPSet]'.
4. Once step 3 is accepted, wait for stack_create_complete and assert DescribeStacks(StackName=stack_name)['Stacks'][0]['StackStatus'] == 'CREATE_COMPLETE'.
5. outputs = {o['OutputKey']: o['OutputValue'] for o in desc.get('Outputs', [])}; waf.get_ip_set(IPSetId=outputs['IPSetId']) -> expect a 200 whose IPSet.IPSetId equals outputs['IPSetId'] and whose IPSet.Name equals ipset_name (i.e. the Ref is a minted IPSetId, not the template Name).
6. cfn.delete_stack(StackName=stack_name); wait for stack_delete_complete; waf.get_ip_set(IPSetId=<that id>) -> expect HTTP 400 WAFNonexistentItemException; then waf.update_ip_set(IPSetId=<that id>, ChangeToken=waf.get_change_token()['ChangeToken'], Updates=[{'Action': 'INSERT', 'IPSetDescriptor': {'Type': 'IPV4', 'Value': '203.0.113.0/32'}}]) -> expect HTTP 400 WAFNonexistentItemException or WAFNonexistentContainerException.
