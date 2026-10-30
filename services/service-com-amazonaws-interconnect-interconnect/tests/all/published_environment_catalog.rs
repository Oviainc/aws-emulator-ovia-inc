// SPDX-License-Identifier: CC0-1.0

//! The Environment catalog is the SERVICE's own, not one the emulator invents.
//!
//! The model binds no `CreateEnvironment`, so an Environment is AWS-managed and the
//! catalog a regional endpoint publishes is the same for every caller in that Region
//! (`smithy_query(query="operation",operation="ListEnvironments")`: `@readonly`, output
//! `environments` `@required`). What a client discovers there is what it then passes to
//! `CreateConnection` — the `environmentId`, the provider-dictated product type and one
//! of the bandwidths — so every value in it has to be one the service really publishes:
//! a partner token spelled as the partner spells it, a remote-side `location` in the
//! PARTNER's own naming rather than an AWS Region id, and the bandwidth sets the
//! offering actually offers.
//!
//! Every assertion below is checked over a TABLE of Regions, and inside each Region over
//! every Environment that Region's endpoint publishes and every bandwidth it offers.
//! Nothing here is keyed to one Region or one offering: each row is read from the live
//! listing.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::emulator::{
    available_bandwidths, code, remote_account_for, text, Emulator, DEFAULT_SIGNING_REGION,
};

/// The Regions the rows address: the one the rest of this crate's suites sign for plus
/// four more on four other continents, so no row's Region is the harness default by
/// accident.
const REGIONS: [&str; 5] = [
    DEFAULT_SIGNING_REGION,
    "us-west-2",
    "eu-west-1",
    "ap-southeast-2",
    "sa-east-1",
];

/// The `@length` the model gives `EnvironmentId`
/// (`smithy_query(query="shape",shape="EnvironmentId")`).
const ENVIRONMENT_ID_LENGTH: (usize, usize) = (1, 64);

/// The `@length` the model gives `ProductType`.
const PRODUCT_TYPE_LENGTH: (usize, usize) = (5, 32);

/// The `@length` the model gives `Location`, which `Environment$location` targets.
const LOCATION_MAX_LENGTH: usize = 255;

/// The `@length` the model gives both targets of `Provider`'s members
/// (`CloudServiceProvider` and `LastMileProvider`).
const PROVIDER_NAME_MAX_LENGTH: usize = 32;

/// The `@length` the model gives `ConnectionBandwidth`, which every entry of both
/// bandwidth lists is a value of.
const BANDWIDTH_LENGTH: (usize, usize) = (1, 8);

/// The values `EnvironmentState` models
/// (`smithy_query(query="shape",shape="EnvironmentState")` -> values: ["available",
/// "limited", "unavailable"]).
const ENVIRONMENT_STATES: [&str; 3] = ["available", "limited", "unavailable"];

/// The values `RemoteAccountIdentifierType` models.
const REMOTE_IDENTIFIER_TYPES: [&str; 2] = ["account", "email"];

/// Whether `bandwidth` is a value `ConnectionBandwidth`'s own `^\d+[MG]bps$` pattern
/// admits: digits, then the unit, then `bps`.
fn is_bandwidth(bandwidth: &str) -> bool {
    let Some(digits) = bandwidth
        .strip_suffix("Mbps")
        .or_else(|| bandwidth.strip_suffix("Gbps"))
    else {
        return false;
    };
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `value` is spelled the way AWS spells a Region id (`us-east-1`,
/// `ap-southeast-2`): the shape an Environment's remote-side `location` must NOT have,
/// because that member is "the provider specific location on the remote side of this
/// Connection" — the partner's own geography, in the partner's own naming.
fn looks_like_an_aws_region(value: &str) -> bool {
    let mut parts = value.rsplitn(2, '-');
    let last = parts.next().unwrap_or_default();
    let head = parts.next().unwrap_or_default();
    !last.is_empty()
        && last.bytes().all(|b| b.is_ascii_digit())
        && head.contains('-')
        && head
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'-')
}

/// `Environment$bandwidths.supported` for one listed Environment.
fn supported_bandwidths(environment: &Value) -> Vec<String> {
    environment["bandwidths"]["supported"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|bandwidth| bandwidth.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The partner token an Environment's `Provider` carries, with the member it arrived in.
fn partner_of(environment: &Value) -> (&'static str, String) {
    if let Some(name) = environment["provider"]["cloudServiceProvider"].as_str() {
        ("cloudServiceProvider", name.to_string())
    } else {
        (
            "lastMileProvider",
            environment["provider"]["lastMileProvider"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    }
}

/// RULE: every Environment a regional endpoint publishes is a record the service really
/// publishes — every member the model requires, inside its modelled constraints, with
/// the partner's own token, a remote-side location in the partner's naming, bandwidth
/// sets whose `available` is a subset of `supported`, and no member the catalog does not
/// publish — and `GetEnvironment` reports the listed record byte for byte.
#[tokio::test]
async fn every_published_environment_is_a_record_the_service_publishes() {
    let emulator = Emulator::new();

    let mut rows = 0;
    for region in REGIONS {
        let catalog = emulator.environments_in(region).await;
        assert!(
            !catalog.is_empty(),
            "case {region:?}: expected the endpoint to publish a catalog, got {catalog:?}"
        );

        for environment in &catalog {
            let id = text(environment, "environmentId");
            let case = format!("{region} / {id}");
            rows += 1;

            let (min, max) = ENVIRONMENT_ID_LENGTH;
            let length = id.chars().count();
            assert!(
                (min..=max).contains(&length),
                "case {case:?}: expected an EnvironmentId of length {min}..{max}, got {length}"
            );
            // The id is the one the service publishes for a partner offering, not a
            // caller-facing name: lower-case, and naming the partner it is an offering
            // of.
            let (member, partner) = partner_of(environment);
            assert!(
                id.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "case {case:?}: expected a published environmentId of lower-case letters, digits and hyphens, got {id:?}"
            );
            assert!(
                id.contains(&partner),
                "case {case:?}: expected the id to name the partner {partner:?} it is an offering of, got {id:?}"
            );

            // Exactly one `Provider` member, carrying the partner's own token.
            assert!(
                environment["provider"]["cloudServiceProvider"].is_string()
                    ^ environment["provider"]["lastMileProvider"].is_string(),
                "case {case:?}: expected exactly one Provider member, got {:?}",
                environment["provider"]
            );
            assert!(
                !partner.is_empty() && partner.chars().count() <= PROVIDER_NAME_MAX_LENGTH,
                "case {case:?}: expected a {member} of 1..{PROVIDER_NAME_MAX_LENGTH} characters, got {partner:?}"
            );
            assert_eq!(
                partner,
                partner.to_ascii_lowercase(),
                "case {case:?}: expected the partner's own lower-case token, got {partner:?}"
            );

            // The remote-side location is the partner's own naming, not an AWS Region.
            let location = text(environment, "location");
            assert!(
                !location.is_empty() && location.chars().count() <= LOCATION_MAX_LENGTH,
                "case {case:?}: expected a location of 1..{LOCATION_MAX_LENGTH} characters, got {location:?}"
            );
            assert!(
                !looks_like_an_aws_region(&location),
                "case {case:?}: expected the remote side's own location naming, got the AWS Region id {location:?}"
            );

            let state = text(environment, "state");
            assert!(
                ENVIRONMENT_STATES.contains(&state.as_str()),
                "case {case:?}: expected an EnvironmentState value, got {state:?}"
            );

            let product_type = text(environment, "type");
            let (min, max) = PRODUCT_TYPE_LENGTH;
            assert!(
                (min..=max).contains(&product_type.chars().count()),
                "case {case:?}: expected a ProductType of length {min}..{max}, got {product_type:?}"
            );

            // Both bandwidth lists hold `ConnectionBandwidth` values, without repeats,
            // and `available` ("currently available") is a subset of `supported`.
            let available = available_bandwidths(environment);
            let supported = supported_bandwidths(environment);
            for bandwidth in available.iter().chain(supported.iter()) {
                let (min, max) = BANDWIDTH_LENGTH;
                assert!(
                    is_bandwidth(bandwidth)
                        && (min..=max).contains(&bandwidth.chars().count()),
                    "case {case:?}: expected a ConnectionBandwidth, got {bandwidth:?}"
                );
            }
            for list in [&available, &supported] {
                let unique: BTreeSet<&String> = list.iter().collect();
                assert_eq!(
                    unique.len(),
                    list.len(),
                    "case {case:?}: expected each bandwidth once, got {list:?}"
                );
            }
            for bandwidth in &available {
                assert!(
                    supported.contains(bandwidth),
                    "case {case:?}: expected the available bandwidth {bandwidth:?} to be supported too, got {supported:?}"
                );
            }
            // `limited` is "available, but overall capacity is limited", which is what an
            // offering holding back part of what it supports is in.
            if state == "limited" {
                assert!(
                    available.len() < supported.len(),
                    "case {case:?}: expected a limited environment to offer fewer bandwidths than it supports, got {available:?} of {supported:?}"
                );
            }

            // The catalog publishes no `activationPageUrl` for this offering, so the
            // record carries no such member at all rather than an invented URL.
            assert!(
                environment.get("activationPageUrl").is_none(),
                "case {case:?}: expected no activationPageUrl on a record the catalog publishes without one, got {:?}",
                environment["activationPageUrl"]
            );
            if let Some(kind) = environment["remoteIdentifierType"].as_str() {
                assert!(
                    REMOTE_IDENTIFIER_TYPES.contains(&kind),
                    "case {case:?}: expected a RemoteAccountIdentifierType value, got {kind:?}"
                );
            }

            // The per-id read answers the listed record, and reading changes nothing.
            let fetched = emulator.environment_in(region, &id).await;
            assert_eq!(
                &fetched, environment,
                "case {case:?}: expected GetEnvironment to report the listed record, got {fetched:?}"
            );
            let again = emulator.environments_in(region).await;
            assert_eq!(
                &again, &catalog,
                "case {case:?}: expected a repeated listing to report the same catalog, got {again:?}"
            );
        }
    }
    assert!(
        rows >= REGIONS.len(),
        "case \"coverage\": expected at least one published Environment per Region, got {rows} rows"
    );
}

/// RULE: every bandwidth an Environment publishes as `available` is one a connection can
/// really be placed at, and one it only `supported`s is not — so the sets a client
/// discovers are the sets it can act on.
#[tokio::test]
async fn every_published_available_bandwidth_takes_a_connection() {
    let emulator = Emulator::new();
    let attach_point = emulator.attach_point().await;

    // Rows: every bandwidth the default Region's Environment offers, plus the first one
    // of every other Region's, so the rows differ in the bandwidth AND in the Region the
    // placement is made through.
    for region in REGIONS {
        let catalog = emulator.environments_in(region).await;
        for environment in &catalog {
            let environment_id = text(environment, "environmentId");
            let available = available_bandwidths(environment);
            let supported = supported_bandwidths(environment);
            let sizes: Vec<String> = if region == DEFAULT_SIGNING_REGION {
                available.clone()
            } else {
                available.iter().take(1).cloned().collect()
            };

            for bandwidth in &sizes {
                let case = format!("{region} / {environment_id} / {bandwidth}");
                let mut request = Map::new();
                request.insert("environmentId".to_string(), json!(environment_id));
                request.insert("bandwidth".to_string(), json!(bandwidth));
                request.insert(
                    "attachPoint".to_string(),
                    json!({ "directConnectGateway": attach_point }),
                );
                if let Some(remote_account) = remote_account_for(environment) {
                    request.insert("remoteAccount".to_string(), remote_account);
                }
                let created = emulator
                    .ic_ok_in(region, "CreateConnection", Value::Object(request))
                    .await;
                let connection = created["connection"].clone();
                assert_eq!(
                    text(&connection, "bandwidth"),
                    *bandwidth,
                    "case {case:?}: expected the connection to hold the published bandwidth, got {:?}",
                    connection["bandwidth"]
                );
                // Read back through the implemented reader: the placement is stored, not
                // just echoed.
                let id = text(&connection, "id");
                let fetched = emulator
                    .ic_ok_in(region, "GetConnection", json!({ "identifier": id }))
                    .await;
                assert_eq!(
                    fetched["connection"]["bandwidth"],
                    json!(bandwidth),
                    "case {case:?}: expected GetConnection to report the stored bandwidth, got {:?}",
                    fetched["connection"]["bandwidth"]
                );
                assert_eq!(
                    fetched["connection"]["environmentId"],
                    json!(environment_id),
                    "case {case:?}: expected the stored connection to name the Environment it was placed on, got {:?}",
                    fetched["connection"]["environmentId"]
                );
            }

            // A size the Environment merely supports is refused: `available` is "the
            // list of currently available bandwidths", so nothing outside it can be
            // allocated.
            for bandwidth in supported.iter().filter(|size| !available.contains(size)) {
                let case = format!("{region} / {environment_id} / supported-only {bandwidth}");
                let mut request = Map::new();
                request.insert("environmentId".to_string(), json!(environment_id));
                request.insert("bandwidth".to_string(), json!(bandwidth));
                request.insert(
                    "attachPoint".to_string(),
                    json!({ "directConnectGateway": attach_point }),
                );
                if let Some(remote_account) = remote_account_for(environment) {
                    request.insert("remoteAccount".to_string(), remote_account);
                }
                let (status, body) = emulator
                    .ic_in(region, "CreateConnection", Value::Object(request))
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a bandwidth outside the available list to be refused, got {status} with {body:?}"
                );
                assert!(
                    code(&body).contains("InterconnectValidationException"),
                    "case {case:?}: expected InterconnectValidationException, got {:?}",
                    code(&body)
                );
            }
        }
    }
}

/// RULE: the `provider` filter narrows the catalog to the offerings of that partner on
/// that side of the union, and the `location` filter narrows nothing — the service
/// answers a listing naming a location distinguisher with the Environments it publishes
/// whatever that distinguisher is.
#[tokio::test]
async fn the_provider_filter_narrows_the_catalog_and_the_location_filter_does_not() {
    let emulator = Emulator::new();

    for region in REGIONS {
        let catalog = emulator.environments_in(region).await;

        // Rows differ in the filter member and in the value: each partner the catalog
        // names, a partner it does not name, the same token on the other side of the
        // union, and — for `location` — the location an Environment publishes, a
        // location no Environment publishes, an AWS Region id and the empty string.
        let mut provider_rows: Vec<(String, Value)> = Vec::new();
        for environment in &catalog {
            provider_rows.push((
                format!("provider {:?}", environment["provider"]),
                environment["provider"].clone(),
            ));
        }
        provider_rows.push((
            "a partner the catalog does not name".to_string(),
            json!({ "cloudServiceProvider": "no-such-partner" }),
        ));
        for environment in &catalog {
            let (member, partner) = partner_of(environment);
            let other = if member == "cloudServiceProvider" {
                "lastMileProvider"
            } else {
                "cloudServiceProvider"
            };
            provider_rows.push((
                format!("{partner} on the {other} side"),
                json!({ other: partner }),
            ));
        }

        for (label, provider) in provider_rows {
            let expected: Vec<Value> = catalog
                .iter()
                .filter(|environment| environment["provider"] == provider)
                .cloned()
                .collect();
            let listed = emulator
                .ic_ok_in(region, "ListEnvironments", json!({ "provider": provider }))
                .await;
            let actual = listed["environments"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {region:?}/{label}: expected the {} Environments of that partner, got {actual:?}",
                expected.len()
            );
        }

        let mut location_rows: Vec<String> = catalog
            .iter()
            .map(|environment| text(environment, "location"))
            .collect();
        location_rows.push("a-location-no-environment-publishes".to_string());
        location_rows.push(region.to_string());
        location_rows.push(String::new());
        for location in location_rows {
            let listed = emulator
                .ic_ok_in(region, "ListEnvironments", json!({ "location": location }))
                .await;
            let actual = listed["environments"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                actual, catalog,
                "case {region:?}/location {location:?}: expected the Region's whole catalog, got {actual:?}"
            );
        }
    }
}
