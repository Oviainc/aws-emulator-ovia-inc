// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the subscription-provider store and the service-settings pair,
//! driven over the real wire path: every call goes through the router's `dispatch`, so
//! the request is deserialized, constraint-checked, handled and serialized exactly as a
//! client's would be.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * every operation answers from the single shared store — what the store holds is what
//!   Get/List report, and what a deregistration removed is gone for every later caller;
//! * `MaxResults`/`NextToken` page the list exactly once over every element, in
//!   registration order, with no token on the last page;
//! * `SubscriptionProviderSources` selects on the stored source;
//! * `UpdateServiceSettings` persists the settings singleton that `GetServiceSettings`
//!   reports, reporting the SETTLED `Status` for the change it applied, and `AllowUpdate`
//!   is what admits a second home Region.
//!
//! The registrations these tests read are seeded into the store directly
//! ([`harness::seed_provider`]) because their rules are about the read, list, page and
//! deregister side of registrations in an ACCEPTED (`ACTIVE`) state, which
//! `RegisterSubscriptionProvider` cannot produce in this emulator: validating a provider
//! access token needs trust material it does not hold, so a registration it creates
//! settles `INVALID`. The create path itself — and the statuses it does report — is
//! covered by `tests/all/subscription_provider_registration.rs`.

use serde_json::{json, Value};

use crate::harness::{
    call_in_region, deregister, get_provider, get_settings, harness, list_providers, listed,
    seed_provider, update_settings, ACTIVE, RED_HAT, REGION,
};

/// Secret ARNs spanning the modelled `SecretArn` grammar
/// `^arn:[a-z0-9-\.]{1,63}:secretsmanager:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:secret:[^/]{1,1023}$`:
/// a plain one, another partition, empty region and account (the `{0,63}` segments), and
/// a secret name using characters the `[^/]` class allows.
const SECRET_ARNS: &[&str] = &[
    "arn:aws:secretsmanager:us-east-1:123456789012:secret:rhel-token",
    "arn:aws-us-gov:secretsmanager:us-gov-west-1:123456789012:secret:rhsm-offline",
    "arn:aws:secretsmanager:::secret:token",
    "arn:aws:secretsmanager:eu-west-1:000000000000:secret:RedHat_Offline+Token=1",
];

/// The values `LinuxSubscriptionsDiscovery` and `OrganizationIntegration` accept
/// (`shared_types`: `Disabled | Enabled`).
const ON_OFF: &[&str] = &["Disabled", "Enabled"];

/// The `Status` a settled change to the service settings reports (`Status` enum:
/// `InProgress | Completed | Successful | Failed`).
const SETTLED: &str = "Completed";

/// Seed one provider per row of [`SECRET_ARNS`] and answer their ARNs in order.
fn seeded(h: &crate::harness::Harness) -> Vec<String> {
    SECRET_ARNS
        .iter()
        .enumerate()
        .map(|(index, secret_arn)| {
            seed_provider(
                h,
                &format!("00000000-0000-4000-8000-{:012x}", index + 1),
                secret_arn,
                &[("owner", "platform")],
            )
        })
        .collect()
}

#[tokio::test]
async fn every_stored_registration_is_read_back_by_get_and_list() {
    let h = harness();
    let arns = seeded(&h);

    for (arn, secret_arn) in arns.iter().zip(SECRET_ARNS) {
        let case = format!("get {arn}");
        let got = get_provider(&h, arn).await;
        let body = got.ok(&case);
        for (member, want) in [
            ("SubscriptionProviderArn", arn.as_str()),
            ("SubscriptionProviderSource", RED_HAT),
            ("SecretArn", secret_arn),
            ("SubscriptionProviderStatus", ACTIVE),
        ] {
            assert_eq!(
                body.get(member).and_then(Value::as_str),
                Some(want),
                "case {case:?}: expected {member} {want:?}, got {body:?}"
            );
        }
        // Nothing was retrieved from the third-party provider and nothing is wrong with
        // the token, so neither member is invented.
        for absent in [
            "SubscriptionProviderStatusMessage",
            "LastSuccessfulDataRetrievalTime",
        ] {
            assert!(
                body.get(absent).is_none_or(Value::is_null),
                "case {case:?}: expected no {absent}, got {body:?}"
            );
        }
    }

    // The list reports every registration, only those, in registration order.
    let case = "list every stored registration";
    let rows = listed(&list_providers(&h, &json!({})).await, case);
    assert_eq!(
        rows.iter()
            .map(|(arn, _, secret, _)| (arn.clone(), secret.clone()))
            .collect::<Vec<_>>(),
        arns.iter()
            .zip(SECRET_ARNS)
            .map(|(arn, secret)| (arn.clone(), (*secret).to_string()))
            .collect::<Vec<_>>(),
        "case {case:?}: the list must report exactly the stored registrations, in order"
    );
    for (arn, source, _, status) in &rows {
        assert_eq!(
            (source.as_str(), status.as_str()),
            (RED_HAT, ACTIVE),
            "case {case:?}: unexpected source/status for {arn}"
        );
    }
}

#[tokio::test]
async fn the_list_pages_exactly_once_over_every_registration() {
    let h = harness();
    let expected = seeded(&h);

    // Every page size from 1 to one past the element count: the pages concatenate to the
    // full list in order, and only the last page omits the token.
    for page_size in 1..=expected.len() + 1 {
        let case = format!("MaxResults {page_size}");
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let reply = list_providers(&h, &request).await;
            let page = listed(&reply, &case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: page of {} exceeds MaxResults, got {page:?}",
                page.len()
            );
            seen.extend(page.into_iter().map(|(arn, _, _, _)| arn));
            token = reply
                .body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must cover every registration exactly once, in order"
        );
        assert_eq!(
            pages,
            expected.len().div_ceil(page_size),
            "case {case:?}: unexpected page count for {} elements",
            expected.len()
        );
    }

    // A cursor is minted by one list and refused elsewhere: a token that was not minted
    // at all is not a position in any list.
    let case = "forged NextToken";
    list_providers(&h, &json!({ "NextToken": "not-a-cursor" }))
        .await
        .error(case, 400, "ValidationException");
}

#[tokio::test]
async fn the_source_selection_narrows_the_list_to_the_stored_source() {
    let h = harness();
    let arns = seeded(&h);

    // An absent or empty selection selects every source; naming the stored source
    // selects the same set.
    for (case, request) in [
        ("no selection", json!({})),
        ("empty selection", json!({ "SubscriptionProviderSources": [] })),
        (
            "RedHat selected",
            json!({ "SubscriptionProviderSources": [RED_HAT] }),
        ),
    ] {
        let rows = listed(&list_providers(&h, &request).await, case);
        assert_eq!(
            rows.len(),
            arns.len(),
            "case {case:?}: expected every registration, got {rows:?}"
        );
    }
}

#[tokio::test]
async fn deregistration_is_observable_by_every_later_caller() {
    let h = harness();
    let arns = seeded(&h);

    for (index, arn) in arns.iter().enumerate() {
        let case = format!("deregister {arn}");
        let reply = deregister(&h, arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            reply.status, reply.body
        );

        // Gone for every later caller: Get refuses it and the list no longer reports it.
        get_provider(&h, arn)
            .await
            .error(&case, 400, "ResourceNotFoundException");
        let rows = listed(&list_providers(&h, &json!({})).await, &case);
        assert_eq!(
            rows.iter().map(|(arn, _, _, _)| arn.clone()).collect::<Vec<_>>(),
            arns[index + 1..],
            "case {case:?}: the list must report only the registrations still in state"
        );

        // And a second deregistration of the same ARN has nothing to remove.
        deregister(&h, arn)
            .await
            .error(&case, 400, "ResourceNotFoundException");
    }
}

#[tokio::test]
async fn service_settings_round_trip_through_state() {
    let h = harness();

    // Before any update the account reports the default, never a canned "Enabled".
    let case = "settings before any update";
    let body = get_settings(&h).await.ok(case).clone();
    assert_eq!(
        body.get("LinuxSubscriptionsDiscovery").and_then(Value::as_str),
        Some("Disabled"),
        "case {case:?}: expected discovery Disabled by default, got {body:?}"
    );
    for absent in ["LinuxSubscriptionsDiscoverySettings", "HomeRegions"] {
        assert!(
            body.get(absent).is_none_or(Value::is_null),
            "case {case:?}: expected no {absent} before an update, got {body:?}"
        );
    }

    // A table over what the rule generalises across: the discovery switch, the
    // Organizations integration, and source-region lists of growing size (the modelled
    // `SourceRegions` is a unique list of 1..100 entries).
    let region_sets: &[&[&str]] = &[
        &["us-east-1"],
        &["us-west-2", "eu-west-1"],
        &["ap-southeast-1", "us-east-2", "eu-central-1"],
    ];
    for discovery in ON_OFF {
        for integration in ON_OFF {
            for regions in region_sets {
                let case = format!("settings {discovery}/{integration}/{regions:?}");
                let request = json!({
                    "LinuxSubscriptionsDiscovery": discovery,
                    "LinuxSubscriptionsDiscoverySettings": {
                        "SourceRegions": regions,
                        "OrganizationIntegration": integration,
                    },
                });
                let updated = update_settings(&h, &request).await.ok(&case).clone();
                // The update answers with what it persisted, and a later read agrees.
                let fetched = get_settings(&h).await.ok(&case).clone();
                for body in [&updated, &fetched] {
                    assert_eq!(
                        body.get("LinuxSubscriptionsDiscovery").and_then(Value::as_str),
                        Some(*discovery),
                        "case {case:?}: expected discovery {discovery}, got {body:?}"
                    );
                    assert_eq!(
                        body.get("Status").and_then(Value::as_str),
                        Some(SETTLED),
                        "case {case:?}: expected Status {SETTLED}, got {body:?}"
                    );
                    assert_eq!(
                        body.get("HomeRegions").and_then(Value::as_array),
                        Some(&vec![json!(REGION)]),
                        "case {case:?}: expected the signed Region as the home Region, got {body:?}"
                    );
                    let settings = body
                        .get("LinuxSubscriptionsDiscoverySettings")
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: no LinuxSubscriptionsDiscoverySettings in {body:?}")
                        });
                    assert_eq!(
                        settings.get("OrganizationIntegration").and_then(Value::as_str),
                        Some(*integration),
                        "case {case:?}: expected OrganizationIntegration {integration}, got {settings:?}"
                    );
                    assert_eq!(
                        settings.get("SourceRegions").and_then(Value::as_array),
                        Some(&regions.iter().map(|r| json!(r)).collect::<Vec<_>>()),
                        "case {case:?}: expected the persisted SourceRegions, got {settings:?}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_second_home_region_needs_allow_update() {
    let h = harness();
    let settings = json!({
        "LinuxSubscriptionsDiscovery": "Enabled",
        "LinuxSubscriptionsDiscoverySettings": {
            "SourceRegions": ["us-east-1"],
            "OrganizationIntegration": "Disabled",
        },
    });

    let case = "first update, in us-west-2";
    update_settings(&h, &settings).await.ok(case);

    // Updating again from the SAME home Region adds no aggregation Region, so it needs
    // no AllowUpdate.
    let case = "repeat update in the home Region";
    let body = update_settings(&h, &settings).await.ok(case).clone();
    assert_eq!(
        body.get("HomeRegions").and_then(Value::as_array),
        Some(&vec![json!(REGION)]),
        "case {case:?}: expected the home Region unchanged, got {body:?}"
    );

    // From another Region it would aggregate the data in a second home Region, which is
    // what AllowUpdate admits.
    let other = "eu-west-1";
    let case = "update from another Region without AllowUpdate";
    call_in_region(
        &h,
        other,
        "/subscription/UpdateServiceSettings",
        &settings,
    )
    .await
    .error(case, 400, "ValidationException");

    let case = "update from another Region with AllowUpdate";
    let mut allowed = settings.clone();
    allowed["AllowUpdate"] = json!(true);
    let body = call_in_region(&h, other, "/subscription/UpdateServiceSettings", &allowed)
        .await
        .ok(case)
        .clone();
    assert_eq!(
        body.get("HomeRegions").and_then(Value::as_array),
        Some(&vec![json!(REGION), json!(other)]),
        "case {case:?}: expected both home Regions, got {body:?}"
    );
    // And the second home Region is persisted, not just echoed.
    let fetched = get_settings(&h).await.ok(case).clone();
    assert_eq!(
        fetched.get("HomeRegions").and_then(Value::as_array),
        Some(&vec![json!(REGION), json!(other)]),
        "case {case:?}: expected both home Regions from state, got {fetched:?}"
    );
}
