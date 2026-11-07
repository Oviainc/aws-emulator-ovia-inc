Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-association-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-association-lifecycle guardduty-association-list-filters-across-two-rules guardduty-custom-detection-rule-association-lifecycle
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/10ed933b467369f8dad0257d44760002"].associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/e051db952e71e30a542aae5471c50002"].associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-user/association/255e8c41fa5576f64d4a731574860003"].associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/dcef5915ec19b50ac0a5de4df6ed0002"].associationId
Issue-Commit: 09ede02538826dd718336321255b799d2c318d86

# ListCustomDetectionRuleAssociations summary reports an associationId that is not interchangeable with its own ARN suffix

> _Note: this issue was identified against the code base at commit `09ede02538826dd718336321255b799d2c318d86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCustomDetectionRuleAssociations

## Description
This issue bundles 4 distinct net-new replay clusters, all in the guardduty crate, all on ListCustomDetectionRuleAssociations, and all on the SAME fault: the `associationId` member the emulator serializes into an AssociationSummary is not interchangeable with the association identity that same entry advertises in its own `arn`. Fix ALL 4 and re-replay every fixture#step named in Traces (the representative in Issue-Trace/Issue-Step plus the three in Issue-Trace-Ids) before submitting. None of the 4 clusters is marked owned by an open issue and none is blocked by a stub.

Shared fault: the list handler builds each summary's id from a source that is not the one the entry's ARN is derived from, so the id a listed entry advertises cannot be fed back as the `AssociationId` path label of a following Get/Update/Delete. The replay's deterministic comparator reports the member as a mismatch even though the expected and actual values RENDER with the same visible digits, so the difference lies in a dimension the rendering collapses (JSON type - e.g. a non-string - case, or surrounding whitespace in the serialized member) rather than in the digits themselves. The digits, the account/region parts of the ARN and the `0002`/`0003` counter suffixes are per-run values and are NOT the requirement; the requirement is that one summary's `associationId` is byte-identical to the trailing `/association/<id>` segment of that same summary's `arn`, and is accepted verbatim as the `AssociationId` path label.

Note for the fixer: an OPEN issue, 20261107T064647Z-guardduty-custom-detection-rule-association-identity-is-not-stable.md, describes this same fault family and its Issue-Key lines name ListCustomDetectionRuleAssociations clusters - but those keys embed the ARNs (and therefore the minted ids) of an EARLIER run, so they do not join to this run's clusters and the replay reports all four here as unowned. A RESOLVED issue, 20261103T231304Z-guardduty-custom-detection-rule-association-identity-diverges-across.md, is marked fixed and does not cover them either. Treat the list projection as still-open behaviour.

Cause 1 of 4: the association-lifecycle listing reports a summary whose `associationId` does not agree with the id embedded in that same entry's `arn`. Fixture guardduty-association-lifecycle, step task#7.
AWS: Smithy ListCustomDetectionRuleAssociations (GET /custom-detection-rule/association, code 200; query params MaxResults @range 1..100, NextToken, RuleId @length 1..100 @pattern ^[a-z0-9]+(-[a-z0-9]+)*$, Mode; readonly; paginated inputToken=NextToken outputToken=NextToken items=RuleAssociations pageSize=MaxResults; errors AccessDeniedException 403, BadRequestException 400, InternalServerErrorException 500) returns a required `ruleAssociations` list of AssociationSummary. Smithy AssociationSummary.AssociationId -> AssociationId, jsonName `associationId`, required, @length 1..64, @pattern ^[a-zA-Z0-9_-]{1,64}$, doc "The unique identifier for the association"; AssociationSummary.Arn -> DetectionRuleArn, jsonName `arn`, required, @length 1..1024, @pattern ^arn:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]*:([0-9]*|aws):.+$, doc "The Amazon Resource Name (ARN) of the association". Smithy GetCustomDetectionRuleAssociation (GET /custom-detection-rule/rule/{RuleId}/association/{AssociationId}) takes AssociationId as a required @httpLabel with the same @length/@pattern, which is what makes the listed id a usable handle. Recorded behavior only for the VALUE: the recording pins each summary's `associationId` to the identity the scenario's create established, and the entry's ARN ends in exactly that same id.
Emulator: HTTP 200 matches; the replay selects the entry by its own `arn` and reports `$.ruleAssociations[arn="...detection-rule/custom/admin-policy-attached-to-role/association/<id>"].associationId: value_mismatch`, with expected and actual rendering identically. The serialized member therefore differs from the id its own ARN names in a non-printing dimension.
Why: list-then-act is the normal discovery path. A client that lists associations and then calls GetCustomDetectionRuleAssociation / UpdateCustomDetectionRuleAssociation / DeleteCustomDetectionRuleAssociation with the id it just read addresses a handle the service does not recognize (404), and ARN-keyed IAM policy or inventory joins written against the listed ARN stop matching the id-keyed calls.
Fix: the AssociationSummary projection of the ListCustomDetectionRuleAssociations handler and the shared association view/ARN helper it serializes through, so the id member and the ARN's trailing segment are derived from one value and emitted as the same JSON string.
Traces: guardduty-association-lifecycle#task#7 (cluster affected_count 1).

Cause 2 of 4: the same list-summary identity divergence on the FIRST rule of the two-rule filtered listing. Fixture guardduty-association-list-filters-across-two-rules, step task#5.
AWS: same ListCustomDetectionRuleAssociations / AssociationSummary citation as cause 1 (RuleId is an @httpQuery filter; `ruleAssociations` required; each entry's `associationId` and `arn` required). Recorded behavior only: a listing filtered by RuleId reports each association under the identity it was created with, with the ARN suffix equal to that entry's `associationId`.
Emulator: 200 matches; the entry located by its own ARN (ruleId admin-policy-attached-to-role) reports a mismatching `associationId`, rendered digits identical to expected.
Why: a caller filtering by RuleId cannot trust the ids it reads back, so a filtered discovery pass yields handles that do not resolve on the following call.
Fix: same list-handler projection as cause 1; a separate cluster with its own fixture and step that must be re-replayed on its own.
Traces: guardduty-association-list-filters-across-two-rules#task#5 (cluster affected_count 1).

Cause 3 of 4: the same divergence on the SECOND rule of that filtered listing - a different ruleId, hence a distinct association record and a distinct cluster. Fixture guardduty-association-list-filters-across-two-rules, step task#6.
AWS: same citation as cause 1; AssociationSummary.RuleId is itself required, so each entry is self-describing and its `associationId` must be the handle for that entry's own rule.
Emulator: 200 matches; the entry for ruleId admin-policy-attached-to-user, selected by its own ARN, reports a mismatching `associationId` (rendered digits identical).
Why: the defect is per-association rather than per-listing, so a caller listing across two rules receives unusable handles for both and neither rule's association can be acted on from the listing.
Fix: same list-handler projection as cause 1.
Traces: guardduty-association-list-filters-across-two-rules#task#6 (cluster affected_count 1).

Cause 4 of 4: the same divergence on the second association-lifecycle fixture's final listing. Fixture guardduty-custom-detection-rule-association-lifecycle, step task#7.
AWS: same citation as cause 1. Recorded behavior only: after create/get/update the listing reports the association under the identity the lifecycle established, with the ARN suffix equal to that entry's `associationId`.
Emulator: 200 matches; the entry selected by its ARN reports a mismatching `associationId` (rendered digits identical).
Why: the lifecycle's final discovery step hands the caller an id that does not round-trip, so a delete issued from the listing addresses nothing and the association is leaked.
Fix: same list-handler projection as cause 1.
Traces: guardduty-custom-detection-rule-association-lifecycle#task#7 (cluster affected_count 1).

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the booted emulator: emulator_up=true, bootstrap_signal=false, 86 fixtures replayed, 559 steps, 516 passed, 43 mismatched, 0 errored, 0 blocked, not_implemented=0, score 92.3%. 29 divergence clusters; 25 are marked owned_by an open guardduty issue, and the 4 below carry no owner (owned_by=[]) and no stub (stub_path=""). Expected = the recorded AWS response, got = the emulator.

Cause 1 - guardduty-association-lifecycle#task#7, ListCustomDetectionRuleAssociations, expected status 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/10ed933b467369f8dad0257d44760002"].associationId: value_mismatch (expected '10ed933b467369f8dad0257d44760002', got '10ed933b467369f8dad0257d44760002')
  divergence_key: service=guardduty|ListCustomDetectionRuleAssociations|status=200->200|keys=$.ruleAssociations[arn="...association/10ed933b467369f8dad0257d44760002"].associationId
  affected_count 1. The two rendered values print identically, so the compared difference is not in the visible digits; the deterministic comparator still reports the member as a mismatch. The id digits and the account/region of the ARN are per-run values and are not the requirement.

Cause 2 - guardduty-association-list-filters-across-two-rules#task#5, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/e051db952e71e30a542aae5471c50002"].associationId: value_mismatch (expected 'e051db952e71e30a542aae5471c50002', got 'e051db952e71e30a542aae5471c50002')
  affected_count 1.

Cause 3 - guardduty-association-list-filters-across-two-rules#task#6, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-user/association/255e8c41fa5576f64d4a731574860003"].associationId: value_mismatch (expected '255e8c41fa5576f64d4a731574860003', got '255e8c41fa5576f64d4a731574860003')
  affected_count 1 (note the different ruleId, admin-policy-attached-to-user, i.e. a second association record).

Cause 4 - guardduty-custom-detection-rule-association-lifecycle#task#7, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/dcef5915ec19b50ac0a5de4df6ed0002"].associationId: value_mismatch (expected 'dcef5915ec19b50ac0a5de4df6ed0002', got 'dcef5915ec19b50ac0a5de4df6ed0002')
  affected_count 1.

Model evidence used (on-disk Smithy, services/service-com-amazonaws-guardduty-guarddutyapiservice/smithy-model/com-amazonaws-guardduty-guarddutyapiservice.json): ListCustomDetectionRuleAssociations GET /custom-detection-rule/association -> required `ruleAssociations` of AssociationSummary; AssociationSummary.associationId required @length 1..64 @pattern ^[a-zA-Z0-9_-]{1,64}$; AssociationSummary.arn required DetectionRuleArn @pattern ^arn:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]*:([0-9]*|aws):.+$; GetCustomDetectionRuleAssociation takes AssociationId as a required @httpLabel with the identical length/pattern constraint.

Related filed issues (neither covers these four clusters): open 20261107T064647Z-guardduty-custom-detection-rule-association-identity-is-not-stable.md carries Issue-Key lines for ListCustomDetectionRuleAssociations, but each embeds a PRIOR run's ARN/id (e5d07a4c..., b7ec5eee..., 01a3967b..., a255b621...), so none joins this run's keys and the replay reports these clusters with owned_by=[]; resolved 20261103T231304Z-guardduty-custom-detection-rule-association-identity-diverges-across.md is marked fixed.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator, then replay the simparity-wire-aws-v1 wire corpus. Confirm emulator_up=true and bootstrap_signal=false before reading any divergence.
2. Cause 1: replay fixture guardduty-association-lifecycle through step task#7 (GET /custom-detection-rule/association). Capture the RAW response body. For the entry whose `arn` ends '/association/<id>', compare that entry's serialized `associationId` member byte-for-byte with the ARN's trailing segment - including JSON type, case and surrounding whitespace, since the two values render identically in the replay report.
3. Cause 2: replay fixture guardduty-association-list-filters-across-two-rules through step task#5 and make the same ARN-suffix vs `associationId` byte comparison for the admin-policy-attached-to-role entry.
4. Cause 3: continue the same fixture to step task#6 and repeat the comparison for the admin-policy-attached-to-user entry.
5. Cause 4: replay fixture guardduty-custom-detection-rule-association-lifecycle through step task#7 and repeat the comparison.
6. Harness-free repro: ensure a detector exists (ListDetectors, else CreateDetector(Enable=true)); ListCustomDetectionRules to pick a published ruleId (e.g. 'admin-policy-attached-to-role'); CreateCustomDetectionRuleAssociation(RuleId=<ruleId>, Mode='DRY_RUN'); then GET /custom-detection-rule/association (and again with ?ruleId=<ruleId>). For every entry assert (a) the serialized `associationId` is a JSON string byte-identical to the trailing '/association/<id>' segment of that entry's own `arn`, and (b) GetCustomDetectionRuleAssociation(RuleId=<entry.ruleId>, AssociationId=<entry.associationId>) returns 200 for that same association. Repeat with a second ruleId so two association records are listed together.
7. Re-replay all four fixture#steps - guardduty-association-lifecycle#task#7, guardduty-association-list-filters-across-two-rules#task#5, guardduty-association-list-filters-across-two-rules#task#6, guardduty-custom-detection-rule-association-lifecycle#task#7 - and confirm all four clusters are gone before submitting.
