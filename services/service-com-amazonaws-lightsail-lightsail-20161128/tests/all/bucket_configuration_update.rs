// SPDX-License-Identifier: CC0-1.0

//! `UpdateBucket` — changing the configuration of a stored Lightsail bucket,
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has.
//!
//! The rule under test: an update applies to the ONE stored bucket record the
//! request addresses in the Region it is made in — each member the request
//! carries replaces what that record held, and a member it carries none of
//! leaves the stored value alone — so a later read answers exactly what the
//! update left behind, and a refused request leaves the record as it was.
//!
//! So the cases are TABLES: one row per member the request can carry and per
//! form that member takes (both access rules and each on its own, both
//! versioning states, a read-only account list of none, one and the ten the
//! member admits, the access log on with and without a prefix and off, a CORS
//! configuration of one rule, of a rule carrying every member, of the twenty
//! rules the configuration admits and of none at all, and every member at once);
//! the refusals are a table of the ways a request can be outside what the
//! service serves, each asserted to leave the stored record untouched.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::BUCKET_VERSIONING_REQUEST_VALUES;

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The endpoint these cases reach the emulator on (its `Host`), which is the
/// address a bucket's `url` is announced under.
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The Region every case is made in: one client, one Region, the way a client
/// that creates a bucket and then configures it is configured.
const REGION: &str = "us-east-1";

/// The bundle every bucket here is created on — the cheapest published plan
/// (`crate::state::BUCKET_BUNDLE_SPECS`).
const BUNDLE_ID: &str = "small_1_0";

/// The bucket every case that turns the access log on saves its logs in: "the
/// destination can be a Lightsail bucket in the same account, and in the same
/// Amazon Web Services Region as the source bucket"
/// (`BucketAccessLogConfig$destination`).
const LOG_DESTINATION: &str = "access-log-destination";

/// The most CORS rules one configuration holds, from `BucketCorsConfig$rules`:
/// "You can add up to 20 rules to the configuration."
const MAX_CORS_RULES: usize = 20;

/// The most accounts one bucket can be opened to, from
/// `UpdateBucketRequest$readonlyAccessAccounts`: "You can give a maximum of 10
/// Amazon Web Services accounts access to a bucket."
const MAX_READONLY_ACCESS_ACCOUNTS: usize = 10;

/// Create the bucket `name` in [`REGION`], asking for object versioning when
/// `versioned`, and answer the record the create reported.
async fn create(emulator: &Emulator, name: &str, versioned: bool) -> Value {
    let mut body = json!({"bucketName": name, "bundleId": BUNDLE_ID});
    if versioned {
        body["enableObjectVersioning"] = json!(true);
    }
    emulator
        .ok_at(Some(ENDPOINT_HOST), Some(REGION), "CreateBucket", body)
        .await["bucket"]
        .clone()
}

/// The bucket `name` as `GetBuckets` reports it, asking for its CORS
/// configuration when `include_cors`.
async fn read(emulator: &Emulator, name: &str, include_cors: bool) -> Value {
    let body = if include_cors {
        json!({"bucketName": name, "includeCors": true})
    } else {
        json!({"bucketName": name})
    };
    let answered = emulator
        .ok_at(Some(ENDPOINT_HOST), Some(REGION), "GetBuckets", body)
        .await;
    answered["buckets"][0].clone()
}

/// Call `UpdateBucket` on `name` with the members `patch` carries, answering the
/// HTTP status and the parsed body.
async fn update(emulator: &Emulator, name: &str, patch: &Value) -> (u16, Value) {
    let mut body = patch.clone();
    body["bucketName"] = json!(name);
    emulator
        .call_at(Some(ENDPOINT_HOST), Some(REGION), "UpdateBucket", body)
        .await
}

/// Call `UpdateBucket` on `name` and require success, answering the bucket it
/// reports.
async fn update_ok(emulator: &Emulator, case: &str, name: &str, patch: &Value) -> Value {
    let (status, answered) = update(emulator, name, patch).await;
    assert_eq!(
        status, 200,
        "case {case}: the update must succeed, got {answered:?}"
    );
    answered["bucket"].clone()
}

/// A list of `count` distinct Amazon Web Services account IDs — 12 digits each,
/// which is what an account is identified by.
fn accounts(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!(format!("{:012}", 100_000_000_000_u64 + index as u64)))
            .collect(),
    )
}

/// A CORS rule allowing one method from one origin, distinct per `index`.
fn cors_rule(index: usize) -> Value {
    json!({
        "id": format!("rule-{index}"),
        "allowedMethods": ["GET"],
        "allowedOrigins": [format!("https://origin-{index}.test")],
    })
}

/// A list of `count` distinct CORS rules.
fn cors_rules(count: usize) -> Value {
    Value::Array((0..count).map(cors_rule).collect())
}

/// A CORS rule carrying every member the modelled `BucketCorsRule` declares,
/// with every HTTP method that shape's `allowedMethods` pattern
/// `^(DELETE|GET|HEAD|POST|PUT)$` admits.
fn cors_rule_with_every_member() -> Value {
    json!({
        "id": "every-member",
        "allowedMethods": ["DELETE", "GET", "HEAD", "POST", "PUT"],
        "allowedOrigins": ["https://one.test", "https://two.test"],
        "allowedHeaders": ["x-amz-request-id", "content-type"],
        "exposeHeaders": ["etag"],
        "maxAgeSeconds": 3600,
    })
}

/// One accepted update case.
struct Row {
    case: &'static str,
    /// Whether the bucket this row updates is created with object versioning
    /// enabled — the state a row that suspends versioning starts from.
    versioned_at_create: bool,
    /// The members the request carries besides `bucketName`.
    patch: Value,
    /// Member name to the value the bucket must report afterwards. EVERY other
    /// member of the record must be exactly what the create answered.
    expected: Value,
    /// The `cors.rules` the request's CORS configuration leaves behind, when it
    /// carried one — the answered record reports `cors` exactly then.
    expected_cors_rules: Option<Value>,
}

/// The accepted cases: one row per member of the request and per form it takes.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the anonymous access of the bucket's objects is widened",
            versioned_at_create: false,
            patch: json!({"accessRules": {"getObject": "public"}}),
            expected: json!({"accessRules": {"getObject": "public", "allowPublicOverrides": false}}),
            expected_cors_rules: None,
        },
        Row {
            case: "only the per-object ACL override is changed",
            versioned_at_create: false,
            patch: json!({"accessRules": {"allowPublicOverrides": true}}),
            expected: json!({"accessRules": {"getObject": "private", "allowPublicOverrides": true}}),
            expected_cors_rules: None,
        },
        Row {
            case: "both access rules at once",
            versioned_at_create: false,
            patch: json!({"accessRules": {"getObject": "public", "allowPublicOverrides": true}}),
            expected: json!({"accessRules": {"getObject": "public", "allowPublicOverrides": true}}),
            expected_cors_rules: None,
        },
        Row {
            case: "versioning is enabled on a bucket that never had it",
            versioned_at_create: false,
            patch: json!({"versioning": "Enabled"}),
            expected: json!({"objectVersioning": "Enabled"}),
            expected_cors_rules: None,
        },
        Row {
            case: "versioning is suspended on a bucket that has it",
            versioned_at_create: true,
            patch: json!({"versioning": "Suspended"}),
            expected: json!({"objectVersioning": "Suspended"}),
            expected_cors_rules: None,
        },
        Row {
            case: "versioning is asked for again on a bucket that already has it",
            versioned_at_create: true,
            patch: json!({"versioning": "Enabled"}),
            expected: json!({"objectVersioning": "Enabled"}),
            expected_cors_rules: None,
        },
        Row {
            case: "one account is given read-only access",
            versioned_at_create: false,
            patch: json!({"readonlyAccessAccounts": accounts(1)}),
            expected: json!({"readonlyAccessAccounts": accounts(1)}),
            expected_cors_rules: None,
        },
        Row {
            case: "as many accounts as the member admits",
            versioned_at_create: false,
            patch: json!({"readonlyAccessAccounts": accounts(MAX_READONLY_ACCESS_ACCOUNTS)}),
            expected: json!({"readonlyAccessAccounts": accounts(MAX_READONLY_ACCESS_ACCOUNTS)}),
            expected_cors_rules: None,
        },
        Row {
            case: "an empty list of accounts leaves the bucket open to none",
            versioned_at_create: false,
            patch: json!({"readonlyAccessAccounts": accounts(0)}),
            expected: json!({"readonlyAccessAccounts": accounts(0)}),
            expected_cors_rules: None,
        },
        Row {
            case: "the access log is turned on with a prefix",
            versioned_at_create: false,
            patch: json!({
                "accessLogConfig": {
                    "enabled": true,
                    "destination": LOG_DESTINATION,
                    "prefix": "logs/",
                }
            }),
            expected: json!({
                "accessLogConfig": {
                    "enabled": true,
                    "destination": LOG_DESTINATION,
                    "prefix": "logs/",
                }
            }),
            expected_cors_rules: None,
        },
        Row {
            case: "the access log is turned on without a prefix",
            versioned_at_create: false,
            patch: json!({"accessLogConfig": {"enabled": true, "destination": LOG_DESTINATION}}),
            expected: json!({"accessLogConfig": {"enabled": true, "destination": LOG_DESTINATION}}),
            expected_cors_rules: None,
        },
        Row {
            case: "the access log is left off",
            versioned_at_create: false,
            patch: json!({"accessLogConfig": {"enabled": false}}),
            expected: json!({"accessLogConfig": {"enabled": false}}),
            expected_cors_rules: None,
        },
        Row {
            case: "one CORS rule",
            versioned_at_create: false,
            patch: json!({"cors": {"rules": cors_rules(1)}}),
            expected: json!({}),
            expected_cors_rules: Some(cors_rules(1)),
        },
        Row {
            case: "a CORS rule carrying every member",
            versioned_at_create: false,
            patch: json!({"cors": {"rules": [cors_rule_with_every_member()]}}),
            expected: json!({}),
            expected_cors_rules: Some(json!([cors_rule_with_every_member()])),
        },
        Row {
            case: "as many CORS rules as the configuration admits",
            versioned_at_create: false,
            patch: json!({"cors": {"rules": cors_rules(MAX_CORS_RULES)}}),
            expected: json!({}),
            expected_cors_rules: Some(cors_rules(MAX_CORS_RULES)),
        },
        Row {
            case: "an empty list of CORS rules",
            versioned_at_create: false,
            patch: json!({"cors": {"rules": []}}),
            expected: json!({}),
            expected_cors_rules: Some(json!([])),
        },
        Row {
            case: "a CORS configuration carrying no rules member at all",
            versioned_at_create: false,
            patch: json!({"cors": {}}),
            expected: json!({}),
            expected_cors_rules: Some(json!([])),
        },
        Row {
            case: "every member the request can carry, at once",
            versioned_at_create: false,
            patch: json!({
                "accessRules": {"getObject": "public", "allowPublicOverrides": true},
                "versioning": "Enabled",
                "readonlyAccessAccounts": accounts(2),
                "accessLogConfig": {"enabled": true, "destination": LOG_DESTINATION},
                "cors": {"rules": cors_rules(2)},
            }),
            expected: json!({
                "accessRules": {"getObject": "public", "allowPublicOverrides": true},
                "objectVersioning": "Enabled",
                "readonlyAccessAccounts": accounts(2),
                "accessLogConfig": {"enabled": true, "destination": LOG_DESTINATION},
            }),
            expected_cors_rules: Some(cors_rules(2)),
        },
        Row {
            case: "a request that names nothing but the bucket",
            versioned_at_create: false,
            patch: json!({}),
            expected: json!({}),
            expected_cors_rules: None,
        },
    ]
}

/// Every accepted case: the update applies exactly the members the request
/// carries, leaves every other member of the record alone, and the read answers
/// the very same record.
#[tokio::test]
async fn an_update_applies_the_members_it_carries_and_leaves_the_rest_alone() {
    let emulator = Emulator::new();
    create(&emulator, LOG_DESTINATION, false).await;

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let name = format!("configured-bucket-{index}");
        let created = create(&emulator, &name, row.versioned_at_create).await;
        let updated = update_ok(&emulator, case, &name, &row.patch).await;

        let named = row
            .expected
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: the expected members are an object"));
        for (member, expected) in named {
            assert_eq!(
                &updated[member], expected,
                "case {case}: expected {member} to be {expected:?}, got {:?}",
                updated[member]
            );
        }

        // A member the request carries none of is left alone, so the rest of the
        // record is exactly what the create answered.
        let changed: BTreeSet<&str> = named.keys().map(String::as_str).collect();
        let created_members = created
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: a bucket is an object, got {created:?}"));
        for (member, before) in created_members {
            if changed.contains(member.as_str()) {
                continue;
            }
            assert_eq!(
                &updated[member], before,
                "case {case}: this request names no {member}, so it must be unchanged, got {:?}",
                updated[member]
            );
        }

        // `Bucket$cors` "is only included in the response when CORS configuration
        // is requested", and an update requests it by carrying one.
        match &row.expected_cors_rules {
            Some(expected) => assert_eq!(
                &updated["cors"]["rules"], expected,
                "case {case}: expected the CORS rules the request carried, got {:?}",
                updated["cors"]
            ),
            None => assert!(
                updated["cors"].is_null(),
                "case {case}: this request updates no CORS policy, so the answer carries no \
                 cors, got {:?}",
                updated["cors"]
            ),
        }

        // The round trip: the stored record is what a later read answers from.
        let read_back = read(&emulator, &name, row.expected_cors_rules.is_some()).await;
        assert_eq!(
            read_back, updated,
            "case {case}: expected the read to answer the record the update stored, got \
             {read_back:?}"
        );
    }
}

/// One refused update case.
struct Refusal {
    case: &'static str,
    /// The bucket the request addresses, or `None` for the configured bucket
    /// this suite holds.
    bucket_name: Option<&'static str>,
    /// The members the request carries besides `bucketName`.
    patch: Value,
    /// The wire error codes this refusal may carry: the service's own
    /// `InvalidInputException` for a rule the model cannot express,
    /// `NotFoundException` for a bucket this account does not hold, and the
    /// centralized walk's `ValidationException` for a modelled bound, which is
    /// refused before the handler runs at all.
    codes: &'static [&'static str],
}

/// The refused cases: one row per way a request can be outside what the service
/// serves.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            case: "a bucket this account does not hold",
            bucket_name: Some("no-such-bucket"),
            patch: json!({"versioning": "Enabled"}),
            codes: &["NotFoundException"],
        },
        Refusal {
            case: "a versioning state spelled in another case",
            bucket_name: None,
            patch: json!({"versioning": "enabled"}),
            codes: &["InvalidInputException"],
        },
        Refusal {
            case: "the versioning state a bucket can only be in, never asked for",
            bucket_name: None,
            patch: json!({"versioning": "NeverEnabled"}),
            codes: &["InvalidInputException"],
        },
        Refusal {
            case: "a versioning state no bucket has",
            bucket_name: None,
            patch: json!({"versioning": "Disabled"}),
            codes: &["InvalidInputException"],
        },
        Refusal {
            case: "the access log enabled with no destination to save logs in",
            bucket_name: None,
            patch: json!({"accessLogConfig": {"enabled": true}}),
            codes: &["InvalidInputException"],
        },
        Refusal {
            case: "a destination carried while the access log is disabled",
            bucket_name: None,
            patch: json!({"accessLogConfig": {"enabled": false, "destination": LOG_DESTINATION}}),
            codes: &["InvalidInputException"],
        },
        Refusal {
            case: "a prefix carried while the access log is disabled",
            bucket_name: None,
            patch: json!({"accessLogConfig": {"enabled": false, "prefix": "logs/"}}),
            codes: &["InvalidInputException"],
        },
        Refusal {
            case: "a destination this account holds no bucket under",
            bucket_name: None,
            patch: json!({"accessLogConfig": {"enabled": true, "destination": "no-such-bucket"}}),
            codes: &["InvalidInputException"],
        },
        Refusal {
            case: "a destination bucket of another Region",
            bucket_name: None,
            patch: json!({"accessLogConfig": {"enabled": true, "destination": "other-region-bucket"}}),
            codes: &["InvalidInputException"],
        },
        Refusal {
            case: "a CORS rule naming no method",
            bucket_name: None,
            patch: json!({
                "cors": {"rules": [{"allowedMethods": [], "allowedOrigins": ["https://one.test"]}]}
            }),
            codes: &["InvalidInputException"],
        },
        Refusal {
            case: "a CORS rule naming no origin",
            bucket_name: None,
            patch: json!({"cors": {"rules": [{"allowedMethods": ["GET"], "allowedOrigins": []}]}}),
            codes: &["InvalidInputException"],
        },
        Refusal {
            case: "a read-only account that is not an account ID at all",
            bucket_name: None,
            patch: json!({"readonlyAccessAccounts": ["an-account-name"]}),
            codes: &["InvalidInputException"],
        },
        Refusal {
            case: "a read-only account one digit short of an account ID",
            bucket_name: None,
            patch: json!({"readonlyAccessAccounts": ["12345678901"]}),
            codes: &["InvalidInputException"],
        },
        Refusal {
            case: "one account more than the member admits",
            bucket_name: None,
            patch: json!({
                "readonlyAccessAccounts": accounts(MAX_READONLY_ACCESS_ACCOUNTS + 1)
            }),
            codes: &["ValidationException"],
        },
        Refusal {
            case: "one CORS rule more than the configuration admits",
            bucket_name: None,
            patch: json!({"cors": {"rules": cors_rules(MAX_CORS_RULES + 1)}}),
            codes: &["ValidationException"],
        },
    ]
}

/// Every refused case: the request is refused with a 400 and the stored record is
/// exactly what it was before the call, so nothing of a refused update is half
/// applied.
#[tokio::test]
async fn a_refused_update_changes_nothing_of_the_stored_record() {
    let emulator = Emulator::new();
    create(&emulator, LOG_DESTINATION, false).await;
    // A bucket of ANOTHER Region, so the destination rule is asserted on a name
    // that does name a bucket of this account — just not one in this Region.
    emulator
        .ok_at(
            Some(ENDPOINT_HOST),
            Some("us-west-2"),
            "CreateBucket",
            json!({"bucketName": "other-region-bucket", "bundleId": BUNDLE_ID}),
        )
        .await;

    // A bucket with every configurable member already set to something other
    // than its created value, so a refusal that wiped one would show.
    let name = "configured-source";
    create(&emulator, name, true).await;
    update_ok(
        &emulator,
        "the configuration every refusal is asserted against",
        name,
        &json!({
            "accessRules": {"getObject": "public", "allowPublicOverrides": true},
            "versioning": "Suspended",
            "readonlyAccessAccounts": accounts(3),
            "accessLogConfig": {"enabled": true, "destination": LOG_DESTINATION, "prefix": "logs/"},
            "cors": {"rules": cors_rules(2)},
        }),
    )
    .await;
    let before = read(&emulator, name, true).await;

    for refusal in refusals() {
        let case = refusal.case;
        let addressed = refusal.bucket_name.unwrap_or(name);
        let (status, body) = update(&emulator, addressed, &refusal.patch).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the request to be refused, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert!(
            refusal.codes.contains(&code.as_str()),
            "case {case}: expected one of {:?}, got {code:?} in {body:?}",
            refusal.codes
        );
        // Lightsail answers a name it cannot find with `DoesNotExist` in the
        // body, which is what an SDK client branches on.
        if code == "NotFoundException" {
            assert_eq!(
                body["code"].as_str(),
                Some("DoesNotExist"),
                "case {case}: expected the DoesNotExist envelope, got {body:?}"
            );
        }
        let after = read(&emulator, name, true).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must leave the record as it was, got {after:?}"
        );
    }
}

/// An update writes ONE terminal operation-log entry for the bucket it changed,
/// and that entry is in the log a later read of the resource's operations walks.
#[tokio::test]
async fn an_update_logs_one_terminal_operation_for_the_bucket() {
    let emulator = Emulator::new();
    let name = "logged-bucket";
    create(&emulator, name, false).await;

    let (status, answered) = update(&emulator, name, &json!({"versioning": "Enabled"})).await;
    assert_eq!(
        status, 200,
        "case the logged update: the update must succeed, got {answered:?}"
    );
    let operations = answered["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case the logged update: expected an operations array, got {answered:?}"))
        .clone();
    assert_eq!(
        operations.len(),
        1,
        "case the logged update: expected one operation for one updated bucket, got \
         {operations:?}"
    );
    let operation = &operations[0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some("UpdateBucket"),
        "case the logged update: expected the UpdateBucket operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some("Bucket"),
        "case the logged update: expected the Bucket resource type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(name),
        "case the logged update: expected the operation to name this bucket, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Succeeded"),
        "case the logged update: the configuration is in force as the call returns, got \
         {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case the logged update: nothing is owed on this operation, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(REGION),
        "case the logged update: expected the operation in the region of the request, got \
         {operation:?}"
    );
    let id = operation["id"].as_str().unwrap_or_else(|| {
        panic!("case the logged update: expected an operation id, got {operation:?}")
    });
    assert!(
        is_uuid_shaped(id),
        "case the logged update: expected a uuid-shaped operation id, got {id:?}"
    );

    // The entry was written to the log, not only answered: the resource's
    // operations carry it alongside the one the create wrote.
    let logged = emulator
        .ok_at(
            Some(ENDPOINT_HOST),
            Some(REGION),
            "GetOperationsForResource",
            json!({"resourceName": name}),
        )
        .await;
    let types: BTreeSet<String> = logged["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case the logged update: expected an operations array, got {logged:?}"))
        .iter()
        .filter_map(|entry| entry["operationType"].as_str().map(str::to_string))
        .collect();
    assert!(
        types.contains("UpdateBucket") && types.contains("CreateBucket"),
        "case the logged update: expected the log to hold both entries, got {types:?}"
    );
}

/// Object versioning is a property of the store that KEEPS the bucket's objects,
/// so both states an update can ask for reach that object store: what the control
/// plane reports is what the data plane does.
///
/// The rows are the two states `UpdateBucketRequest$versioning` publishes, read
/// from the service's own catalog of them.
#[tokio::test]
async fn the_versioning_an_update_asks_for_reaches_the_object_store() {
    let emulator = Emulator::new();

    for asked in BUCKET_VERSIONING_REQUEST_VALUES {
        let case = *asked;
        let name = format!("versioned-bucket-{}", asked.to_lowercase());
        create(&emulator, &name, false).await;
        let updated = update_ok(&emulator, case, &name, &json!({"versioning": asked})).await;
        assert_eq!(
            updated["objectVersioning"].as_str(),
            Some(*asked),
            "case {case}: expected the bucket to report the state asked for, got {updated:?}"
        );

        let object_store =
            ::service_com_amazonaws_s3_amazons3::state::service_state(emulator.engine())
                .expect("the shared engine holds the one object store");
        let status = object_store
            .lock()
            .bucket(&name)
            .unwrap_or_else(|| panic!("case {case}: the bucket is served by the object store"))
            .versioning
            .as_ref()
            .map(|versioning| versioning.status.clone());
        assert_eq!(
            status.as_deref(),
            Some(*asked),
            "case {case}: expected the object store to keep the bucket's objects the way the \
             bucket now reports, got {status:?}"
        );
    }
}

/// Two updates in a row accumulate on the ONE record: the second changes only
/// what it names, so what the first set and the second is silent about is still
/// there — including the CORS rules, which are reported only when a read asks
/// for them.
#[tokio::test]
async fn a_later_update_leaves_what_an_earlier_one_set_and_names_nothing_of() {
    let emulator = Emulator::new();
    let name = "accumulating-bucket";
    create(&emulator, name, false).await;

    update_ok(
        &emulator,
        "the first update",
        name,
        &json!({
            "accessRules": {"getObject": "public", "allowPublicOverrides": true},
            "readonlyAccessAccounts": accounts(2),
            "cors": {"rules": cors_rules(3)},
        }),
    )
    .await;

    // A second update that names only the versioning state.
    let second = update_ok(
        &emulator,
        "the second update",
        name,
        &json!({"versioning": "Enabled"}),
    )
    .await;
    assert!(
        second["cors"].is_null(),
        "case the second update: it updates no CORS policy, so the answer carries no cors, got \
         {:?}",
        second["cors"]
    );
    assert_eq!(
        second["accessRules"],
        json!({"getObject": "public", "allowPublicOverrides": true}),
        "case the second update: the access rules the first update set are still there, got {:?}",
        second["accessRules"]
    );
    assert_eq!(
        second["readonlyAccessAccounts"],
        accounts(2),
        "case the second update: the accounts the first update named are still there, got {:?}",
        second["readonlyAccessAccounts"]
    );
    assert_eq!(
        second["objectVersioning"].as_str(),
        Some("Enabled"),
        "case the second update: expected the state it asked for, got {second:?}"
    );

    // The CORS rules the first update stored are still stored: a read that asks
    // for them answers them.
    let read_back = read(&emulator, name, true).await;
    assert_eq!(
        read_back["cors"]["rules"],
        cors_rules(3),
        "case the second update: it names no cors, so the stored rules are left alone, got {:?}",
        read_back["cors"]
    );

    // And a third update replaces that configuration whole.
    let third = update_ok(
        &emulator,
        "the third update",
        name,
        &json!({"cors": {"rules": cors_rules(1)}}),
    )
    .await;
    assert_eq!(
        third["cors"]["rules"],
        cors_rules(1),
        "case the third update: a CORS configuration replaces the one before it, got {:?}",
        third["cors"]
    );
    assert_eq!(
        read(&emulator, name, true).await["cors"]["rules"],
        cors_rules(1),
        "case the third update: the read answers the replaced configuration"
    );

    // The two access rules are two settings of their own, so a later update
    // carrying one of them leaves the other as an earlier one set it.
    let fourth = update_ok(
        &emulator,
        "the fourth update",
        name,
        &json!({"accessRules": {"getObject": "private"}}),
    )
    .await;
    assert_eq!(
        fourth["accessRules"],
        json!({"getObject": "private", "allowPublicOverrides": true}),
        "case the fourth update: it names no allowPublicOverrides, so the one the first \
         update set is left alone, got {:?}",
        fourth["accessRules"]
    );
}
