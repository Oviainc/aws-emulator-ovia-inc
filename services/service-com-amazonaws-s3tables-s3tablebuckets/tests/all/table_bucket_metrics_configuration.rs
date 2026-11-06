// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `PutTableBucketMetricsConfiguration` and the
//! read that reports what it persisted.
//!
//! The rule under test: a table bucket has a metrics configuration only once a
//! put sets one on it, so a read of a bucket that has none is refused with the
//! modeled not-found; a put persists a configuration under an id the service
//! mints (never a value taken from the request) and, being `@idempotent` and
//! carrying no settings, leaves an existing configuration and its id untouched;
//! a read reports the stored id for the bucket it addresses and changes nothing
//! a later caller sees; and a configuration belongs to its bucket, so it is
//! neither visible on another bucket nor outlives the bucket it was set on.
//!
//! Every case drives the real wire path through the harness, so what is
//! asserted is what an AWS SDK client would see. The table spans what the rule
//! generalises over: how many times the put is repeated, the region the bucket
//! lives in, the shape and length of its name, and which other optional
//! members its create carried.

use serde_json::{json, Map, Value};
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;

use s3tables::state::ComAmazonawsS3tablesS3tablebucketsState as S3TablesState;

use super::harness::{expected_bucket_arn, Emulator};

/// The fixed sentence this service states a table bucket that has no metrics
/// configuration with.
const NO_METRICS_CONFIGURATION: &str = "No such metric configuration exists";

/// The fixed sentence this service states a table bucket the store does not
/// hold with.
const NO_BUCKET: &str = "The specified bucket does not exist.";

/// One row: a table bucket created the way the row says, with the put repeated
/// as many times as the row names.
#[derive(Debug)]
struct Case {
    label: &'static str,
    region: &'static str,
    name: &'static str,
    /// How many times `PutTableBucketMetricsConfiguration` is called. More than
    /// one exercises the operation's `@idempotent` trait.
    puts: usize,
    /// Whichever other optional members the create carries, so the reported
    /// configuration is shown not to depend on them.
    extra_members: &'static [(&'static str, &'static str)],
}

impl Case {
    fn create_request(&self) -> Value {
        let mut request = Map::new();
        request.insert("name".to_string(), json!(self.name));
        for (member, value) in self.extra_members {
            let value: Value = serde_json::from_str(value).expect("a well-formed member value");
            request.insert((*member).to_string(), value);
        }
        Value::Object(request)
    }
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one put, a plain bucket",
            region: "us-east-1",
            name: "metrics-once-east",
            puts: 1,
            extra_members: &[],
        },
        Case {
            label: "the put repeated, in another region",
            region: "eu-west-1",
            name: "metrics-twice-west",
            puts: 2,
            extra_members: &[],
        },
        Case {
            label: "the put repeated three times, on a bucket created with other members",
            region: "us-east-1",
            name: "metrics-thrice-with-others",
            puts: 3,
            extra_members: &[
                ("encryptionConfiguration", r#"{"sseAlgorithm":"AES256"}"#),
                (
                    "storageClassConfiguration",
                    r#"{"storageClass":"INTELLIGENT_TIERING"}"#,
                ),
                ("tags", r#"{"team":"data"}"#),
            ],
        },
        Case {
            // `TableBucketName` is `@length(3, 63)`, so the shortest name a
            // bucket can carry is three characters.
            label: "the shortest name a bucket can carry",
            region: "ap-south-1",
            name: "m3t",
            puts: 1,
            extra_members: &[],
        },
        Case {
            label: "a long hyphenated name, put repeated",
            region: "us-west-2",
            name: "metrics-configuration-on-a-rather-long-table-bucket-name-here",
            puts: 2,
            extra_members: &[("tags", r#"{"tier":"gold","team":"platform"}"#)],
        },
    ]
}

/// The `id` a read reported, with the assertion that the response is the shape
/// the model declares: the bucket's ARN, and an id.
fn reported_id(reply: &super::harness::Reply, arn: &str, case: &str) -> String {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the read must succeed, got {status} {body}",
        status = reply.status,
        body = reply.body
    );
    let reported_arn = reply.text("tableBucketARN");
    assert_eq!(
        reported_arn, arn,
        "case {case:?}: expected tableBucketARN {arn:?}, got {reported_arn:?}"
    );
    let id = reply.text("id");
    assert!(
        !id.is_empty(),
        "case {case:?}: expected a non-empty id, got {id:?}"
    );
    id
}

#[tokio::test]
async fn a_put_turns_metrics_on_and_the_read_reports_what_it_persisted() {
    let emulator = Emulator::new();
    let mut minted: Vec<(String, String)> = Vec::new();

    for case in cases() {
        let created = emulator
            .create_table_bucket(case.region, &case.create_request())
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
        assert_eq!(
            arn,
            expected_bucket_arn(case.region, case.name),
            "case {label:?}: the create must mint the modeled ARN, got {arn:?}",
            label = case.label
        );

        // Metrics are off until a put turns them on, so there is nothing to
        // report for a freshly created bucket.
        let before = emulator
            .get_table_bucket_metrics_configuration(case.region, &arn)
            .await;
        assert_eq!(
            before.status, 404,
            "case {label:?}: expected 404 before the put, got {status} {body}",
            label = case.label,
            status = before.status,
            body = before.body
        );
        assert_eq!(
            before.error_code.as_deref(),
            Some("NotFoundException"),
            "case {label:?}: expected NotFoundException before the put, got {actual:?}",
            label = case.label,
            actual = before.error_code
        );
        assert_eq!(
            before.message(),
            NO_METRICS_CONFIGURATION,
            "case {label:?}: expected the fixed not-configured sentence, got {actual:?}",
            label = case.label,
            actual = before.message()
        );

        let mut first_id: Option<String> = None;
        for attempt in 1..=case.puts {
            let put = emulator
                .put_table_bucket_metrics_configuration(case.region, &arn)
                .await;
            assert_eq!(
                put.status, 204,
                "case {label:?}: put {attempt} must answer the modeled 204, got {status} {body}",
                label = case.label,
                status = put.status,
                body = put.body
            );
            assert_eq!(
                put.body,
                Value::Null,
                "case {label:?}: put {attempt} must carry no body, got {body}",
                label = case.label,
                body = put.body
            );

            let read = emulator
                .get_table_bucket_metrics_configuration(case.region, &arn)
                .await;
            let id = reported_id(&read, &arn, case.label);
            match &first_id {
                // The operation is `@idempotent` and carries no settings, so a
                // repeated put leaves the configuration — and its id — as it
                // was.
                Some(expected) => assert_eq!(
                    &id, expected,
                    "case {label:?}: put {attempt} must leave the id {expected:?}, got {id:?}",
                    label = case.label
                ),
                None => first_id = Some(id.clone()),
            }
        }

        let id = first_id.expect("every row calls the put at least once");

        // The id is minted by the service: it carries nothing a caller sent.
        for sent in [case.name, case.region, arn.as_str()] {
            assert!(
                !id.contains(sent),
                "case {label:?}: the id {id:?} must carry nothing from the request, but it \
                 carries {sent:?}",
                label = case.label
            );
        }

        // A read reports, it does not write: the record still holds exactly the
        // configuration the put persisted, and a second read answers the same.
        {
            let store = emulator.engine.state::<S3TablesState>();
            let store = store.lock();
            let bucket = store.table_buckets.get(&arn).unwrap_or_else(|| {
                panic!("case {label:?}: the bucket is resident", label = case.label)
            });
            let persisted = bucket.metrics.as_ref().map(|metrics| metrics.id.clone());
            assert_eq!(
                persisted.as_deref(),
                Some(id.as_str()),
                "case {label:?}: the store must hold the reported configuration, got {persisted:?}",
                label = case.label
            );
        }
        let again = emulator
            .get_table_bucket_metrics_configuration(case.region, &arn)
            .await;
        assert_eq!(
            reported_id(&again, &arn, case.label),
            id,
            "case {label:?}: two reads must report the same id",
            label = case.label
        );

        minted.push((case.label.to_string(), id));
    }

    // Each bucket's configuration is its own: no two rows report the same id.
    for (index, (label, id)) in minted.iter().enumerate() {
        for (other_label, other_id) in &minted[index + 1..] {
            assert_ne!(
                id, other_id,
                "case {label:?}: its id {id:?} must differ from the one case \
                 {other_label:?} reports"
            );
        }
    }
}

#[tokio::test]
async fn a_configuration_belongs_to_the_bucket_it_was_set_on() {
    let emulator = Emulator::new();

    // Two buckets of the same name in different regions, and a third in the
    // first region: the put below addresses exactly one of them.
    let configured = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "metrics-scoped" }))
        .await
        .text("arn");
    let same_name_other_region = emulator
        .create_table_bucket("eu-west-1", &json!({ "name": "metrics-scoped" }))
        .await
        .text("arn");
    let sibling = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "metrics-unconfigured" }))
        .await
        .text("arn");

    let put = emulator
        .put_table_bucket_metrics_configuration("us-east-1", &configured)
        .await;
    assert_eq!(
        put.status, 204,
        "case \"the configured bucket\": the put must succeed, got {status} {body}",
        status = put.status,
        body = put.body
    );

    let read = emulator
        .get_table_bucket_metrics_configuration("us-east-1", &configured)
        .await;
    let id = reported_id(&read, &configured, "the configured bucket");

    for (label, region, arn) in [
        (
            "the same name in another region",
            "eu-west-1",
            &same_name_other_region,
        ),
        ("a sibling bucket in the same region", "us-east-1", &sibling),
    ] {
        let read = emulator
            .get_table_bucket_metrics_configuration(region, arn)
            .await;
        assert_eq!(
            read.status, 404,
            "case {label:?}: expected 404, got {status} {body}",
            status = read.status,
            body = read.body
        );
        assert_eq!(
            read.message(),
            NO_METRICS_CONFIGURATION,
            "case {label:?}: expected the fixed not-configured sentence, got {actual:?}",
            actual = read.message()
        );
    }

    // A configuration does not outlive the bucket it was set on: a bucket
    // recreated under the same name collects no metrics until its own put.
    let deleted = emulator.delete_table_bucket("us-east-1", &configured).await;
    assert_eq!(
        deleted.status, 204,
        "case \"the configured bucket\": the delete must succeed, got {status} {body}",
        status = deleted.status,
        body = deleted.body
    );
    let recreated = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "metrics-scoped" }))
        .await;
    assert_eq!(
        recreated.status, 200,
        "case \"the recreated bucket\": the create must succeed, got {status} {body}",
        status = recreated.status,
        body = recreated.body
    );
    let read = emulator
        .get_table_bucket_metrics_configuration("us-east-1", &recreated.text("arn"))
        .await;
    assert_eq!(
        read.status, 404,
        "case \"the recreated bucket\": expected 404, got {status} {body}",
        status = read.status,
        body = read.body
    );
    assert_eq!(
        read.message(),
        NO_METRICS_CONFIGURATION,
        "case \"the recreated bucket\": expected the fixed not-configured sentence, got {actual:?}",
        actual = read.message()
    );

    // And the id the one configured bucket reported was not reused by anything
    // above: it is still the id its own read answers after a fresh put.
    let put = emulator
        .put_table_bucket_metrics_configuration("us-east-1", &recreated.text("arn"))
        .await;
    assert_eq!(
        put.status, 204,
        "case \"the recreated bucket\": the put must succeed, got {status} {body}",
        status = put.status,
        body = put.body
    );
    let read = emulator
        .get_table_bucket_metrics_configuration("us-east-1", &recreated.text("arn"))
        .await;
    let fresh_id = reported_id(&read, &recreated.text("arn"), "the recreated bucket");
    assert_ne!(
        fresh_id, id,
        "case \"the recreated bucket\": a fresh configuration must carry a fresh id, got \
         {fresh_id:?}"
    );
}

#[tokio::test]
async fn a_delete_turns_metrics_off_again() {
    let emulator = Emulator::new();

    // The table spans what the rule generalises over: the region, the shape of
    // the name, and how many times the delete is repeated.
    let cases: Vec<(&str, &str, usize)> = vec![
        ("one delete, a plain bucket", "us-east-1", 1),
        ("the delete repeated, in another region", "eu-west-1", 2),
        ("the delete repeated three times", "ap-south-1", 3),
    ];

    for (index, (label, region, deletes)) in cases.iter().enumerate() {
        let name = format!("metrics-deleted-{index}");
        let arn = emulator
            .create_table_bucket(region, &json!({ "name": name }))
            .await
            .text("arn");

        // The operation is `@idempotent`: it asks that the bucket collect no
        // metrics, which a bucket that has no configuration already does, so
        // the delete succeeds before any put.
        let before = emulator
            .delete_table_bucket_metrics_configuration(region, &arn)
            .await;
        assert_eq!(
            before.status, 204,
            "case {label:?}: the delete before the put must answer the modeled 204, got {status} \
             {body}",
            status = before.status,
            body = before.body
        );
        assert_eq!(
            before.body,
            Value::Null,
            "case {label:?}: the delete must carry no body, got {body}",
            body = before.body
        );

        let put = emulator
            .put_table_bucket_metrics_configuration(region, &arn)
            .await;
        assert_eq!(
            put.status, 204,
            "case {label:?}: the put must succeed, got {status} {body}",
            status = put.status,
            body = put.body
        );
        reported_id(
            &emulator
                .get_table_bucket_metrics_configuration(region, &arn)
                .await,
            &arn,
            label,
        );

        for attempt in 1..=*deletes {
            let deleted = emulator
                .delete_table_bucket_metrics_configuration(region, &arn)
                .await;
            assert_eq!(
                deleted.status, 204,
                "case {label:?}: delete {attempt} must answer the modeled 204, got {status} \
                 {body}",
                status = deleted.status,
                body = deleted.body
            );
            assert_eq!(
                deleted.body,
                Value::Null,
                "case {label:?}: delete {attempt} must carry no body, got {body}",
                body = deleted.body
            );
        }

        // The read reports metrics off again, and the store holds nothing.
        let read = emulator
            .get_table_bucket_metrics_configuration(region, &arn)
            .await;
        assert_eq!(
            read.status, 404,
            "case {label:?}: expected 404 after the delete, got {status} {body}",
            status = read.status,
            body = read.body
        );
        {
            let store = emulator.engine.state::<S3TablesState>();
            let store = store.lock();
            let persisted = store
                .table_buckets
                .get(&arn)
                .and_then(|bucket| bucket.metrics.clone());
            assert_eq!(
                persisted, None,
                "case {label:?}: the delete must remove the configuration from the store, got \
                 {persisted:?}"
            );
        }

        // And a later put turns metrics on again, under a fresh id.
        let put = emulator
            .put_table_bucket_metrics_configuration(region, &arn)
            .await;
        assert_eq!(
            put.status, 204,
            "case {label:?}: the put after the delete must succeed, got {status} {body}",
            status = put.status,
            body = put.body
        );
        reported_id(
            &emulator
                .get_table_bucket_metrics_configuration(region, &arn)
                .await,
            &arn,
            label,
        );
    }

    // A delete against a bucket the store does not hold is the bucket's own
    // not-found.
    let absent = emulator
        .delete_table_bucket_metrics_configuration(
            "us-east-1",
            &expected_bucket_arn("us-east-1", "metrics-delete-absent"),
        )
        .await;
    assert_eq!(
        absent.status, 404,
        "case \"a bucket that was never created\": expected 404, got {status} {body}",
        status = absent.status,
        body = absent.body
    );
    assert_eq!(
        absent.message(),
        NO_BUCKET,
        "case \"a bucket that was never created\": expected the fixed not-found sentence, got \
         {actual:?}",
        actual = absent.message()
    );
}

#[tokio::test]
async fn a_call_against_a_bucket_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();

    // One resident, configured bucket, so the refusals below are about the ARN
    // they name and not about an empty store.
    let resident = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "metrics-resident" }))
        .await
        .text("arn");
    let put = emulator
        .put_table_bucket_metrics_configuration("us-east-1", &resident)
        .await;
    assert_eq!(
        put.status, 204,
        "case \"the resident bucket\": the put must succeed, got {status} {body}",
        status = put.status,
        body = put.body
    );

    let doomed = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "metrics-deleted" }))
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
            expected_bucket_arn("us-east-1", "metrics-never-created"),
            "us-east-1",
        ),
        ("a bucket that was deleted", doomed.clone(), "us-east-1"),
        (
            "the resident bucket's name in another region",
            expected_bucket_arn("eu-west-1", "metrics-resident"),
            "eu-west-1",
        ),
        (
            "another account's bucket of the same name",
            resident.replace("000000000000", "123456789012"),
            "us-east-1",
        ),
    ];

    for (label, arn, region) in &cases {
        for (operation, reply) in [
            (
                "PutTableBucketMetricsConfiguration",
                emulator
                    .put_table_bucket_metrics_configuration(region, arn)
                    .await,
            ),
            (
                "GetTableBucketMetricsConfiguration",
                emulator
                    .get_table_bucket_metrics_configuration(region, arn)
                    .await,
            ),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {label:?} on {operation}: expected 404, got {status} {body}",
                status = reply.status,
                body = reply.body
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("NotFoundException"),
                "case {label:?} on {operation}: expected NotFoundException, got {actual:?}",
                actual = reply.error_code
            );
            assert_eq!(
                reply.message(),
                NO_BUCKET,
                "case {label:?} on {operation}: expected the fixed not-found sentence, got \
                 {actual:?}",
                actual = reply.message()
            );
        }
    }

    // The refused puts persisted nothing: the store still holds exactly the one
    // configuration the resident bucket's put wrote.
    {
        let store = emulator.engine.state::<S3TablesState>();
        let store = store.lock();
        let configured: Vec<&String> = store
            .table_buckets
            .values()
            .filter(|bucket| bucket.metrics.is_some())
            .map(|bucket| &bucket.arn)
            .collect();
        assert_eq!(
            configured,
            vec![&resident],
            "case \"the refused calls\": only the resident bucket may carry a configuration, got \
             {configured:?}"
        );
    }

    // The resident bucket still reads, so the refusals above are about the ARN
    // each named.
    let read = emulator
        .get_table_bucket_metrics_configuration("us-east-1", &resident)
        .await;
    assert_eq!(
        read.status, 200,
        "case \"the resident bucket\": the read must still succeed, got {status} {body}",
        status = read.status,
        body = read.body
    );
}
