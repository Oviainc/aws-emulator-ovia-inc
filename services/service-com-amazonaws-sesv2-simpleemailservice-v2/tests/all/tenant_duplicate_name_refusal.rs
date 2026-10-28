// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a tenant name the account already holds, end to end over
//! HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in [`crate::configuration_sets`], so
//! routing, deserialization, the handler and serialization are all exercised.
//!
//! The rule these cases pin: a tenant name is unique within ONE account, so a create of a
//! name the account already holds is refused `AlreadyExistsException` (HTTP 400) with a
//! sentence naming both the requested name and the account the name is already taken in,
//! and the stored tenant is left exactly as the accepted create made it.
//!
//! The table spans what the rule generalises over: the form and length of the name (one
//! character, the longest the service admits, hyphens, underscores, digits, mixed case),
//! and how many creates have already been refused for it.

use serde_json::json;

use crate::configuration_sets::{Reply, Ses};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state;

/// `CreateTenant` with just a name, and `GetTenant` for reading the stored record back.
trait Tenants {
    async fn create_tenant(&self, name: &str) -> Reply;
    async fn get_tenant(&self, name: &str) -> Reply;
}

impl Tenants for Ses {
    async fn create_tenant(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants",
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
}

#[tokio::test]
async fn a_duplicate_tenant_name_is_refused_naming_the_name_and_the_account() {
    let account = emulator_server::emulator_account_id();
    // The rows span the form of a `TenantName` the service admits: its documented
    // character set is letters, digits, hyphens and underscores, up to
    // `MAX_TENANT_NAME_CHARS` of them, so the longest admitted name is one row and a
    // single character another.
    let longest = "n".repeat(state::MAX_TENANT_NAME_CHARS);
    for (case, name) in [
        ("a one-character name", "a".to_string()),
        ("the longest admitted name", longest),
        ("a name carrying hyphens", "marketing-eu-west".to_string()),
        ("a name carrying underscores", "marketing_eu_west".to_string()),
        ("a name carrying digits", "tenant2026".to_string()),
        ("a name in mixed case", "MarketingEU".to_string()),
    ] {
        let ses = Ses::new();
        let created = ses.create_tenant(&name).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from the accepted CreateTenant, got {} {:?}",
            created.status,
            created.message()
        );

        // Every repeat is refused the same way, so the refusal does not depend on how
        // many times the name has already been asked for.
        for attempt in 1..=2 {
            let refused = ses.create_tenant(&name).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?} attempt {attempt}: expected HTTP 400, got {} {:?}",
                refused.status,
                refused.message()
            );
            assert_eq!(
                refused.error_code(),
                "AlreadyExistsException",
                "case {case:?} attempt {attempt}: expected AlreadyExistsException, got {:?}",
                refused.error_code()
            );
            assert_eq!(
                refused.message(),
                format!("Tenant with name {name} already exists in account {account}"),
                "case {case:?} attempt {attempt}: expected the refusal to name the name and \
                 the account, got {:?}",
                refused.message()
            );
        }

        // The refused creates changed nothing: the tenant still reports the record the
        // accepted create made.
        let read = ses.get_tenant(&name).await;
        assert_eq!(
            read.member("Tenant")
                .pointer("/TenantId")
                .cloned()
                .unwrap_or_default(),
            created.member("TenantId"),
            "case {case:?}: a refused duplicate must leave the stored tenant untouched, got \
             {:?}",
            read.member("Tenant")
        );
    }
}

#[tokio::test]
async fn a_name_the_account_does_not_hold_yet_is_accepted_whatever_its_neighbours_are() {
    // The boundary of the rule: only the SAME name collides. Names differing by one
    // character, by case, and by a separator are each created alongside one another.
    let ses = Ses::new();
    for (case, name) in [
        ("the first name", "marketing"),
        ("a name differing by one character", "marketinG"),
        ("a name differing by a separator", "marketing-1"),
        ("a name differing by a suffix", "marketing_1"),
    ] {
        let created = ses.create_tenant(name).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
            created.status,
            created.message()
        );
        assert_eq!(
            created.member("TenantName").as_str().unwrap_or_default(),
            name,
            "case {case:?}: the create must report the name it stored, got {:?}",
            created.member("TenantName")
        );
    }
}
