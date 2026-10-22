// SPDX-License-Identifier: CC0-1.0

//! The rule `visibleServices` / `visibleRegions` follow: a list setting's value
//! is the SEQUENCE a request supplied, so `UpdateAccountCustomizations` stores
//! and reports that sequence verbatim and `GetAccountCustomizations` reports the
//! same one afterwards — the service neither re-orders nor de-duplicates the
//! array it was sent.
//!
//! Every case drives the real wire path through the shared harness in
//! [`crate::wire`], so what is asserted is the emulator's HTTP behaviour; the
//! expectation comes from [`crate::support::reported`].
//!
//! Nothing in the model orders either list (`ServiceList`/`RegionsList` are
//! plain lists, no `@uniqueItems`, no sort rule —
//! `smithy_query(query="shape", shape="ServiceList,RegionsList")`), so a member
//! named twice stays twice and a request's order survives the round trip.
//!
//! Service identifiers here are drawn from the catalog `ListServices` pages over
//! as well as written out by hand; which of the two a value comes from makes no
//! difference to acceptance (`visible_services_round_trip` owns that rule).

use crate::support::reported;
use crate::wire::{build_router, get_customizations, strings, update_customizations, whole_catalog};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// `count` catalog service identifiers, spread across the catalog rather than
/// taken from its head, so the rows are not all near-neighbours in its order.
async fn catalog_services(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let catalog = whole_catalog(router).await;
    assert!(
        catalog.len() >= count,
        "the catalog must hold at least {count} entries to draw rows from, got {}",
        catalog.len()
    );
    let stride = catalog.len() / count;
    (0..count)
        .map(|n| catalog[(n * stride).min(catalog.len() - 1)].clone())
        .collect()
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
    // A member named twice: it stays twice, where it was named.
    &["eu-west-1", "eu-west-1"],
    &["us-west-2", "eu-west-1", "us-west-2"],
];

/// The rule, over a table spanning the class: whatever sequence a request names a
/// list setting's members in, the response reports that very sequence — and
/// `GetAccountCustomizations` reports it again afterwards.
///
/// Rows differ along size (0, 1, 3, 4), supplied sequence (ascending, descending,
/// rotated), element form (plain, hyphenated, digit-bearing) and duplication,
/// which is what the rule generalises over.
#[tokio::test]
async fn a_list_setting_reads_back_as_the_sequence_the_request_supplied() {
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

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched.get("visibleRegions"),
            Some(&expected),
            "case visibleRegions {row:?}: Get must report what Update stored, got {fetched:?}"
        );
    }
}

/// The reported sequence, pinned by HAND rather than by passing the supplied list
/// through a helper: each row names the sequence a request supplies and the exact
/// array the response must carry, written out literally.
///
/// Rows cover a descending and a rotated supply, a single member, a duplicate
/// (kept — no `@uniqueItems`), and the two sequences the issue's recordings show
/// (`['s3', 'ec2']` for `visibleServices`, which must NOT come back sorted).
#[tokio::test]
async fn the_reported_sequence_is_the_one_written_out_in_each_row() {
    // (member, supplied sequence, the exact answer) — the service identifiers are
    // legal under `Service`'s pattern, the Region codes under `Region`'s.
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("visibleServices", vec!["s3", "ec2"], vec!["s3", "ec2"]),
        (
            "visibleServices",
            vec!["sqs", "lambda", "ec2"],
            vec!["sqs", "lambda", "ec2"],
        ),
        ("visibleServices", vec!["s3", "s3"], vec!["s3", "s3"]),
        (
            "visibleServices",
            vec!["account-access", "acm"],
            vec!["account-access", "acm"],
        ),
        (
            "visibleRegions",
            vec!["us-east-1", "us-west-2", "eu-west-1"],
            vec!["us-east-1", "us-west-2", "eu-west-1"],
        ),
        ("visibleRegions", vec!["sa-east-1"], vec!["sa-east-1"]),
        (
            "visibleRegions",
            vec!["us-gov-west-1", "ap-southeast-2", "ca-central-1"],
            vec!["us-gov-west-1", "ap-southeast-2", "ca-central-1"],
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
/// cases, so nothing is left to a seed): each one is answered with ITSELF, and a
/// second update naming the same members in the reverse sequence replaces it with
/// that reverse sequence. The response is a function of the supplied document,
/// element order included.
///
/// Re-applying the SAME permutation is the idempotence the model declares, and it
/// is asserted here too.
#[tokio::test]
async fn every_permutation_is_answered_with_itself() {
    let members = catalog_services(&build_router(), 4).await;
    let borrowed: Vec<&str> = members.iter().map(String::as_str).collect();

    for permutation in permutations(&borrowed) {
        let router = build_router();
        let expected = reported(&permutation);

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

        // A DIFFERENT sequence of the same members is a different document: it is
        // stored and reported as sent.
        let reversed: Vec<&str> = permutation.iter().rev().copied().collect();
        let expected_reversed = reported(&reversed);
        let (_, _, again) =
            update_customizations(&router, json!({"visibleServices": strings(&reversed)})).await;
        assert_eq!(
            again.get("visibleServices"),
            Some(&expected_reversed),
            "case {permutation:?} then {reversed:?}: expected {expected_reversed}, got {again:?}"
        );
        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched.get("visibleServices"),
            Some(&expected_reversed),
            "case {permutation:?} then {reversed:?}: Get must report {expected_reversed}, got {fetched:?}"
        );
    }
}

/// The supplied list decides the whole value: a member the new list does not name
/// is gone, one it adds is there, and the answer is the new sequence — over a
/// table of (first list, second list) pairs covering a superset, a subset, a
/// disjoint set, an overlapping set and the empty list.
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
            name: "the same members in the reverse sequence",
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
/// members, same sequence — and repeated reads do not move them.
#[tokio::test]
async fn a_partial_update_reports_the_other_settings_unchanged() {
    let router = build_router();
    // Two catalog identifiers, named in the reverse of the catalog's own order,
    // so a re-ordering write would be visible.
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
