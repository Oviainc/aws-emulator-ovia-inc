Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-cloudfront-scope-outside-us-east-1-rejected
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-cloudfront-scope-outside-us-east-1-rejected wafv2-managed-rule-group-and-marketplace-product-discovery wafv2-managed-rule-set-vendor-operations-on-fresh-account wafv2-regex-pattern-set-lifecycle wafv2-rulegroup-lifecycle-with-permission-policy-errors wafv2-settlement-records-workflow wafv2-traffic-path-and-settlement-analytics wafv2-error-paths-and-validation wafv2-wafv2-error-paths
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|GetIPSet|4xx->4xx|$.Field
Issue-Key: wafv2|GetIPSet|4xx->4xx|$.Parameter
Issue-Key: wafv2|GetIPSet|4xx->4xx|$.Reason
Issue-Key: wafv2|GetIPSet|4xx->4xx|$.__type
Issue-Key: wafv2|GetIPSet|4xx->4xx|$.message
Issue-Key: wafv2|DescribeManagedRuleGroup|4xx->4xx|$.Message
Issue-Key: wafv2|ListAvailableManagedRuleGroupVersions|4xx->4xx|$.Message
Issue-Key: wafv2|PutManagedRuleSetVersions|4xx->4xx|$.Field
Issue-Key: wafv2|PutManagedRuleSetVersions|4xx->4xx|$.Parameter
Issue-Key: wafv2|PutManagedRuleSetVersions|4xx->4xx|$.Reason
Issue-Key: wafv2|PutManagedRuleSetVersions|4xx->4xx|$.__type
Issue-Key: wafv2|PutManagedRuleSetVersions|4xx->4xx|$.message
Issue-Key: wafv2|CreateRegexPatternSet|4xx->4xx|$.Field
Issue-Key: wafv2|CreateRegexPatternSet|4xx->4xx|$.Parameter
Issue-Key: wafv2|CreateRegexPatternSet|4xx->4xx|$.Reason
Issue-Key: wafv2|CreateRegexPatternSet|4xx->4xx|$.__type
Issue-Key: wafv2|CreateRegexPatternSet|4xx->4xx|$.message
Issue-Key: wafv2|CreateRuleGroup|4xx->4xx|$.Parameter
Issue-Key: wafv2|CreateRuleGroup|4xx->4xx|$.message
Issue-Key: wafv2|ListSettlementRecords|4xx->4xx|$.Message
Issue-Key: wafv2|ListSettlementRecords|4xx->4xx|$.__type
Issue-Key: wafv2|GetTopPathStatisticsByTraffic|2xx->4xx|$.PathStatistics
Issue-Key: wafv2|GetTopPathStatisticsByTraffic|2xx->4xx|$.TotalRequestCount
Issue-Key: wafv2|GetTopPathStatisticsByTraffic|2xx->4xx|status
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# wafv2 refusals carry the wrong exception class, sentence and InvalidParameter detail members

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIPSet

## Description
These eight operations do refuse (or refuse when they should not), but the refusal is the wrong exception class, the wrong sentence, or drops the WAFInvalidParameterException detail members (Field/Parameter/Reason) that AWS populates. This is ONE issue with EIGHT causes; every cause below must be closed before the fix is submitted.

Cause 1 of 8: GetIPSet with Scope=CLOUDFRONT from a non-us-east-1 endpoint is refused as a missing item instead of an invalid scope.
AWS: Smithy: GetIPSet.Scope is a required `Scope` enum; the service models both WAFInvalidParameterException and WAFNonexistentItemException as client/400 errors. The scope check runs before the item lookup, so WAF answers HTTP 400 with __type `WAFInvalidParameterException`, Field = the fixed enum token `SCOPE_VALUE`, Parameter = the request's Scope value, Reason = the fixed sentence `The scope is not valid.` and message = `Error reason: The scope is not valid., field: SCOPE_VALUE, parameter: <scope>`.
Emulator: HTTP 400 but __type `WAFNonexistentItemException`, and Field/Parameter/Reason/message are all absent.
Why: the client is told its IP set does not exist when the real fault is a scope/region mismatch, so it recreates a resource it already has instead of correcting its endpoint; retry classification on the WAFNonexistentItem class also differs.
Fix: order the scope precondition ahead of the item lookup in the GetIPSet handler's validation, and populate the InvalidParameter detail members.
Traces: wafv2-cloudfront-scope-outside-us-east-1-rejected#task#4.

Cause 2 of 8: DescribeManagedRuleGroup for an unknown vendor/group uses the generic nonexistent-item sentence.
AWS: Smithy: DescribeManagedRuleGroup models WAFNonexistentItemException (client, 400); the recorded refusal uses the managed-rules-specific sentence, not the generic one. Fixed text: `ManagedRules with name ` + the request's Name + ` by vendor ` + the request's VendorName + ` does not exist. Please check your inputs.` — the two names are request values, the rest is fixed.
Emulator: HTTP 400 with the right class but Message = `AWS WAF couldn't perform the operation because your resource doesn't exist.`
Why: the message is the only place the vendor/group that failed to resolve is named, so a client (or a human reading the log) cannot tell which of several managed-rule references in a web ACL is the bad one.
Fix: the not-found error construction in the DescribeManagedRuleGroup handler.
Traces: wafv2-managed-rule-group-and-marketplace-product-discovery#task#4.

Cause 3 of 8: ListAvailableManagedRuleGroupVersions for an unknown group, same wrong sentence.
AWS: Smithy: ListAvailableManagedRuleGroupVersions requires VendorName and Name and models WAFNonexistentItemException (400). Message is the same managed-rules template as Cause 2: `ManagedRules with name <Name> by vendor <VendorName> does not exist. Please check your inputs.`
Emulator: Message = `AWS WAF couldn't perform the operation because your resource doesn't exist.`
Why: as Cause 2 — the failing reference is unnamed.
Fix: the not-found error construction in the ListAvailableManagedRuleGroupVersions handler; share one managed-rules-not-found constructor with Cause 2.
Traces: wafv2-managed-rule-group-and-marketplace-product-discovery#task#5.

Cause 4 of 8: PutManagedRuleSetVersions raises a generic ValidationException instead of WAF's own WAFInvalidParameterException.
AWS: Smithy: PutManagedRuleSetVersions.LockToken is required with @pattern ^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$, and the operation's modeled client errors are WAFInvalidOperationException, WAFInvalidParameterException, WAFNonexistentItemException and WAFOptimisticLockException — ValidationException is NOT in the closure. The recorded refusal is HTTP 400, __type `WAFInvalidParameterException`, Field `RESOURCE_ARN`, Parameter = the rule-group ARN from the request, Reason = the fixed sentence `The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes.`, message = `Error reason: ` + that sentence + `, field: RESOURCE_ARN, parameter: ` + the ARN: WAF resolves the referenced resource before it looks at the lock token.
Emulator: HTTP 400 with __type `ValidationException` and a smithy-style constraint message about lockToken; Field/Parameter/Reason absent.
Why: WAFv2 clients and SDK error handlers match on the WAF* error classes; an unmodeled ValidationException is not caught by any WAFv2 catch block, so a recoverable condition surfaces as an unknown service error. It also reports the wrong fault (the lock token rather than the unresolvable ARN).
Fix: the ARN resolution and error mapping in the PutManagedRuleSetVersions handler's validation — resolve the referenced resource first and emit a modeled WAF error.
Traces: wafv2-managed-rule-set-vendor-operations-on-fresh-account#task#3.

Cause 5 of 8: CreateRegexPatternSet with an invalid regular expression is reported as a duplicate item.
AWS: Smithy: CreateRegexPatternSet.RegularExpressionList is required; the operation models both WAFDuplicateItemException and WAFInvalidParameterException (client, 400). A pattern WAF will not compile is refused with HTTP 400, __type `WAFInvalidParameterException`, Field `REGEX_PATTERN_SET`, Parameter = the rejected pattern string from the request, Reason = `The parameter contains formatting that is not valid.`, message = `Error reason: The parameter contains formatting that is not valid., field: REGEX_PATTERN_SET, parameter: <pattern>`.
Emulator: HTTP 400 with __type `WAFDuplicateItemException` and none of Field/Parameter/Reason/message.
Why: the client concludes the set already exists and skips creation (or deletes and retries), when in fact its regex was rejected; the pattern at fault is never named.
Fix: regex validation ahead of the duplicate-name check in the CreateRegexPatternSet handler.
Traces: wafv2-regex-pattern-set-lifecycle#task#4.

Cause 6 of 8: CreateRuleGroup's capacity-limit refusal reports the wrong Parameter value.
AWS: Smithy: CreateRuleGroup.Capacity is required (CapacityUnit, @range min 1) and the operation models WAFLimitsExceededException and WAFInvalidParameterException (400). The refusal's message template is `Error reason: You exceeded the capacity limit for a rule group or web ACL., field: RULE_GROUP, parameter: ` + a value; Reason and Field are fixed text, and the Parameter is the capacity figure the request asked for, not a constant from the emulator.
Emulator: the class, Field and Reason are right, but Parameter and the message's parameter tail carry a different number than the request's — the emulator substitutes its own value where WAF echoes the request's.
Why: the client cannot tell which number was rejected, so it cannot compute a capacity that would be accepted; automated capacity negotiation loops on a figure the service never complained about.
Fix: the capacity-limit error construction in the CreateRuleGroup handler — echo the request's Capacity into Parameter.
Traces: wafv2-rulegroup-lifecycle-with-permission-policy-errors#task#8.

Cause 7 of 8: ListSettlementRecords under REGIONAL scope refuses with the wrong class and no Message.
AWS: Smithy: ListSettlementRecords models WAFInvalidOperationException (client, 400, member `Message`) as well as WAFInvalidParameterException. Monetization is CloudFront-only, so a REGIONAL-scope call is an invalid operation, not an invalid parameter: HTTP 400, __type `WAFInvalidOperationException`, Message = the fixed sentence `Monetization is not available for regional resources. Use CloudFront scope to configure monetization.`
Emulator: HTTP 400 with __type `WAFInvalidParameterException` and no Message member at all.
Why: the client's error handling branches on the class (invalid-operation = unsupported here, invalid-parameter = fix your input) and is pushed down the wrong branch; with no Message it also gets no remediation text.
Fix: the scope gate in the ListSettlementRecords handler's validation — raise WAFInvalidOperationException with the monetization sentence.
Traces: wafv2-settlement-records-workflow#task#3.

Cause 8 of 8: GetTopPathStatisticsByTraffic refuses a request AWS serves.
AWS: Smithy: GetTopPathStatisticsByTraffic output has required members PathStatistics (PathStatisticsList) and TotalRequestCount (RequestCount, default 0, @range min 0). With no matching traffic, WAF answers HTTP 200 with an empty PathStatistics list and TotalRequestCount 0 — an empty result is a success, not an error.
Emulator: HTTP 400; PathStatistics and TotalRequestCount absent.
Why: the inverse of the other causes — a client doing traffic analytics on a quiet web ACL gets a hard failure where AWS returns an empty, well-formed result, so a perfectly normal "no data yet" state aborts the caller.
Fix: the no-data path in the GetTopPathStatisticsByTraffic handler — return the empty-result shape instead of refusing.
Traces: wafv2-error-paths-and-validation#task#7.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up). All eight clusters are verdict=mismatch.

1. GetIPSet wafv2-cloudfront-scope-outside-us-east-1-rejected#task#4 (400->400) — "$.__type: value_mismatch (expected 'WAFInvalidParameterException', got 'WAFNonexistentItemException'); $.Field: missing (expected 'SCOPE_VALUE'); $.Parameter: missing (expected 'CLOUDFRONT'); $.Reason: missing (expected 'The scope is not valid.'); $.message: missing (expected 'Error reason: The scope is not valid., field: SCOPE_VALUE, parameter: CLOUDFRONT')".
2. DescribeManagedRuleGroup wafv2-managed-rule-group-and-marketplace-product-discovery#task#4 (400->400) — "$.Message: value_mismatch (expected 'ManagedRules with name NotARealRuleGroup by vendor NotARealVendor does not exist. Please check your inputs.', got 'AWS WAF couldn\u2019t perform the operation because your resource doesn\u2019t exist.')".
3. ListAvailableManagedRuleGroupVersions wafv2-managed-rule-group-and-marketplace-product-discovery#task#5 (400->400) — "$.Message: value_mismatch (expected 'ManagedRules with name NotARealRuleGroup by vendor AWS does not exist. Please check your inputs.', got 'AWS WAF couldn\u2019t perform the operation because your resource doesn\u2019t exist.')".
4. PutManagedRuleSetVersions wafv2-managed-rule-set-vendor-operations-on-fresh-account#task#3 (400->400) — "$.__type: value_mismatch (expected 'WAFInvalidParameterException', got 'ValidationException'); $.Field: missing (expected 'RESOURCE_ARN'); $.Parameter: missing (expected 'arn:aws:wafv2:us-east-1:000000000000:regional/rulegroup/does-not-exist/00000000-0000-0000-0000-000000000000'); $.Reason: missing (expected \"The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes.\"); $.message: value_mismatch (... got \"1 validation error detected: Value 'stale-token' at 'lockToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$\")".
5. CreateRegexPatternSet wafv2-regex-pattern-set-lifecycle#task#4 (400->400) — "$.__type: value_mismatch (expected 'WAFInvalidParameterException', got 'WAFDuplicateItemException'); $.Field: missing (expected 'REGEX_PATTERN_SET'); $.Parameter: missing (expected '.*'); $.Reason: missing (expected 'The parameter contains formatting that is not valid.')".
6. CreateRuleGroup wafv2-rulegroup-lifecycle-with-permission-policy-errors#task#8 (400->400) — "$.Parameter: value_mismatch (expected '20', got '10'); $.message: value_mismatch (expected 'Error reason: You exceeded the capacity limit for a rule group or web ACL., field: RULE_GROUP, parameter: 20', got '... parameter: 10')" — 20 is the request's Capacity, 10 is the emulator's own constant.
7. ListSettlementRecords wafv2-settlement-records-workflow#task#3 (400->400, also wafv2-traffic-path-and-settlement-analytics#task#3) — "$.__type: value_mismatch (expected 'WAFInvalidOperationException', got 'WAFInvalidParameterException'); $.Message: missing (expected 'Monetization is not available for regional resources. Use CloudFront scope to configure monetization.')".
8. GetTopPathStatisticsByTraffic wafv2-error-paths-and-validation#task#7 (also wafv2-wafv2-error-paths#task#2) — "status: expected 200, got 400; $.PathStatistics: missing (expected []); $.TotalRequestCount: missing (expected 0)".

ARNs, pattern strings, vendor/group names and the capacity figure above are this corpus's request values; the requirement is the fixed sentence plus the value echoed from the request.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Cause 1: from a non-us-east-1 endpoint call GetIPSet with Scope=CLOUDFRONT -> expect WAFInvalidParameterException/SCOPE_VALUE; emulator returns WAFNonexistentItemException.
3. Causes 2-3: DescribeManagedRuleGroup and ListAvailableManagedRuleGroupVersions with a VendorName/Name that does not exist -> expect the 'ManagedRules with name <Name> by vendor <Vendor> does not exist. Please check your inputs.' Message; emulator returns the generic nonexistent-item sentence.
4. Cause 4: PutManagedRuleSetVersions naming a managed rule set that does not exist, with a non-UUID LockToken -> expect WAFInvalidParameterException with Field RESOURCE_ARN; emulator returns ValidationException about lockToken.
5. Cause 5: CreateRegexPatternSet with a RegularExpressionList entry WAF rejects -> expect WAFInvalidParameterException with Field REGEX_PATTERN_SET; emulator returns WAFDuplicateItemException.
6. Cause 6: CreateRuleGroup with Capacity above the account limit -> expect Parameter equal to the requested Capacity; emulator reports its own constant.
7. Cause 7: ListSettlementRecords with Scope=REGIONAL -> expect WAFInvalidOperationException with the monetization sentence; emulator returns WAFInvalidParameterException with no Message.
8. Cause 8: GetTopPathStatisticsByTraffic on a web ACL with no traffic -> expect 200 with PathStatistics [] and TotalRequestCount 0; emulator returns 400.
