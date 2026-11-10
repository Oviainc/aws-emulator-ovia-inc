Issue-Source: trace-sweep-divergence
Issue-Trace: codestar-notifications-list-and-filter-rules
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codestar-notifications-list-and-filter-rules
Issue-Shape-Id: com.amazonaws.codestarnotifications#CodeStarNotifications_20191015
Issue-Key: codestar-notifications|ListNotificationRules|2xx->2xx|$.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/4f2b7532-abc8-4fdd-808c-08e9e65f4604"].Id
Issue-Key: codestar-notifications|ListNotificationRules|2xx->2xx|$.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/ad4ff815-c742-4739-899d-6a6d2faff5aa"].Id
Issue-Commit: 122d2e23ed306dec374f1768a21e001304c75e9d

# codestar-notifications ListNotificationRules reports unmatched rule Id on every page

> _Note: this issue was identified against the code base at commit `122d2e23ed306dec374f1768a21e001304c75e9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListNotificationRules

## Description
Three replay clusters in codestar-notifications, all on ListNotificationRules, share ONE root cause: the identifier value this service mints for a notification rule and then echoes back as NotificationRuleSummary.Id. Every cluster answers the modeled HTTP 200 with the modeled members (NotificationRules, each entry carrying Id and Arn); what diverges is the identity value itself, on the unfiltered page and on both filtered pages. A fixer must close all THREE causes below before submitting.

Shared root cause. crate::state::new_rule_id mints a NotificationRuleId by taking a sha-256 digest of "notificationrule/<sequence>/<wall-clock nanos>" and laying the hex out as a hyphenated 36-character version-4-shaped UUID ("{8hex}-{4hex}-4{3hex}-8{3hex}-{12hex}"). crate::state::notification_rule_arn embeds that same value as the ARN's notificationrule/<id> segment, and ListNotificationRules echoes the stored id into NotificationRuleSummary.Id. So one minting decision decides both the id a listing reports and the ARN it is attached to; the listing handler itself is faithful (it returns the stored id byte-for-byte identical to the ARN's resource segment, which is why the replay's rendered expected and actual read alike while the comparison against the recorded rule identity still fails). The defect is the FORM/derivation of the minted id, not the listing or the filter path.

Cause 1 of 3: the unfiltered ListNotificationRules page reports an Id that does not match the recorded rule identity, for BOTH rules on the page. Fixture codestar-notifications-list-and-filter-rules, step task#6.
  AWS: Smithy: ListNotificationRules (POST /listNotificationRules, HTTP 200, @paginated inputToken=NextToken outputToken=NextToken items=NotificationRules pageSize=MaxResults) -> ListNotificationRulesResult.NotificationRules targets NotificationRuleBatch of NotificationRuleSummary{Id -> NotificationRuleId @length min 1 max 40, "The unique ID of the notification rule"; Arn -> NotificationRuleArn @pattern ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$}. The model constrains only the id's length, never its internal form, so for the form this is recorded behavior only: the recording shows AWS reporting, for each listed rule, an Id that is exactly the id segment of that rule's ARN, in the id form AWS itself mints. No error is modeled for this path at 200 and none is involved.
  Emulator: answers 200 with the right members and the right two rules, but the replay reports value_mismatch at $.NotificationRules[Arn="..."].Id for BOTH entries of the page, because the id carried by each rule was minted by state::new_rule_id (sha-256 of sequence+wall clock, laid out as a v4-shaped UUID) rather than in the form the recording holds. The reported ids change between replays (per-run minting) while the mismatch verdict is identical on every replay, so the fault is the form/derivation, not one recorded value.
  Why: Id is the member a client uses to correlate a listing entry with a rule it already knows, and the pair (Id, Arn) is the rule's wire identity. A listing whose ids cannot be matched against the ids AWS issues breaks that correlation for every rule in the account, on every page, for any client that persisted, logged or asserted an id recorded from AWS.
  Fix: the rule-id minting in crate::state (new_rule_id, and through it notification_rule_arn) — NOT the ListNotificationRules handler, which already reports the stored id unchanged, and not the create handler, which just returns what state minted.
  Traces: codestar-notifications-list-and-filter-rules#task#6.

Cause 2 of 3: the same Id divergence on a FILTERED page returning one rule. Fixture codestar-notifications-list-and-filter-rules, step task#7.
  AWS: as cause 1 — Smithy: NotificationRuleSummary.Id -> NotificationRuleId @length 1..40, HTTP 200. Filtering (Filters: OR within one filter name, AND across names) narrows WHICH rules are listed and changes nothing about the identity reported for a listed rule; the id form is recorded behavior only.
  Emulator: 200, the filter selects the correct single rule (its ARN is matched by the replay selector), and that one entry's $.NotificationRules[Arn="..."].Id still diverges from the recorded rule identity.
  Why: a filtered listing is the normal way a client finds one rule by resource, event type, target address or creator; the single entry it gets back carries an id it cannot match against the one AWS would have issued.
  Fix: same component as cause 1 (crate::state::new_rule_id). Nothing in the filter path (rule_satisfies / grouped_filters) is implicated — the right rule is selected.
  Traces: codestar-notifications-list-and-filter-rules#task#7.

Cause 3 of 3: the same Id divergence on the other filtered page. Fixture codestar-notifications-list-and-filter-rules, step task#8.
  AWS: as cause 1/2.
  Emulator: 200, correct rule selected by the filter, and $.NotificationRules[Arn="..."].Id diverges for that entry.
  Why: as cause 2 — every listing this service serves, filtered or unfiltered, reports a rule id a client cannot match against AWS's.
  Fix: as cause 1/2.
  Traces: codestar-notifications-list-and-filter-rules#task#8.

No HTTP status, error code or error message is involved in any of the three causes: every step answered the modeled 200 with the modeled members, so the divergence is purely in the identity value reported for a rule.

Relation to open issues: 20261110T174500Z-codestar-notifications-rule-identifiers-diverge-createnotificationrule-arn-shape.md describes the same rule-id minting root cause, but its replay pointer is keyed on the CreateNotificationRule $.Arn step plus ListNotificationRules keys naming rule ARNs from an earlier run (notificationrule/2dcfbdba-... and notificationrule/7b85c7a4-...), so the three ListNotificationRules clusters of THIS replay are not owned by it (the replay reports owned_by empty for all three) and their replay keys would not be re-run by a fixer closing it. The other open codestar-notifications issues are 20261102T083845Z (CreateNotificationRule accepting an EventTypeIds entry outside the event-type catalog) and 20261029T183307Z (ListTagsForResource stub); neither touches rule-id minting.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout: 95 steps, 87 passed, 8 mismatched in 4 clusters, score 91.6%, emulator_up=true, bootstrap_signal=false. The three clusters below are service=codestar-notifications, operation=ListNotificationRules, verdict=mismatch, expected_status=200 / actual_status=200, stub_path empty, owned_by EMPTY (the fourth cluster, CreateNotificationRule $.Arn at codestar-notifications-rule-lifecycle#task#4, is owned by an existing issue and is not filed here).

Cause 1 — fixture codestar-notifications-list-and-filter-rules, step task#6, ListNotificationRules, 200 -> 200:
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/4f2b7532-abc8-4fdd-808c-08e9e65f4604"].Id: value_mismatch; and $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/ad4ff815-c742-4739-899d-6a6d2faff5aa"].Id: value_mismatch (two of two listed rules).
  Expected: each summary's Id equal to the recorded rule identity for that rule — the id segment of that rule's ARN in the id form AWS mints.
  Actual: the emulator's own per-run, sha-256-derived, v4-shaped UUID id; the comparison fails for every entry on the page. The literal ids above are per-run values from this replay and are NOT the requirement.

Cause 2 — same fixture, step task#7, ListNotificationRules, 200 -> 200:
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/ad4ff815-c742-4739-899d-6a6d2faff5aa"].Id: value_mismatch (single filtered entry).

Cause 3 — same fixture, step task#8, ListNotificationRules, 200 -> 200:
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/4f2b7532-abc8-4fdd-808c-08e9e65f4604"].Id: value_mismatch (single filtered entry).

Code under the causes (same checkout the emulator is built from):
  services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/src/state.rs:569 new_rule_id — sha256("notificationrule/<sequence>/<nanos>") laid out as "{8}-{4}-4{3}-8{3}-{12}".
  .../src/state.rs:368 notification_rule_arn — emulator_arn("codestar-notifications", region, "notificationrule/<id>").
  .../src/ops/create_notification_rule.rs:329-331 — take_sequence() -> new_rule_id() -> notification_rule_arn(), stored on the rule and returned as the create output's Arn.
  .../src/ops/list_notification_rules.rs:193-201 — NotificationRuleSummary { id: rule.id.clone(), arn: rule.arn.clone() }, i.e. the listing is faithful to the stored id.

Smithy model consulted (services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json): ListNotificationRules http POST /listNotificationRules code 200, @paginated items=NotificationRules; NotificationRuleSummary.Id -> NotificationRuleId @length min 1 max 40 (no pattern — the id's internal form is recorded behavior only); NotificationRuleSummary.Arn -> NotificationRuleArn @pattern ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$; errors InvalidNextTokenException (400) and ValidationException (400), neither involved.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (cargo run -p aws-emulator-router).
2. Replay trace set simparity-wire-aws-v1 against the booted emulator (replay_wire_corpus).
3. Cause 1: replay fixture codestar-notifications-list-and-filter-rules through step task#6 — an unfiltered ListNotificationRules (POST /listNotificationRules) after the fixture's rules have been created. The response is 200 with both rules listed, but $.NotificationRules[Arn="..."].Id diverges for BOTH entries.
4. Causes 2 and 3: continue the same fixture through steps task#7 and task#8 — filtered ListNotificationRules calls. Each answers 200, selects the correct single rule, and still diverges at $.NotificationRules[Arn="..."].Id.
5. Confirm the form, not one value, is at fault: replay the corpus twice. The reported ids differ between runs (per-run minting in state::new_rule_id) while all three clusters diverge identically every time.
6. After a fix, re-replay codestar-notifications-list-and-filter-rules#task#6, #task#7 and #task#8; all three must pass, and each summary's Id must remain byte-for-byte identical to the id segment of that rule's ARN.
