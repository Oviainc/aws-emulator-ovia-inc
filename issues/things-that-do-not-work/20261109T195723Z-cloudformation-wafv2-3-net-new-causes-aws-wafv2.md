Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__wafv2.deploy-loggingconfiguration-provisioned-then-removed cloudformation__wafv2.deploy-rulegroup-cfn-capacity-too-low-rejected cloudformation__wafv2.deploy-webacl-rulegroup-ipset-regexpatternset-provisioned-then-removed
Issue-Key: cloudformation__wafv2|CreateStack|ValidationError
Issue-Key: cloudformation__wafv2.deploy-rulegroup-cfn-capacity-too-low-rejected|run#9|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Key: cloudformation__wafv2.deploy-webacl-rulegroup-ipset-regexpatternset-provisioned-then-removed|run#12|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Commit: 2f19ef1c319cb432a408a2dffd91968e32ad629f

# cloudformation__wafv2: 3 net-new causes — AWS::WAFv2::LoggingConfiguration is an unregistered resource type, a refused WAFv2 resource's CREATE_FAILED reason omits the AWS SDK exception suffix, and AWS::WAFv2::RegexPatternSet always fails to create

> _Note: this issue was identified against the code base at commit `2f19ef1c319cb432a408a2dffd91968e32ad629f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three net-new root causes in the CloudFormation-provisioned WAFv2 surface, each grounded in a failing behavioral check of cloudformation__wafv2. Related open issue which does NOT cover any of them: 20261107T153122Z-wafv2-a-cloudformation-stack-of-aws-wafv2-ipset.md (an IPSet+WebACL+WebACLAssociation stack rolling back, keyed on check wafv2.deploy-ipset-cfn-blocks-listed-address); the handler-reason formatting cause below is the same family as 20261109T155522Z-cloudformation-ses-... and 20261109T155854Z-cloudformation-fis-..., but for the WAFv2 resource providers, which those issues do not name.

Cause 1 of 3: the template processor does not know the resource type AWS::WAFv2::LoggingConfiguration.
AWS: CreateStack accepts a template whose Resources declare AWS::WAFv2::LoggingConfiguration (ResourceArn of a web ACL plus LogDestinationConfigs, RedactedFields, LoggingFilter) and provisions it by calling the modelled WAFv2 control-plane operation PutLoggingConfiguration (Smithy com.amazonaws.wafv2#AWSWAF_20190729: PutLoggingConfiguration, input LoggingConfiguration required, whose refusals are modelled 400s — WAFInvalidParameterException, WAFNonexistentItemException, WAFLimitsExceededException, WAFOptimisticLockException, WAFServiceLinkedRoleErrorException). The stack then reaches CREATE_COMPLETE and deleting it removes the logging configuration. The registration of the type itself is recorded behavior only: AWS accepts the template rather than refusing it.
Emulator: CreateStack is refused outright, so the stack is never created: HTTP 400, error code `ValidationError`, message `Template format error: Unrecognized resource types: [AWS::WAFv2::LoggingConfiguration]`. Fixed text of that message: `Template format error: Unrecognized resource types: [` + the unregistered type names + `]`; the bracketed type list is the variable part, taken from the request's template.
Why: WAF logging cannot be declared as infrastructure-as-code at all. A client whose template carries a LoggingConfiguration alongside its WebACL gets no stack — not a partial one — so every downstream resource in that template is unreachable too, and the failure arrives as a template-format error that no amount of correcting the template can clear.
Fix: the CloudFormation template-processor's resource-type registry and a resource provider for AWS::WAFv2::LoggingConfiguration (create/update/delete handlers mapping the template properties onto PutLoggingConfiguration / DeleteLoggingConfiguration, plus the wafv2 state they write).
Traces: cloudformation__wafv2.deploy-loggingconfiguration-provisioned-then-removed.

Cause 2 of 3: the ResourceStatusReason recorded on a CREATE_FAILED WAFv2 resource carries the WAF message but not the AWS SDK exception suffix.
AWS: when a WAFv2 resource provider's control-plane call is refused, CloudFormation records the reason as the handler message followed by the SDK's exception suffix, i.e. `<WAF message> (Service: Wafv2, Status Code: <status>, Request ID: <id>) (SDK Attempt Count: <n>)`, and the row's HandlerErrorCode is `InvalidRequest`. For a RuleGroup whose declared Capacity is below what its rules cost, the WAF message is WAFLimitsExceededException's own sentence with its reason/field/parameter clauses: `Error reason: You exceeded the capacity limit for a rule group or web ACL., field: RULE_GROUP, parameter: <declared capacity>` (Smithy: CreateRuleGroup, Capacity required @range min 1, error WAFLimitsExceededException, a modelled 400). Fixed text: `Error reason: You exceeded the capacity limit for a rule group or web ACL., field: RULE_GROUP, parameter: `, then the declared Capacity from the request; and the parenthesised suffix with `Service: Wafv2`, the HTTP status of the refusal, and the per-call request id and attempt count. The per-run id/attempt-count values are not the requirement — the presence and shape of the two parenthesised clauses are.
Emulator: the reason stops after the WAF message — `('Error reason: You exceeded the capacity limit for a rule group or web ACL., field: RULE_GROUP, parameter: 10', 'InvalidRequest')` where AWS gives `('Error reason: You exceeded the capacity limit for a rule group or web ACL., field: RULE_GROUP, parameter: 10 (Service: Wafv2, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest')`. The stack status (ROLLBACK_COMPLETE), the HandlerErrorCode and the WAF sentence are all correct; only the suffix is missing.
Why: tooling and operators parse the suffix to recover the originating service, HTTP status and request id of a failed resource — that is how a CFN rollback is correlated to the service call that caused it. A client scraping DescribeStackEvents reasons for `Service: ...` / `Status Code: ...` finds nothing and cannot classify or escalate the failure.
Fix: the CloudFormation resource-provider error path that formats a downstream service refusal into ResourceStatusReason for the AWS::WAFv2::* providers (the same formatting the other service providers need), not the wafv2 refusal itself, which is already correct.
Traces: cloudformation__wafv2.deploy-rulegroup-cfn-capacity-too-low-rejected.

Cause 3 of 3: an AWS::WAFv2::RegexPatternSet resource fails to create, rolling the whole stack back.
AWS: a stack declaring AWS::WAFv2::RegexPatternSet (Name, Scope REGIONAL, RegularExpressionList as a list of plain pattern strings, Tags) reaches CREATE_COMPLETE; the provider translates the template's list of strings into the API's RegularExpressionList of Regex members, so GetRegexPatternSet afterwards returns `RegularExpressionList == [{"RegexString": "<pattern>"}]` (Smithy: CreateRegexPatternSet — Name required @length 1..128 @pattern ^[\w\-]+$, Scope required, RegularExpressionList required; refusals WAFInvalidParameterException / WAFDuplicateItemException / WAFLimitsExceededException, all modelled 400s). Ref is `<Name>|<Id>|<Scope>` and the Arn/Id are readable through Fn::GetAtt.
Emulator: the RegexSet resource goes CREATE_FAILED and is then DELETE_COMPLETE, and the stack settles ROLLBACK_COMPLETE instead of CREATE_COMPLETE — decisive event rows `[('RegexSet', 'CREATE_FAILED'), ('RegexSet', 'DELETE_COMPLETE'), ('<stack name>', 'ROLLBACK_COMPLETE')]`. Every later assertion (the four Refs, the ARNs, the tags, the WebACL's RegexPatternSetReferenceStatement, and the post-delete WAFNonexistentItemException reads) is therefore never reached. The replay carries no ResourceStatusReason text, so the refusal behind CREATE_FAILED is not stated by the recording. Hypothesis: the provider forwards the template's list of bare pattern strings to CreateRegexPatternSet unchanged instead of wrapping each in a Regex member, so the call is refused; a fixer should read the CREATE_FAILED row's ResourceStatusReason to confirm which call failed and with which WAF error.
Why: no WAFv2 stack that includes a regex pattern set can be deployed, and because CloudFormation rolls back the whole stack, the WebACL, RuleGroup and IPSet declared next to it are destroyed too. A client templating a complete WAF gets nothing deployed and no modelled WAF error to act on.
Fix: the AWS::WAFv2::RegexPatternSet resource provider — its create handler and the property mapping from the template's RegularExpressionList of strings onto the API's Regex members, plus the physical-resource-id/attribute resolution (Ref, Arn, Id) the rest of the template reads.
Traces: cloudformation__wafv2.deploy-webacl-rulegroup-ipset-regexpatternset-provisioned-then-removed.

## Evidence
Cause 1 — check cloudformation__wafv2.deploy-loggingconfiguration-provisioned-then-removed, verdict [error]:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::WAFv2::LoggingConfiguration]
Expected: CreateStack is accepted, the stack reaches CREATE_COMPLETE, WAFv2 GetLoggingConfiguration reports the configuration for the stack's web ACL, and deleting the stack removes it.
Actual: CreateStack is refused with ValidationError / "Unrecognized resource types"; no stack exists.

Cause 2 — check cloudformation__wafv2.deploy-rulegroup-cfn-capacity-too-low-rejected, verdict [mismatch]:
  AssertionError: handler reason ('Error reason: You exceeded the capacity limit for a rule group or web ACL., field: RULE_GROUP, parameter: 10', 'InvalidRequest'), expected ('Error reason: You exceeded the capacity limit for a rule group or web ACL., field: RULE_GROUP, parameter: 10 (Service: Wafv2, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest')
(The check masks Request ID and SDK Attempt Count before comparing, so only the presence and fixed text of the two parenthesised clauses are asserted. `parameter: 10` is the Capacity the template declared, not a constant.)
Expected: the CREATE_FAILED row's ResourceStatusReason ends with ` (Service: Wafv2, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)`.
Actual: the reason ends at the WAF sentence; both clauses are absent.

Cause 3 — check cloudformation__wafv2.deploy-webacl-rulegroup-ipset-regexpatternset-provisioned-then-removed, verdict [mismatch]:
  AssertionError: stack ended 'ROLLBACK_COMPLETE': [('RegexSet', 'CREATE_FAILED'), ('RegexSet', 'DELETE_COMPLETE'), ('sim2real-xsvc-wafv2-stack-845a02423220', 'ROLLBACK_COMPLETE')]
Expected: StackStatus CREATE_COMPLETE, then each of RegexSet/IPSet/RuleGroup/WebACL listed by its WAFv2 lister with the Id and ARN the Outputs publish, Ref == "<Name>|<Id>|REGIONAL", and GetRegexPatternSet returning RegularExpressionList == [{"RegexString": "^/blocked$"}].
Actual: the RegexPatternSet resource fails its CREATE, the stack rolls back, and none of the four resources exists. The stack name in the row above is a per-run value and is not part of the requirement.

## Steps to Reproduce
Cause 1:
1. CreateStack with a template whose Resources include an AWS::WAFv2::WebACL and an AWS::WAFv2::LoggingConfiguration (ResourceArn = Fn::GetAtt <WebACL>.Arn, LogDestinationConfigs = [a CloudWatch Logs group ARN named aws-waf-logs-*], plus RedactedFields).
2. The call is refused with ValidationError "Template format error: Unrecognized resource types: [AWS::WAFv2::LoggingConfiguration]" instead of creating the stack.
3. Or replay check cloudformation__wafv2.deploy-loggingconfiguration-provisioned-then-removed.

Cause 2:
1. CreateStack with a single AWS::WAFv2::RuleGroup (Scope REGIONAL, VisibilityConfig, one Rule with a ByteMatchStatement that costs 10 WCU) but Capacity: 1.
2. Poll DescribeStacks until ROLLBACK_COMPLETE (that part is correct).
3. DescribeStackEvents and read the ResourceStatusReason on the RuleGroup's CREATE_FAILED row: it stops after "... field: RULE_GROUP, parameter: 1" and omits " (Service: Wafv2, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)".
4. Or replay check cloudformation__wafv2.deploy-rulegroup-cfn-capacity-too-low-rejected.

Cause 3:
1. CreateStack with a template declaring AWS::WAFv2::RegexPatternSet (Scope REGIONAL, RegularExpressionList: ["^/blocked$"], Tags), AWS::WAFv2::IPSet, AWS::WAFv2::RuleGroup and an AWS::WAFv2::WebACL whose rules reference the three by Fn::GetAtt <logical id>.Arn, with Outputs publishing each Ref/Arn/Id.
2. Poll DescribeStacks: the stack reaches ROLLBACK_COMPLETE.
3. DescribeStackEvents: the RegexSet row is CREATE_FAILED then DELETE_COMPLETE — read its ResourceStatusReason to see which WAFv2 call was refused.
4. Or replay check cloudformation__wafv2.deploy-webacl-rulegroup-ipset-regexpatternset-provisioned-then-removed.
