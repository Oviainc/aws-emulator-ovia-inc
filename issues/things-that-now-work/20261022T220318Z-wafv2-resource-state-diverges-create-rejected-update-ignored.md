Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-webacl-full-lifecycle-with-managed-rules
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-webacl-full-lifecycle-with-managed-rules wafv2-ipset-lifecycle-with-tagging
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|CreateWebACL|2xx->4xx|$.Summary
Issue-Key: wafv2|CreateWebACL|2xx->4xx|status
Issue-Key: wafv2|GetIPSet|4xx->2xx|$
Issue-Key: wafv2|GetIPSet|4xx->2xx|$.Message
Issue-Key: wafv2|GetIPSet|4xx->2xx|$.__type
Issue-Key: wafv2|GetIPSet|4xx->2xx|status
Issue-Key: wafv2|GetIPSet|2xx->2xx|$.IPSet.Addresses[*]
Issue-Key: wafv2|GetIPSet|2xx->2xx|$.IPSet.Description
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# WAFv2 resource state diverges: create rejected, update ignored, delete not applied

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWebACL

## Description
This bundle has 4 causes. They share one emulator defect area: wafv2 mutating operations do not move the stored resource to the state AWS would leave it in -- a create is rejected outright, an update is not applied, a delete does not remove the resource, and list ordering inside a stored resource is not preserved. Every one of them makes a later read in the same fixture disagree with AWS.

Cause 1 of 4: CreateWebACL with managed rule group statements is rejected. AWS: CreateWebACL returns 200 with Summary {ARN, Description, Id, LockToken, Name} for a web ACL whose Rules include AWS managed rule group references (Smithy: CreateWebACL -> CreateWebACLResponse.Summary: WebACLSummary; managed-rule failures are modeled as specific errors -- WAFInvalidParameterException, WAFUnavailableEntityException, WAFExpiredManagedRuleGroupVersionException, WAFSubscriptionNotFoundException -- so a valid managed-rule web ACL is a success, not a 400). Emulator: 400 with no Summary member at all, so the fixture's whole managed-rules lifecycle cannot proceed. Hypothesis: the emulator's rule-statement validation does not accept ManagedRuleGroupStatement (unknown vendor/name, or the statement variant is unsupported) and fails the create. Why: a client cannot create any web ACL that references AWS managed rule groups -- the most common real-world WAF configuration -- against the emulator; nothing downstream of create is testable. Fix: the CreateWebACL handler's rule-statement validation / WebACL state construction (accept ManagedRuleGroupStatement). Traces: wafv2-webacl-full-lifecycle-with-managed-rules#task#3.

Cause 2 of 4: a deleted IPSet remains readable. AWS: after DeleteIPSet succeeds, GetIPSet for that Name+Id+Scope returns 400 with __type WAFNonexistentItemException (Smithy: GetIPSet models WAFNonexistentItemException, httpError 400; DeleteIPSet returns an empty output on success). Emulator: GetIPSet keeps returning 200 with the full IPSet body and a LockToken, and the replay's poll gave up after 15 attempts still seeing 200 -- so the resource is never removed, not merely removed late. Why: teardown and idempotency logic break -- a client that deletes and then verifies absence (or recreates under the same name, which should then not be a duplicate) sees the resource alive forever; the emulator also leaks state across a fixture's resources. Fix: the DeleteIPSet handler and the IPSet store -- actually remove the entry (and make the delete's success/lock-token path consistent with it). Traces: wafv2-ipset-lifecycle-with-tagging#teardown#11.

Cause 3 of 4: UpdateIPSet does not apply the new Addresses or clear the Description. AWS: UpdateIPSet replaces the IP set's contents wholesale -- Smithy UpdateIPSet input: Addresses required, Description optional, LockToken required; it returns NextLockToken, and a following GetIPSet reflects exactly the submitted Addresses, with Description no longer set (recorded as '') when the update omitted it. Emulator: the subsequent GetIPSet still returns the pre-update addresses and the pre-update description. Why: a client's update appears to succeed but the resource never changes, so rules referencing the IP set continue to match the old CIDR list -- a silent correctness failure, not a cosmetic one. Fix: the UpdateIPSet handler / IPSet state -- replace Addresses and unset Description per full-replace semantics. Traces: wafv2-ipset-lifecycle-with-tagging#task#9.

Cause 4 of 4: IPSet Addresses order is not preserved. AWS: Addresses is a list (Smithy IPSet.Addresses -> IPAddresses, required, list) and GetIPSet returns the entries in the order they were supplied; the recording shows '10.0.1.0/24' at index 0 and '10.0.0.0/24' at index 1. Emulator: the same two entries come back in the opposite order. Hypothesis: the store normalizes/sorts the address list (or rebuilds it from an unordered collection) instead of keeping request order. Why: clients that compare the read-back list element-by-element against what they submitted -- the ordinary way to confirm an update -- report a spurious difference, and index-based assertions address the wrong CIDR. Fix: IPSet state -- keep Addresses as an ordered list through create and update. Traces: wafv2-ipset-lifecycle-with-tagging#task#2.

Severity HIGH: causes 1-3 block or silently discard client-visible state changes.

## Evidence
Replay of simparity-wire-aws-v1, 4 divergence clusters:

1. CreateWebACL, expected 200, got 400 (wafv2-webacl-full-lifecycle-with-managed-rules#task#3): $.Summary missing (expected {'ARN': ..., 'Description': 'probe webacl', 'Id': ..., 'LockToken': ..., 'Name': 'sim2real-probe-wafv2-webacl-383e24dd'}).
2. GetIPSet after DeleteIPSet, expected 400, got 200 (wafv2-ipset-lifecycle-with-tagging#teardown#11): $.__type missing (expected 'WAFNonexistentItemException'), $.Message missing (expected the nonexistent-item sentence); poll: status 200 not in [400] after 15 attempts, body still {'IPSet': {'Name': 'sim2real-probe-wafv2-ipset-bcd042b4', 'Id': 'fda8ddf3-...', 'Addresses': ['10.0.0.0/24', '10.0.1.0/24'], ...}, 'LockToken': '95e83b71-...'}.
3. GetIPSet after UpdateIPSet, 200 -> 200 (wafv2-ipset-lifecycle-with-tagging#task#9): $.IPSet.Addresses[0] expected '172.16.0.0/16', got '10.0.0.0/24'; $.IPSet.Addresses[1] extra (expected absent, got '10.0.1.0/24'); $.IPSet.Description expected '', got 'probe ipset'.
4. GetIPSet, 200 -> 200 (wafv2-ipset-lifecycle-with-tagging#task#2): $.IPSet.Addresses[0] expected '10.0.1.0/24', got '10.0.0.0/24'; $.IPSet.Addresses[1] expected '10.0.0.0/24', got '10.0.1.0/24'.

Model grounding: Smithy CreateWebACL -> Summary: WebACLSummary with managed-rule problems modeled as distinct 400 errors; UpdateIPSet input {Addresses required, Description optional, LockToken required} -> NextLockToken; DeleteIPSet -> empty output, GetIPSet models WAFNonexistentItemException (httpError 400); IPSet.Addresses is a required list.

Related: the message text of the WAFNonexistentItemException in cause 2 is covered by the separate wafv2 exception-message issue; the divergence here is the 200-vs-400 status, i.e. the resource still existing.

## Steps to Reproduce
1. Replay fixture wafv2-webacl-full-lifecycle-with-managed-rules step task#3: CreateWebACL whose Rules contain an AWS managed rule group statement. The emulator answers 400 with no Summary where the recording has 200.
2. Replay fixture wafv2-ipset-lifecycle-with-tagging step task#2: GetIPSet right after create and compare IPSet.Addresses element-by-element with the submitted order.
3. Same fixture step task#9: UpdateIPSet replacing Addresses with ['172.16.0.0/16'] and omitting Description, then GetIPSet -- the emulator still returns the two original CIDRs and the original description.
4. Same fixture step teardown#11: DeleteIPSet then GetIPSet -- the emulator returns 200 with the IPSet body for 15 consecutive polls instead of 400 WAFNonexistentItemException.
