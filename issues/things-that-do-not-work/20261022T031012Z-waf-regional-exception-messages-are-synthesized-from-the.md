Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: waf-regional.get-nonexistent-ipset-not-found waf-regional.delete-nonempty-entity-rejected waf-regional.stale-change-token-reused-rejected waf-regional.deploy-cfn-ipset-lifecycle
Issue-Shape-Id: com.amazonaws.wafregional#AWSWAF_Regional_20161128
Issue-Key: waf-regional.get-nonexistent-ipset-not-found|run#4|AssertionError|assert err == ("WAFNonexistentItemException", "The referenced item does not exist."), f"expected WAFNonexistentItemException, got {err}"
Issue-Key: waf-regional.delete-nonempty-entity-rejected|run#8|AssertionError|assert err == ("WAFNonEmptyEntityException", "The referenced entity is not empty."), err
Issue-Key: waf-regional.stale-change-token-reused-rejected|run#8|AssertionError|assert err == ("WAFStaleDataException", "The input token is no longer current."), err
Issue-Key: waf-regional.deploy-cfn-ipset-lifecycle|run#13|KeyError|ip_set_id = outputs["IPSetId"]
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# waf-regional: exception messages are synthesized from the request instead of WAF Classic's fixed per-exception sentence, and CFN AWS::WAFRegional::IPSet provisions no referenceable resource (4 causes)

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIPSet

## Description
Four distinct root causes in the bootstrapped waf-regional emulator (service crate services/service-com-amazonaws-wafregional-awswaf-regional-20161128). Causes 1-3 share a family (error-message construction) but each lives in a different handler and each must be closed separately; cause 4 is the CloudFormation resource provider for the WAF-Regional IPSet type. No open issue covers any of them (list_existing_issues shows no waf-regional fidelity issue; the only waf-regional issues on file are the auto-filed 'unimplemented scaffold stub' ones for CreateByteMatchSet/CreateRule/CreateGeoMatchSet/CreateRuleGroup/CreateRateBasedRule/ListRateBasedRules/CreateRegexPatternSet/CreateSizeConstraintSet/CreateSqlInjectionMatchSet/CreateWebACL/GetWebACL/CreateXssMatchSet, which this issue deliberately does not duplicate).

=== Cause 1 of 4: GetIPSet WAFNonexistentItemException message echoes the requested id ===
AWS: recorded behavior only — the Smithy model (shape WAFNonexistentItemException: structure, errorKind client, httpError 400 [defaulted], single member `message`) documents only the prose 'The operation failed because the referenced object doesn't exist.' and says nothing about the wire text. The recording shows AWS WAF Classic answering GetIPSet for an id it holds nothing for with HTTP 400, code WAFNonexistentItemException and a FIXED, request-independent sentence — the identical string for a never-created id and for an id whose IPSet was deleted ('The referenced item does not exist.' is the recorded example of that constant). The requirement is the rule: one constant sentence per exception type, carrying no request-derived content.
Emulator: returns the right status and the right code, but builds the message from the input — services/.../src/ops/get_ip_set.rs:156-160 does `format!("IPSet {} does not exist", input.ip_set_id)`, observed as 'IPSet 00000000-0000-0000-0000-000000000000 does not exist'.
Why: the message varies with the request where AWS's does not, so any client, log scraper or test that matches WAF Classic's fixed not-found text (and anything that treats the message as non-identifying) sees a different string per call, and the emulator additionally echoes caller-supplied ids back into the error body.
Fix: the WAFNonexistentItemException construction in the GetIPSet handler (and the shared message source it should draw the constant from), not its status or code — those already match.
Traces: waf-regional.get-nonexistent-ipset-not-found.

=== Cause 2 of 4: DeleteIPSet WAFNonEmptyEntityException message is synthesized and names the id and descriptor count ===
AWS: recorded behavior only — model shape WAFNonEmptyEntityException: errorKind client, httpError 400 [defaulted], single `message` member, doc 'The operation failed because you tried to delete an object that isn't empty... You tried to delete an IPSet that references one or more IP addresses.'; the model does not state the wire text. The recording shows DeleteIPSet on an IPSet that still holds IPSetDescriptors answered with HTTP 400, code WAFNonEmptyEntityException and a fixed request-independent sentence (recorded example: 'The referenced entity is not empty.').
Emulator: correct status and code, but a synthesized message naming the resource id and a count: 'IPSet 00000000-0000-4000-8000-000000000002 still includes 1 IP address range(s); remove them with UpdateIPSet before deleting it'.
Why: same as cause 1 — the per-exception constant is replaced by request-derived prose, so the error body is not the stable value AWS emits for this refusal class.
Fix: the WAFNonEmptyEntityException construction in the DeleteIPSet handler.
Traces: waf-regional.delete-nonempty-entity-rejected.

=== Cause 3 of 4: CreateIPSet WAFStaleDataException message is synthesized and echoes the change token ===
AWS: recorded behavior only — model shape WAFStaleDataException: errorKind client, httpError 400 [defaulted], single `message` member, doc 'The operation failed because you tried to create, update, or delete an object by using a change token that has already been used.'; wire text not modeled. The recording shows the reuse of an already-consumed ChangeToken answered with HTTP 400, code WAFStaleDataException and a fixed request-independent sentence (recorded example: 'The input token is no longer current.').
Emulator: correct status and code, but 'The change token 00000000-0000-4000-8000-000000000001 is not a change token this request can use; call GetChangeToken for a new one' — built from the submitted token.
Why: same family; additionally the caller's change token is reflected back into the error body, which AWS's constant never does.
Fix: the WAFStaleDataException construction in the change-token validation shared by the Create*/Update*/Delete* handlers.
Traces: waf-regional.stale-change-token-reused-rejected.

=== Cause 4 of 4: a CloudFormation stack declaring AWS::WAFRegional::IPSet reaches CREATE_COMPLETE but exposes no Ref value, so no IPSet is provisioned or referenceable ===
AWS: recorded behavior only for the CFN side — a stack whose template declares one AWS::WAFRegional::IPSet resource and an Output `{"Value": {"Ref": "TestIPSet"}}` reaches CREATE_COMPLETE with that Output present in DescribeStacks, its value being the IPSetId of a real IPSet that the waf-regional control plane can read with GetIPSet (same Name as the template's Name property). On stack deletion the provider calls DeleteIPSet, after which GetIPSet on that id is refused with WAFNonexistentItemException (HTTP 400) and a mutation (UpdateIPSet) against it is refused with WAFNonexistentItemException or WAFNonexistentContainerException (both modeled on UpdateIPSet, errorKind client, httpError 400).
Emulator: DescribeStacks reports StackStatus CREATE_COMPLETE (that assertion passes) but the stack's Outputs do not contain the key at all — the check fails with KeyError: 'IPSetId' when building the output map from DescribeStacks, i.e. the Outputs section / the {"Ref": <WAF resource>} resolution produces nothing. Because no id is returned, nothing was provisioned through CreateIPSet and the delete-propagation and post-delete-mutation legs of the contract were never reachable.
Why: a client deploying WAF-Regional IPSets through CloudFormation gets a stack that reports success while provisioning no usable resource and exporting no id — downstream stacks/parameters wired to the Ref break, and orphan/cleanup behavior on stack delete is unverifiable.
Fix: the CloudFormation resource provider for AWS::WAFRegional::IPSet (its create path must call the waf-regional CreateIPSet state and register the IPSetId as the resource's Ref/physical id) plus the stack Outputs evaluation that resolves Ref for it; the waf-regional CreateIPSet/GetIPSet/DeleteIPSet state is the dependency it must drive.
Traces: waf-regional.deploy-cfn-ipset-lifecycle.

## Evidence
Behavioral suite: waf-regional 3/24 passed (12%), 21 failed. Four failures survive after skipping every ALREADY FILED unimplemented-stub cluster.

Cause 1 — waf-regional.get-nonexistent-ipset-not-found [mismatch]
  expected: ('WAFNonexistentItemException', 'The referenced item does not exist.')
  actual:   ('WAFNonexistentItemException', 'IPSet 00000000-0000-0000-0000-000000000000 does not exist')
  code and HTTP 400 match; message is request-derived. Source: services/service-com-amazonaws-wafregional-awswaf-regional-20161128/src/ops/get_ip_set.rs:156-160 -> format!("IPSet {} does not exist", input.ip_set_id).

Cause 2 — waf-regional.delete-nonempty-entity-rejected [mismatch]
  expected: ('WAFNonEmptyEntityException', 'The referenced entity is not empty.')
  actual:   ('WAFNonEmptyEntityException', 'IPSet 00000000-0000-4000-8000-000000000002 still includes 1 IP address range(s); remove them with UpdateIPSet before deleting it')

Cause 3 — waf-regional.stale-change-token-reused-rejected [mismatch]
  expected: ('WAFStaleDataException', 'The input token is no longer current.')
  actual:   ('WAFStaleDataException', 'The change token 00000000-0000-4000-8000-000000000001 is not a change token this request can use; call GetChangeToken for a new one')

Cause 4 — waf-regional.deploy-cfn-ipset-lifecycle [error]
  StackStatus == CREATE_COMPLETE asserted OK, then:
    File "check_waf_regional_deploy_cfn_ipset_lifecycle.py", line 62, in run
      ip_set_id = outputs["IPSetId"]
    KeyError: 'IPSetId'
  i.e. {o["OutputKey"]: o["OutputValue"] for o in desc.get("Outputs", [])} has no IPSetId, so the stack exported no Ref for the AWS::WAFRegional::IPSet resource and GetIPSet/DeleteIPSet propagation was never exercised.

Smithy (services/service-com-amazonaws-wafregional-awswaf-regional-20161128/smithy-model/com-amazonaws-wafregional-awswaf-regional-20161128.json): WAFNonexistentItemException, WAFNonEmptyEntityException, WAFStaleDataException, WAFNonexistentContainerException are all errorKind=client, httpError=400 (defaulted), with a single `message` member and no modeled message text — hence 'recorded behavior only' for the message rule; the recorded constant per exception type is the requirement, the quoted sentences are the recorded examples.

## Steps to Reproduce
Boot the emulator and run the waf-regional behavioral checks:

1. Cause 1: waf = client('waf-regional'); waf.get_ip_set(IPSetId='00000000-0000-0000-0000-000000000000') -> expect HTTP 400 WAFNonexistentItemException with WAF Classic's fixed not-found sentence; observe an id-echoing message. Repeat after create_ip_set + delete_ip_set on the real id: the message must be byte-identical to the never-created case.
2. Cause 2: t = waf.get_change_token()['ChangeToken']; id = waf.create_ip_set(Name=<n>, ChangeToken=t)['IPSet']['IPSetId']; waf.update_ip_set(IPSetId=id, ChangeToken=<fresh>, Updates=[{'Action':'INSERT','IPSetDescriptor':{'Type':'IPV4','Value':'198.51.100.7/32'}}]); waf.delete_ip_set(IPSetId=id, ChangeToken=<fresh>) -> expect HTTP 400 WAFNonEmptyEntityException with the fixed non-empty sentence; observe an id/count-bearing message.
3. Cause 3: t = waf.get_change_token()['ChangeToken']; waf.create_ip_set(Name=<a>, ChangeToken=t); waf.create_ip_set(Name=<b>, ChangeToken=t) -> expect HTTP 400 WAFStaleDataException with the fixed stale-token sentence; observe a token-echoing message.
4. Cause 4: cfn.create_stack(StackName=<s>, TemplateBody=json.dumps({'Resources':{'TestIPSet':{'Type':'AWS::WAFRegional::IPSet','Properties':{'Name':<ipset-name>}}},'Outputs':{'IPSetId':{'Value':{'Ref':'TestIPSet'}}}})); wait stack_create_complete; cfn.describe_stacks(StackName=<s>)['Stacks'][0]['Outputs'] -> expect an IPSetId output whose value GetIPSet resolves to an IPSet named <ipset-name>; observe no such output. Then delete_stack and confirm GetIPSet on that id is refused WAFNonexistentItemException and UpdateIPSet on it is refused WAFNonexistentItemException or WAFNonexistentContainerException.
