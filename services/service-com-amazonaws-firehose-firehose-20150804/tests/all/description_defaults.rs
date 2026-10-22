// SPDX-License-Identifier: CC0-1.0

//! What `DescribeDeliveryStream` reports for the members a create left unset.
//!
//! The rule under test: a `DeliveryStreamDescription` member whose default the service
//! documents is reported with that default materialised, never omitted — so a stream's
//! server-side-encryption state is always readable (`DISABLED` for a stream that was
//! never encrypted), and a destination description carries the `S3BackupMode` its own
//! documentation gives that destination kind.
//!
//! Every request goes through the real router, so what is asserted is the wire body a
//! client sees. The harness (`Emulator`, the ARNs, `at`) is the one
//! `delivery_stream_lifecycle` already builds.

use serde_json::{json, Value};
// The modelled `DeliveryStreamEncryptionStatus` catalog, generated from the service's own
// Smithy model — reused rather than hand-copied, so this test cannot drift from it.
use service_com_amazonaws_firehose_firehose_20150804::ops::types::shared_types::DELIVERY_STREAM_ENCRYPTION_STATUS_VALUES;

use crate::delivery_stream_lifecycle::{
    at, s3_configuration, Emulator, BUCKET_ARN, PAST_THE_TRANSITION, REGION, ROLE_ARN,
};

/// One create, and the SSE state its stream must report before and after the enabling
/// span the clock owes it.
struct EncryptionCase {
    stream: &'static str,
    /// The `DeliveryStreamEncryptionConfigurationInput` the create carries, if any.
    encryption_input: Option<Value>,
    /// The destination the create configures: the SSE state is reported whatever it is.
    destination_member: &'static str,
    destination: Value,
    /// `Status` while the create is still in flight, and once the clock has caught up.
    status_while_creating: &'static str,
    status_when_active: &'static str,
    /// `KeyType` / `KeyARN` as the description must report them (`Value::Null` for a
    /// member that must be absent).
    key_type: Value,
    key_arn: Value,
}

fn encryption_cases() -> Vec<EncryptionCase> {
    let customer_key = format!("arn:aws:kms:{REGION}:000000000000:key/demo-key");
    vec![
        // The row the report cites: no encryption was ever asked for.
        EncryptionCase {
            stream: "sse-never-asked-for",
            encryption_input: None,
            destination_member: "S3DestinationConfiguration",
            destination: s3_configuration(),
            status_while_creating: "DISABLED",
            status_when_active: "DISABLED",
            key_type: Value::Null,
            key_arn: Value::Null,
        },
        // A service-account CMK: "If you set KeyType to AWS_OWNED_CMK, Firehose uses a
        // service-account CMK" — so no KeyARN is reported.
        EncryptionCase {
            stream: "sse-aws-owned-cmk",
            encryption_input: Some(json!({ "KeyType": "AWS_OWNED_CMK" })),
            destination_member: "ExtendedS3DestinationConfiguration",
            destination: json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN }),
            status_while_creating: "ENABLING",
            status_when_active: "ENABLED",
            key_type: json!("AWS_OWNED_CMK"),
            key_arn: Value::Null,
        },
        EncryptionCase {
            stream: "sse-customer-managed-cmk",
            encryption_input: Some(json!({
                "KeyType": "CUSTOMER_MANAGED_CMK",
                "KeyARN": customer_key,
            })),
            destination_member: "SplunkDestinationConfiguration",
            destination: json!({
                "HECEndpoint": "https://splunk.example.com:8088",
                "HECEndpointType": "Event",
                "HECToken": "hec-token",
                "S3Configuration": s3_configuration(),
            }),
            status_while_creating: "ENABLING",
            status_when_active: "ENABLED",
            key_type: json!("CUSTOMER_MANAGED_CMK"),
            key_arn: json!(customer_key),
        },
    ]
}

/// The SSE block a describe reports for `stream`.
async fn encryption_of(emulator: &Emulator, stream: &str) -> Value {
    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": stream }),
        )
        .await;
    assert_eq!(status, 200, "case {stream:?}: describe got {described:?}");
    at(
        &described,
        "DeliveryStreamDescription/DeliveryStreamEncryptionConfiguration",
    )
    .clone()
}

#[tokio::test]
async fn every_stream_reports_its_sse_state_whatever_the_create_asked_for() {
    let emulator = Emulator::new();
    let cases = encryption_cases();

    // Create every stream first, so the describes below all land inside the enabling
    // span rather than racing it.
    for case in &cases {
        let mut request = json!({ "DeliveryStreamName": case.stream });
        request[case.destination_member] = case.destination.clone();
        if let Some(encryption_input) = &case.encryption_input {
            request["DeliveryStreamEncryptionConfigurationInput"] = encryption_input.clone();
        }
        let (status, created) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(
            status, 200,
            "case {:?}: create got {created:?}",
            case.stream
        );
    }

    for case in &cases {
        let encryption = encryption_of(&emulator, case.stream).await;
        assert!(
            encryption.is_object(),
            "case {:?}: the description always carries \
             DeliveryStreamEncryptionConfiguration, got {encryption:?}",
            case.stream
        );
        let reported = encryption["Status"].as_str().unwrap_or_default();
        assert!(
            DELIVERY_STREAM_ENCRYPTION_STATUS_VALUES.contains(&reported),
            "case {:?}: Status is a modelled DeliveryStreamEncryptionStatus, got \
             {encryption:?}",
            case.stream
        );
        assert_eq!(
            encryption["Status"],
            json!(case.status_while_creating),
            "case {:?}: describe while creating got {encryption:?}",
            case.stream
        );
        assert_eq!(
            encryption["KeyType"], case.key_type,
            "case {:?}: describe while creating got {encryption:?}",
            case.stream
        );
        assert_eq!(
            encryption["KeyARN"], case.key_arn,
            "case {:?}: describe while creating got {encryption:?}",
            case.stream
        );

        // Describing is reporting: a second read answers exactly the first.
        let again = encryption_of(&emulator, case.stream).await;
        assert_eq!(
            again, encryption,
            "case {:?}: two reads in a row report the same SSE state, got {again:?}",
            case.stream
        );
    }

    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for case in &cases {
        let encryption = encryption_of(&emulator, case.stream).await;
        assert_eq!(
            encryption["Status"],
            json!(case.status_when_active),
            "case {:?}: describe after the enabling span got {encryption:?}",
            case.stream
        );
        assert_eq!(
            encryption["KeyType"], case.key_type,
            "case {:?}: describe after the enabling span got {encryption:?}",
            case.stream
        );
        assert_eq!(
            encryption["KeyARN"], case.key_arn,
            "case {:?}: describe after the enabling span got {encryption:?}",
            case.stream
        );
    }
}

/// One destination configuration and the `S3BackupMode` its description must report.
struct BackupModeCase {
    stream: &'static str,
    config_member: &'static str,
    config: Value,
    description_member: &'static str,
    /// The value the description must carry, whether supplied or defaulted.
    s3_backup_mode: &'static str,
    /// Other members of the same description that must carry their documented default.
    also: Vec<(&'static str, Value)>,
}

/// The rows span: the destination kinds whose model documents an `S3BackupMode` default
/// (`ExtendedS3` / `Redshift` -> `Disabled`, `Elasticsearch` -> `FailedDocumentsOnly`,
/// `Splunk` -> `FailedEventsOnly`), each with the default taken and with an explicit value
/// given, plus a kind whose model documents no default, where a supplied value is still
/// reported as given. Every value is one of the enum's own, from
/// `smithy_query(query="shape",shape="S3BackupMode,RedshiftS3BackupMode,
/// ElasticsearchS3BackupMode,SplunkS3BackupMode,HttpEndpointS3BackupMode")`.
fn backup_mode_cases() -> Vec<BackupModeCase> {
    let redshift_url =
        "jdbc:redshift://example-cluster.abc123.us-west-2.redshift.amazonaws.com:5439/dev";
    vec![
        // The row the report cites.
        BackupModeCase {
            stream: "extended-s3-defaulted",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN }),
            description_member: "ExtendedS3DestinationDescription",
            s3_backup_mode: "Disabled",
            // The three members the description marks @required but the configuration
            // leaves optional are materialised by the same path.
            also: vec![
                ("CompressionFormat", json!("UNCOMPRESSED")),
                ("BufferingHints/SizeInMBs", json!(5)),
                ("BufferingHints/IntervalInSeconds", json!(300)),
                (
                    "EncryptionConfiguration/NoEncryptionConfig",
                    json!("NoEncryption"),
                ),
            ],
        },
        BackupModeCase {
            stream: "extended-s3-enabled",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "S3BackupMode": "Enabled",
                "S3BackupConfiguration": s3_configuration(),
            }),
            description_member: "ExtendedS3DestinationDescription",
            s3_backup_mode: "Enabled",
            also: vec![("S3BackupDescription/BucketARN", json!(BUCKET_ARN))],
        },
        BackupModeCase {
            stream: "redshift-defaulted",
            config_member: "RedshiftDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "ClusterJDBCURL": redshift_url,
                "CopyCommand": { "DataTableName": "events" },
                "Username": "analytics",
                "Password": "sup3r-secret-pw",
                "S3Configuration": s3_configuration(),
            }),
            description_member: "RedshiftDestinationDescription",
            s3_backup_mode: "Disabled",
            also: vec![],
        },
        BackupModeCase {
            stream: "redshift-enabled",
            config_member: "RedshiftDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "ClusterJDBCURL": redshift_url,
                "CopyCommand": { "DataTableName": "events" },
                "Username": "analytics",
                "Password": "sup3r-secret-pw",
                "S3Configuration": s3_configuration(),
                "S3BackupMode": "Enabled",
                "S3BackupConfiguration": s3_configuration(),
            }),
            description_member: "RedshiftDestinationDescription",
            s3_backup_mode: "Enabled",
            also: vec![("S3BackupDescription/BucketARN", json!(BUCKET_ARN))],
        },
        BackupModeCase {
            stream: "elasticsearch-defaulted",
            config_member: "ElasticsearchDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "DomainARN": "arn:aws:es:us-west-2:000000000000:domain/logs-domain",
                "IndexName": "logs",
                "S3Configuration": s3_configuration(),
            }),
            description_member: "ElasticsearchDestinationDescription",
            s3_backup_mode: "FailedDocumentsOnly",
            also: vec![],
        },
        BackupModeCase {
            stream: "elasticsearch-all-documents",
            config_member: "ElasticsearchDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "DomainARN": "arn:aws:es:us-west-2:000000000000:domain/logs-domain",
                "IndexName": "logs",
                "S3Configuration": s3_configuration(),
                "S3BackupMode": "AllDocuments",
            }),
            description_member: "ElasticsearchDestinationDescription",
            s3_backup_mode: "AllDocuments",
            also: vec![],
        },
        BackupModeCase {
            stream: "splunk-defaulted",
            config_member: "SplunkDestinationConfiguration",
            config: json!({
                "HECEndpoint": "https://splunk.example.com:8088",
                "HECEndpointType": "Event",
                "HECToken": "hec-token",
                "S3Configuration": s3_configuration(),
            }),
            description_member: "SplunkDestinationDescription",
            s3_backup_mode: "FailedEventsOnly",
            also: vec![],
        },
        BackupModeCase {
            stream: "splunk-all-events",
            config_member: "SplunkDestinationConfiguration",
            config: json!({
                "HECEndpoint": "https://splunk.example.com:8088",
                "HECEndpointType": "Raw",
                "HECToken": "hec-token",
                "S3Configuration": s3_configuration(),
                "S3BackupMode": "AllEvents",
            }),
            description_member: "SplunkDestinationDescription",
            s3_backup_mode: "AllEvents",
            also: vec![],
        },
        // A kind whose model documents no default: there is nothing to materialise, but a
        // value the create supplied is still reported as given.
        BackupModeCase {
            stream: "http-endpoint-all-data",
            config_member: "HttpEndpointDestinationConfiguration",
            config: json!({
                "EndpointConfiguration": { "Url": "https://example.com/firehose" },
                "S3Configuration": s3_configuration(),
                "S3BackupMode": "AllData",
            }),
            description_member: "HttpEndpointDestinationDescription",
            s3_backup_mode: "AllData",
            also: vec![],
        },
    ]
}

#[tokio::test]
async fn a_destination_description_reports_its_documented_s3_backup_mode() {
    let emulator = Emulator::new();

    for case in backup_mode_cases() {
        let mut request = json!({ "DeliveryStreamName": case.stream });
        request[case.config_member] = case.config.clone();
        let (status, created) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(
            status, 200,
            "case {:?}: create with {} got {created:?}",
            case.stream, case.config_member
        );

        let (status, described) = emulator
            .call(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": case.stream }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: describe got {described:?}",
            case.stream
        );
        // A stream has one destination, so the description under test is the first.
        let description = described["DeliveryStreamDescription"]["Destinations"][0]
            [case.description_member]
            .clone();
        assert_eq!(
            description["S3BackupMode"],
            json!(case.s3_backup_mode),
            "case {:?}: expected S3BackupMode={:?}, got {:?}",
            case.stream,
            case.s3_backup_mode,
            description["S3BackupMode"]
        );
        for (path, expected) in &case.also {
            assert_eq!(
                at(&description, path),
                expected,
                "case {:?}: {path} is {:?}, expected {expected:?}",
                case.stream,
                at(&description, path)
            );
        }
    }
}
