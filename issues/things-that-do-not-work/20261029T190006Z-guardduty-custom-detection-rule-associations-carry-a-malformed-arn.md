Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-association-duplicate-conflict
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-association-duplicate-conflict guardduty-association-list-filters-across-two-rules guardduty-association-lifecycle guardduty-custom-detection-rule-association-errors guardduty-custom-detection-rule-association-lifecycle
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|CreateCustomDetectionRuleAssociation|2xx->2xx|$.ruleAssociation.arn
Issue-Key: guardduty|CreateCustomDetectionRuleAssociation|2xx->2xx|$.ruleAssociation.associationId
Issue-Key: guardduty|GetCustomDetectionRuleAssociation|2xx->2xx|$.ruleAssociation.arn
Issue-Key: guardduty|GetCustomDetectionRuleAssociation|2xx->2xx|$.ruleAssociation.associationId
Issue-Key: guardduty|CreateCustomDetectionRuleAssociation|2xx->2xx|$.ruleAssociation.expiresAt
Issue-Key: guardduty|GetCustomDetectionRuleAssociation|2xx->2xx|$.ruleAssociation.expiresAt
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn
Issue-Key: guardduty|ListCustomDetectionRules|2xx->2xx|$.rules[*]
Issue-Key: guardduty|CreateCustomDetectionRuleAssociation|2xx->4xx|$.ruleAssociation
Issue-Key: guardduty|CreateCustomDetectionRuleAssociation|2xx->4xx|status
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[*]
Issue-Key: guardduty|CreateCustomDetectionRuleAssociation|4xx->4xx|$.__type
Issue-Key: guardduty|CreateCustomDetectionRuleAssociation|4xx->4xx|$.message
Issue-Key: guardduty|CreateCustomDetectionRuleAssociation|4xx->4xx|$.type
Issue-Key: guardduty|CreateCustomDetectionRuleAssociation|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: guardduty|CreateCustomDetectionRuleAssociation|4xx->4xx|status
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# Custom detection rule associations carry a malformed ARN and drop expiresAt

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomDetectionRuleAssociation

## Description
The custom-detection-rule association surface is wrong in three independent ways: the association ARN omits the '/custom/' path segment, a DRY_RUN association never gets its expiresAt, and the published rule catalog contains the wrong rules so associations for valid rule ids cannot be created or listed. 10 causes.

Shared Smithy grounding: AssociationDetail models AssociationId (required, @pattern ^[a-zA-Z0-9_-]{1,64}$), Arn (required, target DetectionRuleArn), RuleId (required, @pattern ^[a-z0-9]+(-[a-z0-9]+)*$), AccountId (required), Mode (required, LIVE | DRY_RUN), CreatedAt and UpdatedAt (required) and the optional ExpiresAt, documented as 'The timestamp when the association expires.' CreateCustomDetectionRuleAssociation models AccessDeniedException(403), BadRequestException(400), ConflictException(409) and ResourceNotFoundException(404).

Cause 1 of 10: CreateCustomDetectionRuleAssociation mints an ARN without the '/custom/' segment.
AWS: the association ARN is arn:aws:guardduty:<region>:<account>:detection-rule/custom/<ruleId>/association/<associationId> — the fixed path is 'detection-rule/custom/', with region, account, ruleId and associationId coming from the request and the minted id.
Emulator: emits arn:aws:guardduty:<region>:<account>:detection-rule/<ruleId>/association/<associationId>, dropping 'custom/'.
Why: the ARN is what callers hand to IAM policies, resource tagging and cross-service references; an ARN missing a path segment does not match any policy written for the real service, and never resolves.
Fix: the ARN construction in the CreateCustomDetectionRuleAssociation handler.
Traces: guardduty-association-duplicate-conflict#task#3.

Cause 2 of 10: GetCustomDetectionRuleAssociation echoes the same malformed ARN.
AWS: the read returns the '/custom/' form described in cause 1.
Emulator: returns the stored ARN without 'custom/'.
Why: as cause 1 — the defect is observable on read as well as create, so it is persisted, not a serializer-only slip.
Fix: the shared association-ARN construction (or the stored value) used by the Get handler.
Traces: guardduty-association-duplicate-conflict#task#5.

Cause 3 of 10: a DRY_RUN association is created without expiresAt.
AWS: an association created in DRY_RUN mode carries ExpiresAt, a timestamp a fixed interval after createdAt (the recording shows an expiry roughly 14 days out).
Emulator: omits the member entirely.
Why: expiresAt is how a caller knows when a dry-run association stops evaluating; without it the client cannot schedule a renewal or warn an operator, and treats a temporary association as permanent.
Fix: the association state record / CreateCustomDetectionRuleAssociation handler, which must compute ExpiresAt for DRY_RUN mode.
Traces: guardduty-association-lifecycle#task#3.

Cause 4 of 10: GetCustomDetectionRuleAssociation omits expiresAt (and echoes the malformed ARN).
AWS: the read returns both the '/custom/' ARN and the DRY_RUN expiresAt.
Emulator: ARN without 'custom/', expiresAt absent.
Why: as causes 1 and 3; confirms both values are missing from state rather than from one serializer.
Fix: the association state record and the Get handler's ARN construction.
Traces: guardduty-association-lifecycle#task#4.

Cause 5 of 10: ListCustomDetectionRuleAssociations does not return the association under its real ARN.
AWS: the listing contains one entry per association, keyed by the '/custom/' ARN, carrying {associationId, arn, ruleId, mode, createdAt, updatedAt, expiresAt}.
Emulator: no entry with that ARN exists in the page (the stored entry has the malformed ARN and the missing expiresAt).
Why: a client that creates an association and then locates it in the listing by ARN never finds it, so reconciliation treats the association as lost and re-creates it.
Fix: the shared association-ARN construction consumed by the List handler.
Traces: guardduty-association-lifecycle#task#7.

Cause 6 of 10: ListCustomDetectionRules publishes the wrong rule catalog.
AWS: the published catalog includes the rule the recording selects first, 'admin-policy-attached-to-user'; the listing is the set of GuardDuty-published custom detection rules, each entry carrying at least its ruleId.
Emulator: the first (and in this fixture the only reachable) entry is 'cloudtrail-logging-disabled', with a full metadata block; 'admin-policy-attached-to-user' is not in the catalog at all.
Why: the catalog is the index a client walks to discover which rules it may associate; a missing rule id makes every downstream association for that rule impossible (see causes 7-8).
Fix: the ListCustomDetectionRules handler's seeded rule catalog (state).
Traces: guardduty-association-list-filters-across-two-rules#setup#2.

Cause 7 of 10: CreateCustomDetectionRuleAssociation refuses a valid rule id.
AWS: creating a DRY_RUN association for the rule id 'admin-policy-attached-to-user' succeeds with HTTP 200 and a full ruleAssociation body {associationId, arn, ruleId, accountId, mode, createdAt, updatedAt, expiresAt}.
Emulator: HTTP 404 with no ruleAssociation at all, because the rule is absent from its catalog (cause 6).
Why: an entire rule family is unusable — the client cannot associate, dry-run or evaluate it.
Fix: the seeded rule catalog consumed by the Create handler's rule-existence check.
Traces: guardduty-association-list-filters-across-two-rules#task#4.

Cause 8 of 10: ListCustomDetectionRuleAssociations returns an empty page where an association exists.
AWS: the first entry is the association created in cause 7, with mode DRY_RUN and its expiresAt.
Emulator: no entry at index 0 (the create never succeeded).
Why: as cause 7 — the listing confirms the association is absent from state, not just from the response.
Fix: the seeded rule catalog / Create handler, then the List handler.
Traces: guardduty-association-list-filters-across-two-rules#task#6.

Cause 9 of 10: a duplicate association is refused as not-found instead of as a conflict.
AWS: creating a second association for a rule that already has one is a conflict — HTTP 409, ConflictException (x-amzn-errortype, $.__type and $.type all 'ConflictException'), message 'Association already exists for rule <ruleId>' where 'Association already exists for rule ' is fixed and the rule id comes from the request.
Emulator: HTTP 404 with ResourceNotFoundException and 'The request is rejected because the input ruleId does not refer to a custom detection rule published by GuardDuty.'
Why: a client that catches ConflictException to treat create-as-idempotent instead sees ResourceNotFoundException and concludes the rule does not exist, so it stops retrying and reports a spurious configuration error.
Fix: the Create handler's precedence between the rule-existence check and the duplicate-association check (the rule-catalog gap of cause 6 is what shadows the conflict).
Traces: guardduty-custom-detection-rule-association-errors#task#4.

Cause 10 of 10: the duplicate-association conflict message is emulator-authored.
AWS: HTTP 409, ConflictException, message 'Association already exists for rule <ruleId>' — fixed prefix 'Association already exists for rule ' plus the ruleId from the request.
Emulator: returns 409 but with 'The request is rejected because the current account already has an association for the given ruleId and mode.'
Why: the status and class are right, so this is only reachable by message, and a caller that surfaces the service message to an operator shows text real GuardDuty never produces (it also implies the conflict is per-(ruleId, mode), which AWS's text does not).
Fix: the Create handler's ConflictException message.
Traces: guardduty-association-duplicate-conflict#task#4.

## Evidence
Cause 1: $.ruleAssociation.arn expected '...:detection-rule/custom/<ruleId>/association/<associationId>', got '...:detection-rule/<ruleId>/association/<associationId>'.
Cause 2: same arn divergence on GetCustomDetectionRuleAssociation.
Cause 3: $.ruleAssociation.expiresAt expected a timestamp, got None.
Cause 4: $.ruleAssociation.arn missing '/custom/' and $.ruleAssociation.expiresAt expected a timestamp, got None.
Cause 5: $.ruleAssociations[arn="...detection-rule/custom/<ruleId>/association/<id>"] expected the full entry {associationId, arn, ruleId, mode, createdAt, updatedAt, expiresAt}, got None.
Cause 6: $.rules[0] expected {'ruleId': 'admin-policy-attached-to-user'}, got a 'cloudtrail-logging-disabled' entry.
Cause 7: status expected 200, got 404; $.ruleAssociation expected the full AssociationDetail, got None.
Cause 8: $.ruleAssociations[0] expected the DRY_RUN association entry, got None.
Cause 9: status expected 409, got 404; $.__type/$.type/x-amzn-errortype expected 'ConflictException', got 'ResourceNotFoundException'; $.message expected 'Association already exists for rule <ruleId>', got 'The request is rejected because the input ruleId does not refer to a custom detection rule published by GuardDuty.'
Cause 10: status 409 both sides; $.message expected 'Association already exists for rule <ruleId>', got 'The request is rejected because the current account already has an association for the given ruleId and mode.'

## Steps to Reproduce
1. Replay guardduty-association-duplicate-conflict: step task#3 (create) and task#5 (get) show the ARN without '/custom/'; step task#4 shows the wrong 409 message.
2. Replay guardduty-association-lifecycle: step task#3 (create DRY_RUN) and task#4 (get) show expiresAt absent; step task#7 shows the listing has no entry under the real ARN.
3. Replay guardduty-association-list-filters-across-two-rules: step setup#2 shows the wrong rule catalog, step task#4 a 404 on a valid ruleId, step task#6 the resulting empty listing.
4. Replay guardduty-custom-detection-rule-association-errors step task#4 and observe 404/ResourceNotFoundException where a 409/ConflictException is required.
