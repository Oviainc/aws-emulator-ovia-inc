// SPDX-License-Identifier: CC0-1.0

//! One PROBE: a single request sent to find out how this service answers it, and the
//! assertions every probing suite makes about that answer.
//!
//! A refusal suite drives a TABLE of requests, and every row of it owes the same two
//! facts — the HTTP status and the wire error code — before the row's own assertion
//! about the sentence. Those two live here so a suite states them once and spends its
//! own rows on the rule it is about.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, error_code, message};
use crate::table_items::create_active_table;

/// Drive one request that must be REFUSED as a bad request, answering the message it
/// was refused with.
///
/// Every refusal of a request's own members is the same 400 `ValidationException`, so
/// the status and the wire code are asserted here and the caller asserts the sentence.
pub(crate) async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
) -> String {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ValidationException"),
        "case {case:?} ({operation}): expected a 400 ValidationException, got {status} with \
         {answered}"
    );
    message(&answered)
}

/// Drive one request that must be refused with a NAMED wire error code, answering its
/// message — for the rows whose fact is that a resource is not there at all.
pub(crate) async fn refusal_coded(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
    code: &str,
) -> String {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, code),
        "case {case:?} ({operation}): expected a 400 {code}, got {status} with {answered}"
    );
    message(&answered)
}

/// A table keyed by one `S` partition key named `pk`, ready to serve: the smallest
/// table a probe of a request's own members needs to address.
pub(crate) async fn probe_table(router: &MultiServiceRouter, name: &str) -> Value {
    create_active_table(router, name, &[("pk", "HASH", "S")]).await
}
