// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListPolicies`: the policy catalog this service reports.
//!
//! Every request goes over the wire the real client uses: a `restJson1` HTTP request
//! (`POST /policies/?List`, `MaxResults` and `NextToken` bound to the query string)
//! dispatched through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON
//! envelope and the HTTP status are all exercised.
//!
//! The rule under test: `ListPolicies` reports the policies the store holds, in ARN
//! order, in pages of at most `MaxResults`, carrying a `NextToken` exactly while more
//! remain — so the tables below span CATALOG SIZE (the seeded catalog, and catalogs
//! grown by recording more policies) crossed with PAGE SIZE (each published bound of
//! `MaxResults`, the sizes between, and one past the maximum), rather than one recorded
//! catalog. Each entry's members are checked against the record they came from and
//! against the model's own catalogs (`PolicyType`'s enum values, `PolicyName`'s
//! `length(max: 64)`, `PolicyVersionId`'s `range(min: 1)`, `UnqualifiedPolicyArn`'s
//! pattern).

use std::collections::BTreeSet;

use serde_json::Value;
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{error_code, text, Harness};

/// `MaxResults` is modelled `range(min: 1, max: 20)` with `default: 20`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 20;

/// `PolicyName` is `length(max: 64)`.
const MAX_POLICY_NAME: usize = 64;

/// `UnqualifiedPolicyArn` is `length(max: 1224)`.
const MAX_POLICY_ARN: usize = 1224;

/// One catalog record as the STORE holds it: `(Arn, Name, PolicyType, DefaultVersion)`.
/// This is what a listed entry must be computed from.
type StoredPolicy = (String, String, String, i64);

/// `POST /policies/?List`, with `query` appended to the `?List` marker.
async fn list_policies(harness: &Harness, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        "/policies/?List".to_string()
    } else {
        format!("/policies/?List&{query}")
    };
    harness.call("POST", &uri, None).await
}

/// The catalog as the store holds it, in ARN order.
fn stored_catalog(harness: &Harness) -> Vec<StoredPolicy> {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard
        .policies
        .values()
        .map(|policy| {
            (
                policy.arn.clone(),
                policy.name.clone(),
                policy.policy_type.as_wire().to_string(),
                i64::from(policy.default_version),
            )
        })
        .collect()
}

/// Record one more policy in the catalog. No modelled operation mints a policy — the
/// catalog is AWS's — so a table that needs a bigger one writes it directly, the way
/// `record_management_account` records an organization.
fn record_policy(harness: &Harness, category: &str, name: &str, policy_type: state::PolicyType) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    store.lock().insert_policy(state::CatalogPolicy {
        arn: state::policy_arn(category, name),
        name: name.to_string(),
        policy_type,
        default_version: state::FIRST_POLICY_VERSION,
    });
}

/// A policy name for row `n`, inside `PolicyName`'s `length(max: 64)`.
fn policy_name(n: usize) -> String {
    format!("TestPolicy{n:03}")
}

/// Grow the catalog by `extra` policies, alternating the `PolicyType` token so no table
/// below tests a single value of it.
fn grow_catalog(harness: &Harness, extra: usize) {
    for n in 1..=extra {
        let policy_type = if n % 2 == 0 {
            state::PolicyType::AwsRam
        } else {
            state::PolicyType::AwsManaged
        };
        record_policy(harness, "IdentityCenter", &policy_name(n), policy_type);
    }
}

/// Every member of one listed entry, against the record it came from and the model's
/// constraints.
fn assert_entry(case: &str, listed: &Value, stored: &StoredPolicy) {
    let (arn, name, policy_type, default_version) = stored;
    assert_eq!(
        &text(listed, "Arn"),
        arn,
        "case {case:?}: expected Arn {arn:?}, got {listed:?}"
    );
    assert_eq!(
        &text(listed, "Name"),
        name,
        "case {case:?}: expected Name {name:?}, got {listed:?}"
    );
    assert_eq!(
        &text(listed, "PolicyType"),
        policy_type,
        "case {case:?}: expected PolicyType {policy_type:?}, got {listed:?}"
    );
    assert_eq!(
        listed.get("DefaultVersion").and_then(Value::as_i64),
        Some(*default_version),
        "case {case:?}: expected DefaultVersion {default_version}, got {listed:?}"
    );

    // The model's own catalogs and bounds, not this test's opinion.
    assert!(
        model::POLICY_TYPE_VALUES.contains(&policy_type.as_str()),
        "case {case:?}: PolicyType {policy_type:?} is not a token the model declares \
         ({:?})",
        model::POLICY_TYPE_VALUES
    );
    assert!(
        !name.is_empty() && name.len() <= MAX_POLICY_NAME,
        "case {case:?}: Name {name:?} is {} bytes, outside PolicyName's \
         length(max: {MAX_POLICY_NAME})",
        name.len()
    );
    assert!(
        *default_version >= 1,
        "case {case:?}: DefaultVersion {default_version} is below PolicyVersionId's \
         range(min: 1)"
    );
    assert!(
        is_unqualified_policy_arn(arn),
        "case {case:?}: Arn {arn:?} does not satisfy UnqualifiedPolicyArn's pattern"
    );
}

/// Whether `arn` satisfies `UnqualifiedPolicyArn`'s modelled pattern
/// `^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\.-]{1,1023}/[a-zA-Z0-9_\.-]{1,1023}$` and
/// its `length(max: 1224)`.
fn is_unqualified_policy_arn(arn: &str) -> bool {
    if arn.len() > MAX_POLICY_ARN {
        return false;
    }
    let Some(rest) = arn.strip_prefix("arn:") else {
        return false;
    };
    let Some((partition, rest)) = rest.split_once(':') else {
        return false;
    };
    if partition.is_empty() || partition.len() > 63 {
        return false;
    }
    let Some(path) = rest.strip_prefix("mpa:::aws:policy/") else {
        return false;
    };
    let segments: Vec<&str> = path.split('/').collect();
    segments.len() == 2
        && segments.iter().all(|segment| {
            (1..=1023).contains(&segment.len())
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        })
}

/// The catalog a fresh store holds is reported as the store holds it — ARN order, every
/// member read out of the record, no `NextToken` while the default page covers it.
#[tokio::test]
async fn the_seeded_catalog_is_reported_out_of_state() {
    // No organization is recorded: reading the catalog is not an organization-only
    // operation, so it answers a plain caller.
    let harness = Harness::without_organization();
    let stored = stored_catalog(&harness);
    assert!(
        !stored.is_empty(),
        "a fresh store must seed the policy catalog; it holds {stored:?}"
    );

    let (status, body) = list_policies(&harness, "").await;
    assert_eq!(status, 200, "ListPolicies returned {status} with {body:?}");
    let listed = body["Policies"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        listed.len(),
        stored.len(),
        "the default MaxResults of {MAX_RESULTS_MAX} must hold the seeded catalog: {body:?}"
    );
    for (entry, record) in listed.iter().zip(stored.iter()) {
        assert_entry("seeded catalog", entry, record);
    }
    assert!(
        body.get("NextToken").is_none(),
        "a complete page carries no NextToken: {body:?}"
    );

    // A read changes nothing a later caller sees: the same call answers the same, and
    // the store is exactly as it was.
    let (status, again) = list_policies(&harness, "").await;
    assert_eq!(status, 200, "the second ListPolicies returned {again:?}");
    assert_eq!(again, body, "ListPolicies is a read: two calls must agree");
    assert_eq!(
        stored_catalog(&harness),
        stored,
        "ListPolicies must not change the catalog it reports"
    );
}

/// Paging walks the catalog in ARN order and shows every policy exactly once, for every
/// catalog size and every page size `MaxResults` allows.
#[tokio::test]
async fn paging_walks_every_policy_exactly_once() {
    // Catalog sizes: the seeded catalog alone, sizes around one page, the page bound
    // itself, and sizes well past it so several pages are needed.
    for extra in [0_usize, 1, 2, 6, 19, 20, 45] {
        let harness = Harness::without_organization();
        grow_catalog(&harness, extra);
        let stored = stored_catalog(&harness);
        let expected: Vec<String> = stored.iter().map(|(arn, ..)| arn.clone()).collect();

        // Each published bound of `MaxResults` (1 and 20) and the sizes between.
        for page_size in [MAX_RESULTS_MIN, 2, 3, 7, MAX_RESULTS_MAX] {
            let case = format!("{} policies, MaxResults={page_size}", stored.len());
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let query = match &token {
                    None => format!("MaxResults={page_size}"),
                    Some(t) => format!("MaxResults={page_size}&NextToken={t}"),
                };
                let (status, body) = list_policies(&harness, &query).await;
                assert_eq!(status, 200, "case {case:?}: ListPolicies returned {body:?}");
                let page = body["Policies"].as_array().cloned().unwrap_or_default();
                assert!(
                    page.len() as i64 <= page_size,
                    "case {case:?}: a page of {} exceeds MaxResults",
                    page.len()
                );
                for listed in &page {
                    let arn = text(listed, "Arn");
                    let record = stored
                        .iter()
                        .find(|(stored_arn, ..)| stored_arn == &arn)
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: {arn:?} is in no stored record: {listed:?}")
                        });
                    assert_entry(&case, listed, record);
                    seen.push(arn);
                }
                pages += 1;
                assert!(
                    pages <= stored.len() + 1,
                    "case {case:?}: paging did not terminate"
                );
                match body.get("NextToken").and_then(Value::as_str) {
                    Some(next) => {
                        assert_eq!(
                            Some(next.to_string()),
                            page.last().map(|listed| text(listed, "Arn")),
                            "case {case:?}: a NextToken must name the last entry of its page"
                        );
                        token = Some(next.to_string());
                    }
                    None => break,
                }
            }
            assert_eq!(
                seen, expected,
                "case {case:?}: paging must yield every policy exactly once, in ARN order"
            );
            assert_eq!(
                seen.iter().collect::<BTreeSet<_>>().len(),
                seen.len(),
                "case {case:?}: paging returned a policy twice: {seen:?}"
            );
        }

        // `MaxResults` omitted takes the modelled default of 20.
        let (status, body) = list_policies(&harness, "").await;
        assert_eq!(status, 200, "the default page returned {body:?}");
        let page = body["Policies"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            page.len(),
            stored.len().min(MAX_RESULTS_MAX as usize),
            "the default MaxResults of {MAX_RESULTS_MAX} must fill the page: {body:?}"
        );
        assert_eq!(
            body.get("NextToken").is_some(),
            stored.len() > MAX_RESULTS_MAX as usize,
            "a NextToken must be present exactly while policies remain: {body:?}"
        );
    }
}

/// A `NextToken` this service never issued is refused with `ValidationException` (400),
/// whatever its form — an ARN the catalog does not hold is one of those forms.
#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let harness = Harness::without_organization();
    grow_catalog(&harness, 3);

    // (case, the token). `%20` is a percent-encoded space: the query decoder hands the
    // handler " ".
    let rows: [(&str, String); 6] = [
        ("not a token at all", "not-a-token".to_string()),
        ("a number", "12".to_string()),
        ("a space", "%20".to_string()),
        (
            "a policy ARN the catalog does not hold",
            state::policy_arn("IdentityCenter", "NeverSeeded"),
        ),
        (
            "an approval-team ARN",
            format!(
                "arn:aws:mpa:us-west-2:{}:approval-team/team",
                engine::emulator_account_id()
            ),
        ),
        (
            "a qualified policy-version ARN",
            format!("{}/1", state::policy_arn("IdentityCenter", &policy_name(1))),
        ),
    ];

    for (case, token) in rows {
        let (status, body) = list_policies(&harness, &format!("NextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a foreign NextToken must be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: wrong error code in {body:?}"
        );
    }

    // The token the service DID issue is accepted, so the refusal is about a token's
    // provenance and not about tokens.
    let (status, body) = list_policies(&harness, "MaxResults=1").await;
    assert_eq!(status, 200, "the first page returned {body:?}");
    let token = text(&body, "NextToken");
    assert!(
        !token.is_empty(),
        "a partial page must carry a NextToken: {body:?}"
    );
    let (status, body) = list_policies(&harness, &format!("MaxResults=1&NextToken={token}")).await;
    assert_eq!(
        status, 200,
        "the service's own NextToken must be accepted, got {body:?}"
    );
}

/// `MaxResults` outside its modelled `range(1..20)` is refused before the handler runs:
/// each published bound is accepted, each value one past it is not.
#[tokio::test]
async fn max_results_outside_its_modelled_range_is_refused() {
    let harness = Harness::without_organization();

    // (MaxResults, accepted)
    let rows: [(i64, bool); 5] = [
        (MAX_RESULTS_MIN - 1, false),
        (MAX_RESULTS_MIN, true),
        (MAX_RESULTS_MAX, true),
        (MAX_RESULTS_MAX + 1, false),
        (-1, false),
    ];

    for (max_results, accepted) in rows {
        let (status, body) = list_policies(&harness, &format!("MaxResults={max_results}")).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case MaxResults={max_results}: expected 200, got {status} with {body:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case MaxResults={max_results}: wrong error code in {body:?}"
        );
    }
}
