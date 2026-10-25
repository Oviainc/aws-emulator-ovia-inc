// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the license expiry a product subscription reports, driven over
//! the real wire path: every call goes through the router's `dispatch`, so the request is
//! deserialized, constraint-checked, handled and serialized exactly as a client's would
//! be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded request:
//! `ProductUserSummary.LicenseExpirationDate`
//! (`smithy_query(query="shape", shape="ProductUserSummary")`: "The expiration date of
//! the license associated with this subscription, in ISO 8601 UTC format ... This field
//! applies only to subscriptions that use license server endpoints ... It returns null for
//! products that don't use license-based subscriptions") is reported exactly when the
//! registration the user was subscribed through is backed by a license server endpoint,
//! as the license term after the instant that subscription started, and is absent for
//! every registration that has none.
//!
//! What the suites below pin:
//!
//! * the member is present exactly for the license-backed rows, in every operation that
//!   reports a product user (start, list and stop), and absent for the others — including
//!   a row of the SAME product as a license-backed one, so the member follows the
//!   endpoint and not the product name;
//! * its value is the license term after `SubscriptionStartDate`, in the ISO-8601 UTC
//!   form the member documents;
//! * it is computed from what the store holds, so it does not move between reads;
//! * an endpoint created after the subscription makes that subscription license-based,
//!   and a deletion takes the expiry away again.

use serde_json::{json, Value};

// ONE wire harness per service: the dispatch plumbing, the signing identity, the
// registration and subscription helpers and the endpoint fixtures come from the sibling
// suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, harness, list_subscriptions, provider_union, register, start_subscription,
    stop_subscription, Harness,
};
use super::license_server_endpoint_deletion::delete_endpoint;
use super::license_server_endpoints::{create_endpoint, ENDPOINTS};

/// The license term the emulator issues a license-based subscription's license for, in
/// seconds: 30 days from the subscription start.
///
/// Spelled out here rather than read from the production constant, so a change to the term
/// fails a test instead of silently changing what this suite accepts.
const LICENSE_TERM_SECONDS: i64 = 30 * 24 * 60 * 60;

/// How long a read waits before it re-reads a subscription, long enough to cross the
/// whole-second precision the member is reported at: a value derived from the clock at
/// READ time could not survive this wait, a value derived from the stored subscription
/// start does.
///
/// A real wait, not a paused one: the handlers read the wall clock
/// (`std::time::SystemTime::now()`) rather than a tokio timer, so the test runtime's
/// virtual clock does not move what they see.
const PAST_A_SECOND: std::time::Duration = std::time::Duration::from_millis(1_100);

/// One subscription to exercise. The rows differ along what the rule generalises over:
/// whether the registration has a license server endpoint, the product, the directory-id
/// form, the user-name form and whether a domain is supplied.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`; every `d-` id below is one the
    /// shared harness created in the account's directory fleet.
    directory_id: &'static str,
    /// From `RegisterIdentityProvider`'s `Product` documentation:
    /// VISUAL_STUDIO_ENTERPRISE | VISUAL_STUDIO_PROFESSIONAL | OFFICE_PROFESSIONAL_PLUS |
    /// OFFICE_STANDARD | REMOTE_DESKTOP_SERVICES.
    product: &'static str,
    /// Whether a license server endpoint is created for this row's registration.
    license_backed: bool,
    username: &'static str,
    domain: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "rds sal: remote desktop services behind a license server endpoint",
        directory_id: "d-123456789a",
        product: "REMOTE_DESKTOP_SERVICES",
        license_backed: true,
        username: "alice",
        domain: None,
    },
    Row {
        case: "remote desktop services, shared directory, upn-shaped user, endpoint",
        directory_id: "sd-abcdef0123",
        product: "REMOTE_DESKTOP_SERVICES",
        license_backed: true,
        username: "carol@example.com",
        domain: Some("example.com"),
    },
    Row {
        case: "another product behind a license server endpoint",
        directory_id: "d-0123456789",
        product: "OFFICE_STANDARD",
        license_backed: true,
        username: "dave.o-brien",
        domain: Some("corp.example.com"),
    },
    Row {
        case: "no license server endpoint for this registration",
        directory_id: "d-00000000ff",
        product: "VISUAL_STUDIO_ENTERPRISE",
        license_backed: false,
        username: "bob",
        domain: None,
    },
    Row {
        case: "remote desktop services with no license server endpoint",
        directory_id: "d-0123456789",
        product: "REMOTE_DESKTOP_SERVICES",
        license_backed: false,
        username: "erin",
        domain: Some("example.com"),
    },
];

fn registration(row: &Row) -> Value {
    json!({
        "IdentityProvider": provider_union(row.directory_id),
        "Product": row.product,
        "Settings": {
            "Subnets": ["subnet-0123456789abcdef0"],
            "SecurityGroupId": "sg-0123abcd",
        },
    })
}

fn subscription(row: &Row) -> Value {
    let mut body = json!({
        "Username": row.username,
        "IdentityProvider": provider_union(row.directory_id),
        "Product": row.product,
    });
    if let Some(domain) = row.domain {
        body["Domain"] = json!(domain);
    }
    body
}

/// What seeding one row left in state.
struct Seeded {
    row: &'static Row,
    /// The `IdentityProviderArn` the registration minted.
    provider_arn: String,
    /// The endpoint the row was given, when it is a license-backed one.
    endpoint_arn: Option<String>,
    /// The `ProductUserSummary` the subscribe answered.
    summary: Value,
}

/// Register every row's identity provider, create the license server endpoint for the
/// license-backed rows, and subscribe each row's user.
async fn seed(h: &Harness) -> Vec<Seeded> {
    let mut seeded = Vec::new();
    for row in ROWS {
        let registered = register(h, &registration(row)).await;
        let provider_arn = as_str(
            &registered.ok(row.case)["IdentityProviderSummary"],
            "IdentityProviderArn",
            row.case,
        )
        .to_string();
        let endpoint_arn = if row.license_backed {
            Some(create_license_server_endpoint(h, &provider_arn, row.case).await)
        } else {
            None
        };
        let started = start_subscription(h, &subscription(row)).await;
        let summary = started.ok(row.case)["ProductUserSummary"].clone();
        seeded.push(Seeded {
            row,
            provider_arn,
            endpoint_arn,
            summary,
        });
    }
    seeded
}

/// Create a license server endpoint for `provider_arn`, answering its ARN. The endpoint's
/// `LicenseServerSettings` come from the endpoint suite's fixture, so there is one
/// description of a valid create for this crate.
async fn create_license_server_endpoint(h: &Harness, provider_arn: &str, case: &str) -> String {
    let reply = create_endpoint(h, &ENDPOINTS[0].request(provider_arn)).await;
    as_str(reply.ok(case), "LicenseServerEndpointArn", case).to_string()
}

/// The whole-second instant `value` names, which also asserts `value` is an ISO-8601 UTC
/// instant of the form the member documents (for example `2025-03-15T00:00:00Z`).
fn iso8601_secs(value: &str, case: &str) -> i64 {
    assert!(
        value.len() == 20 && value.contains('T') && value.ends_with('Z'),
        "case {case:?}: expected an ISO-8601 UTC instant, got {value:?}"
    );
    ::aws_smithy_types::DateTime::from_str(value, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {value:?} is not an ISO-8601 UTC instant: {error}")
        })
        .secs()
}

/// The `LicenseExpirationDate` a summary reports, if it reports one.
fn reported_expiry(summary: &Value) -> Option<String> {
    summary
        .get("LicenseExpirationDate")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Assert `summary` carries the expiry the rule requires — the license term after the
/// subscription start when the registration is license-backed, nothing otherwise — and
/// answer what it reported.
fn assert_expiry(summary: &Value, case: &str, license_backed: bool) -> Option<String> {
    let reported = reported_expiry(summary);
    if !license_backed {
        assert!(
            reported.is_none(),
            "case {case:?}: a subscription through a registration with no license server \
             endpoint has no license expiry, got {reported:?} in {summary:?}"
        );
        return None;
    }
    let expiry = reported.clone().unwrap_or_else(|| {
        panic!(
            "case {case:?}: a subscription through a registration with a license server \
             endpoint must report LicenseExpirationDate, got {summary:?}"
        )
    });
    let start = as_str(summary, "SubscriptionStartDate", case);
    assert_eq!(
        iso8601_secs(&expiry, case) - iso8601_secs(start, case),
        LICENSE_TERM_SECONDS,
        "case {case:?}: the expiry must be the license term after the subscription start, \
         got start {start:?} and expiry {expiry:?}"
    );
    reported
}

/// The summary of `username` in a `ListProductSubscriptions` reply.
fn listed_summary(body: &Value, username: &str, case: &str) -> Value {
    body.get("ProductUserSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: ProductUserSummaries is missing from {body:?}"))
        .iter()
        .find(|summary| summary.get("Username").and_then(Value::as_str) == Some(username))
        .unwrap_or_else(|| panic!("case {case:?}: no summary for {username:?} in {body:?}"))
        .clone()
}

/// List the subscriptions of one row's registration and answer the summary of its user.
async fn listed(h: &Harness, row: &Row) -> Value {
    let reply = list_subscriptions(
        h,
        &json!({
            "IdentityProvider": provider_union(row.directory_id),
            "Product": row.product,
        }),
    )
    .await;
    listed_summary(reply.ok(row.case), row.username, row.case)
}

/// RULE: the expiry is reported exactly for a subscription through a registration backed
/// by a license server endpoint, as the license term after its start, by every operation
/// that reports a product user.
#[tokio::test]
async fn the_expiry_is_reported_exactly_for_license_backed_subscriptions() {
    let h = harness();
    let seeded = seed(&h).await;

    for state in &seeded {
        let case = state.row.case;
        let expected = assert_expiry(&state.summary, case, state.row.license_backed);

        let from_list = listed(&h, state.row).await;
        assert_eq!(
            reported_expiry(&from_list),
            expected,
            "case {case:?}: the list must report the expiry the subscribe answered, got \
             {from_list:?}"
        );
        assert_expiry(&from_list, case, state.row.license_backed);
    }
}

/// RULE: the expiry is computed from what the subscription persisted, so re-reading the
/// same subscription answers the same instant however much later the read happens.
#[tokio::test]
async fn the_expiry_does_not_move_between_reads() {
    let h = harness();
    let seeded = seed(&h).await;

    let first: Vec<Option<String>> = {
        let mut values = Vec::new();
        for state in &seeded {
            values.push(reported_expiry(&listed(&h, state.row).await));
        }
        values
    };

    tokio::time::sleep(PAST_A_SECOND).await;

    for (state, before) in seeded.iter().zip(&first) {
        let case = state.row.case;
        let after = reported_expiry(&listed(&h, state.row).await);
        assert_eq!(
            &after, before,
            "case {case:?}: the expiry is derived from the stored subscription, so it must \
             not move between reads"
        );
        assert_eq!(
            after.is_some(),
            state.row.license_backed,
            "case {case:?}: only a license-backed subscription reports an expiry, got \
             {after:?}"
        );
    }
}

/// RULE: the summary `StopProductSubscription` answers reports the same expiry as the
/// subscription it ended.
#[tokio::test]
async fn stopping_a_subscription_reports_the_same_expiry() {
    let h = harness();
    let seeded = seed(&h).await;

    for state in &seeded {
        let case = state.row.case;
        let reply = stop_subscription(&h, &subscription(state.row)).await;
        let summary = reply.ok(case)["ProductUserSummary"].clone();
        assert_eq!(
            reported_expiry(&summary),
            reported_expiry(&state.summary),
            "case {case:?}: the stop must report the expiry of the subscription it ended, \
             got {summary:?}"
        );
        assert_expiry(&summary, case, state.row.license_backed);
    }
}

/// RULE: the member follows the registration's license server endpoint, not the moment the
/// subscription was created — an endpoint created afterwards makes the subscription
/// license-based (its expiry still counted from the stored start), and a deletion takes
/// the expiry away again.
#[tokio::test]
async fn the_expiry_follows_the_registrations_license_server_endpoint() {
    let h = harness();
    let seeded = seed(&h).await;

    // The rows that started without an endpoint: each one gets one now.
    for state in seeded.iter().filter(|state| !state.row.license_backed) {
        let case = state.row.case;
        assert_expiry(&listed(&h, state.row).await, case, false);

        create_license_server_endpoint(&h, &state.provider_arn, case).await;

        let summary = listed(&h, state.row).await;
        let expiry = assert_expiry(&summary, case, true);
        assert_eq!(
            as_str(&summary, "SubscriptionStartDate", case),
            as_str(&state.summary, "SubscriptionStartDate", case),
            "case {case:?}: the subscription's own start must be unchanged, got {summary:?}"
        );
        assert!(
            expiry.is_some(),
            "case {case:?}: the subscription is license-based once its registration has an \
             endpoint, got {summary:?}"
        );
    }
}

/// RULE: a license-backed subscription stops reporting an expiry once the endpoint that
/// backed it is deleted — the member applies only to subscriptions that use a license
/// server endpoint.
#[tokio::test]
async fn deleting_the_endpoint_takes_the_expiry_away() {
    let h = harness();
    let seeded = seed(&h).await;

    for state in seeded.iter().filter(|state| state.row.license_backed) {
        let case = state.row.case;
        let endpoint_arn = state
            .endpoint_arn
            .as_deref()
            .unwrap_or_else(|| panic!("case {case:?}: a license-backed row has an endpoint"));
        assert_expiry(&listed(&h, state.row).await, case, true);

        delete_endpoint(&h, endpoint_arn).await.ok(case);

        assert_expiry(&listed(&h, state.row).await, case, false);
    }
}
