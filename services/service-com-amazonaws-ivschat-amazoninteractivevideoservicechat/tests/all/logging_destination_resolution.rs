// SPDX-License-Identifier: CC0-1.0

//! Where a logging configuration's destination is resolved: in the service that OWNS it.
//!
//! The rule under test: every member of the modelled `DestinationConfiguration` union
//! names a resource of another service — an Amazon S3 bucket, a CloudWatch Logs log group,
//! an Amazon Kinesis Data Firehose delivery stream — and a write that names one
//! (`CreateLoggingConfiguration`, `UpdateLoggingConfiguration`) is accepted only once the
//! service that owns that resource reports holding it. A request naming one that service
//! does not hold is refused with the modelled `ResourceNotFoundException` (HTTP 404) whose
//! `message` is this service's one not-found rule — `Resource: <what the request named>
//! not found` — and nothing of this service's own is written: no configuration is minted,
//! and an update leaves the stored destination exactly as it was. The gate is the owning
//! service's state AT THE MOMENT OF THE CALL, not a list written here: a destination that
//! resolved once stops resolving when the owning service no longer holds it.
//!
//! Also here, because they are the other two things a caller reads off a
//! logging-configuration refusal: the not-found message rule for an identifier that names
//! no stored configuration, and the conflict wording for deleting one a room still logs
//! to — which names the operation and the identifier and nothing else.
//!
//! Rows span what each rule generalises over rather than one recorded request: WHICH union
//! member carries the destination, that member's value, whether the resource was never
//! created or was created and then deleted, which operation is called, and how many rooms
//! hold an attachment.
//!
//! Every service is mounted on ONE shared engine, exactly as the assembled emulator mounts
//! them, so each destination is created through its own service's wire and resolved through
//! the same state that service serves its own reads from.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::room_lifecycle::{error_code, str_field, ACCOUNT, REGION};

/// A destination each owning service really holds, one per member of the union. The values
/// differ in their alphabet as well as in their kind: a bucket name with dots and dashes,
/// a log group with slashes, a stream with underscores.
const HELD_BUCKET: &str = "ivschat-chat-logs.2026";
const HELD_LOG_GROUP: &str = "/aws/ivschat/held-room-logs";
const HELD_STREAM: &str = "ivschat_held_stream";

/// A destination of each kind that nothing ever created.
const ABSENT_BUCKET: &str = "ivschat-never-created-logs";
const ABSENT_LOG_GROUP: &str = "/aws/ivschat/never-created";
const ABSENT_STREAM: &str = "ivschat-never-created-stream";

/// The bucket the held delivery stream delivers to: a stream is created against an Amazon
/// S3 destination of its own.
const STREAM_SINK_BUCKET: &str = "ivschat-firehose-sink";

/// The role the held delivery stream assumes. No IAM service is mounted here, so this
/// names the role form Firehose documents without asserting anything about IAM.
const STREAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/firehose-delivery";

/// One emulator with this service and the three services that own a log destination, all
/// on one shared engine — what the assembled router hands every handler.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pin authentication off so the suite does not depend on the process setting.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// The SigV4 `Authorization` header of a request signed for `service`, which is how the
    /// router picks the service a request belongs to.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVSCHAT/20260101/{REGION}/{service}/\
             aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
        )
    }

    /// Call one of this service's restJson1 operations and return its status and body.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/{operation}"))
            .header("host", format!("ivschat.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", Self::authorization("ivschat"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        self.dispatch(request).await
    }

    /// Call one awsJson operation of `service`, whose `X-Amz-Target` prefix is
    /// `target_prefix`.
    async fn json_rpc(
        &self,
        service: &str,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{service}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header("authorization", Self::authorization(service))
            .body(Bytes::from(body.to_string()))
            .expect("a valid request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create every destination the accepted rows name, each through its owner's wire.
    async fn given_held_destinations(&self) {
        self.given_bucket(HELD_BUCKET).await;
        self.given_bucket(STREAM_SINK_BUCKET).await;
        self.given_log_group(HELD_LOG_GROUP).await;
        self.given_delivery_stream(HELD_STREAM).await;
    }

    /// Create an Amazon S3 bucket through Amazon S3's own (REST) wire. Path style, and with
    /// no `CreateBucketConfiguration`: an absent `LocationConstraint` places the bucket in
    /// [`REGION`], the Region every request here is signed for.
    async fn given_bucket(&self, name: &str) {
        let request = http::Request::builder()
            .method("PUT")
            .uri(format!("/{name}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", Self::authorization("s3"))
            .body(Bytes::new())
            .expect("a valid request");
        let (status, payload) = self.dispatch(request).await;
        assert_eq!(
            status, 200,
            "CreateBucket({name}) must succeed, got {status} with {payload}"
        );
    }

    /// Remove an Amazon S3 bucket, so nothing holds it any more.
    async fn delete_bucket(&self, name: &str) {
        let request = http::Request::builder()
            .method("DELETE")
            .uri(format!("/{name}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", Self::authorization("s3"))
            .body(Bytes::new())
            .expect("a valid request");
        let (status, payload) = self.dispatch(request).await;
        assert_eq!(
            status, 204,
            "DeleteBucket({name}) must succeed, got {status} with {payload}"
        );
    }

    async fn given_log_group(&self, name: &str) {
        let (status, payload) = self
            .json_rpc(
                "logs",
                "Logs_20140328",
                "CreateLogGroup",
                &json!({ "logGroupName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateLogGroup({name}) must succeed, got {status} with {payload}"
        );
    }

    async fn delete_log_group(&self, name: &str) {
        let (status, payload) = self
            .json_rpc(
                "logs",
                "Logs_20140328",
                "DeleteLogGroup",
                &json!({ "logGroupName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "DeleteLogGroup({name}) must succeed, got {status} with {payload}"
        );
    }

    async fn given_delivery_stream(&self, name: &str) {
        let (status, payload) = self
            .json_rpc(
                "firehose",
                "Firehose_20150804",
                "CreateDeliveryStream",
                &json!({
                    "DeliveryStreamName": name,
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": STREAM_ROLE_ARN,
                        "BucketARN": format!("arn:aws:s3:::{STREAM_SINK_BUCKET}"),
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateDeliveryStream({name}) must succeed, got {status} with {payload}"
        );
    }

    /// Create a logging configuration that must succeed, and answer its body.
    async fn given_logging_configuration(&self, body: &Value) -> Value {
        let (status, created) = self.call("CreateLoggingConfiguration", body).await;
        assert_eq!(
            status, 200,
            "CreateLoggingConfiguration({body}) must answer 200, got {status} {created}"
        );
        created
    }

    /// Every `name` the store currently holds a logging configuration under.
    async fn stored_names(&self) -> Vec<String> {
        let (status, listed) = self.call("ListLoggingConfigurations", &json!({})).await;
        assert_eq!(
            status, 200,
            "ListLoggingConfigurations must answer 200, got {status} {listed}"
        );
        listed
            .get("loggingConfigurations")
            .and_then(Value::as_array)
            .map(|summaries| {
                summaries
                    .iter()
                    .filter_map(|summary| summary.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The `message` an error payload carries, under either casing a protocol may render it.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The one message rule every not-found refusal of this service follows.
fn not_found_message(named: &str) -> String {
    format!("Resource: {named} not found")
}

/// The `destinationConfiguration` body for each member of the modelled union.
fn s3(bucket: &str) -> Value {
    json!({ "s3": { "bucketName": bucket } })
}

fn cloud_watch_logs(log_group: &str) -> Value {
    json!({ "cloudWatchLogs": { "logGroupName": log_group } })
}

fn firehose(stream: &str) -> Value {
    json!({ "firehose": { "deliveryStreamName": stream } })
}

/// (case, the destination body, the resource it names, whether its owner holds it)
fn destination_rows() -> Vec<(&'static str, Value, &'static str, bool)> {
    vec![
        (
            "an s3 bucket Amazon S3 holds",
            s3(HELD_BUCKET),
            HELD_BUCKET,
            true,
        ),
        (
            "an s3 bucket that was never created",
            s3(ABSENT_BUCKET),
            ABSENT_BUCKET,
            false,
        ),
        (
            "a log group CloudWatch Logs holds",
            cloud_watch_logs(HELD_LOG_GROUP),
            HELD_LOG_GROUP,
            true,
        ),
        (
            "a log group that was never created",
            cloud_watch_logs(ABSENT_LOG_GROUP),
            ABSENT_LOG_GROUP,
            false,
        ),
        (
            "a delivery stream Amazon Kinesis Data Firehose holds",
            firehose(HELD_STREAM),
            HELD_STREAM,
            true,
        ),
        (
            "a delivery stream that was never created",
            firehose(ABSENT_STREAM),
            ABSENT_STREAM,
            false,
        ),
        (
            "a bucket name in the log group member",
            cloud_watch_logs(HELD_BUCKET),
            HELD_BUCKET,
            false,
        ),
        (
            "a log group name in the s3 member, which is no bucket",
            s3("aws.ivschat.held-room-logs"),
            "aws.ivschat.held-room-logs",
            false,
        ),
    ]
}

#[tokio::test]
async fn create_resolves_the_destination_against_the_service_that_owns_it() {
    let emulator = Emulator::new();
    emulator.given_held_destinations().await;

    for (index, (case, destination, named, held)) in destination_rows().into_iter().enumerate() {
        let name = format!("cfg-{index}");
        let (status, body) = emulator
            .call(
                "CreateLoggingConfiguration",
                &json!({ "name": name, "destinationConfiguration": destination }),
            )
            .await;

        if held {
            assert_eq!(
                status, 200,
                "case {case:?}: the owning service holds {named:?}, so the create must \
                 answer 200, got {status} {body}"
            );
            // Read the configuration back through the implemented reader: what the create
            // answered is what the store holds.
            let arn = str_field(&body, "arn");
            let (status, fetched) = emulator
                .call("GetLoggingConfiguration", &json!({ "identifier": arn }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the created configuration must be readable, got {status} \
                 {fetched}"
            );
            assert_eq!(
                fetched.get("destinationConfiguration"),
                body.get("destinationConfiguration"),
                "case {case:?}: the stored destination must be the one the create answered, \
                 got {fetched}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {case:?}: no service holds {named:?}, so the create must answer the \
                 modelled 404, got {status} {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {code:?} in {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                not_found_message(named),
                "case {case:?}: expected the service's not-found message rule, got {message:?}"
            );
        }

        // A refused create leaves NOTHING behind: the name it asked for is absent from the
        // store, while an accepted one is there.
        let stored = emulator.stored_names().await;
        assert_eq!(
            stored.contains(&name),
            held,
            "case {case:?}: expected the store to hold {name:?} = {held}, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn update_resolves_the_destination_and_leaves_the_stored_one_on_a_refusal() {
    let emulator = Emulator::new();
    emulator.given_held_destinations().await;

    // Rows differ in the destination the update moves to, and in whether its owner holds
    // it; an accepted row becomes the destination the next row has to find unchanged.
    for (case, destination, named, held) in destination_rows() {
        let created = emulator
            .given_logging_configuration(&json!({
                "name": "updated-config",
                "destinationConfiguration": s3(HELD_BUCKET),
            }))
            .await;
        let arn = str_field(&created, "arn");

        let (status, body) = emulator
            .call(
                "UpdateLoggingConfiguration",
                &json!({ "identifier": arn, "destinationConfiguration": destination }),
            )
            .await;

        let expected_destination = if held {
            assert_eq!(
                status, 200,
                "case {case:?}: the owning service holds {named:?}, so the update must \
                 answer 200, got {status} {body}"
            );
            destination.clone()
        } else {
            assert_eq!(
                status, 404,
                "case {case:?}: no service holds {named:?}, so the update must answer the \
                 modelled 404, got {status} {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                not_found_message(named),
                "case {case:?}: expected the service's not-found message rule, got {message:?}"
            );
            s3(HELD_BUCKET)
        };

        let (status, fetched) = emulator
            .call("GetLoggingConfiguration", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the configuration must stay readable, got {status} {fetched}"
        );
        assert_eq!(
            fetched.get("destinationConfiguration"),
            Some(&expected_destination),
            "case {case:?}: expected the destination {expected_destination} (unchanged by a \
             refusal), got {fetched}"
        );
    }
}

#[tokio::test]
async fn a_destination_its_owner_no_longer_holds_stops_resolving() {
    let emulator = Emulator::new();
    emulator.given_held_destinations().await;

    // (case, the destination body, the resource it names, how its owner stops holding it)
    //
    // Two owners rather than three: Amazon Kinesis Data Firehose refuses to delete a
    // stream it is still creating ("cannot be deleted in CREATING state"), which is that
    // service's own rule and not this one's, so the "stops being held" row for a stream
    // would be asserting Firehose's lifecycle instead of this resolution.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        ("the s3 bucket", s3(HELD_BUCKET), HELD_BUCKET, "bucket"),
        (
            "the log group",
            cloud_watch_logs(HELD_LOG_GROUP),
            HELD_LOG_GROUP,
            "log group",
        ),
    ];

    for (case, destination, named, kind) in rows {
        // Accepted while the owning service holds it.
        emulator
            .given_logging_configuration(&json!({
                "name": "before-deletion",
                "destinationConfiguration": destination,
            }))
            .await;

        if kind == "bucket" {
            emulator.delete_bucket(named).await;
        } else {
            emulator.delete_log_group(named).await;
        }

        // The same request, now that nothing holds the destination.
        let (status, body) = emulator
            .call(
                "CreateLoggingConfiguration",
                &json!({
                    "name": "after-deletion",
                    "destinationConfiguration": destination,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: a destination its owner no longer holds must be refused, got \
             {status} {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            not_found_message(named),
            "case {case:?}: expected the service's not-found message rule, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_not_found_refusal_names_the_logging_configuration_identifier_the_caller_sent() {
    let emulator = Emulator::new();
    emulator.given_held_destinations().await;

    // One stored configuration throughout, so each 404 is about the identifier the request
    // carried and not about an empty store.
    emulator
        .given_logging_configuration(&json!({
            "name": "live",
            "destinationConfiguration": s3(HELD_BUCKET),
        }))
        .await;

    // A configuration that WAS created and then deleted: the rule has to cover an
    // identifier that was valid a moment ago.
    let doomed = emulator
        .given_logging_configuration(&json!({
            "name": "doomed",
            "destinationConfiguration": cloud_watch_logs(HELD_LOG_GROUP),
        }))
        .await;
    let deleted = str_field(&doomed, "arn");
    let (status, body) = emulator
        .call(
            "DeleteLoggingConfiguration",
            &json!({ "identifier": deleted }),
        )
        .await;
    assert_eq!(
        status, 204,
        "DeleteLoggingConfiguration must answer its modelled 204, got {status} {body}"
    );

    // Rows differ in HOW the identifier fails to name a configuration: created and then
    // deleted, never created, a different id length and alphabet, another region.
    let identifiers = vec![
        ("created then deleted", deleted.clone()),
        (
            "never created",
            configuration_arn(REGION, "neverCreated1"),
        ),
        ("a four-character id", configuration_arn(REGION, "Zz09")),
        (
            "another region",
            configuration_arn("eu-west-2", "elsewhere012"),
        ),
    ];

    for (case, identifier) in identifiers {
        for (operation, request) in [
            (
                "GetLoggingConfiguration",
                json!({ "identifier": identifier }),
            ),
            (
                "UpdateLoggingConfiguration",
                json!({ "identifier": identifier, "name": "renamed" }),
            ),
            (
                "DeleteLoggingConfiguration",
                json!({ "identifier": identifier }),
            ),
        ] {
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                status, 404,
                "case {case:?} {operation}: expected HTTP 404, got {status} {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?} {operation}: expected ResourceNotFoundException, got {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                not_found_message(&identifier),
                "case {case:?} {operation}: expected the service's not-found message rule, \
                 got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_an_attached_configuration_names_the_operation_and_the_identifier() {
    // Rows differ in how many rooms hold the attachment: the refusal names the operation
    // and the identifier, and reports nothing about how many rooms hold it.
    for attached_rooms in [1_usize, 2, 3] {
        let emulator = Emulator::new();
        emulator.given_held_destinations().await;
        let created = emulator
            .given_logging_configuration(&json!({
                "name": "attached-config",
                "destinationConfiguration": firehose(HELD_STREAM),
            }))
            .await;
        let arn = str_field(&created, "arn");

        let mut rooms = Vec::new();
        for index in 0..attached_rooms {
            let (status, room) = emulator
                .call(
                    "CreateRoom",
                    &json!({
                        "name": format!("attached-{index}"),
                        "loggingConfigurationIdentifiers": [arn],
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {attached_rooms} rooms: CreateRoom must answer 200, got {status} {room}"
            );
            rooms.push(str_field(&room, "arn"));
        }

        let (status, body) = emulator
            .call("DeleteLoggingConfiguration", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (409, "ConflictException".to_string()),
            "case {attached_rooms} rooms: expected the modelled 409 ConflictException, got \
             {status} {body}"
        );
        let expected = format!(
            "Unable to perform ivschat:DeleteLoggingConfiguration while {arn} is still \
             attached to a chat room"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {attached_rooms} rooms: expected {expected:?}, got {message:?}"
        );

        // Detaching every room clears the conflict, and the delete then succeeds.
        for room in &rooms {
            let (status, updated) = emulator
                .call(
                    "UpdateRoom",
                    &json!({ "identifier": room, "loggingConfigurationIdentifiers": [] }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {attached_rooms} rooms: UpdateRoom must answer 200, got {status} {updated}"
            );
        }
        let (status, body) = emulator
            .call("DeleteLoggingConfiguration", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            status, 204,
            "case {attached_rooms} rooms: the delete must succeed once nothing is attached, \
             got {status} {body}"
        );
    }
}

/// A logging-configuration ARN for `id` in `region`, in the form the modelled
/// `LoggingConfigurationArn` pattern admits.
fn configuration_arn(region: &str, id: &str) -> String {
    format!("arn:aws:ivschat:{region}:{ACCOUNT}:logging-configuration/{id}")
}
