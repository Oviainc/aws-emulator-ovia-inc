Issue-Source: trace-sweep-divergence
Issue-Trace: codestar-notifications-rule-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codestar-notifications-rule-lifecycle codestar-notifications-list-and-filter-rules
Issue-Shape-Id: com.amazonaws.codestarnotifications#CodeStarNotifications_20191015
Issue-Key: codestar-notifications|CreateNotificationRule|2xx->2xx|$.Arn
Issue-Key: codestar-notifications|ListNotificationRules|2xx->2xx|$.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/2dcfbdba-7c89-4c63-80dc-9fcea8f86cc3"].Id
Issue-Key: codestar-notifications|ListNotificationRules|2xx->2xx|$.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/7b85c7a4-d193-47e3-8263-5f8226b949f1"].Id
Issue-Commit: b89f41e6cc4db360cb043b357635b76c01311ad8

# codestar-notifications rule identifiers diverge: CreateNotificationRule Arn shape and ListNotificationRules Id values

> _Note: this issue was identified against the code base at commit `b89f41e6cc4db360cb043b357635b76c01311ad8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNotificationRule

## Description
Four replay clusters, all in codestar-notifications, share ONE root cause: the identifier this service mints for a notification rule. Every cluster answers with the right HTTP status (200) and the right body members; what diverges is the identity value itself — the id segment the emulator puts in a rule ARN, and the `Id` it reports for a rule in a listing. A fixer must close all FOUR causes below before submitting.

Shared root cause. `crate::state::new_rule_id` (services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/src/state.rs:569) takes a sha-256 digest of `notificationrule/<sequence>/<wall-clock nanos>` and lays it out as a hyphenated, 36-character version-4-shaped UUID (`{8hex}-{4hex}-4{3hex}-8{3hex}-{12hex}`). `crate::state::notification_rule_arn` (state.rs:368) embeds that value as the ARN's `notificationrule/<id>` segment, and `ListNotificationRules` echoes the stored id into `NotificationRuleSummary.Id` (src/ops/list_notification_rules.rs:193-201). So one minting decision decides what the create path returns and what every listing reports.

Cause 1 of 4: CreateNotificationRule `$.Arn` fails the fixture's shape assertion for a generated rule ARN. Fixture codestar-notifications-rule-lifecycle, step task#4.
  AWS: Smithy: CreateNotificationRule -> CreateNotificationRuleResult.Arn targets NotificationRuleArn with @pattern ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$, HTTP 200 (POST /createNotificationRule). The model fixes the ARN's six fields but says nothing about the internal FORM of the id in the resource segment (NotificationRuleId is only @length min 1 max 40), so for the form this is recorded behavior only: the recording does not assert the generated ARN literally, it asserts its SHAPE, and the shape it recorded from AWS is the form AWS's own rule ids take.
  Emulator: returns 200 with an `Arn` whose resource segment is the hyphenated 36-character UUID described above (replay: got 'arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/a3a92264-6aac-4c54-8de3-2888ad765943'; a second replay produced '...notificationrule/fe918664-5bc3-40a5-84a2-de0dcf057036', i.e. the value is per-run but the verdict is identical every run). The replay verdict is `mismatch` at `$.Arn` with expected '<shaped-not-asserted>' — the asserted shape, not a literal value, is what the emulator's ARN fails.
  Why: the ARN is the rule's wire identity — the key DescribeNotificationRule, Subscribe, UpdateNotificationRule, DeleteNotificationRule and every tag call read back, and the value clients persist, log and match. A client that validates or round-trips an ARN whose id segment it recorded from AWS, or any test that asserts the shape of a rule ARN rather than its literal value, sees a value AWS would not have issued.
  Fix: the id minting in `crate::state::new_rule_id`, and through it `crate::state::notification_rule_arn` — not the CreateNotificationRule handler, which already just returns what state minted.
  Traces: codestar-notifications-rule-lifecycle#task#4.

Cause 2 of 4: ListNotificationRules reports an `Id` that does not match the recorded id for the same rule — both rules of the unfiltered page. Fixture codestar-notifications-list-and-filter-rules, step task#6.
  AWS: Smithy: ListNotificationRules -> ListNotificationRulesResult.NotificationRules targets NotificationRuleBatch of NotificationRuleSummary{Id: NotificationRuleId @length 1..40 "The unique ID of the notification rule", Arn: NotificationRuleArn}, HTTP 200, @paginated(items=NotificationRules). The recordings show AWS reporting, for each rule, an `Id` that is exactly the id segment of that rule's ARN, in the id form AWS mints; the model constrains only its length, so the form is recorded behavior only.
  Emulator: answers 200 with the expected members, but the replay reports `value_mismatch` at `$.NotificationRules[Arn="..."].Id` for BOTH rules of the page — the id the emulator carries is not the recorded rule id, because it was minted by `new_rule_id` rather than in the form the recording holds.
  Why: `Id` is the member a client uses to correlate a listing entry with a rule it already knows; a listing whose ids cannot be matched against the ids AWS issues breaks that correlation for every rule in the account, on every page.
  Fix: same component as cause 1 — the id minting in `crate::state::new_rule_id`; `ListNotificationRules` must keep reporting that stored id byte-for-byte identical to its ARN's resource segment (src/ops/list_notification_rules.rs:193-201).
  Traces: codestar-notifications-list-and-filter-rules#task#6.

Cause 3 of 4: the same `Id` divergence on a FILTERED page (one rule returned). Fixture codestar-notifications-list-and-filter-rules, step task#7.
  AWS: as cause 2 — Smithy: NotificationRuleSummary.Id (NotificationRuleId @length 1..40); filtering narrows which rules are listed and changes nothing about the identity reported for a listed rule.
  Emulator: 200, the filter selects the right rule (its ARN is matched), and the single entry's `$.NotificationRules[Arn="..."].Id` still diverges.
  Why: a filtered listing is the normal way a client finds one rule by resource, event type, target or creator; the one entry it gets back carries an id it cannot match against AWS's.
  Fix: as cause 1/2 (`crate::state::new_rule_id`); nothing in the filter path (`rule_satisfies`, src/ops/list_notification_rules.rs:86-98) is implicated — the right rule is selected.
  Traces: codestar-notifications-list-and-filter-rules#task#7.

Cause 4 of 4: the same `Id` divergence on the other filtered page. Fixture codestar-notifications-list-and-filter-rules, step task#8.
  AWS: as cause 2.
  Emulator: 200, correct rule selected, `$.NotificationRules[Arn="..."].Id` diverges.
  Why: as cause 3 — every listing this service serves, filtered or not, reports an unmatched rule id.
  Fix: as cause 1/2.
  Traces: codestar-notifications-list-and-filter-rules#task#8.

No status, error code or error message is involved in any of the four causes: every step answered the modeled 200 with the modeled members, so the divergence is purely in the identity values. No open issue covers this: the open codestar-notifications issues are 20261102T083845Z (CreateNotificationRule accepting an EventTypeIds entry outside the catalog) and 20261029T183307Z (ListTagsForResource stub), neither of which touches rule-id minting.

## Evidence
Replay of trace set simparity-wire-aws-v1 (95 steps, 87 passed, 8 mismatched in 4 clusters, score 91.6%, emulator_up=true, bootstrap_signal=false). All four clusters are service=codestar-notifications, verdict=mismatch, expected_status=200 / actual_status=200, stub_path empty, owned_by empty.

Cause 1 — fixture codestar-notifications-rule-lifecycle, step task#4, operation CreateNotificationRule:
  detail: $.Arn: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/a3a92264-6aac-4c54-8de3-2888ad765943')
  Expected: an `Arn` whose generated id segment satisfies the shape the recording asserts for a notification-rule ARN (the recording asserts the shape, not the literal value).
  Actual: an ARN whose id segment is a hyphenated 36-character version-4-shaped UUID produced by state::new_rule_id. The id differs per run ('fe918664-5bc3-40a5-84a2-de0dcf057036' on an earlier replay of the same step) while the mismatch verdict is identical on every replay, so the defect is the FORM, not one recorded value.

Cause 2 — fixture codestar-notifications-list-and-filter-rules, step task#6, operation ListNotificationRules:
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/7b85c7a4-d193-47e3-8263-5f8226b949f1"].Id: value_mismatch; and $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/2dcfbdba-7c89-4c63-80dc-9fcea8f86cc3"].Id: value_mismatch (two of two listed rules).
  Expected: each summary's `Id` equal to the recorded rule id for that rule (the id segment of that rule's ARN, in the form AWS mints).
  Actual: the emulator's own per-run UUID-shaped id; the comparison fails for every entry of the page.

Cause 3 — same fixture, step task#7: detail: $.NotificationRules[Arn="...notificationrule/2dcfbdba-7c89-4c63-80dc-9fcea8f86cc3"].Id: value_mismatch (single filtered entry).

Cause 4 — same fixture, step task#8: detail: $.NotificationRules[Arn="...notificationrule/7b85c7a4-d193-47e3-8263-5f8226b949f1"].Id: value_mismatch (single filtered entry).

Code under the causes (same checkout the emulator is built from):
  services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/src/state.rs:569 `new_rule_id` — sha256("notificationrule/<sequence>/<nanos>") laid out as "{8}-{4}-4{3}-8{3}-{12}".
  .../src/state.rs:368 `notification_rule_arn` — emulator_arn("codestar-notifications", region, "notificationrule/<id>").
  .../src/ops/create_notification_rule.rs:329-331 — take_sequence() -> new_rule_id() -> notification_rule_arn(), returned as CreateNotificationRuleOutput.arn.
  .../src/ops/list_notification_rules.rs:193-201 — NotificationRuleSummary { id: rule.id, arn: rule.arn }.

Smithy model consulted (services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json): CreateNotificationRuleResult.Arn -> NotificationRuleArn @pattern ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$; NotificationRuleSummary.Id -> NotificationRuleId @length min 1 max 40 (no pattern — the id's internal form is recorded behavior only); ListNotificationRules HTTP 200, @paginated items=NotificationRules.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (cargo run -p aws-emulator-router).
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus against the booted emulator).
3. Cause 1: replay fixture codestar-notifications-rule-lifecycle up to step task#4 — a CreateNotificationRule (POST /createNotificationRule) with the fixture's Name/Resource/DetailType/EventTypeIds/Targets. The response is 200, but `$.Arn` fails the step's shape assertion for the generated rule ARN: its resource segment is a hyphenated UUID minted by state::new_rule_id.
4. Causes 2-4: replay fixture codestar-notifications-list-and-filter-rules through steps task#6 (unfiltered ListNotificationRules), task#7 and task#8 (filtered listings). Each answers 200 with the right rules selected, and each diverges at $.NotificationRules[Arn="..."].Id for every entry returned.
5. Confirm the form, not one value, is at fault: replay the corpus twice. The reported ids change between runs (per-run minting) while all four clusters diverge identically every time.
6. After a fix, re-replay codestar-notifications-rule-lifecycle#task#4 and codestar-notifications-list-and-filter-rules#task#6, #task#7, #task#8; all four must pass, and each listing's `Id` must stay byte-for-byte identical to the id segment of that rule's ARN.
