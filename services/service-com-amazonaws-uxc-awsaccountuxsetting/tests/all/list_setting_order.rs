// SPDX-License-Identifier: CC0-1.0

//! The rule `visibleServices` / `visibleRegions` follow: a list setting is
//! CANONICALIZED on the write, so `UpdateAccountCustomizations` answers with the
//! setting's members in one canonical sequence — ascending — whatever sequence
//! the request named them in, and `GetAccountCustomizations` reports that very
//! same sequence afterwards. The members themselves are the caller's: nothing is
//! added, dropped or de-duplicated.
//!
//! Every case drives the real wire path through the shared harness in
//! [`crate::wire`], so what is asserted is the emulator's HTTP behaviour; the
//! expectation comes from [`crate::support::reported`].
//!
//! Nothing in the model fixes the order (`ServiceList`/`RegionsList` are plain
//! lists, no `@uniqueItems`, no sort trait —
//! `smithy_query(query="shape", shape="ServiceList,RegionsList")`), so the order
//! is recorded behaviour: the corpus reports these settings ascending. The absence
//! of `@uniqueItems` is why a member named twice stays twice.
//!
//! Service identifiers here are drawn from the catalog `ListServices` pages over,
//! which is what `visibleServices` admits
//! (`visible_service_catalog_membership` owns that rule); this suite is about the
//! ORDER a settable list is reported in.

use crate::support::{members, reported};
use crate::wire::{
    build_router, catalog_services, get_customizations, strings, update_customizations,
};
use serde_json::{json, Value};

/// The two halves of the rule that do not depend on WHICH order is canonical: the
/// answer holds exactly the members the request supplied (as a multiset, so a
/// duplicate is kept), and it is ascending.
fn assert_canonical(case: &str, supplied: &[&str], answered: &[String]) {
    let mut expected_multiset: Vec<String> = supplied.iter().map(|v| (*v).to_string()).collect();
    expected_multiset.sort();
    let mut answered_multiset = answered.to_vec();
    answered_multiset.sort();
    assert_eq!(
        answered_multiset, expected_multiset,
        "case {case}: the answer must hold exactly the supplied members, got {answered:?}"
    );
    assert!(
        answered.windows(2).all(|pair| pair[0] <= pair[1]),
        "case {case}: the answer must be ascending, got {answered:?}"
    );
}

/// Region codes legal under the model's `Region` pattern
/// `^[a-z]{2}(-[a-z]{1,10}){1,2}-[1-9]$` (`smithy_query(query="shape",
/// shape="RegionsList")`), including the three-segment and long-middle forms.
const REGION_ROWS: &[&[&str]] = &[
    &["us-east-1", "us-west-2", "eu-west-1"],
    &["eu-west-1", "us-east-1", "us-west-2"],
    &["us-west-2", "us-east-1", "eu-west-1"],
    &["ap-southeast-2", "us-gov-west-1", "ca-central-1"],
    &["sa-east-1"],
    &[],
    // A member named twice: it stays twice — no `@uniqueItems`.
    &["eu-west-1", "eu-west-1"],
    &["us-west-2", "eu-west-1", "us-west-2"],
];

/// The rule, over a table spanning the class: whatever sequence a request names a
/// list setting's members in, the response reports those members in the canonical
/// sequence — and `GetAccountCustomizations` reports the same one afterwards.
///
/// Rows differ along size (0, 1, 3, 4), supplied sequence (ascending, descending,
/// rotated), element form (plain, hyphenated, digit-bearing) and duplication,
/// which is what the rule generalises over.
#[tokio::test]
async fn a_list_setting_reads_back_in_the_canonical_sequence() {
    // Four catalog identifiers, then the sequences to name them in. The rows are
    // built from the catalog, so none of them is the pair the issue cited.
    let drawn = catalog_services(&build_router(), 4).await;
    let [first, second, third, fourth] = [&drawn[0], &drawn[1], &drawn[2], &drawn[3]];
    let service_rows: Vec<Vec<&str>> = vec![
        vec![second, first],
        vec![first, second],
        vec![fourth, third, second, first],
        vec![second, fourth, first, third],
        vec![third],
        vec![],
        // A member named twice, and one named three times.
        vec![second, first, second],
        vec![fourth, fourth, fourth],
    ]
    .into_iter()
    .map(|row| row.into_iter().map(String::as_str).collect())
    .collect();

    for row in &service_rows {
        let router = build_router();
        let expected = reported(row);

        let (status, _, updated) =
            update_customizations(&router, json!({"visibleServices": strings(row)})).await;
        assert_eq!(
            status, 200,
            "case visibleServices {row:?}: expected 200, got {status} ({updated})"
        );
        assert_eq!(
            updated.get("visibleServices"),
            Some(&expected),
            "case visibleServices {row:?}: expected {expected}, got {updated:?}"
        );
        assert_canonical(
            &format!("visibleServices {row:?}"),
            row,
            &members(&updated, "visibleServices"),
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched.get("visibleServices"),
            Some(&expected),
            "case visibleServices {row:?}: Get must report what Update stored, got {fetched:?}"
        );
    }

    for row in REGION_ROWS {
        let router = build_router();
        let expected = reported(row);

        let (status, _, updated) =
            update_customizations(&router, json!({"visibleRegions": strings(row)})).await;
        assert_eq!(
            status, 200,
            "case visibleRegions {row:?}: expected 200, got {status} ({updated})"
        );
        assert_eq!(
            updated.get("visibleRegions"),
            Some(&expected),
            "case visibleRegions {row:?}: expected {expected}, got {updated:?}"
        );
        assert_canonical(
            &format!("visibleRegions {row:?}"),
            row,
            &members(&updated, "visibleRegions"),
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched.get("visibleRegions"),
            Some(&expected),
            "case visibleRegions {row:?}: Get must report what Update stored, got {fetched:?}"
        );
    }
}

/// The canonical sequence, pinned by HAND rather than by passing the supplied list
/// through a helper: each row names the sequence a request supplies and the exact
/// array the response must carry, written out literally.
///
/// Rows cover a descending and a rotated supply, a single member, a duplicate
/// (kept — no `@uniqueItems`), identifiers whose ordering turns on a hyphen or a
/// digit, and the sequence the corpus records for `visibleRegions`
/// (`['us-east-1', 'us-west-2', 'eu-west-1']` supplied, answered
/// `['eu-west-1', 'us-east-1', 'us-west-2']`).
#[tokio::test]
async fn the_canonical_sequence_is_the_one_written_out_in_each_row() {
    // (member, supplied sequence, the exact answer) — the service identifiers are
    // entries of the catalog, the Region codes legal under `Region`'s pattern.
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("visibleServices", vec!["s3", "ec2"], vec!["ec2", "s3"]),
        (
            "visibleServices",
            vec!["sqs", "lambda", "ec2"],
            vec!["ec2", "lambda", "sqs"],
        ),
        ("visibleServices", vec!["s3", "s3"], vec!["s3", "s3"]),
        (
            "visibleServices",
            vec!["account-access", "acm"],
            vec!["account-access", "acm"],
        ),
        // A hyphen (0x2D) sorts before a letter, and a digit (0x30-0x39) before
        // both: `health-dashboard` < `healthlake`, and `s3` < `sagemaker`.
        (
            "visibleServices",
            vec!["sagemaker", "healthlake", "s3", "health-dashboard"],
            vec!["health-dashboard", "healthlake", "s3", "sagemaker"],
        ),
        (
            "visibleRegions",
            vec!["us-east-1", "us-west-2", "eu-west-1"],
            vec!["eu-west-1", "us-east-1", "us-west-2"],
        ),
        ("visibleRegions", vec!["sa-east-1"], vec!["sa-east-1"]),
        (
            "visibleRegions",
            vec!["us-gov-west-1", "ap-southeast-2", "ca-central-1"],
            vec!["ap-southeast-2", "ca-central-1", "us-gov-west-1"],
        ),
        (
            "visibleRegions",
            vec!["us-west-2", "us-east-2", "us-east-1"],
            vec!["us-east-1", "us-east-2", "us-west-2"],
        ),
    ];

    for (member, supplied, answer) in cases {
        let router = build_router();
        let expected = strings(&answer);

        let (status, _, updated) =
            update_customizations(&router, json!({member: strings(&supplied)})).await;
        assert_eq!(
            status, 200,
            "case {member} {supplied:?}: expected 200, got {status} ({updated})"
        );
        assert_eq!(
            updated.get(member),
            Some(&expected),
            "case {member} {supplied:?}: expected exactly {answer:?}, got {updated:?}"
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched.get(member),
            Some(&expected),
            "case {member} {supplied:?}: Get must answer exactly {answer:?}, got {fetched:?}"
        );
    }
}

/// Every permutation of one four-member list, enumerated exhaustively (4! = 24
/// cases, so nothing is left to a seed): each one is answered with the SAME
/// canonical document, and so is the record read back afterwards. The response is
/// a function of the SET of members the request named, not of their sequence —
/// which is also what makes the setting converge rather than depend on which of
/// two equivalent requests arrived last.
///
/// Re-applying the same permutation is the idempotence the model declares, and it
/// is asserted here too.
#[tokio::test]
async fn every_permutation_is_answered_with_the_same_canonical_sequence() {
    let drawn = catalog_services(&build_router(), 4).await;
    let borrowed: Vec<&str> = drawn.iter().map(String::as_str).collect();
    let expected = reported(&borrowed);

    for permutation in permutations(&borrowed) {
        let router = build_router();

        let (status, _, updated) =
            update_customizations(&router, json!({"visibleServices": strings(&permutation)})).await;
        assert_eq!(
            status, 200,
            "case {permutation:?}: expected 200, got {status} ({updated})"
        );
        assert_eq!(
            updated.get("visibleServices"),
            Some(&expected),
            "case {permutation:?}: expected {expected}, got {updated:?}"
        );

        // @idempotent: the same request again leaves the same state and body.
        let (_, _, repeated) =
            update_customizations(&router, json!({"visibleServices": strings(&permutation)})).await;
        assert_eq!(
            repeated.get("visibleServices"),
            Some(&expected),
            "case {permutation:?} twice: expected {expected}, got {repeated:?}"
        );

        // A DIFFERENT sequence of the same members names the same setting: the
        // stored value does not move.
        let reversed: Vec<&str> = permutation.iter().rev().copied().collect();
        let (_, _, again) =
            update_customizations(&router, json!({"visibleServices": strings(&reversed)})).await;
        assert_eq!(
            again.get("visibleServices"),
            Some(&expected),
            "case {permutation:?} then {reversed:?}: expected {expected}, got {again:?}"
        );
        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched.get("visibleServices"),
            Some(&expected),
            "case {permutation:?} then {reversed:?}: Get must report {expected}, got {fetched:?}"
        );
    }
}

/// The supplied list decides the whole value: a member the new list does not name
/// is gone, one it adds is there, and the answer is the new list canonicalized —
/// over a table of (first list, second list) pairs covering a superset, a subset, a
/// disjoint set, an overlapping set, a re-ordering and the empty list.
#[tokio::test]
async fn a_later_update_replaces_the_list_it_names() {
    struct Case {
        name: &'static str,
        first: &'static [&'static str],
        second: &'static [&'static str],
    }

    let cases = [
        Case {
            name: "superset adds two members",
            first: &["eu-west-1"],
            second: &["us-east-1", "us-west-2", "eu-west-1"],
        },
        Case {
            name: "subset drops a member",
            first: &["us-east-1", "us-west-2", "eu-west-1"],
            second: &["eu-west-1"],
        },
        Case {
            name: "disjoint set replaces every member",
            first: &["us-east-1", "us-west-2"],
            second: &["ap-south-1", "eu-central-1"],
        },
        Case {
            name: "overlapping set keeps one and swaps one",
            first: &["us-east-1", "us-west-2"],
            second: &["us-west-2", "sa-east-1"],
        },
        Case {
            name: "the same members in the reverse sequence leave the value alone",
            first: &["us-east-1", "us-west-2"],
            second: &["us-west-2", "us-east-1"],
        },
        Case {
            name: "empty list leaves the setting configured and empty",
            first: &["us-east-1", "us-west-2"],
            second: &[],
        },
    ];

    for case in cases {
        let router = build_router();
        let (_, _, first) =
            update_customizations(&router, json!({"visibleRegions": strings(case.first)})).await;
        assert_eq!(
            first.get("visibleRegions"),
            Some(&reported(case.first)),
            "case {:?}: the first update must answer {}, got {first:?}",
            case.name,
            reported(case.first)
        );

        let (status, _, second) =
            update_customizations(&router, json!({"visibleRegions": strings(case.second)})).await;
        let expected = reported(case.second);
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} ({second})",
            case.name
        );
        assert_eq!(
            second.get("visibleRegions"),
            Some(&expected),
            "case {:?}: expected {expected}, got {second:?}",
            case.name
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched.get("visibleRegions"),
            Some(&expected),
            "case {:?}: Get must report {expected}, got {fetched:?}",
            case.name
        );
    }
}

/// "Omitted settings remain unchanged": an update that names only an unrelated
/// setting answers with the other settings exactly as they were stored — same
/// members, same canonical sequence — and repeated reads do not move them.
#[tokio::test]
async fn a_partial_update_reports_the_other_settings_unchanged() {
    let router = build_router();
    // Two catalog identifiers, named in the reverse of the catalog's own order,
    // so the canonicalizing write is visible.
    let drawn = catalog_services(&router, 2).await;
    let services: Vec<&str> = vec![&drawn[1], &drawn[0]]
        .into_iter()
        .map(String::as_str)
        .collect();
    let regions = ["us-east-1", "us-west-2", "eu-west-1"];
    let expected_services = reported(&services);
    let expected_regions = reported(&regions);

    let (_, _, configured) = update_customizations(
        &router,
        json!({
            "visibleServices": strings(&services),
            "visibleRegions": strings(&regions),
        }),
    )
    .await;
    assert_eq!(
        configured.get("visibleServices"),
        Some(&expected_services),
        "configuring both settings must answer {expected_services}, got {configured:?}"
    );

    // Three unrelated partial updates in a row, each naming only a colour.
    for color in ["teal", "purple", "none"] {
        let (status, _, body) =
            update_customizations(&router, json!({"accountColor": color})).await;
        assert_eq!(
            status, 200,
            "case colour {color:?}: expected 200, got {status} ({body})"
        );
        assert_eq!(
            body,
            json!({
                "accountColor": color,
                "visibleServices": expected_services,
                "visibleRegions": expected_regions,
            }),
            "case colour {color:?}: a partial update must report the other settings unchanged, got {body:?}"
        );
    }

    // And a read after them reports the same record: a read changes nothing.
    let (_, _, fetched) = get_customizations(&router).await;
    assert_eq!(
        fetched,
        json!({
            "accountColor": "none",
            "visibleServices": expected_services,
            "visibleRegions": expected_regions,
        }),
        "the record after three partial updates must be unchanged, got {fetched:?}"
    );
}

/// An unconfigured list setting stays ABSENT (null = "all are visible"), while a
/// setting configured empty is present and empty: the two are different states.
#[tokio::test]
async fn an_unconfigured_setting_is_absent_and_an_empty_one_is_present() {
    let router = build_router();

    let (_, _, colour_only) =
        update_customizations(&router, json!({"accountColor": "green"})).await;
    assert_eq!(
        colour_only,
        json!({"accountColor": "green"}),
        "a colour-only update must leave both lists unconfigured, got {colour_only:?}"
    );

    let (_, _, emptied) = update_customizations(&router, json!({"visibleServices": []})).await;
    assert_eq!(
        emptied.get("visibleServices"),
        Some(&Value::Array(Vec::new())),
        "a setting configured empty must be reported as an empty list, got {emptied:?}"
    );
    assert!(
        emptied.get("visibleRegions").is_none(),
        "the still-unconfigured list must stay absent, got {emptied:?}"
    );
}

/// Every permutation of `members`, in a fixed order so a failure names a
/// reproducible case. Heap's algorithm, iterative over the 4! orderings.
fn permutations<'a>(members: &[&'a str]) -> Vec<Vec<&'a str>> {
    let mut current: Vec<&'a str> = members.to_vec();
    let mut counters = vec![0_usize; current.len()];
    let mut all = vec![current.clone()];
    let mut index = 0;
    while index < current.len() {
        if counters[index] < index {
            if index % 2 == 0 {
                current.swap(0, index);
            } else {
                current.swap(counters[index], index);
            }
            all.push(current.clone());
            counters[index] += 1;
            index = 0;
        } else {
            counters[index] = 0;
            index += 1;
        }
    }
    all
}
