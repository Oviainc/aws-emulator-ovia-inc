// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal, over the wire.
//!
//! THE RULE: a CodeConnections refusal carries the FIXED template of the case it
//! refuses, interpolated only from the request and from the resource the service already
//! holds — never a sentence this emulator authored, and never a value AWS leaves out. The
//! refusals of the Git-sync surface (repository links and sync configurations) carry one
//! thing more: the service framework appends
//! ` (Service: CodeConnections; Status Code: <status>; Error Code: <exception>; Request ID: <id>; Proxy: null)`
//! to the message, whose request id is the only per-run field; the connection surface's
//! refusals are the bare sentence.
//!
//! Each test asserts the rule over a TABLE spanning what its template generalises over —
//! which ARN form resolved to nothing, which id, which owner/repository, which resource
//! name, which region the caller signed for. The case each recorded unit cited is one row
//! of its table.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's helpers, so these tests exercise the same path a signed
//! SDK request takes.

use smithy_server_runtime::MultiServiceRouter;

use super::connection_lifecycle::{REGION, call, call_signed_for, create, router};
use super::git_sync::{SYNC_TYPE, error_code, role_arn};
use super::host_association::create_plain_host;

/// The sentence CodeConnections answers for an ARN that resolves to no CONNECTION,
/// written out as the LITERAL the recorded service returns rather than read off the
/// implementation's own constant, so rewording that constant fails this suite.
const CONNECTION_NOT_FOUND_MESSAGE: &str = "No connection exists with the ARN";

/// The sentence this service answers for an ARN that resolves to no HOST, as its host
/// readers word it: the only kind-specific absence this service is known to name the ARN
/// in. Also a literal, for the reason above.
fn host_not_found_message(host_arn: &str) -> String {
    format!("No host exists with the ARN: {host_arn}")
}

/// The sentence the ARN-addressed tagging surface answers for an ARN of neither kind — a
/// repository-link ARN naming no link, say.
fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource not found. Verify the resource ARN and try again: {resource_arn}")
}

/// The published maximum length of the modeled `ResourceName` (`length 1..100`, from
/// `smithy_query(query="shape", shape="ResourceName")`).
const RESOURCE_NAME_MAX_LEN: usize = 100;

/// The published bounds of the modeled `OwnerId` (1..255) and `RepositoryName` (1..128),
/// from `smithy_query(query="operation", operation="CreateRepositoryLink")`.
const OWNER_ID_MAX_LEN: usize = 255;
const REPOSITORY_NAME_MAX_LEN: usize = 128;

/// The fields of the service-framework tail, in the order the framework writes them.
const TAIL_FIELDS: &[&str] = &["Service", "Status Code", "Error Code", "Request ID", "Proxy"];

/// The emulator's canonical account id — the account every refusal of this service
/// reports the lookup as having been made in.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// The message a refused response carries.
fn refusal_message(case: &str, body: &serde_json::Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message, got {body}"))
        .to_string()
}

/// What one framework tail states, split out of the message it was appended to.
struct FrameworkTail {
    /// The message BEFORE the tail: the template body.
    body: String,
    /// The `Status Code` field, as written.
    status_code: String,
    /// The `Error Code` field: the exception the framework names.
    error_code: String,
    /// The `Request ID` field: the one per-run value in the message.
    request_id: String,
}

/// Assert `id` is the 8-4-4-4-12 lowercase-hex version-4 UUID a request id is.
fn assert_request_id_shaped(case: &str, id: &str) {
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected an 8-4-4-4-12 request id, got {id:?}"
    );
    assert!(
        groups.iter().all(|group| group
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte.is_ascii_hexdigit()))),
        "case {case:?}: expected lowercase hex in every group, got {id:?}"
    );
    assert!(
        groups[2].starts_with('4'),
        "case {case:?}: expected a version-4 request id, got {id:?}"
    );
}

/// Split a refusal message into its template body and the framework tail's fields,
/// asserting the tail has exactly the shape the framework writes it in.
fn split_framework_tail(case: &str, message: &str) -> FrameworkTail {
    let at = message.find(" (Service: ").unwrap_or_else(|| {
        panic!("case {case:?}: expected a framework tail, got {message:?}")
    });
    let (body, tail) = message.split_at(at);
    let inner = tail
        .strip_prefix(" (")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_else(|| panic!("case {case:?}: the tail is not parenthesised: {message:?}"));
    let fields: Vec<&str> = inner.split("; ").collect();
    assert_eq!(
        fields.len(),
        TAIL_FIELDS.len(),
        "case {case:?}: expected {} tail fields, got {fields:?}",
        TAIL_FIELDS.len()
    );
    let values: Vec<String> = fields
        .iter()
        .zip(TAIL_FIELDS)
        .map(|(field, name)| {
            field
                .strip_prefix(&format!("{name}: "))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected the tail field {name:?}, got {field:?}")
                })
                .to_string()
        })
        .collect();
    assert_eq!(
        values[0], "CodeConnections",
        "case {case:?}: the tail must name this service, got {message:?}"
    );
    assert_eq!(
        values[4], "null",
        "case {case:?}: the tail's Proxy field is null, got {message:?}"
    );
    assert_request_id_shaped(case, &values[3]);
    FrameworkTail {
        body: body.to_string(),
        status_code: values[1].clone(),
        error_code: values[2].clone(),
        request_id: values[3].clone(),
    }
}

/// Assert one refusal of the Git-sync surface: the HTTP status, the wire error code, the
/// template body, and the tail's own status and exception name. Returns the tail's
/// request id, so a caller can assert two refusals name two different requests.
fn assert_tailed_refusal(
    case: &str,
    status: u16,
    body: &serde_json::Value,
    expect_status: u16,
    expect_wire_code: &str,
    expect_body: &str,
    expect_tail_code: &str,
) -> String {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        expect_wire_code,
        "case {case:?}: expected {expect_wire_code}, got {body}"
    );
    let message = refusal_message(case, body);
    let tail = split_framework_tail(case, &message);
    assert_eq!(
        tail.body, expect_body,
        "case {case:?}: expected the message body {expect_body:?}, got {:?}",
        tail.body
    );
    assert_eq!(
        tail.status_code,
        expect_status.to_string(),
        "case {case:?}: the tail must state the answered status, got {message:?}"
    );
    assert_eq!(
        tail.error_code, expect_tail_code,
        "case {case:?}: expected the tail exception {expect_tail_code:?}, got {message:?}"
    );
    tail.request_id
}

/// Assert one refusal of the connection surface: the status, the wire code, and the exact
/// fixed sentence — with no framework tail and no echo of the identifier that resolved to
/// nothing.
fn assert_bare_refusal(
    case: &str,
    status: u16,
    body: &serde_json::Value,
    expect_status: u16,
    expect_wire_code: &str,
    expect_message: &str,
    must_not_echo: &str,
) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        expect_wire_code,
        "case {case:?}: expected {expect_wire_code}, got {body}"
    );
    let message = refusal_message(case, body);
    assert_eq!(
        message, expect_message,
        "case {case:?}: expected the fixed message {expect_message:?}, got {message:?}"
    );
    assert!(
        !message.contains(must_not_echo),
        "case {case:?}: the refusal must not echo {must_not_echo:?}, got {message:?}"
    );
    assert!(
        !message.contains("(Service:"),
        "case {case:?}: this refusal carries no framework tail, got {message:?}"
    );
}

/// Create a connection and a repository link on it, returning the link's
/// `(RepositoryLinkId, RepositoryLinkArn)`.
///
/// `git_sync::create_link` hands back the CONNECTION's ARN; the templates asserted here
/// name the LINK's, so this helper reports that instead.
async fn create_link(
    router: &MultiServiceRouter,
    index: usize,
    owner_id: &str,
    repository_name: &str,
) -> (String, String) {
    let connection_arn = create(router, &format!("link-conn-{index}"), "GitHub").await;
    let (status, body) = call(
        router,
        "CreateRepositoryLink",
        serde_json::json!({
            "ConnectionArn": connection_arn,
            "OwnerId": owner_id,
            "RepositoryName": repository_name,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {owner_id:?}/{repository_name:?}: expected 200 from CreateRepositoryLink, \
         got {status} {body}"
    );
    let info = &body["RepositoryLinkInfo"];
    let id = info["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRepositoryLink returned no RepositoryLinkId: {body}"))
        .to_string();
    let arn = info["RepositoryLinkArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRepositoryLink returned no RepositoryLinkArn: {body}"))
        .to_string();
    (id, arn)
}

/// Create a sync configuration for `resource_name` against `repository_link_id`.
async fn create_sync_configuration(
    router: &MultiServiceRouter,
    repository_link_id: &str,
    resource_name: &str,
) {
    let (status, body) = call(
        router,
        "CreateSyncConfiguration",
        serde_json::json!({
            "Branch": "main",
            "ConfigFile": "stack.yaml",
            "RepositoryLinkId": repository_link_id,
            "ResourceName": resource_name,
            "RoleArn": role_arn("git-sync"),
            "SyncType": SYNC_TYPE,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {resource_name:?}: expected 200 from CreateSyncConfiguration, got {status} {body}"
    );
}

/// CAUSE 1 — every connection-addressed refusal states the service's fixed sentence.
///
/// The rows are the ways an ARN the modeled patterns admit can resolve to nothing: an id
/// never minted, a connection that was deleted, a live connection's id in another region,
/// account or partition, the legacy `codestar-connections` namespace, and ARNs of the other
/// resource kinds this service mints. `GetConnection` and `DeleteConnection` address a
/// CONNECTION, so every row is refused with the one fixed sentence, which does not carry
/// the ARN; `ListTagsForResource` is addressed by RESOURCE ARN and resolves a connection, a
/// host or a repository link, so it answers in the wording of the kind the ARN names — the
/// third column of each row.
#[tokio::test]
async fn an_unresolvable_connection_arn_is_refused_with_the_services_fixed_sentence() {
    let router = router();
    let live = create(&router, "live-connection", "GitHub").await;
    let doomed = create(&router, "doomed-connection", "Bitbucket").await;
    let live_host = create_plain_host(&router, "live-host", "GitHubEnterpriseServer").await;
    let (status, body) = call(
        &router,
        "DeleteConnection",
        serde_json::json!({ "ConnectionArn": doomed }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first DeleteConnection must succeed, got {status} {body}"
    );

    let account = account();
    let absent_host = live_host.replace("host/", "host/00000000-1111-2222-3333-4444");
    let absent_link = format!(
        "arn:aws:codeconnections:{REGION}:{account}:repository-link/11111111-2222-3333-4444-555555555555"
    );
    // (case, the ARN, the message `ListTagsForResource` answers for it)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a connection id this store never minted",
            format!(
                "arn:aws:codeconnections:{REGION}:{account}:connection/00000000-1111-2222-3333-444444444444"
            ),
            CONNECTION_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "the ARN of a connection that was deleted",
            doomed.clone(),
            CONNECTION_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a live connection's id in another region",
            live.replace(REGION, "ca-central-1"),
            CONNECTION_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a live connection's id in another account",
            live.replace(account, "314159265358"),
            CONNECTION_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a live connection's id in the aws-us-gov partition",
            live.replace("arn:aws:", "arn:aws-us-gov:"),
            CONNECTION_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a live connection's id in the legacy codestar-connections namespace",
            live.replace("codeconnections", "codestar-connections"),
            CONNECTION_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a host ARN this store holds no host for",
            absent_host.clone(),
            host_not_found_message(&absent_host),
        ),
        (
            "a repository-link ARN this store holds no link for",
            absent_link.clone(),
            resource_not_found_message(&absent_link),
        ),
    ];

    for (label, arn, tagging_message) in &rows {
        let (status, body) = call(
            &router,
            "GetConnection",
            serde_json::json!({ "ConnectionArn": arn }),
        )
        .await;
        assert_bare_refusal(
            &format!("GetConnection: {label}"),
            status,
            &body,
            404,
            "ResourceNotFoundException",
            CONNECTION_NOT_FOUND_MESSAGE,
            arn,
        );

        let (status, body) = call(
            &router,
            "ListTagsForResource",
            serde_json::json!({ "ResourceArn": arn }),
        )
        .await;
        let case = format!("ListTagsForResource: {label}");
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for an ARN that resolves to nothing, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        let message = refusal_message(&case, &body);
        assert_eq!(
            &message, tagging_message,
            "case {case:?}: expected the sentence of the kind the ARN names, got {message:?}"
        );
        assert!(
            !message.contains("(Service:"),
            "case {case:?}: this refusal carries no framework tail, got {message:?}"
        );

        let (status, body) = call(
            &router,
            "DeleteConnection",
            serde_json::json!({ "ConnectionArn": arn }),
        )
        .await;
        assert_bare_refusal(
            &format!("DeleteConnection: {label}"),
            status,
            &body,
            404,
            "ResourceNotFoundException",
            CONNECTION_NOT_FOUND_MESSAGE,
            arn,
        );
    }

    // The rows above are absence, not a blanket refusal: the live connection still reads
    // back and still reports its tags, and so does the live host.
    for operation in ["GetConnection", "ListTagsForResource"] {
        let member = if operation == "GetConnection" {
            "ConnectionArn"
        } else {
            "ResourceArn"
        };
        let (status, body) = call(&router, operation, serde_json::json!({ member: live })).await;
        assert_eq!(
            status, 200,
            "case {operation:?} on the live connection: expected 200, got {status} {body}"
        );
    }
    let (status, body) = call(
        &router,
        "ListTagsForResource",
        serde_json::json!({ "ResourceArn": live_host }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource on the live host: expected 200, got {status} {body}"
    );
}

/// CAUSE 2 — the repository-link surface words a `RepositoryLinkId` that resolves to no
/// link with the service's template AND the framework tail.
///
/// Rows span the id forms the modeled `RepositoryLinkId` pattern admits (lowercase and
/// uppercase hex, the all-zero id) plus the id of a link that was deleted. Both operations
/// that resolve an id — `GetRepositoryLink` and `UpdateRepositoryLink` — answer it the
/// same way, and every answer names a different request.
#[tokio::test]
async fn a_repository_link_id_that_resolves_to_nothing_is_refused_with_the_framework_tail() {
    let router = router();
    let (live_id, _live_arn) = create_link(&router, 0, "acme", "service").await;
    let (deleted_id, _) = create_link(&router, 1, "acme", "retired").await;
    let (status, body) = call(
        &router,
        "DeleteRepositoryLink",
        serde_json::json!({ "RepositoryLinkId": deleted_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting an unreferenced link must succeed, got {status} {body}"
    );

    let rows: Vec<(&str, String)> = vec![
        (
            "a lowercase id this store never minted",
            "00000000-1111-2222-3333-444444444444".to_string(),
        ),
        (
            "an uppercase-hex id the pattern also admits",
            "AABBCCDD-EEFF-0011-2233-445566778899".to_string(),
        ),
        (
            "the all-zero id",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("the id of a link that was deleted", deleted_id.clone()),
    ];

    let mut request_ids: Vec<String> = Vec::new();
    for (label, id) in &rows {
        let expected = format!(
            "Repository not found for accountId: {} and UUID: {id}",
            account()
        );
        for operation in ["GetRepositoryLink", "UpdateRepositoryLink"] {
            let (status, body) = call(
                &router,
                operation,
                serde_json::json!({ "RepositoryLinkId": id }),
            )
            .await;
            request_ids.push(assert_tailed_refusal(
                &format!("{operation}: {label}"),
                status,
                &body,
                404,
                "ResourceNotFoundException",
                &expected,
                "ResourceNotFoundException",
            ));
            assert!(
                body["RepositoryLinkInfo"].is_null(),
                "case {label:?}: a refused {operation} reports no link, got {body}"
            );
        }
    }

    // Each refusal names its OWN request: the tail's request id is the one per-run field.
    let mut distinct = request_ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        request_ids.len(),
        "every refusal names a fresh request id, got {request_ids:?}"
    );

    // And the live link still resolves, so the refusals above are absence.
    let (status, body) = call(
        &router,
        "GetRepositoryLink",
        serde_json::json!({ "RepositoryLinkId": live_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the live link must still resolve, got {status} {body}"
    );
}

/// CAUSE 3 — a `CreateRepositoryLink` for a triple that is already linked is refused
/// with the ARN of the link that holds it, plus the 409 tail.
///
/// Rows differ in the owner and repository the link names — each published bound of
/// `OwnerId` (1..255) and `RepositoryName` (1..128), a nested name, and a provider the
/// recorded unit did not cite — because the template interpolates the EXISTING LINK's ARN
/// and nothing of the request: the same refusal whatever the triple was.
#[tokio::test]
async fn a_duplicate_repository_link_is_refused_with_the_existing_links_arn() {
    let router = router();

    let rows: Vec<(&str, String, String)> = vec![
        (
            "a plain owner and repository",
            "acme".to_string(),
            "service".to_string(),
        ),
        (
            "owner and repository at their published minimum length",
            "o".to_string(),
            "r".to_string(),
        ),
        (
            "owner and repository at their published maximum length",
            "o".repeat(OWNER_ID_MAX_LEN),
            "r".repeat(REPOSITORY_NAME_MAX_LEN),
        ),
        (
            "a nested repository name",
            "contoso".to_string(),
            "infra/platform".to_string(),
        ),
    ];

    for (index, (label, owner_id, repository_name)) in rows.iter().enumerate() {
        let connection_arn = create(&router, &format!("dup-conn-{index}"), "GitHub").await;
        let create_body = serde_json::json!({
            "ConnectionArn": connection_arn,
            "OwnerId": owner_id,
            "RepositoryName": repository_name,
        });
        let (status, first) = call(&router, "CreateRepositoryLink", create_body.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from the first CreateRepositoryLink, got {status} {first}"
        );
        let existing_arn = first["RepositoryLinkInfo"]["RepositoryLinkArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no RepositoryLinkArn: {first}"))
            .to_string();
        let existing_id = first["RepositoryLinkInfo"]["RepositoryLinkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no RepositoryLinkId: {first}"))
            .to_string();

        let (status, body) = call(&router, "CreateRepositoryLink", create_body).await;
        assert_tailed_refusal(
            label,
            status,
            &body,
            409,
            "ResourceAlreadyExistsException",
            &format!("RepoConfiguration already exists. Arn: {existing_arn}"),
            "ResourceAlreadyExistsException",
        );
        assert!(
            body["RepositoryLinkInfo"].is_null(),
            "case {label:?}: a refused create mints nothing, got {body}"
        );

        // The refused create left the existing link exactly as it was.
        let (status, got) = call(
            &router,
            "GetRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": existing_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the existing link must still resolve, got {status} {got}"
        );
        assert_eq!(
            got["RepositoryLinkInfo"]["RepositoryLinkArn"],
            serde_json::json!(existing_arn),
            "case {label:?}: the existing link must be unchanged, got {got}"
        );
    }
}

/// CAUSE 4 — deleting a repository link a sync configuration still references is refused
/// naming the REPOSITORY the link points at, plus the 409 tail (whose exception field is
/// the framework's `ResourceStillExistsException`, not the wire code).
///
/// Rows differ in owner, repository and the resource the configuration syncs, including
/// each published bound of `ResourceName` (1..100).
#[tokio::test]
async fn a_link_a_sync_configuration_references_is_refused_naming_its_repository() {
    let router = router();

    let rows: Vec<(&str, String, String, String)> = vec![
        (
            "a plain owner, repository and stack",
            "acme".to_string(),
            "service".to_string(),
            "OrdersStack".to_string(),
        ),
        (
            "owner, repository and resource name at their published minimum length",
            "o".to_string(),
            "r".to_string(),
            "S".to_string(),
        ),
        (
            "a nested repository name and a resource name at its published maximum length",
            "contoso".to_string(),
            "infra/platform".to_string(),
            "a".repeat(RESOURCE_NAME_MAX_LEN),
        ),
    ];

    for (index, (label, owner_id, repository_name, resource_name)) in rows.iter().enumerate() {
        let (link_id, _) = create_link(&router, index, owner_id, repository_name).await;
        create_sync_configuration(&router, &link_id, resource_name).await;

        let (status, body) = call(
            &router,
            "DeleteRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": link_id }),
        )
        .await;
        assert_tailed_refusal(
            label,
            status,
            &body,
            409,
            "SyncConfigurationStillExistsException",
            &format!(
                "Cfn stack sync configurations still exist for repository \
                 {owner_id}/{repository_name}. Please delete them before retrying."
            ),
            "ResourceStillExistsException",
        );

        // The refusal guarded the link rather than removing it, and once the
        // configuration is gone the same delete succeeds.
        let (status, got) = call(
            &router,
            "GetRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": link_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the refused delete must keep the link, got {status} {got}"
        );
        let (status, removed) = call(
            &router,
            "DeleteSyncConfiguration",
            serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from DeleteSyncConfiguration, got {status} {removed}"
        );
        let (status, deleted) = call(
            &router,
            "DeleteRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": link_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the unreferenced link must delete, got {status} {deleted}"
        );
    }
}

/// The message `GetSyncConfiguration` must answer for a pair holding no configuration:
/// the sync type in prose, the account, and the `configTarget` locator of the resource —
/// `cfn-stack-sync/` plus the CloudFormation stack ARN in the region the caller signed
/// for.
fn expected_missing_configuration_body(resource_name: &str, region: &str) -> String {
    let account = account();
    format!(
        "CloudFormation Stack Sync configuration not found for accountId: {account} and \
         configTarget: cfn-stack-sync/arn:aws:cloudformation:{region}:{account}:stack/{resource_name}"
    )
}

/// CAUSE 5 — `GetSyncConfiguration` for a pair with no configuration is refused with the
/// `configTarget` template and the 404 tail.
///
/// Rows span the resource-name forms the modeled `ResourceName` pattern admits and each
/// published length bound, and the region the caller signs for (the locator's ARN is
/// derived from the request, so it follows that region rather than a constant).
#[tokio::test]
async fn a_pair_with_no_sync_configuration_is_refused_with_its_config_target() {
    let rows: Vec<(&str, String)> = vec![
        ("a plain mixed-case stack name", "OrdersStack".to_string()),
        ("a name at the published minimum length", "Z".to_string()),
        (
            "a name at the published maximum length",
            "q".repeat(RESOURCE_NAME_MAX_LEN),
        ),
        (
            "a name carrying the hyphen the pattern admits",
            "orders-stack-2".to_string(),
        ),
        (
            "a name carrying the underscore the pattern admits",
            "orders_stack_3".to_string(),
        ),
        ("a name of digits and mixed case", "Stack42Beta".to_string()),
    ];

    // The region the caller signs for is part of the locator, so each row is asked in two
    // regions — the suite's own and one no other test here signs for.
    for region in [REGION, "ap-southeast-2"] {
        let router = router();
        for (label, resource_name) in &rows {
            let case = format!("{label} signed for {region}");
            let (status, body) = call_signed_for(
                &router,
                region,
                "GetSyncConfiguration",
                serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
            )
            .await;
            assert_tailed_refusal(
                &case,
                status,
                &body,
                404,
                "ResourceNotFoundException",
                &expected_missing_configuration_body(resource_name, region),
                "ResourceNotFoundException",
            );
            let message = refusal_message(&case, &body);
            assert!(
                !message.contains(SYNC_TYPE),
                "case {case:?}: the sync type is spelled out, not quoted as the wire token, \
                 got {message:?}"
            );
        }
    }
}

/// CAUSE 5, continued — the refusal is what the STORE holds, so it is the same before a
/// configuration exists and after it is deleted, and a repeated delete changes nothing:
/// only the tail's request id differs between the two reads.
#[tokio::test]
async fn deleting_a_sync_configuration_restores_the_same_missing_configuration_refusal() {
    let router = router();
    let rows = ["OrdersStack", "orders_stack_3", "Stack42Beta"];

    for (index, resource_name) in rows.iter().enumerate() {
        let expected = expected_missing_configuration_body(resource_name, REGION);

        // Before: nothing configured.
        let (status, body) = call(
            &router,
            "GetSyncConfiguration",
            serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
        )
        .await;
        let before = assert_tailed_refusal(
            &format!("before configuring {resource_name}"),
            status,
            &body,
            404,
            "ResourceNotFoundException",
            &expected,
            "ResourceNotFoundException",
        );

        // Configured: the pair reports its row.
        let (link_id, _) = create_link(&router, index, &format!("owner-{index}"), "repo").await;
        create_sync_configuration(&router, &link_id, resource_name).await;
        let (status, configured) = call(
            &router,
            "GetSyncConfiguration",
            serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {resource_name:?}: the configured pair must report 200, got {status} {configured}"
        );

        // Deleted twice: the delete is idempotent and the refusal comes back unchanged.
        for attempt in 1..=2 {
            let (status, removed) = call(
                &router,
                "DeleteSyncConfiguration",
                serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {resource_name:?} delete #{attempt}: expected 200, got {status} {removed}"
            );

            let (status, body) = call(
                &router,
                "GetSyncConfiguration",
                serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
            )
            .await;
            let after = assert_tailed_refusal(
                &format!("after delete #{attempt} of {resource_name}"),
                status,
                &body,
                404,
                "ResourceNotFoundException",
                &expected,
                "ResourceNotFoundException",
            );
            assert_ne!(
                after, before,
                "case {resource_name:?} delete #{attempt}: each refusal names its own request, \
                 got {after:?} twice"
            );
        }
    }
}
