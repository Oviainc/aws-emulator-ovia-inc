// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the tools the SERVICE provides: the browser and the code
//! interpreter whose ids `BrowserId` and `CodeInterpreterId` reserve
//! (`aws.browser.v1`, `aws.codeinterpreter.v1`), which every account holds without
//! anything having created one.
//!
//! The rule under test: a read of a reserved tool id reports the tool the service
//! provides — `READY`, ARN-stamped for the region the request was signed for and with
//! the resource type that carries no `-custom` — while a DELETE, which addresses only a
//! tool this account created, does not find it and changes nothing. The same rule
//! bounds the tool: it holds none of the state a CREATE keeps for the account's own
//! tools (so its `name` is free for a created tool), and every OTHER id — any id the
//! shape admits for a created tool but nothing has created — is still the modelled 404
//! on both the read and the delete route.
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// One tool the service provides, as the routes and responses spell it.
struct Tool {
    /// The case name every assertion of this row names.
    case: &'static str,
    /// The path segment the tool's routes hang off (`smithy_query
    /// operation=GetBrowser,GetCodeInterpreter`).
    route: &'static str,
    /// The id the tool's own id shape reserves for it.
    reserved_id: &'static str,
    /// The response member carrying the tool's id.
    id_member: &'static str,
    /// The response member carrying the tool's ARN.
    arn_member: &'static str,
    /// The ARN resource type of the SERVICE-provided tool: the spelling of
    /// `browser(-custom)?` / `code-interpreter(-custom)?` without `-custom`.
    arn_resource: &'static str,
    /// The `networkMode` the tool reports: the modelled default of its own network
    /// configuration shape (`smithy_query shape=BrowserNetworkConfiguration,
    /// CodeInterpreterNetworkConfiguration`).
    network_mode: &'static str,
    /// How the tool's own not-found sentence words a missing resource.
    not_found_message: fn(&str) -> String,
    /// A `name` + `networkConfiguration` body that creates a tool of this kind, so the
    /// account's own tool can be told apart from the service's.
    create_body: fn(&str) -> Value,
}

/// The tools under test: every reserved id this service's model declares.
///
/// Source for the ids: the first alternative of `BrowserId` and of
/// `CodeInterpreterId` (`smithy_query operation=GetBrowser,GetCodeInterpreter`).
fn tools() -> [Tool; 2] {
    [
        Tool {
            case: "the service-provided browser",
            route: "browsers",
            reserved_id: "aws.browser.v1",
            id_member: "browserId",
            arn_member: "browserArn",
            arn_resource: "browser",
            network_mode: "PUBLIC",
            not_found_message: |id| format!("Browser with ID {id} does not exist"),
            create_body: |name| {
                json!({"name": name, "networkConfiguration": {"networkMode": "PUBLIC"}})
            },
        },
        Tool {
            case: "the service-provided code interpreter",
            route: "code-interpreters",
            reserved_id: "aws.codeinterpreter.v1",
            id_member: "codeInterpreterId",
            arn_member: "codeInterpreterArn",
            arn_resource: "code-interpreter",
            network_mode: "SANDBOX",
            not_found_message: |id| format!("Code interpreter with ID {id} does not exist"),
            create_body: |name| {
                json!({"name": name, "networkConfiguration": {"networkMode": "SANDBOX"}})
            },
        },
    ]
}

/// The regions the reads are signed for, so the region the ARN carries is the request's
/// own rather than one baked into the handler. One is the emulator's default
/// (`us-east-1`), the others are not.
const REGIONS: [&str; 3] = ["us-east-1", "eu-west-2", "ap-northeast-3"];

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`. It is what the router
/// routes on, and what the handlers read the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(region))
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `GetBrowser` / `GetCodeInterpreter`: `GET /{route}/{id}`.
async fn get_tool(
    router: &MultiServiceRouter,
    tool: &Tool,
    id: &str,
    region: &str,
) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!("/{}/{id}", tool.route),
        region,
        None,
    )
    .await
}

/// `DeleteBrowser` / `DeleteCodeInterpreter`: `DELETE /{route}/{id}`.
async fn delete_tool(
    router: &MultiServiceRouter,
    tool: &Tool,
    id: &str,
    region: &str,
) -> (u16, Value) {
    send(
        router,
        "DELETE",
        &format!("/{}/{id}", tool.route),
        region,
        None,
    )
    .await
}

/// `CreateBrowser` / `CreateCodeInterpreter`: `PUT /{route}`.
async fn create_tool(
    router: &MultiServiceRouter,
    tool: &Tool,
    body: &Value,
    region: &str,
) -> (u16, Value) {
    send(router, "PUT", &format!("/{}", tool.route), region, Some(body)).await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `SandboxName`'s pattern `^[a-zA-Z][a-zA-Z0-9_]{0,47}$` (`smithy_query
/// operation=GetBrowser`), checked by hand because this crate carries no regex engine.
fn is_modelled_sandbox_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && name.len() <= 48
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Assert one `DateTimestamp` member's wire form: a JSON STRING in the canonical RFC
/// 3339 form, naming a real instant.
fn assert_date_time(case: &str, member: &str, value: &Value) {
    let rendered = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an RFC 3339 date-time string, got {value:?}")
    });
    let parsed = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} must parse as a date-time ({error}), got {rendered:?}")
    });
    assert!(
        parsed.secs() > 0,
        "case {case:?}: {member} must name a real instant, got {rendered:?}"
    );
}

/// Every tool the service provides is readable by its reserved id in every region,
/// without anything having created one, and two reads report the same tool.
#[tokio::test]
async fn a_system_tool_is_readable_by_its_reserved_id_in_every_region() {
    let router = router();

    for tool in tools() {
        for region in REGIONS {
            let case = format!("{} in {region}", tool.case);
            let (status, read) = get_tool(&router, &tool, tool.reserved_id, region).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a reserved tool id is readable with nothing having created \
                 it, got {status} ({read})"
            );
            assert_eq!(
                text(&read, tool.id_member),
                tool.reserved_id,
                "case {case:?}: the read reports the reserved id it was asked for, got {}",
                text(&read, tool.id_member)
            );
            assert_eq!(
                text(&read, tool.arn_member),
                format!(
                    "arn:aws:bedrock-agentcore:{region}:{ACCOUNT}:{}/{}",
                    tool.arn_resource, tool.reserved_id
                ),
                "case {case:?}: expected the service-provided ARN for {}, got {}",
                tool.reserved_id,
                text(&read, tool.arn_member)
            );
            assert_eq!(
                text(&read, "status"),
                "READY",
                "case {case:?}: a tool nobody created is READY, got {}",
                text(&read, "status")
            );
            assert!(
                is_modelled_sandbox_name(&text(&read, "name")),
                "case {case:?}: the name must satisfy SandboxName, got {:?}",
                text(&read, "name")
            );
            assert_eq!(
                read["networkConfiguration"],
                json!({"networkMode": tool.network_mode}),
                "case {case:?}: expected the modelled default network mode {}, got {}",
                tool.network_mode,
                read["networkConfiguration"]
            );
            assert!(
                read["executionRoleArn"].is_null(),
                "case {case:?}: no caller supplied an execution role for a service-provided \
                 tool, got {}",
                read["executionRoleArn"]
            );
            assert!(
                read["failureReason"].is_null(),
                "case {case:?}: a READY tool reports no failureReason, got {}",
                read["failureReason"]
            );
            for member in ["createdAt", "lastUpdatedAt"] {
                assert_date_time(&case, member, &read[member]);
            }

            // A read neither creates the tool nor re-dates it: the next read of the same
            // id reports exactly the same resource.
            let (status, again) = get_tool(&router, &tool, tool.reserved_id, region).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the second read also answers HTTP 200, got {status} ({again})"
            );
            assert_eq!(
                again, read,
                "case {case:?}: a read does not change what a later read sees, got {again} \
                 against {read}"
            );
        }
    }
}

/// A delete addresses only a tool this account created, so one naming a reserved id is
/// the modelled 404 — whether sent once or repeated — and the tool is still READY after.
#[tokio::test]
async fn a_system_tool_is_not_deletable_and_the_refused_delete_changes_nothing() {
    let router = router();
    let region = REGIONS[0];

    for tool in tools() {
        let (_, before) = get_tool(&router, &tool, tool.reserved_id, region).await;

        for attempt in 1..=2 {
            let case = format!("{} (delete {attempt})", tool.case);
            let (status, refused) = delete_tool(&router, &tool, tool.reserved_id, region).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a delete of a reserved tool id is the modelled 404, got \
                 {status} ({refused})"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {} ({refused})",
                error_code(&refused)
            );
            assert_eq!(
                text(&refused, "message"),
                (tool.not_found_message)(tool.reserved_id),
                "case {case:?}: the refusal names the id the request carried, got {}",
                text(&refused, "message")
            );
        }

        let (status, after) = get_tool(&router, &tool, tool.reserved_id, region).await;
        assert_eq!(
            status, 200,
            "case {:?}: the tool is still readable after the refused delete, got {status} \
             ({after})",
            tool.case
        );
        assert_eq!(
            after, before,
            "case {:?}: a refused delete changes nothing about the tool, got {after} against \
             {before}",
            tool.case
        );
    }
}

/// A tool the ACCOUNT creates is its own resource beside the service-provided one: its
/// id is minted, its ARN carries the `-custom` resource type, and deleting it leaves the
/// service-provided tool exactly where it was.
#[tokio::test]
async fn a_created_tool_is_its_own_resource_beside_the_system_one() {
    let router = router();
    let region = REGIONS[1];

    for tool in tools() {
        let case = tool.case;
        let (status, created) =
            create_tool(&router, &tool, &(tool.create_body)("ownTool"), region).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from the create, got {status} ({created})"
        );
        let created_id = text(&created, tool.id_member);
        assert_ne!(
            created_id, tool.reserved_id,
            "case {case:?}: a create never mints the reserved id, got {created_id:?}"
        );
        assert_eq!(
            text(&created, tool.arn_member),
            format!(
                "arn:aws:bedrock-agentcore:{region}:{ACCOUNT}:{}-custom/{created_id}",
                tool.arn_resource
            ),
            "case {case:?}: a created tool carries the -custom resource type, got {}",
            text(&created, tool.arn_member)
        );

        // The created tool is deletable; the service-provided one is not, and the delete
        // of the account's own tool does not disturb it.
        let (status, deleted) = delete_tool(&router, &tool, &created_id, region).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a tool this account created is deletable, got {status} ({deleted})"
        );

        let (status, system) = get_tool(&router, &tool, tool.reserved_id, region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the service-provided tool is unaffected, got {status} ({system})"
        );
        assert_eq!(
            text(&system, "status"),
            "READY",
            "case {case:?}: the service-provided tool is still READY, got {}",
            text(&system, "status")
        );
    }
}

/// Ids of the form a CREATE mints — the second alternative of `BrowserId` /
/// `CodeInterpreterId`, `[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}` (`smithy_query
/// operation=GetBrowser,GetCodeInterpreter`) — that name no tool this account created.
///
/// One row per way the pattern's created alternative can be spelled: the shortest name
/// part, the longest, a name carrying the underscores it admits, and suffixes of digits,
/// of letters and of both.
fn ids_naming_no_created_tool() -> Vec<(String, String)> {
    vec![
        (
            "the shortest id the created alternative admits".to_string(),
            "a-0123456789".to_string(),
        ),
        (
            "an id whose name part is the longest the pattern admits".to_string(),
            format!("z{}-abcdefghij", "_".repeat(47)),
        ),
        (
            "an id whose name part carries underscores".to_string(),
            "tool_Name-ABCDEFGHIJ".to_string(),
        ),
        (
            "an id whose suffix mixes letters and digits".to_string(),
            "Tool9-a1b2C3d4e5".to_string(),
        ),
    ]
}

/// Only the RESERVED id resolves without a create: every other id the shape admits names
/// nothing, so both the read and the delete route answer the modelled 404 naming the id
/// the request carried, and the service-provided tool is still readable afterwards.
#[tokio::test]
async fn an_id_that_names_no_created_tool_is_the_modelled_404_on_read_and_on_delete() {
    let router = router();
    let region = REGIONS[2];

    for tool in tools() {
        for (form, id) in ids_naming_no_created_tool() {
            assert_ne!(
                id, tool.reserved_id,
                "case {form:?}: this row must not be the reserved id"
            );
            let answers = [
                ("read", get_tool(&router, &tool, &id, region).await),
                ("delete", delete_tool(&router, &tool, &id, region).await),
            ];
            for (verb, (status, refused)) in answers {
                let case = format!("{form} on the {} route ({verb})", tool.route);
                assert_eq!(
                    status, 404,
                    "case {case:?}: an id naming no created tool is the modelled 404, got \
                     {status} ({refused})"
                );
                assert_eq!(
                    error_code(&refused),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {} ({refused})",
                    error_code(&refused)
                );
                assert_eq!(
                    text(&refused, "message"),
                    (tool.not_found_message)(&id),
                    "case {case:?}: the refusal names the id the request carried, got {}",
                    text(&refused, "message")
                );
            }
        }

        // None of those misses disturbed the tool the service provides.
        let case = tool.case;
        let (status, system) = get_tool(&router, &tool, tool.reserved_id, region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the reserved id still resolves after ids that name nothing, got \
             {status} ({system})"
        );
        assert_eq!(
            text(&system, "status"),
            "READY",
            "case {case:?}: the service-provided tool is still READY, got {}",
            text(&system, "status")
        );
    }
}

/// The tool the service provides holds none of the state a CREATE keeps for the account's
/// own tools: the `name` it reports is free for a created tool (the uniqueness index
/// "the name must be unique within your account" holds only created names), the created
/// tool is a second, distinct resource readable by its minted id, and a repeat of THAT
/// name is the conflict.
#[tokio::test]
async fn the_service_provided_tool_does_not_occupy_the_created_tool_name_index() {
    let router = router();
    let region = REGIONS[0];

    for tool in tools() {
        let case = tool.case;
        let (status, system) = get_tool(&router, &tool, tool.reserved_id, region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the reserved id resolves before anything is created, got {status} \
             ({system})"
        );
        let system_name = text(&system, "name");

        let body = (tool.create_body)(&system_name);
        let (status, created) = create_tool(&router, &tool, &body, region).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the name the service-provided tool reports is free for a created \
             tool, got {status} ({created})"
        );
        let created_id = text(&created, tool.id_member);
        assert_ne!(
            created_id, tool.reserved_id,
            "case {case:?}: a create never mints the reserved id, got {created_id:?}"
        );

        let (status, read_back) = get_tool(&router, &tool, &created_id, region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the created tool is readable by its minted id, got {status} \
             ({read_back})"
        );
        assert_eq!(
            text(&read_back, "name"),
            system_name,
            "case {case:?}: the created tool keeps the name the create supplied, got {}",
            text(&read_back, "name")
        );
        assert_eq!(
            text(&read_back, tool.arn_member),
            format!(
                "arn:aws:bedrock-agentcore:{region}:{ACCOUNT}:{}-custom/{created_id}",
                tool.arn_resource
            ),
            "case {case:?}: the created tool carries the -custom resource type, got {}",
            text(&read_back, tool.arn_member)
        );

        // That created name IS in the index now, so a second create of it conflicts —
        // which is how the first create proves the service-provided tool was never there.
        let (status, conflict) = create_tool(&router, &tool, &body, region).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a repeated created name is the modelled conflict, got {status} \
             ({conflict})"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {} ({conflict})",
            error_code(&conflict)
        );

        let (status, after) = get_tool(&router, &tool, tool.reserved_id, region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the service-provided tool is unaffected, got {status} ({after})"
        );
        assert_eq!(
            after, system,
            "case {case:?}: a create sharing its name changes nothing about the \
             service-provided tool, got {after} against {system}"
        );
    }
}

/// The reserved id of one tool is not an id of the OTHER tool's route: each id shape
/// reserves its own, so the boundary constraint walk refuses the swap.
#[tokio::test]
async fn one_tools_reserved_id_is_not_an_id_of_the_other_tools_route() {
    let router = router();
    let region = REGIONS[0];
    let [browser, code_interpreter] = tools();

    // (case, the route addressed, the id sent)
    let rows = [
        (
            "the interpreter's reserved id on the browser route",
            &browser,
            code_interpreter.reserved_id,
        ),
        (
            "the browser's reserved id on the interpreter route",
            &code_interpreter,
            browser.reserved_id,
        ),
    ];

    for (case, tool, id) in rows {
        let (status, refused) = get_tool(&router, tool, id, region).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an id the shape's pattern refuses is a 400, got {status} ({refused})"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {} ({refused})",
            error_code(&refused)
        );
    }
}
