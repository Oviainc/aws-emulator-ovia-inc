// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the idempotence of an upgrade ELIGIBILITY CHECK, as
//! `GetUpgradeHistory` and `GetUpgradeStatus` report it.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness comes from [`super::domain_lifecycle`] and the
//! `UpgradeDomain` / `GetUpgradeHistory` / `GetUpgradeStatus` calls from
//! [`super::upgrade_history`] and [`super::upgrade_status`]; this suite adds no second
//! copy of them.
//!
//! The rule under test: `UpgradeDomain$PerformCheckOnly` "does not actually perform the
//! upgrade", so a check records nothing a check already recorded — a domain holds at
//! most ONE eligibility-check run for the engine version it currently runs, and a later
//! check from that same version reports that run, leaving the upgrade history exactly
//! as the first check left it, whatever target the later call names. A run that does
//! perform something — an upgrade — records a run of its own every time, and once an
//! upgrade has moved the domain a check from the version it now runs is a different
//! check with a record of its own.
//!
//! So the assertions span SETS of inputs rather than one call: five rows of repeated
//! checks differing along what the rule generalises over — how many checks are repeated
//! (two, three, four), whether the later target is the same as the first or another one,
//! whether the later target sits nearer or further along the upgrade path, and which
//! engine line the targets are on (within the OpenSearch line, within the Elasticsearch
//! line, and across the crossing between them) — plus the boundaries of the rule: an
//! upgrade, which always appends, and a check from a version the domain has since been
//! upgraded onto, which appends too.

use serde_json::Value;

use super::domain_lifecycle::{string_member, Emulator, SETTLE_WAIT};
use super::upgrade_history::{entries, expected_name, history, names, settled_domain, upgrade};
use super::upgrade_status::status;

/// One row of the repeat table: the version the domain is created on and the targets
/// the eligibility checks name, in the order they are checked. The FIRST target is the
/// check the domain records; every later one repeats that check.
struct Repeat {
    case: &'static str,
    source: &'static str,
    targets: &'static [&'static str],
}

/// The repeat cases every assertion below is made over.
///
/// Every target is one `GetCompatibleVersions` offers for the row's source version —
/// the relation `state::compatible_target_versions` publishes — so each call is one the
/// service accepts rather than one it refuses.
const REPEATS: [Repeat; 5] = [
    Repeat {
        case: "two checks, another target on the same line",
        source: "OpenSearch_1.3",
        targets: &["OpenSearch_2.3", "OpenSearch_2.5"],
    },
    Repeat {
        case: "two checks, the identical target twice",
        source: "OpenSearch_2.11",
        targets: &["OpenSearch_2.19", "OpenSearch_2.19"],
    },
    Repeat {
        case: "four checks, nearer targets after the furthest",
        source: "OpenSearch_1.0",
        targets: &[
            "OpenSearch_2.19",
            "OpenSearch_1.3",
            "OpenSearch_1.1",
            "OpenSearch_2.19",
        ],
    },
    Repeat {
        case: "three checks across the engine line crossing",
        source: "Elasticsearch_7.10",
        targets: &["OpenSearch_1.0", "OpenSearch_1.3", "OpenSearch_1.2"],
    },
    Repeat {
        case: "two checks within the Elasticsearch line, a two-digit minor last",
        source: "Elasticsearch_6.8",
        targets: &["Elasticsearch_7.1", "Elasticsearch_7.10"],
    },
];

/// The whole history of a domain, newest first.
async fn whole_history(emulator: &Emulator, name: &str, case: &str) -> Vec<Value> {
    entries(&history(emulator, name, "").await, case)
}

#[tokio::test]
async fn a_repeat_eligibility_check_leaves_the_history_as_the_first_check_left_it() {
    let emulator = Emulator::new();

    for (index, row) in REPEATS.iter().enumerate() {
        let case = row.case;
        // Each row gets its own domain, so the only runs its history holds are the
        // checks the row performs.
        let name = format!("check-repeat-{index}");
        settled_domain(&emulator, &name, row.source).await;

        let first_target: &str = row
            .targets
            .first()
            .copied()
            .expect("a row checks at least once");
        let first = upgrade(&emulator, &name, first_target, true).await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected HTTP 200 for the first check, got {} with {:?}",
            first.status, first.body
        );
        let first_id = string_member(&first.body, "UpgradeId", case);
        assert!(
            !first_id.is_empty(),
            "case {case}: expected the first check to name a run, got {:?}",
            first.body
        );
        // The check takes the span the clock owes it; once that has passed the record
        // it left is terminal, so a repeat is compared against a settled history.
        tokio::time::sleep(SETTLE_WAIT).await;

        let baseline = whole_history(&emulator, &name, case).await;
        assert_eq!(
            baseline.len(),
            1,
            "case {case}: expected the one check recorded, got {:?}",
            names(&baseline)
        );
        assert_eq!(
            string_member(&baseline[0], "UpgradeName", case),
            expected_name(row.source, first_target, true),
            "case {case}: expected the check described by the versions it went between, \
             got {:?}",
            baseline[0]
        );

        for target in &row.targets[1..] {
            let repeated = upgrade(&emulator, &name, target, true).await;
            assert_eq!(
                repeated.status, 200,
                "case {case} / repeat to {target}: expected HTTP 200, got {} with {:?}",
                repeated.status, repeated.body
            );
            assert_eq!(
                string_member(&repeated.body, "TargetVersion", case),
                *target,
                "case {case} / repeat to {target}: expected the requested TargetVersion \
                 echoed, got {:?}",
                repeated.body
            );
            assert_eq!(
                repeated.body["PerformCheckOnly"].as_bool(),
                Some(true),
                "case {case} / repeat to {target}: expected PerformCheckOnly true, got {:?}",
                repeated.body
            );
            // The repeat performs nothing, so it names the run the domain already
            // holds rather than a run of its own.
            assert_eq!(
                string_member(&repeated.body, "UpgradeId", case),
                first_id,
                "case {case} / repeat to {target}: expected the existing run's UpgradeId \
                 {first_id}, got {:?}",
                repeated.body
            );
            assert!(
                repeated.body["ChangeProgressDetails"].is_null(),
                "case {case} / repeat to {target}: a check starts no configuration \
                 change, got {:?}",
                repeated.body
            );
            tokio::time::sleep(SETTLE_WAIT).await;

            let after = whole_history(&emulator, &name, case).await;
            assert_eq!(
                after, baseline,
                "case {case} / repeat to {target}: expected the history the first check \
                 left, got {:?}",
                names(&after)
            );

            // The status is a view of that same single record, so it still describes the
            // first check too.
            let reported = status(&emulator, &name).await;
            assert_eq!(
                string_member(&reported.body, "UpgradeName", case),
                expected_name(row.source, first_target, true),
                "case {case} / repeat to {target}: expected the status to report the \
                 recorded check, got {:?}",
                reported.body
            );

            // A check performs no upgrade, however many times it is performed.
            let described = emulator.describe(&name).await;
            assert_eq!(
                string_member(&described.body["DomainStatus"], "EngineVersion", case),
                row.source,
                "case {case} / repeat to {target}: expected the domain still on the \
                 version it was created on, got {:?}",
                described.body["DomainStatus"]
            );
        }
    }
}

#[tokio::test]
async fn an_upgrade_appends_and_a_check_from_the_version_it_landed_on_is_its_own_check() {
    let emulator = Emulator::new();
    let name = "check-after-upgrade";
    let source = "OpenSearch_1.3";
    settled_domain(&emulator, name, source).await;

    // One check, then a repeat of it: the history holds the single check record.
    let first = upgrade(&emulator, name, "OpenSearch_2.3", true).await;
    assert_eq!(
        first.status, 200,
        "case first check: expected HTTP 200, got {} with {:?}",
        first.status, first.body
    );
    let first_id = string_member(&first.body, "UpgradeId", "first check");
    tokio::time::sleep(SETTLE_WAIT).await;
    let repeated = upgrade(&emulator, name, "OpenSearch_2.5", true).await;
    tokio::time::sleep(SETTLE_WAIT).await;
    assert_eq!(
        string_member(&repeated.body, "UpgradeId", "repeat check"),
        first_id,
        "case repeat check: expected the existing run's UpgradeId, got {:?}",
        repeated.body
    );
    let after_checks = whole_history(&emulator, name, "repeat check").await;
    assert_eq!(
        names(&after_checks),
        vec![expected_name(source, "OpenSearch_2.3", true)],
        "case repeat check: expected the one check recorded, got {:?}",
        names(&after_checks)
    );

    // The upgrade itself performs something, so it records a run of its own.
    let upgraded = upgrade(&emulator, name, "OpenSearch_2.3", false).await;
    assert_eq!(
        upgraded.status, 200,
        "case upgrade: expected HTTP 200, got {} with {:?}",
        upgraded.status, upgraded.body
    );
    assert_ne!(
        string_member(&upgraded.body, "UpgradeId", "upgrade"),
        first_id,
        "case upgrade: expected an upgrade to record a run of its own, got {:?}",
        upgraded.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    let after_upgrade = whole_history(&emulator, name, "upgrade").await;
    assert_eq!(
        names(&after_upgrade),
        vec![
            expected_name(source, "OpenSearch_2.3", false),
            expected_name(source, "OpenSearch_2.3", true),
        ],
        "case upgrade: expected the upgrade recorded ahead of the check, got {:?}",
        names(&after_upgrade)
    );

    // The domain now runs the version that upgrade landed, so a check from THERE is a
    // different check: it records a run of its own.
    let moved_check = upgrade(&emulator, name, "OpenSearch_2.5", true).await;
    assert_eq!(
        moved_check.status, 200,
        "case check after the move: expected HTTP 200, got {} with {:?}",
        moved_check.status, moved_check.body
    );
    let moved_id = string_member(&moved_check.body, "UpgradeId", "check after the move");
    assert_ne!(
        moved_id, first_id,
        "case check after the move: expected a run of its own, got {:?}",
        moved_check.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    let after_moved_check = whole_history(&emulator, name, "check after the move").await;
    assert_eq!(
        names(&after_moved_check),
        vec![
            expected_name("OpenSearch_2.3", "OpenSearch_2.5", true),
            expected_name(source, "OpenSearch_2.3", false),
            expected_name(source, "OpenSearch_2.3", true),
        ],
        "case check after the move: expected the new check recorded, got {:?}",
        names(&after_moved_check)
    );

    // And repeating THAT check collapses into it, just as the first one did.
    let moved_repeat = upgrade(&emulator, name, "OpenSearch_2.7", true).await;
    assert_eq!(
        string_member(&moved_repeat.body, "UpgradeId", "repeat after the move"),
        moved_id,
        "case repeat after the move: expected the existing run's UpgradeId, got {:?}",
        moved_repeat.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    assert_eq!(
        whole_history(&emulator, name, "repeat after the move").await,
        after_moved_check,
        "case repeat after the move: expected the history unchanged"
    );

    // Two upgrades in a row each record a run: nothing collapses what performs
    // something.
    for (case, target) in [
        ("second upgrade", "OpenSearch_2.5"),
        ("third upgrade", "OpenSearch_2.7"),
    ] {
        let answer = upgrade(&emulator, name, target, false).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        tokio::time::sleep(SETTLE_WAIT).await;
    }
    let after_upgrades = whole_history(&emulator, name, "two more upgrades").await;
    assert_eq!(
        names(&after_upgrades),
        vec![
            expected_name("OpenSearch_2.5", "OpenSearch_2.7", false),
            expected_name("OpenSearch_2.3", "OpenSearch_2.5", false),
            expected_name("OpenSearch_2.3", "OpenSearch_2.5", true),
            expected_name(source, "OpenSearch_2.3", false),
            expected_name(source, "OpenSearch_2.3", true),
        ],
        "case two more upgrades: expected a run recorded per upgrade, got {:?}",
        names(&after_upgrades)
    );
}
