// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Account Access runtime: both bound operations over HTTP,
//! against entitlements seeded the way an administrator's assignments would have seeded
//! them (the runtime API models no create operation).
//!
//! Each test asserts a RULE over a table of inputs rather than one case:
//!
//! * `ListRoles` reports exactly the entitlements state holds for the tenant named, in
//!   role-ARN order, with the stored account id and the stored account name when there is
//!   one — and nothing at all for a tenant nobody seeded;
//! * a page holds at most `maxResults` roles, capped at the service's own page size, and
//!   its cursor resumes immediately after the last role it returned (swept over every
//!   entitlement count × page size in range);
//! * a cursor this operation did not issue for that tenant's listing is refused;
//! * `GetRoleCredentials` mints a credential set, stores it, and reports exactly what it
//!   stored, expiring at the window the request asked for;
//! * a role absent from the tenant's entitlements is refused with `AccessDeniedException`
//!   and mints nothing;
//! * a `sessionDuration` below one second, or above the role's own maximum, is refused;
//! * a credential set is forgotten once the clock passes the `expiresAt` it advertised.

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use service_com_amazonaws_accountaccessruntime_awsaccountaccessruntime::state::{
    self, ComAmazonawsAccountaccessruntimeAwsaccountaccessruntimeState as RuntimeState,
    RoleEntitlement, DEFAULT_SESSION_DURATION, MAX_PAGE_SIZE,
};
use service_com_amazonaws_accountaccessruntime_awsaccountaccessruntime::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The endpoint an SDK addresses this service at (`endpointPrefix`
/// `account-access-runtime`), which is how the service claims an unsigned request.
const RUNTIME_HOST: &str = "account-access-runtime.us-east-1.amazonaws.com";

/// The tenant the fixture seeds its entitlements under, in the documented `TenantId`
/// form: begins with `aa-`, 3 to 24 characters of lowercase letters, numbers and hyphens
/// (`smithy_query shape=TenantId`).
const TENANT: &str = "aa-fixture-tenant";

/// A second seeded tenant, so every rule below is tested on a store holding more than one
/// tenant's entitlements and cannot be satisfied by ignoring `tenantId`.
const OTHER_TENANT: &str = "aa-other-tenant";

/// A tenant nobody seeds.
const UNKNOWN_TENANT: &str = "aa-unknown-tenant";

/// The entitlements seeded under [`TENANT`]: `(account id, account name, role name,
/// maximum session duration in seconds)`.
///
/// Rows differ along everything the rules generalise over — whether the account name
/// resolves (`AccessDetails$accountName` is documented as absent when it cannot be), the
/// shape of a role name, which account the role lives in, and the role's own maximum
/// session duration, taken at both ends of IAM's published 1-to-12-hour range for a role's
/// maximum session duration plus a value between them.
const FIXTURE: &[(&str, Option<&str>, &str, u64)] = &[
    ("000000000000", Some("Emulator"), "AdministratorAccess", 3600),
    ("111111111111", None, "ReadOnly", 7200),
    ("222222222222", Some("Audit"), "Audit-Role_1", 43200),
    ("333333333333", Some("Sandbox"), "ZZZ-Break-Glass", 3600),
];

/// An IAM role ARN, the form `AccessDetails$roleArn` carries.
fn role_arn(account_id: &str, role_name: &str) -> String {
    format!("arn:aws:iam::{account_id}:role/{role_name}")
}

/// Every role ARN [`FIXTURE`] seeds, ascending — the order `ListRoles` must report them
/// in and the order its cursor resumes through.
fn fixture_role_arns() -> Vec<String> {
    let mut arns: Vec<String> = FIXTURE
        .iter()
        .map(|(account, _, role, _)| role_arn(account, role))
        .collect();
    arns.sort();
    arns
}

/// The [`FIXTURE`] row for `arn`.
fn fixture_row(arn: &str) -> (&'static str, Option<&'static str>, &'static str, u64) {
    *FIXTURE
        .iter()
        .find(|(account, _, role, _)| role_arn(account, role) == arn)
        .unwrap_or_else(|| panic!("the fixture seeds no role {arn:?}"))
}

/// One emulator holding this service, plus the handle its entitlements are seeded
/// through.
struct Runtime {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Runtime {
    /// An emulator with [`FIXTURE`] seeded under [`TENANT`] and one role under
    /// [`OTHER_TENANT`].
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        let runtime = Self { engine, router };
        {
            let store = runtime.state();
            let mut store = store.lock();
            for (account_id, account_name, role_name, max_seconds) in FIXTURE {
                let mut role = RoleEntitlement::new(*account_id, role_arn(account_id, role_name))
                    .with_max_session_duration(Duration::from_secs(*max_seconds));
                if let Some(account_name) = account_name {
                    role = role.with_account_name(*account_name);
                }
                store.put_role(TENANT, role);
            }
            store.put_role(
                OTHER_TENANT,
                RoleEntitlement::new("444444444444", other_tenant_role()),
            );
        }
        runtime
    }

    /// The same store the handlers reach: the seam an administrator's entitlements arrive
    /// through, since this API models no create operation.
    fn state(&self) -> engine::ServiceState<RuntimeState> {
        state::service_state(&*self.engine).expect("the account access runtime store is reachable")
    }

    /// Seed `count` roles under `tenant_id`, named so their ARNs sort in seeding order,
    /// and answer those ARNs ascending.
    fn seed_counted(&self, tenant_id: &str, count: usize) -> Vec<String> {
        let store = self.state();
        let mut store = store.lock();
        let mut arns = Vec::new();
        for index in 0..count {
            let arn = role_arn("555555555555", &format!("role-{index:04}"));
            store.put_role(tenant_id, RoleEntitlement::new("555555555555", arn.clone()));
            arns.push(arn);
        }
        arns.sort();
        arns
    }

    async fn get(&self, uri: &str) -> Response<Bytes> {
        let request = Request::builder()
            .method(Method::GET)
            .uri(format!("https://{RUNTIME_HOST}{uri}"))
            .header("host", RUNTIME_HOST)
            .body(Bytes::new())
            .expect("the test built a well-formed request");
        self.router.dispatch(request).await
    }

    /// `GET /access`, with the query the arguments name.
    async fn list_roles(
        &self,
        tenant_id: &str,
        max_results: Option<usize>,
        next_token: Option<&str>,
    ) -> Response<Bytes> {
        let mut uri = format!("/access?tenantId={tenant_id}");
        if let Some(max_results) = max_results {
            uri.push_str(&format!("&maxResults={max_results}"));
        }
        if let Some(next_token) = next_token {
            uri.push_str(&format!("&nextToken={next_token}"));
        }
        self.get(&uri).await
    }

    /// `GET /access/credentials`, with the query the arguments name.
    async fn get_role_credentials(
        &self,
        tenant_id: &str,
        role: &str,
        session_name: Option<&str>,
        session_duration: Option<i64>,
    ) -> Response<Bytes> {
        let mut uri =
            format!("/access/credentials?tenantId={tenant_id}&accessType=PROGRAMMATIC_ACCESS");
        uri.push_str(&format!("&roleArn={role}"));
        if let Some(session_name) = session_name {
            uri.push_str(&format!("&sessionName={session_name}"));
        }
        if let Some(session_duration) = session_duration {
            uri.push_str(&format!("&sessionDuration={session_duration}"));
        }
        self.get(&uri).await
    }
}

/// A role seeded under [`OTHER_TENANT`] and under no other tenant.
fn other_tenant_role() -> String {
    role_arn("444444444444", "OtherTenantOnly")
}

fn body_text(response: &Response<Bytes>) -> String {
    String::from_utf8_lossy(response.body()).to_string()
}

fn body_json(response: &Response<Bytes>) -> serde_json::Value {
    serde_json::from_slice(response.body()).unwrap_or_else(|error| {
        panic!(
            "the response body must be JSON ({error}), got {:?}",
            body_text(response)
        )
    })
}

/// The wire error code (`__type`) of a failure response.
fn error_code(response: &Response<Bytes>) -> String {
    let code = body_json(response)["__type"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // restJson1 renders `__type` as a shape id or a bare name depending on the renderer;
    // the name after the last `#` is the code a client matches on.
    code.rsplit('#').next().unwrap_or_default().to_string()
}

/// The `message` an error envelope carries.
fn error_message(response: &Response<Bytes>) -> String {
    body_json(response)["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `roleArn` of every reported role, in the order reported.
fn reported_role_arns(response: &Response<Bytes>) -> Vec<String> {
    body_json(response)["roles"]
        .as_array()
        .map(|roles| {
            roles
                .iter()
                .map(|role| role["roleArn"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `nextToken` of a listing response, when it carries one.
fn next_token(response: &Response<Bytes>) -> Option<String> {
    body_json(response)["nextToken"]
        .as_str()
        .map(str::to_string)
}

/// `at` as whole epoch seconds, the unit restJson1 renders a `Timestamp` in.
fn epoch_seconds(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default()
}

/// Walk every page of `tenant_id`'s listing with `maxResults=page_size`, asserting the
/// page bound and the terminal page as it goes, and answer the role ARNs seen in order.
async fn walk_pages(
    runtime: &Runtime,
    tenant_id: &str,
    page_size: usize,
    case: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // A bound, so a cursor that fails to advance fails the test instead of looping.
    for page in 0..64_u32 {
        let response = runtime
            .list_roles(tenant_id, Some(page_size), token.as_deref())
            .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "case {case:?}: page {page} must succeed, got {} {:?}",
            response.status(),
            body_text(&response)
        );
        let roles = reported_role_arns(&response);
        assert!(
            roles.len() <= page_size,
            "case {case:?}: page {page} returned {} roles, more than maxResults={page_size}: \
             {roles:?}",
            roles.len()
        );
        seen.extend(roles.iter().cloned());
        token = next_token(&response);
        match token {
            // The last page carries no cursor: that is how a client knows to stop.
            None => return seen,
            Some(_) => assert!(
                !roles.is_empty(),
                "case {case:?}: page {page} carried a nextToken but no roles, so the cursor \
                 cannot advance (seen so far: {seen:?})"
            ),
        }
    }
    panic!("case {case:?}: the listing did not terminate within 64 pages: {seen:?}")
}

/// RULE: `ListRoles` reports exactly the entitlements state holds for the tenant named,
/// ascending by role ARN, each carrying the stored account id and the stored account name
/// — absent when state holds none. A tenant with no entitlements reports an empty list.
#[tokio::test]
async fn list_roles_reports_exactly_the_tenants_entitlements() {
    let runtime = Runtime::new();
    let cases: &[(&str, Vec<String>)] = &[
        (TENANT, fixture_role_arns()),
        (OTHER_TENANT, vec![other_tenant_role()]),
        (UNKNOWN_TENANT, Vec::new()),
    ];

    for (tenant_id, expected) in cases {
        let response = runtime.list_roles(tenant_id, None, None).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "case {tenant_id:?}: expected 200, got {} {:?}",
            response.status(),
            body_text(&response)
        );
        let reported = reported_role_arns(&response);
        assert_eq!(
            &reported, expected,
            "case {tenant_id:?}: expected roles {expected:?}, got {reported:?}"
        );
        assert!(
            next_token(&response).is_none(),
            "case {tenant_id:?}: a complete listing must omit nextToken, got {:?}",
            body_text(&response)
        );
    }

    // Every member of a reported role comes from the stored entitlement, including the
    // account name a stored entitlement does not have.
    let response = runtime.list_roles(TENANT, None, None).await;
    let json = body_json(&response);
    let roles = json["roles"].as_array().cloned().unwrap_or_default();
    for role in roles {
        let arn = role["roleArn"].as_str().unwrap_or_default();
        let (account_id, account_name, _, _) = fixture_row(arn);
        assert_eq!(
            role["accountId"].as_str(),
            Some(account_id),
            "case {arn:?}: expected accountId {account_id:?}, got {role:?}"
        );
        assert_eq!(
            role["accountName"].as_str(),
            account_name,
            "case {arn:?}: expected accountName {account_name:?}, got {role:?}"
        );
    }
}

/// RULE: the tenant a freshly built store holds is listable without anything seeding it,
/// and a role it lists is one `GetRoleCredentials` accepts — so a booted emulator can
/// serve both operations.
#[tokio::test]
async fn the_bootstrap_tenant_is_listable_and_its_roles_are_assumable() {
    let runtime = Runtime::new();
    let tenant = state::bootstrap_tenant_id();

    let response = runtime.list_roles(&tenant, None, None).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "case {tenant:?}: expected 200, got {} {:?}",
        response.status(),
        body_text(&response)
    );
    let roles = reported_role_arns(&response);
    assert!(
        !roles.is_empty(),
        "case {tenant:?}: the bootstrap tenant must hold entitlements, got {roles:?}"
    );

    for role in &roles {
        let response = runtime
            .get_role_credentials(&tenant, role, None, None)
            .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "case {role:?}: a listed role must be assumable, got {} {:?}",
            response.status(),
            body_text(&response)
        );
    }
}

/// RULE: a page holds at most `maxResults` roles and its `nextToken` resumes immediately
/// after the last role returned, so walking the pages yields the whole listing exactly
/// once and the last page omits the token.
///
/// Swept over every (entitlement count, page size) pair in range rather than sampled: the
/// count spans 0..=5 and the page size spans 1..=6, one past the longest listing.
#[tokio::test]
async fn paging_a_role_listing_yields_every_entitlement_exactly_once() {
    let runtime = Runtime::new();

    for count in 0..=5_usize {
        let tenant_id = format!("aa-count-{count}");
        let expected = runtime.seed_counted(&tenant_id, count);
        for page_size in 1..=6_usize {
            let case = format!("{count} roles, maxResults={page_size}");
            let seen = walk_pages(&runtime, &tenant_id, page_size, &case).await;
            assert_eq!(
                seen, expected,
                "case {case:?}: expected the whole listing {expected:?} exactly once, got {seen:?}"
            );
        }
    }
}

/// RULE: a page never exceeds the service's own page size, however large a `maxResults`
/// the caller asks for, and the roles it did not fit are reachable through its cursor.
#[tokio::test]
async fn a_page_is_capped_at_the_services_page_size() {
    let runtime = Runtime::new();
    let tenant_id = "aa-capped";
    let expected = runtime.seed_counted(tenant_id, MAX_PAGE_SIZE + 1);

    for requested in [MAX_PAGE_SIZE + 1, MAX_PAGE_SIZE * 10, usize::from(u16::MAX)] {
        let case = format!("maxResults={requested}");
        let response = runtime
            .list_roles(tenant_id, Some(requested), None)
            .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "case {case:?}: expected 200, got {} {:?}",
            response.status(),
            body_text(&response)
        );
        let first = reported_role_arns(&response);
        assert_eq!(
            first.len(),
            MAX_PAGE_SIZE,
            "case {case:?}: expected a page of {MAX_PAGE_SIZE}, got {}",
            first.len()
        );
        let token = next_token(&response).unwrap_or_else(|| {
            panic!("case {case:?}: a truncated listing must carry a nextToken")
        });
        let response = runtime
            .list_roles(tenant_id, Some(requested), Some(&token))
            .await;
        let rest = reported_role_arns(&response);
        let seen: Vec<String> = first.into_iter().chain(rest).collect();
        assert_eq!(
            seen, expected,
            "case {case:?}: the two pages must be the whole listing exactly once"
        );
        assert!(
            next_token(&response).is_none(),
            "case {case:?}: the last page must omit nextToken, got {:?}",
            body_text(&response)
        );
    }
}

/// RULE: a `maxResults` below one names no page and is refused as a validation failure,
/// whatever the tenant.
#[tokio::test]
async fn a_page_bound_below_one_is_refused() {
    let runtime = Runtime::new();
    for tenant_id in [TENANT, UNKNOWN_TENANT] {
        for requested in ["0", "-1", "-2147483648"] {
            let case = format!("{tenant_id} maxResults={requested}");
            let response = runtime
                .get(&format!(
                    "/access?tenantId={tenant_id}&maxResults={requested}"
                ))
                .await;
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "case {case:?}: expected 400, got {} {:?}",
                response.status(),
                body_text(&response)
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                body_text(&response)
            );
            assert!(
                error_message(&response).contains("maxResults"),
                "case {case:?}: the refusal must name the member, got {:?}",
                error_message(&response)
            );
        }
    }
}

/// RULE: a `nextToken` this operation did not issue for the tenant's own role listing is
/// refused rather than applied to it.
#[tokio::test]
async fn a_cursor_this_listing_did_not_issue_is_refused() {
    let runtime = Runtime::new();
    let arns = fixture_role_arns();
    let first = arns.first().cloned().unwrap_or_default();
    // A cursor minted for ANOTHER tenant's listing, and the same cursor spelled with
    // upper-case hex digits — a spelling no response of this service ever carried.
    let other_tenant_cursor =
        state::encode_cursor(&state::roles_cursor_scope(OTHER_TENANT), &first);
    let own_cursor = state::encode_cursor(&state::roles_cursor_scope(TENANT), &first);
    let cases: &[(&str, String)] = &[
        ("not hex", "not-a-cursor".to_string()),
        ("empty", String::new()),
        ("another tenant's cursor", other_tenant_cursor),
        ("upper-case spelling", own_cursor.to_uppercase()),
        ("truncated", own_cursor.chars().take(own_cursor.len() / 2).collect()),
    ];

    for (case, token) in cases {
        let response = runtime.list_roles(TENANT, Some(1), Some(token)).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "case {case:?}: expected 400 for token {token:?}, got {} {:?}",
            response.status(),
            body_text(&response)
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            body_text(&response)
        );
    }

    // And the cursor the listing DID issue resumes it.
    let response = runtime.list_roles(TENANT, Some(1), Some(&own_cursor)).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the cursor this listing issued must be accepted, got {} {:?}",
        response.status(),
        body_text(&response)
    );
    assert_eq!(
        reported_role_arns(&response),
        arns.get(1..2).unwrap_or_default().to_vec(),
        "the issued cursor must resume immediately after the role it names"
    );
}

/// RULE: `GetRoleCredentials` mints a credential set for a role the tenant is entitled to,
/// records it, and reports exactly the recorded values — expiring at the window the
/// request asked for, or the service default when it asked for none.
#[tokio::test]
async fn get_role_credentials_reports_the_session_it_persisted() {
    let runtime = Runtime::new();
    // Rows differ along the role (so the role's own maximum differs), whether a session
    // name is given, and whether a duration is given. The last field is the window the
    // response must advertise: the asked-for duration, or the service default capped by
    // the role's maximum when the request asks for none.
    let mut cases: Vec<(String, Option<&str>, Option<i64>, i64)> = Vec::new();
    for arn in fixture_role_arns() {
        let (_, _, _, max_seconds) = fixture_row(&arn);
        let max = max_seconds as i64;
        let default = (DEFAULT_SESSION_DURATION.as_secs() as i64).min(max);
        cases.push((arn.clone(), None, None, default));
        cases.push((arn.clone(), Some("demo"), None, default));
        cases.push((arn.clone(), Some("demo_session.1"), Some(1), 1));
        let middle = (max / 2).max(1);
        cases.push((arn.clone(), None, Some(middle), middle));
        cases.push((arn, Some("at-the-role-maximum"), Some(max), max));
    }

    let mut minted_ids: Vec<String> = Vec::new();
    for (arn, session_name, session_duration, expected_window) in cases {
        let case = format!("{arn} name={session_name:?} duration={session_duration:?}");
        let before = SystemTime::now();
        let response = runtime
            .get_role_credentials(TENANT, &arn, session_name, session_duration)
            .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "case {case:?}: expected 200, got {} {:?}",
            response.status(),
            body_text(&response)
        );

        let json = body_json(&response);
        let details = json["credentialsDetails"]["programmaticAccess"].clone();
        let reported = |member: &str| {
            details[member]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        let (access_key_id, secret_access_key, session_token) = (
            reported("accessKeyId"),
            reported("secretAccessKey"),
            reported("sessionToken"),
        );
        for (member, value) in [
            ("accessKeyId", &access_key_id),
            ("secretAccessKey", &secret_access_key),
            ("sessionToken", &session_token),
        ] {
            assert!(
                !value.is_empty(),
                "case {case:?}: {member} is required, got {details:?}"
            );
        }
        let expires_at = details["expiresAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expiresAt must be an epoch-seconds number, got {details:?}")
        }) as i64;

        // Every reported value is the one the call stored, read back out of state.
        let store = runtime.state();
        let store = store.lock();
        let stored = store
            .session(TENANT, &access_key_id, before)
            .unwrap_or_else(|| panic!("case {case:?}: no session was recorded for {access_key_id}"));
        assert_eq!(
            (
                stored.secret_access_key.as_str(),
                stored.session_token.as_str(),
                epoch_seconds(stored.expires_at),
            ),
            (secret_access_key.as_str(), session_token.as_str(), expires_at),
            "case {case:?}: the response must report the stored credential set"
        );
        assert_eq!(
            (
                stored.tenant_id.as_str(),
                stored.role_arn.as_str(),
                stored.session_name.as_deref(),
            ),
            (TENANT, arn.as_str(), session_name),
            "case {case:?}: the stored session must record the request it was minted for"
        );
        assert!(
            store.session(OTHER_TENANT, &access_key_id, before).is_none(),
            "case {case:?}: a session minted for {TENANT} must not resolve under another \
             tenant"
        );
        drop(store);

        // The advertised expiry is the window asked for, measured from the call.
        let elapsed_allowance = 5;
        assert!(
            (expires_at - epoch_seconds(before) - expected_window).abs() <= elapsed_allowance,
            "case {case:?}: expected expiresAt about {expected_window} seconds after the call, \
             got {expires_at} against a call at {}",
            epoch_seconds(before)
        );

        // Each call is its own role session: no two share a credential.
        assert!(
            !minted_ids.contains(&access_key_id),
            "case {case:?}: access key id {access_key_id} was already minted for another call"
        );
        minted_ids.push(access_key_id);
    }
}

/// RULE: a role that is not one of the tenant's entitlements is refused with
/// `AccessDeniedException` (HTTP 403), and nothing is minted for it.
#[tokio::test]
async fn a_role_the_tenant_is_not_entitled_to_is_refused() {
    let runtime = Runtime::new();
    let revoked = role_arn("222222222222", "Audit-Role_1");
    {
        let store = runtime.state();
        let mut store = store.lock();
        assert!(
            store.remove_role(TENANT, &revoked),
            "the fixture seeds {revoked} so revoking it must find it"
        );
    }

    let cases: &[(&str, &str, String)] = &[
        (
            "a role of another tenant",
            TENANT,
            other_tenant_role(),
        ),
        (
            "a role nobody seeded",
            TENANT,
            role_arn("999999999999", "not-entitled"),
        ),
        (
            "a revoked entitlement",
            TENANT,
            revoked.clone(),
        ),
        (
            "a tenant with no entitlements",
            UNKNOWN_TENANT,
            role_arn("000000000000", "AdministratorAccess"),
        ),
        (
            "a value that is not an ARN",
            TENANT,
            "AdministratorAccess".to_string(),
        ),
    ];

    for (case, tenant_id, arn) in cases {
        let sessions_before = runtime.state().lock().sessions().len();
        let response = runtime
            .get_role_credentials(tenant_id, arn, None, None)
            .await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "case {case:?}: expected 403 for {arn:?}, got {} {:?}",
            response.status(),
            body_text(&response)
        );
        assert_eq!(
            error_code(&response),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {:?}",
            body_text(&response)
        );
        assert!(
            error_message(&response).contains(arn.as_str()),
            "case {case:?}: the refusal must name the role asked for, got {:?}",
            error_message(&response)
        );
        assert_eq!(
            runtime.state().lock().sessions().len(),
            sessions_before,
            "case {case:?}: a refused request must mint no credential set"
        );
    }
}

/// RULE: a `sessionDuration` below one second, or longer than the role's own maximum
/// session duration, is refused as a validation failure; every value between those bounds
/// is served.
#[tokio::test]
async fn a_session_duration_outside_the_roles_bounds_is_refused() {
    let runtime = Runtime::new();

    for arn in fixture_role_arns() {
        let (_, _, _, max_seconds) = fixture_row(&arn);
        let max = max_seconds as i64;
        // Each published bound of the role's window, and one past each.
        let refused = [-1, 0, max + 1, max * 2];
        let served = [1, max / 2, max];

        for seconds in refused {
            let case = format!("{arn} sessionDuration={seconds} (role maximum {max})");
            let response = runtime
                .get_role_credentials(TENANT, &arn, None, Some(seconds))
                .await;
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "case {case:?}: expected 400, got {} {:?}",
                response.status(),
                body_text(&response)
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                body_text(&response)
            );
            assert!(
                error_message(&response).contains("sessionDuration"),
                "case {case:?}: the refusal must name the member, got {:?}",
                error_message(&response)
            );
        }

        for seconds in served {
            let case = format!("{arn} sessionDuration={seconds} (role maximum {max})");
            let response = runtime
                .get_role_credentials(TENANT, &arn, None, Some(seconds))
                .await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "case {case:?}: expected 200, got {} {:?}",
                response.status(),
                body_text(&response)
            );
        }
    }
}

/// RULE: a minted credential set is forgotten once the clock passes the `expiresAt` the
/// response advertised — the expiry is owed at its own instant, not at the next call about
/// that role.
#[tokio::test]
async fn a_credential_set_is_ended_when_the_clock_makes_it_spent() {
    let runtime = Runtime::new();
    let arn = fixture_role_arns()
        .first()
        .cloned()
        .expect("the fixture seeds at least one role");

    let response = runtime
        .get_role_credentials(TENANT, &arn, Some("short-lived"), Some(1))
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "expected 200, got {} {:?}",
        response.status(),
        body_text(&response)
    );
    let access_key_id = body_json(&response)["credentialsDetails"]["programmaticAccess"]
        ["accessKeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        runtime.state().lock().sessions().len(),
        1,
        "the call must record exactly the one credential set it minted"
    );

    tokio::time::sleep(Duration::from_millis(1_100)).await;

    // Any later call materialises what the clock owed; the listing is the one that
    // reports nothing about sessions, so the sweep is not a side effect of asking again
    // for the same role.
    let response = runtime.list_roles(TENANT, None, None).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "expected 200, got {} {:?}",
        response.status(),
        body_text(&response)
    );
    let store = runtime.state();
    let store = store.lock();
    assert!(
        store.sessions().is_empty(),
        "a spent credential set must be forgotten, still holding {:?}",
        store.sessions()
    );
    assert!(
        store
            .session(TENANT, &access_key_id, SystemTime::now())
            .is_none(),
        "the spent credential set {access_key_id} must no longer resolve"
    );
}
