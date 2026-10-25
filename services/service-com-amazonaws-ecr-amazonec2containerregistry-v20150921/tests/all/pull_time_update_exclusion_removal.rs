// SPDX-License-Identifier: CC0-1.0

//! Removing a principal from the registry's pull time update exclusion list, end to end
//! over the wire.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.
//!
//! The rule under test: an exclusion is a registry-level record keyed by the IAM
//! principal ARN, so `DeregisterPullTimeUpdateExclusion` either removes the one record
//! naming that principal — answering that ARN, after which
//! `ListPullTimeUpdateExclusions` no longer reports it and every other record is
//! untouched — or, for a principal the registry does not exclude, is refused with
//! `ExclusionNotFoundException` and removes nothing. Removal is not a one-way door: the
//! same principal can be registered again afterwards, which creates a new record.
//!
//! The ARN rows span the forms `PrincipalArn` admits — its modelled
//! `@length(min: 0, max: 200)` and
//! `@pattern("^arn:aws(-[a-z]+)*:iam::[0-9]{12}:(role|user)/[\w+=,.@-]+(/[\w+=,.@-]+)*$")`,
//! read with `smithy_query(query="shape",shape="PrincipalArn")` — rather than one
//! recorded ARN: partition, principal kind, account, path depth, name alphabet and
//! length.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness this crate shares — one emulator and one awsJson1_1 call — rather
// than a copy of it: `tests/all/wire.rs` exists so a new suite does not repeat it.
use crate::wire::{call, emulator, ok};

/// The modelled ceiling on `PrincipalArn` (`@length(min: 0, max: 200)`).
const MAX_PRINCIPAL_ARN_LEN: usize = 200;

/// Every exclusion the registry holds, walked page by page so a listing longer than one
/// page is still compared in full. (The sibling suite's walker is module-private to it,
/// and it walks a caller-chosen page size rather than every page of the default one.)
async fn all_exclusions(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    all_exclusions_paged(router, case, None).await
}

/// The same walk as [`all_exclusions`], asking for `max_results` entries per page — for
/// a case that has to cross a page boundary on a list the registry's quota keeps below
/// the default page size.
async fn all_exclusions_paged(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<usize>,
) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(size) = max_results {
            request["maxResults"] = json!(size);
        }
        if let Some(ref next) = token {
            request["nextToken"] = json!(next);
        }
        let body = ok(router, case, "ListPullTimeUpdateExclusions", &request).await;
        let page = body["pullTimeUpdateExclusions"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a listing always carries pullTimeUpdateExclusions: {body:?}")
            })
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: an exclusion is a principal ARN string: {entry:?}")
                    })
                    .to_string()
            })
            .collect::<Vec<String>>();
        if let Some(size) = max_results {
            assert!(
                page.len() <= size,
                "case {case:?}: a page may hold at most {size} entries, got {}",
                page.len()
            );
        }
        all.extend(page);
        token = body["nextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            return all;
        }
        assert!(
            all.len() <= 10_000,
            "case {case:?}: pagination did not terminate ({} entries so far)",
            all.len()
        );
    }
}

/// Principal ARNs spanning what `PrincipalArn` accepts: both principal kinds, a role
/// path, three partitions, several accounts, every character the name alphabet
/// `[\w+=,.@-]` allows, the shortest name and an ARN of exactly the modelled length
/// ceiling. Source: the member's pattern and length from
/// `smithy_query(query="shape", shape="PrincipalArn")`.
fn principal_arn_rows() -> Vec<(&'static str, String)> {
    let prefix = "arn:aws:iam::000000000000:user/";
    let at_ceiling = format!(
        "{prefix}{}",
        "n".repeat(MAX_PRINCIPAL_ARN_LEN - prefix.len())
    );
    assert_eq!(
        at_ceiling.len(),
        MAX_PRINCIPAL_ARN_LEN,
        "the ceiling row is exactly {MAX_PRINCIPAL_ARN_LEN} characters"
    );
    vec![
        (
            "a user in this registry's account",
            "arn:aws:iam::000000000000:user/alice".to_string(),
        ),
        (
            "a role in this registry's account",
            "arn:aws:iam::000000000000:role/ci-build".to_string(),
        ),
        (
            "a role carrying a path",
            "arn:aws:iam::000000000000:role/team/platform/deployer".to_string(),
        ),
        (
            "a principal of another account",
            "arn:aws:iam::123456789012:role/cross-account-puller".to_string(),
        ),
        (
            "the aws-cn partition",
            "arn:aws-cn:iam::210987654321:role/cn-north-1-puller".to_string(),
        ),
        (
            "the aws-us-gov partition",
            "arn:aws-us-gov:iam::999988887777:user/gov.operator".to_string(),
        ),
        (
            "every punctuation character the name alphabet allows",
            "arn:aws:iam::000000000000:user/odd+name=1,2.3@x-y_z".to_string(),
        ),
        (
            "a one-character principal name",
            "arn:aws:iam::000000000000:role/a".to_string(),
        ),
        ("a principal ARN at the length ceiling", at_ceiling),
    ]
}

/// A generated principal ARN, for the cases that need more exclusions than a table.
fn generated_arn(index: usize) -> String {
    format!("arn:aws:iam::000000000000:role/puller-{index:05}")
}

#[tokio::test]
async fn deregistering_removes_exactly_the_named_principal() {
    let router = emulator();
    let rows = principal_arn_rows();

    // Every row is registered first, so each deregistration below has to pick its own
    // record out of a list holding all the others.
    let mut remaining: Vec<String> = Vec::new();
    for (case, arn) in &rows {
        ok(
            &router,
            case,
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        remaining.push(arn.clone());
    }
    remaining.sort();
    assert_eq!(
        all_exclusions(&router, "every row registered").await,
        remaining,
        "the listing reports every registered principal before any is removed"
    );

    for (case, arn) in &rows {
        let removed = ok(
            &router,
            case,
            "DeregisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        assert_eq!(
            removed["principalArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the deregister reports the principal it removed, got {:?}",
            removed["principalArn"]
        );

        remaining.retain(|held| held != arn);
        assert_eq!(
            all_exclusions(&router, case).await,
            remaining,
            "case {case:?}: only {arn} is gone and every other exclusion is intact"
        );
    }

    assert_eq!(
        all_exclusions(&router, "every row removed").await,
        Vec::<String>::new(),
        "the list is empty once every registered principal is deregistered"
    );
}

#[tokio::test]
async fn deregistering_a_principal_the_registry_does_not_exclude_is_refused() {
    let router = emulator();
    let rows = principal_arn_rows();
    // A neighbour the registry does exclude throughout, so a refused deregister is
    // visibly a refusal to touch the list rather than an empty-list answer.
    let neighbour = generated_arn(0);
    ok(
        &router,
        "a neighbouring exclusion",
        "RegisterPullTimeUpdateExclusion",
        &json!({"principalArn": neighbour}),
    )
    .await;

    for (case, arn) in &rows {
        // Never registered.
        let absent = call(
            &router,
            "DeregisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        assert_eq!(
            absent.status, 400,
            "case {case:?}: expected HTTP 400 for a principal that is not excluded, got {} \
             with {:?}",
            absent.status, absent.body
        );
        assert_eq!(
            absent.error_code(),
            "ExclusionNotFoundException",
            "case {case:?}: expected ExclusionNotFoundException, got {:?}",
            absent.body
        );

        // Registered, removed, then removed again: the second removal is refused too,
        // so a deregister is not idempotent-by-silence.
        ok(
            &router,
            case,
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        ok(
            &router,
            case,
            "DeregisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        let again = call(
            &router,
            "DeregisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        assert_eq!(
            again.status, 400,
            "case {case:?}: expected HTTP 400 on the second deregister, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ExclusionNotFoundException",
            "case {case:?}: expected ExclusionNotFoundException on the second deregister, \
             got {:?}",
            again.body
        );

        // And nothing else was removed on the way.
        assert_eq!(
            all_exclusions(&router, case).await,
            vec![neighbour.clone()],
            "case {case:?}: a refused deregister leaves the rest of the list alone"
        );
    }
}

#[tokio::test]
async fn a_deregistered_principal_can_be_registered_again() {
    let router = emulator();

    for (case, arn) in principal_arn_rows() {
        let first = ok(
            &router,
            case,
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        let first_created_at = first["createdAt"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: a register answers the instant its record was created, got {:?}",
                first["createdAt"]
            )
        });

        ok(
            &router,
            case,
            "DeregisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        assert_eq!(
            all_exclusions(&router, case).await,
            Vec::<String>::new(),
            "case {case:?}: the removal is observable before the principal is re-registered"
        );

        // The ARN is free again, so registering it is a create and not the duplicate
        // refusal a still-held record would get.
        let second = ok(
            &router,
            case,
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        let second_created_at = second["createdAt"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: the re-register answers a createdAt too, got {:?}",
                second["createdAt"]
            )
        });
        assert!(
            second_created_at >= first_created_at,
            "case {case:?}: the new record was created no earlier than the removed one, \
             got {second_created_at} after {first_created_at}"
        );
        assert_eq!(
            all_exclusions(&router, case).await,
            vec![arn.clone()],
            "case {case:?}: the re-registered principal is on the list exactly once"
        );

        // Leave the registry empty for the next row.
        ok(
            &router,
            case,
            "DeregisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
    }
}

#[tokio::test]
async fn removals_from_a_multi_page_list_leave_every_remaining_page_walkable() {
    let router = emulator();
    // The registry holds at most as many exclusions as `PullTimeUpdateExclusionList`'s
    // modelled `@length(max: 100)` admits, which is also the default page size, so the
    // walks below ask for a smaller page: with this many entries the walk crosses a page
    // boundary several times, both before and after the removals.
    let count = 97;
    let page_size = 7;
    let mut remaining: Vec<String> = Vec::new();
    for index in 0..count {
        let arn = generated_arn(index);
        ok(
            &router,
            "seeding the exclusion list",
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        remaining.push(arn);
    }
    remaining.sort();

    // Removals scattered through the order, including the first and the last entry, so
    // a page boundary that resolved against a removed key would show up here.
    let removed: Vec<String> = (0..count)
        .filter(|index| index % 3 == 0 || *index == count - 1)
        .map(generated_arn)
        .collect();
    for arn in &removed {
        let answer = ok(
            &router,
            "a scattered removal",
            "DeregisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        assert_eq!(
            answer["principalArn"].as_str(),
            Some(arn.as_str()),
            "case {arn:?}: the deregister reports the principal it removed, got {answer:?}"
        );
    }
    remaining.retain(|held| !removed.contains(held));

    assert_eq!(
        all_exclusions_paged(&router, "after the scattered removals", Some(page_size)).await,
        remaining,
        "every remaining exclusion is still walkable, in principal-ARN order, with \
         {} removed of {count}",
        removed.len()
    );
}
