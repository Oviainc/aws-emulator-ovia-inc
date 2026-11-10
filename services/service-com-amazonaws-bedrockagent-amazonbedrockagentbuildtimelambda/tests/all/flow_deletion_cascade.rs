// SPDX-License-Identifier: CC0-1.0

//! `DeleteFlow`, end to end over HTTP: an accepted delete takes the flow the
//! request names out of the store together with everything identified BY that
//! flow — the versions cut from it and the aliases created on it — so every
//! later read of the flow or of one of those children answers the modelled
//! `ResourceNotFoundException` (404, the error name in `x-amzn-errortype`, and
//! the message AWS words for a flow that does not exist), while a flow the
//! request did not name keeps its own versions and aliases.
//!
//! The table below spans what the rule generalises over: how many numbered
//! versions the deleted flow had been cut into, how many aliases had been
//! created on it, which `FlowIdentifier` form the request names it by, and the
//! value of `skipResourceInUseCheck` — none of which changes the outcome, since
//! the delete's response carries only the deleted flow's id.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.
//!
//! The one thing these tests reach into the store for is the INSTANT a pending
//! preparation comes due: `PrepareFlow` stores it rather than sleeping, so the
//! setup moves it into the past instead of waiting out the span.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::service_state;
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on, and the source of the region its ARNs
/// carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `FlowExecutionRoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/(service-role/)?.+$`.
const EXECUTION_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// The error every read of a deleted flow answers, and the status this service's
/// model binds to it.
///
/// Source: this service's Smithy model
/// (`smithy_query(query="operation",operation="GetFlow,ListFlowVersions,ListFlowAliases,GetFlowAlias,DeleteFlow")`),
/// where `ResourceNotFoundException` is bound with `httpError` 404.
const NOT_FOUND: (&str, u16) = ("ResourceNotFoundException", 404);

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    fn member(&self, path: &[&str]) -> &Value {
        let mut at = &self.body;
        for key in path {
            at = &at[*key];
        }
        at
    }

    fn string(&self, path: &[&str]) -> String {
        self.member(path)
            .as_str()
            .unwrap_or_else(|| panic!("{path:?} is not a string in {:?}", self.body))
            .to_string()
    }

    /// The `version` member of every `ListFlowVersions` summary, in the order
    /// the response listed them.
    fn listed_versions(&self) -> Vec<String> {
        self.member(&["flowVersionSummaries"])
            .as_array()
            .unwrap_or_else(|| panic!("flowVersionSummaries is not an array in {:?}", self.body))
            .iter()
            .map(|summary| summary["version"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// The `name` member of every `ListFlowAliases` summary, in the order the
    /// response listed them.
    fn listed_alias_names(&self) -> Vec<String> {
        self.member(&["flowAliasSummaries"])
            .as_array()
            .unwrap_or_else(|| panic!("flowAliasSummaries is not an array in {:?}", self.body))
            .iter()
            .map(|summary| summary["name"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let bytes = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bedrock-agent.eu-west-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|e| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        let error_type = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// A `flowIdentifier` as it goes into the request path: the `:` and `/` of
    /// an ARN are percent-encoded, which is how a client sends a label carrying
    /// them.
    fn escape(identifier: &str) -> String {
        identifier.replace(':', "%3A").replace('/', "%2F")
    }

    async fn create_flow(&self, case: &str, name: &str) -> Answer {
        let created = self
            .call(
                "POST",
                "/flows/",
                Some(json!({
                    "name": name,
                    "executionRoleArn": EXECUTION_ROLE,
                    "definition": passthrough_definition(),
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected CreateFlow -> 201, got {created:?}"
        );
        created
    }

    /// `PrepareFlow`, with the outcome the clock owes moved into the past, so
    /// the flow stands at `Prepared` without the test sleeping for the span.
    async fn prepare_and_settle(&self, case: &str, flow_id: &str) {
        let prepared = self
            .call("POST", &format!("/flows/{flow_id}/"), Some(json!({})))
            .await;
        assert_eq!(
            prepared.status, 202,
            "case {case}: expected PrepareFlow -> 202, got {prepared:?}"
        );
        let state = service_state(&*self.engine).expect("the bedrock-agent store");
        let mut state = state.lock();
        let due = SystemTime::now() - Duration::from_secs(1);
        let pending = state
            .flows
            .get_mut(flow_id)
            .expect("the stored flow")
            .pending_preparation
            .as_mut()
            .unwrap_or_else(|| panic!("case {case}: expected a preparation in flight"));
        pending.due = due;
        for version in state.flow_versions.values_mut() {
            if let Some((_, next)) = version.pending_status {
                version.pending_status = Some((due, next));
            }
        }
    }

    /// A prepared flow cut into `versions` numbered versions and carrying
    /// `aliases` aliases. Answers `(flow, alias ids)`.
    async fn flow_with_children(
        &self,
        case: &str,
        name: &str,
        versions: u32,
        aliases: u32,
    ) -> (Answer, Vec<String>) {
        let flow = self.create_flow(case, name).await;
        let flow_id = flow.string(&["id"]);
        self.prepare_and_settle(case, &flow_id).await;
        for expected in 1..=versions {
            let cut = self
                .call("POST", &format!("/flows/{flow_id}/versions"), Some(json!({})))
                .await;
            assert_eq!(
                cut.status, 201,
                "case {case}: expected CreateFlowVersion -> 201, got {cut:?}"
            );
            assert_eq!(
                cut.string(&["version"]),
                expected.to_string(),
                "case {case}: expected version {expected}, got {cut:?}"
            );
        }

        let mut alias_ids = Vec::new();
        for index in 1..=aliases {
            // The routing target alternates between a number the flow holds and
            // the DRAFT version every flow has, so the aliases a row creates do
            // not all point at the same version.
            let flow_version = if versions > 0 && index % 2 == 1 {
                versions.to_string()
            } else {
                "DRAFT".to_string()
            };
            // The modelled `Name` is `pattern ^([0-9a-zA-Z][_-]?){1,100}$`.
            let created = self
                .call(
                    "POST",
                    &format!("/flows/{flow_id}/aliases"),
                    Some(json!({
                        "name": format!("alias-{index}"),
                        "routingConfiguration": [{ "flowVersion": flow_version }],
                    })),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "case {case}: expected CreateFlowAlias -> 201, got {created:?}"
            );
            alias_ids.push(created.string(&["id"]));
        }
        (flow, alias_ids)
    }

    async fn get_flow(&self, identifier: &str) -> Answer {
        let escaped = Self::escape(identifier);
        self.call("GET", &format!("/flows/{escaped}/"), None).await
    }

    async fn list_flow_versions(&self, identifier: &str) -> Answer {
        let escaped = Self::escape(identifier);
        self.call("GET", &format!("/flows/{escaped}/versions"), None)
            .await
    }

    async fn list_flow_aliases(&self, identifier: &str) -> Answer {
        let escaped = Self::escape(identifier);
        self.call("GET", &format!("/flows/{escaped}/aliases"), None)
            .await
    }

    async fn get_flow_alias(&self, flow_identifier: &str, alias_identifier: &str) -> Answer {
        let flow = Self::escape(flow_identifier);
        let alias = Self::escape(alias_identifier);
        self.call("GET", &format!("/flows/{flow}/aliases/{alias}"), None)
            .await
    }

    /// `DeleteFlow`, with `skipResourceInUseCheck` carried in the query string
    /// exactly when the row supplies it (the member is `httpQuery`-bound and
    /// defaults to false).
    async fn delete_flow(&self, identifier: &str, skip_in_use_check: Option<bool>) -> Answer {
        let escaped = Self::escape(identifier);
        let query = skip_in_use_check
            .map(|skip| format!("?skipResourceInUseCheck={skip}"))
            .unwrap_or_default();
        self.call("DELETE", &format!("/flows/{escaped}/{query}"), None)
            .await
    }
}

/// A passthrough flow definition: an `Input` node wired straight to an `Output`
/// node. The node types, the union keys and the IO data types are the modelled
/// `FlowNodeType` / `FlowNodeConfiguration` / `FlowNodeIODataType` values.
fn passthrough_definition() -> Value {
    json!({
        "nodes": [
            {
                "name": "FlowInput",
                "type": "Input",
                "configuration": { "input": {} },
                "outputs": [{ "name": "document", "type": "String" }],
            },
            {
                "name": "FlowOutput",
                "type": "Output",
                "configuration": { "output": {} },
                "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
            },
        ],
        "connections": [
            {
                "name": "InputToOutput",
                "type": "Data",
                "source": "FlowInput",
                "target": "FlowOutput",
                "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
            },
        ],
    })
}

/// The refusal an operation answers when the flow it was asked to resolve does
/// not exist: the whole sentence is fixed text for that error class except the
/// operation the caller invoked and the resource type, and the identifier the
/// request named is not part of it.
fn flow_not_found(operation: &str) -> String {
    format!(
        "Couldn't perform {operation} operation. Failed to retrieve resource type Flow \
         because it doesn't exist. Check the resource ID and retry your request."
    )
}

/// Assert `answer` is the modelled not-found for a flow, worded as AWS words it
/// for `operation`.
fn assert_flow_not_found(case: &str, operation: &str, answer: &Answer) {
    let (error, status) = NOT_FOUND;
    assert_eq!(
        answer.status, status,
        "case {case}: expected {operation} -> {status}, got {answer:?}"
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some(error),
        "case {case}: expected {error} from {operation}, got {answer:?}"
    );
    assert_eq!(
        answer.string(&["message"]),
        flow_not_found(operation),
        "case {case}: expected {operation} to answer the wording for a flow that does not \
         exist, got {:?}",
        answer.member(&["message"])
    );
}

/// One row of the deletion table.
struct Row {
    case: &'static str,
    /// How many numbered versions `CreateFlowVersion` cuts before the delete.
    versions: u32,
    /// How many aliases `CreateFlowAlias` creates before the delete.
    aliases: u32,
    /// Whether the request names the flow by its ARN rather than its bare id —
    /// the two forms the modelled `FlowIdentifier` admits.
    by_arn: bool,
    /// The `skipResourceInUseCheck` the request carries, or `None` when it
    /// leaves the member to its modelled default.
    skip_in_use_check: Option<bool>,
}

/// Rows spanning what the rule generalises over: the COUNT of versions and of
/// aliases the deleted flow carried (none at all included), the identifier form
/// the request names it by, and every value `skipResourceInUseCheck` can take.
const ROWS: &[Row] = &[
    Row {
        case: "a bare flow with no versions and no aliases",
        versions: 0,
        aliases: 0,
        by_arn: false,
        skip_in_use_check: None,
    },
    Row {
        case: "one version cut, named by ARN",
        versions: 1,
        aliases: 0,
        by_arn: true,
        skip_in_use_check: Some(false),
    },
    Row {
        case: "one alias created, the in-use check skipped",
        versions: 0,
        aliases: 1,
        by_arn: false,
        skip_in_use_check: Some(true),
    },
    Row {
        case: "two versions and two aliases, named by ARN",
        versions: 2,
        aliases: 2,
        by_arn: true,
        skip_in_use_check: None,
    },
    Row {
        case: "three versions and one alias, the in-use check requested",
        versions: 3,
        aliases: 1,
        by_arn: false,
        skip_in_use_check: Some(false),
    },
];

/// The rule: a `DeleteFlow` this service accepts takes the flow out of the store
/// along with the versions cut from it and the aliases created on it, so every
/// later read of any of them answers the modelled not-found — whatever the flow
/// carried, whichever identifier form named it and whatever
/// `skipResourceInUseCheck` said — while another flow keeps its own children.
#[tokio::test]
async fn delete_flow_removes_the_flow_and_every_resource_identified_by_it() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();

        let (flow, alias_ids) = emulator
            .flow_with_children(case, "deleted-flow", row.versions, row.aliases)
            .await;
        let flow_id = flow.string(&["id"]);
        let flow_arn = flow.string(&["arn"]);
        let identifier = if row.by_arn {
            flow_arn.clone()
        } else {
            flow_id.clone()
        };

        // A bystander flow with children of its own: a delete removes what the
        // request named and nothing else.
        let (bystander, bystander_aliases) = emulator
            .flow_with_children(case, "bystander-flow", 1, 1)
            .await;
        let bystander_id = bystander.string(&["id"]);

        // Everything the row created is readable before the delete.
        assert_eq!(
            emulator.get_flow(&identifier).await.status,
            200,
            "case {case}: expected the flow to be readable before the delete"
        );
        let versions_before = emulator.list_flow_versions(&flow_id).await.listed_versions();
        assert_eq!(
            versions_before.len(),
            row.versions as usize + 1,
            "case {case}: expected the DRAFT plus {} cut version(s) before the delete, got \
             {versions_before:?}",
            row.versions
        );
        let aliases_before = emulator.list_flow_aliases(&flow_id).await.listed_alias_names();
        assert_eq!(
            aliases_before.len(),
            row.aliases as usize,
            "case {case}: expected {} alias(es) before the delete, got {aliases_before:?}",
            row.aliases
        );

        let deleted = emulator
            .delete_flow(&identifier, row.skip_in_use_check)
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected DeleteFlow -> 200, got {deleted:?}"
        );
        // `DeleteFlowResponse` models exactly one member, the flow's id, so the
        // body is that member and nothing else — and the id is the flow's own,
        // whichever identifier form the request used.
        assert_eq!(
            deleted.body,
            json!({ "id": flow_id }),
            "case {case}: expected DeleteFlow to report only the deleted flow's id, got {:?}",
            deleted.body
        );

        // The flow and everything identified by it are gone.
        assert_flow_not_found(case, "GetFlow", &emulator.get_flow(&identifier).await);
        assert_flow_not_found(
            case,
            "ListFlowVersions",
            &emulator.list_flow_versions(&identifier).await,
        );
        assert_flow_not_found(
            case,
            "ListFlowAliases",
            &emulator.list_flow_aliases(&identifier).await,
        );
        for alias_id in &alias_ids {
            assert_flow_not_found(
                case,
                "GetFlowAlias",
                &emulator.get_flow_alias(&identifier, alias_id).await,
            );
        }

        // The ARN form and the bare id name the same flow, so the other form is
        // gone too.
        let other_form = if row.by_arn { &flow_id } else { &flow_arn };
        assert_flow_not_found(case, "GetFlow", &emulator.get_flow(other_form).await);

        // `DeleteFlow` is modelled `@idempotent`, and the flow it named is no
        // longer stored: a repeat is the modelled not-found, not a second
        // success.
        assert_flow_not_found(
            case,
            "DeleteFlow",
            &emulator
                .delete_flow(&identifier, row.skip_in_use_check)
                .await,
        );

        // The flow the request did not name kept everything it held.
        let bystander_read = emulator.get_flow(&bystander_id).await;
        assert_eq!(
            bystander_read.status, 200,
            "case {case}: a delete removes the flow it named alone, got {bystander_read:?}"
        );
        assert_eq!(
            emulator
                .list_flow_versions(&bystander_id)
                .await
                .listed_versions(),
            vec!["DRAFT".to_string(), "1".to_string()],
            "case {case}: the bystander flow keeps the versions cut from it"
        );
        assert_eq!(
            emulator
                .list_flow_aliases(&bystander_id)
                .await
                .listed_alias_names(),
            vec!["alias-1".to_string()],
            "case {case}: the bystander flow keeps the aliases created on it"
        );
        for alias_id in &bystander_aliases {
            let read = emulator.get_flow_alias(&bystander_id, alias_id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: the bystander flow's alias {alias_id} is still readable, \
                 got {read:?}"
            );
        }
    }
}

/// A `flowIdentifier` of neither modelled form is a bad request, not a missing
/// flow: `DeleteFlow` refuses it with `ValidationException` (400) and deletes
/// nothing.
///
/// Rows differ in HOW the value misses the two forms the modelled
/// `FlowIdentifier` pattern admits (a flow ARN or a bare 10-character id): the
/// flow's own name, ids one character short of and one past the modelled length,
/// an id carrying a character outside the alphabet, and ARNs wrong in the
/// partition, the service, the account and the resource type.
#[tokio::test]
async fn delete_flow_refuses_an_identifier_of_neither_modelled_form() {
    let case = "an identifier of neither modelled form";
    let emulator = Emulator::new();
    let (flow, alias_ids) = emulator
        .flow_with_children(case, "well-formed-flow", 1, 1)
        .await;
    let flow_id = flow.string(&["id"]);
    let flow_arn = flow.string(&["arn"]);

    let malformed = [
        ("the flow's own name", "well-formed-flow".to_string()),
        ("nine characters", "012345678".to_string()),
        ("eleven characters", "0123456789a".to_string()),
        (
            "a character outside the id alphabet",
            "0123_56789".to_string(),
        ),
        (
            "an ARN naming another service",
            flow_arn.replace(":bedrock:", ":s3:"),
        ),
        (
            "an ARN naming another resource type",
            flow_arn.replace(":flow/", ":agent/"),
        ),
        (
            "an ARN in another partition",
            flow_arn.replace("arn:aws:", "arn:aws-cn:"),
        ),
        (
            "an ARN whose account is not twelve digits",
            flow_arn.replace(":000000000000:", ":0000:"),
        ),
        (
            "an ARN whose flow id is too short",
            flow_arn.replace(&flow_id, "012345678"),
        ),
    ];

    for (form, identifier) in malformed {
        let refused = emulator.delete_flow(&identifier, None).await;
        assert_eq!(
            refused.status, 400,
            "case {case} ({form}, {identifier}): expected DeleteFlow -> 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case} ({form}, {identifier}): expected ValidationException, got {refused:?}"
        );
    }

    // A well-formed id the store does not hold is the missing flow it is, not a
    // bad request — and it deletes nothing either.
    let unknown = "0123456789";
    assert_flow_not_found(case, "DeleteFlow", &emulator.delete_flow(unknown, None).await);

    // Every refusal above left the stored flow and its children exactly as they
    // were.
    assert_eq!(
        emulator.get_flow(&flow_id).await.status,
        200,
        "case {case}: a refused delete leaves the flow stored"
    );
    assert_eq!(
        emulator.list_flow_versions(&flow_id).await.listed_versions(),
        vec!["DRAFT".to_string(), "1".to_string()],
        "case {case}: a refused delete leaves the flow's versions as they were"
    );
    for alias_id in &alias_ids {
        let read = emulator.get_flow_alias(&flow_id, alias_id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: a refused delete leaves the flow's alias {alias_id} readable, \
             got {read:?}"
        );
    }
}

/// The flowId a delete freed is not reused as a live resource: a flow created
/// after the delete is a new flow, with its own id, and the deleted flow's id
/// still resolves to nothing.
#[tokio::test]
async fn a_flow_created_after_a_delete_is_a_new_flow() {
    let case = "create after delete";
    let emulator = Emulator::new();

    let (first, _) = emulator
        .flow_with_children(case, "recreated-flow", 2, 1)
        .await;
    let first_id = first.string(&["id"]);
    let deleted = emulator.delete_flow(&first_id, None).await;
    assert_eq!(
        deleted.status, 200,
        "case {case}: expected DeleteFlow -> 200, got {deleted:?}"
    );

    // The same name again: the model constrains `FlowName`'s shape, not its
    // uniqueness, so this is a create rather than a resurrection.
    let second = emulator.create_flow(case, "recreated-flow").await;
    let second_id = second.string(&["id"]);
    assert_ne!(
        second_id, first_id,
        "case {case}: a create after a delete mints its own flowId"
    );
    assert_flow_not_found(case, "GetFlow", &emulator.get_flow(&first_id).await);

    // The new flow starts with the DRAFT version alone and no aliases: nothing
    // the deleted flow carried came back with its name.
    assert_eq!(
        emulator.list_flow_versions(&second_id).await.listed_versions(),
        vec!["DRAFT".to_string()],
        "case {case}: the new flow holds only its own DRAFT version"
    );
    assert_eq!(
        emulator
            .list_flow_aliases(&second_id)
            .await
            .listed_alias_names(),
        Vec::<String>::new(),
        "case {case}: the new flow holds none of the deleted flow's aliases"
    );
}
