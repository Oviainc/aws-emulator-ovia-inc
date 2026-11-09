// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK a RUNNING handler reaches the RDS Data API through, end to end over the
//! wire: Lambda and the Data API on ONE engine, a managed-runtime function that ships no
//! dependency of its own, and the rows its statements wrote read back where a client reads
//! them.
//!
//! The rule pinned here: the AWS SDK a managed runtime supplies can build a client for any
//! AWS service the handler names, so a token naming a service THIS emulator serves is
//! signed, routed to that service's own endpoint and answered by that service's own API —
//! a success carrying the operation's own output members, or that service's own modelled
//! refusal raised as the error class its shape names. A service the runtime's SDK has no
//! registry entry for is instead refused before anything is signed, naming the catalogue
//! the runtime holds, which is not an answer any service gave; every row below asserts that
//! it did not happen.
//!
//! The rows differ along what the rule generalises over: WHICH operation of the service is
//! called (a DDL statement, a DML one with bound parameters, a batch of parameter sets, a
//! read with result metadata, and the three calls a transaction is made of) and whether the
//! call succeeds or is refused — with three different modelled refusals, each a different
//! leg of the Data API's own chain. What the statements wrote is then asserted on the DATA
//! API side too, through that service's own endpoint, so a call that answered without
//! writing anything fails here. Which SPELLINGS of a service's token reach it is the other
//! half of the same rule, pinned over its own table beside the seam
//! (`state::execution::aws_api`'s own suite), because a token reaches that seam before any
//! runtime is involved.
//!
//! Every row runs in ONE invocation, which is also what a function that talks to a database
//! does: the handler is given the calls to make and reports what each answered. One run, and
//! a `nodejs*` one, is deliberate: this suite shares a process with every other invocation
//! suite in this crate and a run is bounded by this process's own resident ceiling, so the
//! cheapest engine that exercises the seam is the one to drive it with. The same seam serves
//! every runtime's bundled SDK — the catalogue of reachable services, the signature and the
//! destination are the HOST's ([`state::execution::aws_api`]), not the interpreter's.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the function's own
/// invocation — and therefore the client its handler builds — runs in.
const REGION: &str = "us-east-1";

/// The account segment every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `Runtime` the function carries: a managed Node release that bundles the SDK.
const RUNTIME: &str = "nodejs22.x";

/// The table every statement below runs against.
const TABLE: &str = "pets";

/// What a row that runs inside a transaction names that transaction by: the id is the
/// service's to mint, so the handler fills this in from the answer that opened it.
const TRANSACTION_PLACEHOLDER: &str = "${transactionId}";

/// How long past a due instant this suite waits before reading. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(200);

/// The DB cluster every Data API call addresses — an Amazon RDS cluster ARN, which is what
/// the Data API's own `resourceArn` names.
fn cluster_arn() -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:data-api-demo")
}

/// The secret every Data API call authenticates with, as its `secretArn` names one.
fn secret_arn() -> String {
    format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:data-api-demo")
}

/// Lambda and the RDS Data API over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them: the handler's SDK call is dispatched back into this same router, so the rows it
/// writes are the rows this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_rdsdata_rdsdataservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes
/// the request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Data API call, as an SDK client sends one (`restJson1`, one URI per operation), that
/// must answer 200.
async fn data_api(router: &MultiServiceRouter, case: &str, path: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(path)
        .header("host", format!("rds-data.{REGION}.amazonaws.com"))
        .header("authorization", authorization("rds-data"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the Data API at {path}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status, headers and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// A .zip deployment package holding `source` as `index.mjs` — one ES module, and nothing
/// else: no `node_modules`, so every `@aws-sdk/*` specifier in it is the runtime's to
/// resolve.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("index.mjs", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create the function whose handler makes the calls, and wait out the provisioning
/// window.
async fn create_function(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": "index.handler",
            "Timeout": 30,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(
                    package(HANDLER_SOURCE),
                ),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` the function synchronously with `event`, answering the payload the run
/// produced and the `FunctionError` signal it carried, if any.
async fn invoke(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    event: &Value,
) -> (Option<String>, Value) {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (signal, payload)
}

/// The handler the run executes: an ES module that imports the bundled client, the command
/// class of each operation it calls and the error class of two of the service's own error
/// shapes BY NAME — which is what a v3 consumer writes, and which only resolves because the
/// runtime holds this service — then makes the calls the event names and reports what each
/// one answered.
///
/// A call the runtime makes no client for is refused before anything is signed, and the
/// refusal names the catalogue the runtime holds rather than anything a service said; every
/// row asserts that its answer is not that one.
const HANDLER_SOURCE: &str = r#"
import {
    RDSDataClient,
    ExecuteStatementCommand,
    BatchExecuteStatementCommand,
    BeginTransactionCommand,
    CommitTransactionCommand,
    TransactionNotFoundException,
    DatabaseErrorException,
    RDSDataServiceException,
} from "@aws-sdk/client-rds-data";

const COMMANDS = {
    ExecuteStatement: ExecuteStatementCommand,
    BatchExecuteStatement: BatchExecuteStatementCommand,
    BeginTransaction: BeginTransactionCommand,
    CommitTransaction: CommitTransactionCommand,
};

const PLACEHOLDER = "${transactionId}";

export const handler = async (event) => {
    const client = new RDSDataClient({ region: event.region });
    const results = [];
    let transaction = "";
    for (const call of event.calls) {
        const Command = COMMANDS[call.operation];
        if (Command === undefined) {
            results.push({ ok: false, unbound: call.operation });
            continue;
        }
        const input = {};
        for (const name of Object.keys(call.input)) {
            const value = call.input[name];
            input[name] = value === PLACEHOLDER ? transaction : value;
        }
        try {
            const answered = await client.send(new Command(input));
            const members = {};
            for (const name of Object.keys(answered)) {
                if (name !== "$metadata") {
                    members[name] = answered[name];
                }
            }
            if (typeof members.transactionId === "string") {
                transaction = members.transactionId;
            }
            results.push({
                ok: true,
                httpStatusCode: answered.$metadata.httpStatusCode,
                answered: members,
            });
        } catch (error) {
            results.push({
                ok: false,
                name: error.name,
                ctor: error.constructor.name,
                message: error.message,
                httpStatusCode: error.$metadata === undefined
                    ? null
                    : error.$metadata.httpStatusCode,
                serviceException: error instanceof RDSDataServiceException,
                transactionNotFound: error instanceof TransactionNotFoundException,
                databaseError: error instanceof DatabaseErrorException,
            });
        }
    }
    return { client: RDSDataClient.name, results };
};
"#;

/// The class the SDK publishes this service's client under, which the handler reports back:
/// `@aws-sdk/client-rds-data` exports `RDSDataClient`.
const CLIENT_CLASS: &str = "RDSDataClient";

/// What one row asks the handler to do, and what must come of it.
struct Row {
    case: &'static str,
    /// The operation the row calls, which is the command class's own name without its
    /// suffix.
    operation: &'static str,
    /// What the row's call must answer.
    expected: Expected,
}

/// What a row's call must answer.
enum Expected {
    /// A success whose answer carries these output members.
    Members(&'static [&'static str]),
    /// A refusal naming this modelled error shape, with the status that class carries.
    Refused { name: &'static str, status: u64 },
}

/// The rows, in the order they run: the writes come first, so the read and the transaction
/// that follow them report what they left behind.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a DDL statement creates the table the rows run against",
            operation: "ExecuteStatement",
            expected: Expected::Members(&[]),
        },
        Row {
            case: "a DML statement with bound parameters inserts one row",
            operation: "ExecuteStatement",
            // `ExecuteStatementResponse`: a statement with no result set reports the rows
            // it changed and the fields the engine generated for them.
            expected: Expected::Members(&["numberOfRecordsUpdated", "generatedFields"]),
        },
        Row {
            case: "a batch inserts one row per parameter set",
            operation: "BatchExecuteStatement",
            expected: Expected::Members(&["updateResults"]),
        },
        Row {
            case: "a read with result metadata returns the rows and their columns",
            operation: "ExecuteStatement",
            expected: Expected::Members(&["records", "columnMetadata"]),
        },
        Row {
            case: "a transaction is opened",
            operation: "BeginTransaction",
            expected: Expected::Members(&["transactionId"]),
        },
        Row {
            case: "a statement inside that transaction inserts one more row",
            operation: "ExecuteStatement",
            expected: Expected::Members(&["numberOfRecordsUpdated", "generatedFields"]),
        },
        Row {
            case: "the transaction commits",
            operation: "CommitTransaction",
            expected: Expected::Members(&["transactionStatus"]),
        },
        Row {
            case: "a refused call: the committed transaction's id names no transaction",
            operation: "ExecuteStatement",
            expected: Expected::Refused {
                name: "TransactionNotFoundException",
                status: 404,
            },
        },
        Row {
            case: "a refused call: a statement the database itself refuses",
            operation: "ExecuteStatement",
            expected: Expected::Refused {
                name: "DatabaseErrorException",
                status: 400,
            },
        },
        Row {
            case: "a refused call: a resourceArn that names no DB cluster",
            operation: "ExecuteStatement",
            expected: Expected::Refused {
                name: "ValidationException",
                status: 400,
            },
        },
    ]
}

/// The `resourceArn` the row naming no DB cluster carries: of the modelled `Arn` length,
/// and not a cluster ARN, so the Data API's own chain refuses the call before resolving
/// anything.
const NOT_A_CLUSTER_ARN: &str = "not-an-arn-at-all-but-long-enough";

/// The input each row's call carries, built against the cluster the rows run on; a row that
/// runs inside the transaction an earlier row opened names it by the placeholder the handler
/// fills in.
fn input(case: &str) -> Value {
    let mut members = match case {
        "a DDL statement creates the table the rows run against" => json!({
            "sql": format!("CREATE TABLE {TABLE} (id INT, name VARCHAR(32))"),
        }),
        "a DML statement with bound parameters inserts one row" => json!({
            "sql": format!("INSERT INTO {TABLE} (id, name) VALUES (:id, :name)"),
            "parameters": [
                {"name": "id", "value": {"longValue": 1}},
                {"name": "name", "value": {"stringValue": "Ada"}},
            ],
        }),
        "a batch inserts one row per parameter set" => json!({
            "sql": format!("INSERT INTO {TABLE} (id, name) VALUES (:id, :name)"),
            "parameterSets": [
                [
                    {"name": "id", "value": {"longValue": 2}},
                    {"name": "name", "value": {"stringValue": "Grace"}},
                ],
                [
                    {"name": "id", "value": {"longValue": 3}},
                    {"name": "name", "value": {"stringValue": "Alan"}},
                ],
            ],
        }),
        "a read with result metadata returns the rows and their columns" => json!({
            "sql": format!("SELECT id, name FROM {TABLE} ORDER BY id"),
            "includeResultMetadata": true,
        }),
        "a transaction is opened" => json!({}),
        "a statement inside that transaction inserts one more row" => json!({
            "sql": format!("INSERT INTO {TABLE} (id, name) VALUES (:id, :name)"),
            "parameters": [
                {"name": "id", "value": {"longValue": 4}},
                {"name": "name", "value": {"stringValue": "Edsger"}},
            ],
            "transactionId": TRANSACTION_PLACEHOLDER,
        }),
        "the transaction commits" => json!({ "transactionId": TRANSACTION_PLACEHOLDER }),
        "a refused call: the committed transaction's id names no transaction" => json!({
            "sql": format!("SELECT id FROM {TABLE}"),
            "transactionId": TRANSACTION_PLACEHOLDER,
        }),
        "a refused call: a statement the database itself refuses" => json!({
            "sql": "SELECT id FROM a_table_that_was_never_created",
        }),
        _ => json!({ "sql": format!("SELECT id FROM {TABLE}") }),
    };
    let object = members
        .as_object_mut()
        .expect("a call's input is a JSON object");
    let resource = match case {
        "a refused call: a resourceArn that names no DB cluster" => NOT_A_CLUSTER_ARN.to_string(),
        _ => cluster_arn(),
    };
    object.insert("resourceArn".to_string(), json!(resource));
    object.insert("secretArn".to_string(), json!(secret_arn()));
    members
}

/// One SDK call per row, each asserted on what the guest read back — and the writes
/// asserted again on the DATA API side, where a client reads them.
#[tokio::test]
async fn a_handler_calls_the_data_api_through_the_sdk_the_runtime_supplies() {
    let router = router();
    let function = "rds-data-caller";
    create_function(&router, "the function every row runs", function).await;

    let rows = rows();
    let calls: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "operation": row.operation,
                "input": input(row.case),
            })
        })
        .collect();
    let case = "the one invocation every row runs in";
    let (signal, payload) = invoke(
        &router,
        case,
        function,
        &json!({ "region": REGION, "calls": calls }),
    )
    .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion, got the function error \
         {signal:?} with {payload}"
    );
    assert_eq!(
        payload["client"],
        json!(CLIENT_CLASS),
        "case {case:?}: expected the client the service's own package exports, got {payload}"
    );
    let results = payload["results"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected one result per call, got {payload}"));
    assert_eq!(
        results.len(),
        rows.len(),
        "case {case:?}: expected one result per call, got {payload}"
    );

    for (row, result) in rows.iter().zip(&results) {
        let case = row.case;
        assert!(
            result["unbound"].is_null(),
            "case {case:?}: expected the package to export a command for {:?}, got {result}",
            row.operation
        );
        // The defect this suite was written for: a service the runtime's SDK has no entry
        // for never reaches a service at all, so the handler is told what catalogue the
        // runtime holds instead of what the service answered.
        assert_ne!(
            result["name"],
            json!("NotImplemented"),
            "case {case:?}: expected the call to reach the service, got {result}"
        );

        match row.expected {
            Expected::Members(members) => {
                assert_eq!(
                    result["ok"],
                    json!(true),
                    "case {case:?}: expected the call to succeed, got {result}"
                );
                assert_eq!(
                    result["httpStatusCode"],
                    json!(200),
                    "case {case:?}: expected the SDK to report HTTP 200, got {result}"
                );
                for member in members {
                    assert!(
                        !result["answered"][*member].is_null(),
                        "case {case:?}: expected the answer to carry {member:?}, got {result}"
                    );
                }
            }
            Expected::Refused { name, status } => {
                assert_eq!(
                    result["ok"],
                    json!(false),
                    "case {case:?}: expected the call to be refused, got {result}"
                );
                assert_eq!(
                    result["name"],
                    json!(name),
                    "case {case:?}: expected the modelled error shape {name:?}, got {result}"
                );
                assert_eq!(
                    result["httpStatusCode"],
                    json!(status),
                    "case {case:?}: expected the refusal's own status {status}, got {result}"
                );
                assert_eq!(
                    result["serviceException"],
                    json!(true),
                    "case {case:?}: expected the refusal to stand on this service's own base \
                     exception, got {result}"
                );
                let message = result["message"].as_str().unwrap_or_default();
                assert!(
                    !message.is_empty(),
                    "case {case:?}: expected the service's own message, got {result}"
                );
            }
        }

        // What each answer says about the work its statement did, member by member.
        match case {
            "a DML statement with bound parameters inserts one row"
            | "a statement inside that transaction inserts one more row" => assert_eq!(
                result["answered"]["numberOfRecordsUpdated"],
                json!(1),
                "case {case:?}: expected one row changed, got {result}"
            ),
            "a batch inserts one row per parameter set" => {
                let updated = result["answered"]["updateResults"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert_eq!(
                    updated.len(),
                    2,
                    "case {case:?}: expected one update result per parameter set, got {result}"
                );
            }
            "a read with result metadata returns the rows and their columns" => {
                let names: Vec<&str> = result["answered"]["records"]
                    .as_array()
                    .map(|records| {
                        records
                            .iter()
                            .filter_map(|record| record[1]["stringValue"].as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    names,
                    vec!["Ada", "Grace", "Alan"],
                    "case {case:?}: expected the rows the earlier statements wrote, got \
                     {result}"
                );
                let columns: Vec<&str> = result["answered"]["columnMetadata"]
                    .as_array()
                    .map(|columns| {
                        columns
                            .iter()
                            .filter_map(|column| column["name"].as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    columns,
                    vec!["id", "name"],
                    "case {case:?}: expected the columns the statement selected, got {result}"
                );
            }
            "a transaction is opened" => {
                let transaction = result["answered"]["transactionId"]
                    .as_str()
                    .unwrap_or_default();
                assert!(
                    !transaction.is_empty() && transaction.len() <= 192,
                    "case {case:?}: expected the transaction id the service issued, got \
                     {result}"
                );
            }
            // The two rows whose refusal the service MODELS are the error classes this
            // service's own package exports, so the handler caught each as its own class
            // rather than as the base exception.
            "a refused call: the committed transaction's id names no transaction" => assert_eq!(
                result["transactionNotFound"],
                json!(true),
                "case {case:?}: expected the modelled error class of that shape, got {result}"
            ),
            "a refused call: a statement the database itself refuses" => assert_eq!(
                result["databaseError"],
                json!(true),
                "case {case:?}: expected the modelled error class of that shape, got {result}"
            ),
            "a refused call: a resourceArn that names no DB cluster" => {
                let message = result["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(NOT_A_CLUSTER_ARN),
                    "case {case:?}: expected the message to name the value the call carried, \
                     got {message:?}"
                );
            }
            _ => {}
        }
    }

    // What the DATA API holds: the four rows the handler's statements wrote — three outside
    // a transaction and one committed inside it — read back through that service's own
    // endpoint, which is where a client reads them.
    let case = "the rows the handler wrote, read through the Data API itself";
    let read = data_api(
        &router,
        case,
        "/Execute",
        &json!({
            "resourceArn": cluster_arn(),
            "secretArn": secret_arn(),
            "sql": format!("SELECT id, name FROM {TABLE} ORDER BY id"),
        }),
    )
    .await;
    let written: Vec<(i64, &str)> = read["records"]
        .as_array()
        .map(|records| {
            records
                .iter()
                .filter_map(|record| {
                    Some((
                        record[0]["longValue"].as_i64()?,
                        record[1]["stringValue"].as_str()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        written,
        vec![(1, "Ada"), (2, "Grace"), (3, "Alan"), (4, "Edsger")],
        "case {case:?}: expected every row the handler's statements wrote, got {read}"
    );
}
