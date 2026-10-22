Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rbin.create-rule-basic-lifecycle rbin.delete-rule-nonexistent-not-found rbin.delete-rule-removes-from-list rbin.create-rule-retention-period-out-of-range-rejected rbin.get-rule-invalid-identifier-pattern-rejected rbin.lock-rule-unlock-delay-out-of-range-rejected rbin.create-rule-tag-quota-exceeded-rejected rbin.ebs-snapshot-retained-in-recycle-bin-on-delete
Issue-Shape-Id: com.amazonaws.rbin#AmazonRecycleBin
Issue-Key: rbin.create-rule-basic-lifecycle|run#7|AssertionError|assert created["Status"] == "pending", f"expected a freshly created rule to be pending, got {created['Status']!r}"
Issue-Key: rbin.delete-rule-nonexistent-not-found|run#5|AssertionError|assert message == f"Recycle Bin Rule '{fake_id}' does not exist.", message
Issue-Key: rbin.delete-rule-removes-from-list|run#13|AssertionError|assert gone_message == f"Recycle Bin Rule '{rule_id}' does not exist.", gone_message
Issue-Key: rbin.create-rule-retention-period-out-of-range-rejected|run#5|AssertionError|assert message == (
Issue-Key: rbin.get-rule-invalid-identifier-pattern-rejected|run#8|AssertionError|assert message == (
Issue-Key: rbin.lock-rule-unlock-delay-out-of-range-rejected|run#7|AssertionError|assert message == (
Issue-Key: rbin.create-rule-tag-quota-exceeded-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: rbin|CreateVolume|NotImplemented
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# rbin retention-rule fidelity: CreateRule skips the pending status, wrong ResourceNotFound/validation error messages, no 50-tag-per-rule quota, and the EBS-snapshot e2e path is blocked by the EC2 CreateVolume stub

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule (bundle also covers GetRule, DeleteRule, LockRule input validation, and EC2 CreateVolume as a dependency)

## Description
FIVE distinct root causes in the rbin (Amazon Recycle Bin) emulator, surfaced by the rbin.* behavioral checks. Each is its own work item; all FIVE must be closed before submitting. Not covered by the stub issues the replay report filed for me (those name LockRule, TagResource and ListTagsForResource in this crate — none of the causes below is one of them).

Cause 1 of 5: CreateRule never reports the `pending` status.
AWS: Smithy: CreateRuleOutput.Status -> RuleStatus enum values [pending, available], documented as "The state of the retention rule. Only retention rules that are in the available state retain resources." Recorded behavior: the CreateRule response for a freshly created rule carries Status="pending", and the rule transitions to "available" shortly afterwards (a subsequent GetRule polls to "available"). So `pending` is an observable state of a brand-new rule.
Emulator: CreateRule stores and returns status "available" unconditionally (services/.../src/ops/create_rule.rs sets `status: "available".to_string()` with a comment that there is "no `pending` interval here"), so the create response's Status is "available" and `pending` is never observable.
Why: a client that gates on the documented pending->available transition (wait until Status==available before relying on retention, or asserts the create response is pending) sees a state machine the real service does not have; code that treats "available" straight out of CreateRule as proof the rule is enforcing is validated against the emulator but not against AWS.
Fix: the CreateRule handler plus the rule's status in this crate's state (a freshly minted rule starts `pending` and becomes `available`, with GetRule/ListRules reading the transitioned value).
Traces: rbin.create-rule-basic-lifecycle.

Cause 2 of 5: the ResourceNotFoundException message for a missing/deleted rule does not follow AWS's wording rule.
AWS: Smithy: ResourceNotFoundException is a client error with httpError 404 and members Message/Reason. Recorded behavior: GetRule/DeleteRule on a well-formed but nonexistent Identifier (and on an already-deleted one) answer 404 ResourceNotFoundException whose Message is built from the requested identifier as "Recycle Bin Rule '<identifier>' does not exist." The rule producing the message is known (it is composed from the request's Identifier), so the composition — not one recorded string — is the requirement.
Emulator: the error code and 404 status are right, but the message is composed differently: "Retention rule with ID <identifier> not found" (e.g. "Retention rule with ID zzzzzzzzzzz not found", "Retention rule with ID 00000000001 not found").
Why: a client or test that matches AWS's not-exist wording (log scrapers, assertions, user-facing error surfacing) does not match the emulator, and the deleted-vs-never-existed indistinguishability check that AWS guarantees cannot be validated.
Fix: the not-found error construction in the GetRule and DeleteRule handlers (and any other rbin operation that raises ResourceNotFoundException for an unknown identifier) — compose the message from the requested identifier in AWS's form.
Traces: rbin.delete-rule-nonexistent-not-found, rbin.delete-rule-removes-from-list.

Cause 3 of 5: constraint-validation ValidationException messages embed the rejected value and the regex anchors.
AWS: Smithy: RetentionPeriodUnit and UnlockDelayUnit are enums whose only value is DAYS; RuleIdentifier is a string with pattern ^[0-9a-zA-Z]{11}$; ValidationException is a client error, httpError 400. Recorded behavior: a constraint violation answers 400 ValidationException with a message built from the request as "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [DAYS]" and, for the pattern case, "...Member must satisfy regular expression pattern: [0-9a-zA-Z]{11}" — i.e. the offending value is NOT echoed and the pattern is reported without the ^...$ anchors. The message is built from the request (it names the member and its constraint), so that construction is the requirement.
Emulator: same code and status, but the formatter echoes the value and the raw anchored pattern: "1 validation error detected: Value 'WEEKS' at 'retentionPeriod.retentionPeriodUnit' failed to satisfy constraint: Member must satisfy enum value set: [DAYS]", "...Value 'WEEKS' at 'lockConfiguration.unlockDelay.unlockDelayUnit'...", and "1 validation error detected: Value 'not-a-valid-id!' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{11}$".
Why: AWS deliberately does not echo the rejected member value in these messages (it can carry caller data) and reports the pattern unanchored; a client that parses or surfaces the validation message sees a different shape, and anything asserting on AWS's documented validation wording fails.
Fix: the centralized constraint-validation error formatter used on rbin operation inputs (the pre-handler validate step of the request walk) — emit the member path and constraint without the offending value, and strip the anchors when reporting a pattern.
Traces: rbin.create-rule-retention-period-out-of-range-rejected, rbin.get-rule-invalid-identifier-pattern-rejected, rbin.lock-rule-unlock-delay-out-of-range-rejected.

Cause 4 of 5: CreateRule does not enforce the 50-tags-per-rule quota.
AWS: Smithy: CreateRule models ServiceQuotaExceededException (client error, httpError 402) documented as "The request would cause a service quota for the number of tags per resource to be exceeded", with a Reason member. The modeled TagList bound is 0..200, so this is a service quota enforced by the handler, not a model constraint. Recorded behavior: CreateRule with 51 tags is rejected with ServiceQuotaExceededException (Message "SERVICE_QUOTA_EXCEEDED") and NO rule is persisted; exactly 50 tags succeeds.
Emulator: the 51-tag CreateRule SUCCEEDS — HTTP 201 with Identifier '00000000001' and all 51 tags echoed back, and the rule is persisted (it shows up in ListRules). The only quota the handler enforces is MAX_RULES (rules per Region) in create_rule.rs; there is no per-rule tag-count check.
Why: the emulator is overly permissive — a client can create rules AWS would refuse, so quota-handling code paths (catching ServiceQuotaExceededException, trimming tags) are never exercised and the client fails only in production.
Fix: the CreateRule handler's validation — count input Tags and return the modeled ServiceQuotaExceededException (402) with reason SERVICE_QUOTA_EXCEEDED above 50 tags, persisting nothing.
Traces: rbin.create-rule-tag-quota-exceeded-rejected.

Cause 5 of 5: the EBS-snapshot retention e2e path is unreachable because EC2 CreateVolume is a scaffolded stub.
AWS: recorded behavior only — the check provisions a volume with EC2 CreateVolume(AvailabilityZone, Size=1, VolumeType=gp3), snapshots it, then verifies a tag-level rbin rule retains the deleted snapshot in the Recycle Bin.
Emulator: ec2.create_volume answers error code NotImplemented ("operation CreateVolume is scaffolded but not implemented in this emulator"), so the rbin end-to-end retention behavior (snapshot retained instead of deleted, RecycleBinEnterTime present, restore via RestoreSnapshotFromRecycleBin) is never exercised at all.
Why: the only cross-service proof that rbin rules actually retain resources cannot run; a client's whole reason for creating a retention rule is untestable against the emulator.
Fix: the EC2 CreateVolume handler plus EBS volume state in the ec2 crate (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_volume.rs), then the rbin-side retention enforcement this check exercises. NOTE: the same CreateVolume stub is described as Cause 1 of the open issue 20261022T120642Z-sagemaker-behavioral-checks-blocked-by-neighbouring-service-gaps.md, but that issue carries only sagemaker.* replay keys and does not name this rbin check, so this cause records the rbin replay key for it.
Traces: rbin.ebs-snapshot-retained-in-recycle-bin-on-delete.

## Evidence
Cause 1: rbin.create-rule-basic-lifecycle — AssertionError: expected a freshly created rule to be pending, got 'available' (create_rule.rs persists status "available").
Cause 2: rbin.delete-rule-nonexistent-not-found — expected Message "Recycle Bin Rule 'zzzzzzzzzzz' does not exist.", got "Retention rule with ID zzzzzzzzzzz not found"; rbin.delete-rule-removes-from-list — expected "Recycle Bin Rule '00000000001' does not exist.", got "Retention rule with ID 00000000001 not found" (code ResourceNotFoundException and 404 are correct in both).
Cause 3: rbin.create-rule-retention-period-out-of-range-rejected — got "1 validation error detected: Value 'WEEKS' at 'retentionPeriod.retentionPeriodUnit' failed to satisfy constraint: Member must satisfy enum value set: [DAYS]", expected the same sentence WITHOUT "'WEEKS'"; rbin.lock-rule-unlock-delay-out-of-range-rejected — same with member path 'lockConfiguration.unlockDelay.unlockDelayUnit'; rbin.get-rule-invalid-identifier-pattern-rejected — got "...Value 'not-a-valid-id!' at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{11}$", expected "1 validation error detected: Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z]{11}".
Cause 4: rbin.create-rule-tag-quota-exceeded-rejected — AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'Identifier': '00000000001', ..., 'Tags': [{'Key': 'k0', ...} ... 51 entries]}; expected 402 ServiceQuotaExceededException with Message 'SERVICE_QUOTA_EXCEEDED' and no rule persisted.
Cause 5: rbin.ebs-snapshot-retained-in-recycle-bin-on-delete — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateVolume operation: operation CreateVolume is scaffolded but not implemented in this emulator.

## Steps to Reproduce
1. Cause 1: rbin.create_rule(ResourceType='EBS_SNAPSHOT', RetentionPeriod={'RetentionPeriodValue':5,'RetentionPeriodUnit':'DAYS'}) -> expect the response Status='pending' and a later GetRule to reach 'available'; observe Status='available' immediately.
2. Cause 2: rbin.delete_rule(Identifier='zzzzzzzzzzz') (well-formed, nonexistent) -> expect 404 ResourceNotFoundException with Message "Recycle Bin Rule 'zzzzzzzzzzz' does not exist."; observe "Retention rule with ID zzzzzzzzzzz not found". Same for rbin.get_rule on a deleted identifier.
3. Cause 3: rbin.create_rule(..., RetentionPeriod={'RetentionPeriodValue':30,'RetentionPeriodUnit':'WEEKS'}) and rbin.get_rule(Identifier='not-a-valid-id!') and rbin.lock_rule(Identifier=<id>, LockConfiguration={'UnlockDelay':{'UnlockDelayValue':10,'UnlockDelayUnit':'WEEKS'}}) -> expect 400 ValidationException whose message names the member path and constraint WITHOUT echoing the rejected value and WITHOUT regex anchors; observe the value echoed and the anchored pattern.
4. Cause 4: rbin.create_rule(ResourceType='EBS_VOLUME', RetentionPeriod={'RetentionPeriodValue':3,'RetentionPeriodUnit':'DAYS'}, Tags=[{'Key':f'k{i}','Value':f'v{i}'} for i in range(51)]) -> expect 402 ServiceQuotaExceededException (Message 'SERVICE_QUOTA_EXCEEDED') and nothing in ListRules; observe HTTP 201 and a persisted 51-tag rule. 50 tags must still succeed.
5. Cause 5: ec2.create_volume(AvailabilityZone=<az>, Size=1, VolumeType='gp3') -> expect 200 with a VolumeId; observe error code NotImplemented, which aborts the rbin snapshot-retention e2e flow before any rbin call.
