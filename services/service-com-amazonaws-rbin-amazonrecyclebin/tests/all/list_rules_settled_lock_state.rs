// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the lock state `ListRules` reports and the one it filters on.
//!
//! The rule under test: an operation that reports retention rules answers with the lock
//! state the CLOCK has already reached — a rule whose unlock delay has run out is
//! `unlocked` — and it does so everywhere that state is read, so the state a page renders
//! and the state the `LockState` filter selects on are the same state a single-rule read
//! reports for the same record.
//!
//! The table spans what that rule generalises over: every lock state a rule can be in
//! (never locked, `locked`, `pending_unlock` while the delay still runs, `unlocked`
//! because the delay ran out), both doors a lock arrives through, the published ends of
//! the unlock-delay range and a value between them, every modelled resource type, every
//! modelled filter value, and page sizes that cut the matching rules into one, two and
//! all pages.
//!
//! An unlock delay is at least 7 days, so it cannot elapse inside a test: the instant the
//! transition falls due is moved into the past instead
//! ([`crate::update_rule_lock_invariant::end_unlock_delay`]) and the next request settles
//! the rule on its own, exactly as it would once the delay had really run out.
//!
//! Every case drives the service through the real router, exactly as a signed SDK request
//! would, and each row states its own case so a failure names the input.

use serde_json::{json, Value};

use crate::retention_rule_lifecycle::{
    create_rule, get_rule, list_of, list_rules, lock_configuration, retention,
    supported_retention,
};
use crate::rule_lock_transitions::{lock_rule, unlock_rule};
use crate::update_rule_lock_invariant::{end_unlock_delay, fixture, Fixture};
use smithy_server_runtime::MultiServiceRouter;

/// `ResourceType` accepts exactly these values.
/// Source: smithy_query(query="shape", shape="ResourceType") — enum values.
const RESOURCE_TYPES: [&str; 3] = ["EBS_SNAPSHOT", "EBS_VOLUME", "EC2_IMAGE"];

/// Every value `LockState` models, which is every value the filter accepts.
/// Source: smithy_query(query="shape", shape="LockState") — enum values.
const LOCK_STATES: [&str; 3] = ["locked", "pending_unlock", "unlocked"];

/// Published bounds of `UnlockDelay$UnlockDelayValue`, in `UnlockDelayUnit` `DAYS`.
/// Source: smithy_query(query="shape", shape="UnlockDelay") — range {min: 7, max: 30}.
const UNLOCK_DELAY_MIN: i64 = 7;
const UNLOCK_DELAY_MAX: i64 = 30;
/// A delay strictly between the published bounds, so no row sits only on an edge.
const UNLOCK_DELAY_MID: i64 = 19;

/// Page sizes every walk below is repeated at: unbounded, one rule per page, and a size
/// that cuts the rules of a type into several pages.
const PAGE_SIZES: [Option<usize>; 3] = [None, Some(1), Some(3)];

/// More pages than any row below can need, so a walk that fails to terminate fails the
/// case instead of hanging.
const MAX_PAGES: usize = 64;

/// Where the clock has brought a rule's lock by the time it is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockPhase {
    /// Created without a lock configuration and never locked.
    NeverLocked,
    /// Locked and never unlocked.
    Locked,
    /// Unlocked, with the unlock delay still running.
    PendingUnlock,
    /// Unlocked, with the unlock delay already run out.
    UnlockDelayExpired,
}

impl LockPhase {
    /// The `LockState` a rule in this phase reports once every transition the clock owes
    /// it has been materialised.
    ///
    /// Source: smithy_query(query="doc", shape="RuleSummary", member="LockState") —
    /// `locked`, `pending_unlock` "still within the unlock delay period", `unlocked` once
    /// it is not, and `null` for a rule that "has never been locked".
    fn settled_lock_state(self) -> Option<&'static str> {
        match self {
            LockPhase::NeverLocked => None,
            LockPhase::Locked => Some("locked"),
            LockPhase::PendingUnlock => Some("pending_unlock"),
            LockPhase::UnlockDelayExpired => Some("unlocked"),
        }
    }

    /// Whether a rule in this phase holds a lock at all, so the create has to carry — or
    /// `LockRule` has to take — a lock configuration.
    fn holds_a_lock(self) -> bool {
        self != LockPhase::NeverLocked
    }

    /// Whether a rule in this phase has been sent through `UnlockRule`.
    fn unlocked(self) -> bool {
        matches!(
            self,
            LockPhase::PendingUnlock | LockPhase::UnlockDelayExpired
        )
    }
}

/// One row of the table: the lock phase to bring a rule to, the resource type it retains,
/// the unlock delay its lock configuration names, and whether the lock arrived with the
/// create or through `LockRule` afterwards.
struct Row {
    phase: LockPhase,
    resource_type: &'static str,
    unlock_delay: i64,
    via_lock_rule: bool,
}

/// A rule the table created: its identifier, its type and the lock state it must report.
struct Created {
    case: String,
    identifier: String,
    resource_type: &'static str,
    lock_state: Option<&'static str>,
}

/// The table: every lock phase, at both ends of the unlock-delay range and between them,
/// through both doors a lock arrives by, for every modelled resource type.
fn rows() -> Vec<Row> {
    let phases: [(LockPhase, i64, bool); 8] = [
        (LockPhase::NeverLocked, UNLOCK_DELAY_MIN, false),
        (LockPhase::Locked, UNLOCK_DELAY_MIN, false),
        (LockPhase::Locked, UNLOCK_DELAY_MAX, true),
        (LockPhase::PendingUnlock, UNLOCK_DELAY_MAX, false),
        (LockPhase::PendingUnlock, UNLOCK_DELAY_MID, true),
        (LockPhase::UnlockDelayExpired, UNLOCK_DELAY_MIN, false),
        (LockPhase::UnlockDelayExpired, UNLOCK_DELAY_MAX, true),
        (LockPhase::UnlockDelayExpired, UNLOCK_DELAY_MID, false),
    ];
    let mut rows = Vec::new();
    for resource_type in RESOURCE_TYPES {
        for (phase, unlock_delay, via_lock_rule) in phases {
            rows.push(Row {
                phase,
                resource_type,
                unlock_delay,
                via_lock_rule,
            });
        }
    }
    rows
}

/// Create one rule and bring it to the row's lock phase, returning what it must report.
///
/// A lock holds only on a Region-level rule with no exclusion tags, so no row carries a
/// resource or exclusion tag selection.
async fn create_in_phase(fixture: &Fixture, row: &Row, nth: usize) -> Created {
    let case = format!(
        "{:?} via {} with an unlock delay of {} days ({})",
        row.phase,
        if row.via_lock_rule {
            "LockRule"
        } else {
            "CreateRule"
        },
        row.unlock_delay,
        row.resource_type
    );
    let configuration = lock_configuration(row.unlock_delay);
    let mut body = json!({
        "ResourceType": row.resource_type,
        "RetentionPeriod": retention(supported_retention(row.resource_type, nth)),
    });
    if row.phase.holds_a_lock() && !row.via_lock_rule {
        body["LockConfiguration"] = configuration.clone();
    }
    let (status, created) = create_rule(&fixture.router, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRule expected HTTP 201, got {status} / {created:?}"
    );
    let identifier = created["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier: {created:?}"))
        .to_string();

    if row.phase.holds_a_lock() && row.via_lock_rule {
        let (status, locked) = lock_rule(
            &fixture.router,
            &identifier,
            json!({ "LockConfiguration": configuration.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: LockRule expected HTTP 200, got {status} / {locked:?}"
        );
    }
    if row.phase.unlocked() {
        let (status, unlocking) = unlock_rule(&fixture.router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UnlockRule expected HTTP 200, got {status} / {unlocking:?}"
        );
        assert_eq!(
            unlocking["LockState"].as_str(),
            Some("pending_unlock"),
            "case {case:?}: expected LockState=pending_unlock from UnlockRule, got {:?}",
            unlocking["LockState"]
        );
    }
    if row.phase == LockPhase::UnlockDelayExpired {
        end_unlock_delay(&fixture.engine, &case, &identifier);
    }

    Created {
        case,
        identifier,
        resource_type: row.resource_type,
        lock_state: row.phase.settled_lock_state(),
    }
}

/// Create every row of the table in one store, in table order — which is the order a walk
/// of a resource type reports them in.
async fn create_every_row(fixture: &Fixture) -> Vec<Created> {
    let mut created = Vec::new();
    for (nth, row) in rows().iter().enumerate() {
        created.push(create_in_phase(fixture, row, nth).await);
    }
    created
}

/// One `ListRules` page: every summary's identifier with the lock state it reports, and
/// the cursor the page handed back.
async fn page(
    router: &MultiServiceRouter,
    resource_type: &str,
    lock_state: Option<&str>,
    max_results: Option<usize>,
    next_token: Option<&str>,
    case: &str,
) -> (Vec<(String, Option<String>)>, Option<String>) {
    let mut request = json!({ "ResourceType": resource_type });
    if let Some(wanted) = lock_state {
        request["LockState"] = Value::String(wanted.to_string());
    }
    if let Some(size) = max_results {
        request["MaxResults"] = Value::from(size);
    }
    if let Some(token) = next_token {
        request["NextToken"] = Value::String(token.to_string());
    }
    let (status, body) = list_rules(router, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListRules expected HTTP 200, got {status} / {body:?}"
    );
    let reported = list_of(&body, "Rules")
        .iter()
        .map(|summary| {
            (
                summary["Identifier"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                summary["LockState"].as_str().map(str::to_string),
            )
        })
        .collect();
    let token = body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (reported, token)
}

/// Walk every page of `resource_type`, optionally filtered, and return what the whole
/// walk reported in order.
async fn walk(
    router: &MultiServiceRouter,
    resource_type: &str,
    lock_state: Option<&str>,
    max_results: Option<usize>,
    case: &str,
) -> Vec<(String, Option<String>)> {
    let mut reported = Vec::new();
    let mut token: Option<String> = None;
    for pages in 0.. {
        assert!(
            pages <= MAX_PAGES,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        let (page, next) = page(
            router,
            resource_type,
            lock_state,
            max_results,
            token.as_deref(),
            case,
        )
        .await;
        reported.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    reported
}

/// What a walk of `resource_type` must report: the identifier and settled lock state of
/// every created rule of that type whose state the filter selects.
fn expected(
    created: &[Created],
    resource_type: &str,
    lock_state: Option<&str>,
) -> Vec<(String, Option<String>)> {
    created
        .iter()
        .filter(|rule| rule.resource_type == resource_type)
        .filter(|rule| lock_state.is_none_or(|wanted| rule.lock_state == Some(wanted)))
        .map(|rule| {
            (
                rule.identifier.clone(),
                rule.lock_state.map(str::to_string),
            )
        })
        .collect()
}

#[tokio::test]
async fn list_rules_reports_the_lock_state_the_clock_has_reached() {
    let fixture = fixture();
    let created = create_every_row(&fixture).await;

    for max_results in PAGE_SIZES {
        for resource_type in RESOURCE_TYPES {
            let case = format!(
                "{resource_type}, MaxResults={}",
                max_results.map_or("unset".to_string(), |size| size.to_string())
            );
            let reported = walk(&fixture.router, resource_type, None, max_results, &case).await;
            let wanted = expected(&created, resource_type, None);
            assert_eq!(
                reported, wanted,
                "case {case:?}: every summary must report the lock state the clock has \
                 reached, got {reported:?}"
            );
        }
    }

    // The same record read one rule at a time: a page and a single-rule read of one rule
    // can never disagree about its lock state, and a lock end time is reported only while
    // the unlock delay is still running.
    for rule in &created {
        let case = &rule.case;
        let (status, fetched) = get_rule(&fixture.router, &rule.identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
        );
        assert_eq!(
            fetched["LockState"].as_str(),
            rule.lock_state,
            "case {case:?}: expected GetRule to report LockState {:?}, got {:?}",
            rule.lock_state,
            fetched["LockState"]
        );
        let inside_the_delay = rule.lock_state == Some("pending_unlock");
        assert_eq!(
            fetched.get("LockEndTime").is_some(),
            inside_the_delay,
            "case {case:?}: a lock end time is reported only inside the unlock delay \
             (expected present={inside_the_delay}), got {:?}",
            fetched.get("LockEndTime")
        );
    }
}

#[tokio::test]
async fn list_rules_filters_on_the_lock_state_the_clock_has_reached() {
    let fixture = fixture();
    let created = create_every_row(&fixture).await;

    for resource_type in RESOURCE_TYPES {
        for filter in LOCK_STATES {
            let wanted = expected(&created, resource_type, Some(filter));
            assert!(
                !wanted.is_empty(),
                "case {resource_type:?}/{filter:?}: the table must hold a rule in every \
                 lock state, else the filter is asserted against nothing"
            );
            for max_results in PAGE_SIZES {
                let case = format!(
                    "{resource_type}, LockState={filter}, MaxResults={}",
                    max_results.map_or("unset".to_string(), |size| size.to_string())
                );
                let reported = walk(
                    &fixture.router,
                    resource_type,
                    Some(filter),
                    max_results,
                    &case,
                )
                .await;
                assert_eq!(
                    reported, wanted,
                    "case {case:?}: the filter must select exactly the rules the clock has \
                     brought to that lock state, got {reported:?}"
                );
                for (identifier, state) in &reported {
                    assert_eq!(
                        state.as_deref(),
                        Some(filter),
                        "case {case:?}: rule {identifier:?} was returned by the filter but \
                         reports {state:?}"
                    );
                }
            }
        }
    }
}
