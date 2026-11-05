Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-custom-detection-rule-association-errors
Issue-Step: setup#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-custom-detection-rule-association-errors
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|ListCustomDetectionRules|2xx->2xx|$.rules[*]
Issue-Commit: 9fcc5571ff6cbb375c3cc1ebb135d0cedced4f12

# GuardDuty-published detection-rule catalog entries carry synthesized metadata: account-scoped arn, invented name/description/technique, boot-clock createdAt and no updatedAt

> _Note: this issue was identified against the code base at commit `9fcc5571ff6cbb375c3cc1ebb135d0cedced4f12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCustomDetectionRules

## Description
ListCustomDetectionRules serves the GuardDuty-published detection-rule catalog, but the emulator's seeded catalog entry is synthesized rather than reproducing the published rule, so every member of the entry except ruleId/severity/dataSource/tactic/service/language/schema diverges. 1 cause.

Cause 1 of 1: the published detection-rule catalog entry returned by ListCustomDetectionRules is emulator-authored.
AWS: a rule published by GuardDuty is an AWS-managed catalog object, not an account resource. Smithy (services/service-com-amazonaws-guardduty-guarddutyapiservice, shape RuleSummary): `arn` targets DetectionRuleArn with pattern ^arn:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]*:([0-9]*|aws):.+$ — the region segment may be empty and the account segment may be the literal `aws`, which is exactly the AWS-managed form the recording shows: arn:aws:guardduty::aws:detection-rule/custom/<ruleId> (empty region, account `aws`). `name`, `description`, `tactic` and `technique` are required members whose values are properties of the published rule (recorded behavior only for the specific catalog text); `createdAt` is required and `updatedAt` is optional but present for a rule that has been revised since publication, and both are catalog publication timestamps — properties of the published rule, not of the emulator process. For the recorded entry the published values are name 'AdminPolicyAttachedToRole', description 'Detects when admin policy is attached to an IAM role', technique 'T1098.001 - Account Manipulation: Additional Cloud Credentials', with both createdAt and updatedAt present.
Emulator: returns the entry region/account-scoped as if it were an account resource — arn:aws:guardduty:<region>:<accountId>:detection-rule/custom/<ruleId> — with a prose display name in place of the published name ('Administrator policy attached to an IAM role'), a two-sentence description it authored itself, a different MITRE technique string ('T1098.003 Account Manipulation: Additional Cloud Roles'), `createdAt` derived from the emulator's own boot clock rather than the rule's publication time, and `updatedAt` omitted entirely.
Why: a client that keys the catalog by ARN never matches the AWS-managed ARN form, so a rule looked up or policy-scoped by its published ARN is not found; a client that pins or displays a rule's published name, description or MITRE technique reads values that exist nowhere in AWS; and a client that reads `updatedAt` to detect a catalog revision finds the member absent and a `createdAt` that moves on every emulator restart, so freshness and change-detection logic is wrong.
Fix: the seeded GuardDuty-published detection-rule catalog that backs ListCustomDetectionRules (and the ARN rendering for catalog objects, which must leave the region empty and use the `aws` account segment rather than the request's region/account), plus the catalog's createdAt/updatedAt, which must be the rule's publication timestamps carried by the seed data rather than values taken from the process clock.
Traces: guardduty-custom-detection-rule-association-errors#setup#2.

No open issue covers this: 20261029T190006Z-guardduty-custom-detection-rule-associations-carry-a-malformed-arn.md concerned association ARNs and is already resolved, and the replay reports no owner for this step.

## Evidence
Fixture guardduty-custom-detection-rule-association-errors, step setup#2, ListCustomDetectionRules, status 200 both sides; $.rules[0] value_mismatch.
Expected (recorded AWS): {'ruleId': 'admin-policy-attached-to-role', 'arn': 'arn:aws:guardduty::aws:detection-rule/custom/admin-policy-attached-to-role', 'name': 'AdminPolicyAttachedToRole', 'description': 'Detects when admin policy is attached to an IAM role', 'severity': 'HIGH', 'dataSource': 'CloudTrailManagementEvent', 'tactic': 'Privilege Escalation', 'technique': 'T1098.001 - Account Manipulation: Additional Cloud Credentials', 'service': 'iam', 'language': 'SQL', 'schema': 'CloudTrail', 'createdAt': <publication timestamp>, 'updatedAt': <revision timestamp>}.
Actual (emulator): same ruleId/severity/dataSource/tactic/service/language/schema, but arn 'arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role' (region and account interpolated instead of the empty-region/`aws`-account managed form), name 'Administrator policy attached to an IAM role', description 'Detects a policy granting administrator permissions being attached to an IAM role, which lets every principal that can assume the role control the account.', technique 'T1098.003 Account Manipulation: Additional Cloud Roles', createdAt taken from the emulator's boot clock, and NO updatedAt member at all.
(The concrete timestamp values in the recording are per-run/publication values and are not themselves the requirement; the requirement is that updatedAt is present and that neither timestamp is derived from the emulator's process clock.)

## Steps to Reproduce
1. Replay fixture guardduty-custom-detection-rule-association-errors step setup#2 (ListCustomDetectionRules with no filters) against the emulator.
2. Inspect $.rules[0]: the arn is region/account-scoped instead of 'arn:aws:guardduty::aws:detection-rule/custom/admin-policy-attached-to-role'; name, description and technique are emulator-authored strings; updatedAt is missing.
3. Restart the emulator and replay the same step: $.rules[0].createdAt changes, showing it is derived from the process clock rather than being the published rule's catalog timestamp.
