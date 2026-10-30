// SPDX-License-Identifier: CC0-1.0

//! Which policy writes one `clientToken` may serve, end to end over the wire.
//!
//! RULE: a `clientToken` is bound to the VALUES of the request it first served,
//! and an OPTIONAL member the handler reads is one of those values whether the
//! request carries it or not — so a call that omits the member and a call that
//! carries it, an EMPTY value included, are different requests: the second is
//! refused 409 `ConflictException` and has no effect of its own, while the
//! identical request is a retry answered with what the first call produced.
//!
//! The member the table spans is `expectedPolicyVersion`, the optional claim
//! both policy writers read: an unconstrained string
//! (`smithy_query(query="shape", shape="PolicyVersion")` — no `@length`, no
//! `@pattern`), so the empty string is a value a client can send and the
//! handlers answer on its own terms. The rows differ along the ways two claims
//! can differ — carried against absent, in both directions, absent against
//! empty, empty against the attached version, and a plainly different value
//! (numeric and non-numeric) — and each row is driven on both entry points the
//! claim arrives on: `PutClusterPolicy`'s body member and
//! `DeleteClusterPolicy`'s `expected-policy-version` query parameter.
//!
//! Every row also asserts what the refused request answers under a token of its
//! OWN, which is what makes the two requests different requests: the claim is
//! then read by the handler's own version check, which serves it or refuses it
//! on the version actually attached.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::{json, Value};

use crate::client_token_parameter_distinctness::assert_token_conflict;
use crate::harness::Dsql;
use crate::policy_version_conflict_and_lockout::{benign_document, version_message};

/// One row: two policy writes whose `expectedPolicyVersion` differs, so one
/// `clientToken` cannot serve both.
///
/// Each claim is built from the version attached WHEN ITS OWN CALL IS MADE, so a
/// row can name "the attached version" without spelling the counter. `first`
/// must be a claim the handler serves — absent, or the attached version — since
/// a refused call records no token.
struct Row {
    name: &'static str,
    /// The claim the first request carries, given the version attached then.
    first: fn(&str) -> Option<String>,
    /// The claim the second request carries, given the version attached then.
    second: fn(&str) -> Option<String>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "no claim against an empty claim",
            first: |_| None,
            second: |_| Some(String::new()),
        },
        Row {
            name: "the attached version claimed against no claim",
            first: |attached| Some(attached.to_string()),
            second: |_| None,
        },
        Row {
            name: "the attached version claimed against an empty claim",
            first: |attached| Some(attached.to_string()),
            second: |_| Some(String::new()),
        },
        Row {
            name: "no claim against the attached version claimed",
            first: |_| None,
            second: |attached| Some(attached.to_string()),
        },
        Row {
            name: "the attached version claimed against a version never issued",
            first: |attached| Some(attached.to_string()),
            second: |_| Some("99".to_string()),
        },
        Row {
            name: "no claim against a non-numeric claim",
            first: |_| None,
            second: |_| Some("notaversion".to_string()),
        },
    ]
}

/// What a policy write carrying `claim` must answer under a token of its own,
/// against `attached` being the version currently attached: a claim about
/// another version is the handler's own refusal, anything else is served.
enum OwnAnswer {
    Served,
    VersionRefused,
}

fn own_answer(claim: Option<&str>, attached: &str) -> OwnAnswer {
    match claim {
        None => OwnAnswer::Served,
        Some(claimed) if claimed == attached => OwnAnswer::Served,
        Some(_) => OwnAnswer::VersionRefused,
    }
}

/// Assert the handler's OWN refusal of a claim about a version that is not the
/// attached one: the request-derived sentence, not the token's, under the status
/// the entry point answers it with — 409 for `PutClusterPolicy`, whose claim is a
/// precondition on the replace, and 404 for `DeleteClusterPolicy`, whose claim
/// names the version it was asked to remove.
fn assert_version_refusal(
    case: &str,
    answer: &crate::harness::Answer,
    cluster: &str,
    claim: &str,
    expected_status: u16,
) {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: a claim about another version must answer {expected_status}, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.message(),
        version_message(cluster, claim),
        "case {case:?}: the refusal must name the claimed version, got {:?}",
        answer.message()
    );
}

/// A `PutClusterPolicy` body attaching `document` under `token`, carrying
/// `expectedPolicyVersion` only when the case claims one.
fn put_body(document: &str, token: &str, claim: Option<&str>) -> Value {
    let mut body = json!({"policy": document, "clientToken": token});
    if let Some(claimed) = claim {
        body["expectedPolicyVersion"] = json!(claimed);
    }
    body
}

/// The `DeleteClusterPolicy` query string for `token`, carrying
/// `expected-policy-version` only when the case claims one.
fn delete_query(token: &str, claim: Option<&str>) -> String {
    match claim {
        None => format!("?client-token={token}"),
        Some(claimed) => format!("?client-token={token}&expected-policy-version={claimed}"),
    }
}

/// Attach one benign document to `identifier` without a token and answer with the
/// version it got.
async fn attach(dsql: &Dsql, case: &str, identifier: &str, round: usize) -> String {
    let answer = dsql
        .put_policy(identifier, json!({"policy": benign_document(round)}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the setup attach must answer 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.str("policyVersion")
}

/// The version `GetClusterPolicy` reports for `identifier`, or `None` when
/// nothing is attached.
async fn attached_version(dsql: &Dsql, identifier: &str) -> Option<String> {
    let read = dsql.get_policy(identifier).await;
    if read.status == 200 {
        Some(read.str("policyVersion"))
    } else {
        None
    }
}

/// RULE over `PutClusterPolicy`: the token the first attach used serves only
/// that attach, so a second attach of the same document whose
/// `expectedPolicyVersion` differs — including by being carried at all — is 409
/// `ConflictException` and attaches nothing, while that very request under its
/// own token is answered by the handler's own version check.
#[tokio::test]
async fn a_put_cluster_policy_token_is_bound_to_whether_a_version_was_claimed() {
    let dsql = Dsql::new();
    let cases = rows();
    let clusters = dsql.create_settled_many(cases.len()).await;

    for (index, row) in cases.iter().enumerate() {
        let cluster = &clusters[index];
        let token = format!("put-claim-{index}");
        let document = benign_document(1);

        // A policy to claim a version of, then the first attach under the token.
        let initial = attach(&dsql, row.name, cluster, 0).await;
        let first_claim = (row.first)(&initial);
        let first = dsql
            .put_policy(cluster, put_body(&document, &token, first_claim.as_deref()))
            .await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first attach must answer 200, got {} {:?}",
            row.name, first.status, first.body
        );
        let issued = first.str("policyVersion");

        // The identical request under that token is a retry: the first call's
        // version, and no second version issued.
        let replay = dsql
            .put_policy(cluster, put_body(&document, &token, first_claim.as_deref()))
            .await;
        assert_eq!(
            (replay.status, replay.str("policyVersion")),
            (200, issued.clone()),
            "case {:?}: the identical request must answer the first version {issued:?}, \
             got {} {:?}",
            row.name,
            replay.status,
            replay.body
        );
        assert_eq!(
            attached_version(&dsql, cluster).await,
            Some(issued.clone()),
            "case {:?}: the retry must attach nothing of its own",
            row.name
        );

        // The row's other request claims differently, so the used token refuses
        // it and nothing is attached in its place.
        let second_claim = (row.second)(&issued);
        let conflict = dsql
            .put_policy(cluster, put_body(&document, &token, second_claim.as_deref()))
            .await;
        assert_token_conflict(row.name, &conflict);
        assert_eq!(
            attached_version(&dsql, cluster).await,
            Some(issued.clone()),
            "case {:?}: the refused request must attach nothing",
            row.name
        );

        // The refusal was about the TOKEN: under a token of its own the same
        // request is read by the handler's own version check.
        let own_token = format!("{token}-of-its-own");
        let alone = dsql
            .put_policy(
                cluster,
                put_body(&document, &own_token, second_claim.as_deref()),
            )
            .await;
        match own_answer(second_claim.as_deref(), &issued) {
            OwnAnswer::Served => {
                assert_eq!(
                    alone.status, 200,
                    "case {:?}: the second request must be servable under its own token, \
                     got {} {:?}",
                    row.name, alone.status, alone.body
                );
                let minted = alone.str("policyVersion");
                assert_ne!(
                    minted, issued,
                    "case {:?}: the second attach must mint a version of its own, got {minted:?}",
                    row.name
                );
                assert_eq!(
                    attached_version(&dsql, cluster).await,
                    Some(minted.clone()),
                    "case {:?}: the cluster must report the version just attached ({minted:?})",
                    row.name
                );
            }
            OwnAnswer::VersionRefused => {
                let claimed = second_claim.as_deref().unwrap_or_default();
                assert_version_refusal(row.name, &alone, cluster, claimed, 409);
                assert_eq!(
                    attached_version(&dsql, cluster).await,
                    Some(issued.clone()),
                    "case {:?}: the handler's own refusal must attach nothing",
                    row.name
                );
            }
        }
    }
}

/// RULE over `DeleteClusterPolicy`, where the claim arrives on the
/// `expected-policy-version` query parameter: the token the first delete used
/// serves only that delete, so a later delete whose claim differs — including by
/// being carried at all — is 409 `ConflictException` and removes nothing, rather
/// than being replayed the version the first delete removed.
#[tokio::test]
async fn a_delete_cluster_policy_token_is_bound_to_whether_a_version_was_claimed() {
    let dsql = Dsql::new();
    let cases = rows();
    let clusters = dsql.create_settled_many(cases.len()).await;

    for (index, row) in cases.iter().enumerate() {
        let cluster = &clusters[index];
        let token = format!("delete-claim-{index}");

        // A policy to remove, then the first delete under the token.
        let initial = attach(&dsql, row.name, cluster, 0).await;
        let first_claim = (row.first)(&initial);
        let first = dsql
            .delete_policy(cluster, &delete_query(&token, first_claim.as_deref()))
            .await;
        assert_eq!(
            (first.status, first.str("policyVersion")),
            (200, initial.clone()),
            "case {:?}: the first delete must remove {initial:?}, got {} {:?}",
            row.name,
            first.status,
            first.body
        );
        assert_eq!(
            attached_version(&dsql, cluster).await,
            None,
            "case {:?}: the first delete must leave nothing attached",
            row.name
        );

        // The identical request under that token is a retry: the version the
        // first delete removed, instead of the 404 an independent delete gets.
        let replay = dsql
            .delete_policy(cluster, &delete_query(&token, first_claim.as_deref()))
            .await;
        assert_eq!(
            (replay.status, replay.str("policyVersion")),
            (200, initial.clone()),
            "case {:?}: the identical request must answer the removed version {initial:?}, \
             got {} {:?}",
            row.name,
            replay.status,
            replay.body
        );

        // A policy is attached again, so a delete that IS served removes
        // something — and a replay of the first delete would be visible as a 200
        // that removed nothing.
        let current = attach(&dsql, row.name, cluster, 1).await;
        assert_ne!(
            current, initial,
            "case {:?}: the re-attached policy must carry a version of its own",
            row.name
        );

        let second_claim = (row.second)(&current);
        let conflict = dsql
            .delete_policy(cluster, &delete_query(&token, second_claim.as_deref()))
            .await;
        assert_token_conflict(row.name, &conflict);
        assert_eq!(
            attached_version(&dsql, cluster).await,
            Some(current.clone()),
            "case {:?}: the refused request must remove nothing",
            row.name
        );

        // Under a token of its own the same request is read by the handler's own
        // version check.
        let own_token = format!("{token}-of-its-own");
        let alone = dsql
            .delete_policy(cluster, &delete_query(&own_token, second_claim.as_deref()))
            .await;
        match own_answer(second_claim.as_deref(), &current) {
            OwnAnswer::Served => {
                assert_eq!(
                    (alone.status, alone.str("policyVersion")),
                    (200, current.clone()),
                    "case {:?}: the second delete must remove {current:?} under its own token, \
                     got {} {:?}",
                    row.name,
                    alone.status,
                    alone.body
                );
                assert_eq!(
                    attached_version(&dsql, cluster).await,
                    None,
                    "case {:?}: the served delete must leave nothing attached",
                    row.name
                );
            }
            OwnAnswer::VersionRefused => {
                let claimed = second_claim.as_deref().unwrap_or_default();
                assert_version_refusal(row.name, &alone, cluster, claimed, 404);
                assert_eq!(
                    attached_version(&dsql, cluster).await,
                    Some(current.clone()),
                    "case {:?}: the handler's own refusal must remove nothing",
                    row.name
                );
            }
        }
    }
}
