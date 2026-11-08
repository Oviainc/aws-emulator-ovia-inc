// SPDX-License-Identifier: CC0-1.0

//! What `PutTableBucketReplication` reports about a put it accepted.
//!
//! The rule under test: the required `status` member of the response reports the
//! outcome of the CALL — the model documents it as "The status of the
//! replication configuration operation"
//! (`smithy_query(query="doc", shape="PutTableBucketReplicationResponse",
//! member="status")`) — so it is the same literal for every accepted put and is
//! independent of the configuration the put left behind. A put that is NOT
//! accepted carries a modelled error and no output at all, which is what the
//! refused rows below assert.
//!
//! The table spans what the configuration can differ in — how many destinations
//! the rule names (`TableBucketReplicationRule.destinations` is `@length(1, 5)`,
//! and the rule list itself `@length(1, 1)`), whether the put creates the
//! configuration or replaces one, and whether it presents a version token — so
//! a status read off the stored configuration rather than off the call could not
//! pass it.

use serde_json::json;

use crate::harness::Emulator;

const REGION: &str = "us-east-1";

/// The outcome literal an accepted put reports.
const SUCCESS: &str = "success";

/// The IAM role a replication configuration names.
fn role() -> String {
    format!(
        "arn:aws:iam::{account}:role/s3tables-replication",
        account = crate::harness::ACCOUNT
    )
}

/// A configuration whose one rule — the rule list is `@length(1, 1)` — replicates
/// to `destinations`.
fn configuration(destinations: &[String]) -> serde_json::Value {
    json!({
        "role": role(),
        "rules": [{
            "destinations": destinations
                .iter()
                .map(|arn| json!({ "destinationTableBucketARN": arn }))
                .collect::<Vec<_>>(),
        }],
    })
}

#[tokio::test]
async fn every_accepted_put_reports_the_calls_own_outcome() {
    let emulator = Emulator::new();
    let source = emulator
        .create_table_bucket(REGION, &json!({ "name": "replication-outcome-source" }))
        .await
        .text("arn");
    let first = emulator
        .create_table_bucket(REGION, &json!({ "name": "replication-outcome-dest-1" }))
        .await
        .text("arn");
    let second = emulator
        .create_table_bucket(REGION, &json!({ "name": "replication-outcome-dest-2" }))
        .await
        .text("arn");
    let third = emulator
        .create_table_bucket(REGION, &json!({ "name": "replication-outcome-dest-3" }))
        .await
        .text("arn");

    // Each row is applied in turn to the SAME bucket, so the second row onward
    // replaces a configuration that is already there: the outcome cannot depend
    // on which state the put left.
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("one destination", vec![first.clone()]),
        ("two destinations", vec![first.clone(), second.clone()]),
        (
            "three destinations",
            vec![first.clone(), second.clone(), third.clone()],
        ),
        ("back to one, the other destination", vec![second.clone()]),
    ];

    let mut token: Option<String> = None;
    for (label, destinations) in rows {
        // Half the rows present the token the previous put handed back, which is
        // the precondition form of the same call.
        let query: Vec<(&str, String)> = match &token {
            Some(held) => vec![
                ("tableBucketARN", source.clone()),
                ("versionToken", held.clone()),
            ],
            None => vec![("tableBucketARN", source.clone())],
        };
        let put = emulator
            .put_table_bucket_replication(REGION, &query, &configuration(&destinations))
            .await;
        assert_eq!(
            put.status, 200,
            "case {label:?}: the put must be served, got {status} {body}",
            status = put.status,
            body = put.body
        );
        assert_eq!(
            put.text("status"),
            SUCCESS,
            "case {label:?}: expected the call's own outcome, got {actual:?}",
            actual = put.text("status")
        );

        // The configuration the put stored is what the read reports, and it
        // carries no status of its own: the status belongs to the call.
        let read = emulator.get_table_bucket_replication(REGION, &source).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the read must be served, got {status} {body}",
            status = read.status,
            body = read.body
        );
        let configured = read
            .body
            .get("configuration")
            .expect("a stored configuration is reported");
        assert_eq!(
            configured
                .get("rules")
                .and_then(serde_json::Value::as_array)
                .and_then(|rules| rules.first())
                .and_then(|rule| rule.get("destinations"))
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(destinations.len()),
            "case {label:?}: the read must report the destinations the put stored, got \
             {configured}"
        );
        assert!(
            configured.get("status").is_none(),
            "case {label:?}: the configuration carries no status of its own, got {configured}"
        );
        token = Some(read.text("versionToken"));
    }
}

#[tokio::test]
async fn a_put_that_is_not_accepted_reports_no_outcome_at_all() {
    let emulator = Emulator::new();
    let source = emulator
        .create_table_bucket(REGION, &json!({ "name": "replication-outcome-refused" }))
        .await
        .text("arn");
    let destination = emulator
        .create_table_bucket(REGION, &json!({ "name": "replication-outcome-refused-dest" }))
        .await
        .text("arn");
    let settings = configuration(&[destination]);

    // A bucket the store does not hold, and a token that preconditions a
    // configuration that is not there: neither put is accepted, so neither
    // answers an outcome.
    let absent = crate::harness::expected_bucket_arn(REGION, "replication-outcome-absent");
    let refused = emulator
        .put_table_bucket_replication(REGION, &[("tableBucketARN", absent)], &settings)
        .await;
    assert_eq!(
        refused.status, 404,
        "case \"a bucket that was never created\": expected status 404, got {status} {body}",
        status = refused.status,
        body = refused.body
    );
    assert!(
        refused.optional_text("status").is_none(),
        "case \"a bucket that was never created\": a refused put reports no outcome, got {body}",
        body = refused.body
    );

    let refused = emulator
        .put_table_bucket_replication(
            REGION,
            &[
                ("tableBucketARN", source.clone()),
                ("versionToken", "0".to_string()),
            ],
            &settings,
        )
        .await;
    assert_eq!(
        refused.status, 404,
        "case \"a token for a configuration that is not there\": expected status 404, got \
         {status} {body}",
        status = refused.status,
        body = refused.body
    );
    assert!(
        refused.optional_text("status").is_none(),
        "case \"a token for a configuration that is not there\": a refused put reports no \
         outcome, got {body}",
        body = refused.body
    );
}
