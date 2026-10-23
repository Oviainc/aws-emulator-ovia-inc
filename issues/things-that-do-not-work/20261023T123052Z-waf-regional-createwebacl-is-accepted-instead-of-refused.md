Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: waf-regional.webacl-create-blocked-by-classic-sunset waf-regional.deploy-cfn-ipset-lifecycle
Issue-Shape-Id: com.amazonaws.wafregional#AWSWAF_Regional_20161128
Issue-Key: waf-regional.webacl-create-blocked-by-classic-sunset|_error#3|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: waf-regional|GetIPSet|WAFNonexistentItemException
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# waf-regional: CreateWebACL is accepted instead of refused by the WAF Classic sunset, and the CloudFormation AWS::WAFRegional::IPSet provider exports the template Name as its Ref so no IPSet is reachable (2 causes)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWebACL

## Description
Two distinct net-new root causes in the bootstrapped waf-regional emulator (crate services/service-com-amazonaws-wafregional-awswaf-regional-20161128) plus the CloudFormation resource provider that drives it. Neither is covered by an open issue: the open issue 20261022T031012Z-waf-regional-exception-messages-are-synthesized-from-the.md covers exception-message synthesis (causes 1-3 there) and, as its cause 4, a CFN stack that exposed NO Outputs at all; the CFN failure reported now is a DIFFERENT and later failure mode (the Output is present but carries the wrong value), and the suite did not mark the check as owned by that issue. The other open issue, 20261023T031252Z-waf-regional-getregexpatternset-and-listtagsforresource-responses-omit-modeled.md, concerns omitted response members and is unrelated. Every ALREADY FILED unimplemented-stub cluster (UpdateRuleGroup, PutPermissionPolicy, UpdateRateBasedRule, UpdateRegexPatternSet, UpdateSizeConstraintSet, DeleteRule, AssociateWebACL) is deliberately NOT duplicated here.

=== Cause 1 of 2: CreateWebACL succeeds with HTTP 200 where AWS WAF Classic refuses it unconditionally (WAFBadRequestException, HTTP 400) ===
AWS: Smithy (services/service-com-amazonaws-wafregional-awswaf-regional-20161128/smithy-model/com-amazonaws-wafregional-awswaf-regional-20161128.json): CreateWebACL models WAFBadRequestException (errorKind client, httpError 400 [defaulted], single `message` member) among its errors, and WafAction$Type documents "You can't specify COUNT for the default action for a WebACL", which WAFInvalidParameterException's documentation repeats ("You tried to create a WebACL with a DefaultAction Type other than ALLOW, BLOCK, or COUNT" / "You specified an invalid value"). Beyond the model this is recorded behavior only: AWS WAF Classic (v1, of which waf-regional is the regional half) reached end of support on 2025-09-30, and the recording shows an otherwise well-formed CreateWebACL with DefaultAction ALLOW or BLOCK refused unconditionally with HTTP 400 and code WAFBadRequestException — a service-lifecycle refusal, not a per-parameter validation error — reproducing identically across repeated attempts, while DefaultAction COUNT is refused earlier by ordinary parameter validation with HTTP 400 code WAFInvalidParameterException. The requirements are the statuses and the error codes; the recorded sunset sentence is an example of the message.
Emulator: CreateWebACL with DefaultAction ALLOW returns HTTP 200 and a full WebACL body (observed: WebACLId/Name/MetricName/DefaultAction/Rules/WebACLArn plus a ChangeToken), i.e. it mints and stores a WebACL record. The handler services/.../src/ops/create_web_acl.rs implements name validation, the COUNT exclusion, limits and the change-token protocol, but has no service-lifecycle refusal at all, so no WAFBadRequestException is ever constructed and the state is mutated. The check fails on the first iteration (ALLOW), so the BLOCK and COUNT legs were not reached in this run.
Why: a client exercising the post-sunset WAF Classic control plane is told the creation succeeded and is handed a WebACLId plus an ARN for a web ACL that cannot exist on real AWS. Code written or tested against the emulator takes the success branch, caches the id, and then fails in production where every CreateWebACL is refused; and because the emulator stores the record, ListWebACLs/GetWebACL report a resource real AWS would never have.
Fix: the CreateWebACL handler's validation/refusal path (services/.../src/ops/create_web_acl.rs `handle`): the lifecycle refusal must be returned before any state mutation or change-token consumption, so the request leaves no WebACL behind. Its status (400) and its code (WAFBadRequestException) are the requirement; the COUNT branch already present must keep answering WAFInvalidParameterException.
Traces: waf-regional.webacl-create-blocked-by-classic-sunset [mismatch].

=== Cause 2 of 2: a CloudFormation stack declaring AWS::WAFRegional::IPSet resolves {"Ref": <resource>} to the template's Name property, and no IPSet exists behind it ===
AWS: recorded behavior only for the CFN side. A stack whose template declares one AWS::WAFRegional::IPSet resource with a Name property and an Output {"Value": {"Ref": "TestIPSet"}} reaches CREATE_COMPLETE with that Output present, and the Output's value is the resource's physical id — the IPSetId of a real IPSet that the waf-regional control plane reads back with GetIPSet, returning IPSet.IPSetId equal to that value and IPSet.Name equal to the template's Name. Smithy: GetIPSet takes IPSetId (target ResourceId, required, len 1..128) and models WAFNonexistentItemException (errorKind client, httpError 400 [defaulted]) for an id it holds nothing for; CreateIPSet returns an IPSet whose IPSetId is the id the provider must publish as the Ref. On stack deletion the provider calls DeleteIPSet, after which GetIPSet on that id is refused WAFNonexistentItemException and UpdateIPSet against it is refused WAFNonexistentItemException or WAFNonexistentContainerException (both modeled on UpdateIPSet, errorKind client, httpError 400).
Emulator: the stack reaches CREATE_COMPLETE and now does expose the Output, but its value is the template's Name property rather than a minted IPSetId — the immediately following GetIPSet with that value raises HTTP 400 WAFNonexistentItemException (observed message example: "IPSet sim2real-behav-cfn-ipset-<suffix> does not exist"). So the provider publishes the logical/Name string as the resource's Ref and no CreateIPSet-backed resource is reachable under it; the delete-propagation and post-delete-mutation legs of the contract are unreachable. (This is a different failure point from the previously filed state of this check, which had no Outputs at all.)
Why: a client deploying WAF-Regional IPSets through CloudFormation gets a stack that reports success while exporting an id that resolves to nothing — every downstream use of the Ref (a nested stack parameter, a GetIPSet/UpdateIPSet call, an export consumed by another stack) fails with a not-found refusal, and stack deletion cannot be shown to clean up the underlying resource.
Fix: the CloudFormation resource provider for AWS::WAFRegional::IPSet and the physical-id/Ref resolution it feeds: the create path must call the waf-regional CreateIPSet state and register the returned IPSetId as the resource's physical id so Ref/Outputs resolve to it, and the delete path must call DeleteIPSet for that id. The waf-regional CreateIPSet/GetIPSet/DeleteIPSet state is the dependency it must drive, not the handler to change.
Traces: waf-regional.deploy-cfn-ipset-lifecycle [error].

## Evidence
Behavioral suite run (simparity-behavioral-aws-v1): waf-regional 8/24 passed (33%), 16 failed. After skipping every ALREADY FILED unimplemented-stub cluster and every check marked OWNED by an open issue, two net-new root causes survive (two further surviving mismatches are message-text-only and are not filed — see the run notes).

Cause 1 — waf-regional.webacl-create-blocked-by-classic-sunset [mismatch]
  call: waf.create_web_acl(Name=<n>, MetricName=<m>, DefaultAction={'Type': 'ALLOW'}, ChangeToken=<fresh>)
  expected: ClientError, HTTP 400, Error.Code == 'WAFBadRequestException' (recorded message example: the WAF Classic end-of-support sentence)
  actual:   HTTP 200 with a stored WebACL:
    {'WebACL': {'WebACLId': '00000000-0000-4000-8000-000000000002', 'Name': 'sim2real-behav-webacl-allow-<suffix>', 'MetricName': 'sim2realbehavwebaclallow<suffix>', 'DefaultAction': {'Type': 'ALLOW'}, 'Rules': [], 'WebACLArn': 'arn:aws:waf-regional:<region>:<account>:webacl/00000000-0000-4000-8000-000000000002'}, 'ChangeToken': '00000000-0000-4000-8000-000000000001'}
  harness: AssertionError: expected a ClientError, but the call succeeded and returned {...}
  (ids, names and the ARN above are environment-generated and are not requirements; the 200 status and the absence of any refusal are.)
  Source: services/service-com-amazonaws-wafregional-awswaf-regional-20161128/src/ops/create_web_acl.rs:224-330 — handle() validates MetricName, excludes COUNT as the default action (DEFAULT_ACTION_EXCLUDED, line 168/261), checks limits, consumes the change token and then inserts the record; it never constructs WAFBadRequestException even though CreateWebACLErrorModeled declares that variant (line 133).

Cause 2 — waf-regional.deploy-cfn-ipset-lifecycle [error]
  template: {'Resources': {'TestIPSet': {'Type': 'AWS::WAFRegional::IPSet', 'Properties': {'Name': <ipset-name>}}}, 'Outputs': {'IPSetId': {'Value': {'Ref': 'TestIPSet'}}}}
  DescribeStacks -> StackStatus == 'CREATE_COMPLETE' (asserted OK); Outputs['IPSetId'] == <ipset-name> (the template's Name, not an IPSetId)
  then: waf.get_ip_set(IPSetId=Outputs['IPSetId']) ->
    botocore.errorfactory.WAFNonexistentItemException: An error occurred (WAFNonexistentItemException) when calling the GetIPSet operation: IPSet sim2real-behav-cfn-ipset-cc388f700591 does not exist
  expected: GetIPSet succeeds with IPSet.IPSetId == the Output value and IPSet.Name == <ipset-name>; after delete_stack, GetIPSet on that id is refused HTTP 400 WAFNonexistentItemException and UpdateIPSet on it is refused WAFNonexistentItemException or WAFNonexistentContainerException. Those last two legs were never reached.

Smithy citations used (model path services/service-com-amazonaws-wafregional-awswaf-regional-20161128/smithy-model/com-amazonaws-wafregional-awswaf-regional-20161128.json): CreateWebACL errors include WAFBadRequestException and WAFInvalidParameterException (both errorKind client, httpError 400 defaulted); WafActionType enum values [BLOCK, ALLOW, COUNT]; WafAction$Type doc 'You can't specify COUNT for the default action for a WebACL'; GetIPSet input IPSetId (ResourceId, required) with error WAFNonexistentItemException (client, 400); CreateIPSet output IPSet + ChangeToken.

## Steps to Reproduce
Boot the emulator (the waf-regional service crate is bootstrapped and answering) and run the two waf-regional behavioral checks below, or reproduce by hand with botocore:

Cause 1 (waf-regional.webacl-create-blocked-by-classic-sunset):
1. waf = client('waf-regional')
2. for action in ('ALLOW', 'BLOCK'): name = unique(); waf.create_web_acl(Name=name, MetricName=name.replace('-','')[:128], DefaultAction={'Type': action}, ChangeToken=waf.get_change_token()['ChangeToken'])
   -> expect a ClientError with HTTP 400 and Error.Code == 'WAFBadRequestException' on every attempt; observe HTTP 200 and a WebACL body.
3. waf.create_web_acl(..., DefaultAction={'Type': 'COUNT'}, ChangeToken=<fresh>) -> expect HTTP 400 WAFInvalidParameterException (this leg is implemented; it is listed only to pin the ordering of the two refusals).
4. existing = {a['Name'] for a in waf.list_web_acls(Limit=100)['WebACLs']} -> none of the attempted names may appear; after the refusal no WebACL may have been stored and the change token must remain unconsumed.

Cause 2 (waf-regional.deploy-cfn-ipset-lifecycle):
1. cfn = client('cloudformation'); waf = client('waf-regional'); ipset_name = unique()
2. cfn.create_stack(StackName=unique(), TemplateBody=json.dumps({'Resources': {'TestIPSet': {'Type': 'AWS::WAFRegional::IPSet', 'Properties': {'Name': ipset_name}}}, 'Outputs': {'IPSetId': {'Value': {'Ref': 'TestIPSet'}}}}))
3. wait for stack_create_complete; outputs = {o['OutputKey']: o['OutputValue'] for o in cfn.describe_stacks(StackName=<s>)['Stacks'][0]['Outputs']}
4. waf.get_ip_set(IPSetId=outputs['IPSetId']) -> expect a 200 whose IPSet.IPSetId equals outputs['IPSetId'] and whose IPSet.Name equals ipset_name; observe HTTP 400 WAFNonexistentItemException because outputs['IPSetId'] is ipset_name rather than a minted IPSetId.
5. cfn.delete_stack(StackName=<s>); wait for stack_delete_complete; waf.get_ip_set(IPSetId=<that id>) -> expect HTTP 400 WAFNonexistentItemException; then waf.update_ip_set(IPSetId=<that id>, ChangeToken=<fresh>, Updates=[{'Action':'INSERT','IPSetDescriptor':{'Type':'IPV4','Value':'203.0.113.0/32'}}]) -> expect HTTP 400 WAFNonexistentItemException or WAFNonexistentContainerException.
