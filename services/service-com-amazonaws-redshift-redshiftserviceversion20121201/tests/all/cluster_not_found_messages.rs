// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE a cluster-scoped Redshift operation refuses an unresolvable
//! `ClusterIdentifier` with, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The RULE the suite asserts: the fault's status (404) and error code
//! (`ClusterNotFound`) are what the model fixes for every operation, while the
//! MESSAGE belongs to the OPERATION — each one answers the sentence Amazon
//! Redshift words for that operation, with the `ClusterIdentifier` taken from
//! the request and nothing else varying. So the sentence is fixed per operation
//! and the identifier is interpolated, and two operations asked about the SAME
//! missing identifier can answer differently.
//!
//! The table spans what the rule generalises over: the operation (every family
//! that dereferences a cluster — a configuration change, a cluster-scoped
//! create, a namespace registration change, a read and a delete, the partner
//! integration and custom domain association families, including the two that
//! name their cluster through a member other than `ClusterIdentifier`)
//! and the identifier (its spelling, both documented length bounds, its case,
//! one character off a stored name, and a cluster this account created and then
//! deleted).

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// Wait out the span a clock-owed change falls due after, plus a margin for a
/// loaded runner.
async fn settle() {
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
}

/// The sentence an operation words its `ClusterNotFound` refusal with.
///
/// Spelled out in this suite rather than read off the crate under test, so a row
/// cannot pass by restating whatever the handler happened to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wording {
    /// `Cluster not found: <ClusterIdentifier>`.
    NotFoundThenIdentifier,
    /// `Cluster <ClusterIdentifier> not found.`
    IdentifierThenNotFound,
    /// `Cluster <ClusterIdentifier> can't be found.`
    IdentifierCannotBeFound,
}

/// The message `wording` words about `cluster_identifier`.
fn sentence(wording: Wording, cluster_identifier: &str) -> String {
    match wording {
        Wording::NotFoundThenIdentifier => format!("Cluster not found: {cluster_identifier}"),
        Wording::IdentifierThenNotFound => format!("Cluster {cluster_identifier} not found."),
        Wording::IdentifierCannotBeFound => {
            format!("Cluster {cluster_identifier} can't be found.")
        }
    }
}

/// The form parameters one operation sends to ask about a single cluster
/// identifier: every member it requires, with the identifier in the member that
/// operation names its cluster through.
type Request = fn(&str) -> Vec<(String, String)>;

/// One row of the operation table: the action, the request it makes about one
/// cluster identifier, and the sentence its refusal must carry.
type OperationCase = (&'static str, Request, Wording);

/// One form parameter, owned so a row can build it from a borrowed identifier.
fn pair(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

fn cluster_only(identifier: &str) -> Vec<(String, String)> {
    vec![pair("ClusterIdentifier", identifier)]
}

fn enable_snapshot_copy(identifier: &str) -> Vec<(String, String)> {
    vec![
        pair("ClusterIdentifier", identifier),
        pair("DestinationRegion", "us-east-1"),
    ]
}

fn snapshot_copy_retention(identifier: &str) -> Vec<(String, String)> {
    vec![
        pair("ClusterIdentifier", identifier),
        pair("RetentionPeriod", "7"),
    ]
}

fn create_cluster_snapshot(identifier: &str) -> Vec<(String, String)> {
    vec![
        pair("ClusterIdentifier", identifier),
        pair("SnapshotIdentifier", "cluster-message-probe-snapshot"),
    ]
}

fn create_usage_limit(identifier: &str) -> Vec<(String, String)> {
    // A feature is metered by ONE limit type, so the pairing comes from the
    // model's own catalogue rather than a literal at the call site — a request
    // that pairs them differently would be refused before any cluster lookup.
    let feature = "concurrency-scaling";
    let limit_type = state::usage_limit_type_of_feature(feature)
        .expect("concurrency-scaling is one of the model's feature types");
    vec![
        pair("ClusterIdentifier", identifier),
        pair("FeatureType", feature),
        pair("LimitType", limit_type),
        pair("Amount", "100"),
    ]
}

fn delete_cluster(identifier: &str) -> Vec<(String, String)> {
    vec![
        pair("ClusterIdentifier", identifier),
        pair("SkipFinalClusterSnapshot", "true"),
    ]
}

/// The account this emulator's clusters belong to, which every partner-family
/// request names through its required `AccountId`.
fn account_id() -> String {
    engine::emulator_account_id().to_string()
}

/// The three members a partner-integration request carries beside its status:
/// the account that owns the cluster, the cluster, and the (database, partner)
/// pair the integration is keyed by.
fn partner_members(identifier: &str) -> Vec<(String, String)> {
    vec![
        ("AccountId".to_string(), account_id()),
        pair("ClusterIdentifier", identifier),
        pair("DatabaseName", "analytics"),
        pair("PartnerName", "datadog"),
    ]
}

fn update_partner_status(identifier: &str) -> Vec<(String, String)> {
    let mut params = partner_members(identifier);
    params.push(pair("Status", state::PARTNER_INTEGRATION_INITIAL_STATUS));
    params
}

/// An ACM certificate ARN of the form `CustomDomainCertificateArn`'s own pattern
/// admits.
fn certificate_arn() -> String {
    "arn:aws:acm:us-west-2:000000000000:certificate/11111111-2222-3333-4444-555555555555"
        .to_string()
}

fn custom_domain_association(identifier: &str) -> Vec<(String, String)> {
    vec![
        pair("ClusterIdentifier", identifier),
        pair("CustomDomainName", "wording-probe.example.com"),
        ("CustomDomainCertificateArn".to_string(), certificate_arn()),
    ]
}

fn delete_custom_domain_association(identifier: &str) -> Vec<(String, String)> {
    vec![
        pair("ClusterIdentifier", identifier),
        pair("CustomDomainName", "wording-probe.example.com"),
    ]
}

/// A namespace registration change names its cluster through the provisioned arm
/// of the modelled `NamespaceIdentifierUnion`, not through a `ClusterIdentifier`
/// member of its own — so the rule is about the identifier the request carries,
/// wherever it carries it.
fn namespace_change(identifier: &str) -> Vec<(String, String)> {
    vec![
        pair(
            "NamespaceIdentifier.ProvisionedIdentifier.ClusterIdentifier",
            identifier,
        ),
        pair("ConsumerIdentifiers.member.1", "111122223333"),
    ]
}

/// Every operation this suite holds to the rule, with the sentence each one
/// answers.
fn operations() -> Vec<OperationCase> {
    vec![
        (
            "ModifyCluster",
            cluster_only as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "ModifyClusterIamRoles",
            cluster_only as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "RebootCluster",
            cluster_only as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "DisableSnapshotCopy",
            cluster_only as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "EnableSnapshotCopy",
            enable_snapshot_copy as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "ModifySnapshotCopyRetentionPeriod",
            snapshot_copy_retention as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "CreateClusterSnapshot",
            create_cluster_snapshot as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "CreateUsageLimit",
            create_usage_limit as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "DescribePartners",
            partner_members as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "AddPartner",
            partner_members as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "DeletePartner",
            partner_members as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "UpdatePartnerStatus",
            update_partner_status as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "CreateCustomDomainAssociation",
            custom_domain_association as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "ModifyCustomDomainAssociation",
            custom_domain_association as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "DeleteCustomDomainAssociation",
            delete_custom_domain_association as Request,
            Wording::NotFoundThenIdentifier,
        ),
        (
            "RegisterNamespace",
            namespace_change as Request,
            Wording::IdentifierCannotBeFound,
        ),
        (
            "DeregisterNamespace",
            namespace_change as Request,
            Wording::IdentifierCannotBeFound,
        ),
        (
            "DescribeClusters",
            cluster_only as Request,
            Wording::IdentifierThenNotFound,
        ),
        (
            "DeleteCluster",
            delete_cluster as Request,
            Wording::IdentifierThenNotFound,
        ),
    ]
}

/// POST one action with owned parameters.
async fn call(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

#[tokio::test]
async fn every_operation_words_its_cluster_not_found_refusal_its_own_way() {
    let emulator = Emulator::new();

    // One cluster that DOES exist, so a refusal below cannot be an "everything
    // is refused" artefact, and one created-then-deleted name: a cluster this
    // account held resolves to nothing once it is gone, exactly like one that
    // never existed.
    let present = emulator.create_minimal("message-present-cluster").await;
    assert_eq!(
        present.status, 200,
        "case seed the present cluster: expected 200, got {} with body {}",
        present.status, present.body
    );
    let doomed = emulator.create_minimal("message-deleted-cluster").await;
    assert_eq!(
        doomed.status, 200,
        "case seed the doomed cluster: expected 200, got {} with body {}",
        doomed.status, doomed.body
    );
    let deleted = call(
        &emulator,
        "DeleteCluster",
        &delete_cluster("message-deleted-cluster"),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the doomed cluster: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );

    // `max_len` is the documented bound on `ClusterIdentifier`; see
    // `state::CLUSTER_IDENTIFIER_RULE`.
    let longest: String = std::iter::once('c')
        .chain(std::iter::repeat_n(
            'x',
            state::CLUSTER_IDENTIFIER_RULE.max_len - 1,
        ))
        .collect();
    let identifiers: Vec<(&str, String)> = vec![
        ("a plain identifier nothing created", "no-such-cluster".to_string()),
        (
            "the shortest identifier the documented rule admits",
            "c".to_string(),
        ),
        ("the longest identifier the documented rule admits", longest),
        (
            "an identifier one character off a stored one",
            "message-present-cluste".to_string(),
        ),
        (
            "an identifier in mixed case",
            "Message-Absent-Cluster".to_string(),
        ),
        (
            "a cluster this account created and then deleted",
            "message-deleted-cluster".to_string(),
        ),
    ];

    for (identifier_case, identifier) in &identifiers {
        for (action, request, wording) in operations() {
            let case = format!("{action} on {identifier_case}");
            let refused = call(&emulator, action, &request(identifier)).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (404, Some("ClusterNotFound".to_string())),
                "case {case}: expected 404 ClusterNotFound, got {} / {:?} with body {}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            let expected = sentence(wording, identifier);
            assert_eq!(
                refused.text("Message").as_deref(),
                Some(expected.as_str()),
                "case {case}: expected the message {expected:?}, got {:?} with body {}",
                refused.text("Message"),
                refused.body
            );
        }
    }

    // The sentences are DISTINGUISHABLE: the same missing identifier, asked
    // about through operations of the three wording classes, comes back worded
    // three different ways rather than collapsed onto one template.
    let shared = "message-collapse-probe";
    let mut answered: Vec<String> = Vec::new();
    for action in ["ModifyCluster", "DescribeClusters", "RegisterNamespace"] {
        let request: Request = if action == "RegisterNamespace" {
            namespace_change
        } else {
            cluster_only
        };
        let refused = call(&emulator, action, &request(shared)).await;
        answered.push(
            refused
                .text("Message")
                .unwrap_or_else(|| panic!("case {action} on {shared}: expected a refusal message, got body {}", refused.body)),
        );
    }
    let mut distinct = answered.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        answered.len(),
        "case the three wording classes: expected three different sentences for one identifier, got {answered:?}"
    );
}

#[tokio::test]
async fn a_cluster_this_account_holds_is_never_refused_as_missing() {
    let emulator = Emulator::new();
    let cases = operations();

    // One cluster per row, created before any row runs, so a row that changes or
    // removes the cluster it names cannot disturb another's — and one
    // provisioning span covers every seed.
    for (index, (action, _, _)) in cases.iter().enumerate() {
        let identifier = format!("served-cluster-{index}");
        let created = emulator.create_minimal(&identifier).await;
        assert_eq!(
            created.status, 200,
            "case seed {action}: expected 200, got {} with body {}",
            created.status, created.body
        );
    }
    settle().await;

    for (index, (action, request, _)) in cases.into_iter().enumerate() {
        let identifier = format!("served-cluster-{index}");
        let answer = call(&emulator, action, &request(&identifier)).await;
        assert_ne!(
            answer.error_code().as_deref(),
            Some("ClusterNotFound"),
            "case {action} on a cluster this account holds: expected the identifier to resolve, \
             got {} with body {}",
            answer.status,
            answer.body
        );
    }
}
