// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of HOW this service WORDS the refusal of a `projectArn` that
//! resolves to no data automation project it holds — on the WRITE paths as much as on the
//! read one.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch` with
//! a signed-looking request — so routing, HTTP binding, constraint validation, the
//! handler and response serialization are all exercised, exactly as an AWS SDK
//! `bedrock-data-automation` client would.
//!
//! The rule under test: every operation of this service that resolves a `projectArn`
//! refuses an ARN naming no project in state with `404` `ResourceNotFoundException`
//! carrying ONE FIXED sentence, the same sentence whichever operation was handed the ARN
//! and whatever else the request carried, and taking NOTHING from the request or from
//! state — so the sentence is a constant a client can branch on. `ResourceNotFoundException`
//! models a single `message` member and no code
//! (`smithy_query(operation="UpdateDataAutomationProject")`,
//! `smithy_query(shape="ResourceNotFoundException")`), so that sentence is the only
//! discriminator the refusal has.
//!
//! The rows span what the rule generalises over: the operation (the `@idempotent` PUT
//! write path, the `@readonly` POST read path, the `@idempotent` DELETE, and the listing
//! that resolves the ARN out of a FILTER rather than out of the URI), how the ARN came to
//! be absent (never minted at all, versus minted by this emulator and then deleted — the
//! create/delete/update lifecycle), the ARN's partition, region, account and resource-id
//! length, and, for the write path, which of the request's other members are carried.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at};

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`): `can_handle` resolves the route key from it.
fn authorization() -> String {
    "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/us-east-1/bedrock/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef"
        .to_string()
}

/// The emulator under test: one shared engine, one mounted service, request
/// authentication off so the suite signs nothing for real.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one request and return its status, the raw body text (so a case can assert a
/// value is NOWHERE in the answer, headers aside) and the decoded JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization())
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serializable body"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, text, decoded)
}

/// The one sentence this service answers for a `projectArn` that resolves to no project
/// it holds. FIXED text: the model specifies no prose for `ResourceNotFoundException`, so
/// this is the wording the service itself answers, and it has no variable parts at all.
const PROJECT_ARN_NOT_FOUND: &str = "Project Arn not found.";

/// `/data-automation-projects/{projectArn}/`, the URI `GetDataAutomationProject` (POST),
/// `UpdateDataAutomationProject` (PUT) and `DeleteDataAutomationProject` (DELETE) all
/// bind (`smithy_query(operation="UpdateDataAutomationProject")` and the two beside it).
fn project_uri(arn: &str) -> String {
    format!("/data-automation-projects/{}/", encode_label(arn))
}

/// A blueprint ARN inside the modelled `BlueprintArn` pattern (source:
/// `smithy_query(shape="BlueprintArn")`), so a write row may carry a well-formed
/// `customOutputConfiguration` without breaking a constraint before the lookup runs.
const PUBLIC_BLUEPRINT_ARN: &str =
    "arn:aws:bedrock:us-east-1:000000000000:blueprint/bedrock-data-automation-public-invoice";

/// ARNs that satisfy the modelled `DataAutomationProjectArn` pattern
/// `^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):data-automation-project/[a-zA-Z0-9-]{12,36}$`
/// (source: `smithy_query(operation="UpdateDataAutomationProject")`) and name no project
/// any create ever minted.
///
/// The rows span the pattern: every partition it admits, a short and a long region, two
/// accounts, and the resource id at both published bounds of `{12,36}` as well as in
/// between.
fn never_minted_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "aws partition, resource id at the 12-character lower bound",
            "arn:aws:bedrock:us-east-1:000000000000:data-automation-project/cccccccccccc".to_string(),
        ),
        (
            "aws-cn partition, hyphenated resource id",
            "arn:aws-cn:bedrock:cn-north-1:111111111111:data-automation-project/4d1f-2222-3333-4444"
                .to_string(),
        ),
        (
            "aws-us-gov partition, resource id at the 36-character upper bound",
            format!(
                "arn:aws-us-gov:bedrock:us-gov-west-1:222222222222:data-automation-project/{}",
                "abcdefghijklmnopqrstuvwxyz0123456789"
            ),
        ),
    ]
}

/// The `UpdateDataAutomationProject` request bodies a write row is tried with: the
/// `@required` `standardOutputConfiguration` alone, and then every optional member the
/// operation models (`projectStage`, `projectDescription`, `customOutputConfiguration`,
/// `overrideConfiguration`, `encryptionConfiguration`, source:
/// `smithy_query(operation="UpdateDataAutomationProject")`) carried too — the refusal is
/// about the ARN, so none of them may reach its wording.
fn update_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minimal: only the required standardOutputConfiguration",
            json!({ "standardOutputConfiguration": {} }),
        ),
        (
            "stage DEVELOPMENT, a description and document output",
            json!({
                "projectStage": "DEVELOPMENT",
                "projectDescription": "a description the refusal must not carry",
                "standardOutputConfiguration": {
                    "document": {
                        "outputFormat": {
                            "textFormat": { "types": ["MARKDOWN"] },
                            "additionalFileFormat": { "state": "DISABLED" }
                        }
                    }
                }
            }),
        ),
        (
            "stage LIVE with every optional configuration member carried",
            json!({
                "projectStage": "LIVE",
                "projectDescription": "",
                "standardOutputConfiguration": {
                    "audio": {
                        "extraction": {
                            "category": { "state": "ENABLED", "types": ["TRANSCRIPT"] }
                        }
                    }
                },
                "customOutputConfiguration": {
                    "blueprints": [
                        { "blueprintArn": PUBLIC_BLUEPRINT_ARN, "blueprintStage": "LIVE" }
                    ]
                },
                "overrideConfiguration": {
                    "document": { "splitter": { "state": "ENABLED" } }
                },
                "encryptionConfiguration": {
                    "kmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/abcd-1234",
                    "kmsEncryptionContext": { "tenant": "acme" }
                }
            }),
        ),
    ]
}

/// One request whose `projectArn` resolves to no project this store holds.
struct NotFoundCase {
    case: String,
    method: &'static str,
    uri: String,
    body: Value,
    /// The ARN the request named, which the refusal must not carry.
    named: String,
}

/// Every implemented operation that resolves a `projectArn`, against one ARN that
/// resolves to no project: the write path with each body above, the read path, the
/// delete, and the listing that resolves the ARN out of a filter.
fn cases_for(label: &str, arn: &str) -> Vec<NotFoundCase> {
    let mut rows = Vec::new();
    for (body_label, body) in update_bodies() {
        rows.push(NotFoundCase {
            case: format!("UpdateDataAutomationProject on {label} ({body_label})"),
            method: "PUT",
            uri: project_uri(arn),
            body,
            named: arn.to_string(),
        });
    }
    rows.push(NotFoundCase {
        case: format!("GetDataAutomationProject on {label}"),
        method: "POST",
        uri: project_uri(arn),
        body: json!({}),
        named: arn.to_string(),
    });
    rows.push(NotFoundCase {
        case: format!("GetDataAutomationProject naming a stage too, on {label}"),
        method: "POST",
        uri: project_uri(arn),
        body: json!({ "projectStage": "LIVE" }),
        named: arn.to_string(),
    });
    rows.push(NotFoundCase {
        case: format!("DeleteDataAutomationProject on {label}"),
        method: "DELETE",
        uri: project_uri(arn),
        body: json!({}),
        named: arn.to_string(),
    });
    rows.push(NotFoundCase {
        case: format!("ListBlueprints resolving a projectFilter on {label}"),
        method: "POST",
        uri: "/blueprints/".to_string(),
        body: json!({ "projectFilter": { "projectArn": arn } }),
        named: arn.to_string(),
    });
    rows
}

/// Create a project and delete it again, answering the ARN this emulator minted for it —
/// an ARN that WAS a project's and is one no longer, which is how a client arrives at
/// this refusal in practice.
async fn minted_then_deleted_arn(router: &MultiServiceRouter, name: &str) -> String {
    let (status, _, created) = call(
        router,
        "PUT",
        "/data-automation-projects/",
        &json!({ "projectName": name, "standardOutputConfiguration": {} }),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected CreateDataAutomationProject 201, got {status} with {created:?}"
    );
    let arn = string_at(&created, "projectArn", "minted-then-deleted seed");
    let (status, _, deleted) = call(router, "DELETE", &project_uri(&arn), &json!({})).await;
    assert_eq!(
        status, 200,
        "expected DeleteDataAutomationProject 200, got {status} with {deleted:?}"
    );
    arn
}

/// The resource-id half of a `DataAutomationProjectArn` (what follows
/// `data-automation-project/`), so a case can assert that not even that part of the
/// requested ARN leaks into the refusal.
fn resource_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

/// RULE: a `projectArn` that resolves to no project this store holds is refused `404`
/// `ResourceNotFoundException` with ONE FIXED sentence, whichever operation resolved the
/// ARN, however the ARN came to be absent, and whatever else the request carried — and
/// the sentence takes nothing from the request, so neither the ARN nor its resource id
/// appears in the answer.
#[tokio::test]
async fn a_project_arn_that_resolves_to_nothing_is_refused_with_one_fixed_sentence() {
    let router = emulator();

    // A store that is NOT empty: one live project, so every row below is answered about
    // the ARN it named rather than about an empty store.
    let (status, _, live) = call(
        &router,
        "PUT",
        "/data-automation-projects/",
        &json!({ "projectName": "live-project", "standardOutputConfiguration": {} }),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected the seed CreateDataAutomationProject to be 201, got {status} with {live:?}"
    );

    let deleted = minted_then_deleted_arn(&router, "created-then-deleted").await;
    let mut rows = cases_for("a project this emulator minted and then deleted", &deleted);
    for (label, arn) in never_minted_arns() {
        rows.extend(cases_for(label, &arn));
    }

    for row in rows {
        let NotFoundCase {
            case,
            method,
            uri,
            body,
            named,
        } = row;
        let (status, text, payload) = call(&router, method, &uri, &body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the modelled ResourceNotFoundException status 404, got \
             {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {payload:?}"
        );
        let message = string_at(&payload, "message", &case);
        assert_eq!(
            message, PROJECT_ARN_NOT_FOUND,
            "case {case:?}: expected {PROJECT_ARN_NOT_FOUND:?}, got {message:?}"
        );
        assert!(
            !text.contains(&named),
            "case {case:?}: expected the refusal to carry nothing from the request, but it \
             echoed the ARN {named:?}: {text}"
        );
        let id = resource_id(&named);
        assert!(
            !text.contains(&id),
            "case {case:?}: expected the refusal to carry nothing from the request, but it \
             echoed the ARN's resource id {id:?}: {text}"
        );
    }
}

/// RULE (the same one, stated as the invariant that keeps the paths together): the
/// sentence the WRITE path answers for an unresolvable `projectArn` is byte-identical to
/// the one the read path, the delete and the filtering listing answer for the SAME ARN,
/// and repeating the refused `@idempotent` PUT answers it again unchanged.
#[tokio::test]
async fn the_write_path_words_the_refusal_exactly_as_the_read_path() {
    let router = emulator();
    let deleted = minted_then_deleted_arn(&router, "wording-agreement").await;

    let mut arns = vec![(
        "a project this emulator minted and then deleted".to_string(),
        deleted,
    )];
    for (label, arn) in never_minted_arns() {
        arns.push((label.to_string(), arn));
    }

    for (label, arn) in arns {
        let read = {
            let case = format!("GetDataAutomationProject on {label}");
            let (status, _, payload) = call(&router, "POST", &project_uri(&arn), &json!({})).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} with {payload:?}"
            );
            let message = string_at(&payload, "message", &case);
            // Pinned to the sentence itself as well as compared across paths, so this
            // invariant cannot be satisfied by every path drifting together.
            assert_eq!(
                message, PROJECT_ARN_NOT_FOUND,
                "case {case:?}: expected {PROJECT_ARN_NOT_FOUND:?}, got {message:?}"
            );
            message
        };

        // The write path, twice: an `@idempotent` PUT that was refused is refused with
        // the same sentence on every attempt.
        for attempt in 1..=2 {
            let case = format!("UpdateDataAutomationProject on {label}, attempt {attempt}");
            let (status, _, payload) = call(
                &router,
                "PUT",
                &project_uri(&arn),
                &json!({ "standardOutputConfiguration": {} }),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} with {payload:?}"
            );
            let message = string_at(&payload, "message", &case);
            assert_eq!(
                message, read,
                "case {case:?}: the write path must word the refusal exactly as the read \
                 path ({read:?}), got {message:?}"
            );
        }

        for (op, method, uri, body) in [
            (
                "DeleteDataAutomationProject",
                "DELETE",
                project_uri(&arn),
                json!({}),
            ),
            (
                "ListBlueprints resolving a projectFilter",
                "POST",
                "/blueprints/".to_string(),
                json!({ "projectFilter": { "projectArn": arn } }),
            ),
        ] {
            let case = format!("{op} on {label}");
            let (status, _, payload) = call(&router, method, &uri, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} with {payload:?}"
            );
            let message = string_at(&payload, "message", &case);
            assert_eq!(
                message, read,
                "case {case:?}: every project lookup must word the refusal identically \
                 ({read:?}), got {message:?}"
            );
        }
    }
}
