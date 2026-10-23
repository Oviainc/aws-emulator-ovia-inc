// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the S3 Table integration slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — through
//! the ONE harness this test crate has (`telemetry_rules`), so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. `CreateS3TableIntegration` stores ONE integration record in the region the
//!    request was signed for, mints the id the integration is held by (the operation
//!    carries no caller-supplied name) and answers the ARN that id goes into; every
//!    member `GetS3TableIntegration` reports — role ARN, encryption configuration,
//!    destination table bucket, status and creation timestamp — comes from that
//!    record, for every encryption configuration and role ARN the model admits;
//! 2. the ARN is the only handle on an integration, and an ARN that names no stored
//!    integration in the region the request was signed for is a modelled 404,
//!    whatever region, account or id it carries;
//! 3. a read reports the record and changes nothing, so two reads of one integration
//!    answer identically;
//! 4. the rules the model states but cannot express as a constraint are refused with
//!    the modelled 400 and store nothing: `SseAlgorithm` carries one of
//!    `SSEAlgorithm`'s own values, and `KmsKeyArn` rides on the KMS algorithm alone;
//! 5. a create past either emulator store cap is refused with the modelled 402 before
//!    a record is allocated.

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::ops::types::INTEGRATION_STATUS_VALUES;
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::{
    s3_table_integration_arn, table_bucket_arn, ComAmazonawsObservabilityadminObservabilityadminState,
    StoredS3TableIntegration, INTEGRATION_STATUS_ACTIVE, MAX_S3_TABLE_INTEGRATIONS,
    MAX_STORED_S3_TABLE_INTEGRATION_RECORDS, TABLE_BUCKET_NAME_PREFIX,
};

use crate::telemetry_rules::{call, emulator, error_code, Emulator, ACCOUNT};

/// The region most requests below sign for; the ARNs they mint carry exactly this one.
const HOME_REGION: &str = "us-west-2";

/// A second region, so "reachable only from the region it was created in" can be
/// shown to be per-region.
const OTHER_REGION: &str = "eu-west-1";

async fn create(emulator: &Emulator, region: &str, body: &Value) -> (u16, Value) {
    call(emulator, region, "/CreateS3TableIntegration", body).await
}

async fn get(emulator: &Emulator, region: &str, arn: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/GetS3TableIntegration",
        &json!({ "Arn": arn }),
    )
    .await
}

/// The `Arn` a successful create reported.
fn created_arn(body: &Value) -> String {
    body.get("Arn")
        .and_then(Value::as_str)
        .unwrap_or("<no Arn>")
        .to_string()
}

/// A member of a `GetS3TableIntegration` response, absent members read as `None`.
fn member<'a>(response: &'a Value, name: &str) -> Option<&'a Value> {
    response
        .get(name)
        .filter(|value| !matches!(value, Value::Null))
}

/// How many integration records the shared store holds.
fn stored_count(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<ComAmazonawsObservabilityadminObservabilityadminState>()
        .lock()
        .s3_table_integrations
        .len()
}

/// Whether `id` has the canonical hyphenated UUID form (8-4-4-4-12 lowercase hex)
/// the service mints an integration id in.
fn is_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()))
}

/// Every encryption configuration and role ARN the model admits, differing along what
/// the round-trip rule generalises over: which of `SSEAlgorithm`'s two values
/// (`S_S_E_ALGORITHM_VALUES`, from
/// `smithy_query(query="shape",shape="SSEAlgorithm")`) the request carries, whether it
/// names a KMS key at all, which KMS resource form the key ARN takes, whether the
/// role ARN carries a path, and whether the create carries tags and how many.
fn valid_requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "AES256, no key, no tags",
            json!({
                "Encryption": { "SseAlgorithm": "AES256" },
                "RoleArn": "arn:aws:iam::000000000000:role/telemetry-s3-tables",
            }),
        ),
        (
            "aws:kms with a customer managed key",
            json!({
                "Encryption": {
                    "SseAlgorithm": "aws:kms",
                    "KmsKeyArn":
                        "arn:aws:kms:us-west-2:000000000000:key/1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d",
                },
                "RoleArn": "arn:aws:iam::000000000000:role/telemetry-s3-tables",
            }),
        ),
        (
            "aws:kms with no key of its own",
            json!({
                "Encryption": { "SseAlgorithm": "aws:kms" },
                "RoleArn": "arn:aws:iam::000000000000:role/telemetry-s3-tables",
            }),
        ),
        (
            "aws:kms naming the key by alias, and a role under a path",
            json!({
                "Encryption": {
                    "SseAlgorithm": "aws:kms",
                    "KmsKeyArn": "arn:aws:kms:us-west-2:000000000000:alias/telemetry",
                },
                "RoleArn": "arn:aws:iam::000000000000:role/service-role/CloudWatchTelemetry",
            }),
        ),
        (
            "AES256 with two tags",
            json!({
                "Encryption": { "SseAlgorithm": "AES256" },
                "RoleArn": "arn:aws:iam::000000000000:role/telemetry-s3-tables",
                "Tags": { "team": "observability", "env": "prod" },
            }),
        ),
    ]
}

/// A create answers the ARN of the one record it stored, and every member a read
/// reports comes from that record — for every request the model admits.
#[tokio::test]
async fn a_create_persists_the_integration_a_read_reports() {
    let emulator = emulator();
    let mut minted: Vec<String> = Vec::new();

    for (case, request) in valid_requests() {
        let before = now_millis();
        let (status, response) = create(&emulator, HOME_REGION, &request).await;
        let after = now_millis();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );

        let arn = created_arn(&response);
        let id = arn
            .rsplit('/')
            .next()
            .unwrap_or("<no id>")
            .to_string();
        assert!(
            is_uuid(&id),
            "case {case:?}: expected the minted id to be a hyphenated UUID, got {id:?}"
        );
        assert_eq!(
            arn,
            s3_table_integration_arn(HOME_REGION, &id),
            "case {case:?}: expected \
             arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:s3-table-integration/<id>, \
             got {arn:?}"
        );
        assert!(
            !minted.contains(&arn),
            "case {case:?}: expected a fresh integration ARN, got the already-minted {arn:?}"
        );
        minted.push(arn.clone());

        let (status, read) = get(&emulator, HOME_REGION, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read of {arn} to answer 200, got {status} {read:?}"
        );
        assert_eq!(
            member(&read, "Arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected Arn {arn:?}, got {read:?}"
        );
        assert_eq!(
            member(&read, "RoleArn"),
            request.get("RoleArn"),
            "case {case:?}: expected the stored RoleArn back, got {read:?}"
        );
        assert_eq!(
            member(&read, "Encryption"),
            request.get("Encryption"),
            "case {case:?}: expected the stored Encryption back, got {read:?}"
        );
        let integration_status = member(&read, "Status")
            .and_then(Value::as_str)
            .unwrap_or("<no Status>");
        assert_eq!(
            integration_status, INTEGRATION_STATUS_ACTIVE,
            "case {case:?}: expected Status {INTEGRATION_STATUS_ACTIVE}, got {read:?}"
        );
        assert!(
            INTEGRATION_STATUS_VALUES.contains(&integration_status),
            "case {case:?}: expected the reported status to be one of IntegrationStatus' own \
             values {INTEGRATION_STATUS_VALUES:?}, got {integration_status:?}"
        );
        let bucket = member(&read, "DestinationTableBucketArn")
            .and_then(Value::as_str)
            .unwrap_or("<no DestinationTableBucketArn>");
        assert_eq!(
            bucket,
            table_bucket_arn(HOME_REGION, &id),
            "case {case:?}: expected the integration's own table bucket \
             arn:aws:s3tables:{HOME_REGION}:{ACCOUNT}:bucket/{TABLE_BUCKET_NAME_PREFIX}{id}, \
             got {bucket:?}"
        );
        let created = member(&read, "CreatedTimeStamp")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        assert!(
            (before..=after).contains(&created),
            "case {case:?}: expected CreatedTimeStamp within [{before}, {after}], got {created}"
        );
    }

    assert_eq!(
        stored_count(&emulator),
        minted.len(),
        "expected one stored record per create, got {} for {} creates",
        stored_count(&emulator),
        minted.len()
    );
}

/// Now, in the epoch milliseconds the modelled `CreatedTimeStamp` carries — read from
/// `SystemTime` here rather than through the service's own helper, so the assertion
/// bounds are the test's.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// A read reports the record and changes nothing, so a second read of the same
/// integration answers exactly what the first did.
#[tokio::test]
async fn a_read_changes_nothing_a_later_caller_sees() {
    let emulator = emulator();
    let (status, response) = create(
        &emulator,
        HOME_REGION,
        &json!({
            "Encryption": { "SseAlgorithm": "AES256" },
            "RoleArn": "arn:aws:iam::000000000000:role/telemetry-s3-tables",
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {response:?}");
    let arn = created_arn(&response);

    let (first_status, first) = get(&emulator, HOME_REGION, &arn).await;
    let (second_status, second) = get(&emulator, HOME_REGION, &arn).await;
    assert_eq!(
        (first_status, &first),
        (second_status, &second),
        "case {:?}: expected two reads of one integration to answer identically, got {first:?} \
         then {second:?}",
        "read twice"
    );
    assert_eq!(
        stored_count(&emulator),
        1,
        "case {:?}: expected the reads to leave the one stored record alone, got {} records",
        "read twice",
        stored_count(&emulator)
    );
}

/// The ARN is the only handle on an integration, and it resolves only in the region
/// the integration was created in: anything else is the modelled 404.
#[tokio::test]
async fn an_arn_that_names_no_stored_integration_is_a_404() {
    let emulator = emulator();
    let (status, response) = create(
        &emulator,
        HOME_REGION,
        &json!({
            "Encryption": { "SseAlgorithm": "aws:kms" },
            "RoleArn": "arn:aws:iam::000000000000:role/telemetry-s3-tables",
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {response:?}");
    let arn = created_arn(&response);
    let id = arn.rsplit('/').next().unwrap_or_default().to_string();

    // The integration resolves from its own region, and from no other.
    let (status, read) = get(&emulator, HOME_REGION, &arn).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} {read:?}",
        "the integration's own region"
    );

    let rows: Vec<(&str, String, &str)> = vec![
        (
            "the same ARN read from another region",
            arn.clone(),
            OTHER_REGION,
        ),
        (
            "an ARN naming the integration's id in another region",
            s3_table_integration_arn(OTHER_REGION, &id),
            HOME_REGION,
        ),
        (
            "an ARN naming an id nothing was created under",
            s3_table_integration_arn(HOME_REGION, "ffffffff-ffff-4fff-8fff-ffffffffffff"),
            HOME_REGION,
        ),
        (
            "an ARN of another account",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:000000000001:s3-table-integration/{id}"),
            HOME_REGION,
        ),
        (
            "an ARN of another partition",
            format!("arn:aws-us-gov:observabilityadmin:{HOME_REGION}:{ACCOUNT}:s3-table-integration/{id}"),
            HOME_REGION,
        ),
        (
            "an ARN of another service's resource",
            format!("arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:telemetry-rule/{id}"),
            HOME_REGION,
        ),
    ];

    for (case, candidate, region) in rows {
        let (status, read) = get(&emulator, region, &candidate).await;
        assert_eq!(
            status, 404,
            "case {case:?} ({candidate}): expected 404, got {status} {read:?}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {read:?}"
        );
    }
}

/// The rules the model states in prose are refused with the modelled 400, and a
/// refused create stores nothing.
#[tokio::test]
async fn a_request_outside_the_modelled_rules_is_refused_and_stores_nothing() {
    let role = "arn:aws:iam::000000000000:role/telemetry-s3-tables";
    let key = "arn:aws:kms:us-west-2:000000000000:key/1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";

    // Each row is outside ONE of the two prose rules: `SseAlgorithm`'s value set (the
    // `SSEAlgorithm` catalog the model declares) and `KmsKeyArn`'s pairing with the KMS
    // algorithm. The rows differ along what each rule generalises over: which value is
    // named and how it is misspelled, and which algorithm the key rides on.
    let rows: Vec<(&str, Value)> = vec![
        (
            "SseAlgorithm spelled the way S3 writes AES-256",
            json!({ "Encryption": { "SseAlgorithm": "AES-256" }, "RoleArn": role }),
        ),
        (
            "SseAlgorithm naming no encryption at all",
            json!({ "Encryption": { "SseAlgorithm": "none" }, "RoleArn": role }),
        ),
        (
            "SseAlgorithm in the wrong case",
            json!({ "Encryption": { "SseAlgorithm": "aws:KMS" }, "RoleArn": role }),
        ),
        (
            "SseAlgorithm naming the KMS algorithm the way KMS writes its own name",
            json!({ "Encryption": { "SseAlgorithm": "kms" }, "RoleArn": role }),
        ),
        (
            "a KMS key on the S3 managed algorithm",
            json!({
                "Encryption": { "SseAlgorithm": "AES256", "KmsKeyArn": key },
                "RoleArn": role,
            }),
        ),
        (
            "a KMS key alias on the S3 managed algorithm",
            json!({
                "Encryption": {
                    "SseAlgorithm": "AES256",
                    "KmsKeyArn": "arn:aws:kms:us-west-2:000000000000:alias/telemetry",
                },
                "RoleArn": role,
            }),
        ),
    ];

    for (case, request) in rows {
        let emulator = emulator();
        let (status, response) = create(&emulator, HOME_REGION, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
        assert_eq!(
            stored_count(&emulator),
            0,
            "case {case:?}: expected a refused create to store nothing, got {} records",
            stored_count(&emulator)
        );
    }
}

/// Both store caps are refused before a record is allocated: the per-region budget,
/// and the store-wide one that dominates it because the region a record is keyed by
/// comes from the caller's own credential scope. The store is pre-filled directly
/// through the shared engine, so the test states the cap rather than paying for
/// hundreds of requests to reach it.
#[tokio::test]
async fn a_create_past_either_cap_is_refused_and_stores_nothing() {
    for (case, per_region, records) in [
        ("the per-region budget is full", true, MAX_S3_TABLE_INTEGRATIONS),
        (
            "the store-wide cap is reached from other regions",
            false,
            MAX_STORED_S3_TABLE_INTEGRATION_RECORDS,
        ),
    ] {
        let emulator = emulator();
        {
            let state = emulator
                .engine
                .state::<ComAmazonawsObservabilityadminObservabilityadminState>();
            let mut store = state.lock();
            for index in 0..records {
                // Spread the store-wide rows over many regions, so the per-region
                // budget is NOT what refuses that case.
                let region = if per_region {
                    HOME_REGION.to_string()
                } else {
                    format!("{OTHER_REGION}-{}", index / 10)
                };
                let id = format!("filler-{index}");
                store.insert_s3_table_integration(StoredS3TableIntegration {
                    id: id.clone(),
                    region: region.clone(),
                    arn: s3_table_integration_arn(&region, &id),
                    role_arn: "arn:aws:iam::000000000000:role/telemetry-s3-tables".to_string(),
                    sse_algorithm: "AES256".to_string(),
                    kms_key_arn: None,
                    destination_table_bucket_arn: table_bucket_arn(&region, &id),
                    created_timestamp_ms: 1_700_000_000_000,
                    tags: Default::default(),
                });
            }
        }

        let (status, response) = create(
            &emulator,
            HOME_REGION,
            &json!({
                "Encryption": { "SseAlgorithm": "AES256" },
                "RoleArn": "arn:aws:iam::000000000000:role/telemetry-s3-tables",
            }),
        )
        .await;
        assert_eq!(
            status, 402,
            "case {case:?}: expected 402, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ServiceQuotaExceededException",
            "case {case:?}: expected ServiceQuotaExceededException, got {response:?}"
        );
        assert_eq!(
            stored_count(&emulator),
            records,
            "case {case:?}: expected the refused create to allocate nothing, got {} records \
             beside the {records} pre-filled ones",
            stored_count(&emulator)
        );
    }
}
