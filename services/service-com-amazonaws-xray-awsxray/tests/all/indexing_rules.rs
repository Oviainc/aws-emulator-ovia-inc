// SPDX-License-Identifier: CC0-1.0

//! Indexing rules, end to end over the wire.
//!
//! Every case drives the same front door the production router does (the `Xray`
//! harness in [`super::xray_wire`]: a `MultiServiceRouter` with this service
//! mounted, addressed by the `xray` endpoint host, speaking restJson1) — so the
//! assertions are about HTTP responses, never about internal state.
//!
//! The rules under test, each checked over a table whose rows differ along what
//! the rule generalises over (the configured percentage, the rule name, the
//! shape of a pagination token):
//!
//! * `GetIndexingRules` reports the rules the service HOLDS — the account's
//!   `Default` rule before any update, and the stored configuration after one —
//!   and reading never changes what a later reader sees;
//! * `UpdateIndexingRule` persists the submitted `DesiredSamplingPercentage`
//!   under the named rule and answers the stored record, so a read afterwards
//!   reports exactly what was written (latest write wins);
//! * a rule name other than the account's default one is refused with
//!   `InvalidRequestException` (HTTP 400) and creates nothing — only the default
//!   rule is updatable and X-Ray models no operation that adds an indexing rule,
//!   so no other name can ever name one;
//! * a `DesiredSamplingPercentage` outside the published 0..=100 percentage
//!   range is refused with `InvalidRequestException` (HTTP 400) and leaves the
//!   stored rule as it was;
//! * a `NextToken` this service never issued — the empty string among them — is
//!   refused with `InvalidRequestException`, while an ABSENT token returns the
//!   first page: empty and absent are distinguished, not conflated;
//! * `IndexingRule$ModifiedAt` is whole epoch seconds, stable across reads, and
//!   moves forward only when the rule is modified.

use serde_json::Value;

use service_com_amazonaws_xray_awsxray::ops::update_indexing_rule::ONLY_DEFAULT_RULE_IS_UPDATABLE;

use crate::xray_wire::{Answer, Xray};

/// The rule name every account holds, from `crate::state::DEFAULT_INDEXING_RULE_NAME`.
const DEFAULT_RULE: &str = "Default";

/// The `DesiredSamplingPercentage` the `Default` rule carries until it is
/// updated, from `crate::state::DEFAULT_INDEXING_SAMPLING_PERCENTAGE`.
const DEFAULT_PERCENTAGE: f64 = 1.0;

/// The single `IndexingRule` a successful listing carries.
fn only_rule(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    let rules = answer
        .body
        .get("IndexingRules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: no IndexingRules list in {}", answer.body));
    assert_eq!(
        rules.len(),
        1,
        "case {case}: expected exactly the account's one rule, got {rules:?}"
    );
    rules[0].clone()
}

/// A rule's `Rule.Probabilistic.DesiredSamplingPercentage`.
fn desired(rule: &Value, case: &str) -> f64 {
    rule.pointer("/Rule/Probabilistic/DesiredSamplingPercentage")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case}: no DesiredSamplingPercentage in {rule}"))
}

/// A rule's `Rule.Probabilistic.ActualSamplingPercentage`.
fn actual(rule: &Value, case: &str) -> f64 {
    rule.pointer("/Rule/Probabilistic/ActualSamplingPercentage")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case}: no ActualSamplingPercentage in {rule}"))
}

/// A rule's `ModifiedAt`, as the epoch seconds it travels as.
fn modified_at(rule: &Value, case: &str) -> f64 {
    rule.get("ModifiedAt")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case}: no ModifiedAt in {rule}"))
}

/// Floating-point percentage compare: the values travel as JSON doubles.
fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-9
}

/// Block until just past the next whole-second boundary.
///
/// `IndexingRule$ModifiedAt` is documented in whole Unix seconds, so a stamp
/// taken before this wait and one taken after it are necessarily different
/// values — which is what lets a restamp be asserted strictly. Bounded by one
/// second plus a small margin, and uses no runtime timer feature.
fn wait_past_the_next_second() {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch");
    let to_boundary = ::std::time::Duration::from_nanos(
        1_000_000_000 - u64::from(since_epoch.subsec_nanos()),
    );
    ::std::thread::sleep(to_boundary + ::std::time::Duration::from_millis(50));
}

#[tokio::test]
async fn a_fresh_account_reports_its_default_indexing_rule() {
    let xray = Xray::new();

    let answer = xray.get_rules(None).await;
    let rule = only_rule(&answer, "fresh store");

    assert_eq!(
        rule.get("Name").and_then(Value::as_str),
        Some(DEFAULT_RULE),
        "case fresh store: expected the account's `{DEFAULT_RULE}` rule, got {rule}"
    );
    assert!(
        close(desired(&rule, "fresh store"), DEFAULT_PERCENTAGE),
        "case fresh store: expected DesiredSamplingPercentage {DEFAULT_PERCENTAGE}, got {}",
        desired(&rule, "fresh store")
    );
    assert!(
        close(actual(&rule, "fresh store"), DEFAULT_PERCENTAGE),
        "case fresh store: nothing holds ingest below the configured rate, so the applied \
         percentage must equal the configured {DEFAULT_PERCENTAGE}, got {}",
        actual(&rule, "fresh store")
    );
    // `ModifiedAt` is documented "in Unix time seconds", so it carries no
    // sub-second part.
    let stamp = modified_at(&rule, "fresh store");
    assert!(
        close(stamp.fract(), 0.0) && stamp > 0.0,
        "case fresh store: expected ModifiedAt in whole epoch seconds, got {stamp}"
    );
    assert!(
        answer.body.get("NextToken").is_none(),
        "case fresh store: one rule fits a page, so no NextToken is due: {}",
        answer.body
    );
}

#[tokio::test]
async fn an_update_is_persisted_and_read_back() {
    // The rows differ along the configured percentage, including both published
    // bounds of the 0..=100 percentage range, a fraction, and an integral value
    // the issue did not cite.
    let percentages = [0.0_f64, 0.5, 1.0, 37.25, 42.0, 99.9, 100.0];

    for percentage in percentages {
        let case = format!("update to {percentage}");
        let xray = Xray::new();

        let updated = xray.update_rule(DEFAULT_RULE, percentage).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200, got {} with body {}",
            updated.status, updated.body
        );
        let returned = updated
            .body
            .get("IndexingRule")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: no IndexingRule in {}", updated.body));
        assert_eq!(
            returned.get("Name").and_then(Value::as_str),
            Some(DEFAULT_RULE),
            "case {case}: the update must answer the rule it rewrote, got {returned}"
        );
        assert!(
            close(desired(&returned, &case), percentage),
            "case {case}: expected the submitted DesiredSamplingPercentage back, got {}",
            desired(&returned, &case)
        );

        // The round trip: an independent read reports what was written.
        let read_back = only_rule(&xray.get_rules(None).await, &case);
        assert!(
            close(desired(&read_back, &case), percentage),
            "case {case}: GetIndexingRules must report the stored percentage, got {}",
            desired(&read_back, &case)
        );
        assert!(
            close(actual(&read_back, &case), percentage),
            "case {case}: the applied percentage must track the configured one, got {}",
            actual(&read_back, &case)
        );
        assert_eq!(
            read_back, returned,
            "case {case}: the read and the update must answer the SAME stored record"
        );
    }
}

#[tokio::test]
async fn the_latest_write_wins_and_a_read_changes_nothing() {
    let xray = Xray::new();

    // A sequence, so the reader is pinned to the LAST write rather than to any
    // one request's value.
    for percentage in [12.5_f64, 0.0, 100.0, 7.0] {
        let case = format!("sequence step {percentage}");
        let updated = xray.update_rule(DEFAULT_RULE, percentage).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200, got {} with body {}",
            updated.status, updated.body
        );

        let first = only_rule(&xray.get_rules(None).await, &case);
        let second = only_rule(&xray.get_rules(None).await, &case);
        assert!(
            close(desired(&first, &case), percentage),
            "case {case}: expected the latest write {percentage}, got {}",
            desired(&first, &case)
        );
        // A read must not change what a later reader sees — including the
        // `ModifiedAt` stamp, which only a modification moves.
        assert_eq!(
            first, second,
            "case {case}: two reads with no write between them must answer identically"
        );
    }
}

#[tokio::test]
async fn modified_at_moves_only_when_the_rule_is_modified() {
    let xray = Xray::new();

    let before = modified_at(&only_rule(&xray.get_rules(None).await, "before"), "before");

    // A refused update modifies nothing, so the stamp must not move.
    let refused = xray.update_rule(DEFAULT_RULE, 101.0).await;
    assert_eq!(
        refused.status, 400,
        "case refused update: expected HTTP 400, got {} with body {}",
        refused.status, refused.body
    );
    let after_refusal = modified_at(
        &only_rule(&xray.get_rules(None).await, "after refusal"),
        "after refusal",
    );
    assert!(
        close(after_refusal, before),
        "case after refusal: a refused update must leave ModifiedAt at {before}, got \
         {after_refusal}"
    );

    // Cross a whole-second boundary first, so a stamp that really was rewritten
    // has to land on a LATER second than the one read above — otherwise an
    // implementation that never restamped would satisfy the assertion too.
    wait_past_the_next_second();

    let accepted = xray.update_rule(DEFAULT_RULE, 25.0).await;
    assert_eq!(
        accepted.status, 200,
        "case accepted update: expected HTTP 200, got {} with body {}",
        accepted.status, accepted.body
    );
    let read_back = only_rule(&xray.get_rules(None).await, "after update");
    let after_update = modified_at(&read_back, "after update");
    assert!(
        after_update > before,
        "case after update: a modification must restamp ModifiedAt past {before}, got \
         {after_update}"
    );
    assert!(
        close(after_update.fract(), 0.0),
        "case after update: expected ModifiedAt in whole epoch seconds, got {after_update}"
    );
    assert!(
        close(desired(&read_back, "after update"), 25.0),
        "case after update: expected the written percentage 25, got {}",
        desired(&read_back, "after update")
    );
}

#[tokio::test]
async fn a_percentage_outside_the_range_is_refused_and_stores_nothing() {
    // One row just past each published bound of the 0..=100 percentage range,
    // plus values far outside it.
    let out_of_range = [-0.000_1_f64, -1.0, 100.000_1, 101.0, 1_000.0];

    for percentage in out_of_range {
        let case = format!("percentage {percentage}");
        let xray = Xray::new();

        let answer = xray.update_rule(DEFAULT_RULE, percentage).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {case}: expected InvalidRequestException, got {:?}",
            answer.error_type
        );

        // The refusal left the stored rule exactly as it was.
        let read_back = only_rule(&xray.get_rules(None).await, &case);
        assert!(
            close(desired(&read_back, &case), DEFAULT_PERCENTAGE),
            "case {case}: a refused update must leave the stored percentage at \
             {DEFAULT_PERCENTAGE}, got {}",
            desired(&read_back, &case)
        );
    }
}

#[tokio::test]
async fn a_name_other_than_the_default_rule_is_refused_and_creates_nothing() {
    // The rows differ in how the name misses the one updatable rule: case,
    // surrounding space, the empty string, and a name of another shape
    // entirely.
    let unknown = ["default", "DEFAULT", "Default ", "", "SamplingRule", "Other"];

    for name in unknown {
        let case = format!("name {name:?}");
        let xray = Xray::new();

        let answer = xray.update_rule(name, 50.0).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {case}: expected InvalidRequestException, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.message(),
            Some(ONLY_DEFAULT_RULE_IS_UPDATABLE),
            "case {case}: expected the restriction to be stated, got {}",
            answer.body
        );

        // Nothing was created: the listing still carries the one rule, with its
        // configuration untouched.
        let read_back = only_rule(&xray.get_rules(None).await, &case);
        assert_eq!(
            read_back.get("Name").and_then(Value::as_str),
            Some(DEFAULT_RULE),
            "case {case}: a refused update must not add a rule, got {read_back}"
        );
        assert!(
            close(desired(&read_back, &case), DEFAULT_PERCENTAGE),
            "case {case}: a refused update must not rewrite the stored rule, got {}",
            desired(&read_back, &case)
        );
    }
}

#[tokio::test]
async fn an_absent_next_token_lists_while_a_token_we_never_issued_is_refused() {
    let xray = Xray::new();

    // Absent: the first (and only) page.
    let listed = xray.get_rules(None).await;
    assert_eq!(
        listed.status, 200,
        "case absent token: expected HTTP 200, got {} with body {}",
        listed.status, listed.body
    );

    // Present but never issued — the empty string among them, which is why an
    // empty token is NOT read as an absent one.
    let never_issued = ["", " ", "Default ", "default", "SamplingRule", "0"];
    for token in never_issued {
        let case = format!("token {token:?}");
        let answer = xray.get_rules(Some(token)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {case}: expected InvalidRequestException, got {:?}",
            answer.error_type
        );
    }

    // A token naming a stored rule resumes AT that rule, so the one-rule
    // listing is answered in full rather than refused.
    let resumed = xray.get_rules(Some(DEFAULT_RULE)).await;
    let rule = only_rule(&resumed, "token Default");
    assert_eq!(
        rule.get("Name").and_then(Value::as_str),
        Some(DEFAULT_RULE),
        "case token Default: expected the page to start at the named rule, got {rule}"
    );
}
