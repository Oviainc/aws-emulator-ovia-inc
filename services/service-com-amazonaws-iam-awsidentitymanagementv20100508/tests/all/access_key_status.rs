// SPDX-License-Identifier: CC0-1.0

//! `UpdateAccessKey`, end to end over the awsQuery wire.
//!
//! RULE: `UpdateAccessKey` assigns the requested `statusType` value to the one
//! stored access key the request names — the key of the user `UserName` names, or,
//! with no `UserName`, the key of the user the signing access key id resolves to —
//! moving no other member of that key and no other key, so a later read reports the
//! stored status; an id that user holds none of (never minted, or held by another
//! user) and a user the account does not hold are both `NoSuchEntity` and write
//! nothing.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! status value assigned (the whole `statusType` value set, plus a repeat for
//! idempotence and a return to the value the mint left), the owning user's name
//! alphabet and case, the case the request spells that name in, which of the user's
//! keys is updated, whether the request names a user at all, and which credentials
//! the request is presented as signed by.
//!
//! The awsQuery harness (router, call, element) is the one
//! `tests/all/user_lifecycle.rs` owns, so every suite in this crate builds and reads
//! a request the same way; the mint and the listing this suite reads through are
//! exercised by `tests/all/access_keys.rs` and `tests/all/access_key_listing.rs`.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::STATUS_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// How many access keys one IAM user may hold, from AWS's "IAM and STS quotas" page
/// ("Access keys per IAM user: 2") — how many keys a row can mint to prove that
/// updating one moves no other.
///
/// Restated as the published number rather than imported from the crate's own
/// `state::MAX_ACCESS_KEYS_PER_USER`, which would make the check follow the code
/// wherever it went.
const MAX_ACCESS_KEYS_PER_USER: usize = 2;

/// The status a key carries from the moment it is minted: `CreateAccessKey`
/// documents "the default status for new keys is Active".
const ACTIVE: &str = "Active";

/// One row: the user the account holds, and the case the request spells that user's
/// name in.
///
/// The two differ on most rows because IAM entity names "are not distinguished by
/// case" (`CreateUser$UserName`), so a request may name the owning user in any case.
struct Case {
    /// The name the user is created with — the case IAM stores and reports.
    stored_name: &'static str,
    /// The name `UpdateAccessKey` spells; the same entity, differing only in case.
    requested_name: &'static str,
}

/// Rows spanning the whole `^[\w+=,.@-]+$` alphabet the name constraint admits
/// (`smithy_query(query="operation", operation="UpdateAccessKey")`), both cases of
/// the stored name, and requests that fold the name up, down and not at all.
const CASES: &[Case] = &[
    Case {
        stored_name: "alice",
        requested_name: "alice",
    },
    Case {
        stored_name: "Bob",
        requested_name: "bob",
    },
    Case {
        stored_name: "carol_7",
        requested_name: "CAROL_7",
    },
    Case {
        stored_name: "dave.x",
        requested_name: "dave.X",
    },
    Case {
        stored_name: "Eve+1",
        requested_name: "eve+1",
    },
    Case {
        stored_name: "frank=2",
        requested_name: "frank=2",
    },
    Case {
        stored_name: "grace@3",
        requested_name: "GRACE@3",
    },
    Case {
        stored_name: "heidi-4",
        requested_name: "heidi-4",
    },
];

/// Create `user` and mint `count` access keys for it, answering their ids.
async fn user_with_keys(
    router: &MultiServiceRouter,
    case: &str,
    user: &str,
    count: usize,
) -> Vec<String> {
    assert!(
        count <= MAX_ACCESS_KEYS_PER_USER,
        "case {case:?}: a row cannot ask for more keys than the published quota, got {count}"
    );
    let (status, body) = call(router, "CreateUser", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateUser must succeed, got {status}: {body}"
    );
    let mut ids = Vec::new();
    for index in 0..count {
        let (status, created) = call(router, "CreateAccessKey", &[("UserName", user)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: key {index} must be minted, got {status}: {created}"
        );
        assert_eq!(
            element(&created, "Status"),
            ACTIVE,
            "case {case:?}: expected a new key to be Active, got {:?}",
            element(&created, "Status")
        );
        ids.push(element(&created, "AccessKeyId"));
    }
    ids
}

/// One `ListAccessKeys` call for a user, asserted to succeed — the read that reports
/// what the update persisted.
async fn list_keys(router: &MultiServiceRouter, case: &str, user: &str) -> String {
    let (status, body) = call(router, "ListAccessKeys", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the listing to succeed, got {status}: {body}"
    );
    body
}

/// The member `member` the listing reports for the key `id`.
///
/// Every entry in a listing carries each of these members exactly once, in document
/// order, so the member at the id's position is that key's.
fn member_of(body: &str, id: &str, member: &str) -> String {
    let ids = elements(body, "AccessKeyId");
    let values = elements(body, member);
    assert_eq!(
        ids.len(),
        values.len(),
        "expected one {member} per key, got {} ids and {} values: {body}",
        ids.len(),
        values.len()
    );
    let at = ids
        .iter()
        .position(|listed| listed == id)
        .unwrap_or_else(|| panic!("expected the listing to report {id:?}, got {ids:?}: {body}"));
    values[at].clone()
}

/// One `UpdateAccessKey` call naming a user.
async fn update_status(
    router: &MultiServiceRouter,
    user: &str,
    access_key_id: &str,
    status: &str,
) -> (u16, String) {
    call(
        router,
        "UpdateAccessKey",
        &[
            ("UserName", user),
            ("AccessKeyId", access_key_id),
            ("Status", status),
        ],
    )
    .await
}

/// RULE: the update assigns the requested `statusType` value to the stored key the
/// id names, is idempotent in that value, and moves no other member of that key —
/// so a later read reports the stored status rather than one recomputed from the
/// reading request.
#[tokio::test]
async fn update_assigns_each_modelled_status_and_moves_nothing_else() {
    // The catalog this test spans, taken from the service's own record of
    // `statusType`'s value set (`smithy_query(query="shape", shape="statusType")`),
    // so the test cannot drift from the model.
    assert_eq!(
        STATUS_TYPE_VALUES,
        ["Active", "Expired", "Inactive"],
        "statusType's value set moved, so this test no longer spans it: got {STATUS_TYPE_VALUES:?}"
    );
    // Rows: every value the enum declares, a REPEAT of one (the assignment is
    // idempotent), and a return to the value the mint left the key at — the
    // documented "from Active to Inactive, or vice versa" direction.
    let assignments = ["Inactive", "Inactive", "Expired", "Active"];
    for value in STATUS_TYPE_VALUES {
        assert!(
            assignments.contains(value),
            "expected the assignments to span statusType, {value:?} is missing"
        );
    }

    let router = router();
    for Case {
        stored_name,
        requested_name,
    } in CASES
    {
        let case = format!("{stored_name} requested as {requested_name}");
        let held = user_with_keys(&router, &case, stored_name, MAX_ACCESS_KEYS_PER_USER).await;
        // What the mint persisted, which only `Status` may move from here on.
        let listed = list_keys(&router, &case, stored_name).await;
        let minted: Vec<(String, String)> = held
            .iter()
            .map(|id| (id.clone(), member_of(&listed, id, "CreateDate")))
            .collect();

        // Each key of the user in turn, so a row covers the first key of a full set
        // and the last one, and the untouched key is checked both ways round.
        for (index, updated) in held.iter().enumerate() {
            for assigned in assignments {
                let (status, body) =
                    update_status(&router, requested_name, updated, assigned).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected key {index} Status={assigned} to be accepted, \
                     got {status}: {body}"
                );
                // The output shape is `Unit`: the call reports no key and no secret.
                for member in ["Status", "SecretAccessKey"] {
                    assert!(
                        elements(&body, member).is_empty(),
                        "case {case:?}: expected the update to report no {member}, got {body}"
                    );
                }

                let listed = list_keys(&router, &case, stored_name).await;
                assert_eq!(
                    member_of(&listed, updated, "Status"),
                    assigned,
                    "case {case:?}: expected key {index} to be stored as {assigned}, got {:?}",
                    member_of(&listed, updated, "Status")
                );
                for (id, create_date) in &minted {
                    assert_eq!(
                        &member_of(&listed, id, "CreateDate"),
                        create_date,
                        "case {case:?}: Status={assigned} moved the CreateDate of {id:?}, which \
                         the update must leave as the account holds it; got {:?}",
                        member_of(&listed, id, "CreateDate")
                    );
                    assert_eq!(
                        member_of(&listed, id, "UserName"),
                        *stored_name,
                        "case {case:?}: Status={assigned} moved the owner of {id:?}, got {:?}",
                        member_of(&listed, id, "UserName")
                    );
                    // Every OTHER key of the same user is still as it was minted:
                    // the update reaches exactly the key its id names.
                    if id != updated {
                        assert_eq!(
                            member_of(&listed, id, "Status"),
                            ACTIVE,
                            "case {case:?}: Status={assigned} on key {index} moved {id:?} too, \
                             got {:?}",
                            member_of(&listed, id, "Status")
                        );
                    }
                }
                // The secret "is returned only in response to CreateAccessKey", so
                // neither the update nor the read that follows may carry one.
                assert!(
                    elements(&listed, "SecretAccessKey").is_empty(),
                    "case {case:?}: expected no secret in a listing, got {listed}"
                );
            }
            // The key is returned to the status the mint left it at before the next
            // key is exercised, so the untouched-key check above starts from `Active`
            // whichever key is updated next.
            let (status, body) = update_status(&router, requested_name, updated, ACTIVE).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected key {index} to be returned to {ACTIVE}, got {status}: {body}"
            );
        }
    }
}

/// RULE: an access key is reachable only through the user that holds it, so an id
/// that user holds none of — never minted, or held by another user — and a user the
/// account does not hold are the same answer: `NoSuchEntity` (HTTP 404), with no
/// stored status moved.
#[tokio::test]
async fn update_reaches_only_a_key_the_named_user_holds() {
    let router = router();

    for Case {
        stored_name,
        requested_name,
    } in CASES
    {
        let case = format!("{stored_name} requested as {requested_name}");
        let own = user_with_keys(&router, &case, stored_name, 1).await;
        // A second user, so a row covers an id that exists but belongs elsewhere.
        let other_name = format!("other_{stored_name}");
        let other = user_with_keys(&router, &case, &other_name, 1).await;

        // Rows: an id of the modelled shape this emulator never issued, an id at
        // each bound `accessKeyIdType` admits (length 16 and 128), and an id another
        // user holds.
        let unheld: Vec<String> = vec![
            "AKIAIOSFODNN7EXAMPLE".to_string(),
            "A".repeat(16),
            "B".repeat(128),
            other[0].clone(),
        ];
        for access_key_id in &unheld {
            let (status, refused) =
                update_status(&router, requested_name, access_key_id, "Inactive").await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for {access_key_id:?}, got {status}: {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity for {access_key_id:?}, got {:?}",
                error_code(&refused)
            );
        }

        // Rows: a user the account does not hold, named with an id that does exist.
        let absent = [
            format!("absent_{stored_name}"),
            "no.such@user".to_string(),
            "x".repeat(128),
        ];
        for name in &absent {
            let (status, refused) = update_status(&router, name, &own[0], "Inactive").await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for the absent user {name:?}, got {status}: {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity for the absent user {name:?}, got {:?}",
                error_code(&refused)
            );
        }

        // Nothing was written by any refusal: both keys are still as minted.
        for (user, id) in [(stored_name.to_string(), &own[0]), (other_name, &other[0])] {
            let listed = list_keys(&router, &case, &user).await;
            assert_eq!(
                member_of(&listed, id, "Status"),
                ACTIVE,
                "case {case:?}: a refused update moved the stored status of {id:?}, got {:?}",
                member_of(&listed, id, "Status")
            );
        }
    }
}

/// RULE: with no `UserName` the key updated is the one held by the user the signing
/// access key id resolves to — including the signing key itself — and a key another
/// user holds is still out of that caller's reach; credentials that belong to no IAM
/// user resolve to no user, so such a request is refused rather than having an
/// identity invented for it and writes nothing.
#[tokio::test]
async fn an_omitted_name_updates_the_signing_users_key() {
    let router = router();

    // Two users, each holding the quota, so the resolution is exercised on more than
    // one identity and the cross-user refusal has a second one to come from.
    let mut holders = Vec::new();
    for Case { stored_name, .. } in &CASES[..2] {
        let held =
            user_with_keys(&router, stored_name, stored_name, MAX_ACCESS_KEYS_PER_USER).await;
        holders.push((*stored_name, held));
    }

    for (user, held) in &holders {
        // The signing user's OTHER key, and then the signing key itself: both are
        // that user's to update with no name given.
        for (index, updated) in [&held[1], &held[0]].into_iter().enumerate() {
            let (status, body) = call_signed_by(
                &router,
                Some(&held[0]),
                "UpdateAccessKey",
                &[("AccessKeyId", updated), ("Status", "Inactive")],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {user:?}: expected the signing user's key {index} to be updated, \
                 got {status}: {body}"
            );
            let listed = list_keys(&router, user, user).await;
            assert_eq!(
                member_of(&listed, updated, "Status"),
                "Inactive",
                "case {user:?}: expected the nameless update to reach key {index}, got {:?}",
                member_of(&listed, updated, "Status")
            );
        }
    }

    // A nameless request signed by one holder cannot reach the other's key: the id
    // resolves against the signing user, so it is not found.
    let (status, refused) = call_signed_by(
        &router,
        Some(&holders[0].1[0]),
        "UpdateAccessKey",
        &[("AccessKeyId", &holders[1].1[0]), ("Status", ACTIVE)],
    )
    .await;
    assert_eq!(
        status, 404,
        "expected a nameless update to reach only the signing user's keys, got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "NoSuchEntity",
        "expected NoSuchEntity, got {:?}",
        error_code(&refused)
    );

    // Rows spanning no signing key at all, the fallback id an unsigned request
    // resolves to (`emulator_server::extract_akid` answers "default"), key ids of the
    // modelled shape this emulator never issued, and an id that names a user rather
    // than a key: none of them names an IAM user.
    let signers: &[Option<&str>] = &[
        None,
        Some("default"),
        Some("AKIAIOSFODNN7EXAMPLE"),
        Some("EMULATORAKAAAAAAAAAA"),
        Some(CASES[0].stored_name),
    ];
    for signer in signers {
        let (status, refused) = call_signed_by(
            &router,
            *signer,
            "UpdateAccessKey",
            &[("AccessKeyId", &holders[0].1[0]), ("Status", ACTIVE)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {signer:?}: expected a request with no caller identity to be refused, \
             got {status}: {refused}"
        );
        // The modelled behaviour under test is the REFUSAL: a 400 that writes
        // nothing. The code is the framework's single client-input refusal shape,
        // because `UpdateAccessKey` models no error for a request that named no
        // entity; only the status and the key's survival are claims about IAM.
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {signer:?}: expected the framework's client-input refusal code, got {:?}",
            error_code(&refused)
        );
    }

    // Every refusal above left the stored statuses where the accepted updates put
    // them.
    for (user, held) in &holders {
        let listed = list_keys(&router, user, user).await;
        for id in held {
            assert_eq!(
                member_of(&listed, id, "Status"),
                "Inactive",
                "case {user:?}: a refused update moved the stored status of {id:?}, got {:?}",
                member_of(&listed, id, "Status")
            );
        }
    }
}
