// SPDX-License-Identifier: CC0-1.0

//! Cover for the access-token precondition on a webhook call, driven over the wire the
//! router serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on
//! awsJson1_1.
//!
//! The rule under test: a webhook call CodeBuild has to carry out against the project's
//! repository provider needs an access token for that provider — on the project's source
//! (`ProjectSource$auth`) or imported for that provider's `ServerType` — and with none
//! CodeBuild refuses the call with 400 `ResourceNotFoundException`, worded `Access token
//! not found in CodeBuild project for server type <the project's source type,
//! lowercased>`, without writing anything. The one webhook creation that registers
//! nothing with the provider — the manual one, where the caller adds the hook with its
//! own credentials — needs no token, and neither does a webhook on a project whose
//! source has no provider to authenticate to at all.
//!
//! Each table below spans the catalogue the rule generalises over — every `ServerType`
//! the model declares, every `SourceAuthType`, every webhook-mutating operation, and
//! filter-group sets differing in count, filter type and characters — rather than one
//! recorded request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{call, create_project_body, emulator, error_type};

/// Every `ServerType` the model declares, from
/// `smithy_query(query="shape", shape="ServerType")`: the repository providers CodeBuild
/// authenticates to with an access token, and exactly the source types a
/// repository-event webhook can belong to.
const SERVER_TYPES: &[&str] = &[
    "GITHUB",
    "BITBUCKET",
    "GITHUB_ENTERPRISE",
    "GITLAB",
    "GITLAB_SELF_MANAGED",
];

/// Every `SourceAuthType` the model declares, from
/// `smithy_query(query="shape", shape="SourceAuthType")`, with the `resource` each one
/// carries ("the resource value that applies to the specified authorization type").
const SOURCE_AUTH_TYPES: &[(&str, &str)] = &[
    ("OAUTH", "oauth-connected"),
    (
        "CODECONNECTIONS",
        "arn:aws:codeconnections:us-west-2:000000000000:connection/11111111-2222-3333-4444-555555555555",
    ),
    (
        "SECRETS_MANAGER",
        "arn:aws:secretsmanager:us-west-2:000000000000:secret:codebuild-token-AbCdEf",
    ),
];

/// The message CodeBuild states when it holds no access token for `server_type`: fixed
/// text whose only variable part is the project's stored source type, rendered the way a
/// source credential's ARN renders it.
fn expected_message(server_type: &str) -> String {
    format!(
        "Access token not found in CodeBuild project for server type {}",
        server_type.to_ascii_lowercase()
    )
}

/// The `message` an error body carries.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Two filter-group sets for row `row`: the one a webhook is created with, and the one an
/// update tries to replace it by. The rows differ in group count, in the filter types
/// they pair and in the characters their patterns carry, since the rule is about the call
/// being refused whatever configuration it would have applied. Every group carries an
/// `EVENT` filter, which `filterGroups` requires.
fn filter_group_pair(row: usize) -> (Value, Value) {
    let created = [
        json!([[{"type": "EVENT", "pattern": "PUSH"}]]),
        json!([[
            {"type": "EVENT", "pattern": "PULL_REQUEST_CREATED"},
            {"type": "HEAD_REF", "pattern": "^refs/heads/release-.*$"},
        ]]),
        json!([[{"type": "EVENT", "pattern": "PUSH"}], [{"type": "EVENT", "pattern": "PULL_REQUEST_MERGED"}]]),
    ];
    let replacement = [
        json!([[
            {"type": "EVENT", "pattern": "PUSH,PULL_REQUEST_CREATED"},
            {"type": "HEAD_REF", "pattern": "^refs/heads/機能-🚀-résumé-.*$", "excludeMatchedPattern": true},
        ]]),
        json!([[{"type": "EVENT", "pattern": "WORKFLOW_JOB_QUEUED"}]]),
        json!([[
            {"type": "EVENT", "pattern": "PUSH"},
            {"type": "ACTOR_ACCOUNT_ID", "pattern": "000000000000"},
        ]]),
    ];
    (
        created[row % created.len()].clone(),
        replacement[row % replacement.len()].clone(),
    )
}

/// Create a project named `name` whose primary source is `source`, asserting it
/// succeeded.
async fn create_project_with_source(router: &MultiServiceRouter, name: &str, source: Value) {
    let (status, body) = call(
        router,
        "CreateProject",
        create_project_body(name, json!({"source": source})),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateProject {name:?}: expected 200, got {status} with {body}"
    );
}

/// A source for a project whose repository is hosted by `server_type`.
fn provider_source(server_type: &str) -> Value {
    json!({
        "type": server_type,
        "location": "https://example.com/owner/repository.git",
        "buildspec": "version: 0.2",
    })
}

/// Import an access token for `server_type` and hand back the ARN it is held under.
async fn import_credential(router: &MultiServiceRouter, server_type: &str) -> String {
    let (status, body) = call(
        router,
        "ImportSourceCredentials",
        json!({
            "serverType": server_type,
            "authType": "PERSONAL_ACCESS_TOKEN",
            "token": "an-access-token",
            "shouldOverwrite": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ImportSourceCredentials {server_type}: expected 200, got {status} with {body}"
    );
    body["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Give back the access token held under `arn`, so the account holds none for its server
/// type again.
async fn delete_credential(router: &MultiServiceRouter, arn: &str) {
    let (status, body) = call(router, "DeleteSourceCredentials", json!({"arn": arn})).await;
    assert_eq!(
        status, 200,
        "DeleteSourceCredentials {arn:?}: expected 200, got {status} with {body}"
    );
}

/// The webhook a later read reports for `project`, through the implemented reader
/// `BatchGetProjects`.
async fn stored_webhook(router: &MultiServiceRouter, project: &str) -> Value {
    let (status, body) = call(router, "BatchGetProjects", json!({"names": [project]})).await;
    assert_eq!(
        status, 200,
        "BatchGetProjects {project:?}: expected 200, got {status} with {body}"
    );
    body["projects"][0]["webhook"].clone()
}

/// Assert `body` is the refusal CodeBuild answers a tokenless webhook call with.
fn assert_access_token_refusal(case: &str, status: u16, body: &Value, server_type: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(body).contains("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        error_type(body)
    );
    assert_eq!(
        message(body),
        expected_message(server_type),
        "case {case:?}: expected CodeBuild's access-token sentence, got {:?}",
        message(body)
    );
}

/// Every provider CodeBuild authenticates to: an update of a webhook the project already
/// has is refused while the account holds no token for that provider, states CodeBuild's
/// sentence for it, and leaves the stored webhook exactly as it was — and the very same
/// call is served once a token is held again.
#[tokio::test]
async fn an_update_without_an_access_token_is_refused_for_every_server_type() {
    for (row, server_type) in SERVER_TYPES.iter().enumerate() {
        let case = format!("UpdateWebhook on a {server_type} project holding no token");
        let router = emulator();
        let project = format!("uwt-{}", server_type.to_ascii_lowercase());
        create_project_with_source(&router, &project, provider_source(server_type)).await;

        // The webhook is created while a token IS held and the token is then given back,
        // so the refusal below is decided by the state the update reads rather than by
        // anything about how the webhook came to exist.
        let arn = import_credential(&router, server_type).await;
        let (created_groups, replacement) = filter_group_pair(row);
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "filterGroups": created_groups}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateWebhook with a token held must be served, got {status} \
             with {body}"
        );
        delete_credential(&router, &arn).await;

        let (status, body) = call(
            &router,
            "UpdateWebhook",
            json!({"projectName": project, "filterGroups": replacement}),
        )
        .await;
        assert_access_token_refusal(&case, status, &body, server_type);

        // The refusal has no side effect: the stored webhook keeps the configuration it
        // was created with, so a later read never reports the refused call's groups.
        let stored = stored_webhook(&router, &project).await;
        assert_eq!(
            stored["filterGroups"], created_groups,
            "case {case:?}: a refused UpdateWebhook must change nothing, got {stored}"
        );

        // With a token held again the same call is served, and the read reports what it
        // applied.
        import_credential(&router, server_type).await;
        let (status, body) = call(
            &router,
            "UpdateWebhook",
            json!({"projectName": project, "filterGroups": replacement}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: with a token held the update must be served, got {status} \
             with {body}"
        );
        assert_eq!(
            body["webhook"]["filterGroups"], replacement,
            "case {case:?}: expected the groups the update carried, got {body}"
        );
        let stored = stored_webhook(&router, &project).await;
        assert_eq!(
            stored["filterGroups"], replacement,
            "case {case:?}: the read must report the applied groups, got {stored}"
        );
    }
}

/// A token is held per provider, so one imported for another provider authenticates
/// nothing here: the refusal still names the project's OWN source type.
#[tokio::test]
async fn a_token_for_another_provider_does_not_serve_this_projects_webhook() {
    // (the project's source type, the one server type a token is held for)
    let rows: [(&str, &str); 3] = [
        ("GITHUB", "BITBUCKET"),
        ("GITLAB", "GITHUB"),
        ("BITBUCKET", "GITLAB_SELF_MANAGED"),
    ];

    for (source_type, imported) in rows {
        let case = format!("a {source_type} project with only a {imported} token");
        let router = emulator();
        let project = format!("uwx-{}", source_type.to_ascii_lowercase());
        import_credential(&router, imported).await;
        create_project_with_source(&router, &project, provider_source(source_type)).await;

        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "branchFilter": "^refs/heads/main$"}),
        )
        .await;
        assert_access_token_refusal(&format!("{case}: CreateWebhook"), status, &body, source_type);
        assert!(
            stored_webhook(&router, &project).await.is_null(),
            "case {case:?}: a refused CreateWebhook must persist nothing"
        );

        // The project's own provider's token is what serves the call.
        import_credential(&router, source_type).await;
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "branchFilter": "^refs/heads/main$"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 once the project's provider has a token, got \
             {status} with {body}"
        );
    }
}

/// Creating a webhook is CodeBuild registering a hook with the provider, so it needs a
/// token for every provider — except where it registers nothing: the manual creation AWS
/// documents as handing the caller `payloadUrl` and `secret` to add the hook itself, and
/// a runner webhook on a project with no repository provider at all.
#[tokio::test]
async fn creating_a_webhook_needs_a_token_unless_nothing_is_registered() {
    for server_type in SERVER_TYPES {
        let case = format!("CreateWebhook on a {server_type} project holding no token");
        let router = emulator();
        let project = format!("cwt-{}", server_type.to_ascii_lowercase());
        create_project_with_source(&router, &project, provider_source(server_type)).await;

        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "filterGroups": [[{"type": "EVENT", "pattern": "PUSH"}]]}),
        )
        .await;
        assert_access_token_refusal(&case, status, &body, server_type);
        assert!(
            stored_webhook(&router, &project).await.is_null(),
            "case {case:?}: a refused CreateWebhook must persist nothing"
        );
    }

    // `manualCreation` is "only available for GitHub webhooks", and AWS documents it as
    // "CodeBuild doesn't create a webhook in GitHub and instead returns payloadUrl and
    // secret values for the webhook": nothing is registered with the provider, so no
    // token is needed.
    let router = emulator();
    create_project_with_source(&router, "cwt-manual", provider_source("GITHUB")).await;
    let (status, body) = call(
        &router,
        "CreateWebhook",
        json!({"projectName": "cwt-manual", "manualCreation": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a manually created webhook needs no token: expected 200, got {status} with {body}"
    );
    assert!(
        body["webhook"]["payloadUrl"].is_string() && body["webhook"]["secret"].is_string(),
        "a manually created webhook reports the pair the caller registers by hand, got {body}"
    );

    // A runner webhook's project has no repository provider ("NO_SOURCE: the project
    // does not have input source code"), so there is nothing to authenticate to.
    create_project_with_source(
        &router,
        "cwt-runner",
        json!({"type": "NO_SOURCE", "buildspec": "version: 0.2"}),
    )
    .await;
    let (status, body) = call(
        &router,
        "CreateWebhook",
        json!({"projectName": "cwt-runner", "buildType": "RUNNER_BUILDKITE_BUILD"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a runner webhook needs no token: expected 200, got {status} with {body}"
    );
}

/// Removing a webhook is CodeBuild removing the provider's hook, so it needs a token for
/// every provider — except for a webhook the caller registered itself, which CodeBuild
/// never created upstream: that one can always be disconnected, which is the delete half
/// of the delete-and-recreate path a manually created webhook is changed by.
#[tokio::test]
async fn deleting_a_webhook_needs_a_token_unless_the_caller_registered_it() {
    for (row, server_type) in SERVER_TYPES.iter().enumerate() {
        let case = format!("DeleteWebhook on a {server_type} project holding no token");
        let router = emulator();
        let project = format!("dwt-{}", server_type.to_ascii_lowercase());
        create_project_with_source(&router, &project, provider_source(server_type)).await;

        // CodeBuild's own registration: created while a token was held, and the token
        // then given back.
        let arn = import_credential(&router, server_type).await;
        let (created_groups, _) = filter_group_pair(row);
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "filterGroups": created_groups}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateWebhook with a token held must be served, got {status} \
             with {body}"
        );
        delete_credential(&router, &arn).await;

        let (status, body) = call(&router, "DeleteWebhook", json!({"projectName": project})).await;
        assert_access_token_refusal(&case, status, &body, server_type);
        let stored = stored_webhook(&router, &project).await;
        assert_eq!(
            stored["filterGroups"], created_groups,
            "case {case:?}: a refused DeleteWebhook must remove nothing, got {stored}"
        );

        // With a token held again the same call is served and the webhook is gone.
        import_credential(&router, server_type).await;
        let (status, body) = call(&router, "DeleteWebhook", json!({"projectName": project})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: with a token held the delete must be served, got {status} \
             with {body}"
        );
        assert!(
            stored_webhook(&router, &project).await.is_null(),
            "case {case:?}: the deleted webhook must not be reported any more"
        );
    }

    // A manually created webhook: no token at any point, and the delete-and-recreate
    // path a caller falls back to still works end to end.
    let router = emulator();
    create_project_with_source(&router, "dwt-manual", provider_source("GITHUB")).await;
    let (created_groups, replacement) = filter_group_pair(0);
    let (status, body) = call(
        &router,
        "CreateWebhook",
        json!({
            "projectName": "dwt-manual",
            "manualCreation": true,
            "filterGroups": created_groups,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a manually created webhook needs no token: expected 200, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "DeleteWebhook",
        json!({"projectName": "dwt-manual"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting a manually created webhook needs no token: expected 200, got {status} \
         with {body}"
    );
    assert!(
        stored_webhook(&router, "dwt-manual").await.is_null(),
        "the deleted webhook must not be reported any more"
    );
    let (status, body) = call(
        &router,
        "CreateWebhook",
        json!({
            "projectName": "dwt-manual",
            "manualCreation": true,
            "filterGroups": replacement,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "re-creating a manually created webhook: expected 200, got {status} with {body}"
    );
    let stored = stored_webhook(&router, "dwt-manual").await;
    assert_eq!(
        stored["filterGroups"], replacement,
        "the re-created webhook must carry the groups it was created with, got {stored}"
    );
}

/// A token also reaches a project on its source itself (`ProjectSource$auth`), so a
/// project carrying one is served with nothing imported — for every `SourceAuthType` and
/// every provider.
#[tokio::test]
async fn a_source_that_carries_its_own_auth_needs_no_imported_credential() {
    for (row, (auth_type, resource)) in SOURCE_AUTH_TYPES.iter().enumerate() {
        for server_type in SERVER_TYPES {
            let case = format!("a {server_type} project whose source auth is {auth_type}");
            let router = emulator();
            let project = format!(
                "cwa-{}-{}",
                auth_type.to_ascii_lowercase(),
                server_type.to_ascii_lowercase()
            );
            let mut source = provider_source(server_type);
            source["auth"] = json!({"type": auth_type, "resource": resource});
            create_project_with_source(&router, &project, source).await;

            let (created_groups, replacement) = filter_group_pair(row);
            let (status, body) = call(
                &router,
                "CreateWebhook",
                json!({"projectName": project, "filterGroups": created_groups}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateWebhook to be served, got {status} with {body}"
            );

            let (status, body) = call(
                &router,
                "UpdateWebhook",
                json!({"projectName": project, "filterGroups": replacement}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateWebhook to be served, got {status} with {body}"
            );
            let stored = stored_webhook(&router, &project).await;
            assert_eq!(
                stored["filterGroups"], replacement,
                "case {case:?}: the read must report the applied groups, got {stored}"
            );
        }
    }
}
