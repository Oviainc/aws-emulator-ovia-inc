Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.permission-policy-invalid-document-rejected wafv2.permissionpolicy-wildcard-action-rejected
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.permission-policy-invalid-document-rejected|run#8|AssertionError|assert message == (
Issue-Key: wafv2.permissionpolicy-wildcard-action-rejected|run#13|AssertionError|assert message == POLICY_MESSAGE, message
Issue-Commit: 4446d1d1a0e81f091cf6d5a2b5a63655adeb43fe

# wafv2 PutPermissionPolicy: WAFInvalidPermissionPolicyException carries the Smithy documentation sentence instead of AWS's wire message

> _Note: this issue was identified against the code base at commit `4446d1d1a0e81f091cf6d5a2b5a63655adeb43fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutPermissionPolicy

## Description
This issue bundles ONE net-new root cause; it must be closed before submitting.

=== Cause 1 of 1: the WAFInvalidPermissionPolicyException message emitted by PutPermissionPolicy is not AWS's error message ===

AWS: Smithy (services/service-com-amazonaws-wafv2-awswaf-20190729/smithy-model/com-amazonaws-wafv2-awswaf-20190729.json) models PutPermissionPolicy with input ResourceArn (target ResourceArn, required, @length 20-2048, pattern \S) and Policy (target PolicyString, required, @length 1-395000, pattern \S), and an error set of exactly WAFInternalErrorException (server, 500), WAFInvalidParameterException (client, 400), WAFInvalidPermissionPolicyException (client, 400) and WAFNonexistentItemException (client, 400). The documentation of WAFInvalidPermissionPolicyException states the policy rules AWS enforces: the document must be an IAM policy of version 2012-10-17; it must specify Effect, Action and Principal; Effect must be Allow; Action must specify wafv2:CreateWebACL, wafv2:UpdateWebACL and wafv2:PutFirewallManagerRuleGroups and may optionally specify wafv2:GetRuleGroup, and "WAF rejects any extra actions or wildcard actions in the policy"; and the policy must not include a Resource parameter. Both a syntactically malformed document and a document whose Action is the wildcard "wafv2:*" therefore fail with HTTP 400 / WAFInvalidPermissionPolicyException. Recorded behavior for the message: AWS answers the single fixed sentence "The policy specification doesn't meet the requirements to be attached to an AWS WAF resource." This message is entirely FIXED text for this error class — it interpolates no part of the request or of emulator state (no ARN, no policy text, no action name, no rule-group name or id), so there is no variable part to fill from a recording. The status (400) and the code (WAFInvalidPermissionPolicyException) are already correct in the emulator; only the message diverges.

Emulator: for both refusal classes the emulator answers HTTP 400 with the correct code WAFInvalidPermissionPolicyException but the message "The operation failed because the specified policy isn't in the proper format." Replay output for both checks is the same AssertionError text: `AssertionError: The operation failed because the specified policy isn't in the proper format.` raised by the checks' `assert message == POLICY_MESSAGE` (the preceding `assert code == "WAFInvalidPermissionPolicyException"` passes). That string is verbatim the FIRST SENTENCE of the Smithy documentation trait on the WAFInvalidPermissionPolicyException shape ("<p>The operation failed because the specified policy isn't in the proper format. </p>"), i.e. the error's modeled doc prose is being surfaced as its runtime message. Hypothesis: the emulator derives the Message field from the shape's documentation trait (or from a generated per-shape default string) rather than carrying the service's actual runtime message for the error class.

Why: the message is part of the refusal contract. A client that matches on the refusal text — assertions in customer test suites, operator log/alert rules, SDK/CLI error surfacing, and any tool that distinguishes "your sharing policy does not meet AWS WAF's requirements" from other 400s — sees text that no real AWS endpoint ever returns, so a test or alarm written against AWS silently fails (or passes) against the emulator. Because the same wrong sentence is emitted for every WAFInvalidPermissionPolicyException cause (malformed JSON and a wildcard/extra action alike), callers also lose the one hint AWS's wording gives about the class of defect being reported. Note that in the wildcard check the positive control succeeds (a well-formed policy naming wafv2:CreateWebACL, wafv2:UpdateWebACL, wafv2:PutFirewallManagerRuleGroups, wafv2:GetRuleGroup is attached, read back by GetPermissionPolicy, then deleted), so this is not "every PutPermissionPolicy is refused" — the operation works, the refusal message is wrong.

Fix: the PutPermissionPolicy handler's policy-document validation and the error construction it hands to the serializer for WAFInvalidPermissionPolicyException — emit the service's fixed message for the error class rather than the shape's documentation text, and apply it to every rejection cause of this error (malformed document, missing Effect/Action/Principal, Effect other than Allow, extra or wildcard actions, presence of a Resource parameter). Wherever the emulator generically defaults an error Message from a Smithy documentation trait, that defaulting is the component to correct so other WAFv2 errors do not inherit the same defect. The state side already behaves: nothing is persisted on rejection and the follow-up GetPermissionPolicy still answers 400 / WAFNonexistentItemException with its own message, which both checks assert and which did not fail.

Related open issue, which does NOT cover this: 20261024T114331Z-wafv2-rejection-semantics-missing-on-three-operations-tagresource.md lists wafv2.permission-policy-invalid-document-rejected under its Issue-Trace-Ids, but its recorded cause and Issue-Key are the error CODE being the literal 'NotImplemented' (`assert code == "WAFInvalidPermissionPolicyException"`). That code assertion now passes; the replay report did not mark either check here as OWNED or ALREADY FILED. The surviving divergence is the message text only, which that issue never states as a requirement. The fixed issue 20261024T065754Z-wafv2-putpermissionpolicy-stub.md (operation existence) is likewise not reopened by this: the operation is implemented and the accepted path succeeds.

Traces: wafv2.permission-policy-invalid-document-rejected, wafv2.permissionpolicy-wildcard-action-rejected.

## Evidence
Suite simparity-behavioral-ga-prod-v1 — wafv2 56/97 passed. Two failing checks, neither marked ALREADY FILED nor OWNED:

1) [mismatch] wafv2.permission-policy-invalid-document-rejected
   check_wafv2_permission_policy_invalid_document_rejected.py line 115:
     assert message == (
         "The policy specification doesn't meet the requirements to be attached to an "
         "AWS WAF resource."
     ), message
   AssertionError: The operation failed because the specified policy isn't in the proper format.
   Request: create_rule_group(Scope='REGIONAL', Capacity=10, Rules=[], VisibilityConfig={...}) then
            put_permission_policy(ResourceArn=<rule group ARN>, Policy='{not valid json')
   Expected: HTTP 400, Error.Code 'WAFInvalidPermissionPolicyException',
             Error.Message "The policy specification doesn't meet the requirements to be attached to an AWS WAF resource." (fixed text, no interpolated values)
   Actual:   HTTP 400, Error.Code 'WAFInvalidPermissionPolicyException' (correct),
             Error.Message "The operation failed because the specified policy isn't in the proper format."
   The subsequent assertions (get_permission_policy -> 'WAFNonexistentItemException' with message "AWS WAF couldn\u2019t perform the operation because your resource doesn\u2019t exist.") were never reached.

2) [mismatch] wafv2.permissionpolicy-wildcard-action-rejected
   check_wafv2_permissionpolicy_wildcard_action_rejected.py line 150:
     assert message == POLICY_MESSAGE, message
     where POLICY_MESSAGE = "The policy specification doesn't meet the requirements to be attached to an AWS WAF resource."
   AssertionError: The operation failed because the specified policy isn't in the proper format.
   Positive control in the SAME check succeeded: put_permission_policy with Policy =
     {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},
      "Action":["wafv2:CreateWebACL","wafv2:UpdateWebACL","wafv2:PutFirewallManagerRuleGroups","wafv2:GetRuleGroup"]}]}
     was accepted, read back by get_permission_policy (contained wafv2:PutFirewallManagerRuleGroups) and deleted — so PutPermissionPolicy is implemented and functioning.
   Then put_permission_policy with Action ["wafv2:*"] on the same rule-group ARN:
   Expected: HTTP 400, Error.Code 'WAFInvalidPermissionPolicyException', Error.Message as above.
   Actual:   correct code, message "The operation failed because the specified policy isn't in the proper format."

Correlation establishing the root cause: the emulator's message is character-for-character the first sentence of the documentation trait on the error shape itself, as returned by the on-disk model —
  smithy_query(shape='WAFInvalidPermissionPolicyException').documentation ==
  "<p>The operation failed because the specified policy isn't in the proper format. </p> <p>The policy specifications must conform to the following:</p> ... WAF rejects any extra actions or wildcard actions in the policy ..."
so one error-message source, shared by both rejection causes, explains both failures.

Model source cited: services/service-com-amazonaws-wafv2-awswaf-20190729/smithy-model/com-amazonaws-wafv2-awswaf-20190729.json — PutPermissionPolicy input (ResourceArn, Policy) and error set {WAFInternalErrorException 500, WAFInvalidParameterException 400, WAFInvalidPermissionPolicyException 400, WAFNonexistentItemException 400}; documentation of WAFInvalidPermissionPolicyException for the policy rules (version 2012-10-17; Effect/Action/Principal required; Effect=Allow; the three required actions plus optional wafv2:GetRuleGroup; no extra or wildcard actions; no Resource parameter).

## Steps to Reproduce
Boot the emulator and point a botocore wafv2 client at it (Scope='REGIONAL' works from any region).

A — malformed policy document:
1. summary = create_rule_group(Name=<unique>, Scope='REGIONAL', Capacity=10, Rules=[], VisibilityConfig={'SampledRequestsEnabled': True, 'CloudWatchMetricsEnabled': True, 'MetricName': <unique>})['Summary']; arn = summary['ARN'].
2. put_permission_policy(ResourceArn=arn, Policy='{not valid json').
   Expect ClientError with HTTP 400, Error.Code == 'WAFInvalidPermissionPolicyException' and
   Error.Message == "The policy specification doesn't meet the requirements to be attached to an AWS WAF resource."
   Observed: the code is correct, but Error.Message == "The operation failed because the specified policy isn't in the proper format."
3. get_permission_policy(ResourceArn=arn) must still raise HTTP 400 / 'WAFNonexistentItemException' with message "AWS WAF couldn\u2019t perform the operation because your resource doesn\u2019t exist." (this part already holds).

B — wildcard action (positive control first):
1. arn as in A step 1 (Capacity=10).
2. principal = 'arn:aws:iam::' + sts.get_caller_identity()['Account'] + ':root'.
3. put_permission_policy(ResourceArn=arn, Policy='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"' + principal + '"},"Action":["wafv2:CreateWebACL","wafv2:UpdateWebACL","wafv2:PutFirewallManagerRuleGroups","wafv2:GetRuleGroup"]}]}') — must succeed; get_permission_policy(ResourceArn=arn)['Policy'] contains 'wafv2:PutFirewallManagerRuleGroups'; then delete_permission_policy(ResourceArn=arn).
4. put_permission_policy(ResourceArn=arn, Policy='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"' + principal + '"},"Action":["wafv2:*"]}]}').
   Expect ClientError with HTTP 400, Error.Code == 'WAFInvalidPermissionPolicyException' and the same fixed Error.Message as in A step 2.
   Observed: correct code, message "The operation failed because the specified policy isn't in the proper format."
5. get_permission_policy(ResourceArn=arn) must raise 400 / 'WAFNonexistentItemException' (nothing attached).

Re-replay to confirm the fix: wafv2.permission-policy-invalid-document-rejected, wafv2.permissionpolicy-wildcard-action-rejected.
