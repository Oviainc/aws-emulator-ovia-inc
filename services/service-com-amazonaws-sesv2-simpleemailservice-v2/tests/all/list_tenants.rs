// SPDX-License-Identifier: CC0-1.0

//! The tenant listing, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised.
//!
//! The rule these cases pin: `ListTenants` REPORTS the tenants the account holds, each
//! member of a reported tenant read off the record `CreateTenant` persisted (so a
//! listing and that tenant's own read cannot disagree), ordered by `TenantName`,
//! narrowed to the tenants satisfying every key the request's filter names and windowed
//! by its `PageSize` with a `NextToken` that resumes strictly after the last tenant the
//! page reported; a filter key or value outside the enum the model binds, and a token
//! this listing never handed out, are refused `BadRequestException`, and no call changes
//! what a later caller sees.
//!
//! The tables below span what the rule generalises over: the number of tenants stored
//! (none, one, several), the order they were created in against the order they are
//! reported in, the form of their names (one character, the documented 64-character
//! bound, every documented character class), each `ListTenantsFilterKey` and the values
//! it filters on, each `SendingStatus` a tenant may carry, the page size (absent, one,
//! several, the whole listing, past the whole listing, zero, negative), paging a
//! FILTERED listing, and every refusal leg.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};
use crate::contact_lists::query_escaped;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state;

/// The longest `TenantName` SES accepts, taken from the crate's own constant so the
/// bound is stated once: its documentation states the name "can contain up to 64
/// alphanumeric characters, including letters, numbers, hyphens (-) and underscores (_)
/// only" (`smithy_query(query="shape",shape="TenantName")`).
const MAX_TENANT_NAME_CHARS: usize = state::MAX_TENANT_NAME_CHARS;

/// The one `ReputationEntityType` the model binds, which is how a case moves a tenant's
/// sending status before listing it.
const RESOURCE: &str = state::REPUTATION_ENTITY_TYPE_RESOURCE;

/// Convenience calls for the tenant listing, over the shared harness.
trait Tenants {
    async fn create_tenant(&self, name: &str) -> Reply;
    async fn get_tenant(&self, name: &str) -> Reply;
    async fn list_tenants(&self, body: Value) -> Reply;
    async fn set_sending_status(&self, reference: &str, status: &str) -> Reply;
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

    async fn list_tenants(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/tenants/list", Some(body)).await
    }

    async fn set_sending_status(&self, reference: &str, status: &str) -> Reply {
        // `ReputationEntityReference` is a non-greedy `@httpLabel`, so the tenant's ARN
        // reaches the service percent-encoded, the way an SDK escapes a label.
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

/// Create every name in `names`, in the order given, and fail loudly if any create is
/// refused: a listing case is only about the listing.
async fn stored(ses: &Ses, case: &str, names: &[&str]) {
    for name in names {
        let created = ses.create_tenant(name).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 creating tenant {name:?}, got {} {:?}",
            created.status,
            created.message()
        );
    }
}

/// The tenants a reply reported, in the order reported.
fn reported(reply: &Reply) -> Vec<Value> {
    reply.member("Tenants").as_array().cloned().unwrap_or_default()
}

/// The `TenantName` of each tenant a reply reported, in order.
fn reported_names(reply: &Reply) -> Vec<String> {
    reported(reply)
        .iter()
        .map(|tenant| text(tenant, "TenantName"))
        .collect()
}

/// A string member of one reported tenant, or `""` when absent.
fn text(tenant: &Value, member: &str) -> String {
    tenant
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `NextToken` a reply carried, or `None` when it carried none.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .member("NextToken")
        .as_str()
        .map(str::to_string)
        .filter(|token| !token.is_empty())
}

/// `names`, sorted the way the store orders them: a listing is keyed by `TenantName`.
fn in_name_order(names: &[&str]) -> Vec<String> {
    let mut expected: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    expected.sort();
    expected
}

/// One row of the listing table: the tenants to store, in creation order.
struct ListingCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The names to create, in the order the requests arrive.
    names: Vec<String>,
}

/// The listing table: one row per thing the ordering-and-projection rule generalises
/// over.
fn listing_cases() -> Vec<ListingCase> {
    vec![
        ListingCase {
            case: "no tenants at all",
            names: Vec::new(),
        },
        ListingCase {
            case: "one tenant",
            names: vec!["marketing".to_string()],
        },
        ListingCase {
            case: "several tenants created in an order that is not their name order",
            names: vec![
                "zeta".to_string(),
                "alpha".to_string(),
                "mu".to_string(),
                "beta".to_string(),
            ],
        },
        ListingCase {
            case: "names spanning every documented character class",
            names: vec![
                "Tenant_01-beta".to_string(),
                "tenant-02".to_string(),
                "TENANT_03".to_string(),
                "9lives".to_string(),
            ],
        },
        ListingCase {
            case: "the shortest allowed name beside one at the documented bound",
            names: vec![
                "a".to_string(),
                format!("t{}", "x".repeat(MAX_TENANT_NAME_CHARS - 1)),
            ],
        },
    ]
}

#[tokio::test]
async fn every_stored_tenant_is_reported_in_name_order_and_agrees_with_its_own_read() {
    for case in listing_cases() {
        let ses = Ses::new();
        let names: Vec<&str> = case.names.iter().map(String::as_str).collect();
        stored(&ses, case.case, &names).await;

        let listed = ses.list_tenants(json!({})).await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected HTTP 200 from ListTenants, got {} {:?}",
            case.case, listed.status, listed.message()
        );
        assert_eq!(
            reported_names(&listed),
            in_name_order(&names),
            "case {:?}: ListTenants must report every stored tenant in name order, got {:?}",
            case.case,
            reported_names(&listed)
        );
        assert_eq!(
            next_token(&listed),
            None,
            "case {:?}: an unwindowed listing has nothing left behind it, got {:?}",
            case.case,
            next_token(&listed)
        );

        // The projection: `TenantInfo` carries the members of the stored record that
        // `Tenant` carries under the same names, minus the tags and the suppression
        // attributes `TenantInfo` does not model — so a listed tenant must equal its own
        // read with exactly those two members removed.
        for tenant in reported(&listed) {
            let name = text(&tenant, "TenantName");
            let read = ses.get_tenant(&name).await;
            assert_eq!(
                read.status, 200,
                "case {:?}: expected HTTP 200 from GetTenant for {name:?}, got {}",
                case.case, read.status
            );
            let mut expected = read.member("Tenant");
            if let Some(members) = expected.as_object_mut() {
                members.remove("Tags");
                members.remove("SuppressionAttributes");
            }
            assert_eq!(
                tenant, expected,
                "case {:?}: the listed tenant {name:?} must be the record GetTenant \
                 reports, got {tenant:?}",
                case.case
            );
        }

        // Reporting only: listing twice reports the same thing, so the read changed
        // nothing a later caller sees.
        let again = ses.list_tenants(json!({})).await;
        assert_eq!(
            reported(&again),
            reported(&listed),
            "case {:?}: two listings must agree, got {:?} then {:?}",
            case.case,
            reported(&listed),
            reported(&again)
        );
    }
}

/// One row of the paging table: the `PageSize` a walk asks for, and the longest page it
/// may be answered with.
struct PageCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `PageSize` member to send, or `None` to leave it out.
    page_size: Option<i32>,
    /// The most tenants one page may carry, or `None` when nothing truncates the page.
    longest_page: Option<usize>,
}

/// The paging table, over a listing of six tenants.
fn page_cases() -> Vec<PageCase> {
    vec![
        PageCase {
            case: "no PageSize at all, so nothing truncates the page",
            page_size: None,
            longest_page: None,
        },
        PageCase {
            case: "one tenant per page",
            page_size: Some(1),
            longest_page: Some(1),
        },
        PageCase {
            case: "four tenants per page, which does not divide the listing",
            page_size: Some(4),
            longest_page: Some(4),
        },
        PageCase {
            case: "a page size that exactly spans the listing",
            page_size: Some(6),
            longest_page: Some(6),
        },
        PageCase {
            case: "a page size past the whole listing",
            page_size: Some(60),
            longest_page: Some(60),
        },
        PageCase {
            case: "a page size of zero, read as no truncation so a walk still terminates",
            page_size: Some(0),
            longest_page: None,
        },
        PageCase {
            case: "a negative page size, read the same way",
            page_size: Some(-3),
            longest_page: None,
        },
    ]
}

#[tokio::test]
async fn page_size_windows_the_listing_and_next_token_resumes_after_the_last_reported_tenant() {
    const NAMES: [&str; 6] = ["delta", "alpha", "foxtrot", "charlie", "bravo", "echo"];

    for case in page_cases() {
        let ses = Ses::new();
        stored(&ses, case.case, &NAMES).await;

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One more iteration than there are tenants would mean a token that never
        // terminates, which the bound turns into a failure rather than a hang.
        for step in 0..=NAMES.len() {
            let mut body = json!({});
            if let Some(size) = case.page_size {
                body["PageSize"] = json!(size);
            }
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let page = ses.list_tenants(body).await;
            assert_eq!(
                page.status, 200,
                "case {:?}: expected HTTP 200 from page {step}, got {}",
                case.case, page.status
            );
            let page_names = reported_names(&page);
            if let Some(longest) = case.longest_page {
                assert!(
                    page_names.len() <= longest,
                    "case {:?}: page {step} may carry at most {longest} tenants, got {:?}",
                    case.case,
                    page_names
                );
            }
            walked.extend(page_names);
            token = next_token(&page);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            token, None,
            "case {:?}: the walk must terminate, but a token was still offered: {token:?}",
            case.case
        );
        assert_eq!(
            walked,
            in_name_order(&NAMES),
            "case {:?}: walking every page must report each tenant exactly once, in name \
             order, got {walked:?}",
            case.case
        );
    }
}

/// One row of the filter table: the filter to send and the tenants it must leave.
struct FilterCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `Filter` member to send, or `None` to leave it out.
    filter: Option<Value>,
    /// The tenants the filtered listing must report, in name order.
    expected: Vec<&'static str>,
}

/// Every `SendingStatus` the model binds, taken from the crate's own constant so the
/// value set is stated once
/// (`smithy_query(query="shape",shape="SendingStatus")`: `ENABLED`, `REINSTATED`,
/// `DISABLED`).
const SENDING_STATUSES: [&str; 3] = state::SENDING_STATUSES;

/// The filter table, over a fixture whose tenants span every `SendingStatus` and share
/// name substrings. `prod-apac` keeps the status a new tenant is created with.
fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            case: "no filter at all",
            filter: None,
            expected: vec!["prod-apac", "prod-emea", "test-emea", "test-us"],
        },
        FilterCase {
            case: "an empty filter, which narrows nothing",
            filter: Some(json!({})),
            expected: vec!["prod-apac", "prod-emea", "test-emea", "test-us"],
        },
        FilterCase {
            case: "a name substring shared by a prefix of the names",
            filter: Some(json!({"TENANT_NAME_CONTAINS": "prod-"})),
            expected: vec!["prod-apac", "prod-emea"],
        },
        FilterCase {
            case: "a name substring in the middle of the names",
            filter: Some(json!({"TENANT_NAME_CONTAINS": "-eme"})),
            expected: vec!["prod-emea", "test-emea"],
        },
        FilterCase {
            case: "a whole name as the substring",
            filter: Some(json!({"TENANT_NAME_CONTAINS": "test-us"})),
            expected: vec!["test-us"],
        },
        FilterCase {
            case: "a substring no stored name carries",
            filter: Some(json!({"TENANT_NAME_CONTAINS": "staging"})),
            expected: Vec::new(),
        },
        FilterCase {
            case: "a substring differing only in case, which no stored name carries",
            filter: Some(json!({"TENANT_NAME_CONTAINS": "PROD-"})),
            expected: Vec::new(),
        },
        FilterCase {
            case: "the sending status a tenant is created with",
            filter: Some(json!({"SENDING_STATUS": "ENABLED"})),
            expected: vec!["prod-apac", "test-us"],
        },
        FilterCase {
            case: "a sending status a caller disabled",
            filter: Some(json!({"SENDING_STATUS": "DISABLED"})),
            expected: vec!["prod-emea"],
        },
        FilterCase {
            case: "a sending status a caller reinstated",
            filter: Some(json!({"SENDING_STATUS": "REINSTATED"})),
            expected: vec!["test-emea"],
        },
        FilterCase {
            case: "both keys, each satisfied by a different tenant, so only the overlap is \
                   reported",
            filter: Some(json!({"TENANT_NAME_CONTAINS": "-eme", "SENDING_STATUS": "DISABLED"})),
            expected: vec!["prod-emea"],
        },
        FilterCase {
            case: "both keys, with no tenant satisfying both",
            filter: Some(json!({"TENANT_NAME_CONTAINS": "prod-", "SENDING_STATUS": "REINSTATED"})),
            expected: Vec::new(),
        },
    ]
}

/// The filter fixture: four tenants whose statuses span the `SendingStatus` enum.
async fn filter_fixture(case: &str) -> Ses {
    let ses = Ses::new();
    for (name, status) in [
        ("prod-emea", "DISABLED"),
        ("test-emea", "REINSTATED"),
        ("prod-apac", "ENABLED"),
        ("test-us", "ENABLED"),
    ] {
        let created = ses.create_tenant(name).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 creating tenant {name:?}, got {}",
            created.status
        );
        // A tenant is created `ENABLED`, so only the other statuses need a write; the
        // write goes through the reputation surface, which is what owns the status.
        if status != "ENABLED" {
            let reference = created.member("TenantArn").as_str().unwrap_or_default().to_string();
            let moved = ses.set_sending_status(&reference, status).await;
            assert_eq!(
                moved.status, 200,
                "case {case:?}: expected HTTP 200 setting {name:?} to {status:?}, got {}",
                moved.status
            );
        }
    }
    ses
}

#[tokio::test]
async fn a_filter_reports_only_the_tenants_satisfying_every_key_it_names() {
    // Each status the fixture writes is one the model binds, so the table below covers
    // the whole enum rather than a chosen corner of it.
    for status in SENDING_STATUSES {
        assert!(
            filter_cases()
                .iter()
                .any(|case| case.filter.as_ref().and_then(|filter| filter.get("SENDING_STATUS"))
                    == Some(&json!(status))),
            "case {status:?}: every modelled SendingStatus must appear in the filter table"
        );
    }

    for case in filter_cases() {
        let ses = filter_fixture(case.case).await;
        let mut body = json!({});
        if let Some(filter) = &case.filter {
            body["Filter"] = filter.clone();
        }
        let listed = ses.list_tenants(body).await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected HTTP 200 from ListTenants, got {}",
            case.case, listed.status
        );
        assert_eq!(
            reported_names(&listed),
            in_name_order(&case.expected),
            "case {:?}: the filtered listing must report exactly the tenants satisfying \
             it, in name order, got {:?}",
            case.case,
            reported_names(&listed)
        );
        assert_eq!(
            next_token(&listed),
            None,
            "case {:?}: an unwindowed listing has nothing left behind it, got {:?}",
            case.case,
            next_token(&listed)
        );
    }
}

#[tokio::test]
async fn a_token_is_a_position_in_the_filtered_listing_it_was_handed_out_from() {
    let case = "paging a filtered listing one tenant at a time";
    let ses = filter_fixture(case).await;

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for step in 0..=4 {
        let mut body = json!({"PageSize": 1, "Filter": {"TENANT_NAME_CONTAINS": "-eme"}});
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let page = ses.list_tenants(body).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected HTTP 200 from page {step}, got {}",
            page.status
        );
        let names = reported_names(&page);
        assert!(
            names.len() <= 1,
            "case {case:?}: page {step} may carry one tenant, got {names:?}"
        );
        walked.extend(names);
        token = next_token(&page);
        if token.is_none() {
            break;
        }
    }

    assert_eq!(
        walked,
        vec!["prod-emea".to_string(), "test-emea".to_string()],
        "case {case:?}: the walk must report the narrowed listing exactly once, in name \
         order, got {walked:?}"
    );
    assert_eq!(
        token, None,
        "case {case:?}: the walk must terminate, but a token was still offered: {token:?}"
    );
}

#[tokio::test]
async fn input_outside_what_the_model_binds_is_refused_and_nothing_changes() {
    let ses = Ses::new();
    stored(&ses, "the refusal fixture", &["alpha", "beta"]).await;
    let before = ses.list_tenants(json!({})).await;

    // Each row is a request the listing cannot serve: a filter key the enum does not
    // name, a `SENDING_STATUS` value outside its enum, a token this listing never handed
    // out, and a token another listing of this service handed out.
    let foreign_token = state::encode_tagged_page_token(state::RESOURCE_TENANTS_PAGE_TOKEN_TAG, "alpha");
    for (case, body) in [
        (
            "a filter key the enum does not name",
            json!({"Filter": {"TENANT_NAME": "alpha"}}),
        ),
        (
            "a filter key differing only in case",
            json!({"Filter": {"sending_status": "ENABLED"}}),
        ),
        (
            "a SENDING_STATUS value outside the enum",
            json!({"Filter": {"SENDING_STATUS": "PAUSED"}}),
        ),
        (
            "a SENDING_STATUS value in the wrong case",
            json!({"Filter": {"SENDING_STATUS": "enabled"}}),
        ),
        (
            "a NextToken that is not base64 at all",
            json!({"NextToken": "not a token"}),
        ),
        (
            "a NextToken that is base64 of something else",
            json!({"NextToken": ::aws_smithy_types::base64::encode("somewhere else")}),
        ),
        (
            "a NextToken another listing of this service handed out",
            json!({"NextToken": foreign_token}),
        ),
    ] {
        let refused = ses.list_tenants(body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 from ListTenants, got {}",
            refused.status
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
        assert!(
            !refused.message().is_empty(),
            "case {case:?}: the refusal must say what it refused, got {:?}",
            refused.message()
        );
    }

    // A refused call stored nothing and removed nothing.
    let after = ses.list_tenants(json!({})).await;
    assert_eq!(
        reported(&after),
        reported(&before),
        "case {:?}: a refused listing must leave the stored tenants untouched, got {:?} \
         then {:?}",
        "the refusal fixture",
        reported(&before),
        reported(&after)
    );
}
