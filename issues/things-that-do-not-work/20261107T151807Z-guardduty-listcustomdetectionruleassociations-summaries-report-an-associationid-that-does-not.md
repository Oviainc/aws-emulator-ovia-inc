Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-association-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-association-lifecycle guardduty-association-list-filters-across-two-rules guardduty-custom-detection-rule-association-lifecycle
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/d2b6f18d239088bfd17b5ebadf7e0002"].associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/a56e1aada89396e1c9b450ad82e70002"].associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-user/association/1f6c3d3b3a6e3a0a661776f934e30003"].associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/7874687fa0d12997717e0cd2224d0002"].associationId
Issue-Commit: ef1bb1e3a86f6fb50b04ae6c71e141a327380e0c

# ListCustomDetectionRuleAssociations summaries report an associationId that does not round-trip

> _Note: this issue was identified against the code base at commit `ef1bb1e3a86f6fb50b04ae6c71e141a327380e0c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCustomDetectionRuleAssociations

## Description
This issue bundles 4 distinct net-new replay clusters, all in the guardduty crate, all on ListCustomDetectionRuleAssociations, and all on the SAME member of the listing: the `associationId` each AssociationSummary advertises for an entry the replay locates by that entry's own `arn`. Fix ALL 4 causes and re-replay every fixture#step named under Traces before submitting. None of the 4 clusters is marked owned by an open issue (owned_by is empty for each) and none is blocked by a stub.

Shared fault: the identity an association advertises in the list projection is not interchangeable with the identity the recording establishes for that association, so a list-then-act client gets a handle that does not resolve. In every one of the 4 clusters the status is correct (200) and every other member of the summary matches; only `associationId` is reported as a value_mismatch, and the replay renders the expected and the actual value with the SAME visible characters. That means the compared difference is NOT in the visible digits: it lies in a dimension the report's rendering collapses (the serialized member's JSON type, its case, or surrounding whitespace), or in the identity the entry is bound to rather than in how it prints. A fixer MUST compare the serialized `associationId` member byte-for-byte against the trailing `/association/<id>` segment of the `arn` in the same entry, and against the id a following Get accepts as its path label — not by eye against the report.

Relation to already-open work (named, not relied on): 20261107T064647Z-guardduty-custom-detection-rule-association-identity-is-not-stable.md is open and describes this same fault family, including list clusters, but its replay keys embed per-run association ids (e5d07a4c..., b7ec5eee..., 01a3967b..., a255b621...) that do not match this run's ids (d2b6f18d..., a56e1aad..., 1f6c3d3b..., 7874687f...), which is why the replay reports these four clusters as unowned. It therefore does NOT carry these clusters' replay keys and does not cover them. A resolved issue, 20261103T231304Z-guardduty-custom-detection-rule-association-identity-diverges-across.md, is marked fixed yet the replay still reports the behaviour, so it covers nothing here either. Treat these four as still-open behaviour.

Cause 1 of 4: the single-rule lifecycle listing reports an `associationId` that is not interchangeable with the identity the lifecycle established for the association it is listing. Fixture guardduty-association-lifecycle, step task#7.
AWS: Smithy ListCustomDetectionRuleAssociations (GET /custom-detection-rule/association, success 200, readonly, paginated inputToken NextToken / outputToken NextToken / items RuleAssociations / pageSize MaxResults; input MaxResults @range 1..100, NextToken, RuleId @length 1..100 @pattern ^[a-z0-9]+(-[a-z0-9]+)*$, Mode (AssociationMode) all as httpQuery; errors AccessDeniedException 403, BadRequestException 400, InternalServerErrorException 500). Output member RuleAssociations -> AssociationSummaryList, jsonName `ruleAssociations`, required. Smithy AssociationSummary.AssociationId -> AssociationId, jsonName `associationId`, required, @length 1..64, @pattern ^[a-zA-Z0-9_-]{1,64}$, "The unique identifier for the association"; AssociationSummary.Arn -> DetectionRuleArn, jsonName `arn`, required, @length 1..1024, @pattern ^arn:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]*:([0-9]*|aws):.+$, "The Amazon Resource Name (ARN) of the association". Recorded behavior only for the VALUE: the recording does not pin a literal id; it requires the listed entry to carry the identity the association already holds, and requires the two members of one summary to agree — the ARN's trailing `/association/<id>` segment is exactly that entry's own `associationId`. That agreement is what lets a caller select an association from a listing by ARN and then act on it by id.
Emulator: HTTP 200 as recorded, and the entry is present and selectable by its ARN, but the comparator reports `$.ruleAssociations[arn="...detection-rule/custom/<ruleId>/association/<id>"].associationId` as a value_mismatch. The two sides render as the same characters in the report, so the diverging dimension is one the rendering collapses. The specific id digits, the region/account parts of the ARN and the counter suffix on a minted id are per-run values and are NOT the requirement; the requirement is that the member equals the entry's own ARN suffix and is accepted as a path label by a following Get/Update/Delete.
Why: list-then-act is the normal discovery path for associations. A client that lists associations and then calls GetCustomDetectionRuleAssociation / UpdateCustomDetectionRuleAssociation / DeleteCustomDetectionRuleAssociation with the id a listed entry advertises addresses a handle the service does not recognise, so the call 404s or acts on the wrong record; ARN-keyed IAM policy and inventory joins built from the listing stop matching the id the same payload reports.
Fix: the AssociationSummary projection of the list handler and the shared association view/ARN helper it derives from, so one projection serves create, get and list and the id cannot drift from the ARN it is embedded in. (File/line citations are under Traces.)
Traces: guardduty-association-lifecycle#task#7 (affected_count 1).

Cause 2 of 4: the same list-summary identity divergence on the FIRST rule of the two-rule filtered listing (RuleId filter applied). Fixture guardduty-association-list-filters-across-two-rules, step task#5.
AWS: same ListCustomDetectionRuleAssociations / AssociationSummary citation as cause 1 — RuleId is an optional httpQuery filter narrowing by equality, and the operation models no ResourceNotFoundException, so a filter simply selects a subset. Recorded behavior only for the value: a filtered listing reports each association under the identity it already holds, with each entry's ARN suffix equal to that entry's `associationId`; filtering changes which entries appear, never the identity an entry advertises.
Emulator: 200 as recorded; the entry located by its own `arn` for the admin-policy-attached-to-role rule reports a mismatching `associationId`, again rendering identically to the expected value.
Why: a caller narrowing the listing to one rule cannot trust the ids it reads back, so a filtered discovery pass yields handles that do not resolve — the filtered path is the one automation uses, so this is the most-used variant of the defect.
Fix: same list-handler projection as cause 1; filed as its own cause because it is its own replay cluster with its own fixture and step and must be re-replayed on its own.
Traces: guardduty-association-list-filters-across-two-rules#task#5 (affected_count 1).

Cause 3 of 4: the same divergence on the SECOND rule of the two-rule filtered listing — a different ruleId, hence a distinct association record and a distinct cluster. Fixture guardduty-association-list-filters-across-two-rules, step task#6.
AWS: same citation as cause 1. Recorded behavior only: the entry for the second rule reports its own association's identity, and its ARN suffix equals its own `associationId`.
Emulator: 200 as recorded; the entry selected by its ARN under the admin-policy-attached-to-user rule reports a mismatching `associationId` (rendered characters identical).
Why: the defect is per-association, not per-listing — a caller listing across two rules gets unusable handles for BOTH, so neither rule's association can be acted on from the listing. It also shows the fault is not specific to one rule id or one scenario.
Fix: same list-handler projection as cause 1.
Traces: guardduty-association-list-filters-across-two-rules#task#6 (affected_count 1).

Cause 4 of 4: the same divergence on the second association-lifecycle fixture's final discovery step, after create/get/update have run. Fixture guardduty-custom-detection-rule-association-lifecycle, step task#7.
AWS: same citation as cause 1. Recorded behavior only: after a create/get/update sequence the listing reports the association under the identity that lifecycle established, with the ARN suffix equal to that entry's `associationId`, so the delete the caller issues from the listing addresses the resource it just updated.
Emulator: 200 as recorded; the entry selected by its ARN reports a mismatching `associationId` (rendered characters identical).
Why: the lifecycle's final discovery step hands the caller an id that does not round-trip, so a delete or update issued from the listing addresses nothing and the resource is leaked.
Fix: same list-handler projection as cause 1.
Traces: guardduty-custom-detection-rule-association-lifecycle#task#7 (affected_count 1).

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the booted emulator: emulator_up=true ("/__admin/health answered 200 and the :45913 data listener accepts"), bootstrap_signal=false, not_implemented=0, steps_blocked=0, steps_errored=0. 86 fixtures replayed, 559 steps, 516 passed, 43 mismatched, score 92.3%, 29 divergence clusters. 25 of the 29 clusters are marked owned_by an open guardduty issue; the 4 below carry owned_by=[] and stub_path="". Expected = the recorded AWS response, got = the emulator.

Cause 1 — guardduty-association-lifecycle#task#7, ListCustomDetectionRuleAssociations, expected status 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/d2b6f18d239088bfd17b5ebadf7e0002"].associationId: value_mismatch (expected 'd2b6f18d239088bfd17b5ebadf7e0002', got 'd2b6f18d239088bfd17b5ebadf7e0002')
  divergence_key: service=guardduty|ListCustomDetectionRuleAssociations|status=200->200|keys=$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/d2b6f18d239088bfd17b5ebadf7e0002"].associationId
  affected_count 1. owned_by: none. stub_path: none.
  The two rendered values print identically, so the compared difference is not in the visible digits; the deterministic comparator still reports the member as a mismatch. The id digits and the trailing counter ('0002') are per-run values of this emulator's id minting and are not part of the requirement.

Cause 2 — guardduty-association-list-filters-across-two-rules#task#5, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/a56e1aada89396e1c9b450ad82e70002"].associationId: value_mismatch (expected 'a56e1aada89396e1c9b450ad82e70002', got 'a56e1aada89396e1c9b450ad82e70002')
  affected_count 1. owned_by: none.

Cause 3 — guardduty-association-list-filters-across-two-rules#task#6, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-user/association/1f6c3d3b3a6e3a0a661776f934e30003"].associationId: value_mismatch (expected '1f6c3d3b3a6e3a0a661776f934e30003', got '1f6c3d3b3a6e3a0a661776f934e30003')
  affected_count 1. owned_by: none. Note the different ruleId (admin-policy-attached-to-user), i.e. a second association record.

Cause 4 — guardduty-custom-detection-rule-association-lifecycle#task#7, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/7874687fa0d12997717e0cd2224d0002"].associationId: value_mismatch (expected '7874687fa0d12997717e0cd2224d0002', got '7874687fa0d12997717e0cd2224d0002')
  affected_count 1. owned_by: none.

Model evidence (smithy_query on services/service-com-amazonaws-guardduty-guarddutyapiservice/smithy-model/com-amazonaws-guardduty-guarddutyapiservice.json):
  ListCustomDetectionRuleAssociations: GET /custom-detection-rule/association, code 200, readonly, paginated {inputToken NextToken, outputToken NextToken, items RuleAssociations, pageSize MaxResults}; output RuleAssociations (jsonName ruleAssociations, required) + NextToken; errors AccessDeniedException 403, BadRequestException 400, InternalServerErrorException 500.
  AssociationSummary: associationId (required, AssociationId, @length 1..64, @pattern ^[a-zA-Z0-9_-]{1,64}$), arn (required, DetectionRuleArn, @length 1..1024, @pattern ^arn:...), ruleId (required), mode (required), createdAt/updatedAt (required), expiresAt (optional).

Supporting code reading (for the fix, not a requirement): services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/list_custom_detection_rule_associations.rs lines 248-264 (`summary`) build the AssociationSummary, taking `association_id` from `record.association_id` at line 250 and deriving `arn` through crate::state::custom_detection_rule_association_arn from that same field at lines 251-255 — so the two members agree with each other but inherit whatever identity the record holds. services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state/custom_detection_rules.rs lines 431-450 (`detection_rule_association_view`) is the matching detail projection, and lines 300-310 hold the shared ARN helper. The record's identity is written by the create path, services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/create_custom_detection_rule_association.rs (its id-resolution/mint call), and resolved for reads by state.rs `detection_rule_association_by_id`, which filters on `record.association_id == association_id`.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator, then replay the simparity-wire-aws-v1 wire corpus. Confirm emulator_up=true and bootstrap_signal=false before reading any divergence.
2. Cause 1: replay fixture guardduty-association-lifecycle through step task#7 (GET /custom-detection-rule/association). Capture the RAW response body. For the entry whose `arn` ends '/association/<id>', compare that entry's serialized `associationId` byte-for-byte with the ARN's trailing segment and with the id the fixture's earlier create/get steps bound — including JSON type, case and whitespace, because the two values render identically in the replay report.
3. Cause 2: replay fixture guardduty-association-list-filters-across-two-rules, step task#5, and make the same ARN-suffix vs associationId byte comparison for the admin-policy-attached-to-role entry.
4. Cause 3: same fixture, step task#6, and repeat the comparison for the admin-policy-attached-to-user entry (a second association record under a different ruleId).
5. Cause 4: replay fixture guardduty-custom-detection-rule-association-lifecycle, step task#7, and repeat the comparison after that fixture's create/get/update steps have run.
6. Harness-free repro: ensure a detector exists (ListDetectors, else CreateDetector(Enable=true)); ListCustomDetectionRules to pick a published ruleId (e.g. 'admin-policy-attached-to-role'); CreateCustomDetectionRuleAssociation(RuleId=<ruleId>, Mode='DRY_RUN') and record the returned associationId A1 and arn. Then call ListCustomDetectionRuleAssociations with no filter, and again with RuleId=<ruleId>. For EVERY entry returned, assert (a) the serialized `associationId` is byte-identical to the trailing '/association/<id>' segment of that same entry's `arn`, and (b) GetCustomDetectionRuleAssociation(RuleId=<ruleId>, AssociationId=<the listed associationId>) answers 200 for the same association rather than 404. Repeat with a second rule id so two association records are listed at once.
7. Re-replay all four fixture#steps (guardduty-association-lifecycle#task#7, guardduty-association-list-filters-across-two-rules#task#5, guardduty-association-list-filters-across-two-rules#task#6, guardduty-custom-detection-rule-association-lifecycle#task#7) and confirm all four clusters pass before submitting; closing fewer than four leaves this issue's other causes unexercised.
