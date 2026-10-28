// SPDX-License-Identifier: CC0-1.0

//! The destination-bucket pre-flight over the members and the bucket NAME FORMS the
//! `destination_bucket_preflight.rs` table does not reach.
//!
//! The rule is that suite's: `UpdateDestination` resolves every Amazon S3 bucket the
//! submitted destination update names against Amazon S3 BEFORE it rewrites the stored
//! destination, refuses a bucket Amazon S3 does not hold with HTTP 400
//! `InvalidArgumentException` naming that bucket, and leaves the stream's `VersionId` and
//! stored destination as they were. What is added here are the other two axes the rule
//! generalises over:
//!
//! * the remaining members that carry a `BucketARN` into the operation — the `S3Update` of
//!   a Redshift and of an HTTP endpoint destination, and the whole `S3Configuration` an
//!   Iceberg update replaces rather than merges (its bucket is a REQUIRED member, where
//!   every other row's is optional);
//! * the FORM of the name that is not there, since acceptance must be Amazon S3's own
//!   answer and not any matching done here: a near miss of the held bucket's name, a name
//!   the held one is a prefix of, an absent bucket in another partition, a dotted name, and
//!   the longest name an S3 general purpose bucket has.
//!
//! Both services are mounted on one shared engine through the `destination_delivery`
//! harness, so the buckets are created through Amazon S3's own wire and resolved through
//! the same state the delivery leg writes to.

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{
    at, error_code, error_message, PAST_THE_TRANSITION, REGION, ROLE_ARN,
};
// The harness, the ARN form, the nested configuration and the version/destination read are
// the pre-flight suite's: this one adds rows, not a second way of asking.
use crate::destination_bucket_preflight::{bucket_arn, s3_configuration, version_and_destination};
use crate::destination_delivery::Emulator;

/// The bucket every stream below is created against: the destination a refused update must
/// leave in place.
const HELD: &str = "resolution-held";

/// A SECOND bucket Amazon S3 holds, so each row can tell "this update was refused" from
/// "no update of this member ever lands".
const TARGET: &str = "resolution-target";

/// The bucket name a `BucketARN` names: its resource part, the whole of what follows the
/// last colon.
fn bucket_name(bucket_arn: &str) -> &str {
    bucket_arn.rsplit(':').next().unwrap_or_default()
}

/// One row: a destination type, created delivering to [`HELD`], and the member of that
/// type's update which carries the bucket.
struct MemberCase {
    /// The stream this row drives, named for the member it exercises.
    stream: &'static str,
    /// The `CreateDeliveryStream` member carrying the destination.
    config_member: &'static str,
    /// That destination's configuration, delivering to the bucket named.
    config: fn(&str) -> Value,
    /// The `UpdateDestination` member carrying the update.
    update_member: &'static str,
    /// An update re-pointing THIS row's member at the bucket ARN given.
    update: fn(&str) -> Value,
    /// Where `DescribeDeliveryStream` reports the bucket this row re-points, inside the
    /// stream's sole `DestinationDescription`.
    bucket_path: &'static str,
}

/// The `*DestinationUpdate` members that carry a bucket and are not already a row of
/// `destination_bucket_preflight.rs` (which covers `S3DestinationUpdate`,
/// `ExtendedS3DestinationUpdate`, its `S3BackupUpdate`, and a Splunk `S3Update`).
fn member_cases() -> Vec<MemberCase> {
    vec![
        MemberCase {
            stream: "resolution-redshift",
            config_member: "RedshiftDestinationConfiguration",
            config: |bucket| {
                json!({
                    "RoleARN": ROLE_ARN,
                    "ClusterJDBCURL":
                        "jdbc:redshift://example-cluster.abc123.us-west-2.redshift.amazonaws.com:5439/dev",
                    "CopyCommand": { "DataTableName": "events" },
                    "Username": "analytics",
                    "Password": "sup3r-secret-pw",
                    "S3Configuration": s3_configuration(bucket),
                })
            },
            update_member: "RedshiftDestinationUpdate",
            update: |arn| json!({ "S3Update": { "BucketARN": arn } }),
            bucket_path: "RedshiftDestinationDescription/S3DestinationDescription/BucketARN",
        },
        MemberCase {
            stream: "resolution-http-endpoint",
            config_member: "HttpEndpointDestinationConfiguration",
            config: |bucket| {
                json!({
                    "EndpointConfiguration": {
                        "Url": "https://example.com/firehose",
                        "Name": "example",
                    },
                    "S3Configuration": s3_configuration(bucket),
                })
            },
            update_member: "HttpEndpointDestinationUpdate",
            update: |arn| json!({ "S3Update": { "BucketARN": arn } }),
            bucket_path: "HttpEndpointDestinationDescription/S3DestinationDescription/BucketARN",
        },
        MemberCase {
            stream: "resolution-iceberg",
            config_member: "IcebergDestinationConfiguration",
            config: |bucket| {
                json!({
                    "RoleARN": ROLE_ARN,
                    "CatalogConfiguration": {
                        "CatalogARN": format!("arn:aws:glue:{REGION}:000000000000:catalog"),
                    },
                    "AppendOnly": true,
                    "S3Configuration": s3_configuration(bucket),
                })
            },
            update_member: "IcebergDestinationUpdate",
            // `IcebergDestinationUpdate` carries a whole `S3Configuration`, not an update,
            // so here the bucket travels in a REQUIRED member of a replaced configuration.
            update: |arn| json!({ "S3Configuration": { "RoleARN": ROLE_ARN, "BucketARN": arn } }),
            bucket_path: "IcebergDestinationDescription/S3DestinationDescription/BucketARN",
        },
    ]
}

/// Bucket ARNs naming a bucket this account does not hold, differing along the FORM of the
/// name: a near miss of the held bucket's name (so acceptance cannot be a prefix match), a
/// name that EXTENDS the held one (so it cannot be a substring match either), an absent
/// bucket in another partition, a dotted name, and the longest name an S3 general purpose
/// bucket has (63 characters). Every name here is one S3 would accept as a bucket name, so
/// "not there" is the only thing a row varies.
fn absent_buckets() -> Vec<(&'static str, String)> {
    vec![
        (
            "a near miss of the held bucket's name",
            bucket_arn(&HELD[..HELD.len() - 1]),
        ),
        (
            "a name the held one is a prefix of",
            bucket_arn(&format!("{HELD}-absent")),
        ),
        (
            "an absent bucket in another partition",
            "arn:aws-us-gov:s3:::resolution-absent".to_string(),
        ),
        ("a dotted name", bucket_arn("my.absent.resolution-01")),
        (
            "the longest name an S3 bucket has",
            bucket_arn(&"a".repeat(63)),
        ),
    ]
}

/// Call `UpdateDestination` with `row`'s member re-pointed at `bucket_arn`.
async fn update(
    emulator: &Emulator,
    row: &MemberCase,
    version: &str,
    destination_id: &str,
    bucket_arn: &str,
) -> (u16, Value) {
    let mut request = json!({
        "DeliveryStreamName": row.stream,
        "CurrentDeliveryStreamVersionId": version,
        "DestinationId": destination_id,
    });
    request[row.update_member] = (row.update)(bucket_arn);
    emulator.firehose("UpdateDestination", request).await
}

#[tokio::test]
async fn every_bucket_member_is_resolved_whatever_form_the_absent_name_takes() {
    let emulator = Emulator::new();
    for bucket in [HELD, TARGET] {
        emulator.create_bucket(bucket).await;
    }

    let rows = member_cases();
    // Every stream is created first, so one wait covers the clock-owed CREATING -> ACTIVE
    // span: only an ACTIVE stream's destination can be updated.
    for row in &rows {
        let mut request = json!({ "DeliveryStreamName": row.stream });
        request[row.config_member] = (row.config)(HELD);
        emulator.firehose_ok("CreateDeliveryStream", request).await;
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for row in &rows {
        let (version, destination_id, _) = version_and_destination(&emulator, row.stream).await;

        for (case, absent) in absent_buckets() {
            let (status, refused) = update(&emulator, row, &version, &destination_id, &absent).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "InvalidArgumentException"),
                "case {case:?} on {}: expected 400 InvalidArgumentException, got {status} \
                 with {refused}",
                row.stream
            );
            assert_eq!(
                error_message(&refused),
                format!("S3 Bucket {} does not exist.", bucket_name(&absent)),
                "case {case:?} on {}: expected the refusal to name the bucket the request \
                 asked for, got {refused}",
                row.stream
            );

            // The refusal changed nothing: neither the version nor the stored destination
            // moved, so the destination that was working is still the one configured.
            let (after_version, _, after) = version_and_destination(&emulator, row.stream).await;
            assert_eq!(
                after_version, version,
                "case {case:?} on {}: a refused update must not advance the version, \
                 got {after}",
                row.stream
            );
            assert_eq!(
                at(&after["Destinations"][0], row.bucket_path),
                &json!(bucket_arn(HELD)),
                "case {case:?} on {}: a refused update must leave the stored bucket at \
                 {}, got {after}",
                row.stream,
                bucket_arn(HELD)
            );
        }

        // And the same member re-pointed at a bucket Amazon S3 DOES hold is served, so the
        // refusals above are the answer to an absent bucket rather than to the member:
        // "after the update is applied successfully, the version ID is updated".
        let target = bucket_arn(TARGET);
        let (status, applied) = update(&emulator, row, &version, &destination_id, &target).await;
        assert_eq!(
            status, 200,
            "case a-bucket-the-account-holds on {}: expected 200, got {status} with {applied}",
            row.stream
        );
        let (after_version, _, after) = version_and_destination(&emulator, row.stream).await;
        assert_ne!(
            after_version, version,
            "case a-bucket-the-account-holds on {}: the applied update must move the \
             version on from {version:?}, got {after}",
            row.stream
        );
        assert_eq!(
            at(&after["Destinations"][0], row.bucket_path),
            &json!(target),
            "case a-bucket-the-account-holds on {}: the stored destination must deliver \
             to {target}, got {after}",
            row.stream
        );
    }
}
