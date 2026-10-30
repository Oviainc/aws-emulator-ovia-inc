// SPDX-License-Identifier: CC0-1.0

//! The deprecated `DestinationDescription$S3DestinationDescription` member a described
//! Amazon S3 destination carries.
//!
//! The rule under test: a destination whose own destination IS Amazon S3 is reported under
//! BOTH spellings the model gives it — the deprecated `S3DestinationDescription` beside the
//! variant-specific description — and both are rendered from the same effective
//! configuration, so every member the two shapes share agrees and carries the value the
//! create supplied or the default the model documents for it. A destination that merely
//! uses Amazon S3 as its intermediate location reports that location inside its own
//! description, where the model places it, and carries no top-level mirror.
//!
//! The rows span what the rule generalises over rather than one request: which spelling the
//! create used, whether each shared member was supplied or left to its default, the values
//! and types those members take (both encryption forms, a compression format other than the
//! default, buffering at the published bounds), the extended-only members the mirror cannot
//! carry, and the destination kinds the mirror does not apply to.
//!
//! The harness (`Emulator`, `at`, the ARNs) is the lifecycle suite's.

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{
    at, Emulator, BUCKET_ARN, PAST_THE_TRANSITION, REGION, ROLE_ARN,
};

/// The members `S3DestinationDescription` and `ExtendedS3DestinationDescription` SHARE
/// (`smithy_query(query="shape", shape="S3DestinationDescription,
/// ExtendedS3DestinationDescription")`): the deprecated mirror reports exactly these, so
/// the two spellings of one destination must agree on every one of them.
const SHARED_MEMBERS: [&str; 8] = [
    "RoleARN",
    "BucketARN",
    "Prefix",
    "ErrorOutputPrefix",
    "BufferingHints",
    "CompressionFormat",
    "EncryptionConfiguration",
    "CloudWatchLoggingOptions",
];

/// The members `ExtendedS3DestinationDescription` adds and the deprecated shape does not
/// model, from the same query: the mirror can carry none of them.
const EXTENDED_ONLY_MEMBERS: [&str; 7] = [
    "ProcessingConfiguration",
    "S3BackupMode",
    "S3BackupDescription",
    "DataFormatConversionConfiguration",
    "DynamicPartitioningConfiguration",
    "FileExtension",
    "CustomTimeZone",
];

/// The mirror of a destination that supplied nothing but the role and the bucket: every
/// documented default materialised (`BufferingHints` 300 / 5, `CompressionFormat`
/// `UNCOMPRESSED`, no encryption, CloudWatch logging off).
fn defaulted_mirror() -> Value {
    json!({
        "RoleARN": ROLE_ARN,
        "BucketARN": BUCKET_ARN,
        "BufferingHints": { "IntervalInSeconds": 300, "SizeInMBs": 5 },
        "CompressionFormat": "UNCOMPRESSED",
        "EncryptionConfiguration": { "NoEncryptionConfig": "NoEncryption" },
        "CloudWatchLoggingOptions": { "Enabled": false },
    })
}

/// A KMS key ARN satisfying `KMSEncryptionConfig$AWSKMSKeyARN`'s modelled pattern.
fn destination_key_arn() -> String {
    format!("arn:aws:kms:{REGION}:000000000000:key/mirror-key")
}

/// One stream: the destination its create configures, and the mirror a describe must
/// report for it.
struct MirrorCase {
    case: &'static str,
    stream: &'static str,
    /// The `CreateDeliveryStream` member carrying the destination.
    config_member: &'static str,
    config: Value,
    /// The `*DestinationDescription` the destination is reported under.
    variant_member: &'static str,
    /// The `S3DestinationDescription` the destination's effective configuration renders
    /// to, or `Value::Null` for a destination whose own destination is not Amazon S3.
    mirror: Value,
}

fn mirror_cases() -> Vec<MirrorCase> {
    let redshift_url =
        "jdbc:redshift://example-cluster.abc123.us-west-2.redshift.amazonaws.com:5439/dev";
    vec![
        // The deprecated spelling of the configuration: the mirror IS this destination's
        // description, and the defaults are materialised into it.
        MirrorCase {
            case: "a plain Amazon S3 destination with nothing but the role and the bucket",
            stream: "mirror-plain-defaulted",
            config_member: "S3DestinationConfiguration",
            config: json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN }),
            variant_member: "S3DestinationDescription",
            mirror: defaulted_mirror(),
        },
        MirrorCase {
            case: "a plain Amazon S3 destination with every shared member supplied",
            stream: "mirror-plain-supplied",
            config_member: "S3DestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "Prefix": "raw/",
                "ErrorOutputPrefix": "errors/",
                // The published lower bounds of `BufferingHints` (SizeInMBs 1..128,
                // IntervalInSeconds 0..900).
                "BufferingHints": { "IntervalInSeconds": 0, "SizeInMBs": 1 },
                "CompressionFormat": "GZIP",
                "EncryptionConfiguration": {
                    "KMSEncryptionConfig": { "AWSKMSKeyARN": destination_key_arn() },
                },
                "CloudWatchLoggingOptions": {
                    "Enabled": true,
                    "LogGroupName": "/aws/kinesisfirehose/mirror",
                    "LogStreamName": "S3Delivery",
                },
            }),
            variant_member: "S3DestinationDescription",
            mirror: json!({
                "RoleARN": ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "Prefix": "raw/",
                "ErrorOutputPrefix": "errors/",
                "BufferingHints": { "IntervalInSeconds": 0, "SizeInMBs": 1 },
                "CompressionFormat": "GZIP",
                "EncryptionConfiguration": {
                    "KMSEncryptionConfig": { "AWSKMSKeyARN": destination_key_arn() },
                },
                "CloudWatchLoggingOptions": {
                    "Enabled": true,
                    "LogGroupName": "/aws/kinesisfirehose/mirror",
                    "LogStreamName": "S3Delivery",
                },
            }),
        },
        // The row the report cites: the extended spelling, with the mirror reported beside
        // it and every default materialised.
        MirrorCase {
            case: "an extended Amazon S3 destination with nothing but the role and the bucket",
            stream: "mirror-extended-defaulted",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN }),
            variant_member: "ExtendedS3DestinationDescription",
            mirror: defaulted_mirror(),
        },
        MirrorCase {
            case: "an extended Amazon S3 destination with its own members beside the shared ones",
            stream: "mirror-extended-supplied",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "Prefix": "extended/",
                "ErrorOutputPrefix": "extended-errors/",
                // The published upper bounds of `BufferingHints`.
                "BufferingHints": { "IntervalInSeconds": 900, "SizeInMBs": 128 },
                "CompressionFormat": "Snappy",
                "EncryptionConfiguration": { "NoEncryptionConfig": "NoEncryption" },
                "CloudWatchLoggingOptions": { "Enabled": false },
                // Members only the extended shape models: the mirror carries none of them.
                "FileExtension": ".log",
                "CustomTimeZone": "America/Los_Angeles",
                "S3BackupMode": "Enabled",
                "S3BackupConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "Prefix": "backup/",
                },
            }),
            variant_member: "ExtendedS3DestinationDescription",
            mirror: json!({
                "RoleARN": ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "Prefix": "extended/",
                "ErrorOutputPrefix": "extended-errors/",
                "BufferingHints": { "IntervalInSeconds": 900, "SizeInMBs": 128 },
                "CompressionFormat": "Snappy",
                "EncryptionConfiguration": { "NoEncryptionConfig": "NoEncryption" },
                "CloudWatchLoggingOptions": { "Enabled": false },
            }),
        },
        // Destination kinds whose destination is NOT Amazon S3: the Amazon S3 location they
        // stage through is reported inside their own description, and there is no mirror.
        MirrorCase {
            case: "a Redshift destination staging through Amazon S3",
            stream: "mirror-redshift",
            config_member: "RedshiftDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "ClusterJDBCURL": redshift_url,
                "CopyCommand": { "DataTableName": "events" },
                "Username": "analytics",
                "Password": "sup3r-secret-pw",
                "S3Configuration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "Prefix": "redshift/",
                },
            }),
            variant_member: "RedshiftDestinationDescription",
            mirror: Value::Null,
        },
        MirrorCase {
            case: "a Splunk destination staging through Amazon S3",
            stream: "mirror-splunk",
            config_member: "SplunkDestinationConfiguration",
            config: json!({
                "HECEndpoint": "https://splunk.example.com:8088",
                "HECEndpointType": "Event",
                "HECToken": "hec-token",
                "S3Configuration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "Prefix": "splunk/",
                },
            }),
            variant_member: "SplunkDestinationDescription",
            mirror: Value::Null,
        },
    ]
}

/// The sole destination description of `stream`.
async fn destination_of(emulator: &Emulator, stream: &str) -> Value {
    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": stream }),
        )
        .await;
    assert_eq!(status, 200, "describe {stream}: got {described:?}");
    described["DeliveryStreamDescription"]["Destinations"][0].clone()
}

/// Assert `destination` reports the deprecated mirror and the variant description in step:
/// every shared member agrees, and the mirror carries no extended-only member.
fn assert_spellings_agree(case: &str, destination: &Value, variant_member: &str) {
    let mirror = &destination["S3DestinationDescription"];
    let variant = &destination[variant_member];
    for member in SHARED_MEMBERS {
        assert_eq!(
            mirror[member], variant[member],
            "case {case:?}: the two spellings disagree on {member}: mirror {:?}, \
             {variant_member} {:?}",
            mirror[member], variant[member]
        );
    }
    for member in EXTENDED_ONLY_MEMBERS {
        assert!(
            mirror[member].is_null(),
            "case {case:?}: S3DestinationDescription does not model {member}, got {:?}",
            mirror[member]
        );
    }
}

#[tokio::test]
async fn an_amazon_s3_destination_is_described_under_both_of_its_spellings() {
    let emulator = Emulator::new();
    let cases = mirror_cases();

    for case in &cases {
        let mut request = json!({ "DeliveryStreamName": case.stream });
        request[case.config_member] = case.config.clone();
        let (status, created) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(
            status, 200,
            "case {:?}: create with {} got {created:?}",
            case.case, case.config_member
        );
    }

    for case in &cases {
        let destination = destination_of(&emulator, case.stream).await;
        assert!(
            !destination[case.variant_member].is_null(),
            "case {:?}: the destination is reported under {}, got {destination:?}",
            case.case,
            case.variant_member
        );
        assert_eq!(
            destination["S3DestinationDescription"], case.mirror,
            "case {:?}: expected S3DestinationDescription {:?}, got {:?}",
            case.case, case.mirror, destination["S3DestinationDescription"]
        );

        if case.mirror.is_null() {
            // The Amazon S3 location this kind stages through is still reported, inside
            // its own description where the model places it.
            assert_eq!(
                at(
                    &destination[case.variant_member],
                    "S3DestinationDescription/BucketARN"
                ),
                &json!(BUCKET_ARN),
                "case {:?}: the staged Amazon S3 location is reported, got {destination:?}",
                case.case
            );
        } else {
            assert_spellings_agree(case.case, &destination, case.variant_member);
        }

        // Describing is reporting: a second read answers exactly the first.
        let again = destination_of(&emulator, case.stream).await;
        assert_eq!(
            again, destination,
            "case {:?}: two reads in a row report the same destination, got {again:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_update_keeps_both_spellings_of_an_amazon_s3_destination_in_step() {
    let emulator = Emulator::new();
    let (status, created) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "mirror-updated",
                "S3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "Prefix": "raw/",
                },
            }),
        )
        .await;
    assert_eq!(status, 200, "create: got {created:?}");
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let created_destination = destination_of(&emulator, "mirror-updated").await;
    let destination_id = created_destination["DestinationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Each row applies to the destination the previous one left: the version an update is
    // conditional on is the one the last read reported, and after every applied update
    // both spellings still describe the same destination.
    //
    // The rows differ in which spelling of the update is used and which shared member it
    // rewrites, so the agreement is asserted over a destination that has been through the
    // deprecated update, the extended one, and a change of compression as well as prefix.
    let rows: Vec<(&str, &str, Value, &str, Value)> = vec![
        (
            "the deprecated spelling of the update rewrites the prefix",
            "S3DestinationUpdate",
            json!({ "Prefix": "updated/" }),
            "S3DestinationDescription",
            json!("updated/"),
        ),
        (
            "the deprecated spelling of the update rewrites the compression",
            "S3DestinationUpdate",
            json!({ "CompressionFormat": "ZIP" }),
            "S3DestinationDescription",
            json!("updated/"),
        ),
        (
            "the extended spelling promotes the destination and adds its own member",
            "ExtendedS3DestinationUpdate",
            json!({ "FileExtension": ".log", "ErrorOutputPrefix": "errors/" }),
            "ExtendedS3DestinationDescription",
            json!("updated/"),
        ),
        (
            "the extended spelling rewrites the prefix again",
            "ExtendedS3DestinationUpdate",
            json!({ "Prefix": "promoted/" }),
            "ExtendedS3DestinationDescription",
            json!("promoted/"),
        ),
    ];

    for (case, update_member, update, variant_member, expected_prefix) in rows {
        let (status, described) = emulator
            .call(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": "mirror-updated" }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: describe got {described:?}");
        let version = described["DeliveryStreamDescription"]["VersionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let mut request = json!({
            "DeliveryStreamName": "mirror-updated",
            "CurrentDeliveryStreamVersionId": version,
            "DestinationId": destination_id,
        });
        request[update_member] = update;
        let (status, updated) = emulator.call("UpdateDestination", request).await;
        assert_eq!(status, 200, "case {case:?}: update got {updated:?}");

        let destination = destination_of(&emulator, "mirror-updated").await;
        assert!(
            !destination[variant_member].is_null(),
            "case {case:?}: the destination is reported under {variant_member}, \
             got {destination:?}"
        );
        assert_eq!(
            at(&destination, "S3DestinationDescription/Prefix"),
            &expected_prefix,
            "case {case:?}: expected the mirror's Prefix {expected_prefix:?}, \
             got {destination:?}"
        );
        assert_spellings_agree(case, &destination, variant_member);
    }

    // The member only the extended shape models is reported there and nowhere else.
    let destination = destination_of(&emulator, "mirror-updated").await;
    assert_eq!(
        at(&destination, "ExtendedS3DestinationDescription/FileExtension"),
        &json!(".log"),
        "the extended-only member the update supplied is reported, got {destination:?}"
    );
}
