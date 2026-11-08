// SPDX-License-Identifier: CC0-1.0

//! The names one `@aws-sdk/client-*` package exports, read from the service's own Smithy
//! model.
//!
//! A package of the AWS SDK for JavaScript v3 exports a class per thing its service has: the
//! client, a `<Operation>Command` for every operation the service binds, and one error class
//! per error shape it declares. A handler written against the SDK binds those names — `import
//! { SQSClient, GetQueueUrlCommand, QueueDoesNotExist } from '@aws-sdk/client-sqs'` — and an
//! ES module's named imports are resolved when the module is LINKED, so the package has to
//! state the names before any of them is read. That list is what this module computes.
//!
//! It states none of the names itself: they are read out of the target service's own Smithy
//! model — the same file that service's emulator crate is generated from, embedded by
//! [`crate::state::execution::aws_api`] — so a command or an error class this runtime supplies
//! for a service is one that service really has, and neither can drift from the surface it
//! serves.

use ::std::collections::BTreeSet;
use ::std::sync::{Arc, OnceLock};

use ::serde_json::Value;

use crate::state::execution::aws_api;

/// The package this runtime supplies a client under, and the model whose names it exports.
///
/// One entry per service whose model this crate embeds ([`aws_api`]), under the token the SDK
/// publishes that service's client package by — its SDK id, lower cased with its words
/// hyphenated. A service `aws_api::APIS` lets a handler call that has no entry here is still
/// reachable: its package exports the client alone, because this runtime holds no model to
/// read the rest of the names out of. The test below holds every entry to being a service a
/// call may be made to, so the table cannot name one no call reaches.
const MODELS: &[(&str, &str)] = &[
    ("sqs", aws_api::SQS_MODEL),
    ("eventbridge", aws_api::EVENTBRIDGE_MODEL),
    ("dynamodb", aws_api::DYNAMODB_MODEL),
    ("cloudwatch-logs", aws_api::CLOUDWATCH_LOGS_MODEL),
    ("secrets-manager", aws_api::SECRETSMANAGER_MODEL),
    ("api-gateway", aws_api::APIGATEWAY_MODEL),
    ("sesv2", aws_api::SESV2_MODEL),
    ("s3", aws_api::S3_MODEL),
    ("sfn", aws_api::SFN_MODEL),
];

/// The names one client package exports, beside the client and the base exception its own
/// name gives ([`super`]).
#[derive(Debug, Default)]
pub struct ClientExports {
    /// The operations the service binds, each of which the package exports a
    /// `<Operation>Command` for.
    pub operations: Vec<String>,
    /// The error shapes the service declares, each of which the package exports a class of
    /// the shape's own name for.
    pub errors: Vec<String>,
}

/// The names the `@aws-sdk/client-<token>` package exports, for a service whose model this
/// emulator holds; an empty list for a service it holds none for, whose package therefore
/// exports its client alone.
///
/// What is held between calls is one cell per entry of [`MODELS`], so the memory this holds and
/// the number of models it parses are bounded by that fixed table and not by anything a run
/// asks for: a token no entry of it answers to parses nothing and holds nothing, however many
/// distinct ones a handler names.
#[must_use]
pub fn client_exports(token: &str) -> Arc<ClientExports> {
    /// The answer for a service this emulator holds no model for — one value, shared.
    static NONE_HELD: OnceLock<Arc<ClientExports>> = OnceLock::new();
    let none_held = || Arc::clone(NONE_HELD.get_or_init(|| Arc::new(ClientExports::default())));
    /// One cell per entry of that table, each filled on the first import of that service's
    /// package and never again.
    static CACHE: OnceLock<Vec<OnceLock<Arc<ClientExports>>>> = OnceLock::new();
    let Some(at) = service_at(token) else {
        return none_held();
    };
    let cache = CACHE.get_or_init(|| MODELS.iter().map(|_| OnceLock::new()).collect());
    let Some((cell, (_token, model))) = cache.get(at).zip(MODELS.get(at)) else {
        return none_held();
    };
    Arc::clone(cell.get_or_init(|| Arc::new(exports_of(model))))
}

/// Where in [`MODELS`] the service the package name's token names sits, or `None` when this
/// runtime holds no model for it.
fn service_at(token: &str) -> Option<usize> {
    let wanted = compacted(token);
    MODELS
        .iter()
        .position(|(named, _model)| compacted(named) == wanted)
}

/// A service token with its separators dropped.
///
/// One service is named by two spellings of the same token: the SDK publishes the client
/// package under the service's SDK id with the words hyphenated
/// (`@aws-sdk/client-cloudwatch-logs`) and an SDK client is built from that id with the words
/// run together (`boto3.client("cloudwatchlogs")`). Dropping the separators is what makes the
/// two the one token.
fn compacted(token: &str) -> String {
    token.chars().filter(|character| *character != '-').collect()
}

/// The names `model` states: every operation shape it declares, and every shape it marks as
/// an error.
///
/// Reading the shapes rather than the service's own operation list takes in the operations a
/// resource's lifecycle binds as well as the ones the service lists directly, which is the
/// same set the SDK generates a command for.
fn exports_of(model: &str) -> ClientExports {
    let shapes = ::serde_json::from_str::<Value>(model)
        .ok()
        .and_then(|document| document.get("shapes").cloned());
    let Some(Value::Object(shapes)) = shapes else {
        return ClientExports::default();
    };
    let mut operations = BTreeSet::new();
    let mut errors = BTreeSet::new();
    for (id, shape) in &shapes {
        let Some(name) = id.split('#').next_back().filter(|name| !name.is_empty()) else {
            continue;
        };
        match shape.get("type").and_then(Value::as_str) {
            Some("operation") => {
                operations.insert(name.to_string());
            }
            Some("structure") if is_error(shape) => {
                errors.insert(name.to_string());
            }
            _ => {}
        }
    }
    ClientExports {
        operations: operations.into_iter().collect(),
        errors: errors.into_iter().collect(),
    }
}

/// Whether `shape` is one the model marks as an error (`@error`), which is what makes the SDK
/// export a class for it.
fn is_error(shape: &Value) -> bool {
    shape
        .get("traits")
        .and_then(Value::as_object)
        .is_some_and(|traits| traits.contains_key("smithy.api#error"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names a client package exports are the service's own model's: one row per service
    /// a call may be made to, each asserted on an operation and an error shape of that
    /// service's model, and a row for a service whose model this emulator does not hold.
    #[test]
    fn a_packages_exported_names_are_its_services_own() {
        // Each row: the package's token, an operation the service's model declares, and an
        // error shape it declares.
        let cases: &[(&str, &str, &str)] = &[
            ("sqs", "GetQueueUrl", "QueueDoesNotExist"),
            ("sqs", "SendMessage", "InvalidMessageContents"),
            ("dynamodb", "PutItem", "ResourceNotFoundException"),
            ("eventbridge", "PutEvents", "ResourceNotFoundException"),
            ("eventbridge", "PutRule", "ConcurrentModificationException"),
            // The SDK publishes CloudWatch Logs as `client-cloudwatch-logs`, whose token is
            // the same service's as `cloudwatchlogs`.
            ("cloudwatch-logs", "PutLogEvents", "ResourceNotFoundException"),
            ("secretsmanager", "GetSecretValue", "ResourceNotFoundException"),
            ("apigateway", "GetRestApis", "NotFoundException"),
            ("sesv2", "SendEmail", "MessageRejected"),
            ("s3", "PutObject", "NoSuchKey"),
            ("sfn", "StartExecution", "ExecutionDoesNotExist"),
        ];
        for (token, operation, error) in cases {
            let exports = client_exports(token);
            assert!(
                exports.operations.iter().any(|held| held == operation),
                "case {token:?}: expected the operation {operation:?} among the \
                 {} the model declares, got none of it",
                exports.operations.len()
            );
            assert!(
                exports.errors.iter().any(|held| held == error),
                "case {token:?}: expected the error shape {error:?} among the {} the model \
                 declares, got none of it",
                exports.errors.len()
            );
        }
        for token in ["kinesis", "", "not-a-service"] {
            let exports = client_exports(token);
            assert!(
                exports.operations.is_empty() && exports.errors.is_empty(),
                "case {token:?}: this emulator holds no model for that service, so it states \
                 no name of one, got {} operations and {} errors",
                exports.operations.len(),
                exports.errors.len()
            );
        }
    }

    /// Every service this runtime states the names of is a service a call may be made to: the
    /// table holds a model for no service the seam does not reach.
    #[test]
    fn every_model_named_is_a_service_a_call_reaches() {
        for (token, _model) in MODELS {
            let wanted = compacted(token);
            assert!(
                aws_api::APIS
                    .iter()
                    .any(|api| compacted(api.sdk_token) == wanted),
                "case {token:?}: expected a service a call may be made to, got a token no entry \
                 of the reachable services answers to"
            );
        }
    }

    /// The same token asked twice is answered from the one reading of the model, and a token
    /// no service answers to holds nothing between calls — whatever spelling it is written
    /// in, since what is held is one cell per service and not one per name asked for.
    #[test]
    fn the_model_is_read_once_per_service() {
        let first = client_exports("sqs");
        let again = client_exports("sqs");
        assert!(
            Arc::ptr_eq(&first, &again),
            "expected the one reading of the model to answer both, got two"
        );
        // The two spellings of one token are the one service, so they share that cell.
        assert!(
            Arc::ptr_eq(&client_exports("cloudwatch-logs"), &client_exports("cloudwatchlogs")),
            "expected both spellings of the token to be the one service's cell"
        );
        let unknown = client_exports("not-a-service");
        assert!(
            Arc::ptr_eq(&unknown, &client_exports("another-absent-service")),
            "expected a service this emulator holds no model for to hold nothing of its own, \
             got a second value"
        );
    }
}
