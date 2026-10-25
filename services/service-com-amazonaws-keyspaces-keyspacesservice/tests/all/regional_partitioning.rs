// SPDX-License-Identifier: CC0-1.0

//! Amazon Keyspaces resources are REGIONAL, driven end to end over the service's own
//! `awsJson1_0` wire protocol with a credential scope per request.
//!
//! The rule under test, over a SET of Amazon Web Services Regions rather than one
//! example: a keyspace — with the tables, types and tags it holds — is a resource of the
//! Region the request that created it was signed for, so a request signed for another
//! Region neither enumerates it nor resolves it, and the same name is free there; the
//! service-managed keyspaces are the only ones every Region holds, and each Region
//! reports them with its own Region-qualified ARNs.
//!
//! Concretely:
//!
//! 1. The same `keyspaceName` is created once per Region, each time minting that Region's
//!    ARN, while a second create in the SAME Region is still the modelled conflict.
//! 2. `ListKeyspaces` reports exactly what the asking Region holds, and `GetKeyspace`
//!    answers the modelled 404 for a keyspace only another Region holds.
//! 3. A keyspace's tables, types and tags go with it: every operation that reads or writes
//!    one answers the modelled 404 when asked from another Region, and the same names
//!    created in two Regions are two resources with two ARNs and two tag sets.
//! 4. The service-managed catalog is reported to EVERY Region, including one nothing was
//!    created in, with that Region's keyspace and table ARNs — whichever Region called
//!    first.
//! 5. A delete in one Region leaves the other Region's keyspace of that name untouched.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;
use smithy_server_runtime::MultiServiceRouter;

use crate::keyspace_table_lifecycle::{assert_failure, assert_ok, minimal_schema, settle_wait};

/// The Regions the rows below span: four Regions in different geographies, the last of
/// them the Region an unsigned request is served in
/// (`keyspaces::DEFAULT_REGION`), so no row depends on the emulator's default.
const REGIONS: [&str; 4] = ["us-west-2", "eu-central-1", "ap-southeast-1", "us-east-1"];

/// A Region no request below creates anything in: what a Region holding nothing answers.
const UNTOUCHED_REGION: &str = "eu-west-3";

/// One emulator, called with a SigV4 credential scope per request, so one test drives
/// several Regions against the one store.
struct RegionalEmulator {
    router: MultiServiceRouter,
}

impl RegionalEmulator {
    fn start() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's own concern and is exercised
        // elsewhere; these requests carry a well-formed credential scope so they route
        // and carry a region, but they are not really signed.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_keyspaces_keyspacesservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call one operation as a client signed for `region`, and return its HTTP status and
    /// parsed JSON body.
    async fn call(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method(http::Method::POST)
            .uri("/")
            .header("host", format!("cassandra.{region}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("KeyspacesService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/cassandra/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=abc123"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the test request body serializes"),
            ))
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(raw).unwrap_or_else(|error| {
                panic!(
                    "{operation} answered {status} with a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(raw)
                )
            })
        };
        (status, parsed)
    }
}

/// The published keyspace ARN form, qualified by `region`.
fn keyspace_arn_in(region: &str, keyspace: &str) -> String {
    format!(
        "arn:aws:cassandra:{region}:{}:/keyspace/{keyspace}/",
        engine::emulator_account_id()
    )
}

/// The published table ARN form, qualified by `region`.
fn table_arn_in(region: &str, keyspace: &str, table: &str) -> String {
    format!(
        "arn:aws:cassandra:{region}:{}:/keyspace/{keyspace}/table/{table}",
        engine::emulator_account_id()
    )
}

/// The keyspace a Region holds under a name of its own, so a listing can be checked for
/// what it must NOT report.
fn region_own_keyspace(region: &str) -> String {
    format!("ks_{}", region.replace('-', "_"))
}

/// The `keyspaceName`s of a `ListKeyspaces` body, sorted.
fn keyspace_names(body: &Value) -> Vec<String> {
    let mut names: Vec<String> = body["keyspaces"]
        .as_array()
        .unwrap_or_else(|| panic!("ListKeyspaces must report a `keyspaces` list, got {body}"))
        .iter()
        .map(|summary| {
            summary["keyspaceName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    names.sort();
    names
}

/// The summary a `ListKeyspaces` body reports for `keyspace`.
fn keyspace_summary(body: &Value, keyspace: &str) -> Value {
    body["keyspaces"]
        .as_array()
        .unwrap_or_else(|| panic!("ListKeyspaces must report a `keyspaces` list, got {body}"))
        .iter()
        .find(|summary| summary["keyspaceName"] == json!(keyspace))
        .unwrap_or_else(|| panic!("ListKeyspaces did not report {keyspace:?}: {body}"))
        .clone()
}

/// The names of the service-managed keyspaces every Region holds.
fn service_managed_names() -> Vec<String> {
    keyspaces::SERVICE_MANAGED_KEYSPACES
        .iter()
        .map(|(keyspace, _)| (*keyspace).to_string())
        .collect()
}

/// RULES 1 and 2: a `keyspaceName` names one keyspace PER REGION, and a listing or a read
/// signed for one Region sees that Region's keyspaces alone.
#[tokio::test]
async fn a_keyspace_name_names_one_keyspace_per_region() {
    let emulator = RegionalEmulator::start();
    // The one name every Region in the table creates, which is what makes the Region the
    // only thing telling the resulting keyspaces apart.
    let shared = "regional_ks";

    for region in REGIONS {
        let case = format!("CreateKeyspace {shared:?} signed for {region}");
        let created = assert_ok(
            &case,
            emulator
                .call(region, "CreateKeyspace", json!({"keyspaceName": shared}))
                .await,
        );
        assert_eq!(
            created["resourceArn"],
            json!(keyspace_arn_in(region, shared)),
            "case {case:?}: expected the Region's own keyspace ARN, got {created}"
        );

        // The name is free in each Region and taken in none of the others, but a second
        // create in the SAME Region is still the modelled conflict.
        assert_failure(
            &format!("{case}, a second time"),
            emulator
                .call(region, "CreateKeyspace", json!({"keyspaceName": shared}))
                .await,
            409,
            "ConflictException",
        );

        let own = region_own_keyspace(region);
        assert_ok(
            &format!("CreateKeyspace {own:?} signed for {region}"),
            emulator
                .call(region, "CreateKeyspace", json!({"keyspaceName": own}))
                .await,
        );
    }

    for region in REGIONS {
        let case = format!("ListKeyspaces signed for {region}");
        let listed = assert_ok(
            &case,
            emulator
                .call(region, "ListKeyspaces", json!({"maxResults": 1000}))
                .await,
        );

        // Exactly what this Region holds: the service-managed catalog, the shared name it
        // created, and its own — and nothing another Region created.
        let mut expected = service_managed_names();
        expected.push(shared.to_string());
        expected.push(region_own_keyspace(region));
        expected.sort();
        assert_eq!(
            keyspace_names(&listed),
            expected,
            "case {case:?}: expected {expected:?}, got {listed}"
        );

        // Every ARN the listing carries is this Region's.
        for summary in listed["keyspaces"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no keyspaces list in {listed}"))
        {
            let name = summary["keyspaceName"].as_str().unwrap_or_default();
            assert_eq!(
                summary["resourceArn"],
                json!(keyspace_arn_in(region, name)),
                "case {case:?}: expected {name:?} to carry this Region's ARN, got {summary}"
            );
        }

        // The shared name resolves here, to this Region's keyspace...
        let got = assert_ok(
            &format!("GetKeyspace {shared:?} signed for {region}"),
            emulator
                .call(region, "GetKeyspace", json!({"keyspaceName": shared}))
                .await,
        );
        assert_eq!(
            got["resourceArn"],
            json!(keyspace_arn_in(region, shared)),
            "case {case:?}: expected the Region's own keyspace ARN, got {got}"
        );
        assert_eq!(
            keyspace_summary(&listed, shared)["resourceArn"],
            got["resourceArn"],
            "case {case:?}: the listing and the read must report one resource: {listed} then {got}"
        );

        // ... and a keyspace only ANOTHER Region holds resolves nowhere here.
        for other in REGIONS.iter().filter(|other| **other != region) {
            let elsewhere = region_own_keyspace(other);
            assert_failure(
                &format!("GetKeyspace {elsewhere:?} signed for {region}"),
                emulator
                    .call(region, "GetKeyspace", json!({"keyspaceName": elsewhere}))
                    .await,
                404,
                "ResourceNotFoundException",
            );
            assert_failure(
                &format!("ListTables in {elsewhere:?} signed for {region}"),
                emulator
                    .call(region, "ListTables", json!({"keyspaceName": elsewhere}))
                    .await,
                404,
                "ResourceNotFoundException",
            );
        }
    }

    // A Region nothing was created in holds the catalog and nothing else.
    let untouched = assert_ok(
        UNTOUCHED_REGION,
        emulator
            .call(UNTOUCHED_REGION, "ListKeyspaces", json!({"maxResults": 1000}))
            .await,
    );
    assert_eq!(
        keyspace_names(&untouched),
        service_managed_names(),
        "case {UNTOUCHED_REGION:?}: a Region nothing was created in holds the catalog alone, \
         got {untouched}"
    );
}

/// RULE 3: the tables, types and tags of a keyspace are the Region's too.
#[tokio::test]
async fn a_keyspaces_tables_types_and_tags_belong_to_its_region() {
    let emulator = RegionalEmulator::start();
    let home = REGIONS[0];
    let keyspace = "payments";
    let table = "ledger";
    let udt = "address";

    assert_ok(
        &format!("CreateKeyspace signed for {home}"),
        emulator
            .call(home, "CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    let created = assert_ok(
        &format!("CreateTable signed for {home}"),
        emulator
            .call(
                home,
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "schemaDefinition": minimal_schema("pk"),
                    "tags": [{"key": "owner", "value": "payments-team"}],
                }),
            )
            .await,
    );
    assert_eq!(
        created["resourceArn"],
        json!(table_arn_in(home, keyspace, table)),
        "case {home:?}: expected the Region's own table ARN, got {created}"
    );
    assert_ok(
        &format!("CreateType signed for {home}"),
        emulator
            .call(
                home,
                "CreateType",
                json!({
                    "keyspaceName": keyspace,
                    "typeName": udt,
                    "fieldDefinitions": [{"name": "street", "type": "text"}],
                }),
            )
            .await,
    );

    // Every operation that reads or writes something INSIDE that keyspace answers the
    // modelled 404 when it is asked from another Region, because the keyspace is not a
    // resource of that Region at all.
    let inside: Vec<(&str, Value)> = vec![
        ("ListTables", json!({"keyspaceName": keyspace})),
        (
            "GetTable",
            json!({"keyspaceName": keyspace, "tableName": table}),
        ),
        (
            "GetTableAutoScalingSettings",
            json!({"keyspaceName": keyspace, "tableName": table}),
        ),
        ("ListTypes", json!({"keyspaceName": keyspace})),
        (
            "GetType",
            json!({"keyspaceName": keyspace, "typeName": udt}),
        ),
        (
            "UpdateTable",
            json!({
                "keyspaceName": keyspace,
                "tableName": table,
                "addColumns": [{"name": "injected", "type": "text"}],
            }),
        ),
        (
            "DeleteTable",
            json!({"keyspaceName": keyspace, "tableName": table}),
        ),
        (
            "DeleteType",
            json!({"keyspaceName": keyspace, "typeName": udt}),
        ),
        ("DeleteKeyspace", json!({"keyspaceName": keyspace})),
        (
            "CreateTable",
            json!({
                "keyspaceName": keyspace,
                "tableName": "injected_tbl",
                "schemaDefinition": minimal_schema("pk"),
            }),
        ),
        (
            "CreateType",
            json!({
                "keyspaceName": keyspace,
                "typeName": "injected_type",
                "fieldDefinitions": [{"name": "one", "type": "text"}],
            }),
        ),
        (
            "RestoreTable",
            json!({
                "sourceKeyspaceName": keyspace,
                "sourceTableName": table,
                "targetKeyspaceName": keyspace,
                "targetTableName": "restored_tbl",
            }),
        ),
    ];
    for away in REGIONS.iter().filter(|away| **away != home) {
        for (operation, request) in &inside {
            assert_failure(
                &format!("{operation} on {home}'s keyspace, signed for {away}"),
                emulator.call(away, operation, request.clone()).await,
                404,
                "ResourceNotFoundException",
            );
        }

        // A tag lookup keys off the ARN, which is Region-qualified: the home Region's
        // table ARN names no resource of this one, whether or not it holds a keyspace of
        // that name yet.
        for (operation, request) in [
            (
                "ListTagsForResource",
                json!({"resourceArn": table_arn_in(home, keyspace, table)}),
            ),
            (
                "TagResource",
                json!({
                    "resourceArn": table_arn_in(home, keyspace, table),
                    "tags": [{"key": "owner", "value": "someone-else"}],
                }),
            ),
            (
                "UntagResource",
                json!({
                    "resourceArn": table_arn_in(home, keyspace, table),
                    "tags": [{"key": "owner", "value": "payments-team"}],
                }),
            ),
        ] {
            assert_failure(
                &format!("{operation} on {home}'s table ARN, signed for {away}"),
                emulator.call(away, operation, request).await,
                404,
                "ResourceNotFoundException",
            );
        }
    }

    // The same names created in a SECOND Region are a second resource: its own ARN, and
    // none of the first Region's tags.
    let away = REGIONS[1];
    assert_ok(
        &format!("CreateKeyspace signed for {away}"),
        emulator
            .call(away, "CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    let elsewhere = assert_ok(
        &format!("CreateTable signed for {away}"),
        emulator
            .call(
                away,
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    assert_eq!(
        elsewhere["resourceArn"],
        json!(table_arn_in(away, keyspace, table)),
        "case {away:?}: expected the second Region's own table ARN, got {elsewhere}"
    );
    let away_tags = assert_ok(
        &format!("ListTagsForResource signed for {away}"),
        emulator
            .call(
                away,
                "ListTagsForResource",
                json!({"resourceArn": table_arn_in(away, keyspace, table)}),
            )
            .await,
    );
    assert_eq!(
        away_tags.get("tags"),
        None,
        "case {away:?}: the second Region's table carries its own (empty) tags, got {away_tags}"
    );
    let home_tags = assert_ok(
        &format!("ListTagsForResource signed for {home}"),
        emulator
            .call(
                home,
                "ListTagsForResource",
                json!({"resourceArn": table_arn_in(home, keyspace, table)}),
            )
            .await,
    );
    assert_eq!(
        home_tags["tags"],
        json!([{"key": "owner", "value": "payments-team"}]),
        "case {home:?}: the first Region's table keeps the tags its create applied, got {home_tags}"
    );
    // The second Region's keyspace holds the table it created and nothing of the first's.
    let away_types = assert_ok(
        &format!("ListTypes signed for {away}"),
        emulator
            .call(away, "ListTypes", json!({"keyspaceName": keyspace}))
            .await,
    );
    assert_eq!(
        away_types["types"],
        json!([]),
        "case {away:?}: the type created in {home} is not a type of this Region's keyspace, \
         got {away_types}"
    );
    // And the home Region's table ARN still names the home Region's resource, with the
    // ARN the lookup compares against unchanged by anything above.
    let home_table = assert_ok(
        &format!("GetTable signed for {home}"),
        emulator
            .call(
                home,
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
    );
    assert_eq!(
        home_table["resourceArn"],
        json!(table_arn_in(home, keyspace, table)),
        "case {home:?}: expected the Region's own table ARN, got {home_table}"
    );
}

/// RULE 4: every Region reports the service-managed catalog, with its own ARNs — the
/// Region that happened to call first changes nothing.
#[tokio::test]
async fn the_service_managed_catalog_carries_the_asking_regions_arns() {
    let emulator = RegionalEmulator::start();

    // The Region that calls first is not special, so the rows below are walked once in
    // this order and once reversed, against the one store.
    let mut asking: Vec<&str> = REGIONS.to_vec();
    asking.push(UNTOUCHED_REGION);
    let reversed: Vec<&str> = asking.iter().copied().rev().collect();

    for order in [asking, reversed] {
        for region in order {
            let case = format!("the catalog as reported to {region}");
            let listed = assert_ok(
                &case,
                emulator
                    .call(region, "ListKeyspaces", json!({"maxResults": 1000}))
                    .await,
            );
            for (keyspace, system_tables) in keyspaces::SERVICE_MANAGED_KEYSPACES {
                assert_eq!(
                    keyspace_summary(&listed, keyspace)["resourceArn"],
                    json!(keyspace_arn_in(region, keyspace)),
                    "case {case:?}: {keyspace:?} must carry this Region's ARN, got {listed}"
                );

                let got = assert_ok(
                    &format!("GetKeyspace {keyspace:?} signed for {region}"),
                    emulator
                        .call(region, "GetKeyspace", json!({"keyspaceName": keyspace}))
                        .await,
                );
                assert_eq!(
                    got["resourceArn"],
                    json!(keyspace_arn_in(region, keyspace)),
                    "case {case:?}: {keyspace:?} must resolve to this Region's ARN, got {got}"
                );

                let tables = assert_ok(
                    &format!("ListTables in {keyspace:?} signed for {region}"),
                    emulator
                        .call(region, "ListTables", json!({"keyspaceName": keyspace}))
                        .await,
                );
                for summary in tables["tables"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: no tables list in {tables}"))
                {
                    let name = summary["tableName"].as_str().unwrap_or_default();
                    assert_eq!(
                        summary["resourceArn"],
                        json!(table_arn_in(region, keyspace, name)),
                        "case {case:?}: {keyspace}.{name} must carry this Region's ARN, got \
                         {summary}"
                    );
                }

                // One system table of this keyspace, read back by name.
                let table = system_tables
                    .first()
                    .unwrap_or_else(|| panic!("case {case:?}: the catalog gives it a table"));
                let read = assert_ok(
                    &format!("GetTable {keyspace}.{table} signed for {region}"),
                    emulator
                        .call(
                            region,
                            "GetTable",
                            json!({"keyspaceName": keyspace, "tableName": table}),
                        )
                        .await,
                );
                assert_eq!(
                    read["resourceArn"],
                    json!(table_arn_in(region, keyspace, table)),
                    "case {case:?}: {keyspace}.{table} must carry this Region's ARN, got {read}"
                );
            }
        }
    }
}

/// RULE 5: a delete is a delete in its own Region, and nowhere else.
#[tokio::test]
async fn a_delete_in_one_region_leaves_the_other_regions_keyspace() {
    let emulator = RegionalEmulator::start();
    let keyspace = "shared_name_ks";
    let table = "rows";
    let [dropped, kept, ..] = REGIONS;

    for region in [dropped, kept] {
        assert_ok(
            &format!("CreateKeyspace signed for {region}"),
            emulator
                .call(region, "CreateKeyspace", json!({"keyspaceName": keyspace}))
                .await,
        );
        assert_ok(
            &format!("CreateTable signed for {region}"),
            emulator
                .call(
                    region,
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "schemaDefinition": minimal_schema("pk"),
                    }),
                )
                .await,
        );
    }

    // Both creations settle, then the keyspace of ONE Region is dropped.
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        &format!("DeleteKeyspace signed for {dropped}"),
        emulator
            .call(dropped, "DeleteKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    assert_failure(
        &format!("GetKeyspace signed for {dropped}, after its delete"),
        emulator
            .call(dropped, "GetKeyspace", json!({"keyspaceName": keyspace}))
            .await,
        404,
        "ResourceNotFoundException",
    );
    let survivor = assert_ok(
        &format!("GetKeyspace signed for {kept}, after {dropped}'s delete"),
        emulator
            .call(kept, "GetKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    assert_eq!(
        survivor["resourceArn"],
        json!(keyspace_arn_in(kept, keyspace)),
        "case {kept:?}: a delete in {dropped} must leave this Region's keyspace, got {survivor}"
    );
    let survivor_table = assert_ok(
        &format!("GetTable signed for {kept}, after {dropped}'s delete"),
        emulator
            .call(
                kept,
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
    );
    assert_eq!(
        survivor_table["resourceArn"],
        json!(table_arn_in(kept, keyspace, table)),
        "case {kept:?}: its table goes nowhere either, got {survivor_table}"
    );
    // And the name the delete freed is creatable again in that Region alone.
    assert_ok(
        &format!("CreateKeyspace signed for {dropped}, after its delete"),
        emulator
            .call(dropped, "CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    assert_failure(
        &format!("CreateKeyspace signed for {kept}, which still holds it"),
        emulator
            .call(kept, "CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
        409,
        "ConflictException",
    );
}
