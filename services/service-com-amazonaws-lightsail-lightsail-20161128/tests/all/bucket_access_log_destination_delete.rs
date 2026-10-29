// SPDX-License-Identifier: CC0-1.0

//! What `DeleteBucket` does to the access log configuration of the buckets that
//! saved their logs in the bucket it removes, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the sibling suites' own
//! ([`crate::instance_lifecycle::Emulator`]), and every configuration a case
//! releases was written with `UpdateBucket` — no case reaches into the store.
//!
//! The rule under test: removing a resource releases every live reference another
//! stored record holds to it, so no read reports a configuration naming a
//! resource the account no longer holds — and so no read reports a configuration
//! its own write path would refuse. `BucketAccessLogConfig$destination` names
//! "the bucket where the access logs are saved", which "can be a Lightsail bucket
//! in the same account, and in the same Amazon Web Services Region as the source
//! bucket", and it "is required when enabling the access log for a bucket, and
//! should be omitted when disabling the access log" — so a source bucket whose
//! destination is removed returns to the disabled form of that configuration, the
//! release is logged as that bucket's own `UpdateBucket` entry beside the
//! `DeleteBucket` one, and a configuration naming any OTHER bucket is left
//! exactly as it was.
//!
//! The cases are a TABLE whose rows differ along everything the rule generalises
//! over: the Region the buckets live in (including no credential scope at all),
//! how many buckets saved their logs in the removed one (none, one, several),
//! whether each of those carried a prefix, the length and form of the bucket
//! names, whether the delete was forced, and whether other buckets of the same
//! Region hold an access log configuration naming a different destination.

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The bundle every bucket in this suite is created on — the cheapest published
/// plan (`crate::state::BUCKET_BUNDLE_SPECS`); which plan a bucket is on decides
/// nothing about where its access logs are saved.
const BUCKET_BUNDLE: &str = "small_1_0";

/// A name of exactly 54 characters — the upper bound the modelled `BucketName`
/// length (3..54) admits.
const LONGEST_BUCKET_NAME: &str = "abcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcd";

/// The wire error a Lightsail request for a resource the account does not hold is
/// refused with, and the `code` its body carries (pinned for every operation in
/// `instance_error_envelope.rs`).
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The Region a request made with no credential scope at all is resolved to.
const DEFAULT_REGION: &str = "us-east-1";

/// Create the bucket `name` in `region`.
async fn create_bucket(emulator: &Emulator, region: Option<&str>, name: &str) {
    emulator
        .ok_at(
            None,
            region,
            "CreateBucket",
            json!({"bucketName": name, "bundleId": BUCKET_BUNDLE}),
        )
        .await;
}

/// Turn the access log of `source` on, saving its logs in `destination` under
/// `prefix` when one is given.
async fn enable_access_log(
    emulator: &Emulator,
    region: Option<&str>,
    source: &str,
    destination: &str,
    prefix: Option<&str>,
) {
    let mut config = json!({"enabled": true, "destination": destination});
    if let Some(prefix) = prefix {
        config["prefix"] = json!(prefix);
    }
    emulator
        .ok_at(
            None,
            region,
            "UpdateBucket",
            json!({"bucketName": source, "accessLogConfig": config}),
        )
        .await;
}

/// The access log configuration `GetBuckets` reports for `bucket` in `region`.
async fn access_log_config(
    case: &str,
    emulator: &Emulator,
    region: Option<&str>,
    bucket: &str,
) -> Value {
    let reported = emulator
        .ok_at(None, region, "GetBuckets", json!({"bucketName": bucket}))
        .await["buckets"][0]
        .clone();
    let config = reported["accessLogConfig"].clone();
    assert!(
        config.is_object(),
        "case {case}: expected {bucket} to report an access log configuration, got {reported:?}"
    );
    config
}

/// The `operations` array of a response, as a list of entries.
fn operations(case: &str, body: &Value) -> Vec<Value> {
    body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {body:?}"))
        .clone()
}

/// Assert `config` is the DISABLED form: the access log is off and names neither
/// a destination nor a prefix, which "should be omitted when disabling the access
/// log".
fn assert_disabled(case: &str, bucket: &str, config: &Value) {
    assert_eq!(
        (
            config["enabled"].as_bool(),
            config["destination"].as_str(),
            config["prefix"].as_str(),
        ),
        (Some(false), None, None),
        "case {case}: expected {bucket} to report its access log released, got {config:?}"
    );
}

// ---------------------------------------------------------------------------
// The release
// ---------------------------------------------------------------------------

/// One destination bucket whose removal must release the access log
/// configurations that named it.
struct Row {
    case: &'static str,
    /// The Region every bucket of the row lives in — `None` for a request made
    /// with no credential scope at all, which the service resolves to its
    /// default Region.
    region: Option<&'static str>,
    /// The bucket the case deletes.
    destination: &'static str,
    /// The buckets that save their access logs in `destination`, each with the
    /// prefix it carries, when it carries one.
    sources: &'static [(&'static str, Option<&'static str>)],
    /// Buckets of the same Region that save their access logs in
    /// `other_destination` instead: their configuration must survive untouched.
    bystanders: &'static [&'static str],
    /// The bucket the `bystanders` save their logs in, when the row has any.
    other_destination: Option<&'static str>,
    /// Whether the delete is forced. A delete releases the reference either way,
    /// so the release cannot depend on it.
    force_delete: bool,
}

/// The table for the release.
const ROWS: &[Row] = &[
    Row {
        case: "one source bucket with a prefix, in the default Region",
        region: None,
        destination: "primary-log-store",
        sources: &[("web-source", Some("logs/"))],
        bystanders: &[],
        other_destination: None,
        force_delete: false,
    },
    Row {
        case: "two source buckets, one of them the longest name, in another Region, forced",
        region: Some("us-west-2"),
        destination: "west-log-store",
        sources: &[("west-source-1", None), (LONGEST_BUCKET_NAME, Some("a/b/"))],
        bystanders: &["west-bystander"],
        other_destination: Some("west-other-log-store"),
        force_delete: true,
    },
    Row {
        case: "no bucket saves its logs in the deleted one",
        region: Some("eu-central-1"),
        destination: "unreferenced-log-store",
        sources: &[],
        bystanders: &["eu-bystander"],
        other_destination: Some("eu-other-log-store"),
        force_delete: false,
    },
    Row {
        case: "three source buckets of differing name forms and prefix depths",
        region: Some("ap-southeast-2"),
        destination: "ap-log-store-3",
        sources: &[
            ("ap-source-a", None),
            ("ap1-source-9", Some("x/")),
            ("abc", Some("deep/prefix/path/")),
        ],
        bystanders: &[],
        other_destination: None,
        force_delete: false,
    },
];

/// RULE: deleting a bucket releases every access log configuration that saved its
/// logs in it — each source bucket reports the disabled form afterwards — the
/// response logs the removal plus one entry per release, and a configuration
/// naming another destination is left exactly as it was.
#[tokio::test]
async fn deleting_a_bucket_releases_every_access_log_that_named_it() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        create_bucket(&emulator, row.region, row.destination).await;
        if let Some(other) = row.other_destination {
            create_bucket(&emulator, row.region, other).await;
        }
        for (source, prefix) in row.sources {
            create_bucket(&emulator, row.region, source).await;
            enable_access_log(&emulator, row.region, source, row.destination, *prefix).await;
        }
        for bystander in row.bystanders {
            let other = row
                .other_destination
                .expect("a row with a bystander names the destination it saves its logs in");
            create_bucket(&emulator, row.region, bystander).await;
            enable_access_log(&emulator, row.region, bystander, other, None).await;
        }

        // What a client sees before the delete: every source bucket saves its
        // logs in the bucket about to be deleted.
        for (source, prefix) in row.sources {
            let before = access_log_config(case, &emulator, row.region, source).await;
            assert_eq!(
                (
                    before["enabled"].as_bool(),
                    before["destination"].as_str(),
                    before["prefix"].as_str(),
                ),
                (Some(true), Some(row.destination), *prefix),
                "case {case}: expected {source} to start out saving its logs in {}, got \
                 {before:?}",
                row.destination
            );
        }

        let mut delete = json!({"bucketName": row.destination});
        if row.force_delete {
            delete["forceDelete"] = json!(true);
        }
        let deleted = emulator
            .ok_at(None, row.region, "DeleteBucket", delete)
            .await;

        // The log: the removal itself, then one release per configuration that
        // named the removed bucket — each already done when the call returned,
        // so each is terminal.
        let entries = operations(case, &deleted);
        assert_eq!(
            entries.len(),
            1 + row.sources.len(),
            "case {case}: expected one entry for the removal and one per released access log, \
             got {entries:?}"
        );
        assert_eq!(
            (
                entries[0]["operationType"].as_str(),
                entries[0]["resourceType"].as_str(),
                entries[0]["resourceName"].as_str(),
            ),
            (
                Some("DeleteBucket"),
                Some("Bucket"),
                Some(row.destination)
            ),
            "case {case}: expected the removal entry first, got {:?}",
            entries[0]
        );
        let mut released: Vec<String> = Vec::new();
        for entry in &entries[1..] {
            assert_eq!(
                (
                    entry["operationType"].as_str(),
                    entry["resourceType"].as_str(),
                    entry["status"].as_str(),
                    entry["isTerminal"].as_bool(),
                    entry["location"]["regionName"].as_str(),
                ),
                (
                    Some("UpdateBucket"),
                    Some("Bucket"),
                    Some("Succeeded"),
                    Some(true),
                    Some(row.region.unwrap_or(DEFAULT_REGION)),
                ),
                "case {case}: expected a succeeded UpdateBucket entry located where the source \
                 bucket is, got {entry:?}"
            );
            let id = entry["id"].as_str().unwrap_or_default();
            assert!(
                is_uuid_shaped(id),
                "case {case}: expected a uuid-shaped operation id, got {id:?}"
            );
            released.push(
                entry["resourceName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        released.sort();
        let mut expected: Vec<String> = row
            .sources
            .iter()
            .map(|(source, _)| (*source).to_string())
            .collect();
        expected.sort();
        assert_eq!(
            released, expected,
            "case {case}: expected one release logged per bucket that saved its logs in the \
             deleted one"
        );

        // The deleted bucket is gone from every read...
        let (status, gone) = emulator
            .call_in(row.region, "GetBuckets", json!({"bucketName": row.destination}))
            .await;
        assert_eq!(
            (status, error_code(&gone).as_str(), gone["code"].as_str()),
            (400, NOT_FOUND, Some(DOES_NOT_EXIST)),
            "case {case}: expected the deleted bucket to be gone, got {status} {gone:?}"
        );

        // ...and so is every access log configuration that named it, while a
        // configuration naming another destination is untouched.
        for (source, _) in row.sources {
            let after = access_log_config(case, &emulator, row.region, source).await;
            assert_disabled(case, source, &after);
        }
        for bystander in row.bystanders {
            let other = row
                .other_destination
                .expect("a row with a bystander names the destination it saves its logs in");
            let after = access_log_config(case, &emulator, row.region, bystander).await;
            assert_eq!(
                (
                    after["enabled"].as_bool(),
                    after["destination"].as_str(),
                    after["prefix"].as_str(),
                ),
                (Some(true), Some(other), None),
                "case {case}: expected {bystander} to keep the access log it holds, got {after:?}"
            );
        }
    }
}

/// RULE (the same one, read back through the write path): what a read reports is a
/// configuration `UpdateBucket` accepts, so a source bucket whose destination was
/// deleted can be re-saved as it stands, and can be pointed at another bucket of
/// the Region afterwards.
#[tokio::test]
async fn a_released_source_bucket_can_be_written_back_as_it_reads() {
    let emulator = Emulator::new();
    let cases: &[(&str, Option<&str>, &str, &str, &str)] = &[
        (
            "the default Region",
            None,
            "written-back-source",
            "written-back-log-store",
            "written-back-successor",
        ),
        (
            "another Region",
            Some("ca-central-1"),
            "ca-source-1",
            "ca-log-store",
            "ca-successor-log-store",
        ),
    ];

    for (case, region, source, destination, successor) in cases {
        create_bucket(&emulator, *region, destination).await;
        create_bucket(&emulator, *region, successor).await;
        create_bucket(&emulator, *region, source).await;
        enable_access_log(&emulator, *region, source, destination, Some("logs/")).await;
        emulator
            .ok_at(
                None,
                *region,
                "DeleteBucket",
                json!({"bucketName": destination}),
            )
            .await;

        // The configuration a read now reports, sent straight back through the
        // write path: the disabled form, which the write path accepts.
        let reported = access_log_config(case, &emulator, *region, source).await;
        assert_disabled(case, source, &reported);
        let (status, written) = emulator
            .call_in(
                *region,
                "UpdateBucket",
                json!({"bucketName": source, "accessLogConfig": reported}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the reported configuration to be accepted by its own write \
             path, got {status} {written:?}"
        );
        assert_disabled(case, source, &written["bucket"]["accessLogConfig"]);

        // And the released bucket can be pointed at another bucket of the
        // Region, which is the state a read then reports.
        enable_access_log(&emulator, *region, source, successor, None).await;
        let after = access_log_config(case, &emulator, *region, source).await;
        assert_eq!(
            (after["enabled"].as_bool(), after["destination"].as_str()),
            (Some(true), Some(*successor)),
            "case {case}: expected {source} to save its logs in {successor}, got {after:?}"
        );
    }
}

/// RULE (the same one, read through the Region a destination resolves in): a
/// bucket's access logs are saved in a bucket "in the same Amazon Web Services
/// Region as the source bucket", so the release reaches exactly the
/// configurations of that Region — deleting a destination of one Region leaves
/// another Region's source bucket saving its logs where it did.
#[tokio::test]
async fn deleting_a_destination_of_one_region_leaves_another_regions_access_log_alone() {
    let emulator = Emulator::new();
    let case = "a destination deleted in one of two Regions";
    let deleted_region = Some("eu-west-1");
    let other_region = Some("sa-east-1");

    create_bucket(&emulator, deleted_region, "eu-west-log-store").await;
    create_bucket(&emulator, deleted_region, "eu-west-source").await;
    enable_access_log(
        &emulator,
        deleted_region,
        "eu-west-source",
        "eu-west-log-store",
        Some("logs/"),
    )
    .await;
    create_bucket(&emulator, other_region, "sa-east-log-store").await;
    create_bucket(&emulator, other_region, "sa-east-source").await;
    enable_access_log(
        &emulator,
        other_region,
        "sa-east-source",
        "sa-east-log-store",
        None,
    )
    .await;

    let deleted = emulator
        .ok_at(
            None,
            deleted_region,
            "DeleteBucket",
            json!({"bucketName": "eu-west-log-store"}),
        )
        .await;
    let entries = operations(case, &deleted);
    assert_eq!(
        entries.len(),
        2,
        "case {case}: expected the removal and the one release of its own Region to be logged, \
         got {entries:?}"
    );
    assert_eq!(
        (
            entries[1]["operationType"].as_str(),
            entries[1]["resourceName"].as_str(),
            entries[1]["location"]["regionName"].as_str(),
        ),
        (Some("UpdateBucket"), Some("eu-west-source"), Some("eu-west-1")),
        "case {case}: expected the release to be logged against the source bucket of the \
         deleted destination's Region, got {:?}",
        entries[1]
    );

    let released = access_log_config(case, &emulator, deleted_region, "eu-west-source").await;
    assert_disabled(case, "eu-west-source", &released);
    let untouched = access_log_config(case, &emulator, other_region, "sa-east-source").await;
    assert_eq!(
        (
            untouched["enabled"].as_bool(),
            untouched["destination"].as_str(),
        ),
        (Some(true), Some("sa-east-log-store")),
        "case {case}: expected the other Region's source bucket to keep its access log, got \
         {untouched:?}"
    );
}

/// RULE (the same one, read through a bucket that is its own destination): the
/// reference lives on the record being removed, so the removal takes it with the
/// record — nothing is logged against a bucket this account no longer holds, and
/// the freed name belongs to a bucket that inherits nothing.
#[tokio::test]
async fn a_bucket_that_saves_its_own_logs_leaves_nothing_behind() {
    let emulator = Emulator::new();
    let case = "a bucket that is its own access log destination";
    let region = Some("ap-northeast-1");
    let name = "self-logging-bucket";

    create_bucket(&emulator, region, name).await;
    enable_access_log(&emulator, region, name, name, Some("own/")).await;

    let deleted = emulator
        .ok_at(None, region, "DeleteBucket", json!({"bucketName": name}))
        .await;
    let entries = operations(case, &deleted);
    assert_eq!(
        entries.len(),
        1,
        "case {case}: expected the removal alone to be logged, got {entries:?}"
    );
    assert_eq!(
        (
            entries[0]["operationType"].as_str(),
            entries[0]["resourceName"].as_str(),
        ),
        (Some("DeleteBucket"), Some(name)),
        "case {case}: expected the removal entry, got {:?}",
        entries[0]
    );

    // The name is free again, and the bucket created under it starts with the
    // access log off rather than with the deleted bucket's configuration.
    create_bucket(&emulator, region, name).await;
    let fresh = access_log_config(case, &emulator, region, name).await;
    assert_disabled(case, name, &fresh);
}
