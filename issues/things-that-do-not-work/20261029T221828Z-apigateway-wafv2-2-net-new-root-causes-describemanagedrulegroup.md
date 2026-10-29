Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected
Issue-Key: apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule|run#42|AssertionError|assert labels and all(n.startswith("awswaf:managed:aws:") for n in labels), (
Issue-Key: apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected|_poll#20|Timeout|time.sleep(interval_s)
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway__wafv2: 2 net-new root causes — DescribeManagedRuleGroup declares no AvailableLabels for AWSManagedRulesSQLiRuleSet, and the RegexPatternSet-reference refusal contract never completes (2 causes)

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeManagedRuleGroup

## Description
Two distinct root causes in the apigateway__wafv2 surface, neither marked OWNED nor ALREADY FILED by the replay report. They are bundled because they share one service token; a fixer must close BOTH before submitting.

=== Cause 1 of 2: DescribeManagedRuleGroup returns an empty AvailableLabels for an AWS-vendor managed rule group ===
Failing check id: apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule

AWS: Smithy (wafv2, com-amazonaws-wafv2-awswaf-20190729) — DescribeManagedRuleGroup output carries `Rules` (RuleSummaries), `LabelNamespace` (LabelName, @length 1-1024, pattern ^[0-9A-Za-z_\-:]+$), `AvailableLabels` and `ConsumedLabels` (both LabelSummaries lists). `AvailableLabels` is documented as "The labels that one or more rules in this rule group add to matching web requests. These labels are defined in the RuleLabels for a Rule." For a managed rule group published by the AWS vendor, every declared label name lives under the managed-vendor namespace: the FIXED text of each name is the prefix `awswaf:managed:aws:`, followed by the group's own label namespace segment and the per-rule label segment (both VALUES derived from the group being described, e.g. the SQL-database group's namespace segment and its per-rule label suffixes such as the body/query-argument/cookie SQLi labels). AWS returns a NON-EMPTY AvailableLabels for AWSManagedRulesSQLiRuleSet, and LabelNamespace is the same `awswaf:managed:aws:<group-namespace>:` prefix. Nothing in the model permits an empty list for a group whose Rules list is non-empty and whose rules add labels.

Emulator: `DescribeManagedRuleGroup(VendorName="AWS", Name="AWSManagedRulesSQLiRuleSet", Scope="REGIONAL")` returns `AvailableLabels: []`. The check's earlier assertions on the SAME response passed — `Rules` was non-empty and the terminating rule named in GetSampledRequests' RuleNameWithinRuleGroup (`<vendor>#<group>#<rule>`) was found among them — so the managed-group catalog is populated for rules but carries no label declarations at all. The replay detail is: `AssertionError: AWSManagedRulesSQLiRuleSet does not declare its labels under the managed-vendor namespace: []`.

Why: labels are the documented mechanism for acting on a managed rule group's finding without letting the group terminate the request — a customer sets the group's OverrideAction to Count and then writes a LabelMatchStatement against `awswaf:managed:aws:...`. With AvailableLabels empty, a client cannot discover the namespace at all, tooling that enumerates a group's labels renders nothing, and a rule authored against the real AWS namespace silently never matches on the emulator. The failure is invisible: requests are simply allowed where AWS would refuse them.

Fix: the wafv2 DescribeManagedRuleGroup handler and the bundled managed-rule-group catalog/state it reads — each AWS-vendor group must declare its AvailableLabels (and the matching LabelNamespace) alongside its Rules, with every label name under the `awswaf:managed:aws:` prefix.

Traces: apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule.

=== Cause 2 of 2: the DeleteRegexPatternSet-while-referenced refusal contract is never reached — the check hangs to the 60 s cap ===
Failing check id: apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected

AWS: Smithy — DeleteRegexPatternSet takes Name/Scope/Id/LockToken (all required) and models `WAFAssociatedItemException` (client error, HTTP 400) among its errors. Deleting a RegexPatternSet that a WebACL rule's RegexPatternSetReferenceStatement still references is REFUSED with error code `WAFAssociatedItemException`; AWS words the message for this error class as "AWS WAF couldn't perform the operation because your resource is being used by another resource or it's associated with another resource." — that sentence is fixed text, with no request-derived values. The refusal must have NO effect: GetRegexPatternSet still returns the same RegularExpressionList and the SAME LockToken (a refused delete does not rotate the lock token), and the referencing rule keeps deciding at the edge (the regex-matching stage path stays refused while a sibling non-matching path stays served by the origin). After UpdateWebACL removes the referencing rule, the IDENTICAL DeleteRegexPatternSet succeeds and GetRegexPatternSet then raises `WAFNonexistentItemException` ("AWS WAF couldn't perform the operation because your resource doesn't exist."). The edge status codes (403 for a blocked request, 200 for an allowed one) are recorded behavior only — execute-api is not Smithy-modelled — but what is structural is that an associated web ACL's RegexPatternSetReferenceStatement decides every request and the stage always terminates it with an HTTP response.

Emulator: the check is killed by the harness at its 60 s cap; the detail is exactly `Error: Check timed out after 60.0s`, with no frame, so nothing downstream of the stall is observed — not the WAFAssociatedItemException refusal, not the unchanged RegularExpressionList/LockToken, not the post-unreference success, not the WAFNonexistentItemException. The only unbounded waits the check performs before the delete are (a) polling the freshly deployed stage for its MOCK integration to answer 200 (180 s budget) and (b) polling for a STABLE 403 from the rule whose statement is a RegexPatternSetReferenceStatement over UriPath (360 s budget) — both budgets exceed the 60 s cap, so the check dies inside one of them. Hypothesis: a sibling check in this same service (the managed-SQLi one above) drove the identical plumbing — CreateRestApi + MOCK integration + CreateDeployment + CreateWebACL + AssociateWebACL — all the way through to a 403 at the edge and on to GetSampledRequests, so the stage does answer and web-ACL evaluation does happen; the stall is therefore most plausibly the RegexPatternSetReferenceStatement never producing a 403, leaving the whole DeleteRegexPatternSet reference-counting contract unexercised. The model and the recording do not state which of the two polls stalls.

Why: a client that deletes a RegexPatternSet still wired into a live web ACL gets, on AWS, a 400/WAFAssociatedItemException that protects a working rule; here it gets no decision the suite can observe, and because the rule's effect at the edge is never demonstrated, an emulator that accepted the delete and silently broke the WebACL rule — or one that refused EVERY delete, including the legitimate post-unreference one — would be indistinguishable from a correct one. Either way a client's cleanup/teardown ordering and its edge protection both become unreliable.

Fix: the wafv2 RegexPatternSetReferenceStatement evaluation in the web-ACL inspection hook on the execute-api data plane (so a referencing rule actually decides, and a request is always terminated with a response), plus DeleteRegexPatternSet's reference bookkeeping in wafv2 state — a set referenced by any WebACL rule must be refused with WAFAssociatedItemException and left byte-identical with an unrotated LockToken, and the same delete must succeed once the reference is gone.

Related open issue that does NOT own this check (the suite marks it neither OWNED nor ALREADY FILED): 20261029T041807Z-apigateway-wafv2-three-more-wafv2-at-the-edge.md describes the same 60 s-cap hang family for three other wafv2-at-the-edge checks; its replay key does not cover this one.

Traces: apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected.

## Evidence
Cause 1 — DescribeManagedRuleGroup.AvailableLabels empty:
  [mismatch] apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule
  Error: File "check_apigateway__wafv2_wafv2_managed_sqli_rule_group_refuses_a_payload_and_the_sample_names_the_rule.py", line 389, in run
    assert labels and all(n.startswith("awswaf:managed:aws:") for n in labels)
    AssertionError: AWSManagedRulesSQLiRuleSet does not declare its labels under the managed-vendor namespace: []

Expected (AWS): DescribeManagedRuleGroup(VendorName='AWS', Name='AWSManagedRulesSQLiRuleSet', Scope='REGIONAL') returns a non-empty AvailableLabels list (LabelSummaries) in which every entry's Name starts with the fixed prefix 'awswaf:managed:aws:' followed by the group's label-namespace segment and the per-rule label segment, together with a LabelNamespace carrying the same prefix. Smithy: DescribeManagedRuleGroupResponse members Rules / LabelNamespace / AvailableLabels / ConsumedLabels; AvailableLabels documented as "The labels that one or more rules in this rule group add to matching web requests. These labels are defined in the RuleLabels for a Rule."
Actual (emulator): AvailableLabels == [] while the same response's Rules list is populated (the check's immediately preceding assertions on Rules and on the sample's RuleNameWithinRuleGroup both passed).

Cause 2 — DeleteRegexPatternSet-while-referenced contract unreachable:
  [mismatch] apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected
  Error: Check timed out after 60.0s

Expected (AWS):
- The regex-matching stage path is refused (403) by the rule whose statement is RegexPatternSetReferenceStatement over UriPath, while a sibling non-matching path is served 200 by the origin.
- DeleteRegexPatternSet on the referenced set -> error code 'WAFAssociatedItemException' (Smithy: modelled on DeleteRegexPatternSet, client error, HTTP 400), message "AWS WAF couldn't perform the operation because your resource is being used by another resource or it's associated with another resource." (fixed text, no request-derived values).
- After that refusal GetRegexPatternSet returns the unchanged RegularExpressionList and the SAME LockToken, and the matching path is still refused on every subsequent observation.
- After UpdateWebACL removes the referencing rule, the matching path returns to 200 with the origin's body, the identical DeleteRegexPatternSet succeeds, and GetRegexPatternSet then raises 'WAFNonexistentItemException' ("AWS WAF couldn't perform the operation because your resource doesn't exist.").
Actual (emulator): the check never completes; the harness kills it at the 60 s cap with the detail 'Error: Check timed out after 60.0s' and no frame. None of the above is observed. The pre-delete legs the check runs are a poll for the stage's MOCK 200 (180 s budget) and a poll for a stable 403 from the referencing rule (360 s budget); both exceed the cap. A sibling check in the same service exercised the same CreateRestApi/MOCK/CreateDeployment/CreateWebACL/AssociateWebACL plumbing through to a 403 and GetSampledRequests, so the stage does answer and web-ACL evaluation does run.

## Steps to Reproduce
Cause 1 (DescribeManagedRuleGroup.AvailableLabels):
1. wafv2 DescribeManagedRuleGroup with VendorName='AWS', Name='AWSManagedRulesSQLiRuleSet', Scope='REGIONAL'.
2. Observe Rules is populated but AvailableLabels is [] (and LabelNamespace carries no 'awswaf:managed:aws:' namespace), where AWS returns a non-empty AvailableLabels whose every Name begins with 'awswaf:managed:aws:'.
3. Or replay check apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule.

Cause 2 (DeleteRegexPatternSet while referenced):
1. apigateway CreateRestApi (REGIONAL); add two MOCK-integration child resources under '/' whose response template echoes an origin marker — one path matching the regex, one not; CreateDeployment to a stage; confirm both paths serve 200 with the marker.
2. wafv2 CreateRegexPatternSet (Scope=REGIONAL) with RegularExpressionList=[{RegexString: '^/[^/]+/rx-[0-9]{4}$'}].
3. wafv2 CreateWebACL (Scope=REGIONAL, DefaultAction=Allow) with one Priority-0 rule: Statement=RegexPatternSetReferenceStatement{ARN=<set ARN>, FieldToMatch={UriPath:{}}, TextTransformations=[{Priority:0,Type:NONE}]}, Action=Block.
4. wafv2 AssociateWebACL with the stage ARN (retrying WAFUnavailableEntityException).
5. GET the regex-matching stage path repeatedly — AWS settles on 403 while the non-matching path stays 200; observe here that the expected stable 403 never arrives and the check is killed at the 60 s harness cap.
6. GetRegexPatternSet for its LockToken, then DeleteRegexPatternSet with it — AWS refuses with 400 / WAFAssociatedItemException and leaves the set and its LockToken untouched; this leg is never reached.
7. UpdateWebACL with Rules=[] , then repeat the identical DeleteRegexPatternSet — AWS accepts it and GetRegexPatternSet then raises WAFNonexistentItemException; this leg is never reached either.
8. Or replay check apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected.
