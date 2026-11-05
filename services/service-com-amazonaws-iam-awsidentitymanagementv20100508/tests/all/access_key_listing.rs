// SPDX-License-Identifier: CC0-1.0

//! `ListAccessKeys`, end to end over the awsQuery wire.
//!
//! RULE: `ListAccessKeys` reports, from persisted state, the metadata of every
//! access key held by the IAM user the request names (or, with no `UserName`, the
//! user the signing key id resolves to) — the stored user name, the key id, the
//! key's status and its creation instant, never the secret — in one total order
//! that `MaxItems` pages through and `Marker` resumes; a name the account does not
//! hold is `NoSuchEntity`, and a user holding none gets an empty list.
//!
//! The awsQuery harness (router, call, element) is the one
//! `tests/all/user_lifecycle.rs` owns, so every suite in this crate builds and
//! reads a request the same way.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over:
//! the owning user's name alphabet and case, the case the request spells it in,
//! how many keys the user holds (none, one, the quota), the page size, whether the
//! request names a user at all, and which credentials it is presented as signed
//! by.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// How many access keys one IAM user may hold, from AWS's "IAM and STS quotas"
/// page ("Access keys per IAM user: 2") — the ceiling on how many keys a row can
/// ask this listing to report.
///
/// Restated as the published number rather than imported from the crate's own
/// `state::MAX_ACCESS_KEYS_PER_USER`, which would make the check follow the code
/// wherever it went.
const MAX_ACCESS_KEYS_PER_USER: usize = 2;

/// The status a key carries from the moment it is minted: `CreateAccessKey`
/// documents "the default status for new keys is Active", one of the values
/// `statusType` declares (`smithy_query(query="shape", shape="statusType")`).
const ACTIVE: &str = "Active";

/// One row: the user the account holds, the case the request spells that user's
/// name in, and how many keys the user holds when the listing runs.
///
/// The two names differ on most rows because IAM entity names "are not
/// distinguished by case" (`CreateUser$UserName`), so a request may name the user
/// in any case while the response must report the name as the account holds it.
struct Case {
    /// The name the user is created with — the case IAM stores and reports.
    stored_name: &'static str,
    /// The name `ListAccessKeys` spells; the same entity, differing only in case.
    requested_name: &'static str,
    /// How many access keys to mint for the user before listing: zero, one and the
    /// published quota all appear among the rows.
    held: usize,
}

/// Rows spanning the whole `^[\w+=,.@-]+$` alphabet the name constraint admits
/// (`smithy_query(query="operation", operation="ListAccessKeys")`), both cases of
/// the stored name, requests that fold the name up, down and not at all, and every
/// key count from none to the published quota.
const CASES: &[Case] = &[
    Case {
        stored_name: "alice",
        requested_name: "alice",
        held: 0,
    },
    Case {
        stored_name: "Bob",
        requested_name: "bob",
        held: 1,
    },
    Case {
        stored_name: "carol_7",
        requested_name: "CAROL_7",
        held: 2,
    },
    Case {
        stored_name: "dave.x",
        requested_name: "dave.X",
        held: 1,
    },
    Case {
        stored_name: "Eve+1",
        requested_name: "eve+1",
        held: 2,
    },
    Case {
        stored_name: "frank=2",
        requested_name: "frank=2",
        held: 0,
    },
    Case {
        stored_name: "grace@3",
        requested_name: "GRACE@3",
        held: 2,
    },
    Case {
        stored_name: "heidi-4",
        requested_name: "heidi-4",
        held: 1,
    },
];

/// Create the user `stored_name` and mint `held` access keys for it, answering the
/// minted ids in the order the listing must report them (ascending by key id, the
/// one total order the store is keyed in).
async fn with_user_holding(
    router: &MultiServiceRouter,
    stored_name: &str,
    held: usize,
) -> Vec<String> {
    assert!(
        held <= MAX_ACCESS_KEYS_PER_USER,
        "case {stored_name:?}: a row cannot ask for more keys than the published quota, got {held}"
    );
    let (status, body) = call(router, "CreateUser", &[("UserName", stored_name)]).await;
    assert_eq!(
        status, 200,
        "case {stored_name:?}: CreateUser must succeed, got {status}: {body}"
    );
    let mut minted = Vec::new();
    for index in 0..held {
        let (status, created) = call(router, "CreateAccessKey", &[("UserName", stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {stored_name:?}: key {index} must be minted, got {status}: {created}"
        );
        minted.push(element(&created, "AccessKeyId"));
    }
    minted.sort();
    minted
}

/// `ListAccessKeys`' `IsTruncated` flag, which the model marks required with a
/// default of `false`, so the response always carries it.
fn is_truncated(case: &str, body: &str) -> bool {
    let flag = element(body, "IsTruncated");
    match flag.as_str() {
        "true" => true,
        "false" => false,
        other => panic!("case {case:?}: expected IsTruncated to be a boolean, got {other:?}"),
    }
}

/// RULE: the listing reports exactly the keys the named user holds, each with the
/// stored user name, its own id, its status and its creation instant, and never a
/// secret; a user holding none gets an empty list.
#[tokio::test]
async fn listing_reports_every_key_the_named_user_holds() {
    let router = router();

    for Case {
        stored_name,
        requested_name,
        held,
    } in CASES
    {
        let case = format!("{stored_name} holding {held}, requested as {requested_name}");
        let minted = with_user_holding(&router, stored_name, *held).await;

        let (status, listed) = call(&router, "ListAccessKeys", &[("UserName", requested_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {listed}"
        );
        assert_eq!(
            elements(&listed, "AccessKeyId"),
            minted,
            "case {case:?}: expected exactly the minted ids in key order, got {:?}",
            elements(&listed, "AccessKeyId")
        );
        assert_eq!(
            elements(&listed, "UserName"),
            vec![stored_name.to_string(); *held],
            "case {case:?}: expected the stored user name on every entry, got {:?}",
            elements(&listed, "UserName")
        );
        assert_eq!(
            elements(&listed, "Status"),
            vec![ACTIVE.to_string(); *held],
            "case {case:?}: expected every minted key to be Active, got {:?}",
            elements(&listed, "Status")
        );
        for create_date in elements(&listed, "CreateDate") {
            assert!(
                aws_smithy_types::DateTime::from_str(
                    &create_date,
                    aws_smithy_types::date_time::Format::DateTime,
                )
                .is_ok(),
                "case {case:?}: expected an ISO-8601 CreateDate, got {create_date:?}"
            );
        }
        assert!(
            !is_truncated(&case, &listed),
            "case {case:?}: a page holding every key is not truncated, got {listed}"
        );
        assert!(
            elements(&listed, "Marker").is_empty(),
            "case {case:?}: an untruncated page carries no Marker, got {listed}"
        );
        // The secret "is returned only in response to CreateAccessKey", so nothing
        // this listing reports may carry one.
        assert!(
            elements(&listed, "SecretAccessKey").is_empty(),
            "case {case:?}: expected no secret in a listing, got {listed}"
        );

        // A read cannot change what a later caller sees.
        let (status, again) = call(&router, "ListAccessKeys", &[("UserName", requested_name)]).await;
        assert_eq!(
            (status, elements(&again, "AccessKeyId")),
            (200, minted.clone()),
            "case {case:?}: a second listing answered differently: {again}"
        );
    }
}

/// RULE: the listing reports the keys of ONE user — the keys of every other user
/// the account holds are not among them.
#[tokio::test]
async fn listing_reports_only_the_named_users_own_keys() {
    let router = router();

    // Every row's user is created in the same account, so each listing below runs
    // against a store holding other users' keys too.
    let mut held: Vec<(&str, &str, Vec<String>)> = Vec::new();
    for Case {
        stored_name,
        requested_name,
        held: count,
    } in CASES
    {
        let minted = with_user_holding(&router, stored_name, *count).await;
        held.push((stored_name, requested_name, minted));
    }

    for (stored_name, requested_name, minted) in &held {
        let (status, listed) = call(&router, "ListAccessKeys", &[("UserName", requested_name)]).await;
        assert_eq!(
            status, 200,
            "case {stored_name:?}: expected 200, got {status}: {listed}"
        );
        assert_eq!(
            &elements(&listed, "AccessKeyId"),
            minted,
            "case {stored_name:?}: expected only this user's keys, got {:?}",
            elements(&listed, "AccessKeyId")
        );
        for (other_name, _, other_keys) in &held {
            if other_name == stored_name {
                continue;
            }
            for other_key in other_keys {
                assert!(
                    !elements(&listed, "AccessKeyId").contains(other_key),
                    "case {stored_name:?}: expected {other_name}'s key {other_key:?} to be absent, got {listed}"
                );
            }
        }
    }
}

/// RULE: `MaxItems` is a real page size over that one order, and a truncated page
/// hands out a `Marker` the next call resumes from, so walking the pages yields
/// every key of the user exactly once.
#[tokio::test]
async fn max_items_pages_through_every_key_exactly_once() {
    let router = router();

    for Case {
        stored_name,
        requested_name,
        ..
    } in CASES
    {
        // Paging is only observable on a user that holds more than one key, so
        // every row here holds the quota.
        let minted = with_user_holding(&router, stored_name, MAX_ACCESS_KEYS_PER_USER).await;

        // Page sizes spanning both published bounds of `maxItemsType` (range
        // 1..1000), a size equal to the number of keys, and one above it.
        for page_size in [1usize, 2, 3, 1000] {
            let case = format!("{stored_name} requested as {requested_name} with MaxItems={page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut marker: Option<String> = None;
            let size = page_size.to_string();
            loop {
                let mut params: Vec<(&str, &str)> =
                    vec![("UserName", requested_name), ("MaxItems", size.as_str())];
                if let Some(token) = marker.as_deref() {
                    params.push(("Marker", token));
                }
                let (status, listed) = call(&router, "ListAccessKeys", &params).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status}: {listed}"
                );
                let page = elements(&listed, "AccessKeyId");
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: expected at most {page_size} per page, got {}",
                    page.len()
                );
                seen.extend(page);
                if is_truncated(&case, &listed) {
                    marker = Some(element(&listed, "Marker"));
                } else {
                    assert!(
                        elements(&listed, "Marker").is_empty(),
                        "case {case:?}: a final page carries no Marker, got {listed}"
                    );
                    break;
                }
            }
            assert_eq!(
                seen, minted,
                "case {case:?}: expected every key exactly once in key order, got {seen:?}"
            );
        }
    }
}

/// RULE: `MaxItems` outside its published range is refused before the operation
/// runs, so a page size the model forbids never reaches the store.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = router();
    with_user_holding(&router, "sized", MAX_ACCESS_KEYS_PER_USER).await;

    // One past each published bound of `maxItemsType` (range 1..1000).
    for out_of_range in ["0", "1001", "-1"] {
        let (status, body) = call(
            &router,
            "ListAccessKeys",
            &[("UserName", "sized"), ("MaxItems", out_of_range)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400, got {status}: {body}"
        );
    }
    // And each bound itself is accepted.
    for in_range in ["1", "1000"] {
        let (status, body) = call(
            &router,
            "ListAccessKeys",
            &[("UserName", "sized"), ("MaxItems", in_range)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected 200, got {status}: {body}"
        );
    }
}

/// RULE: with no `UserName` the user is determined from the access key id the
/// request was signed with, so the listing reports that user's own keys.
#[tokio::test]
async fn an_omitted_name_lists_the_signing_users_keys() {
    let router = router();

    for Case { stored_name, .. } in CASES {
        let minted = with_user_holding(&router, stored_name, MAX_ACCESS_KEYS_PER_USER).await;
        // IAM authorizes its own API, so the signing user needs the grant AWS requires
        // for this read before it can make it; what this suite is about is which user a
        // nameless request resolves to, not the decision
        // (`tests/all/identity_policy_enforcement.rs` owns that).
        let (status, put) = call(
            &router,
            "PutUserPolicy",
            &[
                ("UserName", stored_name),
                ("PolicyName", "allow-listaccesskeys"),
                (
                    "PolicyDocument",
                    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListAccessKeys","Resource":"*"}]}"#,
                ),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {stored_name:?}: the fixture PutUserPolicy must succeed, got {status}: {put}"
        );

        // Every key the user holds is a credential the request may be signed by, and
        // each resolves to the same user and so to the same listing.
        for signing_key in &minted {
            let case = format!("{stored_name} signed by {signing_key}");
            let (status, listed) =
                call_signed_by(&router, Some(signing_key), "ListAccessKeys", &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {listed}"
            );
            assert_eq!(
                elements(&listed, "AccessKeyId"),
                minted,
                "case {case:?}: expected the signing user's keys, got {:?}",
                elements(&listed, "AccessKeyId")
            );
            assert_eq!(
                elements(&listed, "UserName"),
                vec![stored_name.to_string(); minted.len()],
                "case {case:?}: expected the stored user name, got {:?}",
                elements(&listed, "UserName")
            );
        }
    }
}

/// RULE: credentials that belong to no IAM user resolve to no user, so a request
/// that names none is refused rather than having an identity invented for it.
#[tokio::test]
async fn an_omitted_name_refuses_non_user_credentials() {
    let router = router();
    with_user_holding(&router, "keyholder", 1).await;

    // Rows spanning no signing key at all, the fallback id an unsigned request
    // resolves to, a key id of the modelled shape this emulator never issued, and
    // an id that names a user rather than a key.
    let signers: &[Option<&str>] = &[
        None,
        Some("default"),
        Some("AKIAIOSFODNN7EXAMPLE"),
        Some("EMULATORAKAAAAAAAAAA"),
        Some("keyholder"),
    ];

    for signer in signers {
        let case = format!("signed by {signer:?}");
        let (status, refused) = call_signed_by(&router, *signer, "ListAccessKeys", &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {refused}"
        );
        // The behaviour under test is the REFUSAL: a 400 reporting no key, under the
        // error code IAM answers a request it cannot accept with, because
        // `ListAccessKeys` models no error for a request that named no entity.
        assert_eq!(
            error_code(&refused),
            "ValidationError",
            "case {case:?}: expected IAM's validation refusal code, got {:?}",
            error_code(&refused)
        );
        assert!(
            elements(&refused, "AccessKeyId").is_empty(),
            "case {case:?}: expected the refusal to report no key, got {refused}"
        );
    }
}

/// RULE: keys are reported for a user the account holds, so a name it does not
/// hold names no user to report on and is not found.
#[tokio::test]
async fn listing_an_absent_user_is_not_found() {
    let router = router();
    with_user_holding(&router, "present", 1).await;

    // Rows spanning a plain absent name, one differing from a held name by more
    // than case, names drawn from the rest of the accepted alphabet, and a name at
    // the 128-character bound `existingUserNameType` admits.
    let absent: Vec<String> = vec![
        "absent".to_string(),
        "present2".to_string(),
        "no.such@user".to_string(),
        "NOT_HELD+1".to_string(),
        "x".repeat(128),
    ];

    for name in &absent {
        let (status, refused) = call(&router, "ListAccessKeys", &[("UserName", name)]).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected 404, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {:?}",
            error_code(&refused)
        );
        assert!(
            elements(&refused, "AccessKeyId").is_empty(),
            "case {name:?}: expected the refusal to report no key, got {refused}"
        );
    }
}

/// RULE: a `Marker` this service did not issue names no position in the key order,
/// so the call is refused rather than silently answered from the beginning; a
/// token it DID issue names a POSITION, which stays usable even when the key it
/// was cut at is gone.
#[tokio::test]
async fn a_marker_this_service_did_not_issue_is_refused() {
    let router = router();
    let minted = with_user_holding(&router, "paged", MAX_ACCESS_KEYS_PER_USER).await;

    // Rows spanning text that is not a token at all, strings that are not valid
    // base64 and base64 of a payload this service's encoding never produced.
    for bogus in [
        "not-a-marker",
        "zz",
        "0g",
        "aWFtOnVzZXJzOnBhZ2Vk",
        "ZGVmaW5pdGVseS1mb3JlaWdu",
    ] {
        let (status, body) = call(
            &router,
            "ListAccessKeys",
            &[("UserName", "paged"), ("Marker", bogus)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {bogus:?}: expected the marker to be refused as invalid, got {:?}",
            error_code(&body)
        );
    }

    // A marker this service issued while paging ANOTHER collection names no
    // position among the access keys either. Paging the user collection needs more
    // than one user in it, so a second user is created first.
    with_user_holding(&router, "other", 0).await;
    let (status, users_page) = call(&router, "ListUsers", &[("MaxItems", "1")]).await;
    assert_eq!(
        status, 200,
        "the user listing must page, got {status}: {users_page}"
    );
    let foreign = element(&users_page, "Marker");
    let (status, body) = call(
        &router,
        "ListAccessKeys",
        &[("UserName", "paged"), ("Marker", foreign.as_str())],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {foreign:?}: expected another collection's marker to be refused, got {status}: {body}"
    );

    // A marker cut at a key that is then deleted still resumes from that position:
    // the rest of the order follows it, minus the deleted key.
    let (status, first_page) = call(
        &router,
        "ListAccessKeys",
        &[("UserName", "paged"), ("MaxItems", "1")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the first page, got {status}: {first_page}"
    );
    let marker = element(&first_page, "Marker");
    let cut_at = element(&first_page, "AccessKeyId");
    let (status, deleted) = call(
        &router,
        "DeleteAccessKey",
        &[("UserName", "paged"), ("AccessKeyId", cut_at.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the key the page was cut at to delete, got {status}: {deleted}"
    );

    let (status, resumed) = call(
        &router,
        "ListAccessKeys",
        &[("UserName", "paged"), ("Marker", marker.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected a stale marker to still resume, got {status}: {resumed}"
    );
    let expected: Vec<String> = minted.iter().filter(|id| **id != cut_at).cloned().collect();
    assert_eq!(
        elements(&resumed, "AccessKeyId"),
        expected,
        "expected the order after the marker's position, got {:?}",
        elements(&resumed, "AccessKeyId")
    );
}

/// RULE: the listing is computed from persisted state, so a key the account no
/// longer holds is no longer among the keys reported for its user.
#[tokio::test]
async fn a_deleted_key_leaves_the_listing() {
    let router = router();

    for Case {
        stored_name,
        requested_name,
        ..
    } in CASES
    {
        let mut remaining = with_user_holding(&router, stored_name, MAX_ACCESS_KEYS_PER_USER).await;

        // Delete each key in turn, so the row covers the first key of a full set and
        // the last key the user holds.
        while let Some(doomed) = remaining.first().cloned() {
            let case = format!("{stored_name} dropping {doomed}");
            let (status, deleted) = call(
                &router,
                "DeleteAccessKey",
                &[("UserName", requested_name), ("AccessKeyId", doomed.as_str())],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the key to delete, got {status}: {deleted}"
            );
            remaining.remove(0);

            let (status, listed) =
                call(&router, "ListAccessKeys", &[("UserName", requested_name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {listed}"
            );
            assert_eq!(
                elements(&listed, "AccessKeyId"),
                remaining,
                "case {case:?}: expected only the keys still held, got {:?}",
                elements(&listed, "AccessKeyId")
            );
        }
    }
}
