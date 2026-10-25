// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateSite`, driven over HTTP through the real router.
//!
//! The rule these suites pin: an update of an existing resource writes ONLY the members
//! the request carries — an absent member leaves the stored value as it was, a supplied
//! one replaces it (an empty string included, which is how the service documents removing
//! information: "To remove information for any of the parameters, specify an empty
//! string", `smithy_query query="doc" shape="UpdateSite"`) — and rewrites nothing else:
//! ids, ARN, `CreatedAt`, tags, the clock-derived lifecycle state and every other record
//! stay as they were. Addressing a resource the store does not hold under the path's
//! parent is the missing-resource answer (404 `ResourceNotFoundException`) and stores
//! nothing.
//!
//! Nothing reaches into `crate::state`: a site is only ever read back through `GetSites`.
//! The harness is the one `global_network_lifecycle` defines.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, expected_scoped_arn, Emulator, PAST_SETTLE};

/// The `Description` every seeded site starts with, so a row that supplies no
/// `Description` has a value to leave alone.
const BASE_DESCRIPTION: &str = "baseline description";

/// The text every `ResourceNotFoundException` of this service carries
/// (`crate::state::RESOURCE_NOT_FOUND_MESSAGE`, pinned by `resource_not_found_message`).
const NOT_FOUND_MESSAGE: &str = "Resource not found.";

/// The ARN resource types a refusal of this suite reports.
const GLOBAL_NETWORK: &str = "global-network";
/// See [`GLOBAL_NETWORK`].
const SITE: &str = "site";

/// The three members of `Location`, each a `ConstrainedString` of len 0..256
/// (`smithy_query shape="Location"`).
const LOCATION_MEMBERS: [&str; 3] = ["Address", "Latitude", "Longitude"];

/// The `Location` every seeded site starts with.
fn base_location() -> Value {
    json!({ "Address": "1 Main St", "Latitude": "47.6062", "Longitude": "-122.3321" })
}

/// The `Tags` every seeded site carries, which no update may rewrite.
fn base_tags() -> Value {
    json!([{ "Key": "env", "Value": "test" }])
}

/// One row of the update table: the request body, and the two mutable members the stored
/// record must hold afterwards (`Value::Null` standing for "the member is absent").
struct UpdateCase {
    case: &'static str,
    body: Value,
    description: Value,
    location: Value,
}

/// The rows of the update table. They differ along WHICH mutable members the request
/// carries (both, one, neither), and along the value's form — an empty string (the
/// documented removal), the modelled maximum length of `ConstrainedString` (256), a
/// multi-line and non-ASCII value, and a `Location` carrying a subset of its members.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "both members supplied",
            body: json!({
                "Description": "updated description",
                "Location": { "Address": "2 Elm St", "Latitude": "40.7128", "Longitude": "-74.0060" },
            }),
            description: json!("updated description"),
            location: json!({ "Address": "2 Elm St", "Latitude": "40.7128", "Longitude": "-74.0060" }),
        },
        UpdateCase {
            case: "Description only, so the stored Location is left alone",
            body: json!({ "Description": "description only" }),
            description: json!("description only"),
            location: base_location(),
        },
        UpdateCase {
            case: "Location only, so the stored Description is left alone",
            body: json!({ "Location": { "Address": "3 Oak Ave", "Latitude": "0", "Longitude": "0" } }),
            description: json!(BASE_DESCRIPTION),
            location: json!({ "Address": "3 Oak Ave", "Latitude": "0", "Longitude": "0" }),
        },
        UpdateCase {
            case: "no mutable member at all, so nothing changes",
            body: json!({}),
            description: json!(BASE_DESCRIPTION),
            location: base_location(),
        },
        UpdateCase {
            case: "an empty Description removes the information it held",
            body: json!({ "Description": "" }),
            description: json!(""),
            location: base_location(),
        },
        UpdateCase {
            case: "empty Location members remove the information they held",
            body: json!({ "Location": { "Address": "", "Latitude": "", "Longitude": "" } }),
            description: json!(BASE_DESCRIPTION),
            location: json!({ "Address": "", "Latitude": "", "Longitude": "" }),
        },
        UpdateCase {
            case: "a Location carrying one member replaces the whole structure",
            body: json!({ "Location": { "Address": "4 Pine Rd" } }),
            description: json!(BASE_DESCRIPTION),
            location: json!({ "Address": "4 Pine Rd" }),
        },
        UpdateCase {
            case: "values at the modelled maximum length of 256 characters",
            body: json!({
                "Description": "d".repeat(256),
                "Location": { "Address": "a".repeat(256), "Latitude": "1".repeat(256), "Longitude": "2".repeat(256) },
            }),
            description: json!("d".repeat(256)),
            location: json!({
                "Address": "a".repeat(256),
                "Latitude": "1".repeat(256),
                "Longitude": "2".repeat(256),
            }),
        },
        UpdateCase {
            case: "multi-line and non-ascii values",
            body: json!({
                "Description": "line one\nline two \u{1f310}",
                "Location": { "Address": "Hauptstraße 1\nBerlin", "Latitude": "52,52", "Longitude": "13,40" },
            }),
            description: json!("line one\nline two \u{1f310}"),
            location: json!({ "Address": "Hauptstraße 1\nBerlin", "Latitude": "52,52", "Longitude": "13,40" }),
        },
    ]
}

/// Create a site seeded with [`BASE_DESCRIPTION`], [`base_location`] and [`base_tags`],
/// and return its minted id.
async fn create_seeded_site(emulator: &Emulator, network: &str) -> String {
    let (status, created) = emulator
        .post(
            &format!("/global-networks/{network}/sites"),
            json!({
                "Description": BASE_DESCRIPTION,
                "Location": base_location(),
                "Tags": base_tags(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateSite: expected 200, got {status} with {created}"
    );
    created["Site"]["SiteId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateSite: no SiteId in {created}"))
        .to_string()
}

/// Read one site back through `GetSites`, the implemented reader for this resource.
async fn read_site(emulator: &Emulator, network: &str, site_id: &str) -> Value {
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/sites?siteIds={site_id}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetSites({site_id}): expected 200, got {status} with {body}"
    );
    let sites = body["Sites"]
        .as_array()
        .unwrap_or_else(|| panic!("GetSites({site_id}): no Sites array in {body}"));
    assert_eq!(
        sites.len(),
        1,
        "GetSites({site_id}): expected exactly that site, got {body}"
    );
    sites[0].clone()
}

/// Assert `actual` holds exactly the `Location` members `expected` names, treating an
/// absent member and a missing structure alike as `Value::Null`.
fn assert_location(case: &str, label: &str, actual: &Value, expected: &Value) {
    for member in LOCATION_MEMBERS {
        let got = actual.get(member).cloned().unwrap_or(Value::Null);
        let want = expected.get(member).cloned().unwrap_or(Value::Null);
        assert_eq!(
            got, want,
            "case {case}: {label} Location.{member}: expected {want}, got {got} (whole value {actual})"
        );
    }
}

#[tokio::test]
async fn update_site_writes_only_the_supplied_members_and_round_trips_through_get_sites() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-site").await;
    let rows = update_cases();

    // One identically seeded site per row, plus a sibling no row names.
    let mut sites = Vec::new();
    for _ in &rows {
        sites.push(create_seeded_site(&emulator, &network).await);
    }
    let sibling = create_seeded_site(&emulator, &network).await;

    // The PENDING -> AVAILABLE transition the create owes is past before the first
    // update, so an update that restarted that clock would be visible here.
    std::thread::sleep(PAST_SETTLE);

    for (row, site_id) in rows.iter().zip(&sites) {
        let before = read_site(&emulator, &network, site_id).await;
        let created_at = before["CreatedAt"].clone();

        let (status, answer) = emulator
            .patch(
                &format!("/global-networks/{network}/sites/{site_id}"),
                row.body.clone(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} with {answer}",
            row.case
        );
        let site = &answer["Site"];

        assert_eq!(
            site["Description"], row.description,
            "case {}: expected Description {}, got {site}",
            row.case, row.description
        );
        assert_location(row.case, "the answer's", &site["Location"], &row.location);

        // Everything the update does not take as input is the record it already was.
        assert_eq!(
            site["SiteId"],
            json!(site_id),
            "case {}: SiteId must be the addressed site, got {site}",
            row.case
        );
        assert_eq!(
            site["SiteArn"],
            json!(expected_scoped_arn(SITE, &network, site_id)),
            "case {}: the ARN must not be rewritten, got {site}",
            row.case
        );
        assert_eq!(
            site["GlobalNetworkId"],
            json!(network.clone()),
            "case {}: the parent back-reference must not be rewritten, got {site}",
            row.case
        );
        assert_eq!(
            site["CreatedAt"], created_at,
            "case {}: CreatedAt must stay the create's instant, got {site}",
            row.case
        );
        assert_eq!(
            site["Tags"],
            base_tags(),
            "case {}: Tags must not be rewritten, got {site}",
            row.case
        );
        assert_eq!(
            site["State"],
            json!("AVAILABLE"),
            "case {}: an update must not restart the create's settle clock, got {site}",
            row.case
        );

        // The answer is the STORED record, not a view of the request.
        let stored = read_site(&emulator, &network, site_id).await;
        assert_eq!(
            stored, *site,
            "case {}: GetSites must answer the record the update returned, got {stored}",
            row.case
        );
    }

    // No update reached a site it did not name.
    let untouched = read_site(&emulator, &network, &sibling).await;
    assert_eq!(
        untouched["Description"],
        json!(BASE_DESCRIPTION),
        "case sibling site: an update of another site must leave it alone, got {untouched}"
    );
    assert_location(
        "sibling site",
        "the stored",
        &untouched["Location"],
        &base_location(),
    );
}

#[tokio::test]
async fn successive_updates_accumulate_on_the_stored_record() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("update-site-sequence").await;

    // Seeded with NEITHER mutable member, so a member nothing has supplied stays absent.
    let (status, created) = emulator
        .post(&format!("/global-networks/{network}/sites"), json!({}))
        .await;
    assert_eq!(
        status, 200,
        "CreateSite: expected 200, got {status} with {created}"
    );
    let site_id = created["Site"]["SiteId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateSite: no SiteId in {created}"))
        .to_string();

    // Each step applies to the record the step before it left behind.
    let steps = vec![
        UpdateCase {
            case: "step 1: an empty update of an empty record",
            body: json!({}),
            description: Value::Null,
            location: Value::Null,
        },
        UpdateCase {
            case: "step 2: a Description where there was none",
            body: json!({ "Description": "first" }),
            description: json!("first"),
            location: Value::Null,
        },
        UpdateCase {
            case: "step 3: a Location where there was none, keeping the Description",
            body: json!({ "Location": { "Address": "1 Main St" } }),
            description: json!("first"),
            location: json!({ "Address": "1 Main St" }),
        },
        UpdateCase {
            case: "step 4: emptying the Description keeps the Location",
            body: json!({ "Description": "" }),
            description: json!(""),
            location: json!({ "Address": "1 Main St" }),
        },
        UpdateCase {
            case: "step 5: a new Location replaces the old one member for member",
            body: json!({ "Location": { "Latitude": "0", "Longitude": "0" } }),
            description: json!(""),
            location: json!({ "Latitude": "0", "Longitude": "0" }),
        },
        UpdateCase {
            case: "step 6: both members at once",
            body: json!({
                "Description": "second",
                "Location": { "Address": "5 Birch Way", "Latitude": "1", "Longitude": "2" },
            }),
            description: json!("second"),
            location: json!({ "Address": "5 Birch Way", "Latitude": "1", "Longitude": "2" }),
        },
    ];

    for step in &steps {
        let (status, answer) = emulator
            .patch(
                &format!("/global-networks/{network}/sites/{site_id}"),
                step.body.clone(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} with {answer}",
            step.case
        );
        let stored = read_site(&emulator, &network, &site_id).await;
        assert_eq!(
            stored["Description"], step.description,
            "case {}: expected the stored Description {}, got {stored}",
            step.case, step.description
        );
        assert_location(step.case, "the stored", &stored["Location"], &step.location);
    }
}

/// One refusal row: the path the request takes, and the resource the 404 must identify.
struct NotFoundCase {
    case: &'static str,
    network: String,
    site: String,
    resource_id: String,
    resource_type: &'static str,
}

#[tokio::test]
async fn update_site_refuses_a_resource_the_store_does_not_hold_and_stores_nothing() {
    let emulator = Emulator::new();
    let owner = emulator.create_global_network("owner").await;
    let other = emulator.create_global_network("other").await;
    let site = create_seeded_site(&emulator, &owner).await;

    let gone = create_seeded_site(&emulator, &owner).await;
    let (status, deleted) = emulator
        .delete(&format!("/global-networks/{owner}/sites/{gone}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteSite: expected 200, got {status} with {deleted}"
    );

    // Well-formed ids the store never minted.
    let unknown_network = "global-network-00000000000000099".to_string();
    let unknown_site = "site-00000000000000099".to_string();

    let rows = vec![
        NotFoundCase {
            case: "a global network that never existed",
            network: unknown_network.clone(),
            site: site.clone(),
            resource_id: unknown_network.clone(),
            resource_type: GLOBAL_NETWORK,
        },
        NotFoundCase {
            case: "a live site addressed through another global network",
            network: other.clone(),
            site: site.clone(),
            resource_id: site.clone(),
            resource_type: SITE,
        },
        NotFoundCase {
            case: "a site that never existed",
            network: owner.clone(),
            site: unknown_site.clone(),
            resource_id: unknown_site.clone(),
            resource_type: SITE,
        },
        NotFoundCase {
            case: "a deleted site",
            network: owner.clone(),
            site: gone.clone(),
            resource_id: gone.clone(),
            resource_type: SITE,
        },
    ];

    for row in &rows {
        let (status, answer) = emulator
            .patch(
                &format!("/global-networks/{}/sites/{}", row.network, row.site),
                json!({ "Description": "must not be stored" }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {}: expected 404, got {status} with {answer}",
            row.case
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {}: wrong error code in {answer}",
            row.case
        );
        assert_eq!(
            answer["Message"].as_str(),
            Some(NOT_FOUND_MESSAGE),
            "case {}: expected the constant message {NOT_FOUND_MESSAGE:?}, got {answer}",
            row.case
        );
        assert_eq!(
            answer["ResourceId"].as_str(),
            Some(row.resource_id.as_str()),
            "case {}: expected ResourceId {:?}, got {answer}",
            row.case,
            row.resource_id
        );
        assert_eq!(
            answer["ResourceType"].as_str(),
            Some(row.resource_type),
            "case {}: expected ResourceType {:?}, got {answer}",
            row.case,
            row.resource_type
        );
    }

    // Every refusal above stored nothing: the live site still holds what it was seeded
    // with, read back through its own global network.
    let stored = read_site(&emulator, &owner, &site).await;
    assert_eq!(
        stored["Description"],
        json!(BASE_DESCRIPTION),
        "case refused updates: the site must keep its stored Description, got {stored}"
    );
    assert_location(
        "refused updates",
        "the stored",
        &stored["Location"],
        &base_location(),
    );
}
