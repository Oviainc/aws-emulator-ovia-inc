Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-association-duplicate-conflict
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-association-duplicate-conflict guardduty-association-list-filters-across-two-rules guardduty-custom-detection-rule-association-errors guardduty-association-lifecycle guardduty-custom-detection-rule-association-lifecycle
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|CreateCustomDetectionRuleAssociation|2xx->2xx|$.ruleAssociation.arn
Issue-Key: guardduty|CreateCustomDetectionRuleAssociation|2xx->2xx|$.ruleAssociation.associationId
Issue-Key: guardduty|GetCustomDetectionRuleAssociation|2xx->2xx|$.ruleAssociation.associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/e5d07a4c1109a6747205dd529f240002"].associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/b7ec5eee4178981c3469387ee0920002"].associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-user/association/01a3967b940b0c00dcd4fdf7a8f20003"].associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/a255b62176be2f5fb35c82650e860002"].associationId
Issue-Commit: 56087ceb4bc65915084aa9f243ba54360138be3f

# Custom detection rule association identity is not stable: create re-mints it and read/list disagree with it

> _Note: this issue was identified against the code base at commit `56087ceb4bc65915084aa9f243ba54360138be3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomDetectionRuleAssociation

## Description
This issue bundles 7 distinct net-new replay clusters, all in the guardduty crate and all on the IDENTITY of a custom detection rule association (`associationId`, and the `arn` whose trailing segment is derived from it) as create, read and list report it. Fix ALL 7 and re-replay every fixture#step named in Traces before submitting. None of the 7 clusters is marked owned by an open issue and none is blocked by a stub.

The shared fault: the emulator treats an association's id as a value it is free to mint afresh on every create, and the id it then serializes on the read and list projections is not interchangeable with the id the caller resolved the association by. Note for the fixer: a RESOLVED issue, 20261103T231304Z-guardduty-custom-detection-rule-association-identity-diverges-across.md, described the same fault family (4 causes) and is marked fixed, but the replay still reports all of these clusters, now split into 7 — so it does NOT cover them; treat this as still-open behaviour and re-verify the create/read/list id path end to end. Two open guardduty association issues also do not cover it: 20261101T205734Z-guardduty-deletecustomdetectionruleassociation-is-not-idempotent-deleting-an-already-deleted.md (delete idempotency) and 20261030T120002Z-guardduty-iam-authorization-is-not-enforced-on-createcustomdetectionruleassociation.md (authorization and not-found wording).

Cause 1 of 7: CreateCustomDetectionRuleAssociation answers with a freshly minted `associationId`, and the association `arn` inherits that same wrong id, where the recording carries the association identity the scenario has already bound. Fixture guardduty-association-duplicate-conflict, step task#3.
AWS: Smithy CreateCustomDetectionRuleAssociation (POST; input RuleId required, @length 1..100, @pattern ^[a-z0-9]+(-[a-z0-9]+)*$; Mode required (LIVE|DRY_RUN); ClientToken @idempotencyToken @length 0..64; errors AccessDeniedException 403, BadRequestException 400, ConflictException 409, ResourceNotFoundException 404) returns AssociationDetail. Smithy AssociationDetail.AssociationId -> AssociationId, jsonName `associationId`, required, @length 1..64, @pattern ^[a-zA-Z0-9_-]{1,64}$, "The unique identifier for the association"; AssociationDetail.Arn -> DetectionRuleArn, jsonName `arn`, required, @length 1..1024, @pattern ^arn:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]*:([0-9]*|aws):.+$. Recorded behavior only for the id's VALUE: the recording does not pin a literal id, it pins the id to the association identity the scenario already holds (the replay renders it as the unresolved template `{{assoc_conflict_id}}`) and requires `arn` to end in exactly that same id. One association of one (account, region, ruleId) therefore has ONE id for the whole scenario and every response naming it repeats that id.
Emulator: HTTP 200 matches, but `$.ruleAssociation.associationId` is a brand-new 32-hex id (this run: 'f2595d7aac3650e82ae4385007c70002') and `$.ruleAssociation.arn` is 'arn:aws:guardduty:<region>:<account>:detection-rule/custom/<ruleId>/association/<that new id>'. The id digits, the host/region/account parts of the ARN and the counter suffix are per-run values and are NOT the requirement; the requirement is that the id equals the association's established identity and that the ARN's trailing `/association/<id>` segment equals the `associationId` in the same payload.
Why: a client that creates an association and then re-issues the create (a retry, a duplicate-conflict probe, or a replay whose ClientToken it did not keep) is handed an id that names nothing it can subsequently resolve — Get/Update/Delete are keyed by (RuleId, AssociationId) and the store keeps one association per (region, ruleId), so the caller's stored handle and the handle just reported disagree and one of them 404s. ARN-keyed IAM policy and inventory joins written against the first ARN stop matching.
Fix: the create handler's id-resolution path and the AssociationDetail it builds, in the guardduty crate's create operation and the shared association view/ARN helper it calls, so a create for an association that already exists resolves the stored identity instead of minting a second one.
Traces: guardduty-association-duplicate-conflict#task#3 (cluster also covers guardduty-association-list-filters-across-two-rules#task#3, guardduty-association-list-filters-across-two-rules#task#4, guardduty-custom-detection-rule-association-errors#task#3).

Cause 2 of 7: GetCustomDetectionRuleAssociation's read-back reports an `associationId` that is not interchangeable with the id the request resolved the association by. Fixture guardduty-association-duplicate-conflict, step task#5.
AWS: Smithy GetCustomDetectionRuleAssociation (GET /custom-detection-rule/rule/{RuleId}/association/{AssociationId}; RuleId and AssociationId both @httpLabel and required; AssociationId @length 1..64, @pattern ^[a-zA-Z0-9_-]{1,64}$; readonly) returns AssociationDetail with required `associationId`. Recorded behavior only: the read is identity-preserving — the id used as the path label and the id echoed in the body are the same string, byte for byte.
Emulator: 200 matches, but the replay reports `$.ruleAssociation.associationId: value_mismatch`. The replay renders expected and actual with the same visible digits ('f2595d7aac3650e82ae4385007c70002'), so the compared difference lies in a dimension the rendering collapses (JSON type, case, or whitespace in the serialized member) rather than in the digits — compare the serialized `associationId` member byte-for-byte against the id used as the request path label when re-replaying; do not assume the digits alone are the whole value.
Why: a client that stores the id create returned and reads the association back cannot round-trip its own handle, so caches, ARN construction and any following Update/Delete keyed on the body's value address nothing.
Fix: the AssociationDetail projection of the read handler and the shared association view it is built from, so one projection serves create, get and list.
Traces: guardduty-association-duplicate-conflict#task#5 (cluster also covers guardduty-association-lifecycle#task#4, guardduty-association-lifecycle#task#6, guardduty-custom-detection-rule-association-lifecycle#task#4, guardduty-custom-detection-rule-association-lifecycle#task#6).

Cause 3 of 7: the same create divergence on the association-lifecycle path, where only `associationId` is asserted. Fixture guardduty-association-lifecycle, step task#3.
AWS: same model citation as cause 1 (CreateCustomDetectionRuleAssociation -> AssociationDetail.associationId, required, @pattern ^[a-zA-Z0-9_-]{1,64}$). Recorded behavior only: the response's id is the association identity the scenario binds (rendered `{{assoc_lifecycle_id}}`), which the fixture's later steps use as the path label for Get/Update/Delete and as the ARN suffix for List.
Emulator: `$.ruleAssociation.associationId` is a newly minted id (this run: 'e5d07a4c1109a6747205dd529f240002') instead of that identity; status 200 matches.
Why: the whole create -> get -> update -> list -> delete lifecycle is keyed on the id create returned, so a create that mints a different identity makes every later step address a resource other than the one the caller believes it created.
Fix: same component as cause 1; listed separately because it is its own replay cluster with its own fixture and step and must be re-replayed on its own.
Traces: guardduty-association-lifecycle#task#3 (cluster also covers guardduty-custom-detection-rule-association-lifecycle#task#3).

Cause 4 of 7: ListCustomDetectionRuleAssociations emits a summary whose `associationId` does not agree with the id embedded in that same entry's `arn`. Fixture guardduty-association-lifecycle, step task#7.
AWS: Smithy ListCustomDetectionRuleAssociations (GET; MaxResults @range 1..100, NextToken, RuleId, Mode as query params; readonly; paginated on NextToken/RuleAssociations) returns a required `ruleAssociations` list of AssociationSummary, each with required `associationId` (@length 1..64, @pattern ^[a-zA-Z0-9_-]{1,64}$) and required `arn` (DetectionRuleArn, @pattern ^arn:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]*:([0-9]*|aws):.+$). Recorded behavior only: the two members of one summary are consistent — the ARN's trailing `/association/<id>` segment is exactly that entry's `associationId`, which is what lets a caller select a listed association by ARN and then act on it by id.
Emulator: 200 matches; the replay selects the entry by its own ARN and reports `value_mismatch` on that entry's `associationId`. As in cause 2 the rendered expected and actual read as the same characters, so compare the serialized summary member against the ARN's trailing segment exactly rather than by eye.
Why: list-then-act is the normal discovery path. If the id a listed entry advertises is not the id its own ARN names, a client that lists and then calls Get/Update/Delete with the listed id operates on a handle the service does not recognize, and ARN-keyed policy or inventory joins silently fail.
Fix: the AssociationSummary projection of the list handler, derived from the same shared association view as causes 1-3 so the id and the ARN cannot drift apart.
Traces: guardduty-association-lifecycle#task#7.

Cause 5 of 7: the same list-summary identity divergence on the first rule of the two-rule filter listing. Fixture guardduty-association-list-filters-across-two-rules, step task#5.
AWS: same ListCustomDetectionRuleAssociations / AssociationSummary citation as cause 4; recorded behavior only for the value — a filtered listing reports each association under the identity it was created with, and each entry's ARN suffix equals that entry's `associationId`.
Emulator: 200 matches; `$.ruleAssociations[arn="...association/<id>"].associationId` is reported as a mismatch for the entry located by its own ARN (rendered digits identical, as in causes 2 and 4).
Why: a caller filtering associations by RuleId cannot trust the ids it reads back, so a filtered discovery pass yields handles that do not resolve.
Fix: same list-handler projection as cause 4; separate cluster, separate fixture and step, must be re-replayed on its own.
Traces: guardduty-association-list-filters-across-two-rules#task#5.

Cause 6 of 7: the same list-summary identity divergence on the SECOND rule of the two-rule filter listing (a different ruleId, so a distinct association record and a distinct cluster). Fixture guardduty-association-list-filters-across-two-rules, step task#6.
AWS: same citation as cause 4. Recorded behavior only: the entry for the second rule reports its own association's identity, and its ARN suffix equals its own `associationId`.
Emulator: 200 matches; the entry selected by its ARN reports a mismatching `associationId` (rendered digits identical).
Why: the defect is per-association, not per-listing — a caller that lists across two rules gets unusable handles for both, so neither rule's association can be acted on from the listing.
Fix: same list-handler projection as cause 4.
Traces: guardduty-association-list-filters-across-two-rules#task#6.

Cause 7 of 7: the same list-summary identity divergence on the second association-lifecycle fixture. Fixture guardduty-custom-detection-rule-association-lifecycle, step task#7.
AWS: same citation as cause 4. Recorded behavior only: after create/get/update the listing reports the association under the identity the lifecycle established, with the ARN suffix equal to that entry's `associationId`.
Emulator: 200 matches; the entry selected by its ARN reports a mismatching `associationId` (rendered digits identical).
Why: the lifecycle's final discovery step hands the caller an id that does not round-trip, so a delete issued from the listing addresses nothing.
Fix: same list-handler projection as cause 4.
Traces: guardduty-custom-detection-rule-association-lifecycle#task#7.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the booted emulator: 86 fixtures, 559 steps, 516 passed, 43 mismatched, score 92.3%, emulator_up=true, bootstrap_signal=false, not_implemented=0, steps_blocked=0. 29 divergence clusters; 22 are marked owned by open guardduty issues, and the 7 below carry no owner and no stub. Expected = the recorded AWS response, got = the emulator.

Cause 1 — guardduty-association-duplicate-conflict#task#3, CreateCustomDetectionRuleAssociation, expected status 200, got 200:
  $.ruleAssociation.associationId: value_mismatch (expected '{{assoc_conflict_id}}', got 'f2595d7aac3650e82ae4385007c70002')
  $.ruleAssociation.arn: value_mismatch (expected 'arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/{{assoc_conflict_id}}', got 'arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/f2595d7aac3650e82ae4385007c70002')
  divergence_key: service=guardduty|CreateCustomDetectionRuleAssociation|status=200->200|keys=$.ruleAssociation.arn,$.ruleAssociation.associationId
  affected_count 4: guardduty-association-duplicate-conflict#task#3, guardduty-association-list-filters-across-two-rules#task#3, guardduty-association-list-filters-across-two-rules#task#4, guardduty-custom-detection-rule-association-errors#task#3
  (region/account/ruleId parts of the two ARNs agree; only the trailing id segment diverges, and the id digits are a per-run value, not a requirement)

Cause 2 — guardduty-association-duplicate-conflict#task#5, GetCustomDetectionRuleAssociation, expected 200, got 200:
  $.ruleAssociation.associationId: value_mismatch (expected 'f2595d7aac3650e82ae4385007c70002', got 'f2595d7aac3650e82ae4385007c70002')
  divergence_key: service=guardduty|GetCustomDetectionRuleAssociation|status=200->200|keys=$.ruleAssociation.associationId
  affected_count 5: guardduty-association-duplicate-conflict#task#5, guardduty-association-lifecycle#task#4, guardduty-association-lifecycle#task#6, guardduty-custom-detection-rule-association-lifecycle#task#4, guardduty-custom-detection-rule-association-lifecycle#task#6
  The two rendered values print identically, so the compared difference is not in the visible digits; the deterministic comparator still reports the member as a mismatch on all five steps.

Cause 3 — guardduty-association-lifecycle#task#3, CreateCustomDetectionRuleAssociation, expected 200, got 200:
  $.ruleAssociation.associationId: value_mismatch (expected '{{assoc_lifecycle_id}}', got 'e5d07a4c1109a6747205dd529f240002')
  divergence_key: service=guardduty|CreateCustomDetectionRuleAssociation|status=200->200|keys=$.ruleAssociation.associationId
  affected_count 2: guardduty-association-lifecycle#task#3, guardduty-custom-detection-rule-association-lifecycle#task#3

Cause 4 — guardduty-association-lifecycle#task#7, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/e5d07a4c1109a6747205dd529f240002"].associationId: value_mismatch (expected 'e5d07a4c1109a6747205dd529f240002', got 'e5d07a4c1109a6747205dd529f240002')
  affected_count 1.

Cause 5 — guardduty-association-list-filters-across-two-rules#task#5, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/b7ec5eee4178981c3469387ee0920002"].associationId: value_mismatch (expected 'b7ec5eee4178981c3469387ee0920002', got 'b7ec5eee4178981c3469387ee0920002')
  affected_count 1.

Cause 6 — guardduty-association-list-filters-across-two-rules#task#6, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-user/association/01a3967b940b0c00dcd4fdf7a8f20003"].associationId: value_mismatch (expected '01a3967b940b0c00dcd4fdf7a8f20003', got '01a3967b940b0c00dcd4fdf7a8f20003')
  affected_count 1 (note the different ruleId, admin-policy-attached-to-user, i.e. a second association record).

Cause 7 — guardduty-custom-detection-rule-association-lifecycle#task#7, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/a255b62176be2f5fb35c82650e860002"].associationId: value_mismatch (expected 'a255b62176be2f5fb35c82650e860002', got 'a255b62176be2f5fb35c82650e860002')
  affected_count 1.

Supporting code reading (for the fix, not a requirement): services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/create_custom_detection_rule_association.rs line 245 calls `state.mint_id()` on every create path that is not an exact ClientToken replay (the ClientToken lookup at lines 185-191 is the only reuse path, and the duplicate-rule check at lines 203-223 refuses with ConflictException rather than resolving the stored identity). services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs lines 313-326 document `mint_id` as a 32-hex id whose low 16 bits carry the store's own sequence counter — which is why every id minted in this run ends in a 4-hex counter such as '0002'/'0003'. services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state/custom_detection_rules.rs lines 431-450 (`detection_rule_association_view`) and lines 300-310 (`custom_detection_rule_association_arn`) are the shared projection the create, get and list handlers all serialize through, and state.rs lines 1064-1072 (`detection_rule_association_by_id`) is the resolver that filters the single (region, ruleId) record on `record.association_id == association_id`, so a re-minted id cannot be resolved by a caller holding the earlier one.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator, then replay the simparity-wire-aws-v1 wire corpus. Confirm emulator_up=true and bootstrap_signal=false before reading any divergence.
2. Cause 1: replay fixture guardduty-association-duplicate-conflict up to step task#3 (CreateCustomDetectionRuleAssociation for ruleId 'admin-policy-attached-to-role'). Compare $.ruleAssociation.associationId and $.ruleAssociation.arn with the association identity the scenario bound earlier; the emulator reports a freshly minted id and an ARN whose trailing segment is that new id.
3. Cause 2: continue the same fixture to step task#5 (GET /custom-detection-rule/rule/<ruleId>/association/<associationId>). Capture the raw response body and compare the serialized `associationId` member byte-for-byte with the id sent as the path label — including JSON type, case and whitespace, since the two values render identically in the replay report.
4. Cause 3: replay fixture guardduty-association-lifecycle, step task#3, and compare $.ruleAssociation.associationId with the identity the fixture binds for the lifecycle association.
5. Cause 4: continue guardduty-association-lifecycle to step task#7 (GET /custom-detection-rule/association). For the entry whose `arn` ends '/association/<id>', compare that entry's serialized `associationId` byte-for-byte with the ARN's trailing segment.
6. Causes 5 and 6: replay fixture guardduty-association-list-filters-across-two-rules, steps task#5 and task#6, and make the same ARN-suffix vs associationId comparison for the admin-policy-attached-to-role entry and then for the admin-policy-attached-to-user entry.
7. Cause 7: replay fixture guardduty-custom-detection-rule-association-lifecycle, step task#7, and repeat the same comparison.
8. Harness-free repro of the create path: ensure a detector exists (ListDetectors, else CreateDetector(Enable=true)); ListCustomDetectionRules to pick a published ruleId (e.g. 'admin-policy-attached-to-role'); CreateCustomDetectionRuleAssociation(RuleId=<ruleId>, Mode='DRY_RUN') and record associationId A1 and its arn; issue the same create again with no ClientToken. The second response must resolve to the same association as A1 (one association per account+region+rule), and every response's arn must end in that response's own associationId. Then GetCustomDetectionRuleAssociation(RuleId, AssociationId=A1) and ListCustomDetectionRuleAssociations(RuleId=<ruleId>) and check that every reported associationId is byte-identical to the ARN suffix in the same payload and is accepted as the path label by a following Get.
9. Re-replay all seven fixture#steps (guardduty-association-duplicate-conflict#task#3, guardduty-association-duplicate-conflict#task#5, guardduty-association-lifecycle#task#3, guardduty-association-lifecycle#task#7, guardduty-association-list-filters-across-two-rules#task#5, guardduty-association-list-filters-across-two-rules#task#6, guardduty-custom-detection-rule-association-lifecycle#task#7) plus the other cluster members listed in the evidence before submitting.
