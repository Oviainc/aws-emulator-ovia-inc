// SPDX-License-Identifier: CC0-1.0

//! Which ERROR SHAPE a handler of this service refuses a request with.
//!
//! The rule under test: when a handler of this service refuses a request for a rule the
//! model cannot express, the error value it returns is the client-input shape THIS
//! service's model declares — `com.amazonaws.supportapp#ValidationException`
//! (`@error client`, `@httpError 400`,
//! `smithy_query(query="shape",shape="ValidationException")`) — and never a shape from
//! outside the model that merely renders the same status and code.
//!
//! The shape identity is what the rule is about, and the wire form of a restJson1 error
//! carries only the status, the `ValidationException` code and `message`, so each row is
//! asserted twice: once on the value the HANDLER returns, called directly with a typed
//! input, where the identity is readable off the schema the error channel renders with
//! (`OperationError::variant_schema`) and would carry any member the modelled shape
//! grew; and once over the wire through the production router, which pins the status, the
//! code and the wording a client observes.
//!
//! The rows span the operations and the handler-enforced rules this service has, taking
//! in both of the ways the crate used to raise this refusal: the `nextToken` rule of
//! EACH listing, and the documented `notifyOnCaseSeverity` / notification-flag rule of
//! `CreateSlackChannelConfiguration`. Within each, the rows differ along what that rule
//! generalises over — the form and length of a token no listing ever minted, and the
//! severity level together with which flags the request carries. The boundary of each
//! rule is a row too: the request each operation SERVES is not refused.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_supportapp_supportapp as supportapp;
use supportapp::ops::create_slack_channel_configuration::{
    CreateSlackChannelConfiguration, CreateSlackChannelConfigurationError,
    CreateSlackChannelConfigurationErrorModeled,
};
use supportapp::ops::list_slack_channel_configurations::{
    ListSlackChannelConfigurations, ListSlackChannelConfigurationsError,
    ListSlackChannelConfigurationsErrorModeled,
};
use supportapp::ops::list_slack_workspace_configurations::{
    ListSlackWorkspaceConfigurations, ListSlackWorkspaceConfigurationsError,
    ListSlackWorkspaceConfigurationsErrorModeled,
};
use supportapp::ops::types::{
    CreateSlackChannelConfigurationInput, CreateSlackChannelConfigurationOutput,
    ListSlackChannelConfigurationsInput, ListSlackChannelConfigurationsOutput,
    ListSlackWorkspaceConfigurationsInput, ListSlackWorkspaceConfigurationsOutput,
    ValidationException,
};
use supportapp::scaffolded::Scaffolded;

use crate::error_message_templates::refusal;
use crate::slack_channel_configurations::{role_arn, Emulator, Reply};
use crate::slack_workspace_configurations::list_workspaces;

/// The Region every request below is signed for; this API's resources are not regional.
const REGION: &str = "us-east-1";

/// The shape id this SERVICE declares for a request whose input it refuses, which every
/// handler-raised request-validation refusal has to carry.
const MODELLED_VALIDATION_SHAPE: &str = "com.amazonaws.supportapp#ValidationException";

/// The wire error code that shape renders with on this service's restJson1 protocol.
const VALIDATION_CODE: &str = "ValidationException";

/// `nextToken` is `len 1..256, pattern ^\S+$` on both listing requests
/// (`smithy_query(query="operation",operation="ListSlackChannelConfigurations,ListSlackWorkspaceConfigurations")`),
/// so a 256-character token is the published upper bound rather than a value past it —
/// and one row sends exactly that, so the refusal is the handler's and not the boundary
/// validator's.
const NEXT_TOKEN_MAX_LEN: usize = 256;

/// The Slack ids the create rows configure. The channel configuration is never stored —
/// every create row below is refused before the store is reached.
const TEAM_ID: &str = "T012ABCDEFG";
const CHANNEL_ID: &str = "C01234A5BCD";

/// The handler leg: this service on its own engine, plus the request metadata and the
/// in-process callback a handler is given, so an operation can be called with a typed
/// input and its error inspected as a value.
struct Handlers {
    engine: Arc<engine::InMemoryEngine>,
    service: supportapp::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Handlers {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(supportapp::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: supportapp::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    async fn list_channels(
        &self,
        next_token: Option<&str>,
    ) -> Result<ListSlackChannelConfigurationsOutput, ListSlackChannelConfigurationsError> {
        ListSlackChannelConfigurations::new(Arc::clone(&self.engine))
            .handle(
                ListSlackChannelConfigurationsInput {
                    next_token: next_token.map(str::to_string),
                },
                &self.ctx(),
            )
            .await
    }

    async fn list_workspaces(
        &self,
        next_token: Option<&str>,
    ) -> Result<ListSlackWorkspaceConfigurationsOutput, ListSlackWorkspaceConfigurationsError> {
        ListSlackWorkspaceConfigurations::new(Arc::clone(&self.engine))
            .handle(
                ListSlackWorkspaceConfigurationsInput {
                    next_token: next_token.map(str::to_string),
                },
                &self.ctx(),
            )
            .await
    }

    async fn create(
        &self,
        input: CreateSlackChannelConfigurationInput,
    ) -> Result<CreateSlackChannelConfigurationOutput, CreateSlackChannelConfigurationError> {
        CreateSlackChannelConfiguration::new(Arc::clone(&self.engine))
            .handle(input, &self.ctx())
            .await
    }
}

/// Request metadata naming this service's signing name and a Region, which is how a
/// request reaches a handler through the framework.
fn signed_request_parts() -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/control/list-slack-channel-configurations")
        .header("host", format!("supportapp.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/supportapp/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// The identity of the refusal a handler returned: the shape the error channel renders
/// it with, the status and the wire code.
///
/// Generic over the operation's error type, because each operation has its own error
/// enum — which is exactly why the shape they all have to carry is asserted here once
/// instead of per operation.
fn assert_modelled_shape<E: OperationError>(case: &str, error: &E) {
    let shape = error.variant_schema().shape_id().to_string();
    assert_eq!(
        shape, MODELLED_VALIDATION_SHAPE,
        "case {case:?}: expected the shape this service models, {MODELLED_VALIDATION_SHAPE}, \
         got {shape:?}"
    );
    assert_eq!(
        error.http_status(),
        400,
        "case {case:?}: expected HTTP 400, got {}",
        error.http_status()
    );
    assert_eq!(
        error.shape_name(),
        VALIDATION_CODE,
        "case {case:?}: expected the wire code {VALIDATION_CODE}, got {:?}",
        error.shape_name()
    );
}

/// The envelope every handler-raised request-validation message of this service is
/// worded in, plus the `message` it carried.
fn assert_envelope(case: &str, operation: &str, exception: &ValidationException) -> String {
    let message = exception.message.clone().unwrap_or_else(|| {
        panic!("case {case:?}: the modelled refusal carries no message: {exception:?}")
    });
    let opening = format!("Your {operation} request isn't valid. ");
    assert!(
        message.starts_with(&opening),
        "case {case:?}: expected the message to open with {opening:?}, got {message:?}"
    );
    assert!(
        message.ends_with("Try your request again."),
        "case {case:?}: expected the message to close with the retry sentence, got {message:?}"
    );
    message
}

/// The wire answer carries the refusal's `message` and nothing else: the modelled shape
/// declares the one member, so a client reads the same body it always did.
fn assert_only_message(case: &str, reply: &Reply) {
    let members: Vec<String> = match &reply.body {
        Value::Object(map) => map
            .keys()
            .filter(|name| name.as_str() != "__type")
            .cloned()
            .collect(),
        other => panic!("case {case:?}: expected a JSON object body, got {other}"),
    };
    assert_eq!(
        members,
        vec!["message".to_string()],
        "case {case:?}: expected the single modelled member, got {members:?}"
    );
}

/// `nextToken` values no response of either listing ever handed out, as `(case, token)`.
///
/// Neither listing mints a token, so the rows differ along the FORM and LENGTH a forged
/// one can take; every row satisfies `nextToken`'s own `@length` / `@pattern`, so the
/// wire leg reaches the handler rather than the boundary validator.
fn forged_tokens() -> Vec<(&'static str, String)> {
    vec![
        ("text that is not hex at all", "not-a-token".to_string()),
        ("hex digits of odd length", "abc".to_string()),
        (
            "hex of bytes that are not text",
            hex::encode([0xff_u8, 0xfe_u8]),
        ),
        ("hex of a team id", hex::encode(TEAM_ID)),
        ("hex of a page position", hex::encode("2")),
        ("an id sent unencoded", TEAM_ID.to_string()),
        (
            "a token at the member's published length bound",
            "t".repeat(NEXT_TOKEN_MAX_LEN),
        ),
    ]
}

/// Which notification flags a create row carries, as the three members in the order the
/// request declares them (`notifyOnCreateOrReopenCase`,
/// `notifyOnAddCorrespondenceToCase`, `notifyOnResolveCase`); `None` is a member the
/// request leaves out.
type Flags = [Option<bool>; 3];

/// Severity/flag combinations the documented rule on `notifyOnCaseSeverity` refuses, as
/// `(case, severity, flags)`: with `all` or `high` at least one flag must be true, and
/// with `none` the flags the request specifies must be false.
fn severity_violations() -> Vec<(&'static str, &'static str, Flags)> {
    vec![
        ("all with no flag specified", "all", [None, None, None]),
        (
            "high with every flag false",
            "high",
            [Some(false), Some(false), Some(false)],
        ),
        (
            "high with no flag specified",
            "high",
            [None, None, None],
        ),
        (
            "none with the resolve flag true",
            "none",
            [None, None, Some(true)],
        ),
        (
            "none with the create-or-reopen flag true",
            "none",
            [Some(true), None, None],
        ),
        (
            "none with every flag true",
            "none",
            [Some(true), Some(true), Some(true)],
        ),
    ]
}

/// The create request a row sends, as a typed input.
fn create_input(severity: &str, flags: Flags) -> CreateSlackChannelConfigurationInput {
    CreateSlackChannelConfigurationInput {
        team_id: TEAM_ID.to_string(),
        channel_id: CHANNEL_ID.to_string(),
        channel_name: None,
        notify_on_create_or_reopen_case: flags[0],
        notify_on_add_correspondence_to_case: flags[1],
        notify_on_resolve_case: flags[2],
        notify_on_case_severity: severity.to_string(),
        channel_role_arn: role_arn("SupportAppRole"),
    }
}

/// The same create request as the JSON body a client sends, so the handler leg and the
/// wire leg of a row are the same request.
fn create_body(severity: &str, flags: Flags) -> Value {
    let mut body = json!({
        "teamId": TEAM_ID,
        "channelId": CHANNEL_ID,
        "notifyOnCaseSeverity": severity,
        "channelRoleArn": role_arn("SupportAppRole"),
    });
    for (member, value) in [
        ("notifyOnCreateOrReopenCase", flags[0]),
        ("notifyOnAddCorrespondenceToCase", flags[1]),
        ("notifyOnResolveCase", flags[2]),
    ] {
        if let Some(value) = value {
            body[member] = json!(value);
        }
    }
    body
}

#[tokio::test]
async fn every_handler_refusal_carries_the_validation_exception_this_service_models() {
    let handlers = Handlers::new();
    let emulator = Emulator::new();

    for (form, token) in forged_tokens() {
        // The channel listing's `nextToken` rule.
        let case = format!("ListSlackChannelConfigurations / {form}");
        let refused = handlers
            .list_channels(Some(&token))
            .await
            .expect_err(&format!(
                "case {case:?}: a token no listing of this service minted must be refused"
            ));
        assert_modelled_shape(&case, &refused);
        let message = match refused {
            Scaffolded::Modeled(ListSlackChannelConfigurationsErrorModeled::ValidationException(
                exception,
            )) => assert_envelope(&case, "ListSlackChannelConfigurations", &exception),
            other => panic!(
                "case {case:?}: expected the modelled ValidationException, got {other:?}"
            ),
        };
        let reply = emulator.list(Some(&token)).await;
        let wire = refusal(&case, &reply, 400, VALIDATION_CODE);
        assert_eq!(
            wire, message,
            "case {case:?}: expected the wire message to be the handler's, got {wire:?}"
        );
        assert_only_message(&case, &reply);

        // The workspace listing's `nextToken` rule — the same rule, the same shape.
        let case = format!("ListSlackWorkspaceConfigurations / {form}");
        let refused = handlers
            .list_workspaces(Some(&token))
            .await
            .expect_err(&format!(
                "case {case:?}: a token no listing of this service minted must be refused"
            ));
        assert_modelled_shape(&case, &refused);
        let message = match refused {
            Scaffolded::Modeled(
                ListSlackWorkspaceConfigurationsErrorModeled::ValidationException(exception),
            ) => assert_envelope(&case, "ListSlackWorkspaceConfigurations", &exception),
            other => panic!(
                "case {case:?}: expected the modelled ValidationException, got {other:?}"
            ),
        };
        let reply = list_workspaces(&emulator, Some(&token)).await;
        let wire = refusal(&case, &reply, 400, VALIDATION_CODE);
        assert_eq!(
            wire, message,
            "case {case:?}: expected the wire message to be the handler's, got {wire:?}"
        );
        assert_only_message(&case, &reply);
    }

    for (combination, severity, flags) in severity_violations() {
        let case = format!("CreateSlackChannelConfiguration / {combination}");
        let refused = handlers
            .create(create_input(severity, flags))
            .await
            .expect_err(&format!(
                "case {case:?}: a request against the documented severity rule must be refused"
            ));
        assert_modelled_shape(&case, &refused);
        let message = match refused {
            Scaffolded::Modeled(
                CreateSlackChannelConfigurationErrorModeled::ValidationException(exception),
            ) => assert_envelope(&case, "CreateSlackChannelConfiguration", &exception),
            other => panic!(
                "case {case:?}: expected the modelled ValidationException, got {other:?}"
            ),
        };
        let reply = emulator.create(create_body(severity, flags)).await;
        let wire = refusal(&case, &reply, 400, VALIDATION_CODE);
        assert_eq!(
            wire, message,
            "case {case:?}: expected the wire message to be the handler's, got {wire:?}"
        );
        assert_only_message(&case, &reply);
    }

    // The boundary of each rule: the request the operation SERVES is not refused at all.
    // Both listings answer without a token, and a severity level the flags hold together
    // with is created.
    let served = emulator.list(None).await;
    assert_eq!(
        served.status, 200,
        "case \"the channel listing without a token\": expected HTTP 200, got {} / {}",
        served.status, served.body
    );
    let served = list_workspaces(&emulator, None).await;
    assert_eq!(
        served.status, 200,
        "case \"the workspace listing without a token\": expected HTTP 200, got {} / {}",
        served.status, served.body
    );
    let created = emulator
        .create_in_registered_workspace(create_body("high", [Some(true), None, None]))
        .await;
    assert_eq!(
        created.status, 200,
        "case \"high with one flag true\": expected HTTP 200, got {} / {}",
        created.status, created.body
    );
}
