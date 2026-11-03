// SPDX-License-Identifier: CC0-1.0

//! Listing the administrators of an `AppInstance`, end to end over HTTP.
//!
//! The RULE pinned here: `ListAppInstanceAdmins` reports the promotions this service
//! RECORDED for the one `AppInstance` it was asked about — each administering identity's
//! ARN and the name that identity carries — in a stable key order, paged at `MaxResults`
//! with a `NextToken` that resumes the walk, and nothing else: not another instance's
//! administrators, not an identity a refused promotion never recorded, and not a
//! promotion a demotion removed.
//!
//! The tables span what the rule generalises over: the kind of identity promoted (a
//! stored `AppInstanceUser`, a stored `AppInstanceBot`, an identity this service does not
//! hold), the number of administrators against the page size (both ends of the model's
//! `MaxResults` range), how many instances hold administrators at once, and each way a
//! page request can be one the listing cannot honour.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::app_instance_bots::bot;
use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// `POST /app-instances/{AppInstanceArn}/admins`.
async fn promote(emulator: &Emulator, app_instance_arn: &str, admin_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/app-instances/{}/admins", uri_encode(app_instance_arn)),
            Some(json!({ "AppInstanceAdminArn": admin_arn })),
        )
        .await
}

/// `GET /app-instances/{AppInstanceArn}/admins`, plus any extra query parameters.
async fn list_admins_raw(
    emulator: &Emulator,
    app_instance_arn: &str,
    query: &str,
) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!(
                "/app-instances/{}/admins{query}",
                uri_encode(app_instance_arn)
            ),
            None,
        )
        .await
}

/// The whole `AppInstanceAdmins` list of one instance as `Arn -> Name`, failing the test
/// if the listing was refused or did not echo the instance it was asked about.
async fn list_admins(
    emulator: &Emulator,
    app_instance_arn: &str,
) -> BTreeMap<String, Option<String>> {
    let (status, body) = list_admins_raw(emulator, app_instance_arn, "").await;
    assert_eq!(
        status, 200,
        "expected ListAppInstanceAdmins to answer 200, got {status} {body:?}"
    );
    assert_eq!(
        body["AppInstanceArn"].as_str(),
        Some(app_instance_arn),
        "expected the listing to echo the AppInstanceArn it was asked about, got {body:?}"
    );
    body["AppInstanceAdmins"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|entry| {
                    (
                        entry["Admin"]["Arn"]
                            .as_str()
                            .unwrap_or_else(|| panic!("an admin summary with no Arn: {entry:?}"))
                            .to_string(),
                        entry["Admin"].get("Name").and_then(Value::as_str).map(str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the promotion table: what is promoted, and the name the listing must
/// report for it.
#[derive(Debug)]
struct AdminCase {
    label: &'static str,
    /// Which identity of the app instance is promoted, built from the instance ARN and
    /// the identities the test created under it.
    admin_arn: fn(&str, &str, &str) -> String,
    /// The `Identity$Name` the listing must carry for the promoted identity, or `None`
    /// for an identity this service holds no record of.
    expected_name: Option<&'static str>,
}

/// RULE: every recorded promotion of the named `AppInstance` is reported with the
/// administering identity's ARN and the name that identity carries — whichever kind of
/// identity it is.
#[tokio::test]
async fn listing_reports_every_recorded_promotion() {
    let cases = [
        AdminCase {
            label: "a stored AppInstanceUser",
            admin_arn: |_instance, user, _bot| user.to_string(),
            expected_name: Some("alice"),
        },
        AdminCase {
            label: "a second stored AppInstanceUser",
            admin_arn: |instance, _user, _bot| format!("{instance}/user/bob"),
            expected_name: Some("bob"),
        },
        AdminCase {
            label: "a stored AppInstanceBot",
            admin_arn: |_instance, _user, bot| bot.to_string(),
            expected_name: Some("greeter"),
        },
        AdminCase {
            label: "an AppInstanceBot this service holds no record of",
            admin_arn: |instance, _user, _bot| format!("{instance}/bot/unstored"),
            expected_name: None,
        },
    ];

    let emulator = Emulator::new();
    let instance = emulator.app_instance("admin-listing", "tok-al").await;
    let alice = emulator.user(&instance, "alice", "tok-al-alice").await;
    let _bob = emulator.user(&instance, "bob", "tok-al-bob").await;
    let greeter = bot(&emulator, &instance, "tok-al-bot", "greeter").await;

    // An instance with no promotion at all reports an empty list, not another
    // instance's administrators.
    let empty = list_admins(&emulator, &instance).await;
    assert!(
        empty.is_empty(),
        "case {:?}: expected no administrators before any promotion, got {empty:?}",
        "an AppInstance nobody administers"
    );

    let mut expected: BTreeMap<String, Option<String>> = BTreeMap::new();
    for case in &cases {
        let admin_arn = (case.admin_arn)(&instance, &alice, &greeter);
        let (status, body) = promote(&emulator, &instance, &admin_arn).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected CreateAppInstanceAdmin to answer 201, got {status} {body:?}",
            case.label
        );
        expected.insert(admin_arn, case.expected_name.map(str::to_string));

        // Every promotion so far is reported together, so the listing cannot be
        // answering with only the last one written.
        let listed = list_admins(&emulator, &instance).await;
        assert_eq!(
            listed, expected,
            "case {:?}: expected the listing to report {expected:?}, got {listed:?}",
            case.label
        );
    }

    // A read leaves the listing exactly as it was: repeating it reports the same set.
    let again = list_admins(&emulator, &instance).await;
    assert_eq!(
        again, expected,
        "case {:?}: expected a repeated listing to report the same administrators, got {again:?}",
        "the listing read twice"
    );

    // The listing is scoped to ONE instance: a sibling instance with its own
    // administrator never reports this one's, and vice versa.
    let sibling = emulator.app_instance("admin-listing-2", "tok-al2").await;
    let carol = emulator.user(&sibling, "carol", "tok-al2-carol").await;
    let (status, body) = promote(&emulator, &sibling, &carol).await;
    assert_eq!(
        status, 201,
        "expected the sibling instance's promotion to answer 201, got {status} {body:?}"
    );
    let sibling_listed = list_admins(&emulator, &sibling).await;
    assert_eq!(
        sibling_listed.keys().cloned().collect::<Vec<String>>(),
        vec![carol.clone()],
        "case {:?}: expected only the sibling's own administrator, got {sibling_listed:?}",
        "two instances holding administrators at once"
    );
    let listed = list_admins(&emulator, &instance).await;
    assert_eq!(
        listed, expected,
        "case {:?}: expected the first instance's administrators unchanged, got {listed:?}",
        "two instances holding administrators at once"
    );
}

/// One row of the refusal table: the identity a promotion names that this service
/// cannot promote.
#[derive(Debug)]
struct RefusalCase {
    label: &'static str,
    /// The admin ARN the refused request carries, built from the app instance's ARN.
    admin_arn: fn(&str) -> String,
}

/// RULE: a promotion the service REFUSED is recorded nowhere, so the listing never
/// reports the identity it named.
#[tokio::test]
async fn a_refused_promotion_is_never_listed() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("admin-refusals", "tok-ar").await;
    let kept = emulator.user(&instance, "kept", "tok-ar-kept").await;
    let (status, body) = promote(&emulator, &instance, &kept).await;
    assert_eq!(status, 201, "expected the kept promotion to answer 201, got {status} {body:?}");

    // Rows: each way a promotion request names something this service cannot promote.
    let refused = [
        RefusalCase {
            label: "an AppInstanceUser the service does not hold",
            admin_arn: |instance| format!("{instance}/user/nobody"),
        },
        RefusalCase {
            label: "an identity of another app instance",
            admin_arn: |_instance| {
                "arn:aws:chime:us-west-2:111122223333:app-instance/other/user/alice".to_string()
            },
        },
        RefusalCase {
            label: "an ARN that is not an identity of the app instance at all",
            admin_arn: |_instance| {
                "arn:aws:chime:us-west-2:111122223333:app-instance/other".to_string()
            },
        },
    ];
    for case in &refused {
        let label = case.label;
        let admin_arn = (case.admin_arn)(&instance);
        let (status, body) = promote(&emulator, &instance, &admin_arn).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected the promotion to be refused, got {status} {body:?}"
        );
        let listed = list_admins(&emulator, &instance).await;
        assert_eq!(
            listed.keys().cloned().collect::<Vec<String>>(),
            vec![kept.clone()],
            "case {label:?}: expected only the kept administrator listed, got {listed:?}"
        );
    }
}

/// One page of the admin listing: the ARNs it carried and the token that resumes after
/// it.
async fn page(
    emulator: &Emulator,
    app_instance_arn: &str,
    max_results: i32,
    next_token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut query = format!("?max-results={max_results}");
    if let Some(token) = next_token {
        query.push_str(&format!("&next-token={}", uri_encode(token)));
    }
    let (status, body) = list_admins_raw(emulator, app_instance_arn, &query).await;
    assert_eq!(
        status, 200,
        "expected a page of {max_results} to answer 200, got {status} {body:?}"
    );
    let arns = body["AppInstanceAdmins"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| entry["Admin"]["Arn"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    (arns, body["NextToken"].as_str().map(str::to_string))
}

/// RULE: a paginated listing returns at most `MaxResults` entries per page in a stable
/// key order, emits a `NextToken` only while entries remain, and resuming from that token
/// yields every remaining entry exactly once.
#[tokio::test]
async fn pagination_partitions_the_whole_admin_list() {
    // Rows vary the collection size against the page size, including a page size equal to
    // the collection (so the first page is also the last), a collection that does not
    // divide evenly by the page size, and both ends of the model's `MaxResults` range
    // (min 1, max 50).
    let cases: &[(&str, usize, i32)] = &[
        ("one administrator, page size 1 (the lower bound)", 1, 1),
        ("seven administrators, page size 1", 7, 1),
        ("five administrators, page size 2 (an uneven split)", 5, 2),
        ("three administrators, page size 3 (one full page)", 3, 3),
        ("four administrators, page size 50 (the upper bound)", 4, 50),
    ];

    let emulator = Emulator::new();
    for (label, count, max_results) in cases {
        let instance = emulator
            .app_instance(
                &format!("paged-{count}-{max_results}"),
                &format!("tok-paged-{count}-{max_results}"),
            )
            .await;
        let mut promoted: BTreeSet<String> = BTreeSet::new();
        for n in 0..*count {
            let user = emulator
                .user(
                    &instance,
                    &format!("u{n}"),
                    &format!("tok-paged-{count}-{max_results}-{n}"),
                )
                .await;
            let (status, body) = promote(&emulator, &instance, &user).await;
            assert_eq!(
                status, 201,
                "case {label:?}: expected the promotion of {user} to answer 201, got {status} {body:?}"
            );
            promoted.insert(user);
        }

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One page more than the collection can need, so a listing that never stops is a
        // failure rather than a hang.
        let page_budget = count / (*max_results as usize) + 2;
        for step in 0..=page_budget {
            let (items, next) = page(&emulator, &instance, *max_results, token.as_deref()).await;
            assert!(
                items.len() <= *max_results as usize,
                "case {label:?}: page {step} carried {} entries, past MaxResults {max_results}",
                items.len()
            );
            walked.extend(items);
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                step < page_budget,
                "case {label:?}: the listing still offered a NextToken after {step} pages, walked {walked:?}"
            );
        }

        assert_eq!(
            walked.len(),
            *count,
            "case {label:?}: expected {count} administrators across the pages, got {walked:?}"
        );
        assert_eq!(
            walked.iter().cloned().collect::<BTreeSet<String>>(),
            promoted,
            "case {label:?}: expected the pages to carry exactly the promoted identities, got {walked:?}"
        );
        let mut sorted = walked.clone();
        sorted.sort();
        assert_eq!(
            walked, sorted,
            "case {label:?}: expected the pages to walk the collection in key order, got {walked:?}"
        );
    }
}

/// RULE: a page request the listing cannot honour — a `MaxResults` outside the modelled
/// range, or a `NextToken` this listing never issued — is refused with a 4xx client error
/// rather than served, and a listing of an `AppInstance` that does not exist is refused
/// with the modelled `NotFound` code.
#[tokio::test]
async fn an_admin_listing_refuses_what_it_cannot_honour() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("admin-bounds", "tok-ab").await;
    let user = emulator.user(&instance, "first", "tok-ab-first").await;
    let (status, body) = promote(&emulator, &instance, &user).await;
    assert_eq!(status, 201, "expected the promotion to answer 201, got {status} {body:?}");

    // Source: `smithy_query(query="operation",operation="ListAppInstanceAdmins")` —
    // `MaxResults` is `range(min: 1, max: 50)`, so each published bound is served and one
    // past either is refused.
    let cases: &[(&str, i32, bool)] = &[
        ("the lower bound", 1, true),
        ("the upper bound", 50, true),
        ("one below the lower bound", 0, false),
        ("one past the upper bound", 51, false),
    ];
    for (label, max_results, served) in cases {
        let (status, body) =
            list_admins_raw(&emulator, &instance, &format!("?max-results={max_results}")).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {label:?}: MaxResults {max_results} is inside the modelled range, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: MaxResults {max_results} is outside the modelled range, got {status} {body:?}"
            );
        }
    }

    // A well-formed token issued by the SAME listing of ANOTHER instance: scoped to the
    // listing that issued it, so it cannot resume this one.
    let other = emulator.app_instance("admin-bounds-2", "tok-ab2").await;
    for n in 0..2 {
        let user = emulator
            .user(&other, &format!("o{n}"), &format!("tok-ab2-{n}"))
            .await;
        let (status, body) = promote(&emulator, &other, &user).await;
        assert_eq!(
            status, 201,
            "expected the other instance's promotion to answer 201, got {status} {body:?}"
        );
    }
    let (_items, foreign_token) = page(&emulator, &other, 1, None).await;
    let foreign_token = foreign_token.expect("the other instance's listing issued a NextToken");

    for (label, token) in [
        ("a token that is not hex at all", "not-a-token".to_string()),
        ("hex that decodes to something else", "6162636465".to_string()),
        (
            "a token issued by the same listing of another instance",
            foreign_token,
        ),
    ] {
        let (status, body) = list_admins_raw(
            &emulator,
            &instance,
            &format!("?next-token={}", uri_encode(&token)),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected a NextToken this listing never issued to be refused, got {status} {body:?}"
        );
    }

    // An AppInstance this service does not hold.
    let absent = format!("{instance}x");
    let (status, body) = list_admins_raw(&emulator, &absent, "").await;
    assert_eq!(
        status, 400,
        "expected listing the administrators of an absent AppInstance to be refused, got {status} {body:?}"
    );
    assert_eq!(
        body["Code"].as_str(),
        Some("NotFound"),
        "expected the absent-AppInstance refusal to carry the NotFound code, got {body:?}"
    );
}
