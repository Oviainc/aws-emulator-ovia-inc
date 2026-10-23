Issue-Source: trace-sweep-divergence
Issue-Trace: waf-regional-regex-pattern-match-set-roundtrip
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: waf-regional-regex-pattern-match-set-roundtrip waf-regional-tag-resource-roundtrip
Issue-Shape-Id: com.amazonaws.wafregional#AWSWAF_Regional_20161128
Issue-Key: waf-regional|GetRegexPatternSet|2xx->2xx|$.RegexPatternSet.RegexPatternStrings[*]
Issue-Key: waf-regional|ListTagsForResource|2xx->2xx|$.NextMarker
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# waf-regional GetRegexPatternSet and ListTagsForResource responses omit modeled members

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRegexPatternSet

## Description
Two net-new wire divergences in the waf-regional emulator, both of which are response bodies that drop a member the model/recording require. 2 causes; both must be closed.

Cause 1 of 2 — GetRegexPatternSet loses the pattern strings written by UpdateRegexPatternSet (fixture waf-regional-regex-pattern-match-set-roundtrip, step task#5).
AWS: Smithy (com.amazonaws.wafregional) — UpdateRegexPatternSet takes Updates: RegexPatternSetUpdates (@required, length min 1) whose RegexPatternSetUpdate carries Action: ChangeAction (INSERT/DELETE) and RegexPatternString (@required, length 1..512). GetRegexPatternSet returns RegexPatternSet, whose RegexPatternStrings member is @required (list, length 0..10). So after an INSERT update of a pattern string, a subsequent GetRegexPatternSet on the same RegexPatternSetId must return that string inside RegexPatternSet.RegexPatternStrings. The recorded step returns HTTP 200 with the inserted pattern present as the first element.
Emulator: returns HTTP 200 (status matches) but the element is absent — replay reports "$.RegexPatternSet.RegexPatternStrings[0]: missing (expected '^admin-[0-9]+$', got None)". The RegexPatternSet is serialized without the pattern string, i.e. the INSERT update was accepted but never reflected back, leaving an @required member empty/absent.
Why: a client that creates a regex pattern set, inserts patterns and then reads the set back sees an empty set. Every read-after-write consumer (CloudFormation AWS::WAFRegional::RegexPatternSet convergence, Terraform's waf_regional_regex_pattern_set refresh, any idempotent reconcile loop) concludes the patterns were never applied and either re-inserts them forever or reports permanent drift; SDK clients that index RegexPatternStrings[0] get an index/None error on a response the model says always carries the list.
Fix: the UpdateRegexPatternSet handler's state write and/or the GetRegexPatternSet handler's read of that state in the waf-regional service crate — the regex-pattern-set store must apply ChangeAction INSERT/DELETE to the set's pattern-string collection and GetRegexPatternSet must serialize RegexPatternStrings from it (always present, even when empty).
Traces: waf-regional-regex-pattern-match-set-roundtrip#task#5.

Cause 2 of 2 — ListTagsForResource omits NextMarker from the response (fixture waf-regional-tag-resource-roundtrip, steps task#4 and task#6).
AWS: Smithy (com.amazonaws.wafregional) — ListTagsForResourceResponse has members NextMarker (NextMarker) and TagInfoForResource (TagInfoForResource). Recorded behavior: the real service emits the NextMarker key on every ListTagsForResource 200, with an empty value when there is no further page (the recording expects NextMarker present with value ''). Note the model's NextMarker constraints (length min 1, pattern \\S) do not admit an empty string, so the empty-but-present marker is recorded behavior only, not a modeled requirement — the requirement being asserted here is that the key is emitted on the wire at all.
Emulator: returns HTTP 200 with the member entirely absent — replay reports "$.NextMarker: missing (expected '', got None)" for both the post-TagResource and post-UntagResource list steps.
Why: paginating clients and strict response-shape consumers distinguish "key absent" from "key present and empty". Code that reads the marker unconditionally (XML/JSON field access, a loop that tests the marker's emptiness rather than its presence) breaks or takes a different branch against the emulator than against AWS, so tag-pagination logic cannot be exercised faithfully.
Fix: the ListTagsForResource response serializer in the waf-regional service crate — emit the NextMarker member on every successful response, empty when the listing is complete, rather than skipping it.
Traces: waf-regional-tag-resource-roundtrip#task#4, waf-regional-tag-resource-roundtrip#task#6.

Related but not covering this: the open issue "waf-regional: exception messages are synthesized from the request instead of WAF Classic's fixed per-exception sentence..." (20261022T031012Z-waf-regional-exception-messages-are-synthesized-from-the.md) concerns error-message text, not these success-response members; neither divergence was reported as owned by it.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200; 26 steps, 14 passed, 8 mismatched, 4 blocked, score 53.8%; state_reset_failures=0, so neither divergence is cross-fixture state bleed).

Cause 1 — fixture waf-regional-regex-pattern-match-set-roundtrip, step task#5, operation GetRegexPatternSet:
  expected status 200, actual status 200
  detail: $.RegexPatternSet.RegexPatternStrings[0]: missing (expected '^admin-[0-9]+$', got None)
  verdict: mismatch; divergence_key service=waf-regional|GetRegexPatternSet|status=200->200|keys=$.RegexPatternSet.RegexPatternStrings[0]
  affected: waf-regional-regex-pattern-match-set-roundtrip#task#5
  Expected: RegexPatternSet.RegexPatternStrings contains the pattern string inserted by the preceding UpdateRegexPatternSet (recorded example value '^admin-[0-9]+$').
  Actual: RegexPatternStrings has no such element.

Cause 2 — fixture waf-regional-tag-resource-roundtrip, step task#4, operation ListTagsForResource:
  expected status 200, actual status 200
  detail: $.NextMarker: missing (expected '', got None)
  verdict: mismatch; divergence_key service=waf-regional|ListTagsForResource|status=200->200|keys=$.NextMarker
  affected: waf-regional-tag-resource-roundtrip#task#4, waf-regional-tag-resource-roundtrip#task#6
  Expected: NextMarker member present on the 200 response (recorded example value: empty string).
  Actual: member absent from the response body.

Model evidence: GetRegexPatternSet output -> RegexPatternSet; RegexPatternSet.RegexPatternStrings @required (list, length 0..10). UpdateRegexPatternSet input Updates @required with RegexPatternSetUpdate{Action: ChangeAction @required, RegexPatternString @required length 1..512}. ListTagsForResourceResponse members: NextMarker, TagInfoForResource.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from the checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).

Cause 1 — fixture waf-regional-regex-pattern-match-set-roundtrip, step task#5:
3. Call CreateRegexPatternSet to create a regex pattern set and note its RegexPatternSetId.
4. Call UpdateRegexPatternSet for that id with Updates=[{Action: INSERT, RegexPatternString: <a pattern, e.g. '^admin-[0-9]+$'>}] and the current ChangeToken.
5. Call GetRegexPatternSet with that RegexPatternSetId. Observe HTTP 200 whose RegexPatternSet.RegexPatternStrings does not contain the inserted pattern (element [0] absent); AWS returns it in the list.

Cause 2 — fixture waf-regional-tag-resource-roundtrip, steps task#4 and task#6:
6. Create a taggable waf-regional resource and call TagResource on its ARN.
7. Call ListTagsForResource with that ResourceARN. Observe HTTP 200 whose body has no NextMarker key; AWS emits NextMarker (empty when the listing is complete). The same omission repeats on the ListTagsForResource after UntagResource (step task#6).
