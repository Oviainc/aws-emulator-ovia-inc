// SPDX-License-Identifier: CC0-1.0

//! Where a zero-ETL integration's `SourceArn` is RESOLVED, and what a source that
//! exists nowhere earns.
//!
//! The rule the tables assert: **`CreateIntegration`'s `SourceArn` names a
//! resource of ANOTHER service — the modelled member admits an Amazon DynamoDB
//! and an Amazon S3 ARN alike (`smithy_query(query="operation",
//! operation="CreateIntegration")` -> SourceArn @pattern
//! `^arn:aws[a-z\-]*:(s3|dynamodb):.*:.*:[a-zA-Z0-9._\-/]+$`) — so whether there
//! is such a table or bucket is asked of the service that OWNS it, before
//! anything about the target is judged, and a source that service answers it
//! does not hold is refused with HTTP 400 `InvalidParameterValue` and the
//! zero-ETL source-access sentence. A source whose owner cannot be asked at all
//! leaves the question undecided: nothing is invented from silence, so the create
//! is admitted to the target decision instead.**
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, with Amazon DynamoDB, Amazon S3 and the Redshift
//! Serverless control plane mounted beside Redshift on one engine exactly as the
//! assembled emulator mounts them — because the table or bucket a `SourceArn`
//! names, and the namespace a `TargetArn` names, are THEIR resources, created
//! here through their own APIs.
//!
//! The rows differ along what the rule generalises over: which service owns the
//! source, whether that service holds the resource, the form the resource is
//! named in, and whether the owning service is reachable at all. What a row
//! asserts is the SOURCE decision, so every row names a target that exists
//! nowhere: a row whose source passes the check is then refused for its target,
//! and that refusal is what shows the source was accepted.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator, REGION};

/// A namespace ARN that names no data warehouse of either control plane, so a
/// create whose SOURCE is accepted is refused for its target — which is how a row
/// shows the source passed.
fn absent_target() -> String {
    format!(
        "arn:aws:redshift-serverless:{REGION}:{}:namespace/00000000-0000-4000-8000-000000000000",
        engine::emulator_account_id()
    )
}

/// The DynamoDB table ARN for `name` in [`REGION`], as a caller writes it.
///
/// `pub(crate)` so the sibling suite about the ORDER the two ARNs are decided in
/// writes the same ARN form rather than a second copy of it.
pub(crate) fn table_arn(name: &str) -> String {
    format!(
        "arn:aws:dynamodb:{REGION}:{}:table/{name}",
        engine::emulator_account_id()
    )
}

/// `CreateIntegration` with just the three required members.
pub(crate) async fn create_integration(
    emulator: &Emulator,
    name: &str,
    source: &str,
    target: &str,
) -> Answer {
    emulator
        .call(
            "CreateIntegration",
            &[
                ("IntegrationName", name),
                ("SourceArn", source),
                ("TargetArn", target),
            ],
        )
        .await
}

/// Create the DynamoDB table `name` through the service that owns it.
pub(crate) async fn create_table(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = emulator
        .call_dynamodb(
            "CreateTable",
            &serde_json::json!({
                "TableName": name,
                "BillingMode": "PAY_PER_REQUEST",
                "AttributeDefinitions": [{"AttributeName": "id", "AttributeType": "S"}],
                "KeySchema": [{"AttributeName": "id", "KeyType": "HASH"}],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTable {name} expected 200, got {status} with {body}"
    );
}

/// Create the bucket `name` through Amazon S3's own `CreateBucket`, in [`REGION`]
/// — a regional endpoint requires the create to state its own location
/// constraint.
pub(crate) async fn create_bucket(emulator: &Emulator, case: &str, name: &str) {
    emulator
        .s3()
        .create_bucket()
        .bucket(name)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::from(REGION))
                .build(),
        )
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the emulated Amazon S3 creates the bucket {name}: {error}")
        });
}

/// What the source decision is expected to be for one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The owning service holds no such resource: 400 `InvalidParameterValue`
    /// with that source kind's access sentence, and nothing recorded.
    Refused(state::zero_etl_source::SourceKind),
    /// The source passed the check, so the create is refused for its target
    /// instead — 404 `IntegrationTargetNotFoundFault`.
    Admitted,
}

/// One row of the source table.
struct Row {
    case: &'static str,
    /// The name the create is made under, unique per row so no row's answer can
    /// come from another row's record.
    name: String,
    source: String,
    expected: Expected,
}

/// Assert one row, and that a refused create recorded nothing.
async fn assert_row(emulator: &Emulator, row: &Row) {
    let answer = create_integration(emulator, &row.name, &row.source, &absent_target()).await;
    match row.expected {
        Expected::Refused(kind) => {
            assert_eq!(
                (answer.status, answer.error_code().as_deref()),
                (400, Some("InvalidParameterValue")),
                "case {}: expected 400 InvalidParameterValue for the source {}, got {} / {:?} \
                 with body {}",
                row.case,
                row.source,
                answer.status,
                answer.error_code(),
                answer.body
            );
            assert_eq!(
                answer.text("Message").as_deref(),
                Some(kind.inaccessible_message()),
                "case {}: expected the source-access sentence, got {:?}",
                row.case,
                answer.text("Message")
            );
        }
        Expected::Admitted => {
            assert_eq!(
                (answer.status, answer.error_code().as_deref()),
                (404, Some("IntegrationTargetNotFoundFault")),
                "case {}: a source the owner did not deny must reach the target decision; \
                 expected 404 IntegrationTargetNotFoundFault, got {} / {:?} with body {}",
                row.case,
                answer.status,
                answer.error_code(),
                answer.body
            );
        }
    }

    // Whatever the answer, a refused create records nothing: no integration of
    // this row's name is readable afterwards.
    let listed = emulator.call("DescribeIntegrations", &[]).await;
    assert!(
        !crate::cluster_lifecycle::all_text(&listed.body, "IntegrationName").contains(&row.name),
        "case {}: a refused create must record no integration, got body {}",
        row.case,
        listed.body
    );
}

#[tokio::test]
async fn a_source_its_owner_does_not_hold_is_refused_before_the_target_is_judged() {
    let emulator = Emulator::with_integration_owners();
    // The one table and the one bucket that really exist, each created through
    // the service that owns it.
    create_table(&emulator, "seed the real source table", "zetl-orders").await;
    create_bucket(&emulator, "seed the real source bucket", "zetl-landing-zone").await;

    let rows = vec![
        Row {
            case: "a DynamoDB table the owner holds",
            name: "zetl-src-real".to_string(),
            source: table_arn("zetl-orders"),
            expected: Expected::Admitted,
        },
        Row {
            case: "a DynamoDB table that was never created",
            name: "zetl-src-absent".to_string(),
            source: table_arn("zetl-never-created"),
            expected: Expected::Refused(state::zero_etl_source::SourceKind::DynamoDbTable),
        },
        Row {
            case: "a DynamoDB table name of another shape, also never created",
            name: "zetl-src-absent-other".to_string(),
            source: table_arn("Zetl.Table_9-X"),
            expected: Expected::Refused(state::zero_etl_source::SourceKind::DynamoDbTable),
        },
        Row {
            case: "a DynamoDB ARN naming a resource that is not a table",
            name: "zetl-src-stream".to_string(),
            source: format!(
                "arn:aws:dynamodb:{REGION}:{}:table/zetl-orders/stream/2026-01-01T00.00.00.000",
                engine::emulator_account_id()
            ),
            expected: Expected::Admitted,
        },
        Row {
            case: "an Amazon S3 bucket the owner holds",
            name: "zetl-src-bucket".to_string(),
            source: "arn:aws:s3:::zetl-landing-zone".to_string(),
            expected: Expected::Admitted,
        },
        Row {
            case: "an Amazon S3 bucket named with a key prefix, the bucket held",
            name: "zetl-src-bucket-prefix".to_string(),
            source: "arn:aws:s3:::zetl-landing-zone/orders/2026/".to_string(),
            expected: Expected::Admitted,
        },
        Row {
            case: "an Amazon S3 bucket that was never created",
            name: "zetl-src-bucket-absent".to_string(),
            source: "arn:aws:s3:::zetl-never-created".to_string(),
            expected: Expected::Refused(state::zero_etl_source::SourceKind::S3Bucket),
        },
        Row {
            // The modelled pattern leaves the Region field as `.*`, so a value
            // that is no Region label at all reaches the handler; it names the
            // Region of no service, so the source is left undecided rather than
            // asked of an address the request composed.
            case: "a source ARN whose Region field is not a Region label",
            name: "zetl-src-bad-region".to_string(),
            source: format!(
                "arn:aws:dynamodb:us-west-2/elsewhere:{}:table/zetl-never-created",
                engine::emulator_account_id()
            ),
            expected: Expected::Admitted,
        },
    ];

    for row in &rows {
        assert_row(&emulator, row).await;
    }
}

#[tokio::test]
async fn a_source_whose_owner_cannot_be_asked_is_not_called_absent() {
    // Redshift with the TARGET's owner but with neither SOURCE owner: nothing can
    // answer whether the table or the bucket exists — which is this emulator
    // failing to resolve a fact about itself, not the caller naming something
    // that is not there.
    let emulator = Emulator::with_serverless();
    let rows = vec![
        Row {
            case: "a DynamoDB source with no DynamoDB mounted",
            name: "zetl-src-unresolvable-table".to_string(),
            source: table_arn("zetl-never-created"),
            expected: Expected::Admitted,
        },
        Row {
            case: "an Amazon S3 source with no object store mounted",
            name: "zetl-src-unresolvable-bucket".to_string(),
            source: "arn:aws:s3:::zetl-never-created".to_string(),
            expected: Expected::Admitted,
        },
    ];
    for row in &rows {
        assert_row(&emulator, row).await;
    }
}
