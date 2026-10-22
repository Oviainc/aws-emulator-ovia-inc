// SPDX-License-Identifier: CC0-1.0

//! The Amazon Keyspaces keyspace/table lifecycle, driven end to end over the service's
//! own `awsJson1_0` wire protocol (`POST /`, `X-Amz-Target: KeyspacesService.<Op>`,
//! SigV4 signing name `cassandra`).
//!
//! Every assertion here goes through the HTTP surface a real client uses, so what is
//! pinned is the emulator's observable behaviour, not its internals: a create is only
//! believed once a read reports it back.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. A create mints the published ARN for the resource it stored, and a second create
//!    of the same name conflicts.
//! 2. A table reads back exactly as it was created, with the modelled defaults filled in
//!    for the specifications the request omitted.
//! 3. `CREATING` is a transient status the clock settles into `ACTIVE`; `DELETING` is one
//!    it settles into absence.
//! 4. `ListTables` pages a keyspace's tables in one total order, resuming exactly where
//!    the previous page stopped, for any page size.
//! 5. A request that breaks a rule the model cannot state is refused with the error the
//!    model declares for it, never with a 501 and never by storing it anyway.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::{
    POINT_IN_TIME_RECOVERY_STATUS_VALUES, TABLE_STATUS_VALUES, THROUGHPUT_MODE_VALUES,
};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for. Deliberately NOT the emulator's default
/// region, so an ARN built from the request's own SigV4 credential scope is
/// distinguishable from one built from a constant.
pub(crate) const REGION: &str = "us-west-2";

/// How long a test waits before expecting a transient status to have settled: the span
/// the service stores, plus a margin for a loaded test machine.
pub(crate) fn settle_wait() -> Duration {
    keyspaces::TRANSIENT_STATUS_SPAN + Duration::from_millis(250)
}

/// One emulator holding one shared engine, driven over HTTP exactly as the router does.
///
/// Shared with the other suites of this test binary (`tests/all/type_lifecycle.rs`), so
/// there is ONE harness for this service rather than a copy per file.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn start() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's own concern and is exercised
        // elsewhere; these requests carry a well-formed credential scope so they route
        // and carry a region, but they are not really signed.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_keyspaces_keyspacesservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call one operation and return its HTTP status and parsed JSON body.
    pub(crate) async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method(http::Method::POST)
            .uri("/")
            .header("host", format!("cassandra.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("KeyspacesService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/cassandra/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=abc123"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the test request body serializes"),
            ))
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(raw).unwrap_or_else(|error| {
                panic!(
                    "{operation} answered {status} with a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(raw)
                )
            })
        };
        (status, parsed)
    }
}

/// The wire error code of a failure body: the `__type` member, stripped of any namespace
/// so `com.amazonaws.keyspaces#X` and `X` compare equal.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type member>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert a call failed with `expected_status` and `expected_code`, naming the case.
pub(crate) fn assert_failure(
    case: &str,
    got: (u16, Value),
    expected_status: u16,
    expected_code: &str,
) {
    let (status, body) = got;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (expected_status, expected_code),
        "case {case:?}: expected {expected_status} {expected_code}, got {status} {body}"
    );
    assert_ne!(
        status, 501,
        "case {case:?}: answered the scaffold's 501 rather than the modelled error: {body}"
    );
}

/// Assert a call succeeded, naming the case, and hand back its body.
pub(crate) fn assert_ok(case: &str, got: (u16, Value)) -> Value {
    let (status, body) = got;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
    body
}

pub(crate) fn keyspace_arn(name: &str) -> String {
    format!(
        "arn:aws:cassandra:{REGION}:{}:/keyspace/{name}/",
        engine::emulator_account_id()
    )
}

pub(crate) fn table_arn(keyspace: &str, table: &str) -> String {
    format!(
        "arn:aws:cassandra:{REGION}:{}:/keyspace/{keyspace}/table/{table}",
        engine::emulator_account_id()
    )
}

/// The smallest schema the model accepts: one partition key drawn from two columns.
pub(crate) fn minimal_schema(partition_key: &str) -> Value {
    json!({
        "allColumns": [
            {"name": partition_key, "type": "text"},
            {"name": "payload", "type": "int"},
        ],
        "partitionKeys": [{"name": partition_key}],
    })
}

/// RULE 1, for keyspaces: `CreateKeyspace` stores the keyspace and answers with the ARN
/// of what it stored, and a name already taken conflicts.
///
/// The rows span the modelled `keyspaceName` shape (`len 1..48`, pattern
/// `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`): each published bound, and a value one past it.
#[tokio::test]
async fn create_keyspace_mints_an_arn_per_name_and_refuses_a_name_already_taken() {
    let emulator = Emulator::start();
    let longest = "k".repeat(48);
    let legal: [&str; 6] = [
        "a",                  // the minimum modelled length
        "0",                  // the pattern's first character may be a digit
        "demo_ks",            // the ordinary case
        "Mixed_Case_9",       // upper case and digits are in the pattern
        &longest,             // the maximum modelled length
        "ks_with__underscores",
    ];

    for name in legal {
        let created = assert_ok(name, emulator.call("CreateKeyspace", json!({"keyspaceName": name})).await);
        assert_eq!(
            created["resourceArn"], json!(keyspace_arn(name)),
            "case {name:?}: expected the published keyspace ARN, got {created}"
        );

        // The same name a second time is the modelled conflict, not a silent overwrite.
        assert_failure(
            name,
            emulator.call("CreateKeyspace", json!({"keyspaceName": name})).await,
            409,
            "ConflictException",
        );
    }

    // One past each published bound, and outside the pattern: refused by the boundary
    // validator the model drives, with the status and code the model declares.
    let over_long = "k".repeat(49);
    let illegal: [&str; 6] = [
        "",            // under the minimum length
        &over_long,    // one past the maximum length
        "bad name!",   // a space and a `!` are outside the pattern
        "_leading",    // the first character must be alphanumeric
        "dash-name",   // `-` is outside the pattern
        "ks.dot",      // `.` is outside the pattern
    ];
    for name in illegal {
        assert_failure(
            name,
            emulator.call("CreateKeyspace", json!({"keyspaceName": name})).await,
            400,
            "ValidationException",
        );
    }
}

/// RULE 2 and RULE 3: a table reads back exactly as created — the schema verbatim, the
/// modelled defaults for every specification the request omitted — and the status the
/// model documents for a freshly created table settles into `ACTIVE`.
#[tokio::test]
async fn create_table_round_trips_through_get_table_with_the_modelled_defaults() {
    /// One row: a `CreateTable` request body (minus the names) and what `GetTable` must
    /// report for the three specifications that have modelled defaults.
    struct Case {
        name: &'static str,
        request: fn() -> Value,
        capacity: Value,
        encryption: Value,
        point_in_time_recovery: &'static str,
    }

    let cases: Vec<Case> = vec![
        Case {
            // Nothing but the required schema: every specification takes its default.
            name: "defaults_only",
            request: || json!({"schemaDefinition": minimal_schema("pk")}),
            capacity: json!({"throughputMode": "PAY_PER_REQUEST"}),
            encryption: json!({"type": "AWS_OWNED_KMS_KEY"}),
            point_in_time_recovery: "DISABLED",
        },
        Case {
            // Clustering keys, static columns and every optional setting supplied.
            name: "fully_specified",
            request: || {
                json!({
                    "schemaDefinition": {
                        "allColumns": [
                            {"name": "pk", "type": "text"},
                            {"name": "ck", "type": "timestamp"},
                            {"name": "shared", "type": "int"},
                            {"name": "payload", "type": "blob"},
                        ],
                        "partitionKeys": [{"name": "pk"}],
                        "clusteringKeys": [{"name": "ck", "orderBy": "DESC"}],
                        "staticColumns": [{"name": "shared"}],
                    },
                    "comment": {"message": "orders by customer"},
                    "capacitySpecification": {
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": 7,
                        "writeCapacityUnits": 3,
                    },
                    "pointInTimeRecovery": {"status": "ENABLED"},
                    "ttl": {"status": "ENABLED"},
                    "defaultTimeToLive": 86400,
                    "clientSideTimestamps": {"status": "ENABLED"},
                    "tags": [{"key": "team", "value": "orders"}],
                })
            },
            capacity: json!({
                "throughputMode": "PROVISIONED",
                "readCapacityUnits": 7,
                "writeCapacityUnits": 3,
            }),
            encryption: json!({"type": "AWS_OWNED_KMS_KEY"}),
            point_in_time_recovery: "ENABLED",
        },
        Case {
            // A composite partition key, both sort orders, a customer managed key, and
            // `defaultTimeToLive` at the bottom of its modelled `0..630720000` range.
            name: "composite_key_customer_managed_key",
            request: || {
                json!({
                    "schemaDefinition": {
                        "allColumns": [
                            {"name": "tenant", "type": "uuid"},
                            {"name": "device", "type": "text"},
                            {"name": "reading_at", "type": "timestamp"},
                            {"name": "sequence", "type": "bigint"},
                            {"name": "reading", "type": "decimal"},
                            {"name": "labels", "type": "map<text, text>"},
                        ],
                        "partitionKeys": [{"name": "tenant"}, {"name": "device"}],
                        "clusteringKeys": [
                            {"name": "reading_at", "orderBy": "DESC"},
                            {"name": "sequence", "orderBy": "ASC"},
                        ],
                    },
                    "encryptionSpecification": {
                        "type": "CUSTOMER_MANAGED_KMS_KEY",
                        "kmsKeyIdentifier":
                            "arn:aws:kms:us-west-2:000000000000:key/11111111-2222-3333-4444-555555555555",
                    },
                    "defaultTimeToLive": 0,
                })
            },
            capacity: json!({"throughputMode": "PAY_PER_REQUEST"}),
            encryption: json!({
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyIdentifier":
                    "arn:aws:kms:us-west-2:000000000000:key/11111111-2222-3333-4444-555555555555",
            }),
            point_in_time_recovery: "DISABLED",
        },
    ];

    let emulator = Emulator::start();
    let keyspace = "round_trip_ks";
    assert_ok(
        "create the keyspace",
        emulator.call("CreateKeyspace", json!({"keyspaceName": keyspace})).await,
    );

    let before_create = SystemTime::now();
    for case in &cases {
        let mut request = (case.request)();
        request["keyspaceName"] = json!(keyspace);
        request["tableName"] = json!(case.name);

        let created = assert_ok(case.name, emulator.call("CreateTable", request.clone()).await);
        assert_eq!(
            created["resourceArn"], json!(table_arn(keyspace, case.name)),
            "case {:?}: expected the published table ARN, got {created}",
            case.name
        );

        let got = assert_ok(
            case.name,
            emulator
                .call(
                    "GetTable",
                    json!({"keyspaceName": keyspace, "tableName": case.name}),
                )
                .await,
        );

        // The identity of the resource, and the ARN create and get agree on.
        assert_eq!(
            (&got["keyspaceName"], &got["tableName"], &got["resourceArn"]),
            (&json!(keyspace), &json!(case.name), &created["resourceArn"]),
            "case {:?}: GetTable must report the resource CreateTable made: {got}",
            case.name
        );

        // The schema VERBATIM — the same nesting, order and types that were submitted.
        assert_eq!(
            got["schemaDefinition"], request["schemaDefinition"],
            "case {:?}: the stored schema differs from the submitted one: {got}",
            case.name
        );

        // The specifications: supplied where supplied, modelled default where not.
        assert_eq!(
            got["capacitySpecification"], case.capacity,
            "case {:?}: unexpected capacitySpecification: {got}",
            case.name
        );
        assert_eq!(
            got["encryptionSpecification"], case.encryption,
            "case {:?}: unexpected encryptionSpecification: {got}",
            case.name
        );
        assert_eq!(
            got["pointInTimeRecovery"]["status"], json!(case.point_in_time_recovery),
            "case {:?}: unexpected pointInTimeRecovery: {got}",
            case.name
        );
        // Continuous backups start when they are enabled, so the earliest restorable
        // point exists exactly while point-in-time recovery does.
        assert_eq!(
            got["pointInTimeRecovery"]["earliestRestorableTimestamp"].is_null(),
            case.point_in_time_recovery == "DISABLED",
            "case {:?}: earliestRestorableTimestamp must accompany an ENABLED PITR only: {got}",
            case.name
        );

        // The optional settings are reported when supplied and absent when not.
        for member in ["ttl", "defaultTimeToLive", "comment", "clientSideTimestamps"] {
            let submitted = request.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                got[member], submitted,
                "case {:?}: member {member} must read back as submitted: {got}",
                case.name
            );
        }

        // The status is a value the model declares, and it is the transient one the
        // documentation names for a table whose creation has just been accepted.
        let status = got["status"].as_str().unwrap_or_default().to_string();
        assert!(
            TABLE_STATUS_VALUES.contains(&status.as_str()),
            "case {:?}: status {status:?} is not a modelled TableStatus value",
            case.name
        );
        assert_eq!(
            status, keyspaces::TABLE_STATUS_CREATING,
            "case {:?}: a table whose creation was just accepted is CREATING: {got}",
            case.name
        );

        // `creationTimestamp` is when CreateTable ran, as epoch seconds on this protocol.
        let created_at = got["creationTimestamp"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {:?}: creationTimestamp must be a timestamp: {got}",
                case.name
            )
        });
        let floor = before_create
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_secs_f64();
        let ceiling = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_secs_f64();
        assert!(
            created_at >= floor - 1.0 && created_at <= ceiling + 1.0,
            "case {:?}: creationTimestamp {created_at} is outside the window \
             [{floor}, {ceiling}] in which CreateTable ran",
            case.name
        );
    }

    // RULE 3: the transition the clock owed has become due, so every table is now ACTIVE
    // — and the schema it reads back is still the one that was submitted.
    tokio::time::sleep(settle_wait()).await;
    for case in &cases {
        let got = assert_ok(
            case.name,
            emulator
                .call(
                    "GetTable",
                    json!({"keyspaceName": keyspace, "tableName": case.name}),
                )
                .await,
        );
        assert_eq!(
            got["status"], json!(keyspaces::TABLE_STATUS_ACTIVE),
            "case {:?}: the creation span has elapsed, so the table is ACTIVE: {got}",
            case.name
        );
        let mut request = (case.request)();
        request["keyspaceName"] = json!(keyspace);
        assert_eq!(
            got["schemaDefinition"], request["schemaDefinition"],
            "case {:?}: settling a status must not disturb the stored schema: {got}",
            case.name
        );
    }
}

/// An unknown keyspace or table is the modelled `ResourceNotFoundException`, and the
/// exception names the ARN of the resource that was looked for.
#[tokio::test]
async fn reading_an_absent_resource_is_a_modelled_not_found() {
    let emulator = Emulator::start();
    assert_ok(
        "create the keyspace",
        emulator.call("CreateKeyspace", json!({"keyspaceName": "present_ks"})).await,
    );
    assert_ok(
        "create the table",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": "present_ks",
                    "tableName": "present_tbl",
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );

    let cases: [(&str, &str, Value, String); 4] = [
        (
            "unknown keyspace, GetTable",
            "GetTable",
            json!({"keyspaceName": "no_such_ks", "tableName": "present_tbl"}),
            keyspace_arn("no_such_ks"),
        ),
        (
            "unknown table in a known keyspace, GetTable",
            "GetTable",
            json!({"keyspaceName": "present_ks", "tableName": "no_such_tbl"}),
            table_arn("present_ks", "no_such_tbl"),
        ),
        (
            "unknown keyspace, ListTables",
            "ListTables",
            json!({"keyspaceName": "no_such_ks"}),
            keyspace_arn("no_such_ks"),
        ),
        (
            "unknown keyspace, DeleteKeyspace",
            "DeleteKeyspace",
            json!({"keyspaceName": "no_such_ks"}),
            keyspace_arn("no_such_ks"),
        ),
    ];

    for (case, operation, request, expected_arn) in cases {
        let (status, body) = emulator.call(operation, request).await;
        assert_failure(case, (status, body.clone()), 404, "ResourceNotFoundException");
        assert_eq!(
            body["resourceArn"], json!(expected_arn),
            "case {case:?}: the exception must name the resource that was looked for: {body}"
        );
    }
}

/// RULE 4: `ListTables` returns a keyspace's tables in one total order, and a page's
/// `nextToken` resumes exactly after the last item of that page — for every page size,
/// including ones that divide the collection evenly and ones that do not.
#[tokio::test]
async fn list_tables_pages_a_keyspace_in_one_total_order_for_any_page_size() {
    let emulator = Emulator::start();
    let keyspace = "paging_ks";
    assert_ok(
        "create the keyspace",
        emulator.call("CreateKeyspace", json!({"keyspaceName": keyspace})).await,
    );

    // An empty keyspace is an empty page, not a missing member and not a token.
    let empty = assert_ok(
        "empty keyspace",
        emulator.call("ListTables", json!({"keyspaceName": keyspace})).await,
    );
    assert_eq!(
        empty["tables"], json!([]),
        "an empty keyspace lists no tables: {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "an exhausted listing carries no nextToken: {empty}"
    );

    // Created in an order that is NOT the sorted order, so a listing that merely replays
    // insertion order fails.
    let created_in_order = ["t_delta", "t_alpha", "t_echo", "t_bravo", "t_charlie"];
    for table in created_in_order {
        assert_ok(
            table,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "schemaDefinition": minimal_schema("pk"),
                    }),
                )
                .await,
        );
    }
    let mut expected: Vec<String> = created_in_order.iter().map(|t| t.to_string()).collect();
    expected.sort();

    // Page sizes that divide the collection, that do not, and the modelled extremes.
    for page_size in [1_i64, 2, 3, 4, 5, 6, 1000] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({"keyspaceName": keyspace, "maxResults": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = assert_ok(&case, emulator.call("ListTables", request).await);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );

            let items = page["tables"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() as i64 <= page_size,
                "case {case:?}: a page of {} items exceeds maxResults: {page}",
                items.len()
            );
            for item in &items {
                assert_eq!(
                    item["keyspaceName"], json!(keyspace),
                    "case {case:?}: a summary must name its own keyspace: {item}"
                );
                let name = item["tableName"].as_str().unwrap_or_default().to_string();
                assert_eq!(
                    item["resourceArn"], json!(table_arn(keyspace, &name)),
                    "case {case:?}: a summary must carry the table's ARN: {item}"
                );
                seen.push(name);
            }

            match page["nextToken"].as_str() {
                Some(next) => {
                    assert!(
                        !items.is_empty(),
                        "case {case:?}: a page that resumes must have returned items: {page}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: paging must yield every table exactly once, in one total order"
        );
    }

    // A token this service did not issue for this keyspace resumes nothing, and is
    // refused rather than quietly treated as the first page.
    let other = "other_paging_ks";
    assert_ok(
        "create the second keyspace",
        emulator.call("CreateKeyspace", json!({"keyspaceName": other})).await,
    );
    let first = assert_ok(
        "first page of the second keyspace",
        emulator
            .call("ListTables", json!({"keyspaceName": keyspace, "maxResults": 1}))
            .await,
    );
    let foreign_token = first["nextToken"]
        .as_str()
        .expect("a five-table keyspace paged one at a time has a next page")
        .to_string();
    for (case, token) in [
        ("not a token at all", "this-is-not-a-token".to_string()),
        ("a token issued for another keyspace", foreign_token),
    ] {
        assert_failure(
            case,
            emulator
                .call("ListTables", json!({"keyspaceName": other, "nextToken": token}))
                .await,
            400,
            "ValidationException",
        );
    }

    // `maxResults` is modelled `1..1000`: one past either published bound is refused.
    for (case, max_results) in [("below the range", 0), ("above the range", 1001)] {
        assert_failure(
            case,
            emulator
                .call(
                    "ListTables",
                    json!({"keyspaceName": keyspace, "maxResults": max_results}),
                )
                .await,
            400,
            "ValidationException",
        );
    }
}

/// RULE 3 for deletion, and RULE 1's other half: a delete is observable by the next call,
/// the clock completes it, and a keyspace takes its tables with it.
#[tokio::test]
async fn deleting_a_table_and_its_keyspace_is_observable_by_the_next_call() {
    let emulator = Emulator::start();
    let keyspace = "delete_ks";
    assert_ok(
        "create the keyspace",
        emulator.call("CreateKeyspace", json!({"keyspaceName": keyspace})).await,
    );
    for table in ["doomed_tbl", "surviving_tbl"] {
        assert_ok(
            table,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": table,
                        "schemaDefinition": minimal_schema("pk"),
                    }),
                )
                .await,
        );
    }

    // "If a table is either in the CREATING or UPDATING states, then Amazon Keyspaces
    // returns a ResourceInUseException" — a shape this model does not declare, so the
    // claim lands on the ConflictException it does declare.
    assert_failure(
        "delete a table that is still CREATING",
        emulator
            .call(
                "DeleteTable",
                json!({"keyspaceName": keyspace, "tableName": "doomed_tbl"}),
            )
            .await,
        409,
        "ConflictException",
    );
    let refused = assert_ok(
        "the refused delete left the table alone",
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": "doomed_tbl"}),
            )
            .await,
    );
    assert_eq!(
        refused["status"], json!(keyspaces::TABLE_STATUS_CREATING),
        "a refused delete must not move the table's status: {refused}"
    );

    tokio::time::sleep(settle_wait()).await;

    // An ACTIVE table can be deleted, and "the specified table is in the DELETING state
    // until Amazon Keyspaces completes the deletion".
    assert_ok(
        "delete the ACTIVE table",
        emulator
            .call(
                "DeleteTable",
                json!({"keyspaceName": keyspace, "tableName": "doomed_tbl"}),
            )
            .await,
    );
    let deleting = assert_ok(
        "the table is deleting",
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": "doomed_tbl"}),
            )
            .await,
    );
    assert_eq!(
        deleting["status"], json!(keyspaces::TABLE_STATUS_DELETING),
        "a table whose deletion was accepted is DELETING: {deleting}"
    );

    // "If the table is already in the DELETING state, no error is returned."
    assert_ok(
        "repeat the delete while DELETING",
        emulator
            .call(
                "DeleteTable",
                json!({"keyspaceName": keyspace, "tableName": "doomed_tbl"}),
            )
            .await,
    );

    tokio::time::sleep(settle_wait()).await;

    // The deletion the clock owed has completed: the table is gone from every reader.
    assert_failure(
        "GetTable after the deletion completed",
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": "doomed_tbl"}),
            )
            .await,
        404,
        "ResourceNotFoundException",
    );
    let listed = assert_ok(
        "ListTables after the deletion completed",
        emulator.call("ListTables", json!({"keyspaceName": keyspace})).await,
    );
    assert_eq!(
        listed["tables"], json!([{
            "keyspaceName": keyspace,
            "tableName": "surviving_tbl",
            "resourceArn": table_arn(keyspace, "surviving_tbl"),
        }]),
        "only the table that was not deleted is listed: {listed}"
    );

    // Deleting a table that was never there, and one whose keyspace is unknown.
    assert_failure(
        "delete an absent table",
        emulator
            .call(
                "DeleteTable",
                json!({"keyspaceName": keyspace, "tableName": "never_existed"}),
            )
            .await,
        404,
        "ResourceNotFoundException",
    );

    // "The DeleteKeyspace operation deletes a keyspace and all of its tables."
    assert_ok(
        "delete the keyspace",
        emulator.call("DeleteKeyspace", json!({"keyspaceName": keyspace})).await,
    );
    for (case, operation, request) in [
        (
            "GetTable after the keyspace was deleted",
            "GetTable",
            json!({"keyspaceName": keyspace, "tableName": "surviving_tbl"}),
        ),
        (
            "ListTables after the keyspace was deleted",
            "ListTables",
            json!({"keyspaceName": keyspace}),
        ),
        (
            "DeleteKeyspace repeated",
            "DeleteKeyspace",
            json!({"keyspaceName": keyspace}),
        ),
    ] {
        assert_failure(
            case,
            emulator.call(operation, request).await,
            404,
            "ResourceNotFoundException",
        );
    }

    // And the name is free again: a keyspace deleted is a keyspace creatable.
    let recreated = assert_ok(
        "recreate the deleted keyspace",
        emulator.call("CreateKeyspace", json!({"keyspaceName": keyspace})).await,
    );
    assert_eq!(
        recreated["resourceArn"], json!(keyspace_arn(keyspace)),
        "recreating a deleted keyspace mints its ARN again: {recreated}"
    );
    let empty_again = assert_ok(
        "the recreated keyspace holds no tables",
        emulator.call("ListTables", json!({"keyspaceName": keyspace})).await,
    );
    assert_eq!(
        empty_again["tables"], json!([]),
        "the tables of a deleted keyspace are gone with it: {empty_again}"
    );
}

/// The store is bounded: past the emulator's own cap a create answers the quota error the
/// model declares, rather than accepting another process-lifetime entry.
#[tokio::test]
async fn creating_past_the_store_cap_answers_the_modelled_quota_error() {
    let emulator = Emulator::start();

    // Tables: fill one keyspace to its cap, then one more.
    let keyspace = "capped_ks";
    assert_ok(
        "create the keyspace",
        emulator.call("CreateKeyspace", json!({"keyspaceName": keyspace})).await,
    );
    for index in 0..keyspaces::MAX_TABLES_PER_KEYSPACE {
        assert_ok(
            "fill the keyspace to its table cap",
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": keyspace,
                        "tableName": format!("t{index}"),
                        "schemaDefinition": minimal_schema("pk"),
                    }),
                )
                .await,
        );
    }
    assert_failure(
        "one table past the cap",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "one_too_many",
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
        402,
        "ServiceQuotaExceededException",
    );

    // Keyspaces: one keyspace already exists, so fill the rest and then ask for one more.
    for index in 1..keyspaces::MAX_KEYSPACES {
        assert_ok(
            "fill the account to its keyspace cap",
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": format!("ks{index}")}))
                .await,
        );
    }
    assert_failure(
        "one keyspace past the cap",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": "one_too_many_ks"}))
            .await,
        402,
        "ServiceQuotaExceededException",
    );

    // A full store does not change what a name already taken answers: no entry would be
    // added, so the conflict is still the conflict.
    assert_failure(
        "a name already taken, with the store full",
        emulator.call("CreateKeyspace", json!({"keyspaceName": keyspace})).await,
        409,
        "ConflictException",
    );
    assert_failure(
        "a table name already taken, with the keyspace full",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "t0",
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
        409,
        "ConflictException",
    );

    // A refusal at the cap is not a refusal of everything: what is already stored still
    // reads back, and the refused names were not stored.
    let listed = assert_ok(
        "the capped keyspace still lists its tables",
        emulator
            .call("ListTables", json!({"keyspaceName": keyspace, "maxResults": 1000}))
            .await,
    );
    assert_eq!(
        listed["tables"].as_array().map(Vec::len),
        Some(keyspaces::MAX_TABLES_PER_KEYSPACE),
        "the tables stored before the cap must all still be there: {}",
        listed["nextToken"]
    );
    assert_failure(
        "the table refused at the cap was not stored",
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": "one_too_many"}),
            )
            .await,
        404,
        "ResourceNotFoundException",
    );
    assert_failure(
        "the keyspace refused at the cap was not stored",
        emulator
            .call("ListTables", json!({"keyspaceName": "one_too_many_ks"}))
            .await,
        404,
        "ResourceNotFoundException",
    );
}

/// RULE 5: each rule the model cannot state is refused with the modelled error, and the
/// request that satisfies it is accepted — so the refusal is the rule, not a blanket no.
#[tokio::test]
async fn requests_that_break_an_unmodelled_rule_are_refused_and_the_rest_accepted() {
    let emulator = Emulator::start();
    let keyspace = "rules_ks";
    assert_ok(
        "create the keyspace",
        emulator.call("CreateKeyspace", json!({"keyspaceName": keyspace})).await,
    );

    // Each row: the case, the CreateTable body (minus names), and the expected answer.
    let table_rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            // "PROVISIONED - Provisioned capacity mode requires readCapacityUnits and
            // writeCapacityUnits as input."
            "provisioned_without_units",
            json!({
                "schemaDefinition": minimal_schema("pk"),
                "capacitySpecification": {"throughputMode": "PROVISIONED"},
            }),
            400,
            "ValidationException",
        ),
        (
            "provisioned_with_only_read_units",
            json!({
                "schemaDefinition": minimal_schema("pk"),
                "capacitySpecification": {
                    "throughputMode": "PROVISIONED",
                    "readCapacityUnits": 4,
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "provisioned_with_both_units",
            json!({
                "schemaDefinition": minimal_schema("pk"),
                "capacitySpecification": {
                    "throughputMode": "PROVISIONED",
                    "readCapacityUnits": 4,
                    "writeCapacityUnits": 4,
                },
            }),
            200,
            "",
        ),
        (
            // "CUSTOMER_MANAGED_KMS_KEY ... requires the kms_key_identifier of the KMS
            // key in Amazon Resource Name (ARN) format as input."
            "customer_managed_key_without_identifier",
            json!({
                "schemaDefinition": minimal_schema("pk"),
                "encryptionSpecification": {"type": "CUSTOMER_MANAGED_KMS_KEY"},
            }),
            400,
            "ValidationException",
        ),
        (
            "customer_managed_key_with_identifier",
            json!({
                "schemaDefinition": minimal_schema("pk"),
                "encryptionSpecification": {
                    "type": "CUSTOMER_MANAGED_KMS_KEY",
                    "kmsKeyIdentifier": "arn:aws:kms:us-west-2:000000000000:key/abc",
                },
            }),
            200,
            "",
        ),
        (
            // A key must be one of the table's own columns.
            "partition_key_is_not_a_declared_column",
            json!({
                "schemaDefinition": {
                    "allColumns": [{"name": "pk", "type": "text"}],
                    "partitionKeys": [{"name": "absent"}],
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "clustering_key_is_not_a_declared_column",
            json!({
                "schemaDefinition": {
                    "allColumns": [{"name": "pk", "type": "text"}],
                    "partitionKeys": [{"name": "pk"}],
                    "clusteringKeys": [{"name": "absent", "orderBy": "ASC"}],
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "static_column_is_not_a_declared_column",
            json!({
                "schemaDefinition": {
                    "allColumns": [{"name": "pk", "type": "text"}],
                    "partitionKeys": [{"name": "pk"}],
                    "staticColumns": [{"name": "absent"}],
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "all_columns_declares_a_name_twice",
            json!({
                "schemaDefinition": {
                    "allColumns": [
                        {"name": "pk", "type": "text"},
                        {"name": "pk", "type": "int"},
                    ],
                    "partitionKeys": [{"name": "pk"}],
                },
            }),
            400,
            "ValidationException",
        ),
        (
            // A column holds one role: naming it twice in one list is not a schema.
            "partition_key_named_twice",
            json!({
                "schemaDefinition": {
                    "allColumns": [
                        {"name": "pk", "type": "text"},
                        {"name": "other", "type": "int"},
                    ],
                    "partitionKeys": [{"name": "pk"}, {"name": "pk"}],
                },
            }),
            400,
            "ValidationException",
        ),
        (
            // ... and neither is a column that is both a key and a clustering key.
            "column_is_both_partition_and_clustering_key",
            json!({
                "schemaDefinition": {
                    "allColumns": [
                        {"name": "pk", "type": "text"},
                        {"name": "other", "type": "int"},
                    ],
                    "partitionKeys": [{"name": "pk"}],
                    "clusteringKeys": [{"name": "pk", "orderBy": "ASC"}],
                },
            }),
            400,
            "ValidationException",
        ),
        (
            // ... nor one that is both a clustering key and static.
            "column_is_both_clustering_key_and_static",
            json!({
                "schemaDefinition": {
                    "allColumns": [
                        {"name": "pk", "type": "text"},
                        {"name": "ck", "type": "int"},
                    ],
                    "partitionKeys": [{"name": "pk"}],
                    "clusteringKeys": [{"name": "ck", "orderBy": "DESC"}],
                    "staticColumns": [{"name": "ck"}],
                },
            }),
            400,
            "ValidationException",
        ),
        (
            // The capacity units are input for PROVISIONED and for nothing else.
            "pay_per_request_with_capacity_units",
            json!({
                "schemaDefinition": minimal_schema("pk"),
                "capacitySpecification": {
                    "throughputMode": "PAY_PER_REQUEST",
                    "readCapacityUnits": 5,
                    "writeCapacityUnits": 5,
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "pay_per_request_with_only_write_units",
            json!({
                "schemaDefinition": minimal_schema("pk"),
                "capacitySpecification": {
                    "throughputMode": "PAY_PER_REQUEST",
                    "writeCapacityUnits": 5,
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "pay_per_request_without_units",
            json!({
                "schemaDefinition": minimal_schema("pk"),
                "capacitySpecification": {"throughputMode": "PAY_PER_REQUEST"},
            }),
            200,
            "",
        ),
        (
            // Modelled: `partitionKeys` is required with `length min 1`.
            "no_partition_keys",
            json!({
                "schemaDefinition": {
                    "allColumns": [{"name": "pk", "type": "text"}],
                    "partitionKeys": [],
                },
            }),
            400,
            "ValidationException",
        ),
        (
            // Modelled: `allColumns` is required with `length min 1`.
            "no_columns",
            json!({
                "schemaDefinition": {"allColumns": [], "partitionKeys": [{"name": "pk"}]},
            }),
            400,
            "ValidationException",
        ),
        (
            // Modelled: `throughputMode` accepts only THROUGHPUT_MODE_VALUES.
            "unknown_throughput_mode",
            json!({
                "schemaDefinition": minimal_schema("pk"),
                "capacitySpecification": {"throughputMode": "ON_DEMAND"},
            }),
            400,
            "ValidationException",
        ),
        (
            // Modelled: `defaultTimeToLive` has range `0..630720000`.
            "default_time_to_live_above_the_range",
            json!({
                "schemaDefinition": minimal_schema("pk"),
                "defaultTimeToLive": 630720001_i64,
            }),
            400,
            "ValidationException",
        ),
        (
            "default_time_to_live_at_the_top_of_the_range",
            json!({
                "schemaDefinition": minimal_schema("pk"),
                "defaultTimeToLive": 630720000_i64,
                "ttl": {"status": "ENABLED"},
            }),
            200,
            "",
        ),
    ];

    for (case, mut request, expected_status, expected_code) in table_rows {
        request["keyspaceName"] = json!(keyspace);
        request["tableName"] = json!(case);
        let answer = emulator.call("CreateTable", request).await;
        if expected_status == 200 {
            assert_ok(case, answer);
            // An accepted create is a stored table: the reader proves it.
            let got = assert_ok(
                case,
                emulator
                    .call("GetTable", json!({"keyspaceName": keyspace, "tableName": case}))
                    .await,
            );
            assert_eq!(
                got["resourceArn"], json!(table_arn(keyspace, case)),
                "case {case:?}: an accepted create must be readable back: {got}"
            );
            let mode = got["capacitySpecification"]["throughputMode"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                THROUGHPUT_MODE_VALUES.contains(&mode.as_str()),
                "case {case:?}: throughputMode {mode:?} is not a modelled value"
            );
            let pitr = got["pointInTimeRecovery"]["status"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                POINT_IN_TIME_RECOVERY_STATUS_VALUES.contains(&pitr.as_str()),
                "case {case:?}: pointInTimeRecovery status {pitr:?} is not a modelled value"
            );
        } else {
            assert_failure(case, answer, expected_status, expected_code);
            // A refused create stored nothing.
            assert_failure(
                case,
                emulator
                    .call("GetTable", json!({"keyspaceName": keyspace, "tableName": case}))
                    .await,
                404,
                "ResourceNotFoundException",
            );
        }
    }

    // A table in a keyspace that does not exist is not found, not created implicitly.
    assert_failure(
        "create a table in an unknown keyspace",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": "no_such_ks",
                    "tableName": "orphan",
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
        404,
        "ResourceNotFoundException",
    );

    // The members this slice does not serve are refused honestly, with the 501 that says
    // the feature is not written yet — never accepted and silently dropped.
    let unserved: [(&str, Value); 4] = [
        (
            "autoScalingSpecification",
            json!({"writeCapacityAutoScaling": {"autoScalingDisabled": true}}),
        ),
        (
            "replicaSpecifications",
            json!([{"region": "eu-west-1", "readCapacityUnits": 5}]),
        ),
        ("cdcSpecification", json!({"status": "ENABLED"})),
        (
            "warmThroughputSpecification",
            json!({"readUnitsPerSecond": 12000, "writeUnitsPerSecond": 4000}),
        ),
    ];
    for (member, value) in unserved {
        let mut request = json!({
            "keyspaceName": keyspace,
            "tableName": "unserved_member_tbl",
            "schemaDefinition": minimal_schema("pk"),
        });
        request[member] = value;
        let (status, body) = emulator.call("CreateTable", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (501, "NotImplemented"),
            "case {member:?}: an unserved member must be refused as not implemented, \
             got {status} {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or_default().contains(member),
            "case {member:?}: the refusal must name the member the caller has to drop: {body}"
        );
        // And nothing was stored for it.
        assert_failure(
            member,
            emulator
                .call(
                    "GetTable",
                    json!({"keyspaceName": keyspace, "tableName": "unserved_member_tbl"}),
                )
                .await,
            404,
            "ResourceNotFoundException",
        );
    }

    // The replication rules of CreateKeyspace: "if the replicationStrategy is
    // MULTI_REGION, the regionList requires the current Region and at least one
    // additional Amazon Web Services Region".
    let keyspace_rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "multi_region_without_region_list",
            json!({"replicationSpecification": {"replicationStrategy": "MULTI_REGION"}}),
            400,
            "ValidationException",
        ),
        (
            "multi_region_without_the_current_region",
            json!({"replicationSpecification": {
                "replicationStrategy": "MULTI_REGION",
                "regionList": ["eu-west-1", "ap-south-1"],
            }}),
            400,
            "ValidationException",
        ),
        (
            // Modelled: `regionList` has `length min 2`.
            "multi_region_with_only_the_current_region",
            json!({"replicationSpecification": {
                "replicationStrategy": "MULTI_REGION",
                "regionList": [REGION],
            }}),
            400,
            "ValidationException",
        ),
        (
            "multi_region_with_the_current_region_and_another",
            json!({"replicationSpecification": {
                "replicationStrategy": "MULTI_REGION",
                "regionList": [REGION, "eu-west-1"],
            }}),
            200,
            "",
        ),
        (
            "single_region",
            json!({"replicationSpecification": {"replicationStrategy": "SINGLE_REGION"}}),
            200,
            "",
        ),
        (
            // Modelled: `replicationStrategy` accepts only SINGLE_REGION / MULTI_REGION.
            "unknown_replication_strategy",
            json!({"replicationSpecification": {"replicationStrategy": "GLOBAL"}}),
            400,
            "ValidationException",
        ),
    ];

    for (case, mut request, expected_status, expected_code) in keyspace_rows {
        request["keyspaceName"] = json!(case);
        let answer = emulator.call("CreateKeyspace", request).await;
        if expected_status == 200 {
            let created = assert_ok(case, answer);
            assert_eq!(
                created["resourceArn"], json!(keyspace_arn(case)),
                "case {case:?}: an accepted keyspace carries its ARN"
            );
            // Stored, so a table can be created in it.
            assert_ok(
                case,
                emulator
                    .call(
                        "CreateTable",
                        json!({
                            "keyspaceName": case,
                            "tableName": "probe_tbl",
                            "schemaDefinition": minimal_schema("pk"),
                        }),
                    )
                    .await,
            );
        } else {
            assert_failure(case, answer, expected_status, expected_code);
            // A refused create stored nothing.
            assert_failure(
                case,
                emulator.call("ListTables", json!({"keyspaceName": case})).await,
                404,
                "ResourceNotFoundException",
            );
        }
    }
}
