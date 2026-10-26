Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.ipset-delete-blocked-while-referenced wafv2.web-acl-challenge-immunity-time-below-service-minimum-rejected wafv2.rulegroup-rules-priority-ordering-preserved wafv2.loggingconfiguration-lifecycle wafv2.permissionpolicy-lifecycle wafv2.put-managed-rule-set-versions-invalid-version-rejected wafv2.managed-rule-set-version-expiry-unreachable wafv2.update-managed-rule-set-version-expiry-nonexistent-version-rejected
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.ipset-delete-blocked-while-referenced|_error_stable#7|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: wafv2.web-acl-challenge-immunity-time-below-service-minimum-rejected|_error_code#12|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded: {result!r}")
Issue-Key: wafv2.rulegroup-rules-priority-ordering-preserved|run#13|AssertionError|assert [r["Name"] for r in returned_rules] == ["r-mid", "r-high", "r-low"], (
Issue-Key: wafv2.loggingconfiguration-lifecycle|run#10|AssertionError|assert code_before == "WAFNonexistentItemException", code_before
Issue-Key: wafv2.permissionpolicy-lifecycle|run#6|AssertionError|assert code_before == "WAFNonexistentItemException", code_before
Issue-Key: wafv2.put-managed-rule-set-versions-invalid-version-rejected|run#4|AssertionError|assert code == "WAFNonexistentItemException", f"got {code!r} instead"
Issue-Key: wafv2.managed-rule-set-version-expiry-unreachable|run#8|AssertionError|assert code_update == "WAFNonexistentItemException", (
Issue-Key: wafv2.update-managed-rule-set-version-expiry-nonexistent-version-rejected|run#7|AssertionError|assert code == "WAFNonexistentItemException", (
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# wafv2 control plane: referenced IPSet deletes, below-minimum Challenge immunity accepted, rule order echoed, and four operations answer NotImplemented instead of their modeled errors

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWebACL

## Description
Seven distinct root causes in the wafv2 control plane. Each is numbered with its own failing check ids; all seven must be closed.

Cause 1 of 7: DeleteIPSet succeeds while the IPSet is still referenced by a web ACL rule.
AWS: Smithy (com.amazonaws.wafv2) DeleteIPSet models WAFAssociatedItemException (client, httpError 400), documented as "WAF couldn't perform the operation because your resource is being used by another resource or it's associated with another resource" — deleting an IPSet referenced by a web ACL rule is refused with 400 WAFAssociatedItemException and the IPSet survives.
Emulator: the DeleteIPSet call returns success. The check's helper reports "expected the call to be refused, but it succeeded", so referential integrity is not enforced and the web ACL is left pointing at a deleted IPSet.
Why: a caller can silently destroy a set a live ACL depends on; teardown ordering bugs that AWS catches pass here.
Fix: DeleteIPSet's handler/state — a reference check over web ACL and rule group statements before removal.
Traces: wafv2.ipset-delete-blocked-while-referenced.

Cause 2 of 7: UpdateWebACL accepts a Challenge ImmunityTime below the service minimum.
AWS: Smithy ImmunityTimeProperty.ImmunityTime -> TimeWindowSecond, required, @range min 60 max 259200, documented "For the Challenge action, the minimum setting is 300." A ChallengeConfig immunity time of 60 or 299 is refused with 400 WAFInvalidParameterException carrying WAF's triple (Field naming the challenge configuration block, Reason, and Parameter echoing the offending value as a string); the write does not happen and the LockToken does not rotate. The message is built from the request ("Error reason: <Reason>, field: <Field>, parameter: <value>") — that rule is the requirement, the recorded text is an example.
Emulator: the update is ACCEPTED — HTTP 200 with a fresh NextLockToken (recorded example: {'NextLockToken': 'e7a56a00-…', HTTPStatusCode 200}) — so the below-minimum value is stored and the lock token rotates.
Why: a web ACL that real AWS refuses to create is accepted here, and code relying on the refusal (or on the lock token not rotating after a refusal) behaves differently in production.
Fix: UpdateWebACL/CreateWebACL validation of ChallengeConfig.ImmunityTimeProperty (the 300-second Challenge floor, distinct from the modeled 60 lower bound that applies to CAPTCHA).
Traces: wafv2.web-acl-challenge-immunity-time-below-service-minimum-rejected.

Cause 3 of 7: rule-group Rules are returned in insertion order, not ascending Priority.
AWS: Smithy Rule.Priority documents that WAF evaluates rules "in order based on the value of Priority", processing lower priority first; GetRuleGroup returns the Rules normalized to ascending Priority (recorded behavior only for the normalization itself).
Emulator: the rules come back in the order they were submitted — recorded example: got ['r-low','r-mid','r-high'] where ascending-Priority order is ['r-mid','r-high','r-low'].
Why: a client that reads back a rule group to reason about evaluation order (or diffs it against a template) sees an order that does not match how the rules are evaluated.
Fix: the GetRuleGroup serializer / rule-group state ordering (sort by Priority on read or on store).
Traces: wafv2.rulegroup-rules-priority-ordering-preserved.

Cause 4 of 7: GetLoggingConfiguration (and DeleteLoggingConfiguration on the same path) is an unimplemented stub.
AWS: Smithy GetLoggingConfiguration models WAFNonexistentItemException (client, httpError 400); asking for the logging configuration of a web ACL that has none is refused with 400 WAFNonexistentItemException, and DeleteLoggingConfiguration on an already-deleted configuration is refused the same way.
Emulator: the call fails with error code NotImplemented ("operation ... is scaffolded but not implemented in this emulator") — the check asserted WAFNonexistentItemException and got 'NotImplemented'.
Why: the whole logging-configuration lifecycle (absent -> put -> read back -> delete -> absent) is unusable, and the absent-case error a client branches on never appears.
Fix: implement the GetLoggingConfiguration / DeleteLoggingConfiguration handlers over web-ACL logging state. (The separate PutLoggingConfiguration stub is covered by its own filed issue; this cause is the Get/Delete side.)
Traces: wafv2.loggingconfiguration-lifecycle.

Cause 5 of 7: GetPermissionPolicy is an unimplemented stub.
AWS: Smithy GetPermissionPolicy models WAFNonexistentItemException (client, httpError 400); on a rule group with no policy attached the call is refused with 400 WAFNonexistentItemException, and after PutPermissionPolicy it returns the stored Policy string.
Emulator: error code NotImplemented where WAFNonexistentItemException was expected.
Why: cross-account rule-group sharing cannot be read back at all, and the no-policy error class is unobservable.
Fix: implement the GetPermissionPolicy handler over rule-group permission-policy state. (The PutPermissionPolicy stub is covered by its own filed issue.)
Traces: wafv2.permissionpolicy-lifecycle.

Cause 6 of 7: PutManagedRuleSetVersions is an unimplemented stub.
AWS: Smithy PutManagedRuleSetVersions models WAFNonexistentItemException and WAFInvalidParameterException (client, httpError 400); against a ManagedRuleSet identity that does not exist the call is refused with 400 WAFNonexistentItemException, and nothing is published under that identity (ListManagedRuleSets stays empty of it).
Emulator: error code NotImplemented ('got \'NotImplemented\' instead').
Why: a managed-rule-set vendor flow cannot be exercised, and the nonexistent-target refusal a caller depends on is replaced by a generic stub error.
Fix: implement the PutManagedRuleSetVersions handler plus its existence/version-key validation over managed-rule-set state.
Traces: wafv2.put-managed-rule-set-versions-invalid-version-rejected.

Cause 7 of 7: UpdateManagedRuleSetVersionExpiryDate is an unimplemented stub.
AWS: Smithy UpdateManagedRuleSetVersionExpiryDate models WAFNonexistentItemException (client, httpError 400); against a fabricated ManagedRuleSet id, or a version that does not exist, the call is refused with 400 WAFNonexistentItemException.
Emulator: error code NotImplemented for both the nonexistent-set and nonexistent-version cases.
Why: version-expiry management is entirely unavailable and its not-found semantics cannot be relied on.
Fix: implement the UpdateManagedRuleSetVersionExpiryDate handler over managed-rule-set version state.
Traces: wafv2.managed-rule-set-version-expiry-unreachable, wafv2.update-managed-rule-set-version-expiry-nonexistent-version-rejected.

## Evidence
Cause 1 — AssertionError: expected the call to be refused, but it succeeded (DeleteIPSet with Name/Scope/Id/LockToken of an IPSet referenced by a web ACL rule). Expected 400 WAFAssociatedItemException.
Cause 2 — AssertionError: expected a rejection, but the call succeeded: {'NextLockToken': 'e7a56a00-e614-4b55-8fdc-4d416640555f', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} for ChallengeConfig.ImmunityTime=60 and 299. Expected 400 WAFInvalidParameterException with Field/Reason/Parameter and an unchanged LockToken.
Cause 3 — AssertionError: expected Rules normalized to ascending-Priority order, got ['r-low', 'r-mid', 'r-high'].
Cause 4 — AssertionError: NotImplemented at `assert code_before == "WAFNonexistentItemException"` for get_logging_configuration(ResourceArn=<web acl ARN>).
Cause 5 — AssertionError: NotImplemented at `assert code_before == "WAFNonexistentItemException"` for get_permission_policy(ResourceArn=<rule group ARN>).
Cause 6 — AssertionError: got 'NotImplemented' instead (expected WAFNonexistentItemException from put_managed_rule_set_versions against a fabricated set).
Cause 7 — AssertionError: expected WAFNonexistentItemException for a fabricated ManagedRuleSet, got 'NotImplemented'; and UpdateManagedRuleSetVersionExpiryDate against a nonexistent ManagedRuleSet id should fail with WAFNonexistentItemException, got 'NotImplemented'.

## Steps to Reproduce
1. CreateIPSet, reference it from an IPSetReferenceStatement rule in a web ACL, then DeleteIPSet with its current LockToken — it succeeds instead of 400 WAFAssociatedItemException (cause 1).
2. CreateWebACL with ChallengeConfig ImmunityTime 300, then UpdateWebACL with 60 and with 299 — both are accepted with 200 and a rotated LockToken (cause 2).
3. CreateRuleGroup with rules whose Priority order differs from submission order and GetRuleGroup — the returned Rules are in submission order (cause 3).
4. Call GetLoggingConfiguration / DeleteLoggingConfiguration on a web ACL with no logging configuration, GetPermissionPolicy on a policy-less rule group, PutManagedRuleSetVersions and UpdateManagedRuleSetVersionExpiryDate against a fabricated ManagedRuleSet — each answers NotImplemented instead of 400 WAFNonexistentItemException (causes 4-7).
5. Or replay the check ids in trace_ids.
