// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListWebApps`.
//!
//! Every test drives the real wire path through the shared [`crate::harness`] — `POST /`
//! with `X-Amz-Target: TransferService.<Op>` — so what is asserted is what an SDK sees,
//! not what a handler returns. The web apps are CREATED through `CreateWebApp`, so what
//! the listing reports has to come out of the record that create stored; nothing here
//! asserts a hardcoded answer.
//!
//! RULE under test: `ListWebApps` reports, for every web app the account holds, the
//! members stored on that record (`Arn`, `WebAppId`, `AccessEndpoint`, `WebAppEndpoint`,
//! `EndpointType`) in `WebAppId` order, at most `MaxResults` per page, with a `NextToken`
//! only while more remain — and, being a read, it changes nothing a later caller sees.
//! `web_app_lifecycle.rs` owns the create/describe side of the same records; this suite
//! owns the listing.
//!
//! Each suite asserts that rule over a TABLE of inputs rather than one call: the rows
//! differ along which members the create named, which `WebAppEndpointType` the web app
//! ends up with, how many web apps are stored, and which page bound the caller asked for.

use serde_json::{json, Value};

use crate::harness::{
    assert_members, drain_pages, is_modelled_web_app_id, member_of, Emulator, ACCOUNT, REGION, ROLE,
};

/// An IAM Identity Center instance ARN satisfying `IdentityCenterInstanceArn`'s modelled
/// pattern `^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
const INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// The `IdentityProviderDetails` union every `CreateWebApp` must send:
/// `IdentityCenterConfig` is the union's only member and the union is `@required`.
fn identity_provider_details() -> Value {
    json!({"IdentityCenterConfig": {"InstanceArn": INSTANCE_ARN, "Role": ROLE}})
}

/// Create one web app from `extra` (merged over the required identity provider details)
/// and return its minted `WebAppId`.
async fn create_web_app(emulator: &Emulator, case: &str, extra: &Value) -> String {
    let mut request = json!({"IdentityProviderDetails": identity_provider_details()});
    for (member, value) in extra.as_object().expect("the row's members are an object") {
        request[member] = value.clone();
    }
    let answer = emulator.call("CreateWebApp", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWebApp 200, got {} with body {}",
        answer.status, answer.body
    );
    let id = member_of(&answer.body, "WebAppId")
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateWebApp returned no WebAppId: {}", answer.body)
        })
        .to_string();
    assert!(
        is_modelled_web_app_id(&id),
        "case {case:?}: expected a modelled WebAppId, got {id:?}"
    );
    id
}

/// The `WebApp` structure `DescribeWebApp` rebuilds for `web_app_id`: the same stored
/// record the listing projects, so it is what a listed entry is checked against.
async fn described(emulator: &Emulator, case: &str, web_app_id: &str) -> Value {
    let answer = emulator
        .call("DescribeWebApp", json!({"WebAppId": web_app_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeWebApp 200, got {} with body {}",
        answer.status, answer.body
    );
    member_of(&answer.body, "WebApp").clone()
}

/// Every web app `ListWebApps` reports in ONE unbounded call, which is the whole set for
/// the handful of web apps these suites create (far below the page bound), so the answer
/// must carry no `NextToken`. Paging itself is asserted over [`drain_pages`].
async fn listed(emulator: &Emulator) -> Vec<Value> {
    let answer = emulator.call("ListWebApps", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "ListWebApps: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer.get("NextToken").is_null(),
        "ListWebApps: expected a single unbounded page with no NextToken, got {}",
        answer.body
    );
    answer.get("WebApps").as_array().cloned().unwrap_or_default()
}

/// The `WebAppId` of each entry of a listing, in the order it was reported.
fn ids_of(web_apps: &[Value]) -> Vec<String> {
    web_apps
        .iter()
        .map(|app| {
            member_of(app, "WebAppId")
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Projection: what a listed web app carries.
// ---------------------------------------------------------------------------

/// RULE: every web app the account holds is listed once, in `WebAppId` order, and each
/// listed entry carries the members stored on that record — the ARN, the web app's own
/// endpoint, its `AccessEndpoint` and its `EndpointType` — exactly as `DescribeWebApp`
/// rebuilds them from the same record.
///
/// The rows span what the rule generalises over: a create that names no optional member
/// (so the service resolves the defaults), one that names a custom `AccessEndpoint`, one
/// that asks for units and a `STANDARD` endpoint policy, and one placed in a VPC — which
/// is what makes the stored `EndpointType` `VPC` rather than `PUBLIC`, i.e. both values
/// of the `WebAppEndpointType` enum (`[PUBLIC, VPC]`).
#[tokio::test]
async fn every_listed_web_app_projects_the_record_describe_reports() {
    let emulator = Emulator::new();

    let rows: Vec<(&str, Value)> = vec![
        ("no optional member: every default resolved", json!({})),
        (
            "a custom access endpoint",
            json!({"AccessEndpoint": "https://files.example.invalid"}),
        ),
        (
            "units and an endpoint policy",
            json!({"WebAppUnits": {"Provisioned": 2}, "WebAppEndpointPolicy": "STANDARD"}),
        ),
        (
            "hosted in a VPC",
            json!({"EndpointDetails": {"Vpc": {
                "VpcId": "vpc-0123456789abcdef0",
                "SubnetIds": ["subnet-0123456789abcdef0", "subnet-00000000000000001"],
            }}}),
        ),
    ];

    let mut created: Vec<(String, String)> = Vec::new();
    for (case, extra) in &rows {
        let id = create_web_app(&emulator, case, extra).await;
        created.push(((*case).to_string(), id));
    }

    let web_apps = listed(&emulator).await;
    let mut want_ids: Vec<String> = created.iter().map(|(_, id)| id.clone()).collect();
    want_ids.sort();
    assert_eq!(
        ids_of(&web_apps),
        want_ids,
        "case \"every created web app once, in WebAppId order\": expected {want_ids:?}, got {:?}",
        ids_of(&web_apps)
    );

    // Each entry must agree, member by member, with the record `DescribeWebApp` reports:
    // `ListedWebApp` is a projection of the same stored web app, not a second copy of it.
    let mut endpoint_types: Vec<String> = Vec::new();
    for (case, id) in &created {
        let found = web_apps
            .iter()
            .find(|app| member_of(app, "WebAppId").as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: expected {id} listed, got {web_apps:?}"));
        let description = described(&emulator, case, id).await;

        assert_members(
            case,
            found,
            &json!({
                "WebAppId": id,
                "Arn": member_of(&description, "Arn"),
                "AccessEndpoint": member_of(&description, "AccessEndpoint"),
                "WebAppEndpoint": member_of(&description, "WebAppEndpoint"),
                "EndpointType": member_of(&description, "EndpointType"),
            }),
        );
        // The ARN is the one this account's web app carries, not a repeat of whatever
        // the description happened to answer.
        assert_eq!(
            member_of(found, "Arn").as_str(),
            Some(format!("arn:aws:transfer:{REGION}:{ACCOUNT}:webapp/{id}").as_str()),
            "case {case:?}: expected the stored web app ARN, got {:?}",
            member_of(found, "Arn")
        );
        // `ListedWebApp` models these five members and no others, so a listing reports
        // the description's heavier members nowhere.
        for unmodelled in [
            "DescribedIdentityProviderDetails",
            "DescribedEndpointDetails",
            "WebAppUnits",
            "WebAppEndpointPolicy",
            "Tags",
        ] {
            assert!(
                member_of(found, unmodelled).is_null(),
                "case {case:?}: ListedWebApp models no {unmodelled}, got {:?}",
                member_of(found, unmodelled)
            );
        }
        endpoint_types.push(
            member_of(found, "EndpointType")
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    // The rows above cover both values of the modelled `WebAppEndpointType` enum.
    for want in ["PUBLIC", "VPC"] {
        assert!(
            endpoint_types.iter().any(|value| value == want),
            "case \"endpoint type coverage\": expected a listed {want} web app, got {endpoint_types:?}"
        );
    }
}

/// RULE: an account with no web apps lists an empty `WebApps` and no `NextToken`, and the
/// listing — a read — leaves the stored web apps exactly as it found them, so a later
/// caller sees the same thing.
#[tokio::test]
async fn listing_is_a_read_that_changes_nothing() {
    let emulator = Emulator::new();

    for case in ["first call", "second call"] {
        let answer = emulator.call("ListWebApps", json!({})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.get("WebApps"),
            &json!([]),
            "case {case:?}: expected an empty WebApps, got {}",
            answer.body
        );
        assert!(
            answer.get("NextToken").is_null(),
            "case {case:?}: expected no NextToken, got {}",
            answer.body
        );
    }

    let mut want_ids = vec![
        create_web_app(&emulator, "read-only row 1", &json!({})).await,
        create_web_app(
            &emulator,
            "read-only row 2",
            &json!({"AccessEndpoint": "https://files.example.invalid"}),
        )
        .await,
    ];
    want_ids.sort();

    let first = listed(&emulator).await;
    let second = listed(&emulator).await;
    assert_eq!(
        first, second,
        "case \"repeated read\": expected the same answer twice, got {first:?} then {second:?}"
    );
    assert_eq!(
        ids_of(&second),
        want_ids,
        "case \"repeated read\": expected the created web apps untouched, got {:?}",
        ids_of(&second)
    );
    // And the read minted nothing: a reporting operation never grows the store.
    assert_eq!(
        emulator.store().lock().web_apps.len(),
        want_ids.len(),
        "case \"repeated read\": expected {} stored web app(s), got {}",
        want_ids.len(),
        emulator.store().lock().web_apps.len()
    );
}

// ---------------------------------------------------------------------------
// Pagination.
// ---------------------------------------------------------------------------

/// RULE: `ListWebApps` returns at most `MaxResults` web apps, hands back a `NextToken`
/// only while more remain, and resuming from that token walks the whole stored set
/// exactly once in `WebAppId` order.
///
/// The rows are the page bounds: both published bounds of `MaxResults`' modelled
/// `range 1..1000`, sizes below/equal to/above the stored count, and no bound at all.
#[tokio::test]
async fn list_web_apps_honours_every_page_bound_and_resumes_from_its_token() {
    let emulator = Emulator::new();

    let mut created: Vec<String> = Vec::new();
    for n in 0..5 {
        // Alternate the created shape so the pages are not rows of one kind of web app.
        let extra = if n % 2 == 0 {
            json!({})
        } else {
            json!({"AccessEndpoint": format!("https://files-{n}.example.invalid")})
        };
        created.push(create_web_app(&emulator, &format!("page row {n}"), &extra).await);
    }
    created.sort();

    let base = json!({});
    for limit in [Some(1), Some(2), Some(4), Some(5), Some(6), Some(1000), None] {
        let (page_sizes, keys) =
            drain_pages(&emulator, "ListWebApps", &base, "WebApps", "WebAppId", limit).await;
        let case = format!("ListWebApps MaxResults={limit:?}");

        assert_eq!(
            keys, created,
            "case {case:?}: expected every web app once in WebAppId order, got {keys:?}"
        );
        match limit {
            Some(limit) => {
                let limit = usize::try_from(limit).expect("a positive page bound");
                for size in &page_sizes {
                    assert!(
                        *size <= limit,
                        "case {case:?}: a page carried {size} items, above the bound {limit}"
                    );
                }
                let want_pages = created.len().div_ceil(limit);
                assert_eq!(
                    page_sizes.len(),
                    want_pages,
                    "case {case:?}: expected {want_pages} page(s), got {page_sizes:?}"
                );
            }
            None => assert_eq!(
                page_sizes,
                vec![created.len()],
                "case {case:?}: expected a single unbounded page, got {page_sizes:?}"
            ),
        }
    }
}

/// RULE: a `NextToken` is a POSITION in the ordered set, so resuming from one lists
/// exactly the web apps that sort after it — including when the web app the token named
/// is gone by the time the caller comes back.
///
/// `DeleteWebApp` is still a scaffold stub (its own issue), so the records are released
/// through the shared store the handlers read rather than over the wire; the listing
/// itself is still driven over the wire.
#[tokio::test]
async fn a_token_resumes_after_its_position_even_when_that_web_app_is_gone() {
    let emulator = Emulator::new();

    let mut created: Vec<String> = Vec::new();
    for n in 0..4 {
        created.push(create_web_app(&emulator, &format!("token row {n}"), &json!({})).await);
    }
    created.sort();

    let first = emulator.call("ListWebApps", json!({"MaxResults": 2})).await;
    let token = first
        .get("NextToken")
        .as_str()
        .unwrap_or_else(|| panic!("expected a NextToken after a page of 2: {}", first.body))
        .to_string();

    // Release the web app the token points AT (the last of the first page) and one that
    // sorts after it, so the case covers both a deleted boundary and a shrunk tail.
    {
        let state = emulator.store();
        let mut guard = state.lock();
        guard.web_apps.remove(&created[1]);
        guard.web_apps.remove(&created[3]);
    }

    let resumed = emulator
        .call("ListWebApps", json!({"NextToken": token}))
        .await;
    assert_eq!(
        resumed.status, 200,
        "case \"resume after a deleted boundary\": expected 200, got {} with body {}",
        resumed.status, resumed.body
    );
    let ids = ids_of(&resumed.get("WebApps").as_array().cloned().unwrap_or_default());
    assert_eq!(
        ids,
        vec![created[2].clone()],
        "case \"resume after a deleted boundary\": expected only the web app that sorts after \
         the token, got {ids:?}"
    );
}

/// RULE: a `NextToken` this service did not issue is refused with
/// `InvalidNextTokenException` (400) rather than silently restarting the listing, and a
/// `MaxResults` outside the modelled `range 1..1000` is refused at the boundary.
#[tokio::test]
async fn an_unissued_next_token_and_an_out_of_range_max_results_are_refused() {
    let emulator = Emulator::new();
    create_web_app(&emulator, "a web app to list", &json!({})).await;

    // Rows: not hex at all, odd-length hex, and valid hex that is not UTF-8.
    for token in ["not-a-token", "zz", "abc", "ff", "c0ffee00"] {
        let answer = emulator
            .call("ListWebApps", json!({"NextToken": token}))
            .await;
        let case = format!("ListWebApps NextToken={token:?}");
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidNextTokenException",
            "case {case:?}: expected InvalidNextTokenException, got {}",
            answer.body
        );
    }

    // One PAST each published bound of `MaxResults`' modelled range 1..1000: the
    // framework's boundary validator refuses it with `ValidationException` before the
    // handler runs.
    for bad in [0, 1001] {
        let answer = emulator
            .call("ListWebApps", json!({"MaxResults": bad}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case \"MaxResults={bad}\": expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case \"MaxResults={bad}\": expected ValidationException, got {}",
            answer.body
        );
    }

    // Both published bounds themselves are accepted.
    for good in [1, 1000] {
        let answer = emulator
            .call("ListWebApps", json!({"MaxResults": good}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case \"MaxResults={good}\": expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
    }
}
