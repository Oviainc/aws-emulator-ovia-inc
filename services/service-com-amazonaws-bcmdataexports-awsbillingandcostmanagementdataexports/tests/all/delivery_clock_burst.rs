// SPDX-License-Identifier: CC0-1.0

//! What the CLOCK alone delivers when a client owes this service MANY deliveries at once,
//! with Amazon S3 beside it on one engine: `CreateExport` (and `UpdateExport`) -> the objects
//! in the destination bucket, read with Amazon S3's own `ListObjectsV2`.
//!
//! The rule under test, in one sentence: every delivery this service has accepted is
//! performed on its own cadence, whatever else the process is doing — what runs at once is
//! bounded, but no delivery that was owed is dropped — so a client that creates exports and
//! then reads nothing but the destination bucket finds every one of their files there, with
//! no further call to this service to make it happen.
//!
//! Why a client cares: a data export exists to put a file in a bucket on a cadence. A
//! delivery the service accepted and then quietly never performed leaves the client holding
//! an ARN for an export that never delivers, with no error branch ever taken and nothing to
//! retry — and no call it could make would be the one that fixes it.
//!
//! The rows differ along what the rule generalises over: how many deliveries are owed inside
//! one schedule window — one, a handful, more than one pass of the clock performs, and more
//! again — and, in the second test, whether the delivery was owed by the create or by a later
//! update, made once the export's first delivery had already finished.
//!
//! Nothing here calls THIS service while it waits: every reading of what was delivered is a
//! call to Amazon S3, because a call to this service would itself apply what the clock owes
//! and so could not tell whether the service had done it on its own.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::OVERWRITE_REPORT;

use crate::delivery_grant::{published_grant, Deliveries};
use crate::export_file_delivery::listed_keys;
use crate::export_lifecycle::PINNED_REGION;

/// The `S3Prefix` every export of a row is delivered under: one place in the bucket, so the
/// listing below reports the files of the whole burst together. Each export's own file sits
/// under its own name inside it, so the keys are distinct.
const PREFIX: &str = "exports/burst";

/// The output encoding every row states — one of the pairs this service publishes — and the
/// extension a file in it carries. What the encoding IS is asserted in
/// `tests/all/export_file_delivery.rs`; here it only has to be one the service writes.
const ENCODING: (&str, &str) = ("TEXT_OR_CSV", "GZIP");

/// The `S3OutputType` every row states: one of the modelled values (smithy_query shape
/// `S3OutputType`), and not what this suite varies.
const OUTPUT_TYPE: &str = "CUSTOM";

/// The `OverwriteOption` under which each delivery is delivered IN ADDITION to the files
/// already there rather than replacing them: the other of the two modelled values
/// (smithy_query shape `OverwriteOption`), which is what makes a SECOND delivery of one
/// export observable as a second file.
const CREATE_NEW_REPORT: &str = "CREATE_NEW_REPORT";

/// The statement every export carries: a projection of one published column of one published
/// table, which is what makes a delivered file a file with a header.
const STATEMENT: &str = "SELECT line_item_usage_account_id FROM COST_AND_USAGE_REPORT";

/// The published table that statement reads, restated by the update below because "All
/// export parameters must be specified in the UpdateExport request".
const TABLE: &str = "COST_AND_USAGE_REPORT";

/// How long a test waits for the files the clock owes, and how often it looks.
///
/// The bound is PATIENCE, not a weakened expectation: the assertion is that every file is
/// there, and a whole-workspace test run shares its cores with every other suite, so the
/// service's own clock can be scheduled later than it is owed.
const WATCH_BUDGET: Duration = Duration::from_secs(60);
/// See [`WATCH_BUDGET`].
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The `CreateExport` request for the export called `name`, delivered to `bucket` under the
/// `overwrite` disposition.
fn create_request(name: &str, bucket: &str, overwrite: &str) -> Value {
    let (format, compression) = ENCODING;
    json!({
        "Export": {
            "Name": name,
            "DataQuery": { "QueryStatement": STATEMENT },
            "DestinationConfigurations": {
                "S3Destination": {
                    "S3Bucket": bucket,
                    "S3Prefix": PREFIX,
                    "S3Region": PINNED_REGION,
                    "S3OutputConfigurations": {
                        "OutputType": OUTPUT_TYPE,
                        "Format": format,
                        "Compression": compression,
                        "Overwrite": overwrite,
                    },
                },
            },
            "RefreshCadence": { "Frequency": "SYNCHRONOUS" },
        },
    })
}

/// Watch the bucket — and nothing else — until it holds at least `wanted` keys, answering
/// the keys last seen. Read through the ONE listing this crate's tests share
/// ([`listed_keys`]), so what is asserted is what a client listing the bucket sees.
async fn watch_keys(emulator: &Deliveries, bucket: &str, wanted: usize) -> Vec<String> {
    let deadline = Instant::now() + WATCH_BUDGET;
    loop {
        let keys = listed_keys(emulator, bucket, PREFIX).await;
        if keys.len() >= wanted || Instant::now() >= deadline {
            return keys;
        }
        ::tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Assert that every export in `names` has a file of its own among `keys`, and that `keys`
/// holds exactly `expected` of them.
fn assert_delivered(case: &str, names: &[String], keys: &[String], expected: usize) {
    let missing: Vec<&String> = names
        .iter()
        .filter(|name| !keys.iter().any(|key| key.contains(name.as_str())))
        .collect();
    assert!(
        missing.is_empty(),
        "case {case:?}: the service must have delivered a file for every export it accepted; \
         {} of {} are missing ({missing:?}), got {keys:?}",
        missing.len(),
        names.len()
    );
    assert_eq!(
        keys.len(),
        expected,
        "case {case:?}: expected {expected} delivered file(s), got {keys:?}"
    );
}

/// RULE: every delivery this service accepts is performed by its own clock, however many are
/// owed at once — what one pass does is bounded, the set of deliveries performed is not.
#[tokio::test]
async fn the_clock_delivers_every_export_of_a_burst() {
    // How many exports are created inside one schedule window. The axis is the COUNT: one, a
    // handful, more than the clock performs in a single pass, and more again — so a row past
    // any bound on the work one pass does is covered.
    for exports in [1_usize, 5, 24, 80] {
        let case = format!("{exports} exports owed at once");
        let emulator = Deliveries::new();
        let bucket = format!("export-burst-{exports}");
        emulator
            .bucket_with(&bucket, Some(&published_grant(&bucket)))
            .await;

        let mut names: Vec<String> = Vec::with_capacity(exports);
        for index in 0..exports {
            let name = format!("export_burst_{exports}_{index}");
            let created = emulator
                .ok(
                    "CreateExport",
                    &create_request(&name, &bucket, OVERWRITE_REPORT),
                )
                .await;
            assert!(
                created["ExportArn"].is_string(),
                "case {case:?}: CreateExport returns an ExportArn, got {created}"
            );
            names.push(name);
        }

        // From here nothing calls this service: the deliveries are its own work, and each
        // export's one file is named after that export.
        let keys = watch_keys(&emulator, &bucket, exports).await;
        assert_delivered(&case, &names, &keys, exports);
    }
}

/// RULE, the same one once the clock has nothing left to do: a delivery owed by an update
/// made after every earlier delivery had finished is performed too, so the service takes its
/// own work up again rather than standing down for good.
#[tokio::test]
async fn the_clock_delivers_a_delivery_owed_after_it_stood_down() {
    let case = "an update owing a second delivery after the first had been delivered";
    let emulator = Deliveries::new();
    let bucket = "export-burst-reowed";
    emulator
        .bucket_with(bucket, Some(&published_grant(bucket)))
        .await;

    // Delivered in ADDITION to the file already there, so the second delivery is a second
    // key rather than a replacement of the first.
    let name = "export_burst_reowed".to_string();
    let created = emulator
        .ok(
            "CreateExport",
            &create_request(&name, bucket, CREATE_NEW_REPORT),
        )
        .await;
    let arn = created["ExportArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateExport returns an ExportArn"))
        .to_string();
    let names = vec![name.clone()];
    let first = watch_keys(&emulator, bucket, 1).await;
    assert_delivered(case, &names, &first, 1);

    // The first delivery is done, so the clock owes nothing. The update restates the whole
    // definition — "All export parameters must be specified in the UpdateExport request" —
    // and owes this export its next delivery.
    let mut request = create_request(&name, bucket, CREATE_NEW_REPORT);
    let object = request.as_object_mut().expect("the request is an object");
    object.insert("ExportArn".to_string(), json!(&arn));
    object["Export"]["DataQuery"]
        .as_object_mut()
        .expect("DataQuery is an object")
        .insert("TableConfigurations".to_string(), json!({ TABLE: {} }));
    let updated = emulator.ok("UpdateExport", &request).await;
    assert_eq!(
        updated["ExportArn"].as_str(),
        Some(arn.as_str()),
        "case {case:?}: UpdateExport answers the export it updated, got {updated}"
    );

    // And that delivery lands too, with nothing calling this service after the update.
    let second = watch_keys(&emulator, bucket, 2).await;
    assert_delivered(case, &names, &second, 2);
}
