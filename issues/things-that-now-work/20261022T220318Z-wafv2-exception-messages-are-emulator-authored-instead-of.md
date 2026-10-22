Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-error-paths-and-validation
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-error-paths-and-validation wafv2-ipset-lifecycle-and-errors wafv2-ipset-lifecycle-with-tagging wafv2-rate-based-rule-analytics-and-sampling wafv2-top-path-traffic-for-webacl wafv2-web-acl-lifecycle-and-list wafv2-wafv2-error-paths wafv2-webacl-full-lifecycle-with-managed-rules
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|CreateIPSet|4xx->4xx|$.Message
Issue-Key: wafv2|DeleteIPSet|4xx->4xx|$.Message
Issue-Key: wafv2|GetIPSet|4xx->4xx|$.Message
Issue-Key: wafv2|GetWebACL|4xx->4xx|$.Message
Issue-Key: wafv2|CreateWebACL|4xx->4xx|$.Message
Issue-Key: wafv2|DeleteWebACL|4xx->4xx|$.Message
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# WAFv2 exception messages are emulator-authored instead of WAF canned text

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIPSet

## Description
This bundle has 6 causes. All six are the same emulator defect: the wafv2 error serializer builds its own request-specific sentence for the modeled WAF client exceptions instead of emitting the fixed, request-independent message AWS returns for that exception type. The HTTP status (400) and the $.__type discriminator are correct in every cause; only $.Message diverges, and the emulator's text additionally leaks internal resource names, ids and scope values that AWS never puts in these bodies.

The shared rule is known, not guessed: in WAFv2 these three exceptions carry a constant sentence per exception type (one message per exception shape, never parameterized by the request). Smithy documents each shape with that sentence -- WAFDuplicateItemException: "WAF couldn't perform the operation because the resource that you tried to save is a duplicate of an existing one."; WAFOptimisticLockException: "WAF couldn't save your changes because you tried to update or delete a resource that has changed since you last retrieved it..."; WAFNonexistentItemException: "WAF couldn't perform the operation because your resource doesn't exist." -- and the recordings confirm the exact wire wording for all of them. The requirement is therefore "a constant message per exception type, carrying no request-derived values"; the recorded strings quoted below are the examples of it.

Cause 1 of 6: CreateIPSet, duplicate name. AWS: on a second CreateIPSet for an existing Name+Scope, 400 with __type WAFDuplicateItemException and the constant duplicate-item sentence (recorded example: "AWS WAF couldn't perform the operation because some resource in your request is a duplicate of an existing one."). Smithy: CreateIPSet/CreateWebACL both model WAFDuplicateItemException, httpError 400. Emulator: 400 WAFDuplicateItemException but Message is synthesized from the request -- recorded example 'An IPSet named "sim2real-probe-wafv2-wafv2-errpath-ipset-828faa63" already exists in scope REGIONAL'. Why: a client that branches on or logs the documented WAF message sees text that never appears on real WAF, and the emulator discloses the caller-supplied name and scope in a body AWS keeps generic. Fix: the CreateIPSet handler's duplicate-detection path / the wafv2 error serializer -- emit the constant exception message, not a formatted one. Traces: wafv2-error-paths-and-validation#task#2 (also wafv2-ipset-lifecycle-and-errors#task#4, wafv2-ipset-lifecycle-with-tagging#task#7).

Cause 2 of 6: DeleteIPSet, stale LockToken. AWS: 400 with __type WAFOptimisticLockException and the constant optimistic-lock sentence (recorded example: "AWS WAF couldn't save your changes because someone changed the resource after you started to edit it. Reapply your changes."). Smithy: DeleteIPSet models WAFOptimisticLockException, httpError 400. Emulator: 400 WAFOptimisticLockException with its own text, recorded example 'The IPSet has changed since you last retrieved it; get it again and retry with the new LockToken'. Why: same as cause 1 -- the documented retry guidance a client surfaces to users is replaced by emulator prose. Fix: the DeleteIPSet handler's lock-token check / the wafv2 error serializer. Traces: wafv2-error-paths-and-validation#task#3 (also wafv2-ipset-lifecycle-and-errors#task#6, wafv2-ipset-lifecycle-and-errors#task#7).

Cause 3 of 6: GetIPSet, unknown resource. AWS: 400 with __type WAFNonexistentItemException and the constant nonexistent-item sentence (recorded example: "AWS WAF couldn't perform the operation because your resource doesn't exist."). Smithy: GetIPSet models WAFNonexistentItemException, httpError 400. Emulator: 400 WAFNonexistentItemException with request-derived text, recorded example 'No IPSet named "sim2real-probe-wafv2-wafv2-errpath-ipset-828faa63" with id "26b00df9-3af3-436f-aef3-fbb33de9c332" exists in scope REGIONAL'. Why: as above, plus it echoes the requested name and id. Fix: the GetIPSet handler's lookup-miss path / the wafv2 error serializer. Traces: wafv2-error-paths-and-validation#task#5.

Cause 4 of 6: GetWebACL, unknown resource. AWS: 400 WAFNonexistentItemException with the same constant sentence (Smithy: GetWebACL models WAFNonexistentItemException, httpError 400). Emulator: 400 WAFNonexistentItemException with recorded example 'No WebACL named "sim2real-probe-wafv2-rate-webacl-5f0a47d1" with id "f6bc793c-3c14-434a-8180-9f11d61dacb1" exists in scope "REGIONAL"'. Why: as above; this is the post-delete probe path, so teardown assertions that match the documented message fail. Fix: the GetWebACL handler's lookup-miss path / the wafv2 error serializer. Traces: wafv2-rate-based-rule-analytics-and-sampling#teardown#11 (also wafv2-top-path-traffic-for-webacl#teardown#4, wafv2-web-acl-lifecycle-and-list#teardown#5).

Cause 5 of 6: CreateWebACL, duplicate name. AWS: 400 WAFDuplicateItemException with the constant duplicate-item sentence. Emulator: 400 WAFDuplicateItemException with recorded example 'A WebACL named "sim2real-probe-wafv2-wafv2-errpath-29f46185" already exists in scope REGIONAL'. Why/Fix: as cause 1, in the CreateWebACL handler's duplicate-detection path. Traces: wafv2-wafv2-error-paths#task#3 (also wafv2-webacl-full-lifecycle-with-managed-rules#task#6).

Cause 6 of 6: DeleteWebACL, stale LockToken. AWS: 400 WAFOptimisticLockException with the constant optimistic-lock sentence. Emulator: 400 WAFOptimisticLockException with recorded example 'The WebACL has changed since you last retrieved it; get it again and retry with the new LockToken'. Why/Fix: as cause 2, in the DeleteWebACL handler's lock-token check. Traces: wafv2-wafv2-error-paths#task#4 (also wafv2-web-acl-lifecycle-and-list#teardown#6).

The whole bundle is one code change in practice: a table of constant messages keyed by WAF exception type, used by every wafv2 error path, replacing the per-handler formatted strings.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator, 6 divergence clusters (status and __type matched; only $.Message diverged):

1. CreateIPSet 400->400, $.Message: expected 'AWS WAF couldn't perform the operation because some resource in your request is a duplicate of an existing one.', got 'An IPSet named "sim2real-probe-wafv2-wafv2-errpath-ipset-828faa63" already exists in scope REGIONAL' (wafv2-error-paths-and-validation#task#2, 3 affected steps).
2. DeleteIPSet 400->400, $.Message: expected 'AWS WAF couldn't save your changes because someone changed the resource after you started to edit it. Reapply your changes.', got 'The IPSet has changed since you last retrieved it; get it again and retry with the new LockToken' (wafv2-error-paths-and-validation#task#3, 3 affected).
3. GetIPSet 400->400, $.Message: expected 'AWS WAF couldn't perform the operation because your resource doesn't exist.', got 'No IPSet named "..." with id "26b00df9-3af3-436f-aef3-fbb33de9c332" exists in scope REGIONAL' (wafv2-error-paths-and-validation#task#5).
4. GetWebACL 400->400, $.Message: expected 'AWS WAF couldn't perform the operation because your resource doesn't exist.', got 'No WebACL named "..." with id "f6bc793c-3c14-434a-8180-9f11d61dacb1" exists in scope "REGIONAL"' (wafv2-rate-based-rule-analytics-and-sampling#teardown#11, 3 affected).
5. CreateWebACL 400->400, $.Message: expected duplicate-item sentence, got 'A WebACL named "..." already exists in scope REGIONAL' (wafv2-wafv2-error-paths#task#3, 2 affected).
6. DeleteWebACL 400->400, $.Message: expected optimistic-lock sentence, got 'The WebACL has changed since you last retrieved it; ...' (wafv2-wafv2-error-paths#task#4, 2 affected).

Model grounding: Smithy shapes WAFDuplicateItemException / WAFOptimisticLockException / WAFNonexistentItemException each carry a single Message member and a documented constant sentence; httpError 400, errorKind client.

## Steps to Reproduce
1. Replay fixture wafv2-error-paths-and-validation step task#2 against the emulator (CreateIPSet with a Name+Scope that already exists) and compare $.Message with the recorded response.
2. Same fixture step task#3 (DeleteIPSet with a stale LockToken) and task#5 (GetIPSet for a deleted id).
3. Replay wafv2-rate-based-rule-analytics-and-sampling#teardown#11 (GetWebACL after delete), wafv2-wafv2-error-paths#task#3 (duplicate CreateWebACL) and wafv2-wafv2-error-paths#task#4 (DeleteWebACL with stale LockToken).
4. In each case the status is 400 and $.__type is the expected WAF* exception, but $.Message is the emulator's own sentence containing the resource name/id/scope instead of the constant WAF message.
