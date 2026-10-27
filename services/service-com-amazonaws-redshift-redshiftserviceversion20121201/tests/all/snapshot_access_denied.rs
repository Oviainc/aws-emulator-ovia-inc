// SPDX-License-Identifier: CC0-1.0

//! Refusing a revoke for an account that holds no restore grant, end to end over
//! the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the seed and helpers of
//! [`crate::snapshot_access`], so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one request: when the snapshot a
//! `RevokeSnapshotAccess` names RESOLVES but the account it names holds no
//! restore grant on it, the refusal is the authorization-specific
//! `AccessToSnapshotDeniedFault` (HTTP 400) — never a not-found, because the
//! snapshot was found — its message carries the snapshot identifier and the
//! account the request named, and the snapshot's `AccountsWithRestoreAccess`
//! set is left exactly as it was: no other account loses its grant and the
//! refused account is not added. It is asserted over a TABLE whose rows differ
//! along the grantee's form (an account identifier, the documented Amazon Web
//! Services Support alias), the way the request names the snapshot (identifier,
//! ARN, identifier with its cluster) and how many grants the snapshot already
//! holds (none, one, several).

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};
use crate::snapshot_access::{
    authorize, describe, granted_aliases, granted_ids, seeded_emulator, SNAPSHOT,
};

/// `RevokeSnapshotAccess` for `account`, naming the snapshot with `locator`.
async fn revoke(emulator: &Emulator, account: &str, locator: &[(&str, &str)]) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![("AccountWithRestoreAccess", account)];
    params.extend_from_slice(locator);
    emulator.call("RevokeSnapshotAccess", &params).await
}

/// Grant `account` restore access to [`SNAPSHOT`], asserting the grant landed.
async fn grant(emulator: &Emulator, account: &str) {
    let answer = authorize(emulator, account, &[("SnapshotIdentifier", SNAPSHOT)]).await;
    assert_eq!(
        answer.status, 200,
        "case seed grant {account}: expected 200, got {} with body {}",
        answer.status, answer.body
    );
}

/// One row: how the request names the snapshot, the grantee it names, and how
/// many accounts already hold a grant when the row runs.
struct DeniedCase<'a> {
    label: &'a str,
    account: &'a str,
    locator: Vec<(&'a str, &'a str)>,
    /// Accounts granted access before the row's revoke runs.
    seeded: Vec<&'a str>,
}

/// A revoke for an account holding no grant is refused with
/// `AccessToSnapshotDenied` (400), carries the snapshot and the account in its
/// message, and leaves the grant set exactly as it was — over every grantee
/// form, every snapshot locator and every size of the set.
#[tokio::test]
async fn a_revoke_for_an_ungranted_account_is_denied_and_changes_no_grant() {
    let (emulator, arn) = seeded_emulator().await;
    let alias = state::SUPPORT_ACCOUNT_ALIAS;
    let cluster = crate::snapshot_access::CLUSTER;

    let rows: Vec<DeniedCase> = vec![
        DeniedCase {
            label: "a snapshot nobody holds a grant on",
            account: "555566667777",
            locator: vec![("SnapshotIdentifier", SNAPSHOT)],
            seeded: vec![],
        },
        DeniedCase {
            label: "one other account granted, named by identifier",
            account: "888899990000",
            locator: vec![("SnapshotIdentifier", SNAPSHOT)],
            seeded: vec!["111122223333"],
        },
        DeniedCase {
            label: "one other account granted, named by ARN",
            account: "123456789012",
            locator: vec![("SnapshotArn", arn.as_str())],
            seeded: vec![],
        },
        DeniedCase {
            label: "several accounts granted, named with the cluster",
            account: "999999999999",
            locator: vec![
                ("SnapshotIdentifier", SNAPSHOT),
                ("SnapshotClusterIdentifier", cluster),
            ],
            seeded: vec!["444455556666", "777788889999"],
        },
        DeniedCase {
            label: "the support alias while only identifiers are held",
            account: alias,
            locator: vec![("SnapshotIdentifier", SNAPSHOT)],
            seeded: vec![],
        },
    ];

    for case in &rows {
        for account in &case.seeded {
            grant(&emulator, account).await;
        }
        let before = describe(&emulator, SNAPSHOT).await;

        let answer = revoke(&emulator, case.account, &case.locator).await;
        let label = case.label;
        assert_eq!(
            answer.status, 400,
            "case {label}: expected status 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code().as_deref(),
            Some("AccessToSnapshotDenied"),
            "case {label}: expected error code AccessToSnapshotDenied, got {:?} with body {}",
            answer.error_code(),
            answer.body
        );
        let message = answer.text("Message").unwrap_or_default();
        assert!(
            message.contains("has not authorized your account"),
            "case {label}: expected the access-denied wording, got {message:?}"
        );
        assert!(
            message.contains(SNAPSHOT) && message.contains(case.account),
            "case {label}: expected the snapshot {SNAPSHOT} and the account {} in the message, \
             got {message:?}",
            case.account
        );

        // The refusal is SCOPED: nothing about the set changed, so no other
        // account lost its grant and the refused account was not added.
        let after = describe(&emulator, SNAPSHOT).await;
        assert_eq!(
            granted_ids(&after),
            granted_ids(&before),
            "case {label}: expected the account identifiers unchanged, got {:?}",
            granted_ids(&after)
        );
        assert_eq!(
            granted_aliases(&after),
            granted_aliases(&before),
            "case {label}: expected the account aliases unchanged, got {:?}",
            granted_aliases(&after)
        );
        assert!(
            !granted_ids(&after).contains(&case.account.to_string())
                && !granted_aliases(&after).contains(&case.account.to_string()),
            "case {label}: expected the refused account absent from the set, got {:?} / {:?}",
            granted_ids(&after),
            granted_aliases(&after)
        );
    }

    // And the denial is specific to the account that holds nothing: every
    // account that IS granted is released by the same operation, leaving the
    // others in place.
    let mut remaining: Vec<String> = granted_ids(&describe(&emulator, SNAPSHOT).await);
    assert!(
        remaining.len() >= 2,
        "the rows above must leave more than one grant to release, got {remaining:?}"
    );
    while let Some(account) = remaining.first().cloned() {
        let released = revoke(&emulator, &account, &[("SnapshotIdentifier", SNAPSHOT)]).await;
        assert_eq!(
            released.status, 200,
            "case a granted account released: expected 200 for {account}, got {} with body {}",
            released.status, released.body
        );
        remaining.retain(|held| held != &account);
        assert_eq!(
            granted_ids(&released),
            remaining,
            "case a granted account released: expected {remaining:?} to remain, got {:?}",
            granted_ids(&released)
        );
        // ROUND TRIP: what the release persisted is what a later read reports.
        let read_back = describe(&emulator, SNAPSHOT).await;
        assert_eq!(
            granted_ids(&read_back),
            remaining,
            "case a granted account released: expected the persisted set {remaining:?}, got {:?}",
            granted_ids(&read_back)
        );
        // Releasing it once is enough: the second revoke of the same account is
        // the denial again, because the snapshot no longer holds that grant.
        let again = revoke(&emulator, &account, &[("SnapshotIdentifier", SNAPSHOT)]).await;
        assert_eq!(
            again.error_code().as_deref(),
            Some("AccessToSnapshotDenied"),
            "case the same account released twice: expected AccessToSnapshotDenied, got {:?}",
            again.error_code()
        );
    }
}
