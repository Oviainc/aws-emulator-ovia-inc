// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the table-bucket replication configuration: the
//! put that sets one, the read that reports it and the delete that removes it.
//!
//! The rule under test: a table bucket has a replication configuration only
//! once a put sets one on it, so both the read and the delete of a bucket that
//! has none are refused with the modeled not-found and the one sentence this
//! service states that class with. A put persists the whole configuration under
//! a freshly minted version token and reports it; the read then reports exactly
//! what the store holds for THAT bucket — its role, its rules' destinations and
//! its version token — and changes nothing a later caller sees; and a request
//! that presents a `versionToken` is a precondition on the configuration the
//! caller believes is there: a bucket with none refuses it as not-found, a token
//! that is not the stored one is the modeled conflict and leaves the
//! configuration in place, and the current token is accepted.
//!
//! The table spans what the rule generalises over: the region the bucket lives
//! in, the role the configuration names, how many destinations its one rule
//! carries (the model bounds a rule at 1..5 of them and the rule list at
//! exactly one), and whether the delete presents the configuration's version
//! token or none at all.

use serde_json::{json, Value};
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;

use s3tables::state::{
    ComAmazonawsS3tablesS3tablebucketsState as S3TablesState, ReplicationConfiguration,
};

use super::harness::{expected_bucket_arn, Emulator, Reply};

/// The fixed sentence this service states a table bucket that has no
/// replication configuration with.
const NO_REPLICATION_CONFIGURATION: &str = "The specified replication configuration does not exist.";

/// The fixed sentence this service states a table bucket the store does not
/// hold with.
const NO_BUCKET: &str = "The specified bucket does not exist.";

/// The fixed sentence this service states a presented version token that is not
/// the configuration's with.
const STALE_VERSION_TOKEN: &str = "The specified version token does not match the configuration.";

/// One row: a table bucket whose replication configuration names `role` and the
/// destination buckets `destinations`, deleted the way `delete_with_token` says.
#[derive(Debug)]
struct Case {
    label: &'static str,
    region: &'static str,
    name: &'static str,
    /// An IAM role ARN of the form the modeled `IAMRole` pattern accepts
    /// (`^arn:.+:iam::[0-9]{12}:role/.+$`, length 20..2048).
    role: &'static str,
    /// The names of the destination table buckets the one rule replicates to.
    /// `ReplicationDestinations` is `@length(min: 1, max: 5)`, so a row carries
    /// between one and five.
    destinations: &'static [&'static str],
    /// Whether the delete that succeeds presents the configuration's version
    /// token (the member is optional on the wire).
    delete_with_token: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one destination, no token on the delete",
            region: "us-east-1",
            name: "replication-source-east",
            role: "arn:aws:iam::000000000000:role/s3tables-replication",
            destinations: &["replication-target-east"],
            delete_with_token: false,
        },
        Case {
            label: "one destination, the delete presents the token",
            region: "eu-west-1",
            name: "replication-source-west",
            role: "arn:aws:iam::000000000000:role/another-replication-role",
            destinations: &["replication-target-west"],
            delete_with_token: true,
        },
        Case {
            label: "two destinations",
            region: "us-west-2",
            name: "replication-source-two",
            role: "arn:aws-us-gov:iam::000000000000:role/path/to/replication",
            destinations: &["replication-target-two-a", "replication-target-two-b"],
            delete_with_token: true,
        },
        Case {
            // The most destinations one rule can carry.
            label: "five destinations, the modeled maximum",
            region: "ap-south-1",
            name: "replication-source-five",
            role: "arn:aws:iam::000000000000:role/five-destination-replication",
            destinations: &[
                "replication-target-five-a",
                "replication-target-five-b",
                "replication-target-five-c",
                "replication-target-five-d",
                "replication-target-five-e",
            ],
            delete_with_token: false,
        },
    ]
}

/// The destination ARNs of `case`, in the order its rule carries them.
fn destination_arns(case: &Case) -> Vec<String> {
    case.destinations
        .iter()
        .map(|name| expected_bucket_arn(case.region, name))
        .collect()
}

/// A `TableBucketReplicationConfiguration` naming `role` and `destinations`.
fn configuration(role: &str, destinations: &[String]) -> Value {
    json!({
        "role": role,
        "rules": [{
            "destinations": destinations
                .iter()
                .map(|arn| json!({ "destinationTableBucketARN": arn }))
                .collect::<Vec<Value>>(),
        }],
    })
}

/// The replication configuration the store holds for `arn`.
fn stored_configuration(emulator: &Emulator, arn: &str) -> Option<ReplicationConfiguration> {
    let store = emulator.engine.state::<S3TablesState>();
    let store = store.lock();
    store
        .table_buckets
        .get(arn)
        .and_then(|bucket| bucket.replication.clone())
}

/// Assert that `reply` is the 404 `NotFoundException` this service words with
/// `message`.
fn assert_not_found(reply: &Reply, message: &str, case: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected 404, got {status} {body}",
        status = reply.status,
        body = reply.body
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("NotFoundException"),
        "case {case:?}: expected NotFoundException, got {actual:?}",
        actual = reply.error_code
    );
    assert_eq!(
        reply.message(),
        message,
        "case {case:?}: expected the fixed sentence {message:?}, got {actual:?}",
        actual = reply.message()
    );
}

/// The version token a put reported, with the assertion that the put succeeded
/// and answered the shape the model declares.
fn accepted_token(reply: &Reply, case: &str) -> String {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the put must succeed, got {status} {body}",
        status = reply.status,
        body = reply.body
    );
    let status = reply.text("status");
    assert!(
        !status.is_empty(),
        "case {case:?}: the put must report a status, got {status:?}"
    );
    let token = reply.text("versionToken");
    assert!(
        (1..=2048).contains(&token.chars().count()),
        "case {case:?}: the version token must be of the modeled length, got {token:?}"
    );
    token
}

/// The `role`, the destination ARNs and the `versionToken` a read reported,
/// with the assertion that the response is the shape the model declares.
fn reported_configuration(reply: &Reply, case: &str) -> (String, Vec<String>, String) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the read must succeed, got {status} {body}",
        status = reply.status,
        body = reply.body
    );
    let configuration = reply.body.get("configuration").cloned().unwrap_or_else(|| {
        panic!(
            "case {case:?}: the read has no configuration: {body}",
            body = reply.body
        )
    });
    let role = configuration
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the configuration has no role: {configuration}"))
        .to_string();
    let rules = configuration
        .get("rules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the configuration has no rules: {configuration}"));
    assert_eq!(
        rules.len(),
        1,
        "case {case:?}: the model bounds the rule list at exactly one, got {rules:?}"
    );
    let destinations: Vec<String> = rules[0]
        .get("destinations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the rule has no destinations: {rules:?}"))
        .iter()
        .map(|destination| {
            destination
                .get("destinationTableBucketARN")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a destination has no ARN: {destination}"))
                .to_string()
        })
        .collect();
    (role, destinations, reply.text("versionToken"))
}

#[tokio::test]
async fn a_put_sets_a_configuration_the_read_reports_and_the_delete_removes() {
    let emulator = Emulator::new();

    for case in cases() {
        let created = emulator
            .create_table_bucket(case.region, &json!({ "name": case.name }))
            .await;
        assert_eq!(
            created.status,
            200,
            "case {label:?}: the create must succeed, got {status} {body}",
            label = case.label,
            status = created.status,
            body = created.body
        );
        let arn = created.text("arn");
        // The destination buckets of the row, so its configuration names table
        // buckets that exist.
        let destinations = destination_arns(&case);
        for destination in case.destinations {
            let created = emulator
                .create_table_bucket(case.region, &json!({ "name": destination }))
                .await;
            assert_eq!(
                created.status,
                200,
                "case {label:?}: creating the destination {destination:?} must succeed, got \
                 {status} {body}",
                label = case.label,
                status = created.status,
                body = created.body
            );
        }
        let configuration = configuration(case.role, &destinations);

        // Replication is not configured until a put sets a configuration, so
        // neither the read nor the delete has anything to work on, and a put
        // that preconditions on a version token has nothing to precondition.
        let before = emulator.get_table_bucket_replication(case.region, &arn).await;
        assert_not_found(&before, NO_REPLICATION_CONFIGURATION, case.label);
        let delete_before = emulator
            .delete_table_bucket_replication(case.region, &[("tableBucketARN", arn.clone())])
            .await;
        assert_not_found(&delete_before, NO_REPLICATION_CONFIGURATION, case.label);
        let conditional_put = emulator
            .put_table_bucket_replication(
                case.region,
                &[
                    ("tableBucketARN", arn.clone()),
                    ("versionToken", "a-token-no-configuration-carries".to_string()),
                ],
                &configuration,
            )
            .await;
        assert_not_found(&conditional_put, NO_REPLICATION_CONFIGURATION, case.label);
        assert_eq!(
            stored_configuration(&emulator, &arn),
            None,
            "case {label:?}: the refused put must persist nothing",
            label = case.label
        );

        let put = emulator
            .put_table_bucket_replication(
                case.region,
                &[("tableBucketARN", arn.clone())],
                &configuration,
            )
            .await;
        let version_token = accepted_token(&put, case.label);

        // The read reports the record, not the request: role, destinations and
        // the token the put minted all come back as the store holds them.
        let read = emulator.get_table_bucket_replication(case.region, &arn).await;
        let (role, reported_destinations, reported_token) =
            reported_configuration(&read, case.label);
        assert_eq!(
            role, case.role,
            "case {label:?}: expected role {expected:?}, got {role:?}",
            label = case.label,
            expected = case.role
        );
        assert_eq!(
            reported_destinations, destinations,
            "case {label:?}: expected destinations {destinations:?}, got \
             {reported_destinations:?}",
            label = case.label
        );
        assert_eq!(
            reported_token, version_token,
            "case {label:?}: expected versionToken {version_token:?}, got {reported_token:?}",
            label = case.label
        );

        // A read reports, it does not write: a second read answers the same and
        // the store still holds exactly what the put persisted.
        let again = emulator.get_table_bucket_replication(case.region, &arn).await;
        assert_eq!(
            reported_configuration(&again, case.label),
            (
                role.clone(),
                reported_destinations.clone(),
                version_token.clone()
            ),
            "case {label:?}: two reads must report the same configuration",
            label = case.label
        );
        let persisted = stored_configuration(&emulator, &arn);
        assert_eq!(
            persisted
                .as_ref()
                .map(|configuration| configuration.role.as_str()),
            Some(case.role),
            "case {label:?}: the store must hold the configuration the put wrote, got \
             {persisted:?}",
            label = case.label
        );

        // A put presenting a token that is not the configuration's is the
        // modeled conflict, and it writes nothing.
        let conflicted = emulator
            .put_table_bucket_replication(
                case.region,
                &[
                    ("tableBucketARN", arn.clone()),
                    ("versionToken", format!("{version_token}-stale")),
                ],
                &configuration,
            )
            .await;
        assert_conflict(&conflicted, case.label, "PutTableBucketReplication");
        assert_eq!(
            stored_configuration(&emulator, &arn),
            persisted,
            "case {label:?}: the refused put must leave the configuration as it was",
            label = case.label
        );

        // The current token IS accepted, and the write advances it.
        let advanced = emulator
            .put_table_bucket_replication(
                case.region,
                &[
                    ("tableBucketARN", arn.clone()),
                    ("versionToken", version_token.clone()),
                ],
                &configuration,
            )
            .await;
        let advanced_token = accepted_token(&advanced, case.label);
        assert_ne!(
            advanced_token, version_token,
            "case {label:?}: a write must mint a fresh version token",
            label = case.label
        );
        let (_, _, reported_token) = reported_configuration(
            &emulator.get_table_bucket_replication(case.region, &arn).await,
            case.label,
        );
        assert_eq!(
            reported_token, advanced_token,
            "case {label:?}: the read must report the token of the last write",
            label = case.label
        );

        // A delete presenting a token that is not the configuration's is the
        // same conflict, and it deletes nothing.
        let conflicted = emulator
            .delete_table_bucket_replication(
                case.region,
                &[
                    ("tableBucketARN", arn.clone()),
                    ("versionToken", version_token.clone()),
                ],
            )
            .await;
        assert_conflict(&conflicted, case.label, "DeleteTableBucketReplication");
        assert!(
            stored_configuration(&emulator, &arn).is_some(),
            "case {label:?}: the refused delete must leave the configuration in place",
            label = case.label
        );

        // The delete the row names removes the configuration.
        let mut query = vec![("tableBucketARN", arn.clone())];
        if case.delete_with_token {
            query.push(("versionToken", advanced_token.clone()));
        }
        let deleted = emulator
            .delete_table_bucket_replication(case.region, &query)
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {label:?}: the delete must answer the modeled 204, got {status} {body}",
            label = case.label,
            status = deleted.status,
            body = deleted.body
        );
        assert_eq!(
            deleted.body,
            Value::Null,
            "case {label:?}: the delete must carry no body, got {body}",
            label = case.label,
            body = deleted.body
        );
        assert_eq!(
            stored_configuration(&emulator, &arn),
            None,
            "case {label:?}: the delete must remove the configuration from the store",
            label = case.label
        );

        // And once it is gone every call answers as it did before it existed.
        let after = emulator.get_table_bucket_replication(case.region, &arn).await;
        assert_not_found(&after, NO_REPLICATION_CONFIGURATION, case.label);
        let delete_again = emulator
            .delete_table_bucket_replication(case.region, &[("tableBucketARN", arn.clone())])
            .await;
        assert_not_found(&delete_again, NO_REPLICATION_CONFIGURATION, case.label);
    }
}

/// Assert that `reply` is the 409 `ConflictException` a stale version token is
/// refused with.
fn assert_conflict(reply: &Reply, case: &str, operation: &str) {
    assert_eq!(
        reply.status, 409,
        "case {case:?} on {operation}: expected 409 for a stale token, got {status} {body}",
        status = reply.status,
        body = reply.body
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("ConflictException"),
        "case {case:?} on {operation}: expected ConflictException, got {actual:?}",
        actual = reply.error_code
    );
    assert_eq!(
        reply.message(),
        STALE_VERSION_TOKEN,
        "case {case:?} on {operation}: expected the fixed stale-token sentence, got {actual:?}",
        actual = reply.message()
    );
}

#[tokio::test]
async fn a_configuration_belongs_to_the_bucket_it_was_set_on() {
    let emulator = Emulator::new();

    // Two buckets of the same name in different regions, and a third in the
    // first region: exactly one of them is configured below.
    let configured = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "replication-scoped" }))
        .await
        .text("arn");
    let same_name_other_region = emulator
        .create_table_bucket("eu-west-1", &json!({ "name": "replication-scoped" }))
        .await
        .text("arn");
    let sibling = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "replication-unconfigured" }))
        .await
        .text("arn");

    let destinations = vec![same_name_other_region.clone()];
    let put = emulator
        .put_table_bucket_replication(
            "us-east-1",
            &[("tableBucketARN", configured.clone())],
            &configuration(
                "arn:aws:iam::000000000000:role/scoped-replication",
                &destinations,
            ),
        )
        .await;
    let version_token = accepted_token(&put, "the configured bucket");

    let read = emulator
        .get_table_bucket_replication("us-east-1", &configured)
        .await;
    let (_, reported_destinations, reported_token) =
        reported_configuration(&read, "the configured bucket");
    assert_eq!(
        reported_destinations, destinations,
        "case \"the configured bucket\": expected destinations {destinations:?}, got \
         {reported_destinations:?}"
    );
    assert_eq!(
        reported_token, version_token,
        "case \"the configured bucket\": expected versionToken {version_token:?}, got \
         {reported_token:?}"
    );

    for (label, region, arn) in [
        (
            "the same name in another region",
            "eu-west-1",
            &same_name_other_region,
        ),
        ("a sibling bucket in the same region", "us-east-1", &sibling),
    ] {
        assert_not_found(
            &emulator.get_table_bucket_replication(region, arn).await,
            NO_REPLICATION_CONFIGURATION,
            label,
        );
        assert_not_found(
            &emulator
                .delete_table_bucket_replication(region, &[("tableBucketARN", arn.to_string())])
                .await,
            NO_REPLICATION_CONFIGURATION,
            label,
        );
    }

    // A configuration does not outlive the bucket it was set on: a bucket
    // recreated under the same name replicates nothing until its own put.
    let deleted = emulator.delete_table_bucket("us-east-1", &configured).await;
    assert_eq!(
        deleted.status, 204,
        "case \"the configured bucket\": the bucket delete must succeed, got {status} {body}",
        status = deleted.status,
        body = deleted.body
    );
    let recreated = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "replication-scoped" }))
        .await;
    assert_eq!(
        recreated.status, 200,
        "case \"the recreated bucket\": the create must succeed, got {status} {body}",
        status = recreated.status,
        body = recreated.body
    );
    assert_not_found(
        &emulator
            .get_table_bucket_replication("us-east-1", &recreated.text("arn"))
            .await,
        NO_REPLICATION_CONFIGURATION,
        "the recreated bucket",
    );
}

#[tokio::test]
async fn a_call_against_a_bucket_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();

    // One resident, configured bucket, so the refusals below are about the ARN
    // they name and not about an empty store.
    let resident = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "replication-resident" }))
        .await
        .text("arn");
    let target = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "replication-resident-target" }))
        .await
        .text("arn");
    let put = emulator
        .put_table_bucket_replication(
            "us-east-1",
            &[("tableBucketARN", resident.clone())],
            &configuration(
                "arn:aws:iam::000000000000:role/resident-replication",
                &[target],
            ),
        )
        .await;
    accepted_token(&put, "the resident bucket");

    let doomed = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "replication-deleted" }))
        .await
        .text("arn");
    let deleted = emulator.delete_table_bucket("us-east-1", &doomed).await;
    assert_eq!(
        deleted.status, 204,
        "case \"a deleted bucket\": the delete must succeed, got {status} {body}",
        status = deleted.status,
        body = deleted.body
    );

    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a bucket that was never created",
            expected_bucket_arn("us-east-1", "replication-never-created"),
            "us-east-1",
        ),
        ("a bucket that was deleted", doomed.clone(), "us-east-1"),
        (
            "the resident bucket's name in another region",
            expected_bucket_arn("eu-west-1", "replication-resident"),
            "eu-west-1",
        ),
        (
            "another account's bucket of the same name",
            resident.replace("000000000000", "123456789012"),
            "us-east-1",
        ),
    ];

    for (label, arn, region) in &cases {
        assert_not_found(
            &emulator.get_table_bucket_replication(region, arn).await,
            NO_BUCKET,
            label,
        );
        assert_not_found(
            &emulator
                .delete_table_bucket_replication(region, &[("tableBucketARN", arn.clone())])
                .await,
            NO_BUCKET,
            label,
        );
        assert_not_found(
            &emulator
                .put_table_bucket_replication(
                    region,
                    &[("tableBucketARN", arn.clone())],
                    &configuration(
                        "arn:aws:iam::000000000000:role/absent-bucket-replication",
                        &[expected_bucket_arn(region, "replication-resident-target")],
                    ),
                )
                .await,
            NO_BUCKET,
            label,
        );
    }

    // The refused calls changed nothing: the one configured bucket still reads.
    let read = emulator
        .get_table_bucket_replication("us-east-1", &resident)
        .await;
    assert_eq!(
        read.status, 200,
        "case \"the resident bucket\": the read must still succeed, got {status} {body}",
        status = read.status,
        body = read.body
    );
}
