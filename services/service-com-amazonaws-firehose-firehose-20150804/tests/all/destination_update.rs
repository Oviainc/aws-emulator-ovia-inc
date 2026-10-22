// SPDX-License-Identifier: CC0-1.0

//! `UpdateDestination` over the real `awsJson1_1` wire: the conditional, merging
//! destination update the model documents.
//!
//! The rules asserted here, each over a SET of inputs rather than one request:
//!
//! * same destination type -> the supplied parameters are merged over the stored
//!   configuration and the unspecified ones keep their values;
//! * different destination type -> nothing is merged, so every required parameter of the
//!   new type must be supplied, and a switch into or out of an Amazon OpenSearch Service
//!   offering is refused;
//! * the update applies only when `CurrentDeliveryStreamVersionId` equals the stream's
//!   current `VersionId`, and a successful update moves that version on;
//! * only an `ACTIVE` stream, and only a destination id the stream actually has, can be
//!   updated.

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{
    at, error_code, s3_configuration, snowflake_private_key, Emulator, BUCKET_ARN,
    PAST_THE_TRANSITION, ROLE_ARN,
};

/// A second role ARN, so a case can tell the role it updated TO from the one the stream
/// was created with.
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/firehose-delivery-v2";

/// Create a stream whose only destination is `member`: `config`.
async fn create(emulator: &Emulator, stream: &str, member: &str, config: Value) {
    let mut request = json!({ "DeliveryStreamName": stream });
    request[member] = config;
    let (status, created) = emulator.call("CreateDeliveryStream", request).await;
    assert_eq!(status, 200, "create {stream}: got {created:?}");
}

/// Call `UpdateDestination` with `member`: `update` as its destination update.
async fn update(
    emulator: &Emulator,
    stream: &str,
    version: &str,
    destination_id: &str,
    member: &str,
    update: Value,
) -> (u16, Value) {
    let mut request = json!({
        "DeliveryStreamName": stream,
        "CurrentDeliveryStreamVersionId": version,
        "DestinationId": destination_id,
    });
    request[member] = update;
    emulator.call("UpdateDestination", request).await
}

/// The `DeliveryStreamDescription` of `stream`.
async fn describe(emulator: &Emulator, stream: &str) -> Value {
    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": stream }),
        )
        .await;
    assert_eq!(status, 200, "describe {stream}: got {described:?}");
    described["DeliveryStreamDescription"].clone()
}

/// `(VersionId, DestinationId)` of a stream's sole destination.
async fn version_and_destination(emulator: &Emulator, stream: &str) -> (String, String) {
    let description = describe(emulator, stream).await;
    let version = description["VersionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let destination_id = description["Destinations"][0]["DestinationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !version.is_empty() && !destination_id.is_empty(),
        "describe {stream}: expected a version and a destination id, got {description:?}"
    );
    (version, destination_id)
}

/// The sole `DestinationDescription` of a stream.
async fn destination_of(emulator: &Emulator, stream: &str) -> Value {
    describe(emulator, stream).await["Destinations"][0].clone()
}

/// One row of the merge table: a destination created one way, updated with a few of that
/// same type's parameters, and read back.
struct MergeCase {
    stream: &'static str,
    /// The `CreateDeliveryStream` member carrying the destination.
    config_member: &'static str,
    config: Value,
    /// The `UpdateDestination` member carrying the update.
    update_member: &'static str,
    update: Value,
    /// The `DestinationDescription` member the destination is reported under.
    description_member: &'static str,
    /// `(path inside the description, expected value)` for parameters the update supplied.
    changed: Vec<(&'static str, Value)>,
    /// `(path, expected)` for parameters the update left out: "If any of the parameters are
    /// not specified in the call, the existing values are retained."
    retained: Vec<(&'static str, Value)>,
}

/// One row per destination type `UpdateDestination` models (the `*DestinationUpdate`
/// members listed by
/// `smithy_query(query="operation", operation="UpdateDestination")`), each changing a
/// different kind of parameter: a scalar, a nested structure, a nested Amazon S3
/// configuration, a boolean, an enum value.
fn merge_cases() -> Vec<MergeCase> {
    vec![
        MergeCase {
            stream: "merge-s3",
            config_member: "S3DestinationConfiguration",
            config: s3_configuration(),
            update_member: "S3DestinationUpdate",
            update: json!({ "Prefix": "merged/", "CompressionFormat": "GZIP" }),
            description_member: "S3DestinationDescription",
            changed: vec![
                ("Prefix", json!("merged/")),
                ("CompressionFormat", json!("GZIP")),
            ],
            retained: vec![
                ("RoleARN", json!(ROLE_ARN)),
                ("BucketARN", json!(BUCKET_ARN)),
                ("BufferingHints/SizeInMBs", json!(5)),
            ],
        },
        MergeCase {
            stream: "merge-extended-s3",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "CompressionFormat": "GZIP",
                "BufferingHints": { "SizeInMBs": 64, "IntervalInSeconds": 60 },
                "FileExtension": ".json",
            }),
            update_member: "ExtendedS3DestinationUpdate",
            update: json!({
                "RoleARN": OTHER_ROLE_ARN,
                "BufferingHints": { "SizeInMBs": 128, "IntervalInSeconds": 900 },
            }),
            description_member: "ExtendedS3DestinationDescription",
            changed: vec![
                ("RoleARN", json!(OTHER_ROLE_ARN)),
                ("BufferingHints/SizeInMBs", json!(128)),
                ("BufferingHints/IntervalInSeconds", json!(900)),
            ],
            retained: vec![
                ("BucketARN", json!(BUCKET_ARN)),
                ("CompressionFormat", json!("GZIP")),
                ("FileExtension", json!(".json")),
            ],
        },
        MergeCase {
            stream: "merge-redshift",
            config_member: "RedshiftDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "ClusterJDBCURL":
                    "jdbc:redshift://example-cluster.abc123.us-west-2.redshift.amazonaws.com:5439/dev",
                "CopyCommand": { "DataTableName": "events" },
                "Username": "analytics",
                "Password": "sup3r-secret-pw",
                "S3Configuration": s3_configuration(),
            }),
            update_member: "RedshiftDestinationUpdate",
            // A nested `S3Update` merges into the nested Amazon S3 configuration the
            // destination already holds.
            update: json!({
                "Username": "loader",
                "S3Update": { "Prefix": "redshift/" },
            }),
            description_member: "RedshiftDestinationDescription",
            changed: vec![
                ("Username", json!("loader")),
                ("S3DestinationDescription/Prefix", json!("redshift/")),
            ],
            retained: vec![
                ("CopyCommand/DataTableName", json!("events")),
                ("S3DestinationDescription/BucketARN", json!(BUCKET_ARN)),
                ("S3DestinationDescription/RoleARN", json!(ROLE_ARN)),
            ],
        },
        MergeCase {
            stream: "merge-elasticsearch",
            config_member: "ElasticsearchDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "DomainARN": "arn:aws:es:us-west-2:000000000000:domain/logs-domain",
                "IndexName": "logs",
                "S3Configuration": s3_configuration(),
            }),
            update_member: "ElasticsearchDestinationUpdate",
            update: json!({ "IndexName": "logs-v2", "IndexRotationPeriod": "OneDay" }),
            description_member: "ElasticsearchDestinationDescription",
            changed: vec![
                ("IndexName", json!("logs-v2")),
                ("IndexRotationPeriod", json!("OneDay")),
            ],
            retained: vec![
                (
                    "DomainARN",
                    json!("arn:aws:es:us-west-2:000000000000:domain/logs-domain"),
                ),
                ("RoleARN", json!(ROLE_ARN)),
            ],
        },
        MergeCase {
            stream: "merge-opensearch",
            config_member: "AmazonopensearchserviceDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "ClusterEndpoint": "https://search.example.com",
                "IndexName": "docs",
                "S3Configuration": s3_configuration(),
            }),
            update_member: "AmazonopensearchserviceDestinationUpdate",
            update: json!({ "ClusterEndpoint": "https://search-v2.example.com" }),
            description_member: "AmazonopensearchserviceDestinationDescription",
            changed: vec![(
                "ClusterEndpoint",
                json!("https://search-v2.example.com"),
            )],
            retained: vec![("IndexName", json!("docs")), ("RoleARN", json!(ROLE_ARN))],
        },
        MergeCase {
            stream: "merge-splunk",
            config_member: "SplunkDestinationConfiguration",
            config: json!({
                "HECEndpoint": "https://splunk.example.com:8088",
                "HECEndpointType": "Event",
                "HECToken": "hec-token",
                "HECAcknowledgmentTimeoutInSeconds": 300,
                "S3Configuration": s3_configuration(),
            }),
            update_member: "SplunkDestinationUpdate",
            update: json!({
                "HECEndpointType": "Raw",
                "HECAcknowledgmentTimeoutInSeconds": 180,
            }),
            description_member: "SplunkDestinationDescription",
            changed: vec![
                ("HECEndpointType", json!("Raw")),
                ("HECAcknowledgmentTimeoutInSeconds", json!(180)),
            ],
            retained: vec![(
                "HECEndpoint",
                json!("https://splunk.example.com:8088"),
            )],
        },
        MergeCase {
            stream: "merge-http-endpoint",
            config_member: "HttpEndpointDestinationConfiguration",
            config: json!({
                "EndpointConfiguration": {
                    "Url": "https://example.com/firehose",
                    "Name": "example",
                    "AccessKey": "http-access-key",
                },
                "S3Configuration": s3_configuration(),
            }),
            update_member: "HttpEndpointDestinationUpdate",
            update: json!({
                "EndpointConfiguration": {
                    "Url": "https://example.com/firehose-v2",
                    "Name": "example-v2",
                },
            }),
            description_member: "HttpEndpointDestinationDescription",
            changed: vec![
                (
                    "EndpointConfiguration/Url",
                    json!("https://example.com/firehose-v2"),
                ),
                ("EndpointConfiguration/Name", json!("example-v2")),
            ],
            retained: vec![("S3DestinationDescription/BucketARN", json!(BUCKET_ARN))],
        },
        MergeCase {
            stream: "merge-snowflake",
            config_member: "SnowflakeDestinationConfiguration",
            config: json!({
                "AccountUrl": "my-account-id.snowflakecomputing.com",
                "PrivateKey": snowflake_private_key(),
                "KeyPassphrase": "passphrase",
                "User": "loader",
                "Database": "analytics",
                "Schema": "public",
                "Table": "events",
                "RoleARN": ROLE_ARN,
                "S3Configuration": s3_configuration(),
            }),
            update_member: "SnowflakeDestinationUpdate",
            update: json!({ "Table": "events_v2", "MetaDataColumnName": "meta" }),
            description_member: "SnowflakeDestinationDescription",
            changed: vec![
                ("Table", json!("events_v2")),
                ("MetaDataColumnName", json!("meta")),
            ],
            retained: vec![
                ("Database", json!("analytics")),
                ("Schema", json!("public")),
                (
                    "AccountUrl",
                    json!("my-account-id.snowflakecomputing.com"),
                ),
            ],
        },
        MergeCase {
            stream: "merge-serverless",
            config_member: "AmazonOpenSearchServerlessDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "CollectionEndpoint": "https://collection.example.com",
                "IndexName": "serverless-index",
                "S3Configuration": s3_configuration(),
            }),
            update_member: "AmazonOpenSearchServerlessDestinationUpdate",
            update: json!({ "IndexName": "serverless-index-v2" }),
            description_member: "AmazonOpenSearchServerlessDestinationDescription",
            changed: vec![("IndexName", json!("serverless-index-v2"))],
            retained: vec![
                (
                    "CollectionEndpoint",
                    json!("https://collection.example.com"),
                ),
                ("RoleARN", json!(ROLE_ARN)),
            ],
        },
        MergeCase {
            stream: "merge-iceberg",
            config_member: "IcebergDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "CatalogConfiguration": {
                    "CatalogARN": "arn:aws:glue:us-west-2:000000000000:catalog",
                },
                "AppendOnly": true,
                "S3Configuration": s3_configuration(),
            }),
            update_member: "IcebergDestinationUpdate",
            // `IcebergDestinationUpdate` carries a whole `S3Configuration`, not an update,
            // so a supplied one replaces what the destination held.
            update: json!({
                "AppendOnly": false,
                "S3Configuration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "Prefix": "iceberg/",
                },
            }),
            description_member: "IcebergDestinationDescription",
            changed: vec![
                ("AppendOnly", json!(false)),
                ("S3DestinationDescription/Prefix", json!("iceberg/")),
            ],
            retained: vec![
                (
                    "CatalogConfiguration/CatalogARN",
                    json!("arn:aws:glue:us-west-2:000000000000:catalog"),
                ),
                ("RoleARN", json!(ROLE_ARN)),
            ],
        },
    ]
}

#[tokio::test]
async fn an_update_of_the_same_destination_type_merges_the_parameters_it_supplies() {
    let emulator = Emulator::new();
    let cases = merge_cases();

    // Every stream is created first, so ONE wait covers every creation span.
    for case in &cases {
        create(
            &emulator,
            case.stream,
            case.config_member,
            case.config.clone(),
        )
        .await;
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for case in &cases {
        let stream = case.stream;
        let (version, destination_id) = version_and_destination(&emulator, stream).await;
        let (status, updated) = update(
            &emulator,
            stream,
            &version,
            &destination_id,
            case.update_member,
            case.update.clone(),
        )
        .await;
        assert_eq!(status, 200, "case {stream:?}: got {updated:?}");

        let description = describe(&emulator, stream).await;
        // "After the update is applied successfully, the version ID is updated."
        assert_eq!(
            description["VersionId"],
            json!("2"),
            "case {stream:?}: the applied update moves the version on from {version:?}, \
             got {description:?}"
        );
        // "The target Firehose stream remains active while the configurations are updated."
        assert_eq!(
            description["DeliveryStreamStatus"],
            json!("ACTIVE"),
            "case {stream:?}: got {description:?}"
        );

        let destination = &description["Destinations"][0];
        assert_eq!(
            destination["DestinationId"],
            json!(destination_id),
            "case {stream:?}: the update rewrites the destination it names rather than \
             adding one, got {description:?}"
        );

        let reported = &destination[case.description_member];
        for (path, expected) in &case.changed {
            assert_eq!(
                at(reported, path),
                expected,
                "case {stream:?}: {path} was supplied by the update, so it is {expected:?}; \
                 got {reported:?}"
            );
        }
        for (path, expected) in &case.retained {
            assert_eq!(
                at(reported, path),
                expected,
                "case {stream:?}: {path} was not supplied by the update, so it keeps \
                 {expected:?}; got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_update_applies_only_on_the_current_version_and_moves_it_on() {
    let emulator = Emulator::new();
    create(
        &emulator,
        "versioned-stream",
        "S3DestinationConfiguration",
        s3_configuration(),
    )
    .await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // Each applied update moves the version on by one, and the next update must carry the
    // version the previous one produced.
    let mut version = "1".to_string();
    let (_, destination_id) = version_and_destination(&emulator, "versioned-stream").await;
    for round in 1..=3 {
        let prefix = format!("round-{round}/");
        let (status, updated) = update(
            &emulator,
            "versioned-stream",
            &version,
            &destination_id,
            "S3DestinationUpdate",
            json!({ "Prefix": prefix }),
        )
        .await;
        assert_eq!(status, 200, "round {round}: got {updated:?}");

        let description = describe(&emulator, "versioned-stream").await;
        let expected_version = (round + 1).to_string();
        assert_eq!(
            description["VersionId"],
            json!(expected_version),
            "round {round}: the version moves on by one from {version:?}, got {description:?}"
        );
        assert_eq!(
            at(
                &description["Destinations"][0],
                "S3DestinationDescription/Prefix"
            ),
            &json!(prefix),
            "round {round}: the applied update is what a later read sees, got {description:?}"
        );
        version = expected_version;
    }

    // A version id that is not the stream's current one is refused, and the refusal writes
    // NOTHING: the version and the configuration a later read sees are untouched.
    // `DeliveryStreamVersionId` is a decimal number (`^[0-9]+$`, 1..50 characters), so it
    // is compared by value: the stale, the never-minted and the unspellably large are all
    // conflicts, and the same number spelled with leading zeros is not.
    let stale: Vec<(&str, &str)> = vec![
        ("the version before the last applied update", "3"),
        ("the version the stream started at", "1"),
        ("a version this stream never reached", "99"),
        (
            "a version id too large for any version",
            "99999999999999999999999999999999999999999999999999",
        ),
    ];
    for (case, supplied) in stale {
        let (status, refused) = update(
            &emulator,
            "versioned-stream",
            supplied,
            &destination_id,
            "S3DestinationUpdate",
            json!({ "Prefix": "conflicting/" }),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ConcurrentModificationException",
            "case {case:?}: an update conditional on {supplied:?} conflicts with version \
             {version:?}, got {refused:?}"
        );
        let description = describe(&emulator, "versioned-stream").await;
        assert_eq!(
            description["VersionId"],
            json!(version),
            "case {case:?}: a refused update leaves the version alone, got {description:?}"
        );
        assert_eq!(
            at(
                &description["Destinations"][0],
                "S3DestinationDescription/Prefix"
            ),
            &json!("round-3/"),
            "case {case:?}: a refused update writes nothing, got {description:?}"
        );
    }

    // The current version spelled with leading zeros is the same number, so it applies.
    let (status, applied) = update(
        &emulator,
        "versioned-stream",
        &format!("000{version}"),
        &destination_id,
        "S3DestinationUpdate",
        json!({ "Prefix": "zero-padded/" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the current version spelled with leading zeros: got {applied:?}"
    );
    let description = describe(&emulator, "versioned-stream").await;
    assert_eq!(
        description["VersionId"],
        json!("5"),
        "the zero-padded current version applies and moves the version on, \
         got {description:?}"
    );
}

#[tokio::test]
async fn a_destination_type_change_replaces_the_configuration_whole() {
    let emulator = Emulator::new();
    let elasticsearch = json!({
        "RoleARN": ROLE_ARN,
        "DomainARN": "arn:aws:es:us-west-2:000000000000:domain/logs-domain",
        "IndexName": "logs",
        "S3Configuration": s3_configuration(),
    });
    create(
        &emulator,
        "switch-s3",
        "S3DestinationConfiguration",
        s3_configuration(),
    )
    .await;
    create(
        &emulator,
        "switch-elasticsearch",
        "ElasticsearchDestinationConfiguration",
        elasticsearch.clone(),
    )
    .await;
    create(
        &emulator,
        "switch-serverless",
        "AmazonOpenSearchServerlessDestinationConfiguration",
        json!({
            "RoleARN": ROLE_ARN,
            "CollectionEndpoint": "https://collection.example.com",
            "IndexName": "serverless-index",
            "S3Configuration": s3_configuration(),
        }),
    )
    .await;
    create(
        &emulator,
        "switch-promoted",
        "S3DestinationConfiguration",
        s3_configuration(),
    )
    .await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let complete_redshift = json!({
        "RoleARN": ROLE_ARN,
        "ClusterJDBCURL":
            "jdbc:redshift://example-cluster.abc123.us-west-2.redshift.amazonaws.com:5439/dev",
        "CopyCommand": { "DataTableName": "events" },
        "S3Update": { "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN, "Prefix": "redshift/" },
    });

    // "If the destination type is not the same ... Firehose does not merge any parameters.
    // In this case, all parameters must be specified": an update missing a required
    // parameter of the new type is refused, and the destination keeps the type it had.
    let incomplete: Vec<(&str, &str, Value)> = vec![
        (
            "a Redshift switch without the S3 configuration it requires",
            "RedshiftDestinationUpdate",
            json!({
                "RoleARN": ROLE_ARN,
                "ClusterJDBCURL":
                    "jdbc:redshift://example-cluster.abc123.us-west-2.redshift.amazonaws.com:5439/dev",
                "CopyCommand": { "DataTableName": "events" },
            }),
        ),
        (
            "a Redshift switch without the cluster URL it requires",
            "RedshiftDestinationUpdate",
            json!({
                "RoleARN": ROLE_ARN,
                "CopyCommand": { "DataTableName": "events" },
                "S3Update": { "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN },
            }),
        ),
        (
            "a Splunk switch without the HEC endpoint it requires",
            "SplunkDestinationUpdate",
            json!({
                "HECEndpointType": "Event",
                "S3Update": { "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN },
            }),
        ),
        (
            "an HTTP endpoint switch without the endpoint configuration it requires",
            "HttpEndpointDestinationUpdate",
            json!({
                "RoleARN": ROLE_ARN,
                "S3Update": { "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN },
            }),
        ),
    ];
    for (case, member, body) in incomplete {
        let (version, destination_id) = version_and_destination(&emulator, "switch-s3").await;
        let (status, refused) = update(
            &emulator,
            "switch-s3",
            &version,
            &destination_id,
            member,
            body,
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "InvalidArgumentException",
            "case {case:?}: got {refused:?}"
        );
        let destination = destination_of(&emulator, "switch-s3").await;
        assert!(
            !destination["S3DestinationDescription"].is_null(),
            "case {case:?}: the refused switch leaves the Amazon S3 destination in place, \
             got {destination:?}"
        );
    }

    // "Switching between Amazon OpenSearch Service and other services is not supported.
    // For an Amazon OpenSearch Service destination, you can only update to another Amazon
    // OpenSearch Service destination."
    let unsupported: Vec<(&str, &str, &str, Value)> = vec![
        (
            "into Amazon OpenSearch Service from Amazon S3",
            "switch-s3",
            "ElasticsearchDestinationUpdate",
            elasticsearch.clone(),
        ),
        (
            "out of Amazon OpenSearch Service to Amazon S3",
            "switch-elasticsearch",
            "S3DestinationUpdate",
            json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN }),
        ),
        (
            "out of Amazon OpenSearch Service to Redshift",
            "switch-elasticsearch",
            "RedshiftDestinationUpdate",
            complete_redshift.clone(),
        ),
        (
            "between the Serverless offering and a domain",
            "switch-serverless",
            "ElasticsearchDestinationUpdate",
            elasticsearch.clone(),
        ),
    ];
    for (case, stream, member, body) in unsupported {
        let (version, destination_id) = version_and_destination(&emulator, stream).await;
        let (status, refused) =
            update(&emulator, stream, &version, &destination_id, member, body).await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "InvalidArgumentException",
            "case {case:?}: got {refused:?}"
        );
        let description = describe(&emulator, stream).await;
        assert_eq!(
            description["VersionId"],
            json!(version),
            "case {case:?}: the refused switch leaves the version alone, got {description:?}"
        );
    }

    // A switch between two types that are not an Amazon OpenSearch Service offering, with
    // every required parameter supplied, replaces the configuration whole.
    let (version, destination_id) = version_and_destination(&emulator, "switch-s3").await;
    let (status, switched) = update(
        &emulator,
        "switch-s3",
        &version,
        &destination_id,
        "RedshiftDestinationUpdate",
        complete_redshift,
    )
    .await;
    assert_eq!(status, 200, "a complete Redshift switch: got {switched:?}");
    let destination = destination_of(&emulator, "switch-s3").await;
    assert!(
        destination["S3DestinationDescription"].is_null(),
        "the replaced Amazon S3 destination is no longer reported, got {destination:?}"
    );
    assert_eq!(
        at(
            &destination,
            "RedshiftDestinationDescription/CopyCommand/DataTableName"
        ),
        &json!("events"),
        "the new destination is the one the update supplied, got {destination:?}"
    );
    assert_eq!(
        at(
            &destination,
            "RedshiftDestinationDescription/S3DestinationDescription/Prefix"
        ),
        &json!("redshift/"),
        "the new destination's nested Amazon S3 configuration is the supplied one, \
         got {destination:?}"
    );

    // An Amazon OpenSearch Service domain may be updated to the same offering under its
    // other modelled name.
    let (version, destination_id) =
        version_and_destination(&emulator, "switch-elasticsearch").await;
    let (status, switched) = update(
        &emulator,
        "switch-elasticsearch",
        &version,
        &destination_id,
        "AmazonopensearchserviceDestinationUpdate",
        json!({
            "RoleARN": ROLE_ARN,
            "ClusterEndpoint": "https://search.example.com",
            "IndexName": "docs",
            "S3Update": { "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an Amazon OpenSearch Service domain switch: got {switched:?}"
    );
    let destination = destination_of(&emulator, "switch-elasticsearch").await;
    assert_eq!(
        at(
            &destination,
            "AmazonopensearchserviceDestinationDescription/IndexName"
        ),
        &json!("docs"),
        "the destination is reported under the name the update used, got {destination:?}"
    );

    // `S3DestinationUpdate` is the deprecated spelling of the Amazon S3 destination and
    // `ExtendedS3DestinationUpdate` its superset: an extended update of a plain Amazon S3
    // destination is a merge, not a type change, so the parameters it leaves out survive.
    let (version, destination_id) = version_and_destination(&emulator, "switch-promoted").await;
    let (status, promoted) = update(
        &emulator,
        "switch-promoted",
        &version,
        &destination_id,
        "ExtendedS3DestinationUpdate",
        json!({ "FileExtension": ".log" }),
    )
    .await;
    assert_eq!(status, 200, "an extended Amazon S3 update: got {promoted:?}");
    let destination = destination_of(&emulator, "switch-promoted").await;
    assert_eq!(
        at(&destination, "ExtendedS3DestinationDescription/FileExtension"),
        &json!(".log"),
        "the extended parameter the update supplied is reported, got {destination:?}"
    );
    assert_eq!(
        at(&destination, "ExtendedS3DestinationDescription/Prefix"),
        &json!("raw/"),
        "the parameters the created Amazon S3 destination held are retained, \
         got {destination:?}"
    );
    assert_eq!(
        at(&destination, "ExtendedS3DestinationDescription/BucketARN"),
        &json!(BUCKET_ARN),
        "the parameters the created Amazon S3 destination held are retained, \
         got {destination:?}"
    );
}

#[tokio::test]
async fn an_update_is_refused_unless_the_stream_is_active_and_the_destination_is_its_own() {
    let emulator = Emulator::new();
    let s3_update = json!({ "Prefix": "attempted/" });

    // An unknown stream: there is no configuration to update.
    let (status, refused) = update(
        &emulator,
        "ghost",
        "1",
        "destinationId-000000000001",
        "S3DestinationUpdate",
        s3_update.clone(),
    )
    .await;
    assert_eq!(status, 400, "an unknown stream: got {refused:?}");
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "an unknown stream: got {refused:?}"
    );

    // A stream that is not ACTIVE is in use by the lifecycle change it is in the middle
    // of, in either direction: still CREATING, or already DELETING.
    create(
        &emulator,
        "creating-stream",
        "S3DestinationConfiguration",
        s3_configuration(),
    )
    .await;
    let (status, refused) = update(
        &emulator,
        "creating-stream",
        "1",
        "destinationId-000000000001",
        "S3DestinationUpdate",
        s3_update.clone(),
    )
    .await;
    assert_eq!(status, 400, "a stream still creating: got {refused:?}");
    assert_eq!(
        error_code(&refused),
        "ResourceInUseException",
        "a stream still creating: got {refused:?}"
    );

    create(
        &emulator,
        "deleting-stream",
        "S3DestinationConfiguration",
        s3_configuration(),
    )
    .await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (version, destination_id) = version_and_destination(&emulator, "deleting-stream").await;
    let (status, deleted) = emulator
        .call(
            "DeleteDeliveryStream",
            json!({ "DeliveryStreamName": "deleting-stream" }),
        )
        .await;
    assert_eq!(status, 200, "delete: got {deleted:?}");
    let (status, refused) = update(
        &emulator,
        "deleting-stream",
        &version,
        &destination_id,
        "S3DestinationUpdate",
        s3_update.clone(),
    )
    .await;
    assert_eq!(status, 400, "a stream being deleted: got {refused:?}");
    assert_eq!(
        error_code(&refused),
        "ResourceInUseException",
        "a stream being deleted: got {refused:?}"
    );

    // An ACTIVE stream, so what is refused below is the destination or the update itself.
    let (version, destination_id) = version_and_destination(&emulator, "creating-stream").await;
    assert_eq!(
        version, "1",
        "no refused update moved the version of creating-stream"
    );

    // A well-formed `DestinationId` that is not a destination of this stream.
    let foreign: Vec<(&str, &str)> = vec![
        (
            "a destination id one past the stream's own",
            "destinationId-000000000002",
        ),
        ("a destination id in another format", "abc-123"),
        ("the shortest destination id the model allows", "a"),
    ];
    for (case, supplied) in foreign {
        let (status, refused) = update(
            &emulator,
            "creating-stream",
            &version,
            supplied,
            "S3DestinationUpdate",
            s3_update.clone(),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "InvalidArgumentException",
            "case {case:?}: {supplied:?} names no destination of the stream, \
             got {refused:?}"
        );
    }

    // A destination has exactly one type, so the update names exactly one: neither none
    // nor two.
    let wrong_count: Vec<(&str, Value)> = vec![
        ("no destination update at all", json!({})),
        (
            "two destination updates",
            json!({
                "S3DestinationUpdate": { "Prefix": "attempted/" },
                "ExtendedS3DestinationUpdate": { "Prefix": "attempted/" },
            }),
        ),
        (
            "three destination updates",
            json!({
                "S3DestinationUpdate": { "Prefix": "attempted/" },
                "ExtendedS3DestinationUpdate": { "Prefix": "attempted/" },
                "SplunkDestinationUpdate": { "HECEndpointType": "Raw" },
            }),
        ),
    ];
    for (case, updates) in wrong_count {
        let mut request = json!({
            "DeliveryStreamName": "creating-stream",
            "CurrentDeliveryStreamVersionId": version,
            "DestinationId": destination_id,
        });
        for (member, body) in updates.as_object().expect("the case is an object") {
            request[member.as_str()] = body.clone();
        }
        let (status, refused) = emulator.call("UpdateDestination", request).await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "InvalidArgumentException",
            "case {case:?}: got {refused:?}"
        );
    }

    // None of the refusals above changed anything a later read sees.
    let description = describe(&emulator, "creating-stream").await;
    assert_eq!(
        description["VersionId"],
        json!("1"),
        "a refused update leaves the version alone, got {description:?}"
    );
    assert_eq!(
        at(
            &description["Destinations"][0],
            "S3DestinationDescription/Prefix"
        ),
        &json!("raw/"),
        "a refused update writes nothing, got {description:?}"
    );
}

#[tokio::test]
async fn an_update_leaves_the_streams_own_identity_type_and_source_alone() {
    let emulator = Emulator::new();
    // `DeliveryStreamType` is set at create and `UpdateDestination` carries no member for
    // it, so a destination update cannot change the type, the name, the ARN, the creation
    // time or the source a stream was created with — whichever type that was.
    // The values come from `DeliveryStreamType` (one of: DatabaseAsSource | DirectPut |
    // KinesisStreamAsSource | MSKAsSource), per
    // `smithy_query(query="operation", operation="CreateDeliveryStream")`; the emulator
    // can create the three with a source configuration this harness supplies.
    let stream_types: Vec<(&str, Option<&str>, &str)> = vec![
        ("the default type", None, "DirectPut"),
        ("an explicit DirectPut", Some("DirectPut"), "DirectPut"),
        (
            "a Kinesis-sourced stream",
            Some("KinesisStreamAsSource"),
            "KinesisStreamAsSource",
        ),
        (
            "an MSK-sourced stream",
            Some("MSKAsSource"),
            "MSKAsSource",
        ),
    ];

    for (index, (_, stream_type, _)) in stream_types.iter().enumerate() {
        emulator
            .create_stream(&format!("identity-stream-{index}"), *stream_type)
            .await;
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for (index, (case, _, expected_type)) in stream_types.iter().enumerate() {
        let stream = format!("identity-stream-{index}");
        let before = describe(&emulator, &stream).await;
        let (version, destination_id) = version_and_destination(&emulator, &stream).await;
        let (status, updated) = update(
            &emulator,
            &stream,
            &version,
            &destination_id,
            "S3DestinationUpdate",
            json!({ "Prefix": "updated/" }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {updated:?}");

        let after = describe(&emulator, &stream).await;
        assert_eq!(
            after["DeliveryStreamType"],
            json!(expected_type),
            "case {case:?}: the stream type is set at create and no destination update \
             changes it, got {after:?}"
        );
        for member in ["DeliveryStreamName", "DeliveryStreamARN", "CreateTimestamp"] {
            assert_eq!(
                after[member], before[member],
                "case {case:?}: {member} is untouched by a destination update, \
                 got {after:?}"
            );
        }
        assert_eq!(
            after["Source"], before["Source"],
            "case {case:?}: the source a stream was created with is untouched by a \
             destination update, got {after:?}"
        );
        assert_eq!(
            after["VersionId"],
            json!("2"),
            "case {case:?}: the applied update moves the version on, got {after:?}"
        );
        assert_eq!(
            at(
                &after["Destinations"][0],
                "S3DestinationDescription/Prefix"
            ),
            &json!("updated/"),
            "case {case:?}: the applied update is what a later read sees, got {after:?}"
        );
    }
}

#[tokio::test]
async fn updated_buffering_hints_are_reported_back_exactly() {
    let emulator = Emulator::new();
    create(
        &emulator,
        "buffered-stream",
        "ExtendedS3DestinationConfiguration",
        json!({
            "RoleARN": ROLE_ARN,
            "BucketARN": BUCKET_ARN,
            "BufferingHints": { "SizeInMBs": 5, "IntervalInSeconds": 300 },
        }),
    )
    .await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (_, destination_id) = version_and_destination(&emulator, "buffered-stream").await;

    // `BufferingHints`: `SizeInMBs` is an integer in 1..128 and `IntervalInSeconds` one in
    // 0..900, from
    // `smithy_query(query="shape", shape="BufferingHints")` — each published bound, the
    // documented defaults and a pair of values that are neither round nor equal.
    let accepted: Vec<(&str, i64, i64)> = vec![
        ("both at their minimum", 1, 0),
        ("both at their maximum", 128, 900),
        ("the documented defaults", 5, 300),
        ("values that are neither round nor equal", 7, 123),
        ("one bound with the other's opposite", 128, 0),
    ];
    for (case, size, interval) in accepted {
        let (version, _) = version_and_destination(&emulator, "buffered-stream").await;
        let (status, updated) = update(
            &emulator,
            "buffered-stream",
            &version,
            &destination_id,
            "ExtendedS3DestinationUpdate",
            json!({ "BufferingHints": { "SizeInMBs": size, "IntervalInSeconds": interval } }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {updated:?}");
        let destination = destination_of(&emulator, "buffered-stream").await;
        let reported = at(
            &destination,
            "ExtendedS3DestinationDescription/BufferingHints",
        );
        assert_eq!(
            reported,
            &json!({ "SizeInMBs": size, "IntervalInSeconds": interval }),
            "case {case:?}: the hints are reported as the numbers they were given as, \
             got {reported:?}"
        );
    }

    // One step outside either published bound is refused by the modelled range, before the
    // handler runs, and nothing is written.
    let out_of_range: Vec<(&str, i64, i64)> = vec![
        ("one under the minimum size", 0, 300),
        ("one over the maximum size", 129, 300),
        ("one under the minimum interval", 5, -1),
        ("one over the maximum interval", 5, 901),
    ];
    for (case, size, interval) in out_of_range {
        let (version, _) = version_and_destination(&emulator, "buffered-stream").await;
        let (status, refused) = update(
            &emulator,
            "buffered-stream",
            &version,
            &destination_id,
            "ExtendedS3DestinationUpdate",
            json!({ "BufferingHints": { "SizeInMBs": size, "IntervalInSeconds": interval } }),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: a value outside the modelled range is refused as invalid, \
             got {refused:?}"
        );
        let destination = destination_of(&emulator, "buffered-stream").await;
        assert_eq!(
            at(
                &destination,
                "ExtendedS3DestinationDescription/BufferingHints"
            ),
            &json!({ "SizeInMBs": 128, "IntervalInSeconds": 0 }),
            "case {case:?}: the refused update leaves the last applied hints in place, \
             got {destination:?}"
        );
    }
}
