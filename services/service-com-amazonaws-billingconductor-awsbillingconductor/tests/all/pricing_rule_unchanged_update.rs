// SPDX-License-Identifier: CC0-1.0

//! `UpdatePricingRule` for a call that leaves the rule exactly as it stood, end to end
//! over the service's real `restJson1` wire through [`MultiServiceRouter::dispatch`] —
//! the same path the production router serves.
//!
//! RULE: `LastModifiedTime` is "the most recent time when the pricing rule was modified"
//! (`smithy_query(query="shape",shape="PricingRuleListElement")`), and the generation of
//! the rule a period-scoped read reports is the one the write that left it was made in —
//! so a write that leaves every member of the rule at the value state already holds
//! modified nothing: the instant stays where it was and no generation is frozen or
//! opened. A write that asks for any value the rule does not hold moves both, and the new
//! value reads back.
//!
//! `UpdatePricingRuleInput` makes every member but `Arn` optional
//! (`smithy_query(query="shape",shape="UpdatePricingRuleInput")`), so "changes nothing"
//! spans a request naming no member at all as well as one restating the values the rule
//! carries, member by member and all at once. The rows below differ along which member
//! the request names, which class that member falls in (text, enum, number, nested
//! structure, list), how many of them it names, and whether the value asked for is the
//! one the rule holds — including a number that differs only below the two decimal places
//! the member is stored rounded to, which is the same value.
//!
//! `tests/all/update_billing_group.rs` and
//! `tests/all/membership_writes_keep_superseded_generations.rs` own the same rule for the
//! sibling resources; `tests/all/pricing_rule_billing_periods.rs` owns the generations a
//! write that DID change the rule leaves behind, and owns the per-period read these rows
//! verify through.
//!
//! The wire cannot write in a PAST billing period — a write happens now — so the
//! generation rows write through the real wire and then backdate the stored instants,
//! which is the one thing the test cannot ask the service for.

use serde_json::{json, Value};

use crate::pricing_rule_association_periods::{period_start_millis, shift_period, Emulator};
use crate::pricing_rule_billing_periods::{assert_members, create_rule, current_period, element_in};

/// `UpdatePricingRule`, at the method its modelled HTTP binding names
/// (`smithy_query(query="operation",operation="UpdatePricingRule")`: a `PUT`).
const UPDATE_PATH: &str = "/update-pricing-rule";

/// Wall-clock millis since the epoch, read from the same clock the service stamps
/// `LastModifiedTime` from, so a row can bracket the instant a call must report.
fn now_millis() -> i64 {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::now_millis()
}

/// The `UpdatePricingRule` members a row names beside `Arn`, built from the rule's
/// current name so a row can restate the value the rule already holds.
type Patch = fn(&str) -> Value;

/// One row of the table: the rule the row starts from, the update it makes, whether that
/// update asks for anything the rule does not already hold, and the rule it must leave.
struct Row {
    label: &'static str,
    /// `CreatePricingRule` members beside `Name`.
    create: Value,
    patch: Patch,
    /// Does the request ask for any value the rule does not already hold?
    changes: bool,
    /// The members a read must report once the call is served.
    after: Value,
    /// The name the rule carries afterwards, when the row renames it.
    renamed_to: Option<&'static str>,
}

/// The `MARKUP` rule most rows start from: every member an update can change carries a
/// known value, so a row can restate it or move it.
fn markup_rule() -> Value {
    json!({"Scope": "GLOBAL", "Type": "MARKUP", "ModifierPercentage": 10.0,
           "Description": "as created"})
}

/// The members a read reports for [`markup_rule`] as created.
fn markup_members() -> Value {
    json!({"Type": "MARKUP", "ModifierPercentage": 10.0, "Description": "as created",
           "Tiering": null})
}

/// A `TIERING` rule that prices by tiers rather than by a percentage: the free tier on
/// its own, so a row can restate or flip one boolean.
fn free_tier_rule() -> Value {
    json!({"Scope": "GLOBAL", "Type": "TIERING", "Tiering": {"FreeTier": {"Activated": true}}})
}

/// A `TIERING` rule carrying both halves of the modelled `Tiering`, so a row can restate
/// or move a LIST member (`CustomTiersList`, 1..10 entries).
fn custom_tier_rule() -> Value {
    json!({"Scope": "GLOBAL", "Type": "TIERING",
           "Tiering": {"FreeTier": {"Activated": false},
                       "CustomTiers": [{"BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0,
                                        "RateValue": 5.0}]}})
}

/// The `Tiering` a read reports for a rule holding `custom_tiers` beside a free tier that
/// is off.
fn custom_tier_members(rate: f64) -> Value {
    json!({"Type": "TIERING", "ModifierPercentage": null,
           "Tiering": {"FreeTier": {"Activated": false},
                       "CustomTiers": [{"BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0,
                                        "RateValue": rate}]}})
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "no member named at all",
            create: markup_rule(),
            patch: |_| json!({}),
            changes: false,
            after: markup_members(),
            renamed_to: None,
        },
        Row {
            label: "the Name the rule already holds",
            create: markup_rule(),
            patch: |name| json!({"Name": name}),
            changes: false,
            after: markup_members(),
            renamed_to: None,
        },
        Row {
            label: "the Description it already holds",
            create: markup_rule(),
            patch: |_| json!({"Description": "as created"}),
            changes: false,
            after: markup_members(),
            renamed_to: None,
        },
        Row {
            label: "the Type it already prices by",
            create: markup_rule(),
            patch: |_| json!({"Type": "MARKUP"}),
            changes: false,
            after: markup_members(),
            renamed_to: None,
        },
        Row {
            label: "the ModifierPercentage it already holds",
            create: markup_rule(),
            patch: |_| json!({"ModifierPercentage": 10.0}),
            changes: false,
            after: markup_members(),
            renamed_to: None,
        },
        Row {
            // "Your entry will be rounded to the nearest 2 decimal places."
            // (`smithy_query(query="doc",shape="UpdatePricingRuleInput",
            // member="ModifierPercentage")`), so a request differing below that precision
            // asks for the value the rule already holds.
            label: "a ModifierPercentage that rounds to the stored one",
            create: markup_rule(),
            patch: |_| json!({"ModifierPercentage": 10.001}),
            changes: false,
            after: markup_members(),
            renamed_to: None,
        },
        Row {
            label: "every member at the value it already holds, all at once",
            create: markup_rule(),
            patch: |name| {
                json!({"Name": name, "Description": "as created", "Type": "MARKUP",
                       "ModifierPercentage": 10.0})
            },
            changes: false,
            after: markup_members(),
            renamed_to: None,
        },
        Row {
            label: "the free tier the rule already activates",
            create: free_tier_rule(),
            patch: |_| json!({"Tiering": {"FreeTier": {"Activated": true}}}),
            changes: false,
            after: json!({"Type": "TIERING", "ModifierPercentage": null,
                          "Tiering": {"FreeTier": {"Activated": true}}}),
            renamed_to: None,
        },
        Row {
            label: "the custom tiers the rule already holds",
            create: custom_tier_rule(),
            patch: |_| {
                json!({"Tiering": {"FreeTier": {"Activated": false},
                                   "CustomTiers": [{"BeginRangeInclusive": 0.0,
                                                    "EndRangeExclusive": 100.0,
                                                    "RateValue": 5.0}]}})
            },
            changes: false,
            after: custom_tier_members(5.0),
            renamed_to: None,
        },
        Row {
            label: "a new Description",
            create: markup_rule(),
            patch: |_| json!({"Description": "moved on"}),
            changes: true,
            after: json!({"Type": "MARKUP", "ModifierPercentage": 10.0,
                          "Description": "moved on"}),
            renamed_to: None,
        },
        Row {
            label: "a Description emptied, which is a value and not an omission",
            create: markup_rule(),
            patch: |_| json!({"Description": ""}),
            changes: true,
            after: json!({"Type": "MARKUP", "ModifierPercentage": 10.0, "Description": ""}),
            renamed_to: None,
        },
        Row {
            label: "a new Name",
            create: markup_rule(),
            patch: |_| json!({"Name": "RuleRenamed"}),
            changes: true,
            after: markup_members(),
            renamed_to: Some("RuleRenamed"),
        },
        Row {
            label: "a new ModifierPercentage",
            create: markup_rule(),
            patch: |_| json!({"ModifierPercentage": 12.5}),
            changes: true,
            after: json!({"Type": "MARKUP", "ModifierPercentage": 12.5,
                          "Description": "as created"}),
            renamed_to: None,
        },
        Row {
            label: "a ModifierPercentage differing in the second decimal place",
            create: markup_rule(),
            patch: |_| json!({"ModifierPercentage": 10.01}),
            changes: true,
            after: json!({"Type": "MARKUP", "ModifierPercentage": 10.01,
                          "Description": "as created"}),
            renamed_to: None,
        },
        Row {
            label: "a new Type, which keeps the percentage the rule prices by",
            create: markup_rule(),
            patch: |_| json!({"Type": "DISCOUNT"}),
            changes: true,
            after: json!({"Type": "DISCOUNT", "ModifierPercentage": 10.0,
                          "Description": "as created"}),
            renamed_to: None,
        },
        Row {
            label: "the free tier switched off",
            create: free_tier_rule(),
            patch: |_| json!({"Tiering": {"FreeTier": {"Activated": false}}}),
            changes: true,
            after: json!({"Type": "TIERING", "ModifierPercentage": null,
                          "Tiering": {"FreeTier": {"Activated": false}}}),
            renamed_to: None,
        },
        Row {
            label: "a new rate on the custom tier",
            create: custom_tier_rule(),
            patch: |_| {
                json!({"Tiering": {"CustomTiers": [{"BeginRangeInclusive": 0.0,
                                                    "EndRangeExclusive": 100.0,
                                                    "RateValue": 7.5}]}})
            },
            changes: true,
            after: custom_tier_members(7.5),
            renamed_to: None,
        },
    ]
}

/// Send the row's `UpdatePricingRule` against `rule` and answer the status and body.
async fn update(em: &Emulator, rule: &str, patch: &Value) -> (u16, Value) {
    let mut body = patch.clone();
    body["Arn"] = json!(rule);
    em.put(UPDATE_PATH, &body).await
}

#[tokio::test]
async fn last_modified_time_moves_only_when_the_update_changed_the_rule() {
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.label;
        // One store per row, so a row that renames its rule cannot collide with the names
        // of the rows beside it.
        let em = Emulator::new();
        let name = format!("ClockRule{index:03}");
        let mut create = row.create.clone();
        create["Name"] = json!(name);
        let rule = create_rule(&em, case, &create).await;

        let created = element_in(&em, case, &rule, None)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the created rule is reported"));
        let creation_time = created["CreationTime"].as_i64().unwrap_or_default();
        let modified_before = created["LastModifiedTime"].as_i64().unwrap_or_default();
        // The clock the service stamps from has millisecond resolution, so a move is only
        // observable once the call lands at a later instant than the create.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let before_call = now_millis();
        let (status, response) = update(&em, &rule, &(row.patch)(&name)).await;
        let after_call = now_millis();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {response:?}"
        );
        let reported = response["LastModifiedTime"].as_i64().unwrap_or_default();

        let listed = element_in(&em, case, &rule, None)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the rule exists, so it is reported"));
        assert_eq!(
            listed["LastModifiedTime"].as_i64(),
            Some(reported),
            "case {case:?}: the answer and the stored rule must agree on LastModifiedTime, \
             got {listed:?}"
        );
        if row.changes {
            assert!(
                (before_call..=after_call).contains(&reported),
                "case {case:?}: expected a call that changed the rule to stamp \
                 LastModifiedTime inside its own window [{before_call}, {after_call}], got \
                 {reported}"
            );
        } else {
            assert_eq!(
                reported, modified_before,
                "case {case:?}: expected a call that changed nothing to leave \
                 LastModifiedTime at {modified_before}, got {reported}"
            );
        }

        // Whatever the call did to the instant, the rule reads back as the call left it:
        // the members it asked for, the ones it named nothing about, and the creation
        // instant no update is ever allowed to touch.
        assert_members(case, &listed, &row.after);
        assert_members(case, &response, &row.after);
        assert_eq!(
            listed["Name"].as_str(),
            Some(row.renamed_to.unwrap_or(&name)),
            "case {case:?}: expected the name the call left, got {listed:?}"
        );
        assert_eq!(
            listed["CreationTime"].as_i64(),
            Some(creation_time),
            "case {case:?}: the creation instant is never an update's to change, got {listed:?}"
        );
    }
}

/// One row of the generation table: how far back the rule's current generation started,
/// the update made NOW, and whether that update changes anything.
struct GenerationRow {
    label: &'static str,
    /// Months back from the current billing period that the rule was created in.
    months_back: i64,
    patch: Patch,
    changes: bool,
}

#[tokio::test]
async fn an_update_that_changed_nothing_opens_no_generation() {
    // Rows differ along how far back the rule's generation started (the month before, a
    // year, two years — crossing a year boundary) and along whether the call made now
    // asks for anything the rule does not hold.
    let rows = [
        GenerationRow {
            label: "no member named, a generation old one month",
            months_back: -1,
            patch: |_| json!({}),
            changes: false,
        },
        GenerationRow {
            label: "the values it already holds, a generation old one year",
            months_back: -12,
            patch: |name| json!({"Name": name, "ModifierPercentage": 10.0}),
            changes: false,
        },
        GenerationRow {
            label: "the values it already holds, a generation old two years",
            months_back: -24,
            patch: |_| json!({"Description": "as created", "Type": "MARKUP"}),
            changes: false,
        },
        GenerationRow {
            label: "a new rate, a generation old one month",
            months_back: -1,
            patch: |_| json!({"ModifierPercentage": 20.5}),
            changes: true,
        },
        GenerationRow {
            label: "a new rate, a generation old two years",
            months_back: -24,
            patch: |_| json!({"ModifierPercentage": 20.5}),
            changes: true,
        },
    ];

    let now = current_period();
    for (index, row) in rows.iter().enumerate() {
        let case = row.label;
        let em = Emulator::new();
        let created_in = shift_period(&now, row.months_back);
        let name = format!("GenerationRule{index:03}");
        let mut create = markup_rule();
        create["Name"] = json!(name);
        let rule = create_rule(&em, case, &create).await;
        // The rule as if it had been created — and so last written — in a billing period
        // that is already closed, which is the world in which a write made now either
        // opens a generation or does not.
        em.create_rule_in(&rule, &created_in);
        let generation_start = period_start_millis(&created_in);

        let (status, response) = update(&em, &rule, &(row.patch)(&name)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {response:?}"
        );

        // The period the rule was created in still reports it exactly as it was created:
        // the generation in force then is frozen whatever the write did.
        let earlier = element_in(&em, case, &rule, Some(&created_in))
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the rule existed then, so it is reported"));
        assert_members(case, &earlier, &markup_members());
        assert_eq!(
            earlier["LastModifiedTime"].as_i64(),
            Some(generation_start),
            "case {case:?}: expected the closed period to report the generation's own \
             instant {generation_start}, got {earlier:?}"
        );

        // The current period is where the two cases part: a write that changed the rule
        // opens a generation there, with its own instant and its new value, while a write
        // that changed nothing leaves the generation that was already in force — so the
        // current period still reports the instant of the write that really did leave the
        // rule as it stands.
        let current = element_in(&em, case, &rule, Some(&now))
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the rule exists now, so it is reported"));
        let reported = current["LastModifiedTime"].as_i64().unwrap_or_default();
        if row.changes {
            assert!(
                reported > generation_start,
                "case {case:?}: expected the write that changed the rule to open a \
                 generation past {generation_start}, got {reported}"
            );
            assert_members(
                case,
                &current,
                &json!({"ModifierPercentage": 20.5, "Type": "MARKUP"}),
            );
        } else {
            assert_eq!(
                reported, generation_start,
                "case {case:?}: expected a write that changed nothing to leave the \
                 generation that started at {generation_start} in force, got {reported}"
            );
            assert_members(case, &current, &markup_members());
        }
        // A read that names no period asks about the current one, so the two agree.
        let unscoped = element_in(&em, case, &rule, None)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the rule exists now, so it is reported"));
        assert_eq!(
            unscoped["LastModifiedTime"].as_i64(),
            Some(reported),
            "case {case:?}: a read naming no period asks about {now:?}, got {unscoped:?}"
        );
    }
}
