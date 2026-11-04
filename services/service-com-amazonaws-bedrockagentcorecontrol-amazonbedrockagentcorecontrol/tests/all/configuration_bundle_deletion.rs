// SPDX-License-Identifier: CC0-1.0

//! What a DELETE of a configuration bundle does to the store, and how this family
//! words a bundle it cannot resolve, end to end over HTTP.
//!
//! Two rules are under test, each asserted over a SET of bundles, operations and
//! identifiers rather than over the one request a report happened to cite:
//!
//! 1. AN ACCEPTED DELETION MAKES THE BUNDLE STOP EXISTING.
//!    `DeleteConfigurationBundle` "deletes a configuration bundle and all of its
//!    versions" and answers HTTP 202 with `bundleArn`, `bundleId` and `status`
//!    (`smithy_query operation=DeleteConfigurationBundle`), `status` being the
//!    status of the deletion operation — `DELETING`, the only
//!    `ConfigurationBundleStatus` this family ever puts on the wire, since no read
//!    of it carries a status member at all. After that answer the bundle is not
//!    there: every operation of the family that addresses that `bundleId` takes the
//!    not-found path, the name it held is free for a new bundle, and the new
//!    bundle's own version is what a read then reports. The rows differ along the
//!    components, the description, the branch the version was committed on, the
//!    tags and the length of the name.
//! 2. THE FAMILY'S NOT-FOUND SENTENCE IS FIXED TEXT.
//!    `GetConfigurationBundle` and `DeleteConfigurationBundle` both model
//!    `ResourceNotFoundException` (HTTP 404, `smithy_query
//!    operation=GetConfigurationBundle,DeleteConfigurationBundle`), and the
//!    configuration-bundle family words it in one short sentence with NO variable
//!    part: not the `bundleId` asked for, not the branch, not the account. The rows
//!    differ along the operation, the shape and length of the identifier, and
//!    whether what could not be resolved was the bundle or a branch of a bundle
//!    that is stored — the sentence is the same text every time. The sibling
//!    families word this refusal their own way, which
//!    `tests/all/resource_error_message_wording.rs` pins for each of them.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The whole sentence this family words a bundle it cannot resolve in. It carries
/// no variable part at all.
const BUNDLE_NOT_FOUND: &str = "Configuration bundle not found";

/// What `DeleteConfigurationBundleResponse$status` reports for the deletion it
/// accepted, out of `ConfigurationBundleStatus`'s values `ACTIVE | CREATING |
/// CREATE_FAILED | UPDATING | UPDATE_FAILED | DELETING | DELETE_FAILED`
/// (`smithy_query shape=ConfigurationBundleStatus`).
const DELETING: &str = "DELETING";

/// The error shape the family's refusals name, in the body's `__type` and in the
/// `x-amzn-errortype` header.
const NOT_FOUND_SHAPE: &str = "ResourceNotFoundException";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on, and
/// what a handler reads the ARN region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the status, the error shape the `x-amzn-errortype` header names, the
/// `message` the body carries and the body itself.
struct Answer {
    status: u16,
    error_type: String,
    message: String,
    body: Value,
}

impl Answer {
    /// The error shape the BODY names in `__type`, stripped of its namespace.
    fn body_type(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        message: parsed
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        body: parsed,
    }
}

/// `CreateConfigurationBundle`: `POST /configuration-bundles/create`.
async fn create_bundle(router: &MultiServiceRouter, body: &Value) -> Answer {
    send(router, "POST", "/configuration-bundles/create", Some(body)).await
}

/// `GetConfigurationBundle`: `GET /configuration-bundles/{bundleId}`, with the
/// optional `branchName` as an `@httpQuery`.
async fn get_bundle(
    router: &MultiServiceRouter,
    bundle_id: &str,
    branch_name: Option<&str>,
) -> Answer {
    let uri = match branch_name {
        Some(branch) => format!(
            "/configuration-bundles/{bundle_id}?branchName={}",
            branch.replace('/', "%2F")
        ),
        None => format!("/configuration-bundles/{bundle_id}"),
    };
    send(router, "GET", &uri, None).await
}

/// `DeleteConfigurationBundle`: `DELETE /configuration-bundles/{bundleId}`.
async fn delete_bundle(router: &MultiServiceRouter, bundle_id: &str) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/configuration-bundles/{bundle_id}"),
        None,
    )
    .await
}

/// The `ConfigurationBundleArn` form the model publishes:
/// `arn:aws[a-zA-Z-]*:bedrock-agentcore:<region>:<account>:configuration-bundle/<bundleId>`.
fn modelled_arn(bundle_id: &str) -> String {
    format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:configuration-bundle/{bundle_id}")
}

/// Assert `answer` is this family's whole not-found refusal, and that none of
/// `echoed` reached it: the sentence has no variable part, so no identifier, branch
/// or account can appear in it.
fn assert_family_not_found(case: &str, answer: &Answer, echoed: &[&str]) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, NOT_FOUND_SHAPE),
        "case {case:?}: expected HTTP 404 with x-amzn-errortype {NOT_FOUND_SHAPE}, got {} {:?} \
         ({})",
        answer.status,
        answer.error_type,
        answer.body
    );
    assert_eq!(
        answer.body_type(),
        NOT_FOUND_SHAPE,
        "case {case:?}: expected the body to name {NOT_FOUND_SHAPE}, got {:?} ({})",
        answer.body_type(),
        answer.body
    );
    assert_eq!(
        answer.message, BUNDLE_NOT_FOUND,
        "case {case:?}: expected the family's sentence {BUNDLE_NOT_FOUND:?}, got {:?}",
        answer.message
    );
    for value in echoed {
        assert!(
            !answer.message.contains(value),
            "case {case:?}: the sentence must not echo {value:?}, got {:?}",
            answer.message
        );
    }
}

/// One bundle to create, delete and look for afterwards.
struct Row {
    case: &'static str,
    name: String,
    /// The branch the create commits the initial version on, or `None` for the
    /// modelled default (`mainline`).
    branch: Option<&'static str>,
    body: Value,
}

fn rows() -> Vec<Row> {
    // A name at `ConfigurationBundleName`'s ceiling: `^[a-zA-Z][a-zA-Z0-9_]{0,99}$`
    // (`smithy_query operation=CreateConfigurationBundle`), so 100 characters.
    let longest_name = format!("n{}", "a".repeat(99));
    vec![
        Row {
            case: "one component, the default branch, no description",
            name: "plain".to_string(),
            branch: None,
            body: json!({
                "bundleName": "plain",
                "components": {"agent": {"configuration": {"model": "a-model"}}},
            }),
        },
        Row {
            case: "several components carrying nested objects and lists, plus a description",
            name: "rich_bundle".to_string(),
            branch: None,
            body: json!({
                "bundleName": "rich_bundle",
                "description": "every component this bundle versions",
                "components": {
                    "agent": {"configuration": {"model": "a-model", "tools": ["one", "two"]}},
                    "memory": {"configuration": {"strategies": [{"type": "SEMANTIC"}]}},
                    "gateway": {"configuration": {}},
                },
                "tags": {"team": "agentcore", "env": "test"},
            }),
        },
        Row {
            case: "a version committed on a named branch rather than on mainline",
            name: "branched".to_string(),
            branch: Some("release/2024"),
            body: json!({
                "bundleName": "branched",
                "branchName": "release/2024",
                "components": {"agent": {"configuration": {"model": "b-model"}}},
                "commitMessage": "the first commit on the release branch",
            }),
        },
        Row {
            case: "a name at the modelled 100-character ceiling",
            name: longest_name.clone(),
            branch: None,
            body: json!({
                "bundleName": longest_name,
                "components": {"agent": {"configuration": []}},
            }),
        },
    ]
}

/// RULE 1: once the deletion is accepted the bundle stops existing — for the read,
/// for a second delete, and for the name it held.
#[tokio::test]
async fn an_accepted_deletion_makes_the_bundle_stop_existing() {
    let router = router();

    for row in rows() {
        let case = row.case;

        let created = create_bundle(&router, &row.body).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateConfigurationBundle, got {} ({})",
            created.status, created.body
        );
        let bundle_id = created.text("bundleId");
        let version_id = created.text("versionId");

        // The resource is there BEFORE the delete, read back through the
        // implemented reader, so what the delete removes is a bundle that existed.
        let before = get_bundle(&router, &bundle_id, row.branch).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: expected HTTP 200 from GetConfigurationBundle before the delete, got \
             {} ({})",
            before.status, before.body
        );
        assert_eq!(
            before.text("versionId"),
            version_id,
            "case {case:?}: expected the created version {version_id:?} before the delete, got {:?}",
            before.text("versionId")
        );

        let deleted = delete_bundle(&router, &bundle_id).await;
        assert_eq!(
            deleted.status, 202,
            "case {case:?}: expected HTTP 202 from DeleteConfigurationBundle, got {} ({})",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.text("bundleId"),
            bundle_id,
            "case {case:?}: expected the deletion to report bundleId {bundle_id:?}, got {:?}",
            deleted.text("bundleId")
        );
        assert_eq!(
            deleted.text("bundleArn"),
            modelled_arn(&bundle_id),
            "case {case:?}: expected the modelled ConfigurationBundleArn for {bundle_id}, got {:?}",
            deleted.text("bundleArn")
        );
        assert_eq!(
            deleted.text("status"),
            DELETING,
            "case {case:?}: expected the deletion status {DELETING:?}, got {:?}",
            deleted.text("status")
        );

        // The read of the deleted bundle: on the branch it was committed on, on
        // the default branch, and with no branch named at all.
        for (what, branch) in [
            ("no branch named", None),
            ("the branch it was committed on", row.branch),
            ("the default branch", Some("mainline")),
        ] {
            let read = get_bundle(&router, &bundle_id, branch).await;
            assert_family_not_found(
                &format!("{case}: a read after the delete, {what}"),
                &read,
                &[bundle_id.as_str(), ACCOUNT],
            );
        }

        // A second delete has nothing left to accept: the bundle the first one
        // took out is gone, which is the same not-found path a read takes.
        let again = delete_bundle(&router, &bundle_id).await;
        assert_family_not_found(
            &format!("{case}: a second delete of the same bundle"),
            &again,
            &[bundle_id.as_str(), ACCOUNT],
        );

        // The name the deleted bundle held is free again, and the bundle created
        // under it is a NEW resource: its own id, its own version, its own
        // components — the deleted lineage is not resurrected behind the name.
        let recreated = create_bundle(
            &router,
            &json!({
                "bundleName": row.name,
                "components": {"agent": {"configuration": {"model": "after-the-delete"}}},
            }),
        )
        .await;
        assert_eq!(
            recreated.status, 201,
            "case {case:?}: the deleted bundle's name is free again, got {} ({})",
            recreated.status, recreated.body
        );
        let fresh_id = recreated.text("bundleId");
        assert_ne!(
            fresh_id, bundle_id,
            "case {case:?}: expected a bundleId other than the deleted {bundle_id:?}, got \
             {fresh_id:?}"
        );
        let fresh = get_bundle(&router, &fresh_id, None).await;
        assert_eq!(
            fresh.status, 200,
            "case {case:?}: expected HTTP 200 for the bundle created after the delete, got {} ({})",
            fresh.status, fresh.body
        );
        assert_eq!(
            fresh.body["components"],
            json!({"agent": {"configuration": {"model": "after-the-delete"}}}),
            "case {case:?}: expected the new bundle's own components, got {}",
            fresh.body["components"]
        );
        assert_ne!(
            fresh.text("versionId"),
            version_id,
            "case {case:?}: expected a version other than the deleted {version_id:?}, got {:?}",
            fresh.text("versionId")
        );

        // And the deleted id is still not there after the name was re-used.
        let old = get_bundle(&router, &bundle_id, None).await;
        assert_family_not_found(
            &format!("{case}: the deleted id after the name was re-used"),
            &old,
            &[bundle_id.as_str(), fresh_id.as_str(), ACCOUNT],
        );
    }
}

/// RULE 2: the family's not-found sentence is the same fixed text whatever
/// operation asked, whatever identifier it asked for, and whether the bundle or
/// only the branch could not be resolved.
#[tokio::test]
async fn the_configuration_bundle_not_found_sentence_carries_no_variable_part() {
    let router = router();

    // A bundle that IS stored, so the rows that ask for a branch it holds no
    // commit on exercise the "bundle found, nothing to get on that branch" path.
    let created = create_bundle(
        &router,
        &json!({
            "bundleName": "stored",
            "components": {"agent": {"configuration": {"model": "a-model"}}},
        }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "setup: CreateConfigurationBundle must succeed for this test to say anything, got {} ({})",
        created.status, created.body
    );
    let stored_id = created.text("bundleId");

    // Identifiers inside `ConfigurationBundleId`'s pattern
    // `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$` (`smithy_query
    // operation=GetConfigurationBundle`), differing in length and in which of the
    // characters the pattern allows they use.
    let unknown_ids = [
        ("a short id", "absent-abcdefghij"),
        (
            "a long id using the hyphens and underscores the pattern allows",
            "a-much_longer-configuration_bundle-identifier-0123456789",
        ),
        ("an id whose name is stored but whose suffix is not", "stored-0000000000"),
        ("an id that is all digits after its first character", "b1234567890-9876543210"),
    ];

    for (what, identifier) in unknown_ids {
        let read = get_bundle(&router, identifier, None).await;
        assert_family_not_found(
            &format!("GetConfigurationBundle on {what}"),
            &read,
            &[identifier, ACCOUNT],
        );

        let deleted = delete_bundle(&router, identifier).await;
        assert_family_not_found(
            &format!("DeleteConfigurationBundle on {what}: the same sentence, not the verb"),
            &deleted,
            &[identifier, ACCOUNT],
        );
    }

    // Branches inside `BranchName`'s pattern `^[a-zA-Z][a-zA-Z0-9_/-]{0,127}$` (len
    // 1..128), none of which the stored bundle holds a commit on. The last is at
    // the modelled 128-character ceiling.
    let branches = [
        ("a branch nothing committed on", "no-such-branch".to_string()),
        (
            "a path-shaped branch",
            "feature/a-branch/nobody-made".to_string(),
        ),
        (
            "a branch at the modelled 128-character ceiling",
            format!("b{}", "x".repeat(127)),
        ),
    ];

    for (what, branch) in branches {
        let read = get_bundle(&router, &stored_id, Some(&branch)).await;
        assert_family_not_found(
            &format!("GetConfigurationBundle on a stored bundle, {what}"),
            &read,
            &[stored_id.as_str(), branch.as_str(), ACCOUNT],
        );
    }

    // The stored bundle itself is untouched by every refusal above: the reads
    // changed nothing.
    let still_there = get_bundle(&router, &stored_id, None).await;
    assert_eq!(
        still_there.status, 200,
        "the stored bundle is still readable after the refusals, got {} ({})",
        still_there.status, still_there.body
    );
}
