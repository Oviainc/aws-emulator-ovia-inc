// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of HOW `GetTableBucketMetricsConfiguration` states a
//! read of a table bucket that carries no metrics configuration.
//!
//! The rule under test: when the addressed table bucket is resident but the
//! sub-configuration the read reports is not set, the refusal carries the fixed
//! sentence for THAT condition — "No such metric configuration exists" — and not
//! the sentence for the addressed bucket being absent; the two 404s this one
//! operation states are therefore told apart by `message`, the only
//! discriminator the single modelled `NotFoundException` carries. The sentence
//! is wholly fixed: it interpolates no name, ARN or id, so it does not depend on
//! which bucket was addressed, nor on HOW the configuration came to be absent
//! (never set, or set and removed again, any number of times).
//!
//! Each case drives the real wire path through the harness, so what is asserted
//! is what an AWS SDK client would see. The table spans what the rule
//! generalises over: the position the read is taken from (the sequence of puts
//! and deletes that precedes it), the region the bucket lives in, and the shape
//! and length of its name — the published `TableBucketName` bounds and the
//! character classes its modelled pattern allows.

use serde_json::json;

use crate::harness::{expected_bucket_arn, Emulator, Reply};

/// The sentence this service states a table bucket that carries no metrics
/// configuration with. Wholly fixed text: that is what the cases below assert.
const NO_METRICS_CONFIGURATION: &str = "No such metric configuration exists";

/// The sentence the SAME operation states a table bucket the store does not
/// hold with, kept here so the two 404s are asserted to be distinguishable.
const NO_BUCKET: &str = "The specified bucket does not exist.";

/// One step taken against the bucket's metrics configuration before the read.
#[derive(Debug, Clone, Copy)]
enum Step {
    Put,
    Delete,
}

/// One position a read that finds no configuration can be taken from: the steps
/// that precede it.
#[derive(Debug, Clone, Copy)]
struct Position {
    label: &'static str,
    steps: &'static [Step],
}

/// The positions the rule has to hold in: before any put at all, after the
/// configuration has been set and removed, after the removal has been repeated,
/// and at the intermediate read of a set/remove/set/remove cycle.
fn positions() -> &'static [Position] {
    &[
        Position {
            label: "before any put",
            steps: &[],
        },
        Position {
            label: "after a delete of a bucket that never had a configuration",
            steps: &[Step::Delete],
        },
        Position {
            label: "after a put and a delete",
            steps: &[Step::Put, Step::Delete],
        },
        Position {
            label: "after a put and the delete repeated",
            steps: &[Step::Put, Step::Delete, Step::Delete],
        },
        Position {
            label: "at the intermediate read of a second put/delete cycle",
            steps: &[Step::Put, Step::Delete, Step::Put, Step::Delete],
        },
    ]
}

/// `TableBucketName` is `@length(3, 63)` and `@pattern(^[0-9a-z-]*$)`: the rows
/// below take the shortest and the longest names those bounds publish, and one
/// of each character class the pattern allows. `index` keeps the name of each
/// position's own bucket distinct without leaving the bounds.
fn shortest_name(index: usize) -> String {
    // The shortest name the published `@length` lower bound allows.
    format!("m{index}t")
}

fn longest_name(index: usize) -> String {
    // 63 characters, the published `@length` upper bound.
    format!("metrics-{}-{index}", "x".repeat(53))
}

fn digits_and_hyphens_name(index: usize) -> String {
    format!("7-metrics-probe-{index}-3")
}

/// One row: a region, and how the bucket's name is built for a position.
struct Row {
    label: &'static str,
    region: &'static str,
    name: fn(usize) -> String,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "shortest published name",
            region: "us-east-1",
            name: shortest_name,
        },
        Row {
            label: "longest published name",
            region: "eu-west-1",
            name: longest_name,
        },
        Row {
            label: "digits and hyphens",
            region: "ap-south-1",
            name: digits_and_hyphens_name,
        },
    ]
}

/// Assert `reply` is the modelled 404 `expected_message` states, and that the
/// sentence took nothing from the request: none of `values` appears in it.
fn assert_not_found(reply: &Reply, expected_message: &str, values: &[&str], case: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected status 404, got {actual} {body}",
        actual = reply.status,
        body = reply.body
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("NotFoundException"),
        "case {case:?}: expected the modelled NotFoundException, got {actual:?}",
        actual = reply.error_code
    );
    assert_eq!(
        reply.message(),
        expected_message,
        "case {case:?}: expected the fixed sentence {expected_message:?}, got {actual:?}",
        actual = reply.message()
    );
    for value in values {
        assert!(
            !reply.message().contains(value),
            "case {case:?}: the sentence must carry nothing from the request, but it carries \
             {value:?}: {actual:?}",
            actual = reply.message()
        );
    }
}

#[tokio::test]
async fn a_bucket_with_no_metrics_configuration_is_refused_with_the_fixed_sentence() {
    let emulator = Emulator::new();

    for row in rows() {
        for (index, position) in positions().iter().enumerate() {
            let case = format!("{row} / {position}", row = row.label, position = position.label);
            let name = (row.name)(index);
            let created = emulator
                .create_table_bucket(row.region, &json!({ "name": name }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the create must be served, got {status} {body}",
                status = created.status,
                body = created.body
            );
            let arn = created.text("arn");

            for (step_index, step) in position.steps.iter().enumerate() {
                let reply = match step {
                    Step::Put => {
                        emulator
                            .put_table_bucket_metrics_configuration(row.region, &arn)
                            .await
                    }
                    Step::Delete => {
                        emulator
                            .delete_table_bucket_metrics_configuration(row.region, &arn)
                            .await
                    }
                };
                assert_eq!(
                    reply.status, 204,
                    "case {case:?}: step {step_index} ({step:?}) must be served, got {status} \
                     {body}",
                    status = reply.status,
                    body = reply.body
                );
            }

            // The read states the configuration's own not-found, naming nothing
            // the request carried — and a second read states the same, because a
            // read reports and does not write.
            for attempt in 1..=2 {
                let read = emulator
                    .get_table_bucket_metrics_configuration(row.region, &arn)
                    .await;
                let case = format!("{case} / read {attempt}");
                assert_not_found(
                    &read,
                    NO_METRICS_CONFIGURATION,
                    &[name.as_str(), row.region, arn.as_str()],
                    &case,
                );
            }

            // The bucket itself is still there, so the refusal above was about
            // the configuration and not about the bucket.
            let bucket = emulator.get_table_bucket(row.region, &arn).await;
            assert_eq!(
                bucket.status, 200,
                "case {case:?}: the addressed bucket must still read, got {status} {body}",
                status = bucket.status,
                body = bucket.body
            );
        }
    }
}

#[tokio::test]
async fn the_absent_configuration_and_the_absent_bucket_are_told_apart_by_the_message() {
    let emulator = Emulator::new();

    // The two sentences are different text: a client branching on `message` can
    // tell "metrics are off" from "no such bucket".
    assert_ne!(
        NO_METRICS_CONFIGURATION, NO_BUCKET,
        "the two conditions this operation states 404 for must not read alike"
    );

    for (index, row) in rows().into_iter().enumerate() {
        let name = (row.name)(index);
        let absent = expected_bucket_arn(row.region, &name);

        // Nothing created yet: the ARN is well formed and names no bucket, so
        // the read states the BUCKET's sentence.
        let read = emulator
            .get_table_bucket_metrics_configuration(row.region, &absent)
            .await;
        assert_not_found(
            &read,
            NO_BUCKET,
            &[name.as_str(), absent.as_str()],
            &format!("{label} / a bucket that was never created", label = row.label),
        );

        // With the bucket resident and its metrics off, the same request states
        // the CONFIGURATION's sentence instead.
        let created = emulator
            .create_table_bucket(row.region, &json!({ "name": name }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the create must be served, got {status} {body}",
            label = row.label,
            status = created.status,
            body = created.body
        );
        let arn = created.text("arn");
        let read = emulator
            .get_table_bucket_metrics_configuration(row.region, &arn)
            .await;
        assert_not_found(
            &read,
            NO_METRICS_CONFIGURATION,
            &[name.as_str(), arn.as_str()],
            &format!("{label} / a resident bucket with metrics off", label = row.label),
        );

        // A put turns metrics on, so the read is served; the delete turns them
        // off again and the configuration's sentence comes back.
        let put = emulator
            .put_table_bucket_metrics_configuration(row.region, &arn)
            .await;
        assert_eq!(
            put.status, 204,
            "case {label:?}: the put must be served, got {status} {body}",
            label = row.label,
            status = put.status,
            body = put.body
        );
        let served = emulator
            .get_table_bucket_metrics_configuration(row.region, &arn)
            .await;
        assert_eq!(
            served.status, 200,
            "case {label:?}: the read must be served once metrics are on, got {status} {body}",
            label = row.label,
            status = served.status,
            body = served.body
        );
        let deleted = emulator
            .delete_table_bucket_metrics_configuration(row.region, &arn)
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {label:?}: the delete must be served, got {status} {body}",
            label = row.label,
            status = deleted.status,
            body = deleted.body
        );
        let read = emulator
            .get_table_bucket_metrics_configuration(row.region, &arn)
            .await;
        assert_not_found(
            &read,
            NO_METRICS_CONFIGURATION,
            &[name.as_str(), arn.as_str(), &served.text("id")],
            &format!("{label} / after the delete", label = row.label),
        );

        // And once the bucket is gone, the bucket's sentence is what the same
        // request states again.
        let dropped = emulator.delete_table_bucket(row.region, &arn).await;
        assert_eq!(
            dropped.status, 204,
            "case {label:?}: deleting the bucket must be served, got {status} {body}",
            label = row.label,
            status = dropped.status,
            body = dropped.body
        );
        let read = emulator
            .get_table_bucket_metrics_configuration(row.region, &arn)
            .await;
        assert_not_found(
            &read,
            NO_BUCKET,
            &[name.as_str(), arn.as_str()],
            &format!("{label} / after the bucket was deleted", label = row.label),
        );
    }
}
