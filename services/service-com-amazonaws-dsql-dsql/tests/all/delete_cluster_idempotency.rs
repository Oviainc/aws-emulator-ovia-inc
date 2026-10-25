// SPDX-License-Identifier: CC0-1.0

//! What `DeleteCluster`'s `clientToken` means, end to end over the wire.
//!
//! RULE: a `clientToken` names ONE request. A delete repeating a request the
//! ledger already served answers that request's recorded result and has no
//! further effect — including after the clock has removed the cluster, which is
//! exactly when a retry of a delete arrives — while the same token carried by a
//! DIFFERENT delete is refused 409 `ConflictException` and deletes nothing.
//!
//! The tables span what the rule generalises over: the shape of the token
//! (`ClientToken` is `@length(1..128)`, `@pattern("^[!-~]+$")`), which window the
//! retry lands in (the cluster still `DELETING`, or already retired), how many
//! retries there are, and which other delete the used token is carried by.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::{
    ComAmazonawsDsqlDsqlState, RecordedCall, TokenScope, CLIENT_TOKEN_MISMATCH_MESSAGE,
    MAX_CLIENT_TOKENS,
};

use crate::cluster_lifecycle::{cluster_id_for, retiring_cluster};
use crate::harness::{wait_for_the_clock, Answer, Dsql};

/// Tokens spanning what `ClientToken` admits: its minimum length, an ordinary
/// one, punctuation only, and its maximum length. Source: the model's own
/// `smithy_query(query="operation", operation="DeleteCluster")` — `clientToken`
/// is `@length(min: 1, max: 128)`, `@pattern("^[!-~]+$")`. The characters a URI
/// query string would itself read (`&`, `=`, `#`, `%`, `+`, `?`) are left out:
/// they are not part of what this rule generalises over.
fn token_shapes() -> Vec<(&'static str, String)> {
    vec![
        ("one character, ClientToken's minimum", "t".to_string()),
        ("an ordinary token", "delete-token-1".to_string()),
        ("punctuation only", "~!*()-_.".to_string()),
        (
            "128 characters, ClientToken's maximum",
            "z".repeat(128),
        ),
    ]
}

/// The four members `DeleteClusterOutput` carries, as one comparable value, so a
/// retry's WHOLE answer is compared with the original's rather than one member
/// of it (`smithy_query(query="operation", operation="DeleteCluster")`: `arn`,
/// `creationTime`, `identifier`, `status`, all `@required`).
fn delete_answer(answer: &Answer) -> Value {
    json!({
        "arn": answer.get("arn"),
        "creationTime": answer.get("creationTime"),
        "identifier": answer.get("identifier"),
        "status": answer.get("status"),
    })
}

/// Assert `answer` is a served delete, and answer with its members.
fn served(case: &str, which: &str, answer: &Answer) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: {which} must answer 200, got {} {:?}",
        answer.status, answer.body
    );
    delete_answer(answer)
}

/// RULE: a delete repeating one the ledger already served answers that call's
/// recorded result — the same ARN, creation time, identifier and `DELETING`
/// status — however many times it is repeated and whether it arrives while the
/// cluster is still on its way out or after the clock has removed it.
#[tokio::test]
async fn a_repeated_delete_answers_the_recorded_result() {
    /// Where each retry of a row lands: before the clock retires the cluster, or
    /// after it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum When {
        StillDeleting,
        AlreadyRetired,
    }

    let rows: Vec<(&str, Vec<When>)> = vec![
        ("one retry while the cluster is still DELETING", vec![When::StillDeleting]),
        ("one retry after the clock retired the cluster", vec![When::AlreadyRetired]),
        (
            "a retry in each window",
            vec![When::StillDeleting, When::AlreadyRetired],
        ),
        (
            "three retries after the clock retired the cluster",
            vec![
                When::AlreadyRetired,
                When::AlreadyRetired,
                When::AlreadyRetired,
            ],
        ),
    ];

    let shapes = token_shapes();
    for (name, retries) in rows {
        for (shape, token) in &shapes {
            let case = format!("{name}, with {shape}");
            let dsql = Dsql::new();
            let identifier = dsql.create_settled().await;

            let first = dsql.delete_with_token(&identifier, token).await;
            let expected = served(&case, "the first delete", &first);
            assert_eq!(
                first.str("status"),
                "DELETING",
                "case {case:?}: the first delete must report DELETING, got {:?}",
                first.str("status")
            );

            let mut retired = false;
            for when in &retries {
                let when = *when;
                if when == When::AlreadyRetired && !retired {
                    wait_for_the_clock().await;
                    retired = true;
                    // The cluster really is gone: a reader no longer holds it,
                    // so the retry below cannot be answered from the store.
                    let read = dsql.get(&identifier).await;
                    assert_eq!(
                        read.status, 404,
                        "case {case:?}: the clock must have removed the cluster, got {} {:?}",
                        read.status, read.body
                    );
                }
                let retry = dsql.delete_with_token(&identifier, token).await;
                assert_eq!(
                    served(&case, &format!("the retry {when:?}"), &retry),
                    expected,
                    "case {case:?}: the retry {when:?} must answer the first call's result \
                     {expected:?}, got {:?}",
                    delete_answer(&retry)
                );
            }

            // The repeats had no effect of their own: a delete that is NOT a
            // repeat still finds the cluster gone.
            let tokenless = dsql.delete(&identifier).await;
            assert_eq!(
                tokenless.status,
                if retired { 404 } else { 200 },
                "case {case:?}: a delete carrying no token must report the live store, \
                 got {} {:?}",
                tokenless.status,
                tokenless.body
            );
        }
    }
}

/// Which other delete a used token is carried by.
#[derive(Debug, Clone, Copy)]
enum Reused {
    /// Another cluster, ready to be deleted.
    OnAnActiveCluster,
    /// Another cluster already on its way out under no token at all.
    OnADeletingCluster,
}

/// RULE: the same `clientToken` carried by a DIFFERENT delete is a different
/// request wearing a used token, so it is refused 409 `ConflictException` with
/// the parameter-mismatch message and the cluster it named is not deleted.
#[tokio::test]
async fn a_used_delete_token_refuses_a_different_delete() {
    let rows = [
        ("the token reused on another ACTIVE cluster", Reused::OnAnActiveCluster),
        (
            "the token reused on a cluster already DELETING",
            Reused::OnADeletingCluster,
        ),
    ];

    let shapes = token_shapes();
    for (name, reused) in rows {
        for (shape, token) in &shapes {
            let case = format!("{name}, with {shape}");
            let dsql = Dsql::new();
            let clusters = dsql.create_settled_many(2).await;
            let (first_target, other) = (&clusters[0], &clusters[1]);

            let first = dsql.delete_with_token(first_target, token).await;
            served(&case, "the first delete", &first);

            let expected_status = match reused {
                Reused::OnAnActiveCluster => "ACTIVE",
                Reused::OnADeletingCluster => {
                    let started = dsql.delete(other).await;
                    served(&case, "the tokenless delete of the other cluster", &started);
                    "DELETING"
                }
            };

            let conflict = dsql.delete_with_token(other, token).await;
            assert_eq!(
                conflict.status, 409,
                "case {case:?}: a used token on another cluster must be refused with 409, \
                 got {} {:?}",
                conflict.status, conflict.body
            );
            assert_eq!(
                conflict.error.as_deref(),
                Some("ConflictException"),
                "case {case:?}: the refusal must be a ConflictException, got {:?}",
                conflict.error
            );
            assert_eq!(
                conflict.message(),
                CLIENT_TOKEN_MISMATCH_MESSAGE,
                "case {case:?}: got {:?}",
                conflict.message()
            );

            // The refused call changed nothing: the other cluster is exactly as
            // the case left it.
            let read = dsql.get(other).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the refused delete must leave the other cluster readable, \
                 got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.str("status"),
                expected_status,
                "case {case:?}: the refused delete must leave the other cluster in \
                 {expected_status}, got {:?}",
                read.str("status")
            );
        }
    }
}

/// RULE: every delete the service SERVES is recorded under the token it carried,
/// not only the one that started the deletion — so a repeat of a delete that
/// answered for a cluster already on its way out answers its recorded result too
/// rather than the 404 the vanished cluster would otherwise give.
#[tokio::test]
async fn every_served_delete_is_recorded_under_its_own_token() {
    // Each row is how many successive deletes, each under its own token, the
    // cluster is asked for before the clock retires it.
    for deletes in [2_usize, 3, 5] {
        let case = format!("{deletes} deletes, each under its own token");
        let dsql = Dsql::new();
        let identifier = dsql.create_settled().await;

        let mut answers = Vec::with_capacity(deletes);
        for round in 0..deletes {
            let token = format!("delete-{round}");
            let answer = dsql.delete_with_token(&identifier, &token).await;
            let members = served(&case, &format!("delete #{round}"), &answer);
            assert_eq!(
                answer.str("status"),
                "DELETING",
                "case {case:?}: delete #{round} must report DELETING, got {:?}",
                answer.str("status")
            );
            answers.push((token, members));
        }

        wait_for_the_clock().await;
        assert_eq!(
            dsql.get(&identifier).await.status,
            404,
            "case {case:?}: the clock must have removed the cluster"
        );

        for (round, (token, expected)) in answers.iter().enumerate() {
            let retry = dsql.delete_with_token(&identifier, token).await;
            assert_eq!(
                served(&case, &format!("the retry of delete #{round}"), &retry),
                *expected,
                "case {case:?}: the retry of delete #{round} must answer that call's result \
                 {expected:?}, got {:?}",
                delete_answer(&retry)
            );
        }
    }
}

/// RULE: the record of a delete outlives the cluster it removed — that is what a
/// retry arriving after the clock has removed it is answered from — while the
/// record of a call ON a cluster goes with it, and the ledger stays bounded
/// either way.
///
/// Driven against the state directly: filling the ledger to its cap over the
/// wire would need thousands of requests, each waiting out a settle span.
#[test]
fn a_delete_record_outlives_its_cluster_and_the_ledger_stays_bounded() {
    let mut state = ComAmazonawsDsqlDsqlState::default();
    let already_due = SystemTime::now() - Duration::from_secs(1);

    // One retirement: the delete's own record stands, the create's goes.
    let identifier = cluster_id_for(0);
    for scope in [TokenScope::CreateCluster, TokenScope::DeleteCluster] {
        state.tokens(scope).record(
            "tok".to_string(),
            "fingerprint".to_string(),
            RecordedCall::new(&identifier),
        );
    }
    state
        .clusters
        .insert(identifier.clone(), retiring_cluster(&identifier, already_due));
    state.settle(SystemTime::now());
    assert!(
        state.clusters.is_empty(),
        "a settled deletion must leave the live store empty, got {}",
        state.clusters.len()
    );
    assert_eq!(
        state.tokens_of(TokenScope::DeleteCluster).len(),
        1,
        "the record of the delete must outlive the cluster it removed, got {}",
        state.tokens_of(TokenScope::DeleteCluster).len()
    );
    assert!(
        state.tokens_of(TokenScope::CreateCluster).is_empty(),
        "the record of a call on the cluster must go with it, got {}",
        state.tokens_of(TokenScope::CreateCluster).len()
    );

    // And a caller looping delete records cannot grow the ledger past its cap,
    // even though nothing forgets them with their clusters.
    for n in 0..(MAX_CLIENT_TOKENS * 2) {
        state.tokens(TokenScope::DeleteCluster).record(
            format!("delete-token-{n}"),
            format!("fingerprint-{n}"),
            RecordedCall::new(&cluster_id_for(n)),
        );
        assert!(
            state.tokens_of(TokenScope::DeleteCluster).len() <= MAX_CLIENT_TOKENS,
            "case token #{n}: the delete ledger must stay at or below {MAX_CLIENT_TOKENS}, got {}",
            state.tokens_of(TokenScope::DeleteCluster).len()
        );
    }
    assert_eq!(
        state.tokens_of(TokenScope::DeleteCluster).len(),
        MAX_CLIENT_TOKENS,
        "the delete ledger must fill to its cap and then evict, got {}",
        state.tokens_of(TokenScope::DeleteCluster).len()
    );
}

/// RULE: a `clientToken` is scoped to the operation that served it, so a token
/// another operation already used is unused as far as a delete is concerned.
#[tokio::test]
async fn a_token_another_operation_used_still_serves_a_delete() {    let rows = [
        ("CreateCluster used the token", false),
        ("CreateCluster and UpdateCluster used the token", true),
    ];

    for (name, also_update) in rows {
        let dsql = Dsql::new();
        let token = "shared-across-operations";
        let created = dsql
            .create(json!({"deletionProtectionEnabled": false, "clientToken": token}))
            .await;
        let identifier = served(name, "CreateCluster", &created)
            .get("identifier")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        wait_for_the_clock().await;

        if also_update {
            let updated = dsql
                .update(
                    &identifier,
                    json!({"deletionProtectionEnabled": false, "clientToken": token}),
                )
                .await;
            served(name, "UpdateCluster", &updated);
            wait_for_the_clock().await;
        }

        let deleted = dsql.delete_with_token(&identifier, token).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: a token another operation used must still serve the delete, \
             got {} {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.str("status"),
            "DELETING",
            "case {name:?}: the delete must report DELETING, got {:?}",
            deleted.str("status")
        );
    }
}
