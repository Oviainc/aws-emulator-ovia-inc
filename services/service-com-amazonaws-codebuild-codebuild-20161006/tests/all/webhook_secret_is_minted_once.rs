// SPDX-License-Identifier: CC0-1.0

//! Cover for where a webhook's `secret` is reported, driven over the wire the router
//! serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! The rule under test: **the secret of a resource is reported by the call that MINTS it
//! and by no other** — for a webhook, `CreateWebhook` and an `UpdateWebhook` that rotated
//! the secret — so every response that merely REPORTS a project's webhook
//! (`BatchGetProjects`, `UpdateProject`, an `UpdateWebhook` that rotated nothing) carries
//! the rest of the webhook without `secret`, while the store keeps the secret the
//! provider signs its payloads with.
//!
//! The rows span what the rule generalises over: every source type a webhook can belong
//! to (the model's `ServerType` set, `crate::ops::types::SERVER_TYPE_VALUES`, including
//! the one provider whose webhook carries no secret at all), both ways a webhook comes to
//! exist (CodeBuild registering it and the manual creation that hands the caller the pair
//! to register by hand), every response that projects a project, and a read asking for
//! several projects at once.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::SERVER_TYPE_VALUES;
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState,
};
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{call, create_project_body, emulator_on};

/// One router over an engine the test holds, so an assertion can read the store the
/// handlers write.
fn emulator_with_state() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    (engine, router)
}

/// Create a project whose primary source is hosted by `server_type`, and hold an access
/// token for that provider — which is what CodeBuild carries the provider's half of a
/// webhook out with (its own rule, see `tests/all/webhook_access_token.rs`).
async fn create_project_for(router: &MultiServiceRouter, name: &str, server_type: &str) {
    let (status, body) = call(
        router,
        "CreateProject",
        create_project_body(
            name,
            json!({"source": {
                "type": server_type,
                "location": "https://example.com/owner/repository.git",
                "buildspec": "version: 0.2",
            }}),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateProject must be served, got {status} with {body}"
    );
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
        "case {name:?}: ImportSourceCredentials must be served, got {status} with {body}"
    );
}

/// The webhook `BatchGetProjects` reports for `project`.
async fn read_webhook(router: &MultiServiceRouter, project: &str) -> Value {
    let (status, body) = call(router, "BatchGetProjects", json!({"names": [project]})).await;
    assert_eq!(
        status, 200,
        "case {project:?}: BatchGetProjects must be served, got {status} with {body}"
    );
    body["projects"][0]["webhook"].clone()
}

/// The secret the store holds for `project`'s webhook.
fn stored_secret(engine: &Arc<engine::InMemoryEngine>, project: &str) -> Option<String> {
    engine
        .state::<CodeBuildState>()
        .lock()
        .projects
        .get(project)
        .and_then(|stored| stored.project.webhook.as_ref())
        .and_then(|webhook| webhook.secret.clone())
}

/// Assert that `reported` carries no `secret` while holding the members that identify
/// the webhook CodeBuild created, which no later call rewrites.
fn assert_withholds_secret(case: &str, reported: &Value, created: &Value) {
    assert!(
        reported["secret"].is_null(),
        "case {case:?}: a report of the webhook must not carry its secret, got {reported}"
    );
    for member in ["payloadUrl", "status", "manualCreation"] {
        assert_eq!(
            reported[member], created[member],
            "case {case:?}: {member} must be reported as it was persisted, got {} where the \
             minting call reported {}",
            reported[member], created[member]
        );
    }
}

/// Every provider, both ways a webhook is created: the creating call reports the secret
/// it minted (where the provider has one at all), and every later report of the same
/// project withholds it while the store keeps it.
#[tokio::test]
async fn only_the_minting_call_reports_the_webhook_secret() {
    // (the project's source type, whether the caller registers the hook itself)
    let mut rows: Vec<(&str, bool)> = SERVER_TYPE_VALUES
        .iter()
        .map(|server_type| (*server_type, false))
        .collect();
    // `manualCreation` is "only available for GitHub webhooks", and it is the creation
    // that exists precisely to hand the caller `payloadUrl` and `secret`.
    rows.push(("GITHUB", true));

    for (server_type, manual) in rows {
        let case = format!("a {server_type} webhook, manualCreation={manual}");
        let (engine, router) = emulator_with_state();
        let project = format!("whs-{}-{manual}", server_type.to_ascii_lowercase());
        create_project_for(&router, &project, server_type).await;

        let mut request = json!({
            "projectName": project,
            "filterGroups": [[{"type": "EVENT", "pattern": "PUSH"}]],
        });
        if manual {
            request["manualCreation"] = json!(true);
        }
        let (status, body) = call(&router, "CreateWebhook", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateWebhook must be served, got {status} with {body}"
        );
        let created = body["webhook"].clone();

        // AWS documents on `Webhook$secret` that "a Bitbucket webhook does not support
        // secret", so that provider's webhook has none to report anywhere.
        let carries_secret = server_type != state::SECRETLESS_WEBHOOK_SOURCE_TYPE;
        assert_eq!(
            created["secret"].is_string(),
            carries_secret,
            "case {case:?}: the minting call reports the secret it minted, so expected \
             present={carries_secret}, got {:?}",
            created["secret"]
        );
        let minted = stored_secret(&engine, &project);
        assert_eq!(
            minted.as_deref(),
            created["secret"].as_str(),
            "case {case:?}: the store must hold the secret the caller was handed, got {minted:?}"
        );

        // Every response that REPORTS the project withholds it.
        let read = read_webhook(&router, &project).await;
        assert_withholds_secret(&format!("{case}: BatchGetProjects"), &read, &created);
        assert_eq!(
            read["filterGroups"], created["filterGroups"],
            "case {case:?}: the read must report the configuration that was stored, got {read}"
        );
        let (status, body) = call(
            &router,
            "UpdateProject",
            json!({"name": project, "description": "touched"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateProject must be served, got {status} with {body}"
        );
        assert_withholds_secret(
            &format!("{case}: UpdateProject"),
            &body["project"]["webhook"],
            &created,
        );

        // An update that rotates nothing is a report too.
        let (status, body) = call(
            &router,
            "UpdateWebhook",
            json!({
                "projectName": project,
                "filterGroups": [[{"type": "EVENT", "pattern": "PUSH"}]],
                "rotateSecret": false,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateWebhook must be served, got {status} with {body}"
        );
        assert!(
            body["webhook"]["secret"].is_null(),
            "case {case:?}: an update that rotated nothing must not report the secret, got {body}"
        );

        // And none of those reports touched the stored secret.
        assert_eq!(
            stored_secret(&engine, &project),
            minted,
            "case {case:?}: reporting the project must leave the stored secret in force"
        );
    }
}

/// A rotation MINTS a secret, so that one call reports the new value — and the reports
/// that follow it withhold the new one exactly as they withheld the first.
#[tokio::test]
async fn a_rotation_reports_the_secret_it_mints_and_later_reads_do_not() {
    for server_type in SERVER_TYPE_VALUES {
        let case = format!("rotating a {server_type} webhook's secret");
        let (engine, router) = emulator_with_state();
        let project = format!("whr-{}", server_type.to_ascii_lowercase());
        create_project_for(&router, &project, server_type).await;

        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({
                "projectName": project,
                "filterGroups": [[{"type": "EVENT", "pattern": "PUSH"}]],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateWebhook must be served, got {status} with {body}"
        );
        let created = body["webhook"].clone();
        let first = stored_secret(&engine, &project);

        let (status, body) = call(
            &router,
            "UpdateWebhook",
            json!({"projectName": project, "rotateSecret": true}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateWebhook must be served, got {status} with {body}"
        );
        // `rotateSecret` is documented as ignored for the provider whose webhook has no
        // secret, so that row mints nothing and reports nothing.
        let rotates = *server_type != state::SECRETLESS_WEBHOOK_SOURCE_TYPE;
        assert_eq!(
            body["webhook"]["secret"].is_string(),
            rotates,
            "case {case:?}: expected the rotated secret present={rotates}, got {:?}",
            body["webhook"]["secret"]
        );
        let rotated = stored_secret(&engine, &project);
        if rotates {
            assert_eq!(
                rotated.as_deref(),
                body["webhook"]["secret"].as_str(),
                "case {case:?}: the store must hold the rotated secret, got {rotated:?}"
            );
            assert_ne!(
                rotated, first,
                "case {case:?}: a rotation must replace the stored secret, got {rotated:?}"
            );
        }

        let read = read_webhook(&router, &project).await;
        assert_withholds_secret(&format!("{case}: after the rotation"), &read, &created);
        assert_eq!(
            stored_secret(&engine, &project),
            rotated,
            "case {case:?}: the read must leave the rotated secret in force"
        );
    }
}

/// The rule holds per project in one answer: a read asking for several projects at once
/// withholds every one of their secrets, and withholds nothing else.
#[tokio::test]
async fn a_multi_project_read_withholds_every_secret() {
    let (engine, router) = emulator_with_state();
    // (the project, the source type its webhook belongs to) — carried together, so the row
    // never has to be read back out of the project's name.
    let mut projects: Vec<(String, &str)> = Vec::new();
    for server_type in SERVER_TYPE_VALUES {
        let project = format!("whm-{}", server_type.to_ascii_lowercase());
        create_project_for(&router, &project, server_type).await;
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({
                "projectName": project,
                "filterGroups": [[{"type": "EVENT", "pattern": "PUSH"}]],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {project:?}: CreateWebhook must be served, got {status} with {body}"
        );
        projects.push((project, server_type));
    }

    let names: Vec<&str> = projects
        .iter()
        .map(|(project, _)| project.as_str())
        .collect();
    let (status, body) = call(&router, "BatchGetProjects", json!({"names": names})).await;
    assert_eq!(
        status, 200,
        "a multi-project read must be served, got {status} with {body}"
    );
    let reported_projects = body["projects"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        reported_projects.len(),
        projects.len(),
        "expected one entry per requested project, got {body}"
    );
    for ((project, server_type), reported) in projects.iter().zip(&reported_projects) {
        assert!(
            reported["webhook"]["secret"].is_null(),
            "case {project:?}: expected no secret in a multi-project read, got {reported}"
        );
        assert!(
            reported["webhook"]["payloadUrl"].is_string(),
            "case {project:?}: the rest of the webhook must still be reported, got {reported}"
        );
        // Every provider but the one whose webhook has no secret at all keeps its secret
        // in the store, which is where the read withheld it from.
        let stored = stored_secret(&engine, project);
        let carries_secret = *server_type != state::SECRETLESS_WEBHOOK_SOURCE_TYPE;
        assert_eq!(
            stored.is_some(),
            carries_secret,
            "case {project:?}: expected the stored secret present={carries_secret}, got \
             {stored:?}"
        );
    }
}
