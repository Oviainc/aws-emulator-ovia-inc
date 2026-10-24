// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Firewall Manager administrator accounts and the
//! administrative scope each of them may manage: `PutAdminAccount` (the write)
//! and `GetAdminScope` (the read that proves what it persisted).
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the router, reusing the request harness in
//! [`crate::policy_lifecycle`] — so the assertions are about the WIRE response.
//!
//! The rules asserted here, each over a TABLE of scopes rather than one
//! request:
//!
//! * `PutAdminAccount` persists the `AdminScope` it was sent for the account it
//!   names, and `GetAdminScope` reports exactly that scope back;
//! * a newly onboarded administrator is `ONBOARDING` and the CLOCK makes
//!   `ONBOARDING_COMPLETE` due; a read never moves that status itself;
//! * a repeated call is idempotent and an absent `AdminScope` leaves the stored
//!   one unchanged, while a different one replaces it;
//! * the organization's DEFAULT administrator has the whole organization as its
//!   scope and this operation cannot narrow it;
//! * an `AdminScope` that breaks one of the constraints the model documents is
//!   refused and writes nothing;
//! * an administrator manages the calling account exactly when its
//!   `AccountScope` covers it, which `ListAdminsManagingAccount` reports;
//! * both operations are organisation-scoped, so with no administrator
//!   association they are refused before they read or write anything.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::{
    ORGANIZATION_STATUS_VALUES, SECURITY_SERVICE_TYPE_VALUES,
};

use crate::administrator_gated_operations::associate_admin;
use crate::policy_lifecycle::{call, call_in, emulator, error_code, ACCOUNT, REGION};

/// A second Region, to assert an answer does not depend on which Region the
/// request addresses.
const OTHER_REGION: &str = "eu-central-1";

/// The account ids onboarded as administrators below. The model constrains
/// `AWSAccountId` only to `^[0-9]+$` with length 1..1024, so these differ in
/// length and digits — including the one-digit and the long forms.
const DELEGATED_ACCOUNTS: &[&str] = &["222233334444", "7", "000000000001", "99999999999999999999"];

/// How long a new administrator stays `ONBOARDING`, from the service's own
/// constant, plus a margin so a waiting test is not racing it.
///
/// A blocking `std::thread::sleep` on purpose: this workspace's `tokio` is
/// built without the `time` feature, so `tokio::time::sleep` is not available
/// here, and a test that waits out one chosen span has nothing else to do
/// meanwhile.
fn wait_out_the_onboarding() {
    sleep(
        service_com_amazonaws_fms_awsfms_20180101::state::ADMIN_ONBOARDING_SPAN
            + Duration::from_millis(250),
    );
}

/// `name` as the model's own `OrganizationStatus` catalog spells it, so an
/// expectation below names a value the enum actually carries instead of a
/// free-standing string.
fn organization_status(name: &str) -> &'static str {
    ORGANIZATION_STATUS_VALUES
        .iter()
        .copied()
        .find(|value| *value == name)
        .unwrap_or_else(|| {
            panic!(
                "{name} must be a modelled OrganizationStatus, catalog is \
                 {ORGANIZATION_STATUS_VALUES:?}"
            )
        })
}

/// `name` as the model's own `SecurityServiceType` catalog spells it — the
/// source of every policy type a `PolicyTypeScope` below names.
fn policy_type(name: &str) -> &'static str {
    SECURITY_SERVICE_TYPE_VALUES
        .iter()
        .copied()
        .find(|value| *value == name)
        .unwrap_or_else(|| {
            panic!(
                "{name} must be a modelled SecurityServiceType, catalog is \
                 {SECURITY_SERVICE_TYPE_VALUES:?}"
            )
        })
}

/// A member of `value` that is present and not null.
fn member<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.get(name).filter(|held| !held.is_null())
}

/// A boolean member, defaulting to the `false` the model gives every flag of
/// `AdminScope`'s four scopes.
fn flag(value: &Value, name: &str) -> bool {
    member(value, name).and_then(Value::as_bool).unwrap_or(false)
}

/// A list member, defaulting to the empty list.
fn list(value: &Value, name: &str) -> Value {
    member(value, name)
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()))
}

/// A wire `AdminScope` with every defaulted member filled in, so an
/// expectation and a response compare as the same value whether or not the
/// serializer emitted a default `false` flag or an absent empty list.
///
/// A scope the request left out stays out: an administrator granted no
/// `RegionScope` is not the same as one granted an empty one.
fn normalize_scope(scope: &Value) -> Value {
    let mut normalized = Map::new();
    if let Some(accounts) = member(scope, "AccountScope") {
        normalized.insert(
            "AccountScope".to_string(),
            json!({
                "Accounts": list(accounts, "Accounts"),
                "AllAccountsEnabled": flag(accounts, "AllAccountsEnabled"),
                "ExcludeSpecifiedAccounts": flag(accounts, "ExcludeSpecifiedAccounts"),
            }),
        );
    }
    if let Some(units) = member(scope, "OrganizationalUnitScope") {
        normalized.insert(
            "OrganizationalUnitScope".to_string(),
            json!({
                "OrganizationalUnits": list(units, "OrganizationalUnits"),
                "AllOrganizationalUnitsEnabled": flag(units, "AllOrganizationalUnitsEnabled"),
                "ExcludeSpecifiedOrganizationalUnits":
                    flag(units, "ExcludeSpecifiedOrganizationalUnits"),
            }),
        );
    }
    if let Some(regions) = member(scope, "RegionScope") {
        normalized.insert(
            "RegionScope".to_string(),
            json!({
                "Regions": list(regions, "Regions"),
                "AllRegionsEnabled": flag(regions, "AllRegionsEnabled"),
            }),
        );
    }
    if let Some(types) = member(scope, "PolicyTypeScope") {
        normalized.insert(
            "PolicyTypeScope".to_string(),
            json!({
                "PolicyTypes": list(types, "PolicyTypes"),
                "AllPolicyTypesEnabled": flag(types, "AllPolicyTypesEnabled"),
            }),
        );
    }
    Value::Object(normalized)
}

/// `GetAdminScope` for `account`, as `(http status, error code or "", scope,
/// status)`.
async fn admin_scope(
    router: &smithy_server_runtime::MultiServiceRouter,
    account: &str,
) -> (u16, String, Value, String) {
    let (status, body) = call(router, "GetAdminScope", &json!({ "AdminAccount": account })).await;
    let code = if status == 200 {
        String::new()
    } else {
        error_code(&body)
    };
    let scope = normalize_scope(&body["AdminScope"]);
    let organization_status = body["Status"].as_str().unwrap_or_default().to_string();
    (status, code, scope, organization_status)
}

/// One row of the scope table: a name for the assertion messages, and the
/// `AdminScope` a client sends.
struct ScopeCase {
    name: &'static str,
    scope: Value,
}

/// The administrative-scope class the persistence rule is asserted over.
///
/// The rows differ along what an `AdminScope` can vary in: which of its four
/// scopes are configured at all, whether a scope grants EVERYTHING
/// (`All…Enabled`), a LIST of named resources, or a list it EXCLUDES, the
/// COUNT of entries in that list (none, one, several), and the FORM of those
/// entries (account ids of different lengths, OU ids, Region names, policy
/// types). Every policy type named here comes from the model's own
/// `SecurityServiceType` catalog, and includes `NETWORK_ACL_COMMON` and
/// `IMPORT_NETWORK_FIREWALL`, which the issue did not cite.
fn scope_cases() -> Vec<ScopeCase> {
    vec![
        ScopeCase {
            // The minimum an AdminScope can be: nothing granted yet.
            name: "an empty scope",
            scope: json!({}),
        },
        ScopeCase {
            name: "all accounts",
            scope: json!({ "AccountScope": { "AllAccountsEnabled": true } }),
        },
        ScopeCase {
            name: "one named account",
            scope: json!({ "AccountScope": { "Accounts": [ACCOUNT] } }),
        },
        ScopeCase {
            name: "several named accounts of differing length",
            scope: json!({
                "AccountScope": { "Accounts": ["7", "000000000001", "99999999999999999999"] },
            }),
        },
        ScopeCase {
            name: "every account except the named ones",
            scope: json!({
                "AccountScope": {
                    "Accounts": ["555566667777"],
                    "ExcludeSpecifiedAccounts": true,
                },
            }),
        },
        ScopeCase {
            name: "an empty exclusion list",
            scope: json!({
                "AccountScope": { "Accounts": [], "ExcludeSpecifiedAccounts": true },
            }),
        },
        ScopeCase {
            name: "all organizational units",
            scope: json!({
                "OrganizationalUnitScope": { "AllOrganizationalUnitsEnabled": true },
            }),
        },
        ScopeCase {
            name: "named organizational units",
            scope: json!({
                "OrganizationalUnitScope": {
                    "OrganizationalUnits": ["ou-abc1-11111111", "ou-abc1-22222222"],
                },
            }),
        },
        ScopeCase {
            name: "every organizational unit except the named one",
            scope: json!({
                "OrganizationalUnitScope": {
                    "OrganizationalUnits": ["ou-abc1-33333333"],
                    "ExcludeSpecifiedOrganizationalUnits": true,
                },
            }),
        },
        ScopeCase {
            name: "all Regions",
            scope: json!({ "RegionScope": { "AllRegionsEnabled": true } }),
        },
        ScopeCase {
            name: "named Regions",
            scope: json!({
                "RegionScope": { "Regions": [REGION, OTHER_REGION, "ap-southeast-2"] },
            }),
        },
        ScopeCase {
            name: "all policy types",
            scope: json!({ "PolicyTypeScope": { "AllPolicyTypesEnabled": true } }),
        },
        ScopeCase {
            name: "named policy types from the modelled catalog",
            scope: json!({
                "PolicyTypeScope": {
                    "PolicyTypes": [
                        policy_type("NETWORK_ACL_COMMON"),
                        policy_type("IMPORT_NETWORK_FIREWALL"),
                        policy_type("SHIELD_ADVANCED"),
                    ],
                },
            }),
        },
        ScopeCase {
            name: "all four scopes at once",
            scope: json!({
                "AccountScope": { "Accounts": ["123456789012", "7"] },
                "OrganizationalUnitScope": {
                    "OrganizationalUnits": ["ou-abc1-44444444"],
                    "ExcludeSpecifiedOrganizationalUnits": true,
                },
                "RegionScope": { "Regions": [REGION] },
                "PolicyTypeScope": { "PolicyTypes": [policy_type("WAFV2")] },
            }),
        },
    ]
}

/// Onboard `account` with `scope`, asserting the 200 and the empty body
/// `PutAdminAccount` models.
async fn put_admin_account(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    account: &str,
    scope: Option<&Value>,
) {
    let mut request = json!({ "AdminAccount": account });
    if let Some(scope) = scope {
        request["AdminScope"] = scope.clone();
    }
    let (status, body) = call(router, "PutAdminAccount", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutAdminAccount({account}) to answer 200, got {status} with \
         {body:?}"
    );
    assert_eq!(
        body,
        json!({}),
        "case {case:?}: PutAdminAccount models no output members, so the body must be empty, got \
         {body:?}"
    );
}

/// `PutAdminAccount` persists the scope it was sent and `GetAdminScope`
/// reports exactly that scope back — for every row of the scope table, over
/// account-id forms spanning the modelled `AWSAccountId`.
#[tokio::test]
async fn put_admin_account_persists_the_scope_that_get_admin_scope_reports() {
    for (index, case) in scope_cases().into_iter().enumerate() {
        // Each row is asserted on a different account-id form, so the rule is
        // not asserted only for one shape of identifier.
        let account = DELEGATED_ACCOUNTS[index % DELEGATED_ACCOUNTS.len()];
        let router = emulator();
        associate_admin(&router).await;
        let name = case.name;

        // Nothing is an administrator before the write.
        let (status, code, _, _) = admin_scope(&router, account).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFoundException"),
            "case {name:?} / {account}: before the write the account is no administrator, so \
             GetAdminScope must answer 400 ResourceNotFoundException, got {status} / {code:?}"
        );

        put_admin_account(&router, name, account, Some(&case.scope)).await;

        let (status, code, scope, organization) = admin_scope(&router, account).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {name:?} / {account}: expected GetAdminScope to answer 200 after the write, \
             got {status} / {code:?}"
        );
        assert_eq!(
            scope,
            normalize_scope(&case.scope),
            "case {name:?} / {account}: expected GetAdminScope to report the scope that was \
             written, got {scope:?}"
        );
        assert_eq!(
            organization,
            organization_status("ONBOARDING"),
            "case {name:?} / {account}: a just-onboarded administrator is still onboarding, got \
             {organization:?}"
        );

        // A read changes nothing a later caller sees.
        let (_, _, again, organization_again) = admin_scope(&router, account).await;
        assert_eq!(
            (again, organization_again),
            (scope.clone(), organization.clone()),
            "case {name:?} / {account}: a repeated GetAdminScope must answer identically"
        );
    }
}

/// The onboarding a `PutAdminAccount` starts is completed by the CLOCK: the
/// administrator is `ONBOARDING` straight after the call — so a caller acting
/// immediately still observes the transient status — and
/// `ONBOARDING_COMPLETE` once the span has passed, with its scope untouched.
#[tokio::test]
async fn the_onboarding_of_a_new_administrator_is_completed_by_the_clock() {
    let rows = [
        (
            "all accounts",
            DELEGATED_ACCOUNTS[0],
            json!({ "AccountScope": { "AllAccountsEnabled": true } }),
        ),
        (
            "named Regions and policy types",
            DELEGATED_ACCOUNTS[1],
            json!({
                "RegionScope": { "Regions": [REGION] },
                "PolicyTypeScope": { "PolicyTypes": [policy_type("WAFV2")] },
            }),
        ),
    ];

    for (case, account, scope) in rows {
        let router = emulator();
        associate_admin(&router).await;
        put_admin_account(&router, case, account, Some(&scope)).await;

        let (status, _, before, organization) = admin_scope(&router, account).await;
        assert_eq!(
            (status, organization.as_str()),
            (200, organization_status("ONBOARDING")),
            "case {case:?}: a caller reading straight after the call must still see the \
             transient status, got {status} / {organization:?}"
        );

        wait_out_the_onboarding();
        let (status, _, after, settled) = admin_scope(&router, account).await;
        assert_eq!(
            (status, settled.as_str()),
            (200, organization_status("ONBOARDING_COMPLETE")),
            "case {case:?}: once the onboarding span has passed the administrator is onboarded, \
             got {status} / {settled:?}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: completing the onboarding must not change the scope, got {after:?}"
        );
    }
}

/// A repeated `PutAdminAccount` is idempotent, an absent `AdminScope` leaves
/// the stored one unchanged, and a different one replaces it — without
/// re-onboarding the account.
#[tokio::test]
async fn a_second_put_admin_account_updates_the_scope_in_place() {
    let account = DELEGATED_ACCOUNTS[0];
    let first = json!({ "AccountScope": { "Accounts": [ACCOUNT] } });
    let second = json!({
        "AccountScope": { "AllAccountsEnabled": true },
        "RegionScope": { "Regions": [REGION] },
    });

    let router = emulator();
    associate_admin(&router).await;
    put_admin_account(&router, "create", account, Some(&first)).await;
    wait_out_the_onboarding();

    // The same scope again: nothing changes, and the account is not onboarded
    // a second time.
    put_admin_account(&router, "same scope again", account, Some(&first)).await;
    let (status, _, scope, organization) = admin_scope(&router, account).await;
    assert_eq!(
        (status, scope.clone(), organization.as_str()),
        (
            200,
            normalize_scope(&first),
            organization_status("ONBOARDING_COMPLETE")
        ),
        "case \"same scope again\": a repeated write must leave the administrator as it was, got \
         {status} / {scope:?} / {organization:?}"
    );

    // No `AdminScope` at all: the stored scope is unchanged, not emptied.
    put_admin_account(&router, "no scope sent", account, None).await;
    let (_, _, scope, organization) = admin_scope(&router, account).await;
    assert_eq!(
        (scope.clone(), organization.as_str()),
        (
            normalize_scope(&first),
            organization_status("ONBOARDING_COMPLETE")
        ),
        "case \"no scope sent\": an absent AdminScope leaves the stored scope unchanged, got \
         {scope:?} / {organization:?}"
    );

    // A different scope: it replaces the stored one wholesale.
    put_admin_account(&router, "a different scope", account, Some(&second)).await;
    let (_, _, scope, organization) = admin_scope(&router, account).await;
    assert_eq!(
        (scope.clone(), organization.as_str()),
        (
            normalize_scope(&second),
            organization_status("ONBOARDING_COMPLETE")
        ),
        "case \"a different scope\": the update must replace the stored scope, got {scope:?} / \
         {organization:?}"
    );
}

/// The organization's DEFAULT administrator manages the whole organization, and
/// `PutAdminAccount` cannot narrow that: it is refused with the modelled
/// `InvalidOperationException` and the default administrator's scope still
/// reads as the full one.
#[tokio::test]
async fn the_default_administrator_has_full_scope_and_cannot_be_rescoped() {
    let full = json!({
        "AccountScope": { "AllAccountsEnabled": true },
        "OrganizationalUnitScope": { "AllOrganizationalUnitsEnabled": true },
        "RegionScope": { "AllRegionsEnabled": true },
        "PolicyTypeScope": { "AllPolicyTypesEnabled": true },
    });

    for region in [REGION, OTHER_REGION] {
        let router = emulator();
        associate_admin(&router).await;

        let (status, code, scope, organization) = admin_scope(&router, ACCOUNT).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "region {region}: expected GetAdminScope of the default administrator to answer 200, \
             got {status} / {code:?}"
        );
        assert_eq!(
            scope,
            normalize_scope(&full),
            "region {region}: the default administrator has full administrative scope, got \
             {scope:?}"
        );
        assert_eq!(
            organization,
            organization_status("ONBOARDING_COMPLETE"),
            "region {region}: an associated default administrator is onboarded, got \
             {organization:?}"
        );

        // Narrower, wider and absent scopes alike: the account is the default
        // administrator, so there is nothing this operation can do to it.
        let rows = [
            ("a narrower scope", Some(json!({ "AccountScope": { "Accounts": ["7"] } }))),
            ("the full scope again", Some(full.clone())),
            ("no scope at all", None),
        ];
        for (case, sent) in rows {
            let mut request = json!({ "AdminAccount": ACCOUNT });
            if let Some(scope) = sent {
                request["AdminScope"] = scope;
            }
            let (status, body) = call_in(&router, "PutAdminAccount", &request, region).await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidOperationException"),
                "case {case:?} in {region}: rescoping the default administrator is not possible, \
                 so expected 400 InvalidOperationException, got {status} / {code:?} with {body:?}"
            );
        }

        let (_, _, scope, _) = admin_scope(&router, ACCOUNT).await;
        assert_eq!(
            scope,
            normalize_scope(&full),
            "region {region}: a refused PutAdminAccount must leave the default administrator's \
             scope untouched, got {scope:?}"
        );
    }
}

/// An account that was never onboarded as an administrator is no administrator
/// resource to report: the modelled `ResourceNotFoundException`, not an empty
/// scope.
#[tokio::test]
async fn an_account_that_is_no_administrator_is_not_found() {
    let router = emulator();
    associate_admin(&router).await;
    // One administrator IS onboarded, so "not found" cannot be an artefact of
    // an empty store.
    put_admin_account(&router, "seed", DELEGATED_ACCOUNTS[0], None).await;

    let unknown = ["555566667777", "1", "0", "123456789012345678901234567890"];
    for account in unknown {
        let (status, code, _, _) = admin_scope(&router, account).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFoundException"),
            "case {account:?}: expected 400 ResourceNotFoundException for an account that is no \
             administrator, got {status} / {code:?}"
        );
    }
}

/// An `AdminScope` that breaks one of the constraints the model documents is
/// refused with the modelled `InvalidInputException`, and the refusal writes
/// nothing — neither a new administrator nor a change to an existing one.
#[tokio::test]
async fn an_admin_scope_that_breaks_a_documented_constraint_is_refused_and_writes_nothing() {
    // Each row names the documented constraint it breaks.
    let rows = vec![
        (
            // "You can either enable management of all accounts through this
            // operation, or you can specify a list of accounts to manage in
            // AccountScope$Accounts. You cannot specify both."
            "all accounts and a list of accounts",
            json!({
                "AccountScope": { "AllAccountsEnabled": true, "Accounts": ["7"] },
            }),
        ),
        (
            // The same sentence, from AccountScope$ExcludeSpecifiedAccounts.
            "all accounts and an exclusion",
            json!({
                "AccountScope": {
                    "AllAccountsEnabled": true,
                    "ExcludeSpecifiedAccounts": true,
                },
            }),
        ),
        (
            // OrganizationalUnitScope$AllOrganizationalUnitsEnabled: "You can
            // either enable management of all OUs through this operation, or
            // you can specify OUs to manage … You cannot specify both."
            "all organizational units and a list of them",
            json!({
                "OrganizationalUnitScope": {
                    "AllOrganizationalUnitsEnabled": true,
                    "OrganizationalUnits": ["ou-abc1-11111111"],
                },
            }),
        ),
        (
            "all organizational units and an exclusion",
            json!({
                "OrganizationalUnitScope": {
                    "AllOrganizationalUnitsEnabled": true,
                    "ExcludeSpecifiedOrganizationalUnits": true,
                },
            }),
        ),
        (
            // PolicyTypeScope$AllPolicyTypesEnabled: "Third-party policy types
            // can only be managed by the Firewall Manager default
            // administrator."
            "a third-party policy type alone",
            json!({
                "PolicyTypeScope": { "PolicyTypes": [policy_type("THIRD_PARTY_FIREWALL")] },
            }),
        ),
        (
            "a third-party policy type among others",
            json!({
                "PolicyTypeScope": {
                    "PolicyTypes": [
                        policy_type("WAFV2"),
                        policy_type("THIRD_PARTY_FIREWALL"),
                        policy_type("DNS_FIREWALL"),
                    ],
                },
            }),
        ),
    ];

    let held = json!({ "AccountScope": { "Accounts": [ACCOUNT] } });
    for (case, scope) in &rows {
        let router = emulator();
        associate_admin(&router).await;
        let fresh = DELEGATED_ACCOUNTS[1];
        let existing = DELEGATED_ACCOUNTS[2];
        put_admin_account(&router, case, existing, Some(&held)).await;

        for account in [fresh, existing] {
            let (status, body) = call(
                &router,
                "PutAdminAccount",
                &json!({ "AdminAccount": account, "AdminScope": scope }),
            )
            .await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidInputException"),
                "case {case:?} / {account}: expected 400 InvalidInputException, got {status} / \
                 {code:?} with {body:?}"
            );
        }

        // Nothing was created for the account that was not an administrator…
        let (status, code, _, _) = admin_scope(&router, fresh).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a refused PutAdminAccount must onboard no administrator, but \
             GetAdminScope({fresh}) now answers {status} / {code:?}"
        );
        // …and the administrator that did exist still holds the scope it held.
        let (_, _, scope_after, _) = admin_scope(&router, existing).await;
        assert_eq!(
            scope_after,
            normalize_scope(&held),
            "case {case:?}: a refused PutAdminAccount must leave the stored scope untouched, got \
             {scope_after:?}"
        );
    }
}

/// An `AccountScope$Accounts` entry that is not an Amazon Web Services account
/// id is refused with a 400 client error and writes nothing.
#[tokio::test]
async fn an_account_scope_naming_a_non_account_is_refused() {
    // The model's `AWSAccountId` is `^[0-9]+$`, length 1..1024.
    let rows = [
        ("letters", json!(["not-an-account"])),
        ("an ARN", json!([format!("arn:aws:fms:{REGION}:{ACCOUNT}:policy/abc")])),
        ("an empty string", json!([""])),
        ("a valid id beside an invalid one", json!([ACCOUNT, "12a4"])),
    ];

    for (case, accounts) in rows {
        let router = emulator();
        associate_admin(&router).await;
        let account = DELEGATED_ACCOUNTS[0];

        let (status, body) = call(
            &router,
            "PutAdminAccount",
            &json!({
                "AdminAccount": account,
                "AdminScope": { "AccountScope": { "Accounts": accounts } },
            }),
        )
        .await;
        let code = error_code(&body);
        assert_eq!(
            status, 400,
            "case {case:?}: expected an account list that is not account ids to be refused with \
             400, got {status} / {code:?} with {body:?}"
        );
        assert!(
            code == "InvalidInputException" || code == "ValidationException",
            "case {case:?}: expected the refusal to carry a modelled client error code, got \
             {code:?} with {body:?}"
        );

        let (status, code, _, _) = admin_scope(&router, account).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a refused PutAdminAccount must onboard no administrator, got \
             {status} / {code:?}"
        );
    }
}

/// An onboarded administrator manages the calling account exactly when its
/// `AccountScope` covers that account, which is what
/// `ListAdminsManagingAccount` reports beside the default administrator.
#[tokio::test]
async fn an_administrator_manages_the_caller_exactly_when_its_account_scope_covers_it() {
    let other = "555566667777";
    // Each row: the scope granted, and whether it covers the calling account.
    let rows = vec![
        (
            "all accounts",
            json!({ "AccountScope": { "AllAccountsEnabled": true } }),
            true,
        ),
        (
            "the caller is listed",
            json!({ "AccountScope": { "Accounts": [ACCOUNT] } }),
            true,
        ),
        (
            "another account is listed",
            json!({ "AccountScope": { "Accounts": [other] } }),
            false,
        ),
        (
            "another account is excluded",
            json!({
                "AccountScope": { "Accounts": [other], "ExcludeSpecifiedAccounts": true },
            }),
            true,
        ),
        (
            "the caller is excluded",
            json!({
                "AccountScope": { "Accounts": [ACCOUNT], "ExcludeSpecifiedAccounts": true },
            }),
            false,
        ),
        (
            "no account scope at all",
            json!({ "RegionScope": { "AllRegionsEnabled": true } }),
            false,
        ),
    ];

    for (case, scope, manages) in rows {
        let router = emulator();
        associate_admin(&router).await;
        let account = DELEGATED_ACCOUNTS[0];
        put_admin_account(&router, case, account, Some(&scope)).await;
        wait_out_the_onboarding();

        let (status, body) = call(&router, "ListAdminsManagingAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListAdminsManagingAccount to answer 200, got {status} with \
             {body:?}"
        );
        let mut admins: Vec<String> = body["AdminAccounts"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        admins.sort();
        // The default administrator manages every account in the organization,
        // so it is always there; the onboarded one only when its scope covers
        // the caller.
        let mut expected = vec![ACCOUNT.to_string()];
        if manages {
            expected.push(account.to_string());
        }
        expected.sort();
        assert_eq!(
            admins, expected,
            "case {case:?}: expected the administrators managing the caller to be {expected:?}, \
             got {admins:?}"
        );

        // The onboarded administrator is an account of the organization from
        // then on, whatever its scope covers.
        let (status, body) = call(&router, "ListMemberAccounts", &json!({})).await;
        let members: Vec<String> = body["MemberAccounts"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListMemberAccounts to answer 200, got {status} with {body:?}"
        );
        assert!(
            members.contains(&account.to_string()),
            "case {case:?}: an onboarded administrator is a member account of the organization, \
             got {members:?}"
        );
    }
}

/// The number of administrator accounts an organization holds is bounded: the
/// bound itself is accepted, one past it is refused with the modelled
/// `LimitExceededException`, and updating an administrator that already exists
/// still succeeds at the bound.
#[tokio::test]
async fn the_number_of_administrator_accounts_is_bounded() {
    // The service's own bound, read off the behaviour rather than hard-coded:
    // onboard accounts until one is refused.
    let router = emulator();
    associate_admin(&router).await;

    let mut onboarded: Vec<String> = Vec::new();
    let mut refused: Option<(String, u16, String)> = None;
    for index in 0..64 {
        let account = format!("9000000000{index:02}");
        let (status, body) = call(
            &router,
            "PutAdminAccount",
            &json!({
                "AdminAccount": account,
                "AdminScope": { "AccountScope": { "AllAccountsEnabled": true } },
            }),
        )
        .await;
        if status == 200 {
            onboarded.push(account);
            continue;
        }
        refused = Some((account, status, error_code(&body)));
        break;
    }

    let (account, status, code) = refused.expect(
        "case bound: the number of administrator accounts must be bounded, but 64 of them were \
         onboarded",
    );
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "case bound: expected the write past the bound to answer 400 LimitExceededException, got \
         {status} / {code:?}"
    );
    assert!(
        !onboarded.is_empty(),
        "case bound: expected at least one administrator to be onboarded before the bound"
    );

    // The refused account was not stored…
    let (status, code, _, _) = admin_scope(&router, &account).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ResourceNotFoundException"),
        "case bound: a refused PutAdminAccount must onboard no administrator, got {status} / \
         {code:?}"
    );
    // …and an UPDATE of one that exists is still answered at the bound: it
    // creates nothing.
    let held = &onboarded[0];
    put_admin_account(
        &router,
        "update at the bound",
        held,
        Some(&json!({ "AccountScope": { "Accounts": [ACCOUNT] } })),
    )
    .await;
    let (_, _, scope, _) = admin_scope(&router, held).await;
    assert_eq!(
        scope,
        normalize_scope(&json!({ "AccountScope": { "Accounts": [ACCOUNT] } })),
        "case \"update at the bound\": expected the update to replace the stored scope, got \
         {scope:?}"
    );
}

/// Both operations are organisation-scoped, so with no Firewall Manager
/// administrator association they are refused with HTTP 400
/// `AccessDeniedException` — whatever their payload and whichever Region the
/// request is signed for — before anything is read or written.
#[tokio::test]
async fn both_operations_are_refused_without_an_administrator_association() {
    let payloads = vec![
        (
            "PutAdminAccount with a scope",
            "PutAdminAccount",
            json!({
                "AdminAccount": DELEGATED_ACCOUNTS[0],
                "AdminScope": { "AccountScope": { "AllAccountsEnabled": true } },
            }),
        ),
        (
            "PutAdminAccount with no scope",
            "PutAdminAccount",
            json!({ "AdminAccount": DELEGATED_ACCOUNTS[1] }),
        ),
        (
            "PutAdminAccount with a scope that is invalid anyway",
            "PutAdminAccount",
            json!({
                "AdminAccount": DELEGATED_ACCOUNTS[2],
                "AdminScope": { "AccountScope": { "AllAccountsEnabled": true, "Accounts": ["7"] } },
            }),
        ),
        (
            "GetAdminScope of an unknown account",
            "GetAdminScope",
            json!({ "AdminAccount": DELEGATED_ACCOUNTS[0] }),
        ),
        (
            "GetAdminScope of the calling account",
            "GetAdminScope",
            json!({ "AdminAccount": ACCOUNT }),
        ),
    ];

    for region in [REGION, OTHER_REGION] {
        let router = emulator();
        for (case, operation, body) in &payloads {
            let (status, answered) = call_in(&router, operation, body, region).await;
            let code = error_code(&answered);
            assert_eq!(
                (status, code.as_str()),
                (400, "AccessDeniedException"),
                "case {case:?} in {region}: with no administrator association expected 400 \
                 AccessDeniedException, got {status} / {code:?} with {answered:?}"
            );
            let message = answered
                .get("message")
                .or_else(|| answered.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                message.contains(ACCOUNT) && message.contains(region),
                "case {case:?} in {region}: the refusal's message is built from the request, so \
                 it must name the account and Region, got {message:?}"
            );
        }

        // The refused writes stored nothing: once an administrator IS
        // associated, none of those accounts is an administrator.
        associate_admin(&router).await;
        for account in &DELEGATED_ACCOUNTS[..3] {
            let (status, code, _, _) = admin_scope(&router, account).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "region {region}: a refused PutAdminAccount must write nothing, but \
                 GetAdminScope({account}) answers {status} / {code:?}"
            );
        }
    }
}
