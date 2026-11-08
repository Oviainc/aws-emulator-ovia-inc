// SPDX-License-Identifier: CC0-1.0

//! What Personalize answers when a create collides with a resource that is already there.
//!
//! The class is worded by the ARN of the resource the create collided WITH, and by nothing
//! else: the sentence names no resource kind, no requested name and no container, so one
//! wording serves every creating operation this service implements. The recorded case — a
//! second `CreateDatasetGroup` under a name already taken — is one row among them.
//!
//! The rows vary what the rule generalises over: the KIND of resource being created
//! (schema, dataset group, dataset), the shape of the name its ARN is minted from (a plain
//! name, every permitted character class, the longest the model allows), and whether the
//! colliding ARN is the one the request's own name would mint (a schema's and a group's) or
//! the one the EXISTING resource carries (a dataset's, addressed by group and type, so a
//! clash under a different name still names the resource already there).

use serde_json::json;

use crate::dataset_group_lifecycle::{
    avro_schema, create_active_group, create_dataset, create_schema, error_code, error_message,
    Emulator, ACCOUNT, REGION,
};

/// The sentence Personalize answers a colliding create with. The colliding resource's ARN
/// is its one variable part.
///
/// Written out here rather than read from `crate::state`: a test that asserted the
/// handler's own helper would pass whatever that helper said.
fn resource_already_exists_message(arn: &str) -> String {
    format!("Another resource with Arn {arn} already exists.")
}

/// The longest `name` the model allows: `Name` is `len 1..63`
/// (`smithy_query(query="shape", shape="Name")`).
const LONGEST_NAME: usize = 63;

/// One row of the table: the colliding request, and the ARN of the resource already there.
struct Row {
    case: String,
    operation: &'static str,
    body: serde_json::Value,
    colliding_arn: String,
}

#[tokio::test]
async fn a_colliding_create_names_the_arn_of_the_resource_already_there() {
    let emulator = Emulator::new();
    let setup = "already-exists setup";

    // Names spanning what the rule generalises over: a plain one, one of every character
    // class the model's `^[a-zA-Z0-9][a-zA-Z0-9\-_]*$` pattern permits, and the longest the
    // length constraint allows.
    let names: &[(&str, String)] = &[
        ("a plain name", "collides".to_string()),
        (
            "dashes, digits and underscores",
            "Collides-9_again".to_string(),
        ),
        ("the longest name", "c".repeat(LONGEST_NAME)),
    ];

    let mut rows: Vec<Row> = Vec::new();

    for (case, name) in names {
        // Each resource is created once, so the SECOND create below is the collision.
        let schema_arn = create_schema(&emulator, name, &avro_schema("Interactions"), setup).await;
        let group_arn = create_active_group(&emulator, name, setup).await;
        let dataset_arn =
            create_dataset(&emulator, &group_arn, &schema_arn, "Interactions", setup).await;

        rows.push(Row {
            case: format!("CreateSchema with {case} already taken"),
            operation: "CreateSchema",
            body: json!({"name": name, "schema": avro_schema("Interactions")}),
            colliding_arn: schema_arn.clone(),
        });
        // A different document under the same name is still a collision: the ARN is minted
        // from the name alone.
        rows.push(Row {
            case: format!("CreateSchema with {case} already taken, another document"),
            operation: "CreateSchema",
            body: json!({"name": name, "schema": avro_schema("Items")}),
            colliding_arn: schema_arn.clone(),
        });
        rows.push(Row {
            case: format!("CreateDatasetGroup with {case} already taken"),
            operation: "CreateDatasetGroup",
            body: json!({"name": name}),
            colliding_arn: group_arn.clone(),
        });
        // A dataset's identity inside its group is its TYPE, so a second Interactions
        // dataset collides under ANY name — and the sentence carries the ARN of the dataset
        // already there rather than anything from this request.
        rows.push(Row {
            case: format!("a second Interactions dataset in the group named {case}"),
            operation: "CreateDataset",
            body: json!({
                "name": "a-completely-different-name",
                "datasetType": "Interactions",
                "schemaArn": schema_arn,
                "datasetGroupArn": group_arn,
            }),
            colliding_arn: dataset_arn.clone(),
        });
        // The documented matching of `datasetType` is case insensitive, so a differently
        // cased type is the same dataset and the same collision.
        rows.push(Row {
            case: format!("a second INTERACTIONS dataset in the group named {case}"),
            operation: "CreateDataset",
            body: json!({
                "name": "another-name-again",
                "datasetType": "INTERACTIONS",
                "schemaArn": schema_arn,
                "datasetGroupArn": group_arn,
            }),
            colliding_arn: dataset_arn,
        });
    }

    for row in &rows {
        let Row {
            case,
            operation,
            body,
            colliding_arn,
        } = row;
        let expected = resource_already_exists_message(colliding_arn);
        let (status, answered) = emulator.call(operation, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to answer HTTP 400, got {status} {answered:?}"
        );
        let code = error_code(&answered);
        assert!(
            code.contains("ResourceAlreadyExistsException"),
            "case {case:?}: expected ResourceAlreadyExistsException, got {code:?} in \
             {answered:?}"
        );
        let message = error_message(&answered);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // Every refused create left the resource it collided with exactly where it was, so the
    // wording above belongs to the refusal and not to a create that half-succeeded.
    for row in &rows {
        let (operation, member) = match row.operation {
            "CreateSchema" => ("DescribeSchema", "schemaArn"),
            "CreateDatasetGroup" => ("DescribeDatasetGroup", "datasetGroupArn"),
            _ => ("DescribeDataset", "datasetArn"),
        };
        let (status, body) = emulator
            .call_with_arn(operation, member, &row.colliding_arn)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to still answer 200 for {arn}, got {status} \
             {body:?}",
            case = row.case,
            arn = row.colliding_arn
        );
    }

    // A name nothing holds is still free, so the collision is about the stored resource and
    // not about creating at all.
    let free = create_schema(
        &emulator,
        "never-taken",
        &avro_schema("Users"),
        "a free name",
    )
    .await;
    assert_eq!(
        free,
        format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/never-taken"),
        "case \"a free name\": expected the minted schema ARN, got {free}"
    );
}
