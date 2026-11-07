// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue security configuration slice:
//! `CreateSecurityConfiguration`, the `GetSecurityConfiguration` that reads back what it
//! stored, the `GetSecurityConfigurations` that pages over them and the
//! `DeleteSecurityConfiguration` that removes one.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The round-trip suite is a table: each row is one input along the axis the rule
//! generalises over (which encryption properties the configuration carries, their modes,
//! whether a KMS key ARN is named, how many S3 encryption entries there are, the form and
//! length of the name), and a configuration carrying a single S3 encryption entry — the
//! shape a lifecycle client sends — is one row among them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateSecurityConfigurationInput;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, SecurityConfigurationEntry, MAX_SECURITY_CONFIGURATIONS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, emulator_on, error_code, error_message, ok};

/// The longest `Name` the model accepts: "len 1..255" on `NameString`
/// (`smithy_query operation=CreateSecurityConfiguration`).
const MAX_NAME_LENGTH: usize = 255;

/// A KMS key ARN of the form `KmsKeyArn`'s pattern admits (`^$|arn:aws[a-z0-9-]*:kms:`,
/// `smithy_query shape=S3Encryption`).
const KMS_KEY_ARN: &str = "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// Create a security configuration and return the response body.
async fn create(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateSecurityConfiguration", body).await
}

/// The `SecurityConfiguration` `GetSecurityConfiguration` reports for `name`.
async fn get(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "GetSecurityConfiguration", &json!({ "Name": name })).await
        ["SecurityConfiguration"]
        .clone()
}

/// One page of `GetSecurityConfigurations`.
async fn list(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "GetSecurityConfigurations", request).await
}

/// The names in a `GetSecurityConfigurations` page, in the order the service returned
/// them.
fn listed_names(page: &Value) -> Vec<String> {
    page["SecurityConfigurations"]
        .as_array()
        .expect("SecurityConfigurations is a list")
        .iter()
        .map(|configuration| {
            configuration["Name"]
                .as_str()
                .expect("SecurityConfiguration.Name is a string")
                .to_string()
        })
        .collect()
}

/// Assert that a body is the refusal a security configuration miss is answered with: the
/// modelled `EntityNotFoundException` and NO message, which is how the recorded refusal
/// for a name this account holds no security configuration under is worded.
fn assert_entity_not_found(case: &str, body: &Value) {
    assert!(
        error_code(body).contains("EntityNotFoundException"),
        "case {case}: expected EntityNotFoundException, got {body}"
    );
    assert_eq!(
        error_message(body),
        "<no Message>",
        "case {case}: the refusal must carry no message, got {body}"
    );
    assert!(
        body.get("message").is_none(),
        "case {case}: the refusal must carry no message, got {body}"
    );
}

/// The rows every suite below drives: one security configuration request per form of
/// encryption configuration the model admits.
fn cases() -> Vec<(&'static str, Value)> {
    let long_name = "a".repeat(MAX_NAME_LENGTH);
    vec![
        (
            // The shape a lifecycle client sends.
            "one S3 encryption entry",
            json!({
                "Name": "s3-sse-s3",
                "EncryptionConfiguration": {
                    "S3Encryption": [{ "S3EncryptionMode": "SSE-S3" }],
                },
            }),
        ),
        (
            "S3 encryption naming a KMS key",
            json!({
                "Name": "s3-sse-kms",
                "EncryptionConfiguration": {
                    "S3Encryption": [{
                        "S3EncryptionMode": "SSE-KMS",
                        "KmsKeyArn": KMS_KEY_ARN,
                    }],
                },
            }),
        ),
        (
            "two S3 encryption entries",
            json!({
                "Name": "s3-two-entries",
                "EncryptionConfiguration": {
                    "S3Encryption": [
                        { "S3EncryptionMode": "SSE-KMS", "KmsKeyArn": KMS_KEY_ARN },
                        { "S3EncryptionMode": "DISABLED" },
                    ],
                },
            }),
        ),
        (
            "CloudWatch encryption only",
            json!({
                "Name": "cloudwatch-only",
                "EncryptionConfiguration": {
                    "CloudWatchEncryption": {
                        "CloudWatchEncryptionMode": "SSE-KMS",
                        "KmsKeyArn": KMS_KEY_ARN,
                    },
                },
            }),
        ),
        (
            "job bookmarks encryption only",
            json!({
                "Name": "job-bookmarks-only",
                "EncryptionConfiguration": {
                    "JobBookmarksEncryption": {
                        "JobBookmarksEncryptionMode": "CSE-KMS",
                        "KmsKeyArn": KMS_KEY_ARN,
                    },
                },
            }),
        ),
        (
            "every encryption property at once",
            json!({
                "Name": "all-properties",
                "EncryptionConfiguration": {
                    "S3Encryption": [{ "S3EncryptionMode": "SSE-S3" }],
                    "CloudWatchEncryption": { "CloudWatchEncryptionMode": "DISABLED" },
                    "JobBookmarksEncryption": { "JobBookmarksEncryptionMode": "DISABLED" },
                    "DataQualityEncryption": {
                        "DataQualityEncryptionMode": "SSE-KMS",
                        "KmsKeyArn": KMS_KEY_ARN,
                    },
                },
            }),
        ),
        (
            // Every member of `EncryptionConfiguration` is optional, so a configuration
            // that encrypts nothing is a configuration the model admits.
            "an encryption configuration carrying no property",
            json!({
                "Name": "no-properties",
                "EncryptionConfiguration": {},
            }),
        ),
        (
            "a name at the modelled length bound",
            json!({
                "Name": long_name,
                "EncryptionConfiguration": {
                    "S3Encryption": [{ "S3EncryptionMode": "SSE-S3" }],
                },
            }),
        ),
        (
            "a name carrying a space and a tab",
            json!({
                "Name": "security config\tone",
                "EncryptionConfiguration": {
                    "CloudWatchEncryption": { "CloudWatchEncryptionMode": "SSE-KMS" },
                },
            }),
        ),
    ]
}

/// The `Name` of a case's request.
fn case_name(request: &Value) -> &str {
    request["Name"].as_str().expect("the case carries a Name")
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateSecurityConfiguration` persists is what `GetSecurityConfiguration`
/// reports: the create answers with the name it stored and the instant it stored it at,
/// and the encryption configuration the request carried comes back from the store
/// unchanged.
#[tokio::test]
async fn created_security_configurations_are_read_back_from_the_store() {
    let router = emulator();
    let cases = cases();

    for (case, request) in &cases {
        let name = case_name(request);

        let created = create(&router, request).await;
        // "The name assigned to the new security configuration."
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: CreateSecurityConfiguration must report the stored name, got \
             {created}"
        );
        // "The time at which the new security configuration was created": a timestamp,
        // after the epoch.
        let created_at = created
            .get("CreatedTimestamp")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {case}: expected a CreatedTimestamp, got {created}")
            });
        assert!(
            created_at > 0.0,
            "case {case}: expected a CreatedTimestamp after the epoch, got {created_at}"
        );

        let read = get(&router, name).await;
        let expected = json!({
            "Name": name,
            "EncryptionConfiguration": request["EncryptionConfiguration"],
        });
        assert_echoed(case, &expected, &read);
        // The read reports the SAME creation instant the create did: the stored one.
        assert_eq!(
            read.get("CreatedTimeStamp").and_then(Value::as_f64),
            Some(created_at),
            "case {case}: the read must report the stored creation instant {created_at}, got \
             {read}"
        );
    }

    // Each configuration is its own record: reading one back after every create has
    // landed still reports that configuration's own encryption properties, so one create
    // never overwrote another.
    for (case, request) in &cases {
        let read = get(&router, case_name(request)).await;
        let expected = as_reported(&request["EncryptionConfiguration"]);
        assert_eq!(
            read.get("EncryptionConfiguration"),
            Some(&expected),
            "case {case}: each configuration keeps its own EncryptionConfiguration, got \
             {read}"
        );
    }
}

/// The `EncryptionConfiguration` `configured` states, as a READ reports it: with
/// `EnforceJdbcSsl` beside the members the request carried.
///
/// The service reports whether a configuration enforces SSL on the JDBC connections made
/// under it inside every encryption configuration it answers with. The published
/// `EncryptionConfiguration` carries no member a create could state it from, so every
/// stored configuration reports it `false` — a value rather than an absence, so a client
/// auditing JDBC SSL enforcement has something to read.
fn as_reported(configured: &Value) -> Value {
    let mut reported = configured.clone();
    reported["EnforceJdbcSsl"] = json!(false);
    reported
}

/// Two reads of one configuration agree, and a read is not a write: the creation instant
/// a read reports does not move, however many reads there are.
#[tokio::test]
async fn reading_a_security_configuration_changes_nothing() {
    let router = emulator();

    for (case, request) in cases() {
        create(&router, &request).await;
        let name = case_name(&request);
        let first = get(&router, name).await;
        let second = get(&router, name).await;
        assert_eq!(
            first, second,
            "case {case}: two reads of one configuration must agree; first {first}, second \
             {second}"
        );
    }
}

// ---------------------------------------------------------------------------
// The listing
// ---------------------------------------------------------------------------

/// `GetSecurityConfigurations` reports every stored configuration, in name order, and
/// pages over them: the rows are page sizes, and whatever the size, paging to exhaustion
/// yields every stored name exactly once.
#[tokio::test]
async fn the_listing_pages_over_every_stored_configuration_in_name_order() {
    let router = emulator();
    let requests = cases();
    for (_, request) in &requests {
        create(&router, request).await;
    }

    let mut expected: Vec<String> = requests
        .iter()
        .map(|(_, request)| case_name(request).to_string())
        .collect();
    expected.sort();

    // One page large enough for everything stored reports them all, in name order.
    let whole = list(&router, &json!({})).await;
    assert_eq!(
        listed_names(&whole),
        expected,
        "case one page: expected every stored configuration in name order, got {whole}"
    );
    assert!(
        whole.get("NextToken").is_none_or(Value::is_null),
        "case one page: a page that ends the listing carries no NextToken, got {whole}"
    );

    // Each row is a page size; the walk collects the names page by page.
    for page_size in [1_usize, 2, 3, expected.len(), expected.len() + 1] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // Bounded by one request per stored configuration plus the final empty-tokened
        // page, so a token that failed to advance cannot loop forever.
        for _ in 0..expected.len() + 1 {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(cursor) = &token {
                request["NextToken"] = json!(cursor);
            }
            let page = list(&router, &request).await;
            let names = listed_names(&page);
            assert!(
                names.len() <= page_size,
                "case page size {page_size}: a page must hold at most {page_size} entries, \
                 got {page}"
            );
            collected.extend(names);
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case page size {page_size}: the walk must end without a NextToken, got {token:?}"
        );
        assert_eq!(
            collected, expected,
            "case page size {page_size}: paging must yield every stored name exactly once, \
             got {collected:?}"
        );
    }
}

/// Every configuration the listing reports carries the same members a single read does,
/// so a client that paged over the namespace and one that read a record by name see one
/// resource.
#[tokio::test]
async fn the_listing_reports_what_a_single_read_reports() {
    let router = emulator();
    let requests = cases();
    for (_, request) in &requests {
        create(&router, request).await;
    }

    let page = list(&router, &json!({})).await;
    let listed = page["SecurityConfigurations"]
        .as_array()
        .expect("SecurityConfigurations is a list")
        .clone();

    for (case, request) in &requests {
        let name = case_name(request);
        let read = get(&router, name).await;
        let from_listing = listed
            .iter()
            .find(|configuration| configuration["Name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("case {case}: {name} must appear in the listing {page}"));
        assert_eq!(
            from_listing, &read,
            "case {case}: the listing entry must be the record a read reports; listing \
             {from_listing}, read {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// A deleted configuration is served no more: the read misses with the modelled
/// `EntityNotFoundException`, the listing drops it, and the configurations beside it are
/// untouched.
#[tokio::test]
async fn a_deleted_security_configuration_is_served_no_more() {
    let router = emulator();
    let requests = cases();
    for (_, request) in &requests {
        create(&router, request).await;
    }

    let mut remaining: Vec<String> = requests
        .iter()
        .map(|(_, request)| case_name(request).to_string())
        .collect();
    remaining.sort();

    // Each row deletes one configuration; after each delete the rest are still served.
    for (case, request) in &requests {
        let name = case_name(request);
        ok(
            &router,
            "DeleteSecurityConfiguration",
            &json!({ "Name": name }),
        )
        .await;

        let (status, body) =
            call(&router, "GetSecurityConfiguration", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: a read of a deleted configuration must be refused with 400, got \
             {status} with {body}"
        );
        assert_entity_not_found(case, &body);

        remaining.retain(|stored| stored != name);
        let page = list(&router, &json!({})).await;
        assert_eq!(
            listed_names(&page),
            remaining,
            "case {case}: the listing must drop only the deleted configuration, got {page}"
        );
    }
}

/// A delete for a name this account holds no configuration under is refused with the
/// modelled `EntityNotFoundException`, including a second delete of a configuration that
/// is already gone.
#[tokio::test]
async fn deleting_an_unknown_security_configuration_is_refused() {
    let router = emulator();
    create(
        &router,
        &json!({
            "Name": "StoredConfiguration",
            "EncryptionConfiguration": { "S3Encryption": [{ "S3EncryptionMode": "SSE-S3" }] },
        }),
    )
    .await;
    ok(
        &router,
        "DeleteSecurityConfiguration",
        &json!({ "Name": "StoredConfiguration" }),
    )
    .await;

    for (case, name) in [
        ("never created", "no-such-configuration"),
        ("already deleted", "StoredConfiguration"),
        ("a prefix of a deleted name", "Stored"),
    ] {
        let (status, body) = call(
            &router,
            "DeleteSecurityConfiguration",
            &json!({ "Name": name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {name:?}, got {status} with {body}"
        );
        assert_entity_not_found(case, &body);
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A configuration name is unique per account: the second create under a name already
/// taken is refused with the modelled `AlreadyExistsException`, and the stored
/// configuration is left as the first create wrote it.
#[tokio::test]
async fn a_duplicate_security_configuration_name_is_refused_and_changes_nothing() {
    let router = emulator();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "same name, different S3 encryption mode",
            json!({
                "Name": "duplicate-one",
                "EncryptionConfiguration": { "S3Encryption": [{ "S3EncryptionMode": "SSE-S3" }] },
            }),
            json!({
                "Name": "duplicate-one",
                "EncryptionConfiguration": {
                    "S3Encryption": [{ "S3EncryptionMode": "SSE-KMS", "KmsKeyArn": KMS_KEY_ARN }],
                },
            }),
        ),
        (
            "same name, a property added",
            json!({
                "Name": "duplicate-two",
                "EncryptionConfiguration": { "S3Encryption": [{ "S3EncryptionMode": "SSE-S3" }] },
            }),
            json!({
                "Name": "duplicate-two",
                "EncryptionConfiguration": {
                    "S3Encryption": [{ "S3EncryptionMode": "SSE-S3" }],
                    "CloudWatchEncryption": { "CloudWatchEncryptionMode": "DISABLED" },
                },
            }),
        ),
        (
            "same name and the identical definition",
            json!({
                "Name": "duplicate-three",
                "EncryptionConfiguration": {
                    "JobBookmarksEncryption": { "JobBookmarksEncryptionMode": "CSE-KMS" },
                },
            }),
            json!({
                "Name": "duplicate-three",
                "EncryptionConfiguration": {
                    "JobBookmarksEncryption": { "JobBookmarksEncryptionMode": "CSE-KMS" },
                },
            }),
        ),
    ];

    for (case, first, second) in &cases {
        let created = create(&router, first).await;
        let created_at = created
            .get("CreatedTimestamp")
            .and_then(Value::as_f64)
            .expect("the create reports a CreatedTimestamp");

        let (status, body) = call(&router, "CreateSecurityConfiguration", second).await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name must be refused with 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );

        // The stored configuration is still the FIRST create's, properties and creation
        // instant alike.
        let name = case_name(first);
        let read = get(&router, name).await;
        let expected = json!({
            "Name": name,
            "EncryptionConfiguration": first["EncryptionConfiguration"],
        });
        assert_echoed(case, &expected, &read);
        assert_eq!(
            read.get("CreatedTimeStamp").and_then(Value::as_f64),
            Some(created_at),
            "case {case}: the refused create must not move the creation instant, got {read}"
        );
    }
}

/// A read for a name this account holds no configuration under is refused with the
/// modelled `EntityNotFoundException`. The rows differ along how the requested name
/// misses: never created, a prefix of a stored name, a suffix of one, and a stored name in
/// another case — the model documents no case fold for a configuration name, so a
/// differently-cased name is a different configuration.
#[tokio::test]
async fn reading_an_unknown_security_configuration_is_refused() {
    let router = emulator();
    create(
        &router,
        &json!({
            "Name": "StoredConfiguration",
            "EncryptionConfiguration": { "S3Encryption": [{ "S3EncryptionMode": "SSE-S3" }] },
        }),
    )
    .await;

    for (case, name) in [
        ("never created", "no-such-configuration"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Configuration"),
        ("a stored name lowercased", "storedconfiguration"),
        ("a stored name uppercased", "STOREDCONFIGURATION"),
    ] {
        let (status, body) =
            call(&router, "GetSecurityConfiguration", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {name:?}, got {status} with {body}"
        );
        assert_entity_not_found(case, &body);
    }

    // The name as stored still resolves, so the misses above are misses and not a broken
    // lookup.
    let read = get(&router, "StoredConfiguration").await;
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some("StoredConfiguration"),
        "case the stored name: expected the stored configuration, got {read}"
    );
}

/// The store holds a bounded number of security configurations: the create at the ceiling
/// succeeds, the one past it is refused with the modelled limit error, and freeing a slot
/// lets a create succeed again — so the ceiling bounds what is stored rather than latching
/// on how many creates were ever made.
#[tokio::test]
async fn a_create_past_the_security_configuration_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_SECURITY_CONFIGURATIONS - 1 {
            let name = format!("seeded-{index}");
            let input = CreateSecurityConfigurationInput {
                name: name.clone(),
                ..CreateSecurityConfigurationInput::default()
            };
            store.security_configurations.insert(
                name,
                SecurityConfigurationEntry::from_create_input(&input, std::time::SystemTime::now()),
            );
        }
    }

    let last = json!({
        "Name": "last-configuration",
        "EncryptionConfiguration": { "S3Encryption": [{ "S3EncryptionMode": "SSE-S3" }] },
    });
    let past = json!({
        "Name": "one-too-many",
        "EncryptionConfiguration": { "S3Encryption": [{ "S3EncryptionMode": "SSE-S3" }] },
    });

    create(&router, &last).await;
    let (status, body) = call(&router, "CreateSecurityConfiguration", &past).await;
    assert_eq!(
        status, 400,
        "case configuration {} of {MAX_SECURITY_CONFIGURATIONS}: expected 400, got {status} \
         with {body}",
        MAX_SECURITY_CONFIGURATIONS + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one configuration too many: expected ResourceNumberLimitExceededException, got \
         {body}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().security_configurations.remove("seeded-0");
    }
    create(&router, &past).await;
}
