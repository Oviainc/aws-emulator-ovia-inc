// SPDX-License-Identifier: CC0-1.0

//! What a handler logs through its language's LOGGING FACILITY, rather than by writing to its
//! own output: where those records go.
//!
//! RULE: a managed runtime delivers to the invocation's log every record the handler emits
//! through the language's standard logging facility whose level the emitting logger admits,
//! rendered as that runtime renders a record — for Python, the level in brackets, the instant
//! the record was created in UTC to the millisecond, the invocation's request id and the
//! message, tab separated. The runtime does that by installing the handler the facility needs
//! to have an output at all, and it is the only one: a record is written once, and a record
//! the logger's own level does not admit is written not at all.
//!
//! That is why AWS's own documented shape of a Python handler —
//! `logger = logging.getLogger()`, `logger.setLevel(logging.INFO)`, `logger.info(...)` —
//! leaves lines in the function's log group, while a library with no handler installed takes
//! nothing below a warning and renders the bare message.
//!
//! Every row drives real HTTP through the router and reads what the RUN produced: the log the
//! invocation itself reports (`X-Amz-Log-Result`, "the last 4 KB of the execution log"), and
//! for one row the events CloudWatch Logs holds for the function's group, which is where a
//! client reads them. The rows differ along what the rule generalises over: the level the
//! logger is set to, the level of the record emitted, whether the logger is the root one or a
//! named one, whether the message carries `%`-style arguments, whether the record carries an
//! exception, and whether the handler itself calls the library's own configuration helper.
//!
//! Every test here RUNS handlers, so each takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use crate::invocation_log_delivery::{create_function, emulator, invoke, logs, Created,
                                    PYTHON_RUNTIME};
use crate::invoke_request_id::one_invocation_stream_at_a_time;
use serde_json::json;

/// A well-formed execution role ARN. No IAM is mounted here, so nothing is known about the
/// role and no log write is withheld.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How many fields one rendered record carries: the level, the instant, the request id and the
/// message.
const RECORD_FIELDS: usize = 4;

/// One row: the handler's body, the records it must leave in the log, and the messages it must
/// leave nowhere.
struct Row {
    case: &'static str,
    /// The body of the handler, which decides what it logs.
    source: &'static str,
    /// The `(level, message)` of every record the run must have left, in the order emitted.
    logged: &'static [(&'static str, &'static str)],
    /// Message text the log must not carry at all — a record the logger's level excludes.
    silent: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a logger set to INFO carries every record at or above it",
        source: "import logging\n\
                 \n\
                 LOGGER = logging.getLogger()\n\
                 LOGGER.setLevel(logging.INFO)\n\
                 \n\
                 \n\
                 def lambda_handler(event, context):\n    \
                     LOGGER.debug('below the level')\n    \
                     LOGGER.info('the info record')\n    \
                     LOGGER.warning('the warning record')\n    \
                     LOGGER.error('the error record')\n    \
                     LOGGER.critical('the critical record')\n    \
                     return {'ok': True}\n",
        logged: &[
            ("INFO", "the info record"),
            ("WARNING", "the warning record"),
            ("ERROR", "the error record"),
            ("CRITICAL", "the critical record"),
        ],
        silent: &["below the level"],
    },
    Row {
        case: "a logger left at the library's default takes nothing below a warning",
        source: "import logging\n\
                 \n\
                 LOGGER = logging.getLogger()\n\
                 \n\
                 \n\
                 def lambda_handler(event, context):\n    \
                     LOGGER.info('not at the default level')\n    \
                     LOGGER.warning('at the default level')\n    \
                     return {'ok': True}\n",
        logged: &[("WARNING", "at the default level")],
        silent: &["not at the default level"],
    },
    Row {
        case: "a named logger set to DEBUG carries a debug record",
        source: "import logging\n\
                 \n\
                 LOGGER = logging.getLogger('orders.intake')\n\
                 LOGGER.setLevel(logging.DEBUG)\n\
                 \n\
                 \n\
                 def lambda_handler(event, context):\n    \
                     LOGGER.debug('the debug record')\n    \
                     return {'ok': True}\n",
        logged: &[("DEBUG", "the debug record")],
        silent: &[],
    },
    Row {
        case: "a message carrying arguments is interpolated",
        source: "import logging\n\
                 \n\
                 LOGGER = logging.getLogger()\n\
                 LOGGER.setLevel(logging.INFO)\n\
                 \n\
                 \n\
                 def lambda_handler(event, context):\n    \
                     LOGGER.info('processing %s/%s', 'raw-bucket', 'inbox/one.json')\n    \
                     return {'ok': True}\n",
        logged: &[("INFO", "processing raw-bucket/inbox/one.json")],
        silent: &["%s"],
    },
    Row {
        case: "a record emitted while the module is imported is carried too",
        source: "import logging\n\
                 \n\
                 LOGGER = logging.getLogger()\n\
                 LOGGER.setLevel(logging.INFO)\n\
                 LOGGER.info('the import-time record')\n\
                 \n\
                 \n\
                 def lambda_handler(event, context):\n    \
                     return {'ok': True}\n",
        logged: &[("INFO", "the import-time record")],
        silent: &[],
    },
    Row {
        case: "the library's own configuration helper adds no second handler",
        source: "import logging\n\
                 \n\
                 logging.basicConfig()\n\
                 LOGGER = logging.getLogger()\n\
                 LOGGER.setLevel(logging.INFO)\n\
                 \n\
                 \n\
                 def lambda_handler(event, context):\n    \
                     LOGGER.info('written once')\n    \
                     return {'ok': True}\n",
        logged: &[("INFO", "written once")],
        silent: &[],
    },
];

/// Every row: the records the handler emitted are in the log, rendered as the runtime renders
/// one, and the records their logger's level excluded are nowhere in it.
#[tokio::test]
async fn what_a_handler_logs_through_the_logging_facility_reaches_its_log() {
    let _stream = one_invocation_stream_at_a_time().await;
    let router = emulator();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("logging-facility-{index}");
        create_function(
            &router,
            &Created {
                case,
                name: &name,
                source: row.source,
                runtime: PYTHON_RUNTIME,
                role: ROLE,
                log_group: None,
            },
        )
        .await;
        let (log, payload) = invoke(&router, case, &name).await;
        assert_eq!(
            payload,
            json!({ "ok": true }),
            "case {case:?}: the handler must have returned, got {payload} with the log {log:?}"
        );
        let request_id = request_id_of(case, &log);
        for (level, message) in row.logged {
            let rendered = rendered_record(&log, level, message);
            assert_eq!(
                rendered.len(),
                1,
                "case {case:?}: expected exactly one [{level}] record of {message:?}, got \
                 {rendered:?} in the log {log:?}"
            );
            let fields: Vec<&str> = rendered[0].split('\t').collect();
            assert_eq!(
                fields.len(),
                RECORD_FIELDS,
                "case {case:?}: expected {RECORD_FIELDS} tab-separated fields on the record, \
                 got {fields:?}"
            );
            assert!(
                is_instant(fields[1]),
                "case {case:?}: expected the record's instant in UTC to the millisecond, got \
                 {:?}",
                fields[1]
            );
            assert_eq!(
                fields[2], request_id,
                "case {case:?}: expected the record to name the invocation {request_id:?}, got \
                 {:?}",
                fields[2]
            );
        }
        for message in row.silent {
            assert!(
                !log.contains(*message),
                "case {case:?}: expected {message:?} nowhere in the log, got {log:?}"
            );
        }
    }
}

/// The records the handler emitted are what CloudWatch Logs holds for the function's group:
/// one event per record, read the way a client reads them.
#[tokio::test]
async fn the_logged_records_are_the_events_the_log_group_holds() {
    let _stream = one_invocation_stream_at_a_time().await;
    let case = "the group a logged record lands in";
    let router = emulator();
    let name = "logging-facility-delivered";
    create_function(
        &router,
        &Created {
            case,
            name,
            source: ROWS[0].source,
            runtime: PYTHON_RUNTIME,
            role: ROLE,
            log_group: None,
        },
    )
    .await;
    let (log, _) = invoke(&router, case, name).await;
    let request_id = request_id_of(case, &log);
    let answered = logs(
        &router,
        case,
        "FilterLogEvents",
        &json!({ "logGroupName": format!("/aws/lambda/{name}") }),
    )
    .await;
    let messages: Vec<String> = answered["events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected events, got {answered}"))
        .iter()
        .map(|event| event["message"].as_str().unwrap_or_default().to_string())
        .collect();
    for (level, message) in ROWS[0].logged {
        let carried: Vec<&String> = messages
            .iter()
            .filter(|line| line.starts_with(&format!("[{level}]\t")) && line.ends_with(message))
            .collect();
        assert_eq!(
            carried.len(),
            1,
            "case {case:?}: expected one event for the [{level}] record of {message:?}, got \
             {messages:?}"
        );
        assert!(
            carried[0].contains(&request_id),
            "case {case:?}: expected the event to name the invocation {request_id:?}, got {:?}",
            carried[0]
        );
    }
    for message in ROWS[0].silent {
        assert!(
            !messages.iter().any(|line| line.contains(*message)),
            "case {case:?}: expected {message:?} in no event, got {messages:?}"
        );
    }
}

/// Every line of `log` that renders a record of `level` whose message is `message`.
fn rendered_record<'log>(log: &'log str, level: &str, message: &str) -> Vec<&'log str> {
    let opens = format!("[{level}]\t");
    log.lines()
        .filter(|line| line.starts_with(&opens) && line.ends_with(message))
        .collect()
}

/// Whether `field` is the instant a rendered record carries: an ISO-8601 UTC instant to the
/// millisecond, `YYYY-MM-DDTHH:MM:SS.mmmZ`.
fn is_instant(field: &str) -> bool {
    let bytes = field.as_bytes();
    bytes.len() == 24
        && bytes[10] == b'T'
        && bytes[19] == b'.'
        && bytes[23] == b'Z'
        && bytes[23 - 3..23].iter().all(u8::is_ascii_digit)
        && field[..19]
            .chars()
            .all(|character| character.is_ascii_digit() || "-T:".contains(character))
}

/// The invocation's request id, read off the `START` line the runtime writes.
fn request_id_of(case: &str, log: &str) -> String {
    log.lines()
        .find_map(|line| line.strip_prefix("START RequestId: "))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("case {case:?}: expected a START line in the log {log:?}"))
        .to_string()
}
