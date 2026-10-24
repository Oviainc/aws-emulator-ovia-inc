// SPDX-License-Identifier: CC0-1.0

//! The Region restriction on Lightsail's domain (DNS zone) API, driven over the
//! wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the sibling suite's (`instance_lifecycle`), so this
//! file stands up no second emulator of its own; its `Authorization` header
//! carries only the credential SCOPE, which is what names the Region a request
//! is made in.
//!
//! The rule under test: Lightsail's domain API is served by the N. Virginia
//! (`us-east-1`) endpoint ALONE, so EVERY domain operation — the creates, the
//! read and the edits alike — refuses a request whose credential scope names any
//! other Region with the modelled `InvalidInputException` (HTTP 400) carrying the
//! restriction the model documents, and changes nothing in the store; a request
//! made in `us-east-1`, or one carrying no credential scope at all (served in the
//! service's default Region, which IS `us-east-1`), is served.

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::REGION_NAME_VALUES;

use super::instance_lifecycle::{error_code, Emulator};

/// The only Region Lightsail serves the domain API from, from the modelled
/// `InvalidInputException` documentation
/// (`smithy_query(query="doc",shape="InvalidInputException")`).
const DOMAIN_REGION: &str = "us-east-1";

/// The refusal the documentation states verbatim, so its text is a requirement
/// and not an example: `smithy_query(query="doc",shape="InvalidInputException")`
/// reads "Domain and distribution APIs are only available in the N. Virginia
/// (us-east-1) Amazon Web Services Region. Please set your Amazon Web Services
/// Region configuration to us-east-1 to create, view, or edit these resources."
const REFUSAL: &str = "Domain and distribution APIs are only available in the N. Virginia \
                       (us-east-1) Amazon Web Services Region. Please set your Amazon Web \
                       Services Region configuration to us-east-1 to create, view, or edit \
                       these resources.";

/// An Amazon Web Services Region the modelled `RegionName` enum does NOT list,
/// so a case can sit outside the catalog: a scope naming a Region Lightsail does
/// not serve at all is no more the domain Region than any other, so it is refused
/// too.
const OFF_ENUM_REGION: &str = "il-central-1";

/// Every Region a domain call can be made in that is NOT the domain Region.
///
/// Taken from the crate's own `REGION_NAME_VALUES` — the modelled `RegionName`
/// enum's values (`smithy_query(query="shape",shape="RegionName")`) — minus the
/// domain Region, so the table cannot drift from the model as the catalog grows,
/// plus [`OFF_ENUM_REGION`].
fn other_regions() -> Vec<&'static str> {
    assert!(
        REGION_NAME_VALUES.contains(&DOMAIN_REGION),
        "case the Region catalog: expected the modelled RegionName enum to list the domain \
         Region, got {REGION_NAME_VALUES:?}"
    );
    assert!(
        !REGION_NAME_VALUES.contains(&OFF_ENUM_REGION),
        "case the Region catalog: expected {OFF_ENUM_REGION} to sit OUTSIDE the modelled \
         RegionName enum, got {REGION_NAME_VALUES:?}"
    );
    let mut regions: Vec<&'static str> = REGION_NAME_VALUES
        .iter()
        .copied()
        .filter(|region| *region != DOMAIN_REGION)
        .collect();
    regions.push(OFF_ENUM_REGION);
    regions
}

/// The zone, the stored entry and the never-created name every row's request is
/// built from.
struct Fixture {
    /// A zone this account holds, created in the domain Region.
    zone: String,
    /// That zone's one stored entry, as `GetDomain` reports it.
    entry: Value,
    /// A zone name nothing ever created, so a refused `CreateDomain` can be
    /// shown to have created nothing.
    fresh_zone: String,
}

impl Fixture {
    /// The stored entry's server-assigned id.
    fn entry_id(&self) -> &str {
        self.entry["id"]
            .as_str()
            .expect("the stored entry carries a server-assigned id")
    }

    /// The stored entry's name.
    fn entry_name(&self) -> &str {
        self.entry["name"]
            .as_str()
            .expect("the stored entry carries a name")
    }
}

/// One domain operation, with the request body it is exercised with.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: fn(&Fixture) -> Value,
}

/// Every implemented domain operation. `GetDomains` and `DeleteDomain` are not
/// here because they are still scaffold stubs answering 501 — their own issues
/// own them — and the distribution operations the same documentation covers are
/// stubs as well.
const ROWS: &[Row] = &[
    Row {
        case: "creating a zone",
        operation: "CreateDomain",
        body: |fixture| json!({"domainName": fixture.fresh_zone}),
    },
    Row {
        case: "reading a zone",
        operation: "GetDomain",
        body: |fixture| json!({"domainName": fixture.zone}),
    },
    Row {
        case: "adding a recordset entry",
        operation: "CreateDomainEntry",
        body: |fixture| {
            json!({
                "domainName": fixture.zone,
                "domainEntry": {
                    "name": format!("added.{}", fixture.zone),
                    "type": "A",
                    "target": "192.0.2.21",
                },
            })
        },
    },
    Row {
        case: "updating a recordset entry",
        operation: "UpdateDomainEntry",
        body: |fixture| {
            json!({
                "domainName": fixture.zone,
                "domainEntry": {
                    "id": fixture.entry_id(),
                    "name": fixture.entry_name(),
                    "type": "A",
                    "target": "192.0.2.22",
                },
            })
        },
    },
    Row {
        case: "deleting a recordset entry",
        operation: "DeleteDomainEntry",
        body: |fixture| {
            json!({
                "domainName": fixture.zone,
                "domainEntry": {
                    "id": fixture.entry_id(),
                    "name": fixture.entry_name(),
                    "type": "A",
                    "target": "192.0.2.10",
                },
            })
        },
    },
];

/// Stand up an emulator holding one zone with one entry, created in the domain
/// Region, and answer it beside the fixture its rows are built from.
async fn emulator_with_a_zone() -> (Emulator, Fixture) {
    let emulator = Emulator::new();
    let zone = "example.com".to_string();
    let (status, body) = emulator
        .call_in(Some(DOMAIN_REGION), "CreateDomain", json!({"domainName": zone}))
        .await;
    assert_eq!(
        status, 200,
        "case the fixture zone: CreateDomain in {DOMAIN_REGION} must succeed, got {status} \
         {body:?}"
    );
    let (status, body) = emulator
        .call_in(
            Some(DOMAIN_REGION),
            "CreateDomainEntry",
            json!({
                "domainName": zone,
                "domainEntry": {"name": format!("www.{zone}"), "type": "A", "target": "192.0.2.10"},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the fixture entry: CreateDomainEntry in {DOMAIN_REGION} must succeed, got {status} \
         {body:?}"
    );
    let entry = read_zone(&emulator, &zone).await["domainEntries"][0].clone();
    assert!(
        entry["id"].is_string(),
        "case the fixture entry: expected a stored entry carrying an id, got {entry:?}"
    );
    (
        emulator,
        Fixture {
            zone,
            entry,
            fresh_zone: "never-created.example".to_string(),
        },
    )
}

/// The stored zone `name`, read through the domain Region's endpoint.
async fn read_zone(emulator: &Emulator, name: &str) -> Value {
    read_zone_in(emulator, Some(DOMAIN_REGION), name).await
}

/// RULE: every domain operation refuses a request whose credential scope names a
/// Region other than the domain Region, with the modelled
/// `InvalidInputException` (HTTP 400) and the documented restriction, and leaves
/// the store exactly as it was.
#[tokio::test]
async fn every_domain_api_is_refused_outside_the_domain_region() {
    let (emulator, fixture) = emulator_with_a_zone().await;
    let before = read_zone(&emulator, &fixture.zone).await;

    for region in other_regions() {
        for row in ROWS {
            let case = format!("{} in {region}", row.case);
            let (status, body) = emulator
                .call_in(Some(region), row.operation, (row.body)(&fixture))
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected a 400 refusal, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case}: expected the modelled InvalidInputException, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(REFUSAL),
                "case {case}: expected the documented restriction, got {:?}",
                body["message"]
            );

            // The refusal changed nothing: the zone and its one entry are
            // exactly as they were, and the name the refused create named is
            // still not a zone of this account.
            let after = read_zone(&emulator, &fixture.zone).await;
            assert_eq!(
                after, before,
                "case {case}: expected the stored zone to be untouched, got {after:?}"
            );
            let (status, body) = emulator
                .call_in(
                    Some(DOMAIN_REGION),
                    "GetDomain",
                    json!({"domainName": fixture.fresh_zone}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: a refused create stores nothing, so reading {} must be refused, \
                 got {status} {body:?}",
                fixture.fresh_zone
            );
            assert_eq!(
                body["code"].as_str(),
                Some("DoesNotExist"),
                "case {case}: a refused create stores nothing, so reading {} must answer code \
                 DoesNotExist, got {body:?}",
                fixture.fresh_zone
            );
        }
    }
}

/// RULE: the same operations ARE served through the domain Region's endpoint,
/// and through a request that carries no credential scope at all — which the
/// service serves in its default Region, the domain Region — so the restriction
/// refuses only the other Regions and the zone round-trips.
#[tokio::test]
async fn the_domain_apis_are_served_in_the_domain_region() {
    // The two ways a request names the domain Region: the scope says so, or
    // there is no scope to say otherwise.
    for (case, region) in [
        ("the us-east-1 credential scope", Some(DOMAIN_REGION)),
        ("no credential scope at all", None),
    ] {
        let emulator = Emulator::new();
        let zone = "served.example";
        let entry_name = format!("api.{zone}");

        let (status, body) = emulator
            .call_in(region, "CreateDomain", json!({"domainName": zone}))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: CreateDomain must be served, got {status} {body:?}"
        );

        let (status, body) = emulator
            .call_in(
                region,
                "CreateDomainEntry",
                json!({
                    "domainName": zone,
                    "domainEntry": {"name": entry_name, "type": "A", "target": "192.0.2.30"},
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: CreateDomainEntry must be served, got {status} {body:?}"
        );

        // The round trip: the created zone and its entry read back through the
        // same endpoint.
        let (status, body) = emulator
            .call_in(region, "GetDomain", json!({"domainName": zone}))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: GetDomain must be served, got {status} {body:?}"
        );
        let domain = body["domain"].clone();
        assert_eq!(
            domain["name"].as_str(),
            Some(zone),
            "case {case}: expected the created zone, got {domain:?}"
        );
        assert_eq!(
            domain["location"]["regionName"].as_str(),
            Some(DOMAIN_REGION),
            "case {case}: expected the domain Region in the reported location, got {:?}",
            domain["location"]
        );
        let entry = domain["domainEntries"][0].clone();
        assert_eq!(
            entry["name"].as_str(),
            Some(entry_name.as_str()),
            "case {case}: expected the added entry, got {entry:?}"
        );
        let entry_id = entry["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an entry id, got {entry:?}"))
            .to_string();

        let (status, body) = emulator
            .call_in(
                region,
                "UpdateDomainEntry",
                json!({
                    "domainName": zone,
                    "domainEntry": {
                        "id": entry_id,
                        "name": entry_name,
                        "type": "A",
                        "target": "192.0.2.31",
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateDomainEntry must be served, got {status} {body:?}"
        );
        let domain = read_zone_in(&emulator, region, zone).await;
        assert_eq!(
            domain["domainEntries"][0]["target"].as_str(),
            Some("192.0.2.31"),
            "case {case}: expected the updated target, got {:?}",
            domain["domainEntries"]
        );

        let (status, body) = emulator
            .call_in(
                region,
                "DeleteDomainEntry",
                json!({
                    "domainName": zone,
                    "domainEntry": {
                        "id": entry_id,
                        "name": entry_name,
                        "type": "A",
                        "target": "192.0.2.31",
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: DeleteDomainEntry must be served, got {status} {body:?}"
        );
        let domain = read_zone_in(&emulator, region, zone).await;
        assert_eq!(
            domain["domainEntries"].as_array().map(Vec::len),
            Some(0),
            "case {case}: expected the deleted entry to be gone, got {:?}",
            domain["domainEntries"]
        );
    }
}

/// The stored zone `name` as `GetDomain` reports it in `region`.
async fn read_zone_in(emulator: &Emulator, region: Option<&str>, name: &str) -> Value {
    let (status, body) = emulator
        .call_in(region, "GetDomain", json!({"domainName": name}))
        .await;
    assert_eq!(
        status, 200,
        "case reading {name} in {region:?}: GetDomain must succeed, got {status} {body:?}"
    );
    body["domain"].clone()
}
