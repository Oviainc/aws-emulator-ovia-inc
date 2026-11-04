// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the identities a Glue create REPORTS: the `Id` of a registered
//! asset type and the `ResourcePropertyArn` of an integration resource property record.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: an identity a create reports is how a client ADDRESSES the
//! resource afterwards, so it is derived from what the request named — the asset type's
//! `Name`, the described resource's own ARN — and not drawn per call. A caller that kept
//! only the request it sent can therefore reconstruct it, and two reads of one resource
//! report the same identity.
//!
//! The rows differ along what the rule generalises over: the form and length of the name,
//! which resource kind the described ARN names, and whether the create is the first for
//! that resource or a later one.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{emulator, ok, ACCOUNT_ID, REGION};

/// The longest `Name` an asset type accepts: "len 1..128" on `AssetTypeName`
/// (`smithy_query operation=PutAssetType`), whose pattern
/// `^(?![0-9_])\w+$|^_\w*[a-zA-Z0-9]\w*$` is what the rows below keep to.
const MAX_ASSET_TYPE_NAME_LENGTH: usize = 128;

/// Register a form type, so an asset type's form entries reference a form type this
/// account really holds, and answer the identifier the catalog assigned it.
async fn form_type(router: &MultiServiceRouter, name: &str) -> String {
    let stored = ok(
        router,
        "PutFormType",
        &json!({ "Name": name, "Schema": format!("structure {name} {{ a: String }}") }),
    )
    .await;
    stored
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("PutFormType must report an Id, got {stored}"))
        .to_string()
}

/// A `Forms` map in the modelled `AssetTypeFormReference` shape.
fn forms(entries: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (form_name, form_type_id) in entries {
        map.insert(
            (*form_name).to_string(),
            json!({ "FormTypeIdentifier": form_type_id }),
        );
    }
    Value::Object(map)
}

/// The `Id` of a response that reports an asset type.
fn asset_type_id(case: &str, body: &Value) -> String {
    body.get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: the response must carry an Id, got {body}"))
        .to_string()
}

/// The `ResourcePropertyArn` of a response that reports an integration resource property
/// record.
fn property_arn(case: &str, body: &Value) -> String {
    body.get("ResourcePropertyArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: the response must carry a ResourcePropertyArn, got {body}")
        })
        .to_string()
}

// ---------------------------------------------------------------------------
// The identifier of an asset type
// ---------------------------------------------------------------------------

/// `PutAssetType` reports the asset type's `Name` as its `Id` — the identifier every
/// operation that addresses an asset type carries — so the identity is the one the caller
/// submitted: a read of it reports the same asset type, a second put under the name reports
/// the same identifier, and two names are two identities.
#[tokio::test]
async fn an_asset_type_is_addressed_by_the_name_it_was_registered_under() {
    let router = emulator();
    let order_form = form_type(&router, "OrderForm").await;

    let mut longest = String::from("A");
    longest.push_str(&"x".repeat(MAX_ASSET_TYPE_NAME_LENGTH - 1));

    // (case, the asset type's name)
    let names: Vec<(&str, String)> = vec![
        ("the shortest name the model admits", "A".to_string()),
        ("a plain name", "OrderAsset".to_string()),
        ("a name carrying digits", "OrderAsset2024".to_string()),
        ("a name carrying an underscore", "order_asset".to_string()),
        ("a name opening with an underscore", "_orderAsset1".to_string()),
        ("an all-lowercase name", "orderasset".to_string()),
        ("the longest name the model admits", longest),
    ];

    let mut identities: Vec<String> = Vec::new();
    for (case, name) in &names {
        let created = ok(
            &router,
            "PutAssetType",
            &json!({ "Name": name, "Forms": forms(&[("Details", &order_form)]) }),
        )
        .await;
        let id = asset_type_id(case, &created);
        assert_eq!(
            &id, name,
            "case {case}: the Id a put reports is the name it registered; expected {name:?}, \
             got {id:?}"
        );
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case}: the put must report the name it stored, got {created}"
        );

        // The identity is usable: the read of that identifier serves the record the put
        // stored, which is what makes the Id an address rather than a label.
        let read = ok(&router, "GetAssetType", &json!({ "Identifier": id })).await;
        assert_eq!(
            read.get("Id").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case}: the read must report the same identifier, got {read}"
        );
        assert_eq!(
            read.get("Forms"),
            created.get("Forms"),
            "case {case}: the read must report the forms the put stored, got {read}"
        );

        // A second put under the same name is an update, and the identity is the name, so
        // it is unchanged by it.
        let updated = ok(
            &router,
            "PutAssetType",
            &json!({
                "Name": name,
                "Forms": forms(&[("Details", &order_form), ("Audit", &order_form)]),
            }),
        )
        .await;
        assert_eq!(
            asset_type_id(case, &updated),
            id,
            "case {case}: an update must report the same identifier, got {updated}"
        );

        identities.push(id);
    }

    let distinct: std::collections::BTreeSet<&String> = identities.iter().collect();
    assert_eq!(
        distinct.len(),
        identities.len(),
        "case every asset type: two names are two identities, got {identities:?}"
    );
}

// ---------------------------------------------------------------------------
// The ARN of an integration resource property record
// ---------------------------------------------------------------------------

/// `CreateIntegrationResourceProperty` reports a `ResourcePropertyArn` built from the
/// resource the record describes — the resource type and name the request's `ResourceArn`
/// carried, under this record type's own ARN segment — so a caller that holds only the ARN
/// it submitted derives the record's, every read of the record reports the same one, and
/// two resources never share one.
#[tokio::test]
async fn an_integration_resource_property_arn_is_derived_from_the_resource_it_describes() {
    let router = emulator();

    // (case, the resource kind's ARN segment, the name the request's ARN carries, the name
    // the record's ARN must carry, the side the create states). The two names differ for a
    // Data Catalog database, whose name the catalog folds to lowercase: the record names
    // the resource as the catalog keys it.
    let rows: Vec<(&str, &str, &str, &str, Value)> = vec![
        (
            "a database target",
            "database",
            "integration_target_db",
            "integration_target_db",
            json!({ "TargetProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/t" } }),
        ),
        (
            "a connection source",
            "connection",
            "integration-source-connection",
            "integration-source-connection",
            json!({ "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/s" } }),
        ),
        (
            "a database whose name is one character",
            "database",
            "d",
            "d",
            json!({ "TargetProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/t" } }),
        ),
        (
            "a database named in mixed case",
            "database",
            "Mixed_Case_Target_DB",
            "mixed_case_target_db",
            json!({ "TargetProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/t" } }),
        ),
        (
            "a connection whose name carries dots and digits",
            "connection",
            "etl.connection.2024",
            "etl.connection.2024",
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/s" },
                "Tags": [ { "Key": "env", "Value": "dev" } ],
            }),
        ),
    ];

    let mut arns: Vec<String> = Vec::new();
    for (case, segment, name, derived_name, properties) in &rows {
        match *segment {
            "database" => {
                ok(
                    &router,
                    "CreateDatabase",
                    &json!({ "DatabaseInput": { "Name": name } }),
                )
                .await;
            }
            "connection" => {
                ok(
                    &router,
                    "CreateConnection",
                    &json!({
                        "ConnectionInput": {
                            "Name": name,
                            "ConnectionType": "JDBC",
                            "ConnectionProperties": {
                                "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/w",
                                "USERNAME": "etl",
                            },
                        },
                    }),
                )
                .await;
            }
            other => panic!("case {case}: no row describes a {other}"),
        }

        let resource_arn = format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:{segment}/{name}");
        let mut request = properties.clone();
        request["ResourceArn"] = json!(resource_arn);
        let created = ok(&router, "CreateIntegrationResourceProperty", &request).await;

        // The one expectation, written from the request: the record's ARN names the
        // resource the create addressed, as the catalog keys it.
        let expected = format!(
            "arn:aws:glue:{REGION}:{ACCOUNT_ID}:integrationresourceproperty/\
             {segment}/{derived_name}"
        );
        let got = property_arn(case, &created);
        assert_eq!(
            got, expected,
            "case {case}: the record ARN must be derived from {resource_arn}; expected \
             {expected:?}, got {got:?}"
        );

        // The read of the record reports the same ARN, so the identity is stored and not
        // drawn per call.
        let read = ok(
            &router,
            "GetIntegrationResourceProperty",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await;
        assert_eq!(
            property_arn(case, &read),
            expected,
            "case {case}: the read must report the same record ARN, got {read}"
        );

        // A second create for the same resource writes that record, under the ARN its
        // resource derives.
        let second = ok(&router, "CreateIntegrationResourceProperty", &request).await;
        assert_eq!(
            property_arn(case, &second),
            expected,
            "case {case}: a second create must report the same record ARN, got {second}"
        );

        arns.push(expected);
    }

    let distinct: std::collections::BTreeSet<&String> = arns.iter().collect();
    assert_eq!(
        distinct.len(),
        arns.len(),
        "case every record: two resources must not share a ResourcePropertyArn, got {arns:?}"
    );
}
