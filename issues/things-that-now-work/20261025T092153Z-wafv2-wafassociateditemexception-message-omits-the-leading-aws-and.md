Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.delete-webacl-blocked-while-associated-rejected wafv2.webacl-delete-blocked-while-associated-to-resource wafv2.deploy-webacl-cfn-blocks-matching-request wafv2.deploy-webacl-cfn-enforces-default-block-action
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.delete-webacl-blocked-while-associated-rejected|run#20|AssertionError|assert message == (
Issue-Key: wafv2.webacl-delete-blocked-while-associated-to-resource|run#19|AssertionError|assert error.get("Message") == _ASSOCIATED_MESSAGE, f"unexpected message: {error}"
Issue-Key: wafv2|AssociateWebACL|WAFInvalidParameterException
Issue-Key: wafv2|GetWebACL|WAFInvalidParameterException
Issue-Commit: dd185e64193b12242704f82dcc412d70cd33b9eb

# wafv2: WAFAssociatedItemException message omits the leading "AWS ", and a CloudFormation-published AWS::WAFv2::WebACL Arn is refused by wafv2's own AssociateWebACL/GetWebACL with WAFInvalidParameterException (2 causes)

> _Note: this issue was identified against the code base at commit `dd185e64193b12242704f82dcc412d70cd33b9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteWebACL

## Description
This bundle has 2 net-new root causes in the wafv2 service, each grounded in failing behavioral checks of this suite.

Cause 1 of 2: the constant message AWS returns for WAFAssociatedItemException is emitted without its leading "AWS ".
AWS: DeleteWebACL on a web ACL that is still associated with a protected resource is refused with HTTP 400 and error code WAFAssociatedItemException, and this exception carries a fixed, request-independent sentence (one message per exception shape, never parameterized by the request). Smithy (com.amazonaws.wafv2): DeleteWebACL models WAFAssociatedItemException, errorKind client, httpError 400; the shape is documented as "WAF couldn’t perform the operation because your resource is being used by another resource or it’s associated with another resource." The wire wording real AWS emits prefixes the service name: recorded in two independent checks as "AWS WAF couldn’t perform the operation because your resource is being used by another resource or it’s associated with another resource." The requirement is therefore "the constant AWS-worded sentence for this exception type", of which that recorded string is the example; the status (400) and the code are requirements outright.
Emulator: status and code are correct (400 / WAFAssociatedItemException) and the web ACL is correctly not deleted, but the Message is "WAF couldn’t perform the operation because your resource is being used by another resource or it’s associated with another resource." — the Smithy doc-string wording, missing the "AWS " prefix that real WAF emits. Observed identically from two different checks, so it is a constant in the emulator, not a request artifact.
Why: a client or operator that matches, logs or surfaces the documented WAF sentence (and any assertion harness pinned to it) sees text that never appears on real WAF. This is the same class as the already-fixed issue 20261022T220318Z-wafv2-exception-messages-are-emulator-authored-instead-of.md — that fix replaced the emulator's request-derived sentences with the Smithy doc strings, which for the WAF exception family are one token short of the wire text; WAFAssociatedItemException was not in that issue's key set and still diverges.
Fix: the wafv2 error serializer's constant-message table (the entry for WAFAssociatedItemException), shared by the DeleteWebACL / DeleteIPSet / DeleteRegexPatternSet associated-item refusal paths.
Traces: wafv2.delete-webacl-blocked-while-associated-rejected, wafv2.webacl-delete-blocked-while-associated-to-resource.

Cause 2 of 2: the web-ACL ARN a CloudFormation stack publishes for AWS::WAFv2::WebACL does not have the WAFv2 web-ACL ARN layout, and wafv2 then refuses that ARN as malformed.
AWS: a stack-created AWS::WAFv2::WebACL is a real WAFv2 web ACL, and GetAtt Arn resolves against the WAFv2 control plane — the ARN carries the scope, the entity name and the entity Id, so it can be decomposed into the triple the API needs and handed straight to AssociateWebACL(WebACLArn=…) and GetWebACL(ARN=…). Smithy: AssociateWebACL.WebACLArn and GetWebACL.ARN both target ResourceArn (@length 20..2048); WAFInvalidParameterException (errorKind client, httpError 400) is documented for "Your request references an ARN that is malformed, or corresponds to a resource with which a web ACL can't be associated", and its Field member takes ParameterExceptionField RESOURCE_ARN. Recorded behavior only for the exact ARN text; what is structural is that the published ARN is an ARN the service accepts.
Emulator: GetAtt Arn yields an ARN of the form arn:aws:wafv2:<region>:<account>:web-acl/<name> — no scope segment, no /webacl/<name>/<id> tail and no entity id at all. Passing it straight back into wafv2 is then refused by the emulator's own validator with HTTP 400 WAFInvalidParameterException, Reason "The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes.", Field RESOURCE_ARN, Parameter set to that same ARN — on AssociateWebACL in one check and on GetWebACL in the other. So the emulator publishes an identifier it immediately rejects.
Why: no IaC-deployed WAFv2 web ACL can be associated with a protected resource or read back; every template that passes GetAtt Arn on to a WAFv2 call, a WebACLAssociation or a logging configuration hands on an ARN that identifies nothing, and the whole deploy-then-enforce path is dead at the first control-plane call.
Fix: the CloudFormation WAFv2 resource provider's attribute resolution for AWS::WAFv2::WebACL — it must surface the ARN minted by the wafv2 CreateWebACL state (name + id, with the scope segment) instead of fabricating one; the wafv2 ARN parser used by AssociateWebACL/GetWebACL is the component that proves it.
Related open issue, which does NOT carry these two check ids: 20261024T065754Z-wafv2-cloudformation-deployed-wafv2-resources-are-not-real-waf.md describes the same malformed GetAtt Arn as its Cause 2, grounded in wafv2.dsw-e2e-list-waf-webacl-findings-lifecycle, and lists wafv2.deploy-webacl-cfn-enforces-default-block-action only under a timeout key that no longer reproduces; wafv2.deploy-webacl-cfn-blocks-matching-request is not referenced by it at all. Both checks now fail earlier and harder (a 400 refusal rather than a hang), so they are filed here with their own replay keys.
Traces: wafv2.deploy-webacl-cfn-blocks-matching-request, wafv2.deploy-webacl-cfn-enforces-default-block-action.

## Evidence
Cause 1 — wafv2.delete-webacl-blocked-while-associated-rejected: AssertionError on the message assertion; the error returned was {'Code': 'WAFAssociatedItemException', 'Message': 'WAF couldn’t perform the operation because your resource is being used by another resource or it’s associated with another resource.'}. wafv2.webacl-delete-blocked-while-associated-to-resource: "AssertionError: unexpected message: {'Message': 'WAF couldn’t perform the operation because your resource is being used by another resource or it’s associated with another resource.', 'Code': 'WAFAssociatedItemException'}". Expected (recorded from real AWS by both checks): the same sentence prefixed "AWS WAF couldn’t perform the operation because …". Status 400 and code WAFAssociatedItemException matched, and the positive controls (the web ACL survives the rejected delete, and deletion succeeds after DisassociateWebACL) are not what failed.

Cause 2 — wafv2.deploy-webacl-cfn-blocks-matching-request: botocore.errorfactory.WAFInvalidParameterException: An error occurred (WAFInvalidParameterException) when calling the AssociateWebACL operation: Error reason: The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes., field: RESOURCE_ARN, parameter: arn:aws:wafv2:us-west-2:000000000000:web-acl/sim2real-waf-block-8da04ef26959. wafv2.deploy-webacl-cfn-enforces-default-block-action: the same exception on GetWebACL, parameter: arn:aws:wafv2:us-west-2:000000000000:web-acl/sim2real-behav-block-930e0b4febca. In both cases the ARN came from the stack's own Outputs (!GetAtt <WebACL>.Arn). Expected (AWS): AssociateWebACL returns 200 and GetWebACL returns the web ACL, because the published ARN carries the scope, the name and the entity Id.

## Steps to Reproduce
Cause 1:
1. wafv2 CreateWebACL (Scope=REGIONAL, DefaultAction Allow); cognito-idp CreateUserPool; AssociateWebACL(WebACLArn=<acl arn>, ResourceArn=<pool arn>).
2. GetWebACL to read the LockToken, then DeleteWebACL with it while the association is live.
3. The call is refused 400 WAFAssociatedItemException as it should be, but compare $.Message with real AWS: it is missing the leading "AWS ".
4. Or replay wafv2.delete-webacl-blocked-while-associated-rejected and wafv2.webacl-delete-blocked-while-associated-to-resource.

Cause 2:
1. Create a REST API with a MOCK integration and deploy it to a stage.
2. CreateStack with a template containing one AWS::WAFv2::WebACL (Scope REGIONAL, a header-match Block rule) and Outputs WebACLArn: !GetAtt <WebACL>.Arn and WebACLId: !GetAtt <WebACL>.Id; wait for CREATE_COMPLETE and read the outputs.
3. Call wafv2 AssociateWebACL(WebACLArn=<that output>, ResourceArn=<stage arn>) — refused with 400 WAFInvalidParameterException / Field RESOURCE_ARN naming that very ARN. Calling GetWebACL(ARN=<that output>) is refused the same way.
4. Note the output ARN has the shape arn:aws:wafv2:<region>:<account>:web-acl/<name> with no scope segment and no entity id.
5. Or replay wafv2.deploy-webacl-cfn-blocks-matching-request and wafv2.deploy-webacl-cfn-enforces-default-block-action.
