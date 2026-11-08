// SPDX-License-Identifier: CC0-1.0

//! The two preconditions `CreateWorkflowDefinition` settles BEFORE it stores anything.
//!
//! The rules under test, both of which AWS answers instead of creating the definition:
//!
//! 1. `CreateWorkflowDefinitionRequest$name` "Must be unique within your account and
//!    region", so a create naming a definition this account already holds is refused with
//!    the modelled `ConflictException` (HTTP 409) naming that definition's ARN as
//!    `resourceId` and `workflow-definition` as `resourceType` — whatever the rest of the
//!    body says and whatever status the held definition is in — and the held definition
//!    keeps every attribute its own create wrote. Only the `clientToken` the create
//!    carried replays its success, which is what `@idempotent` asks for; that same token
//!    sent with a different body conflicts too.
//! 2. an `exportConfig` names the Amazon S3 bucket this service exports workflow data to,
//!    so a create naming a bucket the account does not hold — or a name no account could
//!    hold a bucket under at all — is refused with the modelled `ValidationException`
//!    (HTTP 400) and NOTHING is stored, while a bucket the account does hold is created
//!    and reads back.
//!
//! The rows span what each rule generalises over — the name's form and length, which
//! optional members the repeat carries, whether a token is sent, the held definition's
//! status, whether the bucket exists, was deleted again, was never created or is a name
//! Amazon S3 holds no bucket under, the bucket's name length, and whether a key prefix is
//! carried — because the rule is about the state the call is judged against, not about one
//! request's bytes.
//!
//! The Amazon S3 service is mounted beside this one on ONE engine, exactly as the
//! assembled router mounts them, and every bucket is created and deleted through S3's own
//! front door: what the export refusal is grounded in is S3's answer, not anything this
//! suite seeded.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::state::DELETION_COMPLETION_DELAY;
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::workflow_lifecycle::client_token;

/// The region every request below is signed for, and so the region of every ARN they
/// report.
const REGION: &str = "us-east-1";

/// The refusal text a create conflicting on its name answers with.
const ALREADY_EXISTS: &str = "Workflow definition already exists";

/// Long enough for the `DELETING` definition's removal to have become due
/// (`DELETION_COMPLETION_DELAY`), with margin for a loaded machine.
const PAST_THE_DELETION: Duration =
    DELETION_COMPLETION_DELAY.saturating_add(Duration::from_millis(400));

/// One answer off the wire: the status, the parsed body and the error code the protocol
/// put in `x-amzn-errortype` (the header an SDK reads the error class off).
struct Reply {
    status: u16,
    body: Value,
    error_type: Option<String>,
}

/// This service AND Amazon S3 on one shared engine, reached through the production router.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these cases are about; the credential scope below is
        // still read for the routing key and the ARN region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router }
    }

    /// One request to this service's front door.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serializable body")),
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20240101/{REGION}/nova-act/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .header("content-type", "application/json")
            .body(payload)
            .expect("valid request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Reply {
            status,
            body,
            error_type,
        }
    }

    /// `CreateWorkflowDefinition` with `body` as written.
    async fn create(&self, body: Value) -> Reply {
        self.call("PUT", "/workflow-definitions", Some(body)).await
    }

    /// `GetWorkflowDefinition` for `name`.
    async fn get(&self, name: &str) -> Reply {
        self.call("GET", &format!("/workflow-definitions/{name}"), None)
            .await
    }

    /// `DeleteWorkflowDefinition` for `name`.
    async fn delete(&self, name: &str) -> Reply {
        self.call("DELETE", &format!("/workflow-definitions/{name}"), None)
            .await
    }

    /// One request to Amazon S3's front door, path-style because the emulator serves one
    /// endpoint per region. Answers the HTTP status.
    async fn s3(&self, method: &str, bucket: &str) -> u16 {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com/{bucket}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("content-type", "application/xml")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/s3/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(Bytes::new())
            .expect("valid request");
        self.router.dispatch(request).await.status().as_u16()
    }

    /// `CreateBucket` through Amazon S3 itself. No `LocationConstraint`, so the bucket
    /// lands in `us-east-1`, the region these requests are signed for.
    async fn create_bucket(&self, bucket: &str) {
        let status = self.s3("PUT", bucket).await;
        assert_eq!(
            status, 200,
            "bucket {bucket}: expected CreateBucket to answer 200, got {status}"
        );
    }

    /// `DeleteBucket` through Amazon S3 itself.
    async fn delete_bucket(&self, bucket: &str) {
        let status = self.s3("DELETE", bucket).await;
        assert_eq!(
            status, 204,
            "bucket {bucket}: expected DeleteBucket to answer 204, got {status}"
        );
    }
}

/// `arn:aws:nova-act:<region>:<account>:workflow-definition/<name>`, the identifier a
/// conflict about a definition names it by.
fn definition_arn(name: &str) -> String {
    format!(
        "arn:aws:nova-act:{REGION}:{}:workflow-definition/{name}",
        engine::emulator_account_id()
    )
}

/// A name `WorkflowDefinitionName` admits (`^[a-zA-Z0-9_-]{1,40}$`), of `length`
/// characters, seeded with `seed` so two rows never collide.
fn name_of_length(seed: &str, length: usize) -> String {
    let mut name = seed.to_string();
    while name.len() < length {
        name.push('x');
    }
    name.truncate(length);
    name
}

/// Assert `reply` is the modelled conflict about `name`, by status, error class, and the
/// `resourceId`/`resourceType` members the model requires.
fn assert_name_conflict(case: &str, reply: &Reply, name: &str) {
    assert_eq!(
        reply.status, 409,
        "case {case:?}: expected HTTP 409, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.body["__type"], "ConflictException",
        "case {case:?}: expected ConflictException, got {}",
        reply.body
    );
    let error_type = reply.error_type.clone().unwrap_or_default();
    assert!(
        error_type.starts_with("ConflictException"),
        "case {case:?}: expected the x-amzn-errortype header to name ConflictException, got {error_type:?}"
    );
    assert_eq!(
        reply.body["resourceId"], definition_arn(name),
        "case {case:?}: the conflict must name the held definition's ARN, got {}",
        reply.body
    );
    assert_eq!(
        reply.body["resourceType"], "workflow-definition",
        "case {case:?}: the conflict must name the resource type, got {}",
        reply.body
    );
}

/// RULE 1: a name the account already holds a definition under is refused, whatever the
/// rest of the body says, and the held definition is left exactly as its own create wrote
/// it.
#[tokio::test]
async fn a_name_the_account_already_holds_is_refused_whatever_the_body_says() {
    let harness = Harness::new();
    // One bucket the account really holds, so the first create of each row carries an
    // `exportConfig` the export precondition accepts.
    harness.create_bucket("nova-act-precondition-export").await;

    // Names differing along what the rule generalises over: the shortest the model admits,
    // a mixed-case name carrying both punctuation characters, and the longest it admits.
    let names = [
        ("the shortest name the model admits", name_of_length("a", 1)),
        ("a name with both punctuation characters", "Export_Probe-01".to_string()),
        ("the longest name the model admits", name_of_length("Long_name", 40)),
    ];

    for (index, (name_case, name)) in names.into_iter().enumerate() {
        // The token's seed is the ROW's index, not its name: `ClientToken`'s pattern
        // (`^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`) admits no underscore, which the names do.
        let token = client_token(&format!("first{index}"));
        let first = json!({
            "name": name,
            "description": "the first body",
            "exportConfig": { "s3BucketName": "nova-act-precondition-export" },
            "clientToken": token,
        });
        let created = harness.create(first).await;
        assert_eq!(
            created.status, 201,
            "case {name_case:?}: the first create must answer 201, got {} {}",
            created.status, created.body
        );

        // Repeats differing along what the rule generalises over: which optional members
        // the body carries, whether they match the stored record, and whether a (fresh)
        // idempotency token is sent at all.
        let repeats = vec![
            (
                "the identical body with no token",
                json!({
                    "name": name,
                    "description": "the first body",
                    "exportConfig": { "s3BucketName": "nova-act-precondition-export" },
                }),
            ),
            (
                "a different description with no token",
                json!({ "name": name, "description": "a second body" }),
            ),
            ("the name alone", json!({ "name": name })),
            (
                "the identical body under a fresh token",
                json!({
                    "name": name,
                    "description": "the first body",
                    "exportConfig": { "s3BucketName": "nova-act-precondition-export" },
                    "clientToken": client_token(&format!("fresh{index}")),
                }),
            ),
            (
                "a different export prefix under a fresh token",
                json!({
                    "name": name,
                    "description": "the first body",
                    "exportConfig": {
                        "s3BucketName": "nova-act-precondition-export",
                        "s3KeyPrefix": "exports/second",
                    },
                    "clientToken": client_token(&format!("second{index}")),
                }),
            ),
        ];

        for (repeat_case, body) in repeats {
            let case = format!("{name_case}, {repeat_case}");
            let reply = harness.create(body).await;
            assert_name_conflict(&case, &reply, &name);
            assert_eq!(
                reply.body["message"], ALREADY_EXISTS,
                "case {case:?}: expected the refusal to state that the definition exists, got {}",
                reply.body
            );
        }

        // Nothing the refused repeats carried reached the store: the definition still
        // reports what its OWN create wrote.
        let read = harness.get(&name).await;
        assert_eq!(
            read.status, 200,
            "case {name_case:?}: the held definition must still resolve, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.body["description"], "the first body",
            "case {name_case:?}: a refused repeat must not rewrite the record, got {}",
            read.body
        );
        assert_eq!(
            read.body["exportConfig"],
            json!({ "s3BucketName": "nova-act-precondition-export" }),
            "case {name_case:?}: a refused repeat must not rewrite the export config, got {}",
            read.body
        );
        assert_eq!(
            read.body["arn"], definition_arn(&name),
            "case {name_case:?}: the held definition reports the ARN the conflict named, got {}",
            read.body
        );
    }
}

/// RULE 1, continued: the conflict is decided from the store at the moment of the call —
/// a definition on its way out still holds its name, and once the removal the clock owes
/// has happened the name is free again.
#[tokio::test]
async fn the_name_is_held_while_a_definition_is_deleting_and_free_once_it_is_gone() {
    let harness = Harness::new();
    let name = "fading-precondition";
    let created = harness.create(json!({ "name": name })).await;
    assert_eq!(
        created.status, 201,
        "the first create must answer 201, got {} {}",
        created.status, created.body
    );

    let deleted = harness.delete(name).await;
    assert_eq!(
        deleted.status, 202,
        "the delete must answer 202, got {} {}",
        deleted.status, deleted.body
    );

    // While the definition is DELETING the name is still that resource's.
    let refused = harness.create(json!({ "name": name })).await;
    assert_name_conflict("a DELETING definition still holds its name", &refused, name);

    // Once the removal is due and has been made, the same create is served.
    tokio::time::sleep(PAST_THE_DELETION).await;
    let again = harness.create(json!({ "name": name })).await;
    assert_eq!(
        again.status, 201,
        "case the removal is due: the name must be free again, got {} {}",
        again.status, again.body
    );
    assert_eq!(
        again.body["status"], "ACTIVE",
        "case the removal is due: the fresh definition reports its stored status, got {}",
        again.body
    );
}

/// RULE 1 and `@idempotent` together: the token a create carried replays THAT create, and
/// nothing else does — the same token with a different body conflicts, naming the ARN of
/// the definition it created.
#[tokio::test]
async fn the_token_that_created_a_definition_replays_it_and_a_different_body_conflicts() {
    let harness = Harness::new();
    let name = "tokened-precondition";
    let token = client_token("replay");
    let body = json!({
        "name": name,
        "description": "the first body",
        "clientToken": token,
    });

    let created = harness.create(body.clone()).await;
    assert_eq!(
        created.status, 201,
        "the first create must answer 201, got {} {}",
        created.status, created.body
    );

    let replayed = harness.create(body).await;
    assert_eq!(
        replayed.status, 201,
        "case the token's own request: a replay must answer 201, got {} {}",
        replayed.status, replayed.body
    );
    assert_eq!(
        replayed.body["status"], "ACTIVE",
        "case the token's own request: the replay reports the stored status, got {}",
        replayed.body
    );

    let mismatched = harness
        .create(json!({
            "name": name,
            "description": "a second body",
            "clientToken": token,
        }))
        .await;
    assert_name_conflict("the same token with a different body", &mismatched, name);
}

/// One row of the export-destination table: the bucket the create names, how it got into
/// (or out of) Amazon S3, and what the create must answer.
struct ExportCase {
    case: &'static str,
    /// The definition's name, so each row reads its own record back.
    name: &'static str,
    /// The bucket the `exportConfig` names, or `None` for a create that carries no
    /// `exportConfig` at all.
    bucket: Option<&'static str>,
    /// An optional `s3KeyPrefix` beside the bucket.
    prefix: Option<&'static str>,
    /// Whether the bucket is created through Amazon S3 before the call.
    create_bucket: bool,
    /// Whether that bucket is then deleted again before the call.
    delete_bucket: bool,
    /// Whether the create must be refused.
    refused: bool,
}

fn export_cases() -> Vec<ExportCase> {
    vec![
        ExportCase {
            case: "a bucket the account holds",
            name: "export-held",
            bucket: Some("nova-act-export-held"),
            prefix: None,
            create_bucket: true,
            delete_bucket: false,
            refused: false,
        },
        ExportCase {
            case: "a bucket the account holds, with a key prefix",
            name: "export-prefixed",
            bucket: Some("nova-act-export-prefixed"),
            prefix: Some("exports/workflow-data"),
            create_bucket: true,
            delete_bucket: false,
            refused: false,
        },
        // The shortest bucket name Amazon S3 admits (3 characters), so the row varies the
        // destination's length as well as its existence.
        ExportCase {
            case: "a bucket at the shortest name S3 admits",
            name: "export-short-name",
            bucket: Some("nov"),
            prefix: None,
            create_bucket: true,
            delete_bucket: false,
            refused: false,
        },
        ExportCase {
            case: "a bucket nothing ever created",
            name: "export-absent",
            bucket: Some("nova-act-export-absent"),
            prefix: None,
            create_bucket: false,
            delete_bucket: false,
            refused: true,
        },
        ExportCase {
            case: "a bucket that was created and then deleted",
            name: "export-deleted",
            bucket: Some("nova-act-export-deleted"),
            prefix: Some("exports/gone"),
            create_bucket: true,
            delete_bucket: true,
            refused: true,
        },
        // Names the modelled `@length`/`@pattern` admits but the published Amazon S3
        // general-purpose bucket naming rules do not, so no account holds a bucket under
        // any of them: a reserved prefix, a reserved suffix, adjacent periods, and the
        // IP-address form (`service_common::s3_bucket_name`).
        ExportCase {
            case: "a name carrying a reserved prefix",
            name: "export-reserved-prefix",
            bucket: Some("xn--nova-act-exports"),
            prefix: None,
            create_bucket: false,
            delete_bucket: false,
            refused: true,
        },
        ExportCase {
            case: "a name carrying a reserved suffix",
            name: "export-reserved-suffix",
            bucket: Some("nova-act-exports-s3alias"),
            prefix: None,
            create_bucket: false,
            delete_bucket: false,
            refused: true,
        },
        ExportCase {
            case: "a name with adjacent periods",
            name: "export-adjacent-periods",
            bucket: Some("nova..act.exports"),
            prefix: None,
            create_bucket: false,
            delete_bucket: false,
            refused: true,
        },
        ExportCase {
            case: "a name formatted as an IP address",
            name: "export-ip-form",
            bucket: Some("192.168.5.4"),
            prefix: None,
            create_bucket: false,
            delete_bucket: false,
            refused: true,
        },
        ExportCase {
            case: "no export configuration at all",
            name: "export-none",
            bucket: None,
            prefix: None,
            create_bucket: false,
            delete_bucket: false,
            refused: false,
        },
    ]
}

/// RULE 2: the export destination is resolved against Amazon S3 before the definition is
/// stored — a bucket the account does not hold is refused 400 and leaves no definition
/// behind, and one it does hold is created and reads its destination back.
#[tokio::test]
async fn an_export_bucket_the_account_does_not_hold_is_refused_before_anything_is_stored() {
    let harness = Harness::new();

    for case in export_cases() {
        if let (Some(bucket), true) = (case.bucket, case.create_bucket) {
            harness.create_bucket(bucket).await;
            if case.delete_bucket {
                harness.delete_bucket(bucket).await;
            }
        }

        let mut body = json!({ "name": case.name, "description": "exporting" });
        if let Some(bucket) = case.bucket {
            let mut export = json!({ "s3BucketName": bucket });
            if let Some(prefix) = case.prefix {
                export["s3KeyPrefix"] = json!(prefix);
            }
            body["exportConfig"] = export;
        }
        let reply = harness.create(body).await;

        if case.refused {
            let bucket = case.bucket.expect("a refused row names a bucket");
            assert_eq!(
                reply.status, 400,
                "case {:?}: expected HTTP 400, got {} {}",
                case.case, reply.status, reply.body
            );
            assert_eq!(
                reply.body["__type"], "ValidationException",
                "case {:?}: expected ValidationException, got {}",
                case.case, reply.body
            );
            let error_type = reply.error_type.clone().unwrap_or_default();
            assert!(
                error_type.starts_with("ValidationException"),
                "case {:?}: expected the x-amzn-errortype header to name ValidationException, got {error_type:?}",
                case.case
            );
            let expected = format!(
                "Failed to validate export bucket '{bucket}'. Ensure the bucket exists, is \
                 accessible, and is owned by the calling account ({}).",
                engine::emulator_account_id()
            );
            assert_eq!(
                reply.body["message"], expected,
                "case {:?}: expected the refusal to name the bucket and the account, got {}",
                case.case, reply.body
            );
            assert_eq!(
                reply.body["reason"], "FieldValidationFailed",
                "case {:?}: the refusal reports which class of validation failed, got {}",
                case.case, reply.body
            );
            assert_eq!(
                reply.body["fieldList"][0]["name"], "exportConfig.s3BucketName",
                "case {:?}: the refusal names the member to fix, got {}",
                case.case, reply.body
            );

            // Refused BEFORE anything was stored: the definition does not exist.
            let read = harness.get(case.name).await;
            assert_eq!(
                read.status, 404,
                "case {:?}: a refused create must leave no definition behind, got {} {}",
                case.case, read.status, read.body
            );
        } else {
            assert_eq!(
                reply.status, 201,
                "case {:?}: expected HTTP 201, got {} {}",
                case.case, reply.status, reply.body
            );
            assert_eq!(
                reply.body["status"], "ACTIVE",
                "case {:?}: the create reports the stored status, got {}",
                case.case, reply.body
            );

            // Round trip: the destination the create accepted is the one the record
            // reports.
            let read = harness.get(case.name).await;
            assert_eq!(
                read.status, 200,
                "case {:?}: the definition must resolve, got {} {}",
                case.case, read.status, read.body
            );
            let expected_export = match (case.bucket, case.prefix) {
                (None, _) => Value::Null,
                (Some(bucket), None) => json!({ "s3BucketName": bucket }),
                (Some(bucket), Some(prefix)) => {
                    json!({ "s3BucketName": bucket, "s3KeyPrefix": prefix })
                }
            };
            assert_eq!(
                read.body
                    .get("exportConfig")
                    .cloned()
                    .unwrap_or(Value::Null),
                expected_export,
                "case {:?}: the stored export configuration is reported, got {}",
                case.case, read.body
            );
        }
    }
}
