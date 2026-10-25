// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the REGION a connection belongs to, driven over the wire the
//! way a client drives it: `POST /` with `X-Amz-Target: Interconnect.<Operation>` on
//! `awsJson1_0`, through the same `MultiServiceRouter::dispatch` the socket front
//! door uses.
//!
//! The RULE under test: this service's endpoints are regional
//! (`smithy_query(query="service_info")` gives
//! `endpointResolution.urlTemplates = https://interconnect.{Region}.{dnsSuffix}`, and
//! `ListEnvironments` is documented as listing the environments "that can produce
//! connections that will land in the called AWS region"), so a connection belongs to
//! the region whose endpoint created it: only a request signed for that region
//! resolves, lists, updates or deletes it, and every other region answers exactly as
//! it does for an identifier that was never allocated.
//!
//! Every assertion is checked over a TABLE whose rows differ in the region the
//! connection is created in, the environment and provider kind it is created on, the
//! bandwidth, and — for the reads — the region the read is issued from, including a
//! region that holds no connection at all. Pagination is walked at a page size of one
//! as well as at the default, so the rule is checked over the cursor sequence too.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use service_com_amazonaws_interconnect_interconnect::state::{
    connection_absent_message, connection_not_retrievable_message, proposal_not_retrievable_message,
};

use crate::emulator::{
    code, message, placements, remote_account_for, text, Emulator, DEFAULT_SIGNING_REGION,
};

/// The account id every emulator ARN carries.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The signing name the model gives this service; also its ARN namespace.
const SIGNING_NAME: &str = "interconnect";

/// The regions the rows are created in: the one the rest of this crate's suites sign
/// for, plus three more on three other continents, so a row's region is never the
/// harness default by accident.
const CREATE_REGIONS: [&str; 4] = [
    DEFAULT_SIGNING_REGION,
    "eu-west-1",
    "ap-southeast-2",
    "us-west-2",
];

/// A region no row creates anything in, so its endpoint must answer with an empty
/// listing however many connections the other regions hold.
const EMPTY_REGION: &str = "eu-central-1";

/// A well-formed `ConnectionId` (`^(mcc|lmcc)-[a-z0-9]{8}$`) that no create ever
/// allocates: the baseline refusal every out-of-region call is compared against.
const NEVER_ALLOCATED_ID: &str = "mcc-00000000";

/// One row: a connection to create, and the region whose endpoint creates it.
#[derive(Debug, Clone)]
struct RegionCase {
    region: &'static str,
    environment_id: String,
    bandwidth: String,
    /// A Direct Connect gateway of the account, which is what an attach point IS, so
    /// the create is about the region and not about the attach point.
    attach_point: String,
    /// The identifying information this row's Environment asks a create for, or `None`
    /// for one that declares no kind and so asks for nothing.
    remote_account: Option<Value>,
    seed: usize,
}

impl RegionCase {
    fn label(&self) -> String {
        format!(
            "{} / {} / {} in {}",
            self.seed, self.environment_id, self.bandwidth, self.region
        )
    }

    fn request(&self) -> Value {
        let mut request = Map::new();
        request.insert("environmentId".to_string(), json!(self.environment_id));
        request.insert("bandwidth".to_string(), json!(self.bandwidth));
        request.insert(
            "attachPoint".to_string(),
            json!({ "directConnectGateway": self.attach_point }),
        );
        request.insert(
            "description".to_string(),
            json!(format!("region case {}", self.seed)),
        );
        if let Some(remote_account) = &self.remote_account {
            request.insert("remoteAccount".to_string(), remote_account.clone());
        }
        Value::Object(request)
    }
}

/// The table: every `(environmentId, bandwidth)` the live catalog offers, each row
/// placed in the next region of [`CREATE_REGIONS`] so the rows differ in the region
/// as well as in the environment and the bandwidth.
async fn region_table(emulator: &Emulator) -> Vec<RegionCase> {
    let attach_point = emulator.attach_point().await;
    let mut rows = Vec::new();
    for (seed, (environment_id, bandwidth)) in placements(emulator).await.into_iter().enumerate() {
        let remote_account = remote_account_for(&emulator.environment(&environment_id).await);
        rows.push(RegionCase {
            region: CREATE_REGIONS[seed % CREATE_REGIONS.len()],
            environment_id,
            bandwidth,
            attach_point: attach_point.clone(),
            remote_account,
            seed,
        });
    }
    assert!(
        rows.len() >= CREATE_REGIONS.len(),
        "the table must place a connection in every region under test, got {} rows",
        rows.len()
    );
    rows
}

/// Create one row's connection and answer the stored `Connection`, checking the one
/// thing that grounds the rule: the record's ARN carries the region that created it.
async fn create(emulator: &Emulator, case: &RegionCase) -> Value {
    let created = emulator
        .ic_ok_in(case.region, "CreateConnection", case.request())
        .await;
    let connection = created["connection"].clone();
    let id = text(&connection, "id");
    let arn = text(&connection, "arn");
    assert_eq!(
        arn,
        format!("arn:aws:{SIGNING_NAME}:{}:{ACCOUNT_ID}:connection/{id}", case.region),
        "case {:?}: expected the ARN to carry the region the create was signed for, got {arn:?}",
        case.label()
    );
    connection
}

/// Assert `(status, body)` is the 404 this service answers an identifier that names
/// no connection with, worded `expected`.
fn assert_absent(case: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404 for an identifier this endpoint does not hold, got {status} with {body:?}"
    );
    assert!(
        code(body).contains("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        body["__type"]
    );
    let reported = message(body);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the service's own not-found wording, got {reported:?}"
    );
}

/// Every connection `ListConnections` reports in `region`, walking the pages at
/// `max_results` so the answer is the whole listing however it is windowed.
async fn listed_in(emulator: &Emulator, region: &str, max_results: Option<i32>) -> Vec<Value> {
    let mut all = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = Map::new();
        if let Some(max) = max_results {
            request.insert("maxResults".to_string(), json!(max));
        }
        if let Some(token) = &token {
            request.insert("nextToken".to_string(), json!(token));
        }
        let listed = emulator
            .ic_ok_in(region, "ListConnections", Value::Object(request))
            .await;
        all.extend(
            listed["connections"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        token = listed["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return all;
        }
    }
}

/// The `id`s of a listing, as a set, so a region's listing can be compared with what
/// that region created.
fn ids_of(connections: &[Value]) -> BTreeSet<String> {
    connections
        .iter()
        .map(|summary| text(summary, "id"))
        .collect()
}

/// An identifier read, updated or deleted through a region that does not hold the
/// connection is refused exactly as one that was never allocated is — and the
/// refused call changes nothing, so the region that does hold it still reads back
/// the record it created.
#[tokio::test]
async fn a_connection_is_reached_only_through_the_region_that_created_it() {
    let emulator = Emulator::new();
    let table = region_table(&emulator).await;

    for case in &table {
        let label = case.label();
        let connection = create(&emulator, case).await;
        let id = text(&connection, "id");
        let activation_key = text(&connection, "activationKey");

        // The region that created it holds it: the record reads back, and so does
        // the proposal its activation key names.
        let fetched = emulator
            .ic_ok_in(case.region, "GetConnection", json!({ "identifier": id }))
            .await;
        assert_eq!(
            fetched["connection"], connection,
            "case {label:?}: expected the creating region to read the record back, got {:?}",
            fetched["connection"]
        );
        emulator
            .ic_ok_in(
                case.region,
                "DescribeConnectionProposal",
                json!({ "activationKey": activation_key }),
            )
            .await;

        // Every OTHER region answers as it does for an identifier that was never
        // allocated — the same status, code and wording, for the read, the update,
        // the delete and the proposal the activation key names.
        let elsewhere: Vec<&str> = CREATE_REGIONS
            .iter()
            .copied()
            .chain(std::iter::once(EMPTY_REGION))
            .filter(|region| *region != case.region)
            .collect();
        for region in elsewhere {
            let read_case = format!("{label} read from {region}");
            let (status, body) = emulator
                .ic_in(region, "GetConnection", json!({ "identifier": id }))
                .await;
            assert_absent(
                &read_case,
                status,
                &body,
                &connection_not_retrievable_message(&id),
            );
            // The baseline: an id no create ever allocated, in the same region.
            let (baseline_status, baseline_body) = emulator
                .ic_in(
                    region,
                    "GetConnection",
                    json!({ "identifier": NEVER_ALLOCATED_ID }),
                )
                .await;
            assert_absent(
                &format!("{read_case} (never-allocated id)"),
                baseline_status,
                &baseline_body,
                &connection_not_retrievable_message(NEVER_ALLOCATED_ID),
            );

            let (status, body) = emulator
                .ic_in(
                    region,
                    "UpdateConnection",
                    json!({ "identifier": id, "description": "cross region write" }),
                )
                .await;
            assert_absent(
                &format!("{label} update from {region}"),
                status,
                &body,
                &connection_absent_message(&id),
            );

            let (status, body) = emulator
                .ic_in(region, "DeleteConnection", json!({ "identifier": id }))
                .await;
            assert_absent(
                &format!("{label} delete from {region}"),
                status,
                &body,
                &connection_not_retrievable_message(&id),
            );

            let (status, body) = emulator
                .ic_in(
                    region,
                    "DescribeConnectionProposal",
                    json!({ "activationKey": activation_key }),
                )
                .await;
            assert_absent(
                &format!("{label} proposal from {region}"),
                status,
                &body,
                &proposal_not_retrievable_message(),
            );
        }

        // The refused calls wrote nothing: the creating region still reads the
        // record exactly as it was created.
        let refetched = emulator
            .ic_ok_in(case.region, "GetConnection", json!({ "identifier": id }))
            .await;
        assert_eq!(
            refetched["connection"], connection,
            "case {label:?}: expected the out-of-region calls to have changed nothing, got {:?}",
            refetched["connection"]
        );
    }
}

/// `ListConnections` reports the connections of the region it is called in and no
/// others — whatever page size it is walked at, and whatever filter it carries.
#[tokio::test]
async fn list_connections_reports_only_the_region_it_is_called_in() {
    let emulator = Emulator::new();
    let table = region_table(&emulator).await;

    // Create every row first, so each listing below runs against a store holding
    // connections of several regions at once.
    let mut created: Vec<(RegionCase, String)> = Vec::new();
    for case in &table {
        let connection = create(&emulator, case).await;
        created.push((case.clone(), text(&connection, "id")));
    }

    for region in CREATE_REGIONS.iter().copied().chain([EMPTY_REGION]) {
        let expected: BTreeSet<String> = created
            .iter()
            .filter(|(case, _)| case.region == region)
            .map(|(_, id)| id.clone())
            .collect();

        // Walked at the default page size and at a page size of one: the cursor
        // sequence covers the region's own set and nothing else.
        for max_results in [None, Some(1), Some(2)] {
            let listed = listed_in(&emulator, region, max_results).await;
            let got = ids_of(&listed);
            assert_eq!(
                got, expected,
                "case {region:?} at maxResults {max_results:?}: expected exactly this region's connections, got {got:?}"
            );
            for summary in &listed {
                let arn = text(summary, "arn");
                assert!(
                    arn.starts_with(&format!("arn:aws:{SIGNING_NAME}:{region}:")),
                    "case {region:?}: expected every listed ARN to carry the called region, got {arn:?}"
                );
            }
        }

        // A filter selects inside the region's set, never across regions: the
        // environmentId rows of OTHER regions are not reported here.
        for environment_id in created
            .iter()
            .map(|(case, _)| case.environment_id.clone())
            .collect::<BTreeSet<String>>()
        {
            let listed = emulator
                .ic_ok_in(
                    region,
                    "ListConnections",
                    json!({ "environmentId": environment_id }),
                )
                .await;
            let got = ids_of(listed["connections"].as_array().unwrap_or(&Vec::new()));
            let want: BTreeSet<String> = created
                .iter()
                .filter(|(case, _)| case.region == region && case.environment_id == environment_id)
                .map(|(_, id)| id.clone())
                .collect();
            assert_eq!(
                got, want,
                "case {region:?} filtered on {environment_id:?}: expected this region's matching connections, got {got:?}"
            );
        }
    }
}

/// A `clientToken` is scoped to the regional endpoint it was sent to: the same token
/// sent to another region allocates that region's own connection instead of replaying
/// one the caller cannot see, and each region keeps replaying its own.
#[tokio::test]
async fn a_client_token_replays_only_within_the_region_it_was_sent_to() {
    let emulator = Emulator::new();
    let table = region_table(&emulator).await;

    for case in &table {
        let label = case.label();
        let token = format!("token-{}", case.seed);
        let mut request = case.request();
        request["clientToken"] = json!(token);

        // Rows differ in the pair of regions the same token is sent to: the row's
        // own region, and the next one along — always a different region, because
        // the row's own is `CREATE_REGIONS[seed % len]` and this is the next index.
        let other = CREATE_REGIONS[(case.seed + 1) % CREATE_REGIONS.len()];

        let first = emulator
            .ic_ok_in(case.region, "CreateConnection", request.clone())
            .await;
        let elsewhere = emulator
            .ic_ok_in(other, "CreateConnection", request.clone())
            .await;
        let first_id = text(&first["connection"], "id");
        let other_id = text(&elsewhere["connection"], "id");
        assert_ne!(
            first_id, other_id,
            "case {label:?}: expected the same token in {other} to allocate that region's own connection, got {other_id:?}"
        );

        // Each region replays ITS own connection for that token.
        for (region, expected) in [(case.region, &first_id), (other, &other_id)] {
            let replay = emulator
                .ic_ok_in(region, "CreateConnection", request.clone())
                .await;
            let replayed = text(&replay["connection"], "id");
            assert_eq!(
                replayed, *expected,
                "case {label:?}: expected {region} to replay its own connection for token {token:?}, got {replayed:?}"
            );
            let fetched = emulator
                .ic_ok_in(region, "GetConnection", json!({ "identifier": expected }))
                .await;
            assert_eq!(
                text(&fetched["connection"], "id"),
                *expected,
                "case {label:?}: expected {region} to hold the connection it allocated, got {:?}",
                fetched["connection"]
            );
        }

        // And neither region sees the other's: the token's two connections are two
        // records, each in its own scope.
        let (status, body) = emulator
            .ic_in(case.region, "GetConnection", json!({ "identifier": other_id }))
            .await;
        assert_absent(
            &format!("{label}: {other}'s connection read from {}", case.region),
            status,
            &body,
            &connection_not_retrievable_message(&other_id),
        );
    }
}
