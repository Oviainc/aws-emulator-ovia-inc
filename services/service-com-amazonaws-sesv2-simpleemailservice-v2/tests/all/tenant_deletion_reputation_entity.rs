// SPDX-License-Identifier: CC0-1.0

//! What the reputation-entity surface reports once a tenant is deleted, end to end over
//! HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised; the tenant calls are the sibling
//! [`crate::tenant_resource_association_removal`] suite's.
//!
//! The rule these cases pin: a view the service derives from a resource record is
//! computed from that record on every read, so once `DeleteTenant` removes the tenant
//! record nothing derived from it answers for it any more. Applied to the reputation
//! entity a tenant is tracked under — no request creates one, it exists for as long as
//! the resource it references does — the account's listing stops reporting the deleted
//! tenant's entity, a read of that reference and a write of its customer-managed status
//! are each refused as invalid input, and a tenant recreated under the same name is
//! tracked as an entity of its own that carries none of the deleted tenant's status.
//! Every other tenant's entity is untouched — including one carrying a status the account
//! wrote before the delete, so the survivors are checked against something other than the
//! status a tenant starts with.
//!
//! The table below spans what the rule generalises over: how many tenants the account
//! holds (one, several), which of them is deleted (first, last, in between), the form of
//! the tenant name (shortest allowed, the documented 64-character bound, every documented
//! character class), and whether a customer-managed sending status was written on the
//! deleted tenant's entity before the delete — each `SendingStatus` a caller may write,
//! and none at all.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};
use crate::contact_lists::query_escaped;
use crate::tenant_resource_association_removal::Associations;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state;

/// The one `ReputationEntityType` the model binds, taken from the crate's own constant
/// so the value is stated once
/// (`smithy_query(query="shape",shape="ReputationEntityType")`: `RESOURCE`).
const RESOURCE: &str = state::REPUTATION_ENTITY_TYPE_RESOURCE;

/// The longest `TenantName` SES accepts, taken from the crate's own constant so the
/// bound is stated once: its documentation states the name "can contain up to 64
/// alphanumeric characters, including letters, numbers, hyphens (-) and underscores (_)
/// only" (`smithy_query(query="shape",shape="TenantName")`).
const MAX_TENANT_NAME_CHARS: usize = state::MAX_TENANT_NAME_CHARS;

/// The `SendingStatus` a tenant's customer-managed record carries until the account
/// writes one, taken from the crate's own constant for exactly that invariant so the
/// value is stated once.
const FRESH_TENANT_STATUS: &str = state::TENANT_SENDING_STATUS_CREATED;

/// The reputation-entity and tenant-deletion calls this suite adds to the shared tenant
/// calls.
trait Entities {
    async fn delete_tenant(&self, name: &str) -> Reply;
    async fn list_entities(&self) -> Reply;
    async fn get_entity(&self, reference: &str) -> Reply;
    async fn write_status(&self, reference: &str, status: &str) -> Reply;
}

impl Entities for Ses {
    async fn delete_tenant(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants/delete",
            Some(json!({"TenantName": name})),
        )
        .await
    }

    async fn list_entities(&self) -> Reply {
        self.call("POST", "/v2/email/reputation/entities", Some(json!({})))
            .await
    }

    async fn get_entity(&self, reference: &str) -> Reply {
        // `ReputationEntityReference` is a non-greedy `@httpLabel`, so an ARN reaches the
        // service percent-encoded, the way an SDK escapes a label.
        self.call(
            "GET",
            &format!(
                "/v2/email/reputation/entities/{RESOURCE}/{}",
                query_escaped(reference)
            ),
            None,
        )
        .await
    }

    async fn write_status(&self, reference: &str, status: &str) -> Reply {
        self.call(
            "PUT",
            &format!(
                "/v2/email/reputation/entities/{RESOURCE}/{}/customer-managed-status",
                query_escaped(reference)
            ),
            Some(json!({"SendingStatus": status})),
        )
        .await
    }
}

/// The `ReputationEntityReference` of every entity a listing reported, in the order
/// reported.
fn references(reply: &Reply) -> Vec<String> {
    reply
        .member("ReputationEntities")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entity| at(entity, "/ReputationEntityReference"))
        .collect()
}

/// The string at `pointer` inside a reported entity, or `""` when absent.
fn at(entity: &Value, pointer: &str) -> String {
    entity
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The entity a `GetReputationEntity` reply reported.
fn read_entity(reply: &Reply) -> Value {
    reply.member("ReputationEntity")
}

/// One case: the tenants the account holds, which one is deleted, and the
/// customer-managed status written on its entity first.
struct Case {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `TenantName` of every tenant created before the delete.
    tenants: Vec<String>,
    /// The one of them the delete names.
    target: String,
    /// The `SendingStatus` written on the target's entity before the delete, or `None`
    /// when the case writes none.
    written: Option<&'static str>,
}

/// The table: the tenant count, which tenant is deleted, the form of its name, and each
/// `SendingStatus` a caller may write on its entity beforehand
/// (`smithy_query(query="shape",shape="SendingStatus")`, taken from the crate's own
/// constants).
fn cases() -> Vec<Case> {
    let longest = format!(
        "tenant-{}",
        "x".repeat(MAX_TENANT_NAME_CHARS - "tenant-".len())
    );
    vec![
        Case {
            case: "the only tenant the account holds, under the shortest name allowed",
            tenants: vec!["a".to_string()],
            target: "a".to_string(),
            written: None,
        },
        Case {
            case: "the first of several tenants, with sending disabled before the delete",
            tenants: vec![
                "alpha-one".to_string(),
                "beta_two".to_string(),
                "Gamma3".to_string(),
            ],
            target: "alpha-one".to_string(),
            written: Some(state::SENDING_STATUS_DISABLED),
        },
        Case {
            case: "the last of several tenants, reinstated before the delete",
            tenants: vec![
                "alpha-one".to_string(),
                "beta_two".to_string(),
                "Gamma3".to_string(),
            ],
            target: "Gamma3".to_string(),
            written: Some(state::SENDING_STATUS_REINSTATED),
        },
        Case {
            case: "a tenant between two others, with the status it already carried written",
            tenants: vec![
                "alpha-one".to_string(),
                "beta_two".to_string(),
                "Gamma3".to_string(),
            ],
            target: "beta_two".to_string(),
            written: Some(state::SENDING_STATUS_ENABLED),
        },
        Case {
            case: "a tenant whose name is at the documented 64-character bound",
            tenants: vec![longest.clone(), "survivor".to_string()],
            target: longest,
            written: Some(state::SENDING_STATUS_DISABLED),
        },
    ]
}

#[tokio::test]
async fn a_deleted_tenants_reputation_entity_goes_with_the_tenant() {
    for Case {
        case,
        tenants,
        target,
        written,
    } in cases()
    {
        let ses = Ses::new();

        // Each tenant's entity is referenced by the ARN its create minted, so the
        // reference is read off the create's own answer, never constructed here.
        let mut created_references = Vec::new();
        for name in &tenants {
            let created = ses.create_tenant(name).await;
            assert_eq!(
                created.status,
                200,
                "case {case:?}: expected HTTP 200 creating tenant {name:?}, got {} {:?}",
                created.status,
                created.message()
            );
            let reference = created
                .member("TenantArn")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !reference.is_empty(),
                "case {case:?}: expected the create of {name:?} to mint the tenant's ARN, got {:?}",
                created.member("TenantArn")
            );
            created_references.push((name.clone(), reference));
        }
        let target_reference = created_references
            .iter()
            .find(|(name, _)| *name == target)
            .map(|(_, reference)| reference.clone())
            .unwrap_or_default();

        // One surviving tenant carries a NON-default status across the delete, so the
        // "every other entity is untouched" leg is not a default compared against a
        // default: a store that dropped the write, or that let the delete reach another
        // record, reports `ENABLED` here instead.
        let survivor_written = created_references
            .iter()
            .find(|(name, _)| *name != target)
            .cloned();
        if let Some((name, reference)) = &survivor_written {
            let write = ses
                .write_status(reference, state::SENDING_STATUS_DISABLED)
                .await;
            assert_eq!(
                write.status,
                200,
                "case {case:?}: expected HTTP 200 writing the status of {name:?}, got {} {:?}",
                write.status,
                write.message()
            );
        }

        if let Some(status) = written {
            let write = ses.write_status(&target_reference, status).await;
            assert_eq!(
                write.status,
                200,
                "case {case:?}: expected HTTP 200 writing the status {status:?}, got {} {:?}",
                write.status,
                write.message()
            );
            let read = ses.get_entity(&target_reference).await;
            assert_eq!(
                at(&read_entity(&read), "/CustomerManagedStatus/Status"),
                status,
                "case {case:?}: expected the entity to carry the status just written, got {:?}",
                read_entity(&read)
            );
        }

        let mut expected_before: Vec<String> = created_references
            .iter()
            .map(|(_, reference)| reference.clone())
            .collect();
        expected_before.sort();
        assert_eq!(
            references(&ses.list_entities().await),
            expected_before,
            "case {case:?}: expected every tenant's entity before the delete, got {:?}",
            references(&ses.list_entities().await)
        );

        let deleted = ses.delete_tenant(&target).await;
        assert_eq!(
            deleted.status,
            200,
            "case {case:?}: expected HTTP 200 deleting the tenant, got {} {:?}",
            deleted.status,
            deleted.message()
        );

        // The listing is recomputed from the records that are left, so the deleted
        // tenant's entity is simply not among them, in the same reference order.
        let mut expected_after: Vec<String> = created_references
            .iter()
            .filter(|(name, _)| *name != target)
            .map(|(_, reference)| reference.clone())
            .collect();
        expected_after.sort();
        let listed = ses.list_entities().await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: expected HTTP 200 listing the entities, got {} {:?}",
            listed.status,
            listed.message()
        );
        assert_eq!(
            references(&listed),
            expected_after,
            "case {case:?}: expected the deleted tenant's entity to be gone and every other \
             one kept, got {:?}",
            references(&listed)
        );

        // A reference that resolves to no resource names no entity at all: the read and
        // the status write are both refused as invalid input, with the sentence that
        // names the reference they could not resolve.
        let read = ses.get_entity(&target_reference).await;
        assert_eq!(
            read.status,
            400,
            "case {case:?}: expected HTTP 400 reading the deleted tenant's entity, got {} {:?}",
            read.status,
            read.message()
        );
        assert_eq!(
            read.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException from the read, got {:?} {:?}",
            read.error_code(),
            read.message()
        );
        assert_eq!(
            read.message(),
            state::reputation_entity_not_found(&target_reference),
            "case {case:?}: expected the refusal to name the reference it did not resolve, \
             got {:?}",
            read.message()
        );
        let write = ses
            .write_status(&target_reference, state::SENDING_STATUS_DISABLED)
            .await;
        assert_eq!(
            write.status,
            400,
            "case {case:?}: expected HTTP 400 writing the deleted tenant's status, got {} {:?}",
            write.status,
            write.message()
        );
        assert_eq!(
            write.message(),
            state::reputation_entity_not_found(&target_reference),
            "case {case:?}: expected the refused write to name the reference it did not \
             resolve, got {:?}",
            write.message()
        );

        // Every surviving tenant's entity reads exactly as it did: the delete reached one
        // record only, so the survivor whose status was written keeps it and the rest keep
        // the status a tenant carries until the account writes one.
        for (name, reference) in created_references
            .iter()
            .filter(|(name, _)| *name != target)
        {
            let expected = match &survivor_written {
                Some((written_name, _)) if written_name == name => {
                    state::SENDING_STATUS_DISABLED
                }
                _ => FRESH_TENANT_STATUS,
            };
            let survivor = ses.get_entity(reference).await;
            assert_eq!(
                survivor.status,
                200,
                "case {case:?}: expected HTTP 200 reading the entity of {name:?}, got {} {:?}",
                survivor.status,
                survivor.message()
            );
            assert_eq!(
                at(&read_entity(&survivor), "/CustomerManagedStatus/Status"),
                expected,
                "case {case:?}: expected {name:?} to keep the status {expected:?} it carried, \
                 got {:?}",
                read_entity(&survivor)
            );
        }

        // The name is free again, and the tenant created under it is tracked as its own
        // entity: a new reference, and none of the deleted tenant's status.
        let recreated = ses.create_tenant(&target).await;
        assert_eq!(
            recreated.status,
            200,
            "case {case:?}: expected the deleted name to be free again, got {} {:?}",
            recreated.status,
            recreated.message()
        );
        let new_reference = recreated
            .member("TenantArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            new_reference, target_reference,
            "case {case:?}: expected the recreated tenant to be its own record, got the \
             reference {new_reference:?} twice"
        );
        let reread = ses.get_entity(&new_reference).await;
        assert_eq!(
            reread.status,
            200,
            "case {case:?}: expected HTTP 200 reading the recreated tenant's entity, got {} {:?}",
            reread.status,
            reread.message()
        );
        assert_eq!(
            at(&read_entity(&reread), "/CustomerManagedStatus/Status"),
            FRESH_TENANT_STATUS,
            "case {case:?}: expected the recreated tenant to carry none of the deleted \
             tenant's status, got {:?}",
            read_entity(&reread)
        );
        let after_recreate = references(&ses.list_entities().await);
        assert!(
            after_recreate.contains(&new_reference) && !after_recreate.contains(&target_reference),
            "case {case:?}: expected the listing to report the recreated tenant's entity and \
             not the deleted one's, got {after_recreate:?}"
        );
    }
}
