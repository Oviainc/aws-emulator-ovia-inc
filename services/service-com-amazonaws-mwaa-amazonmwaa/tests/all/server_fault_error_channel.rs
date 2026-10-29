// SPDX-License-Identifier: CC0-1.0

//! What this service answers when the EMULATOR ITSELF fails, for every operation it
//! implements.
//!
//! RULE: an operation that HAS an implementation answers a fault on this side of the wire
//! with the server error this service models for that class — `InternalServerException`,
//! HTTP 500 — and never with the 501 `NotImplemented` scaffold signal, whose claim is that
//! nobody has written the operation yet. The two are different claims about the emulator,
//! and a client's error dispatch (and a human triaging a fixture) branches on the wire
//! code it is given.
//!
//! The fault driven here is the one every handler of this service can meet: the shared
//! store is reachable only on the engine this service is mounted on, so an operation
//! constructed over an engine that carries no store for it meets the fault on its first
//! read. The second fault `CreateCliToken` guards — the credential store minting no secret
//! — has no seam a test can drive and is answered by the same channel in the handler.
//!
//! The rows differ along what the rule generalises over: the OPERATION. All twelve this
//! crate implements are asserted — the control plane and the `env.` data endpoint, readers
//! and writers, the tagging operations and the two token minters — so the answer is pinned
//! for the whole class rather than for the one handler the defect was found in.

use std::collections::HashMap;
use std::sync::Arc;

use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError,
};

use service_com_amazonaws_mwaa_amazonmwaa as mwaa;

use mwaa::ops::create_cli_token::CreateCliToken;
use mwaa::ops::create_environment::CreateEnvironment;
use mwaa::ops::create_web_login_token::CreateWebLoginToken;
use mwaa::ops::delete_environment::DeleteEnvironment;
use mwaa::ops::get_environment::GetEnvironment;
use mwaa::ops::invoke_rest_api::InvokeRestApi;
use mwaa::ops::list_environments::ListEnvironments;
use mwaa::ops::list_tags_for_resource::ListTagsForResource;
use mwaa::ops::publish_metrics::PublishMetrics;
use mwaa::ops::tag_resource::TagResource;
use mwaa::ops::types::{
    CreateCliTokenInput, CreateEnvironmentInput, CreateWebLoginTokenInput, DeleteEnvironmentInput,
    GetEnvironmentInput, InvokeRestApiInput, ListEnvironmentsInput, ListTagsForResourceInput,
    NetworkConfiguration, PublishMetricsInput, TagResourceInput, UntagResourceInput,
    UpdateEnvironmentInput,
};
use mwaa::ops::untag_resource::UntagResource;
use mwaa::ops::update_environment::UpdateEnvironment;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The environment the fault rows name. Nothing is stored for it: the fault is met before
/// any name is resolved, which is the point — a fault of the emulator is not a lookup.
const ENVIRONMENT: &str = "FaultEnv";

/// The status and wire error code a modelled server fault of this service carries, as its
/// model declares them (`smithy_query(query="shape",shape="InternalServerException")`:
/// `errorKind` server, `httpError` 500).
const SERVER_FAULT: (u16, &str) = (500, "InternalServerException");

/// The scaffold signal's status and wire code — what an operation with NO implementation
/// answers, and what none of the rows below may answer.
const SCAFFOLD_SIGNAL: (u16, &str) = (501, "NotImplemented");

/// A SigV4 `Authorization` header whose credential scope names the Region and this
/// service's signing name, which is where a request names the Region it addresses. Nothing
/// here signs; the signature value is never checked.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/airflow/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// The request metadata of one call signed for [`REGION`].
fn parts() -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri(format!("/clitoken/{ENVIRONMENT}"))
        .header("host", format!("env.airflow.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(())
        .expect("the harness request is well formed")
        .into_parts();
    parts
}

/// An engine that is NOT the emulator's shared `InMemoryEngine`, so
/// `mwaa::state::service_state` resolves no store on it: this is the seam that reaches the
/// fault branch each handler guards its first read with. The router only ever builds the
/// engine that does carry the store, so no served request can be in this position.
#[derive(Debug)]
struct StorelessEngine;

/// One operation per call over an engine carrying no store, with the request context every
/// typed call reuses.
struct Faulted {
    engine: Arc<StorelessEngine>,
    definition: mwaa::ServiceDefinition<StorelessEngine>,
    callback: EmbeddedEmulator,
    parts: http::request::Parts,
}

impl Faulted {
    fn new() -> Self {
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        Self {
            engine: Arc::new(StorelessEngine),
            definition: mwaa::ServiceDefinition::new(Arc::new(StorelessEngine)),
            callback: EmbeddedEmulator::from_router(router),
            parts: parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.definition.identity(),
            self.callback.clone(),
        )
    }

    fn engine(&self) -> Arc<StorelessEngine> {
        Arc::clone(&self.engine)
    }
}

/// One emulator over the engine that DOES carry this service's store, for the rows that
/// assert the fix left the served path alone.
struct Served {
    engine: Arc<engine::InMemoryEngine>,
    definition: mwaa::ServiceDefinition<engine::InMemoryEngine>,
    callback: EmbeddedEmulator,
    parts: http::request::Parts,
}

impl Served {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        Self {
            definition: mwaa::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            callback: EmbeddedEmulator::from_router(router),
            parts: parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.definition.identity(),
            self.callback.clone(),
        )
    }

    fn engine(&self) -> Arc<engine::InMemoryEngine> {
        Arc::clone(&self.engine)
    }
}

/// The status and wire error code an operation refused `case` with — the two values a
/// client resolves an error by.
fn refusal<T, E: OperationError>(case: &str, result: Result<T, E>) -> (u16, &'static str) {
    match result {
        Ok(_) => panic!("case {case:?}: expected the fault to be refused, got a success"),
        Err(error) => (error.http_status(), error.shape_name()),
    }
}

/// A create request every member of which satisfies its modelled constraints and the rules
/// the model cannot state, so the handler reaches its store read rather than refusing the
/// request first.
fn valid_create(name: &str) -> CreateEnvironmentInput {
    CreateEnvironmentInput {
        name: name.to_string(),
        execution_role_arn: format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
        source_bucket_arn: "arn:aws:s3:::my-airflow-bucket".to_string(),
        dag_s3_path: "dags".to_string(),
        network_configuration: NetworkConfiguration {
            subnet_ids: Some(vec![
                "subnet-00000000000000001".to_string(),
                "subnet-00000000000000002".to_string(),
            ]),
            security_group_ids: Some(vec!["sg-00000000000000001".to_string()]),
        },
        ..CreateEnvironmentInput::default()
    }
}

/// The ARN of `name`, the form the tagging operations address an environment by.
fn environment_arn(name: &str) -> String {
    format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/{name}")
}

/// The host of a web server URL: everything between the scheme and the first path
/// separator, which is what `WebServerHostname` reports.
fn host_of(url: &str) -> &str {
    let without_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    without_scheme
        .split_once('/')
        .map_or(without_scheme, |(host, _)| host)
}

/// RULE: every implemented operation answers a fault of the emulator with the modelled
/// server error (HTTP 500, wire code `InternalServerException`), never with the 501
/// scaffold signal that claims the operation is unwritten.
#[tokio::test]
async fn every_implemented_operation_answers_an_emulator_fault_as_a_modelled_server_error() {
    let harness = Faulted::new();
    let ctx = harness.ctx();

    let answers = vec![
        (
            "CreateCliToken",
            refusal(
                "CreateCliToken",
                CreateCliToken::new(harness.engine())
                    .handle(
                        CreateCliTokenInput {
                            name: ENVIRONMENT.to_string(),
                        },
                        &ctx,
                    )
                    .await,
            ),
        ),
        (
            "CreateWebLoginToken",
            refusal(
                "CreateWebLoginToken",
                CreateWebLoginToken::new(harness.engine())
                    .handle(
                        CreateWebLoginTokenInput {
                            name: ENVIRONMENT.to_string(),
                        },
                        &ctx,
                    )
                    .await,
            ),
        ),
        (
            "CreateEnvironment",
            refusal(
                "CreateEnvironment",
                CreateEnvironment::new(harness.engine())
                    .handle(valid_create(ENVIRONMENT), &ctx)
                    .await,
            ),
        ),
        (
            "GetEnvironment",
            refusal(
                "GetEnvironment",
                GetEnvironment::new(harness.engine())
                    .handle(
                        GetEnvironmentInput {
                            name: ENVIRONMENT.to_string(),
                        },
                        &ctx,
                    )
                    .await,
            ),
        ),
        (
            "ListEnvironments",
            refusal(
                "ListEnvironments",
                ListEnvironments::new(harness.engine())
                    .handle(ListEnvironmentsInput::default(), &ctx)
                    .await,
            ),
        ),
        (
            "UpdateEnvironment",
            refusal(
                "UpdateEnvironment",
                UpdateEnvironment::new(harness.engine())
                    .handle(
                        UpdateEnvironmentInput {
                            name: ENVIRONMENT.to_string(),
                            environment_class: Some("mw1.small".to_string()),
                            ..UpdateEnvironmentInput::default()
                        },
                        &ctx,
                    )
                    .await,
            ),
        ),
        (
            "DeleteEnvironment",
            refusal(
                "DeleteEnvironment",
                DeleteEnvironment::new(harness.engine())
                    .handle(
                        DeleteEnvironmentInput {
                            name: ENVIRONMENT.to_string(),
                        },
                        &ctx,
                    )
                    .await,
            ),
        ),
        (
            "ListTagsForResource",
            refusal(
                "ListTagsForResource",
                ListTagsForResource::new(harness.engine())
                    .handle(
                        ListTagsForResourceInput {
                            resource_arn: environment_arn(ENVIRONMENT),
                        },
                        &ctx,
                    )
                    .await,
            ),
        ),
        (
            "TagResource",
            refusal(
                "TagResource",
                TagResource::new(harness.engine())
                    .handle(
                        TagResourceInput {
                            resource_arn: environment_arn(ENVIRONMENT),
                            tags: HashMap::from([("team".to_string(), "airflow".to_string())]),
                        },
                        &ctx,
                    )
                    .await,
            ),
        ),
        (
            "UntagResource",
            refusal(
                "UntagResource",
                UntagResource::new(harness.engine())
                    .handle(
                        UntagResourceInput {
                            resource_arn: environment_arn(ENVIRONMENT),
                            tag_keys: vec!["team".to_string()],
                        },
                        &ctx,
                    )
                    .await,
            ),
        ),
        (
            "PublishMetrics",
            refusal(
                "PublishMetrics",
                PublishMetrics::new(harness.engine())
                    .handle(
                        PublishMetricsInput {
                            environment_name: ENVIRONMENT.to_string(),
                            metric_data: Vec::new(),
                        },
                        &ctx,
                    )
                    .await,
            ),
        ),
        (
            "InvokeRestApi",
            refusal(
                "InvokeRestApi",
                InvokeRestApi::new(harness.engine())
                    .handle(
                        InvokeRestApiInput {
                            name: ENVIRONMENT.to_string(),
                            path: "/dags".to_string(),
                            method: "GET".to_string(),
                            ..InvokeRestApiInput::default()
                        },
                        &ctx,
                    )
                    .await,
            ),
        ),
    ];

    for (case, answer) in answers {
        assert_eq!(
            answer, SERVER_FAULT,
            "case {case:?}: expected a fault of the emulator to be reported as \
             {SERVER_FAULT:?}, got {answer:?}"
        );
        assert_ne!(
            answer, SCAFFOLD_SIGNAL,
            "case {case:?}: the scaffold signal claims the operation is unwritten, which this \
             implemented operation is not"
        );
    }
}

/// RULE (the same rule from the other side): the fault channel is only the fault channel —
/// over the engine that carries the store, both token operations still mint a token and
/// report the host of the environment's OWN stored web server URL, read back through
/// `GetEnvironment`.
#[tokio::test]
async fn the_token_operations_still_mint_over_the_engine_that_carries_the_store() {
    let harness = Served::new();
    let ctx = harness.ctx();

    // Rows differ in the environment, which is what the reported host is computed from.
    for case in ["Tokens", "A_second_environment-2"] {
        CreateEnvironment::new(harness.engine())
            .handle(valid_create(case), &ctx)
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the fixture CreateEnvironment must succeed, got {error}")
            });

        let read_back = GetEnvironment::new(harness.engine())
            .handle(
                GetEnvironmentInput {
                    name: case.to_string(),
                },
                &ctx,
            )
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: GetEnvironment must answer the stored environment, got {error}")
            });
        let webserver_url = read_back
            .environment
            .and_then(|environment| environment.webserver_url)
            .unwrap_or_else(|| panic!("case {case:?}: the stored environment carries no WebserverUrl"));
        let expected_host = host_of(&webserver_url).to_string();

        let cli = CreateCliToken::new(harness.engine())
            .handle(
                CreateCliTokenInput {
                    name: case.to_string(),
                },
                &ctx,
            )
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: CreateCliToken must mint a token, got {error}")
            });
        let web = CreateWebLoginToken::new(harness.engine())
            .handle(
                CreateWebLoginTokenInput {
                    name: case.to_string(),
                },
                &ctx,
            )
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: CreateWebLoginToken must mint a token, got {error}")
            });

        assert_eq!(
            cli.web_server_hostname.as_deref(),
            Some(expected_host.as_str()),
            "case {case:?}: expected the host of the stored {webserver_url:?}, got {:?}",
            cli.web_server_hostname
        );
        assert_eq!(
            web.web_server_hostname.as_deref(),
            Some(expected_host.as_str()),
            "case {case:?}: expected the host of the stored {webserver_url:?}, got {:?}",
            web.web_server_hostname
        );
        let cli_token = cli
            .cli_token
            .unwrap_or_else(|| panic!("case {case:?}: CreateCliToken reported no CliToken"));
        let web_token = web
            .web_token
            .unwrap_or_else(|| panic!("case {case:?}: CreateWebLoginToken reported no WebToken"));
        assert!(
            !cli_token.is_empty(),
            "case {case:?}: expected a minted CLI token, got an empty one"
        );
        assert_ne!(
            cli_token, web_token,
            "case {case:?}: two mints are two secrets, got one value twice"
        );
    }
}
