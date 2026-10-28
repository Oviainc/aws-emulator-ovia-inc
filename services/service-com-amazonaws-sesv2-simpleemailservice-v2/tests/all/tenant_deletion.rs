// SPDX-License-Identifier: CC0-1.0

//! Removing a tenant, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in [`crate::configuration_sets`], so
//! routing, deserialization, the handler and serialization are all exercised.
//!
//! The rule these cases pin: a tenant is removed UNCONDITIONALLY — the operation models
//! no in-use or conflict error, so a tenant still holding resource associations goes
//! exactly like one holding none, and the resources themselves are kept — answering an
//! empty HTTP 200; afterwards every view that resolves the tenant by name reports it
//! missing, while no other tenant is touched. The only refusal is a name the account does
//! not hold (`NotFoundException`, HTTP 404) or one outside the documented name form
//! (`BadRequestException`, HTTP 400).
//!
//! The tables below span what the rule generalises over: the form and length of the name,
//! how many tenants the account holds, whether the tenant held associations or none, the
//! order the tenants are removed in, and every refusal leg (a name never created, a name
//! already removed, and names outside the documented form).

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state;

/// The tenant calls these cases drive.
trait TenantLifecycle {
    async fn create_tenant(&self, name: &str) -> Reply;
    async fn delete_tenant(&self, name: &str) -> Reply;
    async fn get_tenant(&self, name: &str) -> Reply;
    async fn tenant_names(&self) -> Vec<String>;
    async fn associate(&self, name: &str, resource_arn: &str) -> Reply;
    async fn tenant_resources(&self, name: &str) -> Reply;
    async fn resource_tenants(&self, resource_arn: &str) -> Reply;
}

impl TenantLifecycle for Ses {
    async fn create_tenant(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants",
            Some(json!({"TenantName": name})),
        )
        .await
    }

    async fn delete_tenant(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants/delete",
            Some(json!({"TenantName": name})),
        )
        .await
    }

    async fn get_tenant(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants/get",
            Some(json!({"TenantName": name})),
        )
        .await
    }

    async fn tenant_names(&self) -> Vec<String> {
        self.call("POST", "/v2/email/tenants/list", Some(json!({})))
            .await
            .member("Tenants")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|tenant| {
                tenant
                    .pointer("/TenantName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    async fn associate(&self, name: &str, resource_arn: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants/resources",
            Some(json!({"TenantName": name, "ResourceArn": resource_arn})),
        )
        .await
    }

    async fn tenant_resources(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants/resources/list",
            Some(json!({"TenantName": name})),
        )
        .await
    }

    async fn resource_tenants(&self, resource_arn: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/resources/tenants/list",
            Some(json!({"ResourceArn": resource_arn})),
        )
        .await
    }
}

#[tokio::test]
async fn a_tenant_the_account_holds_is_removed_and_no_other_tenant_is_touched() {
    // The rows span the form and length of a name the service admits, and the position of
    // the removed tenant among the ones the account holds: the removal takes the one
    // tenant it names, whichever it is.
    let longest = "n".repeat(state::MAX_TENANT_NAME_CHARS);
    let held = ["a", "marketing-eu", "marketing_eu", "Tenant2026", &longest];

    for target in held {
        let ses = Ses::new();
        for name in held {
            assert_eq!(
                ses.create_tenant(name).await.status,
                200,
                "case {target:?}: every tenant must be created first, {name:?} was not"
            );
        }

        let removed = ses.delete_tenant(target).await;
        assert_eq!(
            removed.status, 200,
            "case {target:?}: expected HTTP 200 from DeleteTenant, got {} {:?}",
            removed.status,
            removed.message()
        );
        assert!(
            removed.is_empty_body(),
            "case {target:?}: DeleteTenant reports no members, got {:?}",
            removed.member("TenantName")
        );

        // Every view that resolves the tenant by name now reports it missing.
        let read = ses.get_tenant(target).await;
        assert_eq!(
            (read.status, read.error_code().as_str()),
            (404, "NotFoundException"),
            "case {target:?}: expected 404 NotFoundException from GetTenant, got {} {:?}",
            read.status,
            read.error_code()
        );
        assert_eq!(
            read.message(),
            state::tenant_not_found(target),
            "case {target:?}: expected the refusal to name the tenant, got {:?}",
            read.message()
        );

        // No other tenant moved.
        let mut expected: Vec<String> = held
            .iter()
            .filter(|name| **name != target)
            .map(|name| (*name).to_string())
            .collect();
        expected.sort();
        let mut reported = ses.tenant_names().await;
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {target:?}: expected the remaining tenants to be reported, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_tenant_still_holding_resource_associations_is_removed_and_its_resources_are_kept() {
    let ses = Ses::new();
    let set = "events";
    assert_eq!(
        ses.create_set(json!({"ConfigurationSetName": set})).await.status,
        200,
        "the resource must be created first"
    );
    let resource_arn = format!(
        "arn:aws:ses:us-east-1:{}:configuration-set/{set}",
        emulator_server::emulator_account_id()
    );

    // Two tenants sharing the one resource: removing one must not disturb the other's
    // association, and must not touch the resource.
    for name in ["keeper", "doomed"] {
        assert_eq!(
            ses.create_tenant(name).await.status,
            200,
            "case {name:?}: the tenant must be created first"
        );
        assert_eq!(
            ses.associate(name, &resource_arn).await.status,
            200,
            "case {name:?}: the association must be created first"
        );
    }

    let removed = ses.delete_tenant("doomed").await;
    assert_eq!(
        removed.status, 200,
        "case {:?}: expected HTTP 200 removing a tenant that still holds an association, got \
         {} {:?}",
        "a tenant holding an association",
        removed.status,
        removed.message()
    );

    // The tenant's own side of the association is gone with the tenant: the listing
    // resolves the tenant first and reports it missing.
    let listed = ses.tenant_resources("doomed").await;
    assert_eq!(
        (listed.status, listed.error_code().as_str()),
        (404, "NotFoundException"),
        "case {:?}: expected 404 NotFoundException from ListTenantResources, got {} {:?}",
        "the removed tenant",
        listed.status,
        listed.error_code()
    );

    // The resource itself is not deleted, and the tenant that still exists keeps its
    // association with it.
    let kept = ses.get_set(set).await;
    assert_eq!(
        kept.status, 200,
        "case {:?}: the resource must survive the tenant, got {} {:?}",
        "the shared resource",
        kept.status,
        kept.message()
    );
    let keeper = ses.tenant_resources("keeper").await;
    assert_eq!(
        keeper
            .member("TenantResources")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case {:?}: the surviving tenant keeps its association, got {:?}",
        "the surviving tenant",
        keeper.member("TenantResources")
    );

    // The resource's own view of its tenants no longer names the removed tenant — the
    // association is gone from both sides — while the surviving tenant is still there.
    let reverse = ses.resource_tenants(&resource_arn).await;
    assert_eq!(
        reverse.status, 200,
        "case {:?}: the resource's own view must still answer, got {} {:?}",
        "the shared resource",
        reverse.status,
        reverse.message()
    );
    let named: Vec<String> = reverse
        .member("ResourceTenants")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tenant| {
            tenant
                .pointer("/TenantName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        named,
        vec!["keeper".to_string()],
        "case {:?}: the resource must name only the tenant that still holds it, got {:?}",
        "the shared resource",
        reverse.member("ResourceTenants")
    );

    // A tenant recreated under the removed name inherits nothing: it holds no
    // association, and the pair the removed tenant held can be associated again.
    assert_eq!(
        ses.create_tenant("doomed").await.status,
        200,
        "case {:?}: the removed name must be free to create again",
        "the recreated tenant"
    );
    let recreated = ses.tenant_resources("doomed").await;
    assert_eq!(
        recreated
            .member("TenantResources")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "case {:?}: a recreated tenant must hold no association, got {:?}",
        "the recreated tenant",
        recreated.member("TenantResources")
    );
    assert_eq!(
        ses.associate("doomed", &resource_arn).await.status,
        200,
        "case {:?}: the pair the removed tenant held must be associable again",
        "the recreated tenant"
    );
}

#[tokio::test]
async fn a_name_the_account_does_not_hold_is_refused_and_removes_nothing() {
    let ses = Ses::new();
    assert_eq!(
        ses.create_tenant("keeper").await.status,
        200,
        "the tenant that must survive has to be created first"
    );
    assert_eq!(
        ses.create_tenant("removed").await.status,
        200,
        "the tenant to remove has to be created first"
    );
    assert_eq!(
        ses.delete_tenant("removed").await.status,
        200,
        "the first removal must be accepted"
    );

    // One row per way a name can resolve to no tenant: never created, already removed,
    // and the held name spelled in another case.
    for (case, name) in [
        ("a name never created", "absent_tenant"),
        ("a name already removed", "removed"),
        ("the held name in another case", "KEEPER"),
    ] {
        let refused = ses.delete_tenant(name).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {:?}",
            refused.status,
            refused.error_code()
        );
        assert_eq!(
            refused.message(),
            state::tenant_not_found(name),
            "case {case:?}: expected the refusal to name the tenant, got {:?}",
            refused.message()
        );
    }

    // A name no tenant could carry is malformed input rather than a missing tenant: the
    // rows span the documented form's bounds — too long, and characters outside the set.
    let too_long = "n".repeat(state::MAX_TENANT_NAME_CHARS + 1);
    for (case, name) in [
        ("a name past the documented length", too_long.as_str()),
        ("a name carrying a space", "marketing eu"),
        ("a name carrying a separator", "marketing/eu"),
    ] {
        let refused = ses.delete_tenant(name).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            refused.status,
            refused.error_code()
        );
    }

    // Not one refused call removed the tenant that is there.
    assert_eq!(
        ses.get_tenant("keeper").await.status,
        200,
        "case {:?}: a refused removal must leave the held tenant alone",
        "every refused removal"
    );
    assert_eq!(
        ses.tenant_names().await,
        vec!["keeper".to_string()],
        "case {:?}: the account must still hold exactly the tenant nothing removed",
        "every refused removal"
    );
}
