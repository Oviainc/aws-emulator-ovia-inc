Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.web-acl-challenge-immunity-time-below-service-minimum-rejected wafv2.put-managed-rule-set-versions-invalid-version-rejected
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.web-acl-challenge-immunity-time-below-service-minimum-rejected|run#12|AssertionError|assert resp.get("Reason") == _LOW_REASON, f"unexpected Reason: {resp.get('Reason')!r}"
Issue-Key: wafv2.put-managed-rule-set-versions-invalid-version-rejected|run#14|AssertionError|assert invalid_version_message == (
Issue-Commit: 2d15277b1a42408e6b04070a38b0e3902f63d4b5

# wafv2: two refusals carry the right class but not WAF's own sentence — the Challenge immunity `Reason` and the invalid version-key validation clause (2 causes)

> _Filed by the fixer of `20261024T065754Z-wafv2-control-plane-referenced-ipset-deletes-below-minimum.md`, whose seven causes are implemented: both refusals below now happen, with the right HTTP status, the right error code and the right structured members, and each is covered by an integration suite. What remains in both is the TEXT of one sentence, which neither the Smithy model nor the published documentation states, so it can only be taken from the recorded response — which is why it is filed rather than guessed at._

## Operation
UpdateWebACL

## Description

Cause 1 of 2: the `Reason` of the `WAFInvalidParameterException` that refuses a below-minimum Challenge immunity.
AWS: `ImmunityTimeProperty$ImmunityTime` is documented "The amount of time, in seconds, that a `CAPTCHA` or challenge timestamp is considered valid by WAF. The default setting is 300. For the Challenge action, the minimum setting is 300." A `ChallengeConfig` immunity below 300 is refused 400 `WAFInvalidParameterException` carrying `Field` `CHALLENGE_CONFIG`, `Parameter` the offending value as a string, a `Reason`, and a message composed as "Error reason: &lt;Reason&gt;, field: &lt;Field&gt;, parameter: &lt;value&gt;".
Emulator: all of that is now served — the call is refused, nothing is stored and the lock token does not rotate — and the `Reason` carries the model's own statement of the broken rule, "For the Challenge action, the minimum setting is 300." (`services/service-com-amazonaws-wafv2-awswaf-20190729/src/state.rs`, `CHALLENGE_IMMUNITY_BELOW_MINIMUM_REASON`). The check compares that member against the sentence recorded from WAF and they differ; four candidate wordings were tried against the recording and none matched, and the recorded sentence appears in no model doc, in no public documentation page and in no other check this repository records.
Why: a client that surfaces or matches WAF's own prose sees a sentence that does not appear on real WAF, even though the error class, the status and the structured members it branches on are right.
Fix: set `CHALLENGE_IMMUNITY_BELOW_MINIMUM_REASON` to the `Reason` of the recorded response for this class (and, if the recording also refuses an immunity ABOVE the modelled ceiling with this exception rather than at the request boundary, carry that sentence for the high end too — the check's constant is named `_LOW_REASON`, which suggests a sibling for it).
Traces: wafv2.web-acl-challenge-immunity-time-below-service-minimum-rejected.

Cause 2 of 2: the validation clause that refuses a `VersionsToPublish` KEY outside `VersionKeyString`'s form.
AWS: `VersionsToPublish` is a map whose keys are `VersionKeyString` (`@length(1..64)`, `@pattern(^[\w#:\.\-/]+$)`), and a key outside that form is a request-validation failure — `ValidationException`, in the validator's own envelope, not one of the operation's modelled errors.
Emulator: the code is right (`ValidationException`, ahead of the managed-rule-set resolution, publishing nothing), and the clause reads "1 validation error detected: Value 'not a valid version!!' at 'versionsToPublish' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 64, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^[\w#:\.\-/]+$]" (`src/ops/put_managed_rule_set_versions.rs` through `crate::state::map_key_constraint_envelope` and `VERSION_KEY_CONSTRAINTS`). The check compares the whole clause against the recorded one and they differ in some detail the assertion output does not show — the plain per-constraint form was tried first and also differed.
Why: a caller that parses or asserts on the validator's clause sees a different rendering of the same violation.
Fix: render the clause exactly as the recording shows it — the value echoed for a map-key violation (the key, or the whole map as the validator renders a collection), the constraint set and its order — in `crate::state::map_key_constraint_envelope`, the one place this service builds that clause.
Traces: wafv2.put-managed-rule-set-versions-invalid-version-rejected.

## Evidence
Cause 1 — AssertionError: unexpected Reason: 'For the Challenge action, the minimum setting is 300.' at `assert resp.get("Reason") == _LOW_REASON`. The assertions before it — the 400, the error code `WAFInvalidParameterException`, `Field` `CHALLENGE_CONFIG`, `Parameter` the offending value, and the unchanged `LockToken` — all pass.
Cause 2 — AssertionError at `assert invalid_version_message == (…)`, the actual message being the clause quoted above. The preceding assertion, `invalid_version_code == "ValidationException"`, passes.

## Steps to Reproduce
1. CreateWebACL (Scope REGIONAL, DefaultAction Allow), then UpdateWebACL with `ChallengeConfig={'ImmunityTimeProperty': {'ImmunityTime': 60}}`: the call is refused 400 WAFInvalidParameterException with Field CHALLENGE_CONFIG and Parameter '60', and `Reason` is the sentence above. Compare it with the recorded WAF response (cause 1).
2. Call PutManagedRuleSetVersions with a `VersionsToPublish` key that breaks `VersionKeyString`'s pattern: the call is refused 400 ValidationException and the message is the clause above. Compare it with the recorded WAF response (cause 2).
3. Or replay the two check ids in trace_ids.
