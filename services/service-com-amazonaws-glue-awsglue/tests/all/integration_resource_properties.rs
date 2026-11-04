// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue integration resource property slice:
//! `CreateIntegrationResourceProperty`, the `GetIntegrationResourceProperty` that reads
//! back what it stored, and the `DeleteIntegrationResourceProperty` that removes it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a record is addressed by the ARN of the resource whose properties
//! it carries, and there is ONE record per resource — so a create for a resource that
//! already has one writes that record (each stated member replacing the stored one, each
//! unstated member left alone) under the one `ResourcePropertyArn` that resource derives,
//! rather than minting a second record or refusing the write. The round-trip suite is a
//! table whose rows differ along which side of the integration the properties belong to,
//! which resource kind the ARN names, how many members the side carries, their length, and
//! whether the create carried tags.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateIntegrationResourcePropertyInput;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, IntegrationResourcePropertyEntry,
    MAX_INTEGRATION_RESOURCE_PROPERTIES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    assert_echoed, call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID, REGION,
};

/// The longest `RoleArn` either side accepts: "len 1..128" on `String128`
/// (`smithy_query shape=SourceProcessingProperties,TargetProcessingProperties`).
const MAX_ROLE_ARN_LENGTH: usize = 128;

/// The longest `KmsArn`/`EventBusArn` a target carries: "len 1..2048" on `String2048`
/// (same query).
const MAX_KMS_ARN_LENGTH: usize = 2048;

/// The ARN of the Glue connection `name`, as this emulator mints a connection's ARN.
fn connection_arn(name: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:connection/{name}")
}

/// The ARN of the Glue database `name`, the same way.
fn database_arn(name: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:database/{name}")
}

/// Register a Glue connection, the resource a source's properties belong to.
async fn create_connection(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateConnection",
        &json!({
            "ConnectionInput": {
                "Name": name,
                "ConnectionType": "JDBC",
                "ConnectionProperties": {
                    "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                    "USERNAME": "etl",
                },
            },
        }),
    )
    .await;
}

/// Register a Glue database, the resource a target's properties belong to.
async fn create_database(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": name } }),
    )
    .await;
}

/// Write one integration resource property record and return the response body.
async fn create_property(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateIntegrationResourceProperty", body).await
}

/// The `GetIntegrationResourceProperty` response for `resource_arn`.
async fn get_property(router: &MultiServiceRouter, resource_arn: &str) -> Value {
    ok(
        router,
        "GetIntegrationResourceProperty",
        &json!({ "ResourceArn": resource_arn }),
    )
    .await
}

/// The `ResourcePropertyArn` of a response, which every call that reports a record carries.
fn property_arn(case: &str, body: &Value) -> String {
    body.get("ResourcePropertyArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: the response must carry a ResourcePropertyArn, got {body}")
        })
        .to_string()
}

/// Assert that `arn` is the record ARN the resource `resource_arn` names derives: the
/// documented resource type ("The format is something like
/// arn:aws:glue:::integrationresourceproperty/*" —
/// `CreateIntegrationResourcePropertyResponse$ResourcePropertyArn`) under this endpoint's
/// Region and account, carrying the resource the record describes — the resource type and
/// name its own ARN carries — so a caller that holds only the ARN it submitted derives the
/// record's.
fn assert_record_arn(case: &str, resource_arn: &str, arn: &str) {
    let described = resource_arn
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("case {case}: {resource_arn:?} carries no resource portion"))
        .1;
    let expected =
        format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:integrationresourceproperty/{described}");
    assert_eq!(
        arn, expected,
        "case {case}: the record ARN must be derived from the resource it describes; expected \
         {expected:?}, got {arn:?}"
    );
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateIntegrationResourceProperty` persists is what
/// `GetIntegrationResourceProperty` reports: both calls answer the record now stored for
/// the ARN the request named, member for member, under the one `ResourcePropertyArn` that
/// resource derives.
#[tokio::test]
async fn created_integration_resource_properties_are_read_back_from_the_store() {
    let router = emulator();

    let long_role = format!("arn:aws:iam::{ACCOUNT_ID}:role/")
        + &"r".repeat(MAX_ROLE_ARN_LENGTH - format!("arn:aws:iam::{ACCOUNT_ID}:role/").len());
    let long_kms = format!("arn:aws:kms:{REGION}:{ACCOUNT_ID}:key/")
        + &"k".repeat(
            MAX_KMS_ARN_LENGTH - format!("arn:aws:kms:{REGION}:{ACCOUNT_ID}:key/").len(),
        );

    // (case, whether the ARN names a connection, the properties the create carries)
    let cases: Vec<(&str, bool, Value)> = vec![
        (
            // The shape a client setting up a source sends: the connection's role alone.
            "a connection's source role",
            true,
            json!({
                "SourceProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-source"),
                },
            }),
        ),
        (
            "a database's target role",
            false,
            json!({
                "TargetProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-target"),
                },
            }),
        ),
        (
            "a target carrying every modelled member",
            false,
            json!({
                "TargetProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-target-full"),
                    "KmsArn": format!("arn:aws:kms:{REGION}:{ACCOUNT_ID}:key/11111111-2222-3333-4444-555555555555"),
                    "ConnectionName": "vpc-network-connection",
                    "EventBusArn": format!("arn:aws:events:{REGION}:{ACCOUNT_ID}:event-bus/integration-status"),
                },
            }),
        ),
        (
            "both sides stated in one call",
            true,
            json!({
                "SourceProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-both-source"),
                },
                "TargetProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-both-target"),
                    "ConnectionName": "both-sides-connection",
                },
            }),
        ),
        (
            "tags sent with the create",
            true,
            json!({
                "SourceProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-tagged"),
                },
                "Tags": [
                    { "key": "owner", "value": "data-platform" },
                    { "key": "tier", "value": "gold" },
                ],
            }),
        ),
        (
            "a role at the modelled length bound",
            true,
            json!({ "SourceProcessingProperties": { "RoleArn": long_role } }),
        ),
        (
            "a KMS ARN at the modelled length bound",
            false,
            json!({ "TargetProcessingProperties": { "KmsArn": long_kms } }),
        ),
        (
            "a role of the shortest length the model admits",
            false,
            json!({ "TargetProcessingProperties": { "RoleArn": "r" } }),
        ),
    ];

    let mut stored: Vec<(&str, String, Value)> = Vec::new();
    for (index, (case, is_connection, properties)) in cases.iter().enumerate() {
        let name = format!("integration-resource-{index}");
        let resource_arn = if *is_connection {
            create_connection(&router, &name).await;
            connection_arn(&name)
        } else {
            create_database(&router, &name).await;
            database_arn(&name)
        };

        let mut request = properties.clone();
        request["ResourceArn"] = json!(resource_arn);
        let created = create_property(&router, &request).await;
        // The two property sides are what a response carries; `Tags` has no member on it.
        let mut sides = properties.clone();
        if let Some(members) = sides.as_object_mut() {
            members.remove("Tags");
        }

        // "The connection ARN of the source, or the database ARN of the target": the
        // create reports the resource it wrote the record for.
        assert_eq!(
            created.get("ResourceArn").and_then(Value::as_str),
            Some(resource_arn.as_str()),
            "case {case}: the create must report the ResourceArn it stored, got {created}"
        );
        let record_arn = property_arn(case, &created);
        assert_record_arn(case, &resource_arn, &record_arn);
        // Every property member the request carried comes back from the store unchanged.
        assert_echoed(case, &sides, &created);
        // `CreateIntegrationResourcePropertyResponse` models no `Tags` member, so the tags
        // a create carried are not part of what it answers.
        assert!(
            created.get("Tags").is_none(),
            "case {case}: the create must report no Tags, got {created}"
        );

        let read = get_property(&router, &resource_arn).await;
        assert_eq!(
            read.get("ResourceArn").and_then(Value::as_str),
            Some(resource_arn.as_str()),
            "case {case}: the read must report the resource it was asked for, got {read}"
        );
        assert_eq!(
            property_arn(case, &read),
            record_arn,
            "case {case}: the read must report the same record ARN the create did, got {read}"
        );
        assert_echoed(case, &sides, &read);

        stored.push((case, resource_arn, sides));
    }

    // Each resource has its OWN record: reading one back after every create has landed
    // still reports that resource's properties and its own ARN, so no create overwrote
    // another resource's record.
    let mut record_arns: Vec<String> = Vec::new();
    for (case, resource_arn, properties) in &stored {
        let read = get_property(&router, resource_arn).await;
        assert_echoed(case, properties, &read);
        record_arns.push(property_arn(case, &read));
    }
    let distinct: std::collections::BTreeSet<&String> = record_arns.iter().collect();
    assert_eq!(
        distinct.len(),
        record_arns.len(),
        "case every record: each resource must carry its own ResourcePropertyArn, got \
         {record_arns:?}"
    );
}

/// A create for a resource that already has a record writes THAT record: it answers 200
/// rather than refusing a duplicate, reports the same `ResourcePropertyArn` as the first
/// create, replaces every member it states and leaves every member it does not state as
/// stored — and the store still holds one record per resource.
#[tokio::test]
async fn a_second_create_for_one_resource_writes_the_record_it_already_has() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    let source = |role: &str| json!({ "SourceProcessingProperties": { "RoleArn": role } });
    let target = |role: &str| json!({ "TargetProcessingProperties": { "RoleArn": role } });

    // (case, the first create's properties, the second create's, what must then be stored)
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "the same side with another role replaces it",
            source("arn:aws:iam::000000000000:role/first"),
            source("arn:aws:iam::000000000000:role/second"),
            source("arn:aws:iam::000000000000:role/second"),
        ),
        (
            "the other side added leaves the first side stored",
            source("arn:aws:iam::000000000000:role/kept-source"),
            target("arn:aws:iam::000000000000:role/added-target"),
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/kept-source" },
                "TargetProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/added-target" },
            }),
        ),
        (
            "a member of a stated side replaces only that side",
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/src" },
                "TargetProcessingProperties": {
                    "RoleArn": "arn:aws:iam::000000000000:role/tgt",
                    "ConnectionName": "first-network",
                },
            }),
            json!({
                "TargetProcessingProperties": {
                    "RoleArn": "arn:aws:iam::000000000000:role/tgt",
                    "ConnectionName": "second-network",
                },
            }),
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/src" },
                "TargetProcessingProperties": {
                    "RoleArn": "arn:aws:iam::000000000000:role/tgt",
                    "ConnectionName": "second-network",
                },
            }),
        ),
        (
            "tags sent beside a write of the stored side leave both sides as stored",
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/tags-source" },
            }),
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/tags-source" },
                "Tags": [{ "key": "owner", "value": "data-platform" }],
            }),
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/tags-source" },
            }),
        ),
        (
            "the identical create repeated changes nothing",
            source("arn:aws:iam::000000000000:role/idempotent"),
            source("arn:aws:iam::000000000000:role/idempotent"),
            source("arn:aws:iam::000000000000:role/idempotent"),
        ),
    ];

    for (index, (case, first, second, expected)) in cases.iter().enumerate() {
        let name = format!("upserted-{index}");
        create_connection(&router, &name).await;
        let resource_arn = connection_arn(&name);

        let mut request = first.clone();
        request["ResourceArn"] = json!(resource_arn);
        let created = create_property(&router, &request).await;
        let record_arn = property_arn(case, &created);

        let mut again = second.clone();
        again["ResourceArn"] = json!(resource_arn);
        let (status, body) = call(&router, "CreateIntegrationResourceProperty", &again).await;
        assert_eq!(
            status, 200,
            "case {case}: a second create for one resource must answer 200, got {status} with \
             {body}"
        );
        assert_eq!(
            property_arn(case, &body),
            record_arn,
            "case {case}: the record keeps the ResourcePropertyArn the first create \
             reported, expected {record_arn}, got {body}"
        );
        assert_echoed(case, expected, &body);

        let read = get_property(&router, &resource_arn).await;
        assert_echoed(case, expected, &read);
        assert_eq!(
            property_arn(case, &read),
            record_arn,
            "case {case}: the read must report the same record ARN, expected {record_arn}, got \
             {read}"
        );
    }

    // One record per resource: the second create of each row wrote the record its row's
    // resource already had rather than adding one.
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let held = state.lock().integration_resource_properties.len();
    assert_eq!(
        held,
        cases.len(),
        "case every row: expected one record per resource ({}), got {held}",
        cases.len()
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The sentence a create that states neither processing side is refused with: fixed text
/// naming the two sides, carrying no value read off the request or the store.
const NEITHER_SIDE_MESSAGE: &str = "Either source processing properties or target \
     processing properties must be provided in the request.";

/// What a create writes is a side of the integration, so a request stating NEITHER
/// `SourceProcessingProperties` nor `TargetProcessingProperties` is refused with the
/// modelled `ValidationException` and persists nothing — whichever resource kind it
/// addresses, whatever else it carries, and whether or not that resource already has a
/// record — while a request stating either side, or both, is served. The rows differ along
/// which sides the request states, how many members those sides carry, and what the request
/// carries beside them.
#[tokio::test]
async fn a_create_stating_neither_processing_side_is_refused() {
    let router = emulator();

    // (case, whether the ARN names a connection, the members beside `ResourceArn`, whether
    // the create must be served)
    let cases: Vec<(&str, bool, Value, bool)> = vec![
        (
            // The recorded case: a database ARN and nothing else.
            "neither side, on a database",
            false,
            json!({}),
            false,
        ),
        ("neither side, on a connection", true, json!({}), false),
        (
            "neither side, with tags alone",
            false,
            json!({ "Tags": [{ "key": "owner", "value": "data-platform" }] }),
            false,
        ),
        (
            "the source side alone",
            true,
            json!({
                "SourceProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-source"),
                },
            }),
            true,
        ),
        (
            "the target side alone",
            false,
            json!({
                "TargetProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-target"),
                },
            }),
            true,
        ),
        (
            "both sides at once",
            true,
            json!({
                "SourceProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-both-source"),
                },
                "TargetProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-both-target"),
                    "ConnectionName": "both-sides-connection",
                },
            }),
            true,
        ),
        (
            "a side stated beside tags",
            false,
            json!({
                "TargetProcessingProperties": {
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-tagged-target"),
                },
                "Tags": [{ "key": "tier", "value": "gold" }],
            }),
            true,
        ),
    ];

    for (index, (case, is_connection, members, served)) in cases.iter().enumerate() {
        let name = format!("side-required-{index}");
        let resource_arn = if *is_connection {
            create_connection(&router, &name).await;
            connection_arn(&name)
        } else {
            create_database(&router, &name).await;
            database_arn(&name)
        };

        let mut request = members.clone();
        request["ResourceArn"] = json!(resource_arn);
        let (status, body) = call(&router, "CreateIntegrationResourceProperty", &request).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: a create stating a side must be served, got {status} with {body}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: a create stating neither side is refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            NEITHER_SIDE_MESSAGE,
            "case {case:?}: expected the fixed refusal sentence, got {:?}",
            error_message(&body)
        );

        // The refusal persisted nothing, so the resource still has no record.
        let (read_status, read_body) = call(
            &router,
            "GetIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: the refused create must have stored nothing, got {read_status} with \
             {read_body}"
        );
        assert!(
            error_code(&read_body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException from the read, got {}",
            error_code(&read_body)
        );
        assert_eq!(
            error_message(&read_body),
            format!("The target resource {resource_arn} is not found."),
            "case {case:?}: expected the record miss to name the resource it was asked for, got \
             {:?}",
            error_message(&read_body)
        );
    }

    // The requirement is on the REQUEST, not on what the store already holds: a create
    // stating neither side is refused for a resource that already has a record too, and
    // leaves that record exactly as it was.
    create_connection(&router, "already-recorded").await;
    let resource_arn = connection_arn("already-recorded");
    let stored_role = format!("arn:aws:iam::{ACCOUNT_ID}:role/glue-stored");
    let created = create_property(
        &router,
        &json!({
            "ResourceArn": resource_arn,
            "SourceProcessingProperties": { "RoleArn": &stored_role },
        }),
    )
    .await;
    let record_arn = property_arn("a resource that already has a record", &created);

    let (status, body) = call(
        &router,
        "CreateIntegrationResourceProperty",
        &json!({ "ResourceArn": resource_arn }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {:?}: a create stating neither side is refused, got {status} with {body}",
        "a resource that already has a record"
    );
    assert_eq!(
        error_message(&body),
        NEITHER_SIDE_MESSAGE,
        "case {:?}: expected the fixed refusal sentence, got {:?}",
        "a resource that already has a record",
        error_message(&body)
    );
    let read = get_property(&router, &resource_arn).await;
    assert_echoed(
        "a resource that already has a record",
        &json!({ "SourceProcessingProperties": { "RoleArn": &stored_role } }),
        &read,
    );
    assert_eq!(
        property_arn("a resource that already has a record", &read),
        record_arn,
        "case {:?}: the record keeps the ResourcePropertyArn the first create reported, expected \
         {record_arn}, got {read}",
        "a resource that already has a record"
    );
}

/// A `ResourceArn` that cannot name a Glue connection or a Glue database is a bad
/// parameter: the write is refused with the modelled `InvalidInputException` and stores
/// nothing, and a read of the same value is refused the same way. The rows differ along how
/// the value misses: no ARN at all, another service's namespace, a Glue ARN of another
/// resource kind, and a Glue ARN carrying no resource name.
#[tokio::test]
async fn a_resource_arn_naming_no_connection_or_database_is_refused() {
    let router = emulator();

    for (case, resource_arn) in [
        ("not an ARN at all", "my-connection".to_string()),
        (
            "another service's namespace",
            "arn:aws:s3:::bucket/key".to_string(),
        ),
        (
            "a Glue crawler ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:crawler/nightly"),
        ),
        (
            "a Glue job ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:job/etl"),
        ),
        (
            "a Glue table ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:table/sales/orders"),
        ),
        (
            "the Data Catalog's own ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:catalog"),
        ),
        (
            "a connection ARN carrying no name",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:connection/"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "CreateIntegrationResourceProperty",
            &json!({
                "ResourceArn": resource_arn,
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/r" },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {resource_arn:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException for {resource_arn:?}, got {body}"
        );
        assert!(
            error_message(&body).contains("ResourceArn"),
            "case {case}: the refusal must name the member it read, got {body}"
        );

        let (status, body) = call(
            &router,
            "GetIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a read of {resource_arn:?} must be refused too, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException from the read, got {body}"
        );
    }
}

/// The properties are the named resource's own, so a `ResourceArn` naming a connection or
/// database this account does not hold earns the modelled `EntityNotFoundException`, and
/// nothing is stored for it. The rows differ along how the resource misses: a connection
/// never created, a database never created, a stored name in another account, and a stored
/// name in the other resource's kind.
#[tokio::test]
async fn a_resource_arn_naming_an_absent_resource_is_refused() {
    let router = emulator();
    create_connection(&router, "stored-connection").await;
    create_database(&router, "stored-database").await;

    for (case, resource_arn) in [
        ("a connection never created", connection_arn("absent")),
        ("a database never created", database_arn("absent")),
        (
            "a stored connection's name in another account",
            format!("arn:aws:glue:{REGION}:111122223333:connection/stored-connection"),
        ),
        (
            "a stored connection's name as a database",
            database_arn("stored-connection"),
        ),
        (
            "a stored database's name as a connection",
            connection_arn("stored-database"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "CreateIntegrationResourceProperty",
            &json!({
                "ResourceArn": resource_arn,
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/r" },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {resource_arn:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {resource_arn:?}, got {body}"
        );

        // A refused create stores nothing, so the read misses too.
        let (status, body) = call(
            &router,
            "GetIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: the refused create must have stored nothing, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException from the read, got {body}"
        );
    }

    // The resources that DO exist still take a record, so the misses above are misses and
    // not a broken resolution.
    for (case, resource_arn) in [
        ("the stored connection", connection_arn("stored-connection")),
        ("the stored database", database_arn("stored-database")),
    ] {
        let created = create_property(
            &router,
            &json!({
                "ResourceArn": resource_arn,
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/r" },
            }),
        )
        .await;
        assert_record_arn(case, &resource_arn, &property_arn(case, &created));
    }
}

/// A read for a resource this account holds no record for is refused with the modelled
/// `EntityNotFoundException` — the resource existing is not the record existing. The rows
/// differ along the resource kind, and the resource that DOES have a record still reads
/// back.
#[tokio::test]
async fn reading_a_resource_with_no_record_is_refused() {
    let router = emulator();
    create_connection(&router, "with-record").await;
    create_connection(&router, "without-record").await;
    create_database(&router, "database-without-record").await;
    create_property(
        &router,
        &json!({
            "ResourceArn": connection_arn("with-record"),
            "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/kept" },
        }),
    )
    .await;

    for (case, resource_arn) in [
        (
            "a connection with no record",
            connection_arn("without-record"),
        ),
        (
            "a database with no record",
            database_arn("database-without-record"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GetIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {resource_arn:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {resource_arn:?}, got {body}"
        );
    }

    let read = get_property(&router, &connection_arn("with-record")).await;
    assert_eq!(
        read.get("SourceProcessingProperties")
            .and_then(|side| side.get("RoleArn"))
            .and_then(Value::as_str),
        Some("arn:aws:iam::000000000000:role/kept"),
        "case the resource with a record: expected the stored role, got {read}"
    );
}

/// The store holds a bounded number of records: the create at the ceiling succeeds, the one
/// past it is refused with the closest client error this operation models, a create for a
/// resource that already HAS a record still succeeds at the ceiling (the bound is on
/// records, not on writes), and freeing a slot lets a new resource take one again.
#[tokio::test]
async fn a_create_past_the_integration_resource_property_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_INTEGRATION_RESOURCE_PROPERTIES - 1 {
            let resource_arn = connection_arn(&format!("seeded-{index}"));
            let input = CreateIntegrationResourcePropertyInput {
                resource_arn: resource_arn.clone(),
                ..CreateIntegrationResourcePropertyInput::default()
            };
            store.integration_resource_properties.insert(
                resource_arn,
                IntegrationResourcePropertyEntry::from_create_input(&input),
            );
        }
    }

    create_connection(&router, "last-resource").await;
    create_connection(&router, "one-too-many").await;
    let last = json!({
        "ResourceArn": connection_arn("last-resource"),
        "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/last" },
    });
    let past = json!({
        "ResourceArn": connection_arn("one-too-many"),
        "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/past" },
    });

    create_property(&router, &last).await;
    let (status, body) = call(&router, "CreateIntegrationResourceProperty", &past).await;
    assert_eq!(
        status, 400,
        "case record {} of {MAX_INTEGRATION_RESOURCE_PROPERTIES}: expected 400, got {status} \
         with {body}",
        MAX_INTEGRATION_RESOURCE_PROPERTIES + 1
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case one record too many: expected InvalidInputException, got {body}"
    );

    // A write to a record the store already holds adds nothing, so it is served at the
    // ceiling.
    let again = create_property(
        &router,
        &json!({
            "ResourceArn": connection_arn("last-resource"),
            "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/again" },
        }),
    )
    .await;
    assert_eq!(
        again
            .get("SourceProcessingProperties")
            .and_then(|side| side.get("RoleArn"))
            .and_then(Value::as_str),
        Some("arn:aws:iam::000000000000:role/again"),
        "case a write at the ceiling: expected the role the write stated, got {again}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state
            .lock()
            .integration_resource_properties
            .remove(&connection_arn("seeded-0"));
    }
    create_property(&router, &past).await;
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// `DeleteIntegrationResourceProperty` removes the record of the resource its `ResourceArn`
/// names and nothing else: the read of that resource then misses, a second delete is that
/// same miss, every other resource's record still reads back, and a create after the delete
/// stores the record afresh under the ARN its resource derives. A `ResourceArn` that
/// addresses neither leg of an integration is the bad parameter the create and the read
/// answer for it.
///
/// The rows differ along which resource kind the record belongs to and which side of the
/// integration its properties state.
#[tokio::test]
async fn deleting_an_integration_resource_property_removes_only_the_record_it_names() {
    let router = emulator();

    // (case, the ARN of the resource, the side the create states)
    let mut rows: Vec<(&str, String, Value)> = Vec::new();
    create_connection(&router, "deleted-connection").await;
    rows.push((
        "a connection's source properties",
        connection_arn("deleted-connection"),
        json!({ "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/s" } }),
    ));
    create_database(&router, "deleted_database").await;
    rows.push((
        "a database's target properties",
        database_arn("deleted_database"),
        json!({ "TargetProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/t" } }),
    ));

    // A bystander, to prove each delete is scoped to the record it names.
    create_connection(&router, "kept-connection").await;
    let bystander = connection_arn("kept-connection");
    create_property(
        &router,
        &json!({
            "ResourceArn": &bystander,
            "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/kept" },
        }),
    )
    .await;

    for (case, resource_arn, properties) in &rows {
        let mut request = properties.clone();
        request["ResourceArn"] = json!(resource_arn);
        let created = create_property(&router, &request).await;
        let record_arn = property_arn(case, &created);
        assert_record_arn(case, resource_arn, &record_arn);

        ok(
            &router,
            "DeleteIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;

        // The record is gone: the read of that resource misses, which is what makes the
        // delete observable.
        for (attempt, operation) in [
            ("the read after the delete", "GetIntegrationResourceProperty"),
            ("a second delete", "DeleteIntegrationResourceProperty"),
        ] {
            let (status, body) =
                call(&router, operation, &json!({ "ResourceArn": resource_arn })).await;
            assert_eq!(
                status, 400,
                "case {case}: {attempt} must be refused, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("EntityNotFoundException"),
                "case {case}: {attempt} expected EntityNotFoundException, got {body}"
            );
        }

        // A create after the delete stores the record afresh, under the ARN the resource
        // derives — the identity is the resource's, not the write's.
        let reborn = create_property(&router, &request).await;
        assert_eq!(
            property_arn(case, &reborn),
            record_arn,
            "case {case}: the re-created record must carry the same ARN, got {reborn}"
        );
    }

    // A value that addresses neither leg of an integration is a bad parameter, as it is for
    // the create and the read.
    for (case, resource_arn) in [
        (
            "a catalog table's ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:table/deleted_database/orders"),
        ),
        ("a value that is no ARN at all", "deleted-connection".to_string()),
    ] {
        let (status, body) = call(
            &router,
            "DeleteIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {resource_arn:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException for {resource_arn:?}, got {body}"
        );
    }

    // Nothing any delete did reached the record it did not name.
    let kept = get_property(&router, &bystander).await;
    assert_eq!(
        kept.get("SourceProcessingProperties")
            .and_then(|side| side.get("RoleArn"))
            .and_then(Value::as_str),
        Some("arn:aws:iam::000000000000:role/kept"),
        "case the bystander: a delete must leave every other record in place, got {kept}"
    );
}
