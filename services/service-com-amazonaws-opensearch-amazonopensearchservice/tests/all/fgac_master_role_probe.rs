// SPDX-License-Identifier: CC0-1.0

//! Fine-grained access control on a domain whose MASTER USER is an IAM role: who the
//! cluster serves when the principal is a session of that role.
//!
//! RULE: on a domain with fine-grained access control switched on, the principal
//! `AdvancedSecurityOptions.MasterUserOptions.MasterUserARN` names is granted every cluster
//! and index action, and a principal that is neither that master nor bound by a role
//! mapping is refused `403` with `error.type` `security_exception` — and an AWS principal is
//! that master when the ARN the create named is any of the identities its credential speaks
//! for, which for a role session is the IAM ROLE it is a session of.
//!
//! The sibling suite [`super::domain_rest_data_plane`] pins the same boundary for a master
//! named by a long-term IAM identity; this one probes it for the master form a client
//! provisioning such a domain actually uses — an IAM role it assumes through STS — so the
//! identity that reaches the cluster is a SESSION ARN and the configured master is the
//! role's. Its rows span who asks (the master role's session, that role itself as a producer
//! writes as, a role no mapping binds) and what they ask for (the cluster's health and
//! banner, an index create, a document write, a count, a search).
//!
//! The session and the role's policies live in the STS and IAM stores of the SAME shared
//! engine, so the principal each decision is taken for is the one `sts:GetCallerIdentity`
//! answers for the key the request signed with. The harness is this crate's own
//! ([`super::domain_lifecycle`]) and the data-plane seeding comes from
//! [`super::domain_rest_data_plane`] rather than a second copy of it.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::ingest::{
    self, DocumentWrite,
};

use super::domain_lifecycle::Emulator;
use super::domain_rest_data_plane::{
    delivery_role_arn, endpoint_of, error_type, given_data_plane_policy, given_session, rest,
    SESSION_KEY_ID,
};

/// The domain every case below is created with: fine-grained access control switched on,
/// its three prerequisites met, and the master user named by the ARN of an IAM ROLE.
fn create_body(domain: &str, master_user_arn: &str) -> Value {
    json!({
        "DomainName": domain,
        "EncryptionAtRestOptions": { "Enabled": true },
        "NodeToNodeEncryptionOptions": { "Enabled": true },
        "DomainEndpointOptions": { "EnforceHTTPS": true },
        "AdvancedSecurityOptions": {
            "Enabled": true,
            "InternalUserDatabaseEnabled": false,
            "MasterUserOptions": { "MasterUserARN": master_user_arn }
        }
    })
}

/// The routes the boundary covers, as `(case, method, path, body)`: the cluster's own
/// reads, an index create, a document write, a count and a search.
fn routes() -> Vec<(&'static str, &'static str, &'static str, Option<Value>)> {
    vec![
        ("the cluster health", "GET", "/_cluster/health", None),
        ("the cluster banner", "GET", "/", None),
        ("an index create", "PUT", "/catalog", None),
        (
            "a document write",
            "PUT",
            "/catalog/_doc/one",
            Some(json!({ "title": "a" })),
        ),
        ("a count", "GET", "/catalog/_count", None),
        ("a search", "POST", "/catalog/_search", Some(json!({}))),
    ]
}

/// RULE: a request signed as a SESSION of the role the create named as master user is the
/// master's, so every route the boundary covers serves it; a principal no mapping binds is
/// refused `403 security_exception` on each of them.
#[tokio::test]
async fn a_session_of_the_master_role_is_the_master_the_cluster_serves() {
    let emulator = Emulator::new();
    let created = emulator
        .create(create_body("role-mastered", &delivery_role_arn()))
        .await;
    assert_eq!(
        created.status, 200,
        "case \"CreateDomain with an IAM role as master user\": expected 200, got {} with \
         {:?}",
        created.status, created.body
    );
    let host = endpoint_of(&emulator, "role-mastered");
    // The session the master role's credential speaks for, and the role's own IAM grant for
    // this service's data-plane actions, so what is left to decide is the cluster's part.
    given_session(&emulator);
    given_data_plane_policy(&emulator);

    for (case, method, path, body) in routes() {
        let answered = rest(&emulator, SESSION_KEY_ID, &host, method, path, body).await;
        assert!(
            (200..300).contains(&answered.status),
            "case {case:?}: expected the master role's session to be served, got {} with {:?}",
            answered.status,
            answered.body
        );
    }

    // The master is also told it holds the all-permitting role, by the one route the plugin
    // answers for every authenticated caller.
    let who = rest(
        &emulator,
        SESSION_KEY_ID,
        &host,
        "GET",
        "/_plugins/_security/authinfo",
        None,
    )
    .await;
    assert_eq!(
        (who.status, who.body["roles"].clone()),
        (200, json!(["all_access"])),
        "case \"the master role's session asking who it is\": got {} with {:?}",
        who.status,
        who.body
    );

    // And a document a producer delivers AS THE ROLE — the identity the create named, not a
    // session of it — lands too: the master is the role, however its credential spells it.
    let delivered = ingest::index_document(
        &**emulator.engine(),
        &DocumentWrite {
            domain: &host,
            index_name: "catalog",
            document_id: Some("delivered"),
            document: br#"{"title":"delivered"}"#,
            principal_arn: &delivery_role_arn(),
            now: SystemTime::now(),
        },
    );
    assert!(
        delivered.is_ok(),
        "case \"a producer delivering as the master role\": expected it to land, got \
         {delivered:?}"
    );
}

/// RULE: the same domain refuses a principal no mapping binds `403 security_exception` on
/// every route — the other half of the boundary, with the master named by a role.
#[tokio::test]
async fn a_role_no_mapping_binds_is_refused_every_route() {
    let emulator = Emulator::new();
    // The master is a role NOBODY holds a session of here, so the seeded session below is
    // an unmapped principal on this domain.
    let master = format!(
        "arn:aws:iam::{}:role/search-master",
        ::engine::emulator_account_id()
    );
    let created = emulator.create(create_body("role-guarded", &master)).await;
    assert_eq!(
        created.status, 200,
        "case \"CreateDomain with another role as master user\": expected 200, got {} with \
         {:?}",
        created.status, created.body
    );
    let host = endpoint_of(&emulator, "role-guarded");
    given_session(&emulator);
    given_data_plane_policy(&emulator);

    for (case, method, path, body) in routes() {
        let answered = rest(&emulator, SESSION_KEY_ID, &host, method, path, body).await;
        assert_eq!(
            (answered.status, error_type(&answered)),
            (403, "security_exception".to_string()),
            "case {case:?}: expected the cluster's refusal of an unmapped principal, got {} \
             with {:?}",
            answered.status,
            answered.body
        );
    }
}
