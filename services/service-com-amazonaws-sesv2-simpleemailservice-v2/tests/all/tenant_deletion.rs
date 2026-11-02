// SPDX-License-Identifier: CC0-1.0

//! Deleting a tenant, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised; the tenant-and-resource set-up calls are the sibling
//! [`crate::tenant_resource_association_removal`] suite's, so both suites drive one
//! surface the same way.
//!
//! The rule these cases pin: `DeleteTenant` resolves the `TenantName` it is given and
//! then removes that tenant UNCONDITIONALLY — the operation models no in-use or conflict
//! error, so nothing the tenant holds can refuse the call — answering HTTP 200 with an
//! empty body, with the resources it was associated with left in place. Afterwards every
//! view keyed by the deleted tenant resolves that name to the modelled terminal 404
//! (`GetTenant`, `ListTenantResources`, a read of its suppression list) and the account's
//! tenant listing no longer reports it, while the RESOURCE-keyed view of an association
//! it held still names it: the two views are deliberately not inverses after a delete.
//! The name is free again, and the tenant created under it holds none of the deleted
//! tenant's state. A name the account does not hold is refused 404, a name outside the
//! documented form 400, and either way nothing is removed.
//!
//! The tables below span what the rule generalises over: how many resources the deleted
//! tenant held (none, one, several), the kinds those resources are (each
//! `ResourceType` this emulator can be made to hold), how many other tenants share one
//! of them, whether the tenant held suppression-list entries of its own, the form of the
//! tenant name (shortest allowed, the documented 64-character bound, every documented
//! character class), the deleted tenant's position among the ones the account holds, and
//! each refusal leg.

use serde_json::Value;

use crate::configuration_sets::{Reply, Ses};
use crate::tenant_resource_association_removal::{
    listed_resources, listed_tenants, Associations, Resource,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state;

/// The longest `TenantName` SES accepts, taken from the crate's own constant so the
/// bound is stated once: its documentation states the name "can contain up to 64
/// alphanumeric characters, including letters, numbers, hyphens (-) and underscores (_)
/// only" (`smithy_query(query="shape",shape="TenantName")`).
const MAX_TENANT_NAME_CHARS: usize = state::MAX_TENANT_NAME_CHARS;

/// The tenant every case keeps beside the one it deletes: it shares every resource the
/// deleted tenant held, so each case shows the delete reached one tenant only.
const BYSTANDER: &str = "bystander-tenant";

/// The tenant-lifecycle and suppression-list calls this suite adds to the shared
/// association calls.
trait Lifecycle {
    async fn delete_tenant(&self, name: &str) -> Reply;
    async fn list_tenants(&self) -> Reply;
    async fn suppress(&self, address: &str, tenant: &str) -> Reply;
    async fn get_suppressed(&self, address: &str, tenant: &str) -> Reply;
    async fn list_suppressed(&self, tenant: &str) -> Reply;
}

impl Lifecycle for Ses {
    async fn delete_tenant(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants/delete",
            Some(serde_json::json!({"TenantName": name})),
        )
        .await
    }

    async fn list_tenants(&self) -> Reply {
        self.call("POST", "/v2/email/tenants/list", Some(serde_json::json!({})))
            .await
    }

    async fn suppress(&self, address: &str, tenant: &str) -> Reply {
        self.call(
            "PUT",
            "/v2/email/suppression/addresses",
            Some(serde_json::json!({
                "EmailAddress": address,
                "Reason": "BOUNCE",
                "TenantName": tenant
            })),
        )
        .await
    }

    async fn get_suppressed(&self, address: &str, tenant: &str) -> Reply {
        // `EmailAddress` is an `@httpLabel` and `TenantName` an `@httpQuery`.
        self.call(
            "GET",
            &format!("/v2/email/suppression/addresses/{address}?TenantName={tenant}"),
            None,
        )
        .await
    }

    async fn list_suppressed(&self, tenant: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/suppression/addresses?TenantName={tenant}"),
            None,
        )
        .await
    }
}

/// The `TenantName` of every tenant the account's listing reported, in the order
/// reported.
fn listed_tenant_names(reply: &Reply) -> Vec<String> {
    rows(reply, "Tenants")
        .iter()
        .map(|row| text(row, "TenantName"))
        .collect()
}

/// The `(TenantName, TenantId)` of every tenant a resource's listing reported, in the
/// order reported: the id is what says WHICH tenant record of that name a reported
/// association was made against.
fn listed_tenant_identities(reply: &Reply) -> Vec<(String, String)> {
    rows(reply, "ResourceTenants")
        .iter()
        .map(|row| (text(row, "TenantName"), text(row, "TenantId")))
        .collect()
}

/// The rows a list member reported, or none when the member is absent.
fn rows(reply: &Reply, member: &str) -> Vec<Value> {
    reply.member(member).as_array().cloned().unwrap_or_default()
}

/// A string member of one reported row, or `""` when absent.
fn text(row: &Value, member: &str) -> String {
    row.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A string member of a reply's body, or `""` when absent.
fn body_text(reply: &Reply, member: &str) -> String {
    reply.member(member).as_str().unwrap_or_default().to_string()
}

/// One accepted delete: the tenant to delete, what it holds when the delete arrives.
struct DeletionCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `TenantName` to create and then delete.
    tenant_name: String,
    /// The resources the tenant holds at the moment of the delete.
    held: Vec<Resource>,
    /// The addresses on the tenant's own suppression list when the delete arrives.
    suppressed: Vec<&'static str>,
}

/// The accepted-delete table: a tenant holding nothing, one resource and several, across
/// the resource kinds this emulator can hold, against tenant names spanning the
/// documented form from its shortest to its 64-character bound.
fn deletion_cases() -> Vec<DeletionCase> {
    vec![
        DeletionCase {
            case: "a tenant holding nothing at all, and the shortest name the model allows",
            tenant_name: "a".to_string(),
            held: Vec::new(),
            suppressed: Vec::new(),
        },
        DeletionCase {
            case: "a tenant still holding one configuration set",
            tenant_name: "Tenant_01-beta".to_string(),
            held: vec![Resource::ConfigurationSet("nightly")],
            suppressed: Vec::new(),
        },
        DeletionCase {
            case: "a tenant still holding several resources of both kinds",
            tenant_name: "multi_tenant-9".to_string(),
            held: vec![
                Resource::ConfigurationSet("promo_set-2"),
                Resource::EmailIdentity("sender@example.com"),
                Resource::EmailIdentity("example.com"),
            ],
            suppressed: Vec::new(),
        },
        DeletionCase {
            case: "a tenant holding a resource and entries on its own suppression list",
            tenant_name: "suppressing-tenant".to_string(),
            held: vec![Resource::ConfigurationSet("shared_set")],
            suppressed: vec!["bounced@example.com", "complained@example.net"],
        },
        DeletionCase {
            case: "a tenant whose name is at the documented 64-character bound",
            tenant_name: format!(
                "tenant-{}",
                "x".repeat(MAX_TENANT_NAME_CHARS - "tenant-".len())
            ),
            held: vec![Resource::EmailIdentity("bounds@example.org")],
            suppressed: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn a_tenant_is_deleted_whatever_it_holds_and_only_its_own_views_go() {
    for DeletionCase {
        case,
        tenant_name,
        held,
        suppressed,
    } in deletion_cases()
    {
        let ses = Ses::new();
        for name in [tenant_name.as_str(), BYSTANDER] {
            let created = ses.create_tenant(name).await;
            assert_eq!(
                created.status,
                200,
                "case {case:?}: expected HTTP 200 creating tenant {name:?}, got {} {:?}",
                created.status,
                created.message()
            );
        }

        // Every held resource is associated with the tenant under test AND with the
        // bystander, so the delete can be shown to reach one tenant's view only.
        let mut held_arns = Vec::new();
        for resource in &held {
            resource.create(&ses, case).await;
            let arn = resource.arn();
            for name in [tenant_name.as_str(), BYSTANDER] {
                let associated = ses.associate(name, &arn).await;
                assert_eq!(
                    associated.status,
                    200,
                    "case {case:?}: expected HTTP 200 associating {arn} with {name:?}, got {} {:?}",
                    associated.status,
                    associated.message()
                );
            }
            held_arns.push(arn);
        }
        held_arns.sort();
        if !suppressed.is_empty() {
            // A suppression write is refused while the account is in the sandbox, which
            // is not what this case is about.
            ses.grant_production_access();
        }
        for address in &suppressed {
            let put = ses.suppress(address, &tenant_name).await;
            assert_eq!(
                put.status,
                200,
                "case {case:?}: expected HTTP 200 suppressing {address} for the tenant, got {} {:?}",
                put.status,
                put.message()
            );
        }

        let before = ses.list_tenant_resources(&tenant_name).await;
        assert_eq!(
            listed_resources(&before),
            held_arns,
            "case {case:?}: expected the tenant to report what it holds before the delete, got {:?}",
            listed_resources(&before)
        );

        // The delete itself: an empty 200, whatever the tenant still holds.
        let deleted = ses.delete_tenant(&tenant_name).await;
        assert_eq!(
            deleted.status,
            200,
            "case {case:?}: expected HTTP 200 deleting the tenant, got {} {:?}",
            deleted.status,
            deleted.message()
        );
        assert!(
            deleted.is_empty_body(),
            "case {case:?}: expected an empty response body, got {:?}",
            deleted.member("")
        );

        // Every tenant-keyed view of the deleted name now resolves to the modelled 404.
        let read = ses.get_tenant(&tenant_name).await;
        assert_eq!(
            read.status,
            404,
            "case {case:?}: expected GetTenant to report the tenant missing, got {} {:?}",
            read.status,
            read.message()
        );
        assert_eq!(
            read.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException from GetTenant, got {:?} {:?}",
            read.error_code(),
            read.message()
        );
        assert_eq!(
            read.message(),
            format!("The requested tenant <{tenant_name}> does not exist."),
            "case {case:?}: expected the refusal to name the tenant it did not find, got {:?}",
            read.message()
        );
        let tenant_resources = ses.list_tenant_resources(&tenant_name).await;
        assert_eq!(
            tenant_resources.status,
            404,
            "case {case:?}: expected ListTenantResources to report the tenant missing, got {} {:?}",
            tenant_resources.status,
            tenant_resources.message()
        );
        assert_eq!(
            tenant_resources.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException from ListTenantResources, got {:?} {:?}",
            tenant_resources.error_code(),
            tenant_resources.message()
        );
        let listing = ses.list_tenants().await;
        assert_eq!(
            listed_tenant_names(&listing),
            vec![BYSTANDER.to_string()],
            "case {case:?}: expected the account's listing to report only the surviving \
             tenant, got {:?}",
            listed_tenant_names(&listing)
        );
        for address in &suppressed {
            let entry = ses.get_suppressed(address, &tenant_name).await;
            assert_eq!(
                entry.status,
                404,
                "case {case:?}: expected the deleted tenant's suppression list to be \
                 unreachable for {address}, got {} {:?}",
                entry.status,
                entry.message()
            );
            assert_eq!(
                entry.message(),
                format!("The requested tenant <{tenant_name}> does not exist."),
                "case {case:?}: expected the suppression read to report the tenant missing, \
                 got {:?}",
                entry.message()
            );
        }

        // The resource-keyed view is NOT cascaded: it still names the deleted tenant,
        // beside the tenant that survived. And the resources themselves are untouched.
        for (resource, arn) in held.iter().zip(held_arns.iter()) {
            let resource_tenants = ses.list_resource_tenants(arn).await;
            assert_eq!(
                resource_tenants.status,
                200,
                "case {case:?}: expected HTTP 200 listing the tenants of {arn}, got {} {:?}",
                resource_tenants.status,
                resource_tenants.message()
            );
            let mut expected = vec![BYSTANDER.to_string(), tenant_name.clone()];
            expected.sort();
            assert_eq!(
                listed_tenants(&resource_tenants),
                expected,
                "case {case:?}: expected {arn} to still name the deleted tenant, got {:?}",
                listed_tenants(&resource_tenants)
            );
            let read_back = resource.read(&ses).await;
            assert_eq!(
                read_back.status,
                200,
                "case {case:?}: expected {arn} to survive the delete, got {} {:?}",
                read_back.status,
                read_back.message()
            );
        }

        // The tenant that shared every resource keeps every association it held.
        let bystander_resources = ses.list_tenant_resources(BYSTANDER).await;
        assert_eq!(
            listed_resources(&bystander_resources),
            held_arns,
            "case {case:?}: expected the surviving tenant to keep what it holds, got {:?}",
            listed_resources(&bystander_resources)
        );

        // "Delete an existing tenant": the tenant existing is the one condition, so a
        // second delete of the same name is the modelled 404.
        let again = ses.delete_tenant(&tenant_name).await;
        assert_eq!(
            again.status,
            404,
            "case {case:?}: expected the second delete to report the tenant missing, got {} {:?}",
            again.status,
            again.message()
        );
        assert_eq!(
            again.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException from the second delete, got {:?} {:?}",
            again.error_code(),
            again.message()
        );
    }
}

#[tokio::test]
async fn only_the_named_tenant_goes_whichever_of_the_held_ones_it_is() {
    // The rule is about the tenant the call NAMES, so the rows vary which of the tenants
    // the account holds is deleted — first, last and in between — across the form and
    // length of a name the service admits.
    let longest = "n".repeat(MAX_TENANT_NAME_CHARS);
    let held = ["a", "marketing-eu", "marketing_eu", "Tenant2026", &longest];

    for target in held {
        let ses = Ses::new();
        for name in held {
            let created = ses.create_tenant(name).await;
            assert_eq!(
                created.status,
                200,
                "case {target:?}: expected HTTP 200 creating tenant {name:?}, got {} {:?}",
                created.status,
                created.message()
            );
        }

        let deleted = ses.delete_tenant(target).await;
        assert_eq!(
            deleted.status,
            200,
            "case {target:?}: expected HTTP 200 deleting the tenant, got {} {:?}",
            deleted.status,
            deleted.message()
        );

        let mut expected: Vec<String> = held
            .iter()
            .filter(|name| **name != target)
            .map(|name| (*name).to_string())
            .collect();
        expected.sort();
        let mut reported = listed_tenant_names(&ses.list_tenants().await);
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {target:?}: expected every other tenant to be left alone, got {reported:?}"
        );
        let read = ses.get_tenant(target).await;
        assert_eq!(
            read.status,
            404,
            "case {target:?}: expected the deleted tenant to read as missing, got {} {:?}",
            read.status,
            read.message()
        );
    }
}

#[tokio::test]
async fn a_deleted_name_is_free_again_and_the_new_tenant_holds_none_of_the_old_state() {
    let case = "a tenant deleted while holding a resource and a suppression entry, recreated";
    let ses = Ses::new();
    let name = "recycled_tenant";
    let address = "bounced@example.com";
    let resource = Resource::ConfigurationSet("recycled-set");
    let arn = resource.arn();
    resource.create(&ses, case).await;

    let first = ses.create_tenant(name).await;
    assert_eq!(
        first.status,
        200,
        "case {case:?}: expected HTTP 200 creating the tenant, got {} {:?}",
        first.status,
        first.message()
    );
    let first_id = body_text(&first, "TenantId");
    let associated = ses.associate(name, &arn).await;
    assert_eq!(
        associated.status,
        200,
        "case {case:?}: expected HTTP 200 associating the resource, got {} {:?}",
        associated.status,
        associated.message()
    );
    // A suppression write is refused while the account is in the sandbox, which is not
    // what this case is about.
    ses.grant_production_access();
    let put = ses.suppress(address, name).await;
    assert_eq!(
        put.status,
        200,
        "case {case:?}: expected HTTP 200 suppressing the address, got {} {:?}",
        put.status,
        put.message()
    );

    let deleted = ses.delete_tenant(name).await;
    assert_eq!(
        deleted.status,
        200,
        "case {case:?}: expected HTTP 200 deleting the tenant, got {} {:?}",
        deleted.status,
        deleted.message()
    );

    // The name is free: the create is accepted and mints a tenant of its own.
    let second = ses.create_tenant(name).await;
    assert_eq!(
        second.status,
        200,
        "case {case:?}: expected the name to be free again, got {} {:?}",
        second.status,
        second.message()
    );
    let second_id = body_text(&second, "TenantId");
    assert_ne!(
        second_id, first_id,
        "case {case:?}: expected the recreated tenant to be its own record, got the id \
         {second_id:?} twice"
    );

    // The dangling reverse-index row still names the DELETED tenant record — its id is
    // the deleted tenant's, not the new one's — and the new tenant holds nothing.
    let resource_tenants = ses.list_resource_tenants(&arn).await;
    assert_eq!(
        listed_tenant_identities(&resource_tenants),
        vec![(name.to_string(), first_id.clone())],
        "case {case:?}: expected the resource to still name the deleted tenant record, got {:?}",
        listed_tenant_identities(&resource_tenants)
    );
    let tenant_resources = ses.list_tenant_resources(name).await;
    assert_eq!(
        tenant_resources.status,
        200,
        "case {case:?}: expected HTTP 200 listing the recreated tenant's resources, got {} {:?}",
        tenant_resources.status,
        tenant_resources.message()
    );
    assert!(
        listed_resources(&tenant_resources).is_empty(),
        "case {case:?}: expected the recreated tenant to hold nothing, got {:?}",
        listed_resources(&tenant_resources)
    );

    // Nor does it inherit the deleted tenant's suppression list.
    let entry = ses.get_suppressed(address, name).await;
    assert_eq!(
        entry.status,
        404,
        "case {case:?}: expected the recreated tenant not to hold the old entry, got {} {:?}",
        entry.status,
        entry.message()
    );
    let suppression_list = ses.list_suppressed(name).await;
    assert_eq!(
        suppression_list.status,
        200,
        "case {case:?}: expected HTTP 200 listing the recreated tenant's suppression list, \
         got {} {:?}",
        suppression_list.status,
        suppression_list.message()
    );
    assert!(
        rows(&suppression_list, "SuppressedDestinationSummaries").is_empty(),
        "case {case:?}: expected the recreated tenant's suppression list to be empty, got {:?}",
        suppression_list.member("SuppressedDestinationSummaries")
    );

    // The association the deleted tenant held is not one the new tenant holds, so making
    // it again is accepted, and both views then agree it is the NEW record's.
    let reassociated = ses.associate(name, &arn).await;
    assert_eq!(
        reassociated.status,
        200,
        "case {case:?}: expected the recreated tenant to be able to hold the resource, got {} {:?}",
        reassociated.status,
        reassociated.message()
    );
    let after = ses.list_resource_tenants(&arn).await;
    assert_eq!(
        listed_tenant_identities(&after),
        vec![(name.to_string(), second_id.clone())],
        "case {case:?}: expected the resource to name the recreated tenant record, got {:?}",
        listed_tenant_identities(&after)
    );
    let held = ses.list_tenant_resources(name).await;
    assert_eq!(
        listed_resources(&held),
        vec![arn.clone()],
        "case {case:?}: expected the recreated tenant to report the resource it now holds, \
         got {:?}",
        listed_resources(&held)
    );
}

/// One refused delete: the name the call carries, and the error it must be answered
/// with.
struct RefusalCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `TenantName` the refused call carries.
    tenant_name: String,
    /// The HTTP status of the refusal.
    status: u16,
    /// The modelled error shape the refusal names.
    error_code: &'static str,
    /// The sentence the refusal carries.
    message: String,
}

/// The name of the one tenant every refusal row is set up against.
const LIVE_TENANT: &str = "live-tenant";

/// The refusal table: a name that resolves to no tenant, and input outside the
/// documented form.
fn refusal_cases() -> Vec<RefusalCase> {
    let over_long_name = "t".repeat(MAX_TENANT_NAME_CHARS + 1);
    vec![
        RefusalCase {
            case: "a tenant the account never created",
            tenant_name: "ghost-tenant".to_string(),
            status: 404,
            error_code: "NotFoundException",
            message: "The requested tenant <ghost-tenant> does not exist.".to_string(),
        },
        RefusalCase {
            case: "a name differing from a real tenant's only in case",
            tenant_name: "Live-Tenant".to_string(),
            status: 404,
            error_code: "NotFoundException",
            message: "The requested tenant <Live-Tenant> does not exist.".to_string(),
        },
        RefusalCase {
            case: "a name one character past the documented bound",
            tenant_name: over_long_name.clone(),
            status: 400,
            error_code: "BadRequestException",
            message: state::invalid_tenant_name(&over_long_name),
        },
        RefusalCase {
            case: "a name carrying a character the documented form excludes",
            tenant_name: "live.tenant".to_string(),
            status: 400,
            error_code: "BadRequestException",
            message: state::invalid_tenant_name("live.tenant"),
        },
        RefusalCase {
            case: "a name carrying a space",
            tenant_name: "live tenant".to_string(),
            status: 400,
            error_code: "BadRequestException",
            message: state::invalid_tenant_name("live tenant"),
        },
    ]
}

#[tokio::test]
async fn a_name_that_resolves_to_no_tenant_is_refused_and_deletes_nothing() {
    for RefusalCase {
        case,
        tenant_name,
        status,
        error_code,
        message,
    } in refusal_cases()
    {
        let ses = Ses::new();
        // One real tenant holding one real resource, so each refusal can be shown to
        // leave the store exactly as it was.
        let resource = Resource::ConfigurationSet("live-set");
        let arn = resource.arn();
        resource.create(&ses, case).await;
        let created = ses.create_tenant(LIVE_TENANT).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: expected HTTP 200 creating the tenant, got {} {:?}",
            created.status,
            created.message()
        );
        let associated = ses.associate(LIVE_TENANT, &arn).await;
        assert_eq!(
            associated.status,
            200,
            "case {case:?}: expected HTTP 200 associating the resource, got {} {:?}",
            associated.status,
            associated.message()
        );

        let refused = ses.delete_tenant(&tenant_name).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            refused.status,
            refused.message()
        );
        assert_eq!(
            refused.error_code(),
            error_code,
            "case {case:?}: expected {error_code}, got {:?} {:?}",
            refused.error_code(),
            refused.message()
        );
        assert_eq!(
            refused.message(),
            message,
            "case {case:?}: expected message {message:?}, got {:?}",
            refused.message()
        );

        let read = ses.get_tenant(LIVE_TENANT).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the refusal to leave the live tenant alone, got {} {:?}",
            read.status,
            read.message()
        );
        let held = ses.list_tenant_resources(LIVE_TENANT).await;
        assert_eq!(
            listed_resources(&held),
            vec![arn.clone()],
            "case {case:?}: expected the refusal to leave the association alone, got {:?}",
            listed_resources(&held)
        );
    }
}
