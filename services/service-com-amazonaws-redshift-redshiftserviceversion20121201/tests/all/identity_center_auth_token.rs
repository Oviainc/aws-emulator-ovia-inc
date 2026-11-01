// SPDX-License-Identifier: CC0-1.0

//! `GetIdentityCenterAuthToken`: the IAM Identity Center authentication token a
//! caller asks Amazon Redshift to generate, end to end over the awsQuery wire.
//!
//! Two rules are under test, and each row below is one member of the class the
//! rule generalises over rather than one recorded request:
//!
//! 1. The clusters a request scopes its token to are judged against the
//!    constraints the `ClusterIds` member documents — at least 1 and at most 20
//!    identifiers, each 1 to 63 alphanumeric-or-hyphen characters starting with a
//!    letter, ending with no hyphen and holding no two consecutive hyphens — and
//!    a list outside them is refused with `RedshiftInvalidParameter` (400). The
//!    class is therefore the SIZE of the list crossed with the FORM of its
//!    entries.
//! 2. The identity such a token carries comes from the credentials the request
//!    was signed with, and this emulator attaches an IAM Identity Center identity
//!    to none of the credentials it accepts, so a well-formed request is refused
//!    with `UnsupportedOperation` (400) — whatever its scope, and whether or not
//!    this account holds the clusters it names. The class is the scope crossed
//!    with what the account's control plane holds.
//!
//! Both rules together mean no call ever answers a `Token`, and no call changes
//! anything a later caller reads — which the third case asserts by reading every
//! cluster back after a batch of refused requests.

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// A cluster identifier of exactly [`state::TOKEN_SCOPE_CLUSTER_ID_RULE`]'s
/// documented maximum length, and one character past it.
fn identifier_of_length(len: usize) -> String {
    format!("c{}", "a".repeat(len - 1))
}

/// `ClusterIds` travels as its member's own element name — the list member's
/// `xmlName` is `ClusterIdentifier` — which is what an SDK sends.
fn scope(cluster_ids: &[String]) -> Vec<(String, String)> {
    cluster_ids
        .iter()
        .enumerate()
        .map(|(position, cluster_id)| {
            (
                format!("ClusterIds.ClusterIdentifier.{}", position + 1),
                cluster_id.clone(),
            )
        })
        .collect()
}

/// One `GetIdentityCenterAuthToken` call scoped to these cluster identifiers.
async fn token(emulator: &Emulator, cluster_ids: &[String]) -> Answer {
    let params = scope(cluster_ids);
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator
        .call("GetIdentityCenterAuthToken", &borrowed)
        .await
}

/// Owned identifiers from a slice of literals, so a row can be written inline.
fn ids(cluster_ids: &[&str]) -> Vec<String> {
    cluster_ids.iter().map(|id| (*id).to_string()).collect()
}

/// The `ClusterStatus` a `DescribeClusters` reports for one cluster, or `None`
/// when this account holds no such cluster.
async fn cluster_status(emulator: &Emulator, identifier: &str) -> Option<String> {
    emulator
        .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
        .await
        .text("ClusterStatus")
}

/// Create one cluster and wait out the provisioning span, so the row that names
/// a cluster of this account names one that is `available`.
async fn create_available_cluster(emulator: &Emulator, identifier: &str) {
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {identifier}: expected 200, got {} with body {}",
        created.status, created.body
    );
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    let settled = cluster_status(emulator, identifier).await;
    assert_eq!(
        settled.as_deref(),
        Some(state::STATUS_AVAILABLE),
        "case seed cluster {identifier}: expected it to settle available, got {settled:?}"
    );
}

/// RULE 2: a request whose scope satisfies every documented constraint is still
/// refused as unsupported, because no credential this emulator accepts carries an
/// embedded IAM Identity Center identity — so the refusal is about the CALLER and
/// not about the clusters, and it is the same for a scope of held and unheld
/// clusters alike.
#[tokio::test]
async fn a_caller_without_an_embedded_identity_center_identity_is_refused_as_unsupported() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "warehouse-one").await;
    create_available_cluster(&emulator, "warehouse-two").await;

    // The class: how many clusters the token is scoped to, and what the account
    // holds of them. Every entry satisfies the member's documented syntax.
    let twenty: Vec<String> = (1..=state::MAX_TOKEN_SCOPE_CLUSTERS)
        .map(|index| format!("warehouse-{index}"))
        .collect();
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("one cluster this account holds", ids(&["warehouse-one"])),
        (
            "one cluster this account does not hold",
            ids(&["no-such-warehouse"]),
        ),
        (
            "two held clusters",
            ids(&["warehouse-one", "warehouse-two"]),
        ),
        (
            "a held cluster beside an unheld one",
            ids(&["warehouse-one", "no-such-warehouse"]),
        ),
        ("the documented maximum number of clusters", twenty),
        (
            "an identifier of the documented maximum length",
            vec![identifier_of_length(63)],
        ),
        (
            "mixed case and digits, which this member's syntax admits",
            ids(&["Warehouse-7"]),
        ),
    ];

    for (case, cluster_ids) in rows {
        let answered = token(&emulator, &cluster_ids).await;
        assert_eq!(
            (answered.status, answered.error_code().as_deref()),
            (400, Some("UnsupportedOperation")),
            "case {case:?}: expected 400 UnsupportedOperation, got {} / {:?} with body {}",
            answered.status,
            answered.error_code(),
            answered.body
        );
        assert_eq!(
            answered.text("Message").as_deref(),
            Some(state::IDENTITY_CENTER_ENHANCED_CREDENTIALS_REQUIRED),
            "case {case:?}: expected Amazon Redshift's refusal of a caller that is not an \
             Identity Center user, got {:?}",
            answered.text("Message")
        );
        assert_eq!(
            (answered.text("Token"), answered.text("ExpirationTime")),
            (None, None),
            "case {case:?}: expected no token and no expiry in a refusal, got {:?} / {:?}",
            answered.text("Token"),
            answered.text("ExpirationTime")
        );
    }
}

/// RULE 1: the scope is judged against the constraints its member documents
/// BEFORE anything about the caller, so a list outside them earns the fault the
/// operation models for a parameter it cannot honour — one row per constraint,
/// including the published bound and the first value past it.
#[tokio::test]
async fn a_scope_outside_its_documented_constraints_is_refused_as_an_invalid_parameter() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "warehouse-one").await;

    let too_many: Vec<String> = (1..=state::MAX_TOKEN_SCOPE_CLUSTERS + 1)
        .map(|index| format!("warehouse-{index}"))
        .collect();
    // Each row names the documented constraint it breaks and the fragment of the
    // refusal that must name what the caller has to fix.
    let rows: Vec<(&str, Vec<String>, &str)> = vec![
        (
            "more identifiers than the documented maximum",
            too_many,
            "maximum of 20",
        ),
        (
            "an identifier one character past the documented maximum length",
            vec![identifier_of_length(64)],
            "longer than allowed",
        ),
        (
            "an identifier that does not start with a letter",
            ids(&["1warehouse"]),
            "first character must be a letter",
        ),
        (
            "an identifier ending with a hyphen",
            ids(&["warehouse-"]),
            "cannot end with a hyphen",
        ),
        (
            "an identifier with two consecutive hyphens",
            ids(&["ware--house"]),
            "two consecutive hyphens",
        ),
        (
            "an identifier with a character the member does not admit",
            ids(&["ware_house"]),
            "neither alphanumeric nor a hyphen",
        ),
        (
            "a well-formed identifier beside a malformed one",
            ids(&["warehouse-one", "ware_house"]),
            "neither alphanumeric nor a hyphen",
        ),
    ];

    for (case, cluster_ids, expected_fragment) in rows {
        let answered = token(&emulator, &cluster_ids).await;
        assert_eq!(
            (answered.status, answered.error_code().as_deref()),
            (400, Some("RedshiftInvalidParameter")),
            "case {case:?}: expected 400 RedshiftInvalidParameter, got {} / {:?} with body {}",
            answered.status,
            answered.error_code(),
            answered.body
        );
        let message = answered.text("Message").unwrap_or_default();
        assert!(
            message.contains("ClusterIds") && message.contains(expected_fragment),
            "case {case:?}: expected a refusal naming ClusterIds and {expected_fragment:?}, \
             got {message:?}"
        );
        assert_eq!(
            answered.text("Token"),
            None,
            "case {case:?}: expected no token in a refusal, got {:?}",
            answered.text("Token")
        );
    }

    // The documented minimum — "ClusterIds must contain at least 1 cluster
    // identifier" — is the member's own `@required` trait, so a request naming
    // none never reaches the handler: the boundary validator refuses it, naming
    // the member it is missing.
    let silent = token(&emulator, &[]).await;
    assert_eq!(
        (silent.status, silent.error_code().as_deref()),
        (400, Some("MalformedInput")),
        "case \"no cluster identifier at all\": expected the boundary to refuse it, got {} / \
         {:?} with body {}",
        silent.status,
        silent.error_code(),
        silent.body
    );
    assert!(
        silent
            .text("Message")
            .is_some_and(|message| message.to_ascii_lowercase().contains("clusterids")),
        "case \"no cluster identifier at all\": expected the refusal to name the member, got {:?}",
        silent.text("Message")
    );
}

/// A refused request is not a write: every cluster of the account reads back
/// exactly as it did before a batch of refused calls, and no new cluster appears.
#[tokio::test]
async fn a_refused_request_changes_nothing_a_later_caller_reads() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "warehouse-one").await;
    create_available_cluster(&emulator, "warehouse-two").await;

    let listed = |answer: &Answer| {
        crate::cluster_lifecycle::all_text(&answer.body, "ClusterIdentifier")
            .into_iter()
            .zip(crate::cluster_lifecycle::all_text(
                &answer.body,
                "ClusterStatus",
            ))
            .collect::<Vec<(String, String)>>()
    };

    let before = emulator.call("DescribeClusters", &[]).await;
    assert_eq!(
        listed(&before),
        vec![
            ("warehouse-one".to_string(), state::STATUS_AVAILABLE.to_string()),
            ("warehouse-two".to_string(), state::STATUS_AVAILABLE.to_string()),
        ],
        "case the seeded account: expected two available clusters, got {:?}",
        listed(&before)
    );

    // Refused for the caller (rule 2), refused for the request (rule 1), and
    // repeated — none of which may leave a trace.
    for cluster_ids in [
        ids(&["warehouse-one"]),
        ids(&["warehouse-one", "warehouse-two"]),
        ids(&["no-such-warehouse"]),
        ids(&["ware_house"]),
        Vec::new(),
    ] {
        let answered = token(&emulator, &cluster_ids).await;
        assert_eq!(
            answered.status, 400,
            "case scope {cluster_ids:?}: expected a 400 refusal, got {} with body {}",
            answered.status, answered.body
        );
    }

    let after = emulator.call("DescribeClusters", &[]).await;
    assert_eq!(
        listed(&after),
        listed(&before),
        "case the account after refused token requests: expected the clusters unchanged, \
         got {:?}",
        listed(&after)
    );
}
