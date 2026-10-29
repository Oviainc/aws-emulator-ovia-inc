// SPDX-License-Identifier: CC0-1.0

//! The Region a request is SERVED in, and what this service records from it.
//!
//! Every call goes through the router's `dispatch` with the credential scope a client's
//! signed request carries, so the Region a case signs for is the Region the handler
//! resolves.
//!
//! The rule under test: the Region a request is served in is the Region its SigV4
//! credential scope names when that names an AWS Region of the shared Region catalog, and
//! the emulator's default Region otherwise — so every Region code this service RECORDS
//! (the account's `HomeRegions`, the Region segment of a minted `SubscriptionProviderArn`)
//! names an AWS Region, exactly the set a Region-valued member such as `SourceRegions` is
//! held to (`tests/all/error_taxonomy.rs` pins that member's side of the same catalog).
//!
//! The rows span what the rule generalises over: which catalog entry a scope names (the
//! whole catalog, across partitions), and the FORM of a scope Region outside it — a
//! near-miss numeral, an invented infix, a truncated code, a trailing separator, the wrong
//! case, a length past the ARN grammar's segment, and a real code carrying surrounding
//! text.

use serde_json::{json, Value};

use crate::harness::{
    call_in_region, harness, provider_arn, Harness, RED_HAT, REGION,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::{
    is_known_region, DEFAULT_REGION,
};

/// The catalog of AWS Region codes a scope Region is held to. SOURCE:
/// `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the wire may carry"),
/// the one catalog this service checks a Region-valued member against
/// (`state::is_known_region`), read here rather than copied so a suite cannot hold a
/// second, drifting list.
const CATALOG: &[&str] = ::emulator_telemetry::policy::KNOWN_REGIONS;

/// Credential-scope Regions that name no AWS Region, each a different FORM of not being
/// one. None of them is in [`CATALOG`], which every case asserts rather than assumes.
fn regions_outside_the_catalog() -> Vec<String> {
    vec![
        // A real prefix with a number no Region carries.
        "us-east-99".to_string(),
        "us-west-3".to_string(),
        // An invented infix between real ones.
        "eu-fake-1".to_string(),
        // A catalog code with its trailing index cut off.
        "us-east".to_string(),
        // A catalog code with a trailing separator.
        "us-east-1.".to_string(),
        // A catalog code in the wrong case.
        "US-WEST-2".to_string(),
        // A single character, and a value past the ARN grammar's 63-byte region segment.
        "x".to_string(),
        "a".repeat(70),
        // A catalog code with an extra segment, and one carrying surrounding text.
        "us-west-2-b".to_string(),
        "the-us-west-2".to_string(),
    ]
}

/// The settings body a case sends: discovery on for one real source Region, so what
/// differs between rows is only the Region the request is SIGNED for.
fn settings_request(allow_update: bool) -> Value {
    let mut body = json!({
        "LinuxSubscriptionsDiscovery": "Enabled",
        "LinuxSubscriptionsDiscoverySettings": {
            "SourceRegions": ["us-east-1"],
            "OrganizationIntegration": "Disabled",
        },
    });
    if allow_update {
        body["AllowUpdate"] = json!(true);
    }
    body
}

async fn update_signed_for(h: &Harness, region: &str, allow_update: bool) -> crate::harness::Reply {
    call_in_region(
        h,
        region,
        "/subscription/UpdateServiceSettings",
        &settings_request(allow_update),
    )
    .await
}

async fn settings_read_in(h: &Harness, region: &str) -> crate::harness::Reply {
    call_in_region(h, region, "/subscription/GetServiceSettings", &json!({})).await
}

/// The `HomeRegions` a 200 settings body reports.
fn home_regions(reply: &crate::harness::Reply, case: &str) -> Vec<String> {
    reply
        .ok(case)
        .get("HomeRegions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no HomeRegions in {:?}", reply.body))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a HomeRegions entry is not a string"))
                .to_string()
        })
        .collect()
}

/// Signing for a catalog Region makes THAT Region the home Region, for every entry of the
/// catalog and in every partition it holds.
#[tokio::test]
async fn a_scope_naming_an_aws_region_records_that_region_as_the_home_region() {
    for region in CATALOG {
        let case = format!("signed for {region}");
        assert!(
            is_known_region(region),
            "case {case:?}: the catalog this suite reads must hold its own entries"
        );
        // A fresh account per row: the home Region a row records is the one its own
        // update set, not one a previous row left behind.
        let h = harness();
        let applied = update_signed_for(&h, region, false).await;
        assert_eq!(
            home_regions(&applied, &case),
            vec![(*region).to_string()],
            "case {case:?}: expected the signed Region as the home Region, got {:?}",
            applied.body
        );
        // Persisted, not echoed: the read reports what the update stored.
        let fetched = settings_read_in(&h, region).await;
        assert_eq!(
            home_regions(&fetched, &case),
            vec![(*region).to_string()],
            "case {case:?}: expected the stored home Region from a later read, got {:?}",
            fetched.body
        );
    }
}

/// A scope Region that names no AWS Region is served in the default Region, so the home
/// Region recorded is a real Region rather than the string the caller signed for.
#[tokio::test]
async fn a_scope_outside_the_region_catalog_records_the_default_home_region() {
    assert!(
        is_known_region(DEFAULT_REGION),
        "the Region a request falls back to must itself name an AWS Region, got \
         {DEFAULT_REGION:?}"
    );

    for region in regions_outside_the_catalog() {
        let case = format!("signed for {region:?}");
        assert!(
            !is_known_region(&region),
            "case {case:?}: this row is only a row while the catalog does not hold it"
        );
        let h = harness();
        let applied = update_signed_for(&h, &region, false).await;
        let recorded = home_regions(&applied, &case);
        assert_eq!(
            recorded,
            vec![DEFAULT_REGION.to_string()],
            "case {case:?}: expected the default Region as the home Region, got {:?}",
            applied.body
        );
        for entry in &recorded {
            assert!(
                is_known_region(entry),
                "case {case:?}: every recorded home Region must name an AWS Region, got \
                 {entry:?}"
            );
        }
        // And the account's settings hold that, so no later read reports the signed
        // string either.
        let fetched = settings_read_in(&h, &region).await;
        assert_eq!(
            home_regions(&fetched, &case),
            vec![DEFAULT_REGION.to_string()],
            "case {case:?}: expected the stored home Region from a later read, got {:?}",
            fetched.body
        );
    }
}

/// The home-Region set an account accumulates holds only AWS Regions, whichever scopes it
/// was written by — and the refusal that guards a SECOND home Region names them.
#[tokio::test]
async fn the_second_home_region_path_only_ever_names_aws_regions() {
    for region in regions_outside_the_catalog() {
        let case = format!("first update signed for {region:?}");
        let h = harness();
        assert_eq!(
            home_regions(&update_signed_for(&h, &region, false).await, &case),
            vec![DEFAULT_REGION.to_string()],
            "case {case:?}: the first update records the Region it is served in"
        );

        // A legitimate update from another real Region would aggregate the data in a
        // SECOND home Region, which is what AllowUpdate admits — and the refusal states
        // the home Regions the account actually has.
        let case = format!("{case}, then {REGION} without AllowUpdate");
        let refused = update_signed_for(&h, REGION, false).await;
        refused.error(&case, 400, "ValidationException");
        assert_eq!(
            refused.message(),
            format!(
                "Linux subscription data is already aggregated in home Region(s) \
                 {DEFAULT_REGION}; set AllowUpdate to true to aggregate data in more than \
                 one home Region"
            ),
            "case {case:?}: the refusal must name the home Regions the account holds, got \
             {:?}",
            refused.message()
        );

        let case = format!("{case}, then {REGION} with AllowUpdate");
        let allowed = update_signed_for(&h, REGION, true).await;
        assert_eq!(
            home_regions(&allowed, &case),
            vec![DEFAULT_REGION.to_string(), REGION.to_string()],
            "case {case:?}: expected both home Regions, got {:?}",
            allowed.body
        );
    }
}

/// The same rule where the served Region is MINTED into a resource identifier: a
/// registration's `SubscriptionProviderArn` carries the Region the request was served in,
/// which is an AWS Region whatever the scope named — and the resource is readable back by
/// that ARN.
#[tokio::test]
async fn a_minted_provider_arn_carries_the_served_region() {
    // Rows: catalog entries across partitions keep their own Region; a scope outside the
    // catalog falls back to the default one.
    let mut rows: Vec<(String, String)> = vec![
        ("il-central-1", "il-central-1"),
        ("cn-northwest-1", "cn-northwest-1"),
        ("us-gov-west-1", "us-gov-west-1"),
        ("eu-south-2", "eu-south-2"),
        ("mx-central-1", "mx-central-1"),
    ]
    .into_iter()
    .map(|(signed, served)| (signed.to_string(), served.to_string()))
    .collect();
    rows.extend(
        regions_outside_the_catalog()
            .into_iter()
            .map(|signed| (signed, DEFAULT_REGION.to_string())),
    );

    for (index, (signed, served)) in rows.iter().enumerate() {
        let case = format!("register signed for {signed:?}");
        // No Secrets Manager beside this service, so the credential lookup establishes
        // nothing and the registration is taken (`provider_credential_validation.rs`,
        // rule 3) — what this row is about is the ARN's Region segment.
        let h = harness();
        let registered = call_in_region(
            &h,
            signed,
            "/subscription/RegisterSubscriptionProvider",
            &json!({
                "SubscriptionProviderSource": RED_HAT,
                "SecretArn": format!(
                    "arn:aws:secretsmanager:{served}:000000000000:secret:rhel-token-{index}"
                ),
            }),
        )
        .await;
        let arn = provider_arn(&registered, &case);
        let region_segment = arn
            .split(':')
            .nth(3)
            .unwrap_or_else(|| panic!("case {case:?}: {arn:?} is not an ARN"));
        assert_eq!(
            region_segment, served,
            "case {case:?}: expected the ARN minted in {served}, got {arn:?}"
        );
        assert!(
            is_known_region(region_segment),
            "case {case:?}: a minted ARN's Region segment must name an AWS Region, got \
             {arn:?}"
        );
        // The resource the call created is the one that ARN addresses.
        let read_back = call_in_region(
            &h,
            signed,
            "/subscription/GetRegisteredSubscriptionProvider",
            &json!({ "SubscriptionProviderArn": arn }),
        )
        .await;
        assert_eq!(
            read_back
                .ok(&case)
                .get("SubscriptionProviderArn")
                .and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: the minted ARN must address the registration, got {:?}",
            read_back.body
        );
    }
}
